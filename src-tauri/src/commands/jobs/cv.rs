//! The CV adapted to one posting.
//!
//! The profile (`job_profile` setting, JSON, edited by the user) is the only
//! source of facts. The AI chooses and rewords: which figures to show, which
//! job title variant, which bullets and in what words. It never supplies a
//! date, an employer or a number: those are copied from the profile by code,
//! and every line it writes goes through `guard`, which drops a line naming
//! anything the profile does not hold.
//!
//! The PDF is printed by a Chromium browser found on the machine (headless),
//! from one HTML flow in reading order: what a parser extracts is what a
//! person reads, top to bottom. No letter-spacing and no ligatures either:
//! both come out of a PDF as broken words ("T A R G E T", "ﬂuent").

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::commands::residency::claude::{extract_json, Llm, LlmRequest, SystemBlock};

/// Shown in the editor when no profile is stored yet. No personal data here:
/// the repository is public, the real profile lives in the local database.
pub const PROFILE_SKELETON: &str = r#"{
  "identity": {
    "name": "",
    "location": { "en": "", "fr": "" },
    "phone": "",
    "email": "",
    "links": [],
    "affiliations": "",
    "languages": { "en": "", "fr": "" }
  },
  "summary": { "en": "", "fr": "" },
  "work_rights": "",
  "figures": [ { "value": "", "label": { "en": "", "fr": "" } } ],
  "experience": [
    {
      "id": "job1",
      "period": { "en": "2020 to present", "fr": "2020 à aujourd'hui" },
      "titles": [ { "en": "", "fr": "" } ],
      "org": "",
      "bullets": [ { "en": "", "fr": "" } ]
    }
  ],
  "skills": [ { "name": { "en": "", "fr": "" }, "items": { "en": "", "fr": "" } } ],
  "placements": [],
  "facts": []
}"#;

/// A text in the two languages of the CVs. One side may be empty.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct L {
    #[serde(default)]
    pub en: String,
    #[serde(default)]
    pub fr: String,
}

impl L {
    pub fn get(&self, lang: &str) -> &str {
        let (first, second) = if lang == "fr" { (&self.fr, &self.en) } else { (&self.en, &self.fr) };
        if first.trim().is_empty() { second.trim() } else { first.trim() }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Identity {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub location: L,
    #[serde(default)]
    pub phone: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub links: Vec<String>,
    #[serde(default)]
    pub affiliations: String,
    #[serde(default)]
    pub languages: L,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Figure {
    pub value: String,
    #[serde(default)]
    pub label: L,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Experience {
    pub id: String,
    #[serde(default)]
    pub period: L,
    #[serde(default)]
    pub titles: Vec<L>,
    #[serde(default)]
    pub org: String,
    #[serde(default)]
    pub bullets: Vec<L>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Skill {
    #[serde(default)]
    pub name: L,
    #[serde(default)]
    pub items: L,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Placement {
    #[serde(default)]
    pub group: L,
    #[serde(default)]
    pub programme: String,
    #[serde(default)]
    pub broadcaster: String,
    #[serde(default)]
    pub track: String,
    #[serde(default)]
    pub reach: String,
    #[serde(default)]
    pub role: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Profile {
    #[serde(default)]
    pub identity: Identity,
    #[serde(default)]
    pub summary: L,
    /// Right to work per country, in the user's words. Given to the AI for the
    /// analysis of a posting, never printed.
    #[serde(default)]
    pub work_rights: String,
    #[serde(default)]
    pub figures: Vec<Figure>,
    #[serde(default)]
    pub experience: Vec<Experience>,
    #[serde(default)]
    pub skills: Vec<Skill>,
    #[serde(default)]
    pub placements: Vec<Placement>,
    /// Other true facts the AI may draw on (earlier jobs, tools, studies).
    #[serde(default)]
    pub facts: Vec<String>,
}

pub fn load_profile(conn: &Connection) -> Option<Profile> {
    let raw = super::setting(conn, "job_profile")?;
    let p: Profile = serde_json::from_str(&raw).ok()?;
    (!p.identity.name.trim().is_empty() && !p.experience.is_empty()).then_some(p)
}

/// The CV as printed: every field resolved in one language.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct CvDoc {
    pub lang: String,
    pub target: String,
    pub place: String,
    pub name: String,
    pub summary: String,
    pub location: String,
    pub phone: String,
    pub email: String,
    pub links: Vec<String>,
    pub affiliations: String,
    pub languages: String,
    pub figures: Vec<(String, String)>,
    pub experience: Vec<DocExperience>,
    pub skills: Vec<(String, String)>,
    pub placements: Vec<(String, Vec<Placement>)>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct DocExperience {
    pub period: String,
    pub title: String,
    pub org: String,
    pub bullets: Vec<String>,
}

fn norm_lang(lang: &str) -> &'static str {
    if lang.trim().to_lowercase().starts_with("fr") { "fr" } else { "en" }
}

fn placements_of(p: &Profile, lang: &str) -> Vec<(String, Vec<Placement>)> {
    let mut groups: Vec<(String, Vec<Placement>)> = Vec::new();
    for pl in &p.placements {
        let g = pl.group.get(lang).to_string();
        match groups.iter_mut().find(|(name, _)| *name == g) {
            Some((_, rows)) => rows.push(pl.clone()),
            None => groups.push((g, vec![pl.clone()])),
        }
    }
    groups
}

/// The CV with nothing chosen by the AI: first title, every bullet, first
/// figures and skills. Used when no AI is configured or its answer is unusable.
pub fn base_doc(p: &Profile, lang: &str, target: &str, place: &str) -> CvDoc {
    let lang = norm_lang(lang);
    CvDoc {
        lang: lang.to_string(),
        target: target.trim().to_string(),
        place: place.trim().to_string(),
        name: p.identity.name.trim().to_string(),
        summary: p.summary.get(lang).to_string(),
        location: p.identity.location.get(lang).to_string(),
        phone: p.identity.phone.trim().to_string(),
        email: p.identity.email.trim().to_string(),
        links: p.identity.links.clone(),
        affiliations: p.identity.affiliations.trim().to_string(),
        languages: p.identity.languages.get(lang).to_string(),
        figures: p.figures.iter().take(4).map(|f| (f.value.clone(), f.label.get(lang).to_string())).collect(),
        experience: p
            .experience
            .iter()
            .map(|e| DocExperience {
                period: e.period.get(lang).to_string(),
                title: e.titles.first().map(|t| t.get(lang).to_string()).unwrap_or_default(),
                org: e.org.clone(),
                bullets: e.bullets.iter().map(|b| b.get(lang).to_string()).filter(|b| !b.is_empty()).collect(),
            })
            .collect(),
        skills: p.skills.iter().take(3).map(|s| (s.name.get(lang).to_string(), s.items.get(lang).to_string())).collect(),
        placements: placements_of(p, lang),
    }
}

// ---------------- Fact guard ----------------

fn words(s: &str) -> impl Iterator<Item = String> + '_ {
    s.split(|c: char| !(c.is_alphanumeric() || c == '&' || c == '+' || c == '#'))
        .filter(|w| !w.is_empty())
        .map(|w| w.to_lowercase())
}

/// Every word the profile contains, both languages, lower case.
pub fn corpus(p: &Profile) -> HashSet<String> {
    let all = serde_json::to_string(p).unwrap_or_default().replace("\\n", " ");
    words(&all).collect()
}

/// Why a line written by the AI cannot be printed, or None when every name
/// and number in it is backed by the profile.
/// - a number must exist in the profile;
/// - a capitalised word that does not start a sentence (a name, a tool, a
///   company) must exist in the profile;
/// - a term of the posting that the profile does not hold may not be claimed.
pub fn guard(line: &str, corpus: &HashSet<String>, posting_terms: &HashSet<String>) -> Option<String> {
    let mut sentence_start = true;
    for raw in line.split_whitespace() {
        let token = raw.trim_matches(|c: char| !(c.is_alphanumeric() || c == '&' || c == '+' || c == '#' || c == '$' || c == '€' || c == '£'));
        let starts = sentence_start;
        sentence_start = raw.ends_with('.') || raw.ends_with(':') || raw.ends_with(';') || token.is_empty();
        if token.is_empty() {
            continue;
        }
        for w in words(token) {
            let has_digit = w.chars().any(|c| c.is_ascii_digit());
            let proper = !starts && token.chars().next().map(|c| c.is_uppercase()).unwrap_or(false);
            let known = corpus.contains(&w);
            if has_digit && !known {
                return Some(format!("chiffre absent du profil : {token}"));
            }
            if proper && !known && w.len() > 1 {
                return Some(format!("nom absent du profil : {token}"));
            }
            if !known && w.len() > 3 && posting_terms.contains(&w) {
                return Some(format!("terme de l'annonce absent du profil : {token}"));
            }
        }
    }
    None
}

/// Words of the posting's key terms (lower case), for the guard.
pub fn posting_terms(keywords: &[String]) -> HashSet<String> {
    keywords.iter().flat_map(|k| words(k).collect::<Vec<_>>()).filter(|w| w.len() > 3).collect()
}

// ---------------- Tailoring ----------------

pub struct Posting<'a> {
    pub title: &'a str,
    pub company: &'a str,
    pub location: &'a str,
    pub description: &'a str,
    pub lang: &'a str,
    pub keywords: &'a [String],
}

fn tailor_system(p: &Profile) -> Vec<SystemBlock> {
    let rules = "You adapt one person's CV to one job posting. You are an editor, not an author: \
the profile below is the only source of facts and you may not add to it.\n\
What you decide: which figures to show (up to 4, by index), which title variant to print for each job (by index), \
which bullets to keep for each job and how to word them, and three skill groups.\n\
Rules:\n\
- Keep every job of the profile, in the same order. Dates and employers are printed from the profile, you do not write them.\n\
- A bullet restates facts of that job's bullets (or of the 'facts' list). You may merge, shorten, reorder and reword so the \
vocabulary matches the posting where the profile truly supports it. Never add a number, a company, a tool, a title, a result \
or a skill that is not in the profile. If the posting asks for something the profile does not show, leave it out.\n\
- Put first the bullets that answer the posting. 2 to 4 bullets per job, one line each, no first person, no full stop needed.\n\
- Skill groups: a short name and one line of items separated by commas, drawn from the profile's skills and facts only.\n\
- Write in the language given. Plain words. No em dash, no semicolon chains, no superlatives.\n\
Answer with JSON only.";
    vec![
        SystemBlock { text: rules.to_string(), cache: false },
        SystemBlock { text: format!("PROFILE (JSON)\n{}", serde_json::to_string(p).unwrap_or_default()), cache: true },
    ]
}

fn tailor_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["figures", "experience", "skills"],
        "properties": {
            "figures": { "type": "array", "items": { "type": "integer" } },
            "experience": { "type": "array", "items": {
                "type": "object", "additionalProperties": false, "required": ["id", "title", "bullets"],
                "properties": { "id": { "type": "string" }, "title": { "type": "integer" }, "bullets": { "type": "array", "items": { "type": "string" } } }
            } },
            "skills": { "type": "array", "items": {
                "type": "object", "additionalProperties": false, "required": ["name", "items"],
                "properties": { "name": { "type": "string" }, "items": { "type": "string" } }
            } }
        }
    })
}

pub fn tailor_request(p: &Profile, post: &Posting, model: &str) -> LlmRequest {
    let lang = norm_lang(post.lang);
    let description: String = post.description.chars().take(6000).collect();
    LlmRequest {
        model: model.to_string(),
        max_tokens: 2500,
        system: tailor_system(p),
        user: format!(
            "Language of the CV: {}\nPOSTING\nTitle: {}\nCompany: {}\nPlace: {}\nTerms a screening tool is likely to look for: {}\n\n{}",
            if lang == "fr" { "French" } else { "English" },
            post.title, post.company, post.location, post.keywords.join(", "), description
        ),
        schema: Some(tailor_schema()),
    }
}

fn clean_line(s: &str) -> String {
    crate::commands::residency::claude::no_dashes(s)
        .trim()
        .trim_start_matches(['-', '•', '*', '–'])
        .trim()
        .trim_end_matches('.')
        .to_string()
}

/// Turn the AI's choices into a CV. Anything unusable falls back to the
/// profile's own wording; each fallback is reported in the warnings.
pub fn apply_choices(p: &Profile, post: &Posting, choices: &Value) -> (CvDoc, Vec<String>) {
    let lang = norm_lang(post.lang);
    let mut doc = base_doc(p, lang, post.title, post.location);
    let mut warnings = Vec::new();
    let corpus = corpus(p);
    let terms = posting_terms(post.keywords);

    let figs: Vec<(String, String)> = choices["figures"]
        .as_array()
        .map(|a| a.iter().filter_map(|i| i.as_u64()).filter_map(|i| p.figures.get(i as usize)).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .take(4)
        .map(|f| (f.value.clone(), f.label.get(lang).to_string()))
        .collect();
    if !figs.is_empty() {
        doc.figures = figs;
    }

    let empty = Vec::new();
    let chosen = choices["experience"].as_array().unwrap_or(&empty);
    for (i, e) in p.experience.iter().enumerate() {
        let Some(c) = chosen.iter().find(|c| c["id"].as_str() == Some(e.id.as_str())) else { continue };
        if let Some(t) = c["title"].as_u64().and_then(|t| e.titles.get(t as usize)) {
            doc.experience[i].title = t.get(lang).to_string();
        }
        let mut bullets = Vec::new();
        for b in c["bullets"].as_array().unwrap_or(&empty).iter().filter_map(|b| b.as_str()) {
            let line = clean_line(b);
            if line.is_empty() {
                continue;
            }
            match guard(&line, &corpus, &terms) {
                None => bullets.push(line),
                Some(why) => warnings.push(format!("Ligne écartée ({}) : {}", why, line)),
            }
        }
        if !bullets.is_empty() {
            doc.experience[i].bullets = bullets.into_iter().take(5).collect();
        }
    }

    let mut skills = Vec::new();
    for s in choices["skills"].as_array().unwrap_or(&empty) {
        let (name, items) = (clean_line(s["name"].as_str().unwrap_or("")), clean_line(s["items"].as_str().unwrap_or("")));
        if name.is_empty() || items.is_empty() {
            continue;
        }
        // Skill lines are lists: every item is checked as if it started a sentence.
        let bad = items.split(',').find_map(|it| guard(&format!("x {}", it.trim()), &corpus, &terms)).or_else(|| guard(&format!("x {name}"), &corpus, &terms));
        match bad {
            None => skills.push((name, items)),
            Some(why) => warnings.push(format!("Compétence écartée ({}) : {} · {}", why, name, items)),
        }
    }
    if skills.len() >= 2 {
        doc.skills = skills.into_iter().take(3).collect();
    }
    (doc, warnings)
}

/// The adapted CV: AI choices when a client is given and answers usably,
/// otherwise the profile as it stands.
pub fn tailor(p: &Profile, post: &Posting, llm: Option<&dyn Llm>, model: &str, usage: &mut dyn FnMut(&str, &crate::commands::residency::claude::Usage)) -> (CvDoc, Vec<String>) {
    let lang = norm_lang(post.lang);
    let Some(llm) = llm else {
        return (base_doc(p, lang, post.title, post.location), vec!["CV non adapté : IA non réglée.".into()]);
    };
    match llm.complete(&tailor_request(p, post, model)) {
        Ok(resp) => {
            usage(&resp.model, &resp.usage);
            match extract_json(&resp.text) {
                Some(v) => apply_choices(p, post, &v),
                None => (base_doc(p, lang, post.title, post.location), vec!["CV non adapté : réponse de l'IA illisible.".into()]),
            }
        }
        Err(e) => (base_doc(p, lang, post.title, post.location), vec![format!("CV non adapté : {e}")]),
    }
}

// ---------------- HTML ----------------

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

struct Labels {
    target: &'static str,
    contact: &'static str,
    links: &'static str,
    languages: &'static str,
    affiliations: &'static str,
    figures: &'static str,
    experience: &'static str,
    skills: &'static str,
    placements: &'static str,
}

fn labels(lang: &str) -> Labels {
    if lang == "fr" {
        Labels { target: "Profil de recherche", contact: "Contact", links: "Liens", languages: "Langues", affiliations: "Affiliations",
            figures: "En chiffres", experience: "Expérience", skills: "Compétences", placements: "Placements" }
    } else {
        Labels { target: "Target role", contact: "Contact", links: "Links", languages: "Languages", affiliations: "Affiliations",
            figures: "In figures", experience: "Experience", skills: "Skills", placements: "Placements" }
    }
}

const CSS: &str = "@page{size:A4;margin:15mm 16mm}*{box-sizing:border-box}\
body{font-family:'Helvetica Neue',Helvetica,Arial,sans-serif;font-variant-ligatures:none;font-feature-settings:'liga' 0,'clig' 0;letter-spacing:0;color:#111;font-size:9.6pt;line-height:1.42;margin:0}\
h1{font-size:25pt;line-height:1;margin:0 0 3mm;text-transform:uppercase;font-weight:700}\
.target{font-size:9pt;text-transform:uppercase;font-weight:700;margin:0 0 4mm}\
.summary{margin:0 0 4mm;max-width:165mm;color:#222}\
.meta{border-top:.4pt solid #111;border-bottom:.4pt solid #111;padding:2.6mm 0;margin:0 0 4mm}\
.meta p{margin:0 0 .6mm}.meta b,.k{font-size:7.4pt;text-transform:uppercase;font-weight:700}\
.figs{margin:0 0 5mm;padding:0;list-style:none;display:flex;gap:7mm;flex-wrap:wrap}\
.figs li{white-space:nowrap}.figs b{font-size:14pt;margin-right:1.6mm}.figs span{font-size:7.6pt;text-transform:uppercase}\
h2{font-size:9pt;text-transform:uppercase;margin:5mm 0 2.4mm;padding-bottom:1.2mm;border-bottom:.4pt solid #111}\
.job{margin:0 0 3.2mm;break-inside:avoid}.job h3{font-size:10.2pt;margin:0;font-weight:700}\
.job .org{margin:0;color:#333}.job .org .per{font-size:7.8pt;text-transform:uppercase}\
ul.b{margin:1mm 0 0;padding-left:4.2mm}ul.b li{margin:0 0 .5mm}\
.skill{margin:0 0 1.6mm}.skill b{font-weight:700}\
h4{font-size:8pt;text-transform:uppercase;margin:3.4mm 0 1.2mm;color:#333}\
.pl{margin:0;padding:0;list-style:none}.pl li{margin:0 0 .7mm;break-inside:avoid;font-size:8.8pt}.pl b{font-weight:700}";

/// One HTML flow, in the order a person reads it. No columns, no tables, no
/// text in images: a parser gets the same lines in the same order.
pub fn render_html(d: &CvDoc) -> String {
    let l = labels(&d.lang);
    let mut h = String::new();
    h.push_str(&format!(
        "<!doctype html><html lang=\"{}\"><head><meta charset=\"utf-8\"><title>{} · {}</title><style>{}</style></head><body>",
        esc(&d.lang), esc(&d.name), esc(&d.target), CSS
    ));
    h.push_str(&format!("<h1>{}</h1>", esc(&d.name)));
    let target = if d.place.is_empty() { d.target.clone() } else { format!("{} · {}", d.target, d.place) };
    h.push_str(&format!("<p class=\"target\">{} : {}</p>", l.target, esc(&target)));
    if !d.summary.is_empty() {
        h.push_str(&format!("<p class=\"summary\">{}</p>", esc(&d.summary)));
    }
    h.push_str("<div class=\"meta\">");
    let contact: Vec<&str> = [d.email.as_str(), d.phone.as_str(), d.location.as_str()].into_iter().filter(|s| !s.is_empty()).collect();
    h.push_str(&format!("<p><b>{}</b> : {}</p>", l.contact, esc(&contact.join(" · "))));
    if !d.links.is_empty() {
        h.push_str(&format!("<p><b>{}</b> : {}</p>", l.links, esc(&d.links.join(" · "))));
    }
    if !d.languages.is_empty() {
        h.push_str(&format!("<p><b>{}</b> : {}</p>", l.languages, esc(&d.languages)));
    }
    if !d.affiliations.is_empty() {
        h.push_str(&format!("<p><b>{}</b> : {}</p>", l.affiliations, esc(&d.affiliations)));
    }
    h.push_str("</div>");
    if !d.figures.is_empty() {
        h.push_str(&format!("<ul class=\"figs\" aria-label=\"{}\">", l.figures));
        for (v, label) in &d.figures {
            h.push_str(&format!("<li><b>{}</b><span>{}</span></li>", esc(v), esc(label)));
        }
        h.push_str("</ul>");
    }
    h.push_str(&format!("<h2>{}</h2>", l.experience));
    for e in &d.experience {
        h.push_str(&format!(
            "<div class=\"job\"><h3>{}</h3><p class=\"org\">{} · <span class=\"per\">{}</span></p>",
            esc(&e.title), esc(&e.org), esc(&e.period)
        ));
        if !e.bullets.is_empty() {
            h.push_str("<ul class=\"b\">");
            for b in &e.bullets {
                h.push_str(&format!("<li>{}</li>", esc(b)));
            }
            h.push_str("</ul>");
        }
        h.push_str("</div>");
    }
    if !d.skills.is_empty() {
        h.push_str(&format!("<h2>{}</h2>", l.skills));
        for (name, items) in &d.skills {
            h.push_str(&format!("<p class=\"skill\"><b>{}</b> : {}</p>", esc(name), esc(items)));
        }
    }
    if !d.placements.is_empty() {
        h.push_str(&format!("<h2>{}</h2>", l.placements));
        for (group, rows) in &d.placements {
            if !group.is_empty() {
                h.push_str(&format!("<h4>{}</h4>", esc(group)));
            }
            h.push_str("<ul class=\"pl\">");
            for r in rows {
                let rest: Vec<&str> = [r.broadcaster.as_str(), r.track.as_str(), r.reach.as_str(), r.role.as_str()]
                    .into_iter().filter(|s| !s.trim().is_empty()).collect();
                h.push_str(&format!("<li><b>{}</b>{}{}</li>", esc(&r.programme), if rest.is_empty() { "" } else { " · " }, esc(&rest.join(" · "))));
            }
            h.push_str("</ul>");
        }
    }
    h.push_str("</body></html>");
    h
}

// ---------------- PDF ----------------

/// A Chromium browser installed on this Mac, used headless to print the PDF.
pub fn browser_path() -> Option<PathBuf> {
    const CANDIDATES: &[&str] = &[
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
        "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
        "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
        "/Applications/Arc.app/Contents/MacOS/Arc",
    ];
    let home = std::env::var("HOME").unwrap_or_default();
    CANDIDATES
        .iter()
        .flat_map(|c| [PathBuf::from(c), PathBuf::from(format!("{home}{c}"))])
        .find(|p| p.exists())
}

pub fn file_name(d: &CvDoc) -> String {
    let slug = |s: &str| {
        s.split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .map(|w| {
                let mut c = w.chars();
                c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
            })
            .collect::<Vec<_>>()
            .join("_")
    };
    let role: String = slug(&d.target).chars().take(48).collect();
    format!("CV_{}_{}_{}.pdf", slug(&d.name), role, d.lang.to_uppercase())
}

/// Print the CV to `dir`. Returns the path of the PDF.
pub fn print_pdf(d: &CvDoc, dir: &Path) -> Result<PathBuf, String> {
    let browser = browser_path().ok_or("Aucun navigateur Chromium trouvé (Chrome, Brave, Edge) pour produire le PDF.")?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let html_path = dir.join("cv.html");
    std::fs::write(&html_path, render_html(d)).map_err(|e| e.to_string())?;
    let pdf_path = dir.join(file_name(d));
    let _ = std::fs::remove_file(&pdf_path);
    let profile_dir = dir.join(".print-profile");
    let out = std::process::Command::new(&browser)
        .arg("--headless=new")
        .arg("--disable-gpu")
        .arg("--no-first-run")
        .arg("--no-pdf-header-footer")
        .arg(format!("--user-data-dir={}", profile_dir.display()))
        .arg(format!("--print-to-pdf={}", pdf_path.display()))
        .arg(format!("file://{}", html_path.display()))
        .output()
        .map_err(|e| format!("Impression PDF impossible : {e}"))?;
    let _ = std::fs::remove_dir_all(&profile_dir);
    if !pdf_path.exists() {
        return Err(format!("Le navigateur n'a pas produit le PDF ({})", String::from_utf8_lossy(&out.stderr).lines().last().unwrap_or("")));
    }
    Ok(pdf_path)
}

/// Text of a PDF as a parser reads it, None when it cannot be read.
pub fn pdf_text(path: &Path) -> Option<String> {
    let path = path.to_path_buf();
    // The extractor panics on some malformed files: never take the app down.
    std::panic::catch_unwind(move || pdf_extract::extract_text(&path).ok())
        .ok()
        .flatten()
        .filter(|t| t.trim().len() > 40)
}

#[cfg(test)]
pub(crate) fn sample_profile() -> Profile {
    serde_json::from_value(json!({
        "identity": { "name": "Alex Martin", "location": { "en": "Lyon, France", "fr": "Lyon, France" }, "phone": "+33 6 00 00 00 00",
            "email": "alex@example.test", "links": ["example.test"], "affiliations": "SACEM", "languages": { "en": "French · English, fluent", "fr": "Français · Anglais courant" } },
        "summary": { "en": "Ten years in music licensing.", "fr": "Dix ans dans le licensing musical." },
        "work_rights": "FR yes",
        "figures": [
            { "value": "12", "label": { "en": "years in sync", "fr": "ans de sync" } },
            { "value": "236", "label": { "en": "programmes aired", "fr": "programmes diffusés" } },
            { "value": "41", "label": { "en": "broadcast countries", "fr": "pays de diffusion" } }
        ],
        "experience": [
            { "id": "label", "period": { "en": "2017 to present", "fr": "2017 à aujourd'hui" },
              "titles": [ { "en": "Founder, label manager", "fr": "Fondateur, label manager" }, { "en": "Founder, label and artist manager", "fr": "" } ],
              "org": "Acme Records, Lyon",
              "bullets": [ { "en": "Licence deals with brands and producers", "fr": "Licences avec marques et producteurs" },
                           { "en": "Built an in-house licensing platform in Next.js", "fr": "" } ] },
            { "id": "sales", "period": { "en": "2016 to 2018", "fr": "2016 à 2018" }, "titles": [ { "en": "Sales manager", "fr": "Responsable des ventes" } ],
              "org": "Widgets Inc, Houston", "bullets": [ { "en": "50 accounts, around $1.5M in annual revenue", "fr": "" } ] }
        ],
        "skills": [ { "name": { "en": "Rights and licensing", "fr": "Droits et licensing" }, "items": { "en": "Sync, clearance, cue sheets", "fr": "" } },
                    { "name": { "en": "Tools", "fr": "Outils" }, "items": { "en": "Logic Pro, Zendesk", "fr": "" } } ],
        "placements": [ { "group": { "en": "Film and series", "fr": "Films et séries" }, "programme": "Holidate (2020)", "broadcaster": "Netflix", "track": "Bloodprint", "reach": "28 countries", "role": "Composer" } ],
        "facts": [ "Negotiated fees with ad agencies" ]
    }))
    .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn posting<'a>(keywords: &'a [String]) -> Posting<'a> {
        Posting { title: "Sync Manager", company: "Big Publisher", location: "London", description: "Pitch and clear music. Salesforce a plus.", lang: "en", keywords }
    }

    #[test]
    fn guard_blocks_numbers_names_and_posting_terms_the_profile_lacks() {
        let p = sample_profile();
        let c = corpus(&p);
        let terms = posting_terms(&["Salesforce".to_string(), "clearance".to_string()]);
        assert_eq!(guard("Licence deals with brands, 50 accounts", &c, &terms), None);
        assert!(guard("Closed 80 licence deals", &c, &terms).unwrap().contains("chiffre"));
        assert!(guard("Licence deals with Universal", &c, &terms).unwrap().contains("nom"));
        assert!(guard("Tracked deals in salesforce", &c, &terms).unwrap().contains("annonce"));
        assert_eq!(guard("Clearance and cue sheets for Netflix", &c, &terms), None);
        assert_eq!(guard("Negotiated fees with agencies", &c, &terms), None, "plain wording is free");
    }

    #[test]
    fn choices_keep_profile_dates_and_drop_invented_lines() {
        let p = sample_profile();
        let kw = vec!["Salesforce".to_string()];
        let choices = json!({
            "figures": [2, 0, 9],
            "experience": [
                { "id": "label", "title": 1, "bullets": ["Licence deals with brands and producers.", "Managed a Salesforce pipeline of 300 deals"] },
                { "id": "ghost", "title": 0, "bullets": ["Invented job"] }
            ],
            "skills": [ { "name": "Rights and licensing", "items": "Sync, clearance" }, { "name": "Tools", "items": "Zendesk, Salesforce" }, { "name": "Tools", "items": "Logic Pro" } ]
        });
        let (doc, warnings) = apply_choices(&p, &posting(&kw), &choices);
        assert_eq!(doc.figures, vec![("41".into(), "broadcast countries".into()), ("12".into(), "years in sync".into())]);
        assert_eq!(doc.experience.len(), 2, "every job of the profile stays, no job is added");
        assert_eq!(doc.experience[0].title, "Founder, label and artist manager");
        assert_eq!(doc.experience[0].period, "2017 to present");
        assert_eq!(doc.experience[0].bullets, vec!["Licence deals with brands and producers".to_string()]);
        assert_eq!(doc.experience[1].bullets, vec!["50 accounts, around $1.5M in annual revenue".to_string()], "untouched job keeps the profile wording");
        assert_eq!(doc.skills.len(), 2);
        assert!(doc.skills.iter().all(|(_, items)| !items.contains("Salesforce")));
        assert_eq!(warnings.len(), 2);
    }

    #[test]
    fn html_reads_top_to_bottom() {
        let p = sample_profile();
        let doc = base_doc(&p, "fr", "Sync Manager", "Paris");
        let text = crate::commands::jobs::sources::html_to_text(&render_html(&doc));
        let pos = |s: &str| text.find(s).unwrap_or_else(|| panic!("missing {s}"));
        assert!(pos("Alex Martin") < pos("alex@example.test"));
        assert!(pos("alex@example.test") < pos("Expérience"));
        assert!(pos("Fondateur, label manager") < pos("Acme Records, Lyon"));
        assert!(pos("Acme Records, Lyon") < pos("Responsable des ventes"));
        assert!(pos("Compétences") < pos("Placements"));
        assert_eq!(file_name(&doc), "CV_Alex_Martin_Sync_Manager_FR.pdf");
    }
}
