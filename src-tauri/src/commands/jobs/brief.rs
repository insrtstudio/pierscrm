//! What can be known about one posting, condensed.
//!
//! Two passes, both read-only on the web:
//! 1. the posting itself: is it one of the user's roles, how well does it
//!    fit, one line that says it all, the terms a screening tool will look
//!    for, how to apply and who to write to (only when the posting says so);
//! 2. the company: what it does, and for a label, publisher, agency or
//!    management firm the artists it puts forward, read on its own site.
//!
//! The AI summarises pages it is given. Anything checkable is checked by
//! code: an address must be in the text read and its domain must receive
//! mail; a roster name must be on the company's page.

use rusqlite::{params, Connection};
use serde_json::{json, Value};

use super::cv::Profile;
use super::sources::Http;
use crate::commands::email_finder::find_emails;
use crate::commands::enrich;
use crate::commands::residency::claude::{extract_json, Llm, LlmRequest, SystemBlock, Usage, DEFAULT_MODEL};

/// The profile as a short text for the analysis (no contact details).
pub fn profile_digest(p: &Profile) -> String {
    let mut s = String::new();
    s.push_str(&format!("Summary: {}\n", p.summary.get("en")));
    s.push_str(&format!("Based in: {}\nLanguages: {}\nRight to work: {}\n", p.identity.location.get("en"), p.identity.languages.get("en"), p.work_rights));
    s.push_str("Figures: ");
    s.push_str(&p.figures.iter().map(|f| format!("{} {}", f.value, f.label.get("en"))).collect::<Vec<_>>().join("; "));
    s.push_str("\nExperience:\n");
    for e in &p.experience {
        s.push_str(&format!("- {} | {} | {}\n", e.titles.first().map(|t| t.get("en")).unwrap_or(""), e.org, e.period.get("en")));
        for b in &e.bullets {
            s.push_str(&format!("  · {}\n", b.get("en")));
        }
    }
    s.push_str("Skills: ");
    s.push_str(&p.skills.iter().map(|k| format!("{} ({})", k.name.get("en"), k.items.get("en"))).collect::<Vec<_>>().join("; "));
    if !p.placements.is_empty() {
        s.push_str(&format!("\nSync placements: {} listed, e.g. {}", p.placements.len(),
            p.placements.iter().take(8).map(|x| format!("{} ({})", x.programme, x.broadcaster)).collect::<Vec<_>>().join(", ")));
    }
    if !p.facts.is_empty() {
        s.push_str(&format!("\nOther facts: {}", p.facts.join("; ")));
    }
    s
}

fn analysis_schema() -> Value {
    let s = json!({ "type": "string" });
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["is_posting", "domain", "score", "line", "lang", "contract", "salary", "seniority", "deadline", "work_right",
            "strengths", "gaps", "keywords", "apply_email", "apply_url", "contact_name", "contact_role"],
        "properties": {
            "is_posting": { "type": "boolean" },
            "domain": { "type": "string", "enum": ["sync", "label", "booking", "other"] },
            "score": { "type": "integer" },
            "line": s,
            "lang": { "type": "string", "enum": ["fr", "en"] },
            "contract": s, "salary": s, "seniority": s, "deadline": s,
            "work_right": s,
            "strengths": { "type": "array", "items": s },
            "gaps": { "type": "array", "items": s },
            "keywords": { "type": "array", "items": s },
            "apply_email": s, "apply_url": s,
            "contact_name": s, "contact_role": s
        }
    })
}

pub fn analysis_request(p: &Profile, title: &str, company: &str, location: &str, text: &str) -> LlmRequest {
    let rules = "You screen job postings for one candidate in the music industry (sync and publishing, label and artist \
management, booking and agencies) looking in France, the UK and the USA. You are given the candidate's profile and the \
text of a page.\n\
- is_posting: false when the page is not one open position (a list of jobs, an article, a course, a closed role).\n\
- domain: sync, label, booking, or other when the role is outside those three.\n\
- score: 0 to 100, how likely this candidate is to be shortlisted. Judge the must-haves against the profile honestly. \
A role in a domain the candidate has never worked in scores under 40. A required right to work the candidate lacks \
costs 15 points unless the posting offers sponsorship.\n\
- line: ONE line in French, at most 230 characters, that condenses the role for a tracking table: what the job really is, \
level, contract, pay if stated, the one requirement that matters, and the catch if there is one. No company name, no \
place (they have their own columns). No em dash.\n\
- lang: language of the posting.\n\
- contract, salary, seniority, deadline: as stated, empty string when not stated. Never estimate pay.\n\
- work_right: one short French sentence on the right to work for this candidate in that country, empty if no issue.\n\
- strengths: up to 4 short French points where the profile answers the posting. gaps: up to 4 short French points the \
posting asks for that the profile does not show.\n\
- keywords: 8 to 14 terms (skills, tools, duties, in the posting's language, 1 to 3 words each) that a screening tool \
would look for in a CV for this role.\n\
- apply_email: an address the posting gives for applications, copied exactly, else empty. apply_url: the application link \
given in the text, else empty. contact_name and contact_role: the person named in the posting as recruiter or hiring \
manager, copied exactly, else empty. Never invent a person or an address.\n\
Answer with JSON only.";
    let body: String = text.chars().take(9000).collect();
    LlmRequest {
        model: DEFAULT_MODEL.to_string(),
        max_tokens: 2200,
        system: vec![
            SystemBlock { text: rules.to_string(), cache: false },
            SystemBlock { text: format!("CANDIDATE PROFILE\n{}", profile_digest(p)), cache: true },
        ],
        user: format!("Title: {title}\nCompany: {company}\nPlace: {location}\n\nPAGE TEXT\n{body}"),
        schema: Some(analysis_schema()),
    }
}

fn s(v: &Value, k: &str) -> Option<String> {
    v[k].as_str().map(|x| crate::commands::residency::claude::no_dashes(x).trim().to_string()).filter(|x| !x.is_empty())
}

fn list(v: &Value, k: &str, max: usize) -> Vec<String> {
    v[k].as_array()
        .map(|a| a.iter().filter_map(|x| x.as_str()).map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).take(max).collect())
        .unwrap_or_default()
}

/// An address is kept only when it stands in the text that was read and its
/// domain receives mail.
pub fn verified_email(candidate: Option<String>, text: &str, mx: &mut dyn FnMut(&str) -> Option<bool>) -> Option<String> {
    let e = enrich::clean_email(&candidate?)?;
    if !text.to_lowercase().contains(&e) || enrich::is_placeholder_email(&e) {
        return None;
    }
    (mx(&enrich::email_domain(&e)) == Some(true)).then_some(e)
}

/// Store the analysis of a posting. Returns the score, or None when the page
/// is not a posting (the line is then dismissed).
pub fn store_analysis(conn: &Connection, id: i64, v: &Value, text: &str, mx: &mut dyn FnMut(&str) -> Option<bool>) -> Option<i64> {
    if v["is_posting"].as_bool() == Some(false) {
        let _ = conn.execute(
            "UPDATE job_posts SET status = 'dismissed', analysed_at = datetime('now'), notes = 'Écartée : la page n''est pas une offre ouverte' WHERE id = ?1",
            params![id],
        );
        return None;
    }
    let score = v["score"].as_i64().unwrap_or(0).clamp(0, 100);
    let domain = v["domain"].as_str().unwrap_or("other");
    let fit = json!({
        "strengths": list(v, "strengths", 4),
        "gaps": list(v, "gaps", 4),
        "keywords": list(v, "keywords", 14),
        "work_right": s(v, "work_right"),
        "seniority": s(v, "seniority"),
        "deadline": s(v, "deadline"),
    });
    let email = verified_email(s(v, "apply_email"), text, mx);
    let apply_url = s(v, "apply_url").filter(|u| u.starts_with("http") && text.contains(u.as_str()));
    // A name is only kept when it stands in the page.
    let contact_name = s(v, "contact_name").filter(|n| text.to_lowercase().contains(&n.to_lowercase()));
    let _ = conn.execute(
        "UPDATE job_posts SET score = ?2, domain = ?3, line = ?4, lang = ?5, fit = ?6,
            contract = COALESCE(contract, ?7), salary = COALESCE(salary, ?8),
            apply_email = COALESCE(apply_email, ?9),
            apply_kind = CASE WHEN COALESCE(apply_email, ?9) IS NOT NULL THEN 'email' WHEN url IS NOT NULL THEN 'form' ELSE apply_kind END,
            apply_url = COALESCE(?10, apply_url),
            contact_name = COALESCE(contact_name, ?11), contact_role = COALESCE(contact_role, ?12),
            contact_email = COALESCE(contact_email, ?9),
            contact_source = CASE WHEN ?9 IS NOT NULL AND contact_email IS NULL THEN 'l''annonce' ELSE contact_source END,
            status = CASE WHEN status = 'new' AND ?3 = 'other' THEN 'dismissed' ELSE status END,
            analysed_at = datetime('now'), updated_at = datetime('now')
         WHERE id = ?1",
        params![id, score, domain, s(v, "line"), s(v, "lang").unwrap_or_else(|| "en".into()), fit.to_string(),
            s(v, "contract"), s(v, "salary"), email, apply_url, contact_name,
            s(v, "contact_role").filter(|_| contact_name.is_some())],
    );
    Some(score)
}

/// Analyse one stored posting. The description is completed from the page
/// when the source only gave a snippet.
pub fn analyse(conn: &Connection, http: &Http, llm: &dyn Llm, p: &Profile, id: i64, mx: &mut dyn FnMut(&str) -> Option<bool>) -> Result<Option<i64>, String> {
    let (title, company, location, url, mut description): (String, String, String, Option<String>, String) = conn
        .query_row("SELECT title, company, location, url, description FROM job_posts WHERE id = ?1", params![id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })
        .map_err(|e| e.to_string())?;
    if description.chars().count() < 600 {
        if let Some(page) = url.as_deref().and_then(|u| http.page_text(u)) {
            if page.len() > description.len() {
                description = page.chars().take(12000).collect();
                let _ = conn.execute("UPDATE job_posts SET description = ?2 WHERE id = ?1", params![id, description]);
            }
        }
    }
    let resp = llm.complete(&analysis_request(p, &title, &company, &location, &description))?;
    crate::commands::residency::claude::log_usage(conn, "jobs_analysis", None, &resp.model, &resp.usage);
    let v = extract_json(&resp.text).ok_or("analyse illisible")?;
    Ok(store_analysis(conn, id, &v, &description, mx))
}

// ---------------- Company ----------------

/// Hosts that are never a company's own site.
const NOT_A_SITE: &[&str] = &["linkedin.", "facebook.", "instagram.", "twitter.", "x.com", "youtube.", "wikipedia.",
    "glassdoor.", "indeed.", "crunchbase.", "bloomberg.", "zoominfo.", "rocketreach.", "tiktok.", "soundcloud.",
    "spotify.", "apple.com", "greenhouse.io", "lever.co", "ashbyhq.", "smartrecruiters.", "workable.", "recruitee.",
    "welcometothejungle.", "adzuna.", "musicbusinessworldwide.", "musicweek.", "billboard.", "discogs.", "bandcamp.",
    "google.", "pitchbook.", "dnb.com", "societe.com", "pappers.", "companieshouse.", "gov.uk", "yelp."];

pub fn pick_site(search: &Value, company: &str) -> Option<String> {
    let name: String = company.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();
    let first_word: String = company.to_lowercase().split_whitespace().next().unwrap_or("").chars().filter(|c| c.is_alphanumeric()).collect();
    let empty = Vec::new();
    search["organic"].as_array().unwrap_or(&empty).iter().take(6).find_map(|r| {
        let url = r["link"].as_str()?;
        let host = enrich::host_of(url);
        if host.is_empty() || NOT_A_SITE.iter().any(|x| host.contains(x)) {
            return None;
        }
        // The domain must carry the company's name: a press article does not.
        let bare: String = host.chars().filter(|c| c.is_alphanumeric()).collect();
        let hit = (name.len() >= 4 && bare.contains(&name)) || (first_word.len() >= 4 && bare.contains(&first_word));
        hit.then(|| format!("https://{host}"))
    })
}

fn company_schema() -> Value {
    let s = json!({ "type": "string" });
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["what", "kind", "roster", "hq", "size"],
        "properties": {
            "what": s,
            "kind": { "type": "string", "enum": ["label", "publisher", "sync", "management", "agency", "promoter", "distributor", "platform", "library", "collecting society", "media", "other"] },
            "roster": { "type": "array", "items": s },
            "hq": s, "size": s
        }
    })
}

pub fn company_request(company: &str, pages: &str) -> LlmRequest {
    let rules = "You are given text from a company's own website. Describe the company for a job seeker in the music \
industry.\n\
- what: one or two sentences in French, at most 260 characters: what the company does, for whom, and what it is known \
for. Only what the text says. No em dash.\n\
- kind: the closest category.\n\
- roster: when the company is a label, publisher, management firm, agency or library, up to 8 of the best-known artists, \
writers or clients NAMED IN THE TEXT, most prominent first. Empty when the text names none. Never add a name from memory.\n\
- hq: city and country of the head office if the text states it, else empty. size: staff or offices if stated, else empty.\n\
Answer with JSON only.";
    LlmRequest {
        model: DEFAULT_MODEL.to_string(),
        max_tokens: 600,
        system: vec![SystemBlock { text: rules.to_string(), cache: true }],
        user: format!("Company: {company}\n\nWEBSITE TEXT\n{}", pages.chars().take(9000).collect::<String>()),
        schema: Some(company_schema()),
    }
}

/// Best address to write to about a job, among those found on the site:
/// a recruiting mailbox first, a general one otherwise. Own domain only.
pub fn hiring_email(html: &str, host: &str) -> Option<String> {
    let site = host.trim_start_matches("www.");
    let mut found: Vec<String> = find_emails(html)
        .into_iter()
        .filter_map(|f| enrich::clean_email(&f.email))
        .filter(|e| !enrich::is_placeholder_email(e))
        .filter(|e| {
            let d = enrich::email_domain(e);
            d == site || site.ends_with(&format!(".{d}")) || d.ends_with(&format!(".{site}"))
        })
        .collect();
    found.sort();
    found.dedup();
    let rank = |e: &String| {
        let local = e.split('@').next().unwrap_or("");
        const HIRING: &[&str] = &["job", "career", "recruit", "recrut", "talent", "hr", "rh", "people", "hiring", "emploi", "work"];
        const GENERAL: &[&str] = &["info", "contact", "hello", "bonjour", "office", "enquir", "general", "team"];
        if HIRING.iter().any(|w| local.contains(w)) {
            0
        } else if GENERAL.iter().any(|w| local.contains(w)) {
            1
        } else {
            2
        }
    };
    found.sort_by_key(rank);
    found.into_iter().find(|e| rank(e) < 2)
}

pub struct CompanyFinds {
    pub site: Option<String>,
    pub what: Option<String>,
    pub kind: Option<String>,
    pub roster: Vec<String>,
    pub email: Option<String>,
    pub email_source: Option<String>,
}

/// Names of the roster that really stand in the pages read.
pub fn roster_in_text(v: &Value, text: &str) -> Vec<String> {
    let lower = text.to_lowercase();
    list(v, "roster", 8).into_iter().filter(|n| n.chars().count() > 1 && lower.contains(&n.to_lowercase())).collect()
}

/// Read the company's own site: what it does, who it represents, where to write.
pub fn company(http: &Http, llm: &dyn Llm, serper_key: Option<&str>, name: &str, known_site: Option<&str>,
    mx: &mut dyn FnMut(&str) -> Option<bool>, usage: &mut dyn FnMut(&str, &Usage)) -> CompanyFinds {
    let mut out = CompanyFinds { site: known_site.map(str::to_string), what: None, kind: None, roster: Vec::new(), email: None, email_source: None };
    if out.site.is_none() {
        if let Some(key) = serper_key {
            if let Ok(v) = http.search(key, &format!("{name} music company official website"), "US", false) {
                out.site = pick_site(&v, name);
            }
        }
    }
    let Some(site) = out.site.clone() else { return out };
    let host = enrich::host_of(&site);
    let mut text = String::new();
    for path in ["", "/about", "/about-us", "/a-propos", "/artists", "/roster", "/contact", "/careers", "/jobs"] {
        let url = format!("{}{}", site.trim_end_matches('/'), path);
        let Some(html) = http.page_html(&url) else { continue };
        if out.email.is_none() {
            if let Some(e) = hiring_email(&html, &host) {
                if mx(&enrich::email_domain(&e)) == Some(true) {
                    out.email = Some(e);
                    out.email_source = Some(url.clone());
                }
            }
        }
        let t = super::sources::html_to_text(&html);
        if t.len() > 150 && !text.contains(&t.chars().take(200).collect::<String>()) {
            text.push_str(&format!("\n\n[{}]\n{}", if path.is_empty() { "/" } else { path }, t.chars().take(3500).collect::<String>()));
        }
        if text.len() > 12000 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
    if text.len() < 300 {
        return out;
    }
    if let Ok(resp) = llm.complete(&company_request(name, &text)) {
        usage(&resp.model, &resp.usage);
        if let Some(v) = extract_json(&resp.text) {
            out.what = s(&v, "what").map(|w| match s(&v, "hq") {
                Some(hq) => format!("{w} Siège : {hq}."),
                None => w,
            });
            out.kind = s(&v, "kind");
            out.roster = roster_in_text(&v, &text);
        }
    }
    out
}

pub fn store_company(conn: &Connection, id: i64, f: &CompanyFinds) {
    let _ = conn.execute(
        "UPDATE job_posts SET company_site = COALESCE(?2, company_site), company_brief = COALESCE(?3, company_brief),
            company_kind = COALESCE(?4, company_kind), roster = COALESCE(?5, roster),
            contact_email = COALESCE(contact_email, ?6),
            contact_source = CASE WHEN contact_email IS NULL AND ?6 IS NOT NULL THEN ?7 ELSE contact_source END,
            briefed_at = datetime('now'), updated_at = datetime('now')
         WHERE id = ?1",
        params![id, f.site, f.what, f.kind, (!f.roster.is_empty()).then(|| f.roster.join(", ")), f.email, f.email_source],
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn yes(_: &str) -> Option<bool> {
        Some(true)
    }

    #[test]
    fn analysis_keeps_only_what_the_page_says() {
        let conn = crate::commands::jobs::test_db();
        conn.execute("INSERT INTO job_posts (source_key, title, url) VALUES ('k', 'Sync Manager', 'https://x.test/job')", []).unwrap();
        let text = "Sync Manager wanted. Send your CV to jobs@acme.test. Questions to Dana Reed, Head of Sync.";
        let v = json!({
            "is_posting": true, "domain": "sync", "score": 82, "line": "Poste de sync — CDI", "lang": "en",
            "strengths": ["12 ans de sync"], "gaps": [], "keywords": ["sync", "clearance"],
            "apply_email": "jobs@acme.test", "contact_name": "Dana Reed", "contact_role": "Head of Sync", "salary": ""
        });
        assert_eq!(store_analysis(&conn, 1, &v, text, &mut yes), Some(82));
        let j = crate::commands::jobs::one(&conn, 1).unwrap();
        assert_eq!((j.apply_kind.as_str(), j.apply_email.as_deref()), ("email", Some("jobs@acme.test")));
        assert_eq!(j.contact_name.as_deref(), Some("Dana Reed"));
        assert_eq!(j.contact_source.as_deref(), Some("l'annonce"));
        assert!(!j.line.unwrap().contains('—'));

        conn.execute("INSERT INTO job_posts (source_key, title, url) VALUES ('k2', 'Label Manager', 'https://x.test/2')", []).unwrap();
        let v = json!({ "is_posting": true, "domain": "label", "score": 60, "line": "x", "lang": "en", "strengths": [], "gaps": [], "keywords": [],
            "apply_email": "boss@elsewhere.test", "contact_name": "Invented Person" });
        store_analysis(&conn, 2, &v, "Apply on our site.", &mut yes);
        let j = crate::commands::jobs::one(&conn, 2).unwrap();
        assert_eq!(j.apply_email, None, "an address absent from the page is not kept");
        assert_eq!(j.contact_name, None);
        assert_eq!(j.apply_kind, "form");
    }

    #[test]
    fn a_page_that_is_not_a_posting_is_dismissed() {
        let conn = crate::commands::jobs::test_db();
        conn.execute("INSERT INTO job_posts (source_key, title) VALUES ('k', 'Sync jobs')", []).unwrap();
        assert_eq!(store_analysis(&conn, 1, &json!({ "is_posting": false }), "", &mut yes), None);
        assert_eq!(crate::commands::jobs::one(&conn, 1).unwrap().status, "dismissed");
    }

    #[test]
    fn email_needs_the_page_and_a_mail_domain() {
        assert_eq!(verified_email(Some("Jobs@Acme.test".into()), "write to jobs@acme.test", &mut yes), Some("jobs@acme.test".into()));
        assert_eq!(verified_email(Some("jobs@acme.test".into()), "no address here", &mut yes), None);
        assert_eq!(verified_email(Some("jobs@acme.test".into()), "jobs@acme.test", &mut |_| Some(false)), None);
    }

    #[test]
    fn site_must_carry_the_company_name() {
        let v = json!({"organic": [
            {"link": "https://www.linkedin.com/company/acme-music"},
            {"link": "https://www.musicweek.com/acme-signs"},
            {"link": "https://www.acmemusic.com/about"}
        ]});
        assert_eq!(pick_site(&v, "Acme Music"), Some("https://acmemusic.com".into()));
        assert_eq!(pick_site(&json!({"organic": [{"link": "https://news.test/acme"}]}), "Acme Music"), None);
    }

    #[test]
    fn hiring_mailbox_first_and_own_domain_only() {
        let html = "<a href='mailto:press@acme.test'>p</a> info@acme.test careers@acme.test someone@gmail.com";
        assert_eq!(hiring_email(html, "www.acme.test"), Some("careers@acme.test".into()));
        assert_eq!(hiring_email("info@acme.test", "acme.test"), Some("info@acme.test".into()));
        assert_eq!(hiring_email("press@acme.test bob@other.test", "acme.test"), None);
    }

    #[test]
    fn roster_names_must_be_on_the_page() {
        let v = json!({"roster": ["Monomotion", "Famous Star", "NEUS"]});
        assert_eq!(roster_in_text(&v, "Our artists: monomotion, Neus and more"), vec!["Monomotion", "NEUS"]);
    }
}
