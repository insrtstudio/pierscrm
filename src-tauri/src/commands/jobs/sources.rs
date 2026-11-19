//! Where postings come from. Only public, documented endpoints:
//! - Adzuna API (France, UK, USA), with the user's own free developer key;
//! - the public job-board APIs of applicant tracking systems (Greenhouse,
//!   Lever, Ashby, SmartRecruiters, Workable, Recruitee), one board per company;
//! - RSS feeds of job boards;
//! - a web search (Serper key already used by the app) for postings on the
//!   sites of small companies.
//! No login, no scraping of protected sites.

use std::time::Duration;

use rusqlite::{params, Connection};
use serde_json::Value;

pub const PLATFORMS: &[&str] = &["greenhouse", "lever", "ashby", "smartrecruiters", "workable", "recruitee", "rss"];
const UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) PiersCRM/jobs";

/// One posting as a source gives it, before any analysis.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Lead {
    pub source: String,
    pub key: String,
    pub url: String,
    pub title: String,
    pub company: String,
    pub location: String,
    pub country: String,
    pub domain: String,
    pub contract: Option<String>,
    pub salary: Option<String>,
    pub posted_at: Option<String>,
    pub description: String,
}

// ---------------- Defaults ----------------

/// (domain, query) searched in each of the three countries.
const QUERIES: &[(&str, &str)] = &[
    ("sync", "sync manager"),
    ("sync", "music licensing"),
    ("sync", "music supervisor"),
    ("sync", "music publishing"),
    ("label", "label manager"),
    ("label", "artist manager"),
    ("label", "day-to-day manager"),
    ("label", "A&R"),
    ("booking", "booking agent"),
    ("booking", "talent agent music"),
];

/// French wording of the same roles, searched in France only.
const QUERIES_FR: &[(&str, &str)] = &[
    ("sync", "synchronisation musique"),
    ("sync", "éditions musicales"),
    ("label", "chef de projet label"),
    ("label", "manager d'artistes"),
    ("booking", "booker musique"),
];

pub fn seed(conn: &Connection) {
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM job_searches", [], |r| r.get(0)).unwrap_or(0);
    if n == 0 {
        for (domain, q) in QUERIES {
            for country in ["FR", "UK", "US"] {
                let _ = conn.execute(
                    "INSERT OR IGNORE INTO job_searches (domain, query, country) VALUES (?1, ?2, ?3)",
                    params![domain, q, country],
                );
            }
        }
        for (domain, q) in QUERIES_FR {
            let _ = conn.execute(
                "INSERT OR IGNORE INTO job_searches (domain, query, country) VALUES (?1, ?2, 'FR')",
                params![domain, q],
            );
        }
    }
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM job_boards", [], |r| r.get(0)).unwrap_or(0);
    if n == 0 {
        for (platform, token, company) in super::daily::DEFAULT_BOARDS {
            let _ = conn.execute(
                "INSERT OR IGNORE INTO job_boards (platform, token, company) VALUES (?1, ?2, ?3)",
                params![platform, token, company],
            );
        }
    }
}

// ---------------- Text helpers (pure) ----------------

/// Readable text of an HTML fragment: tags out, entities decoded, blank lines
/// collapsed. Good enough for a job description, not a browser.
pub fn html_to_text(html: &str) -> String {
    let mut s = html.to_string();
    // Escaped HTML (Greenhouse `content`) is unescaped first.
    if s.contains("&lt;") && !s.contains('<') {
        s = decode_entities(&s);
    }
    let re_drop = regex::Regex::new(r"(?is)<(script|style|noscript|svg|head)[^>]*>.*?</(script|style|noscript|svg|head)>").unwrap();
    let s = re_drop.replace_all(&s, " ");
    let re_break = regex::Regex::new(r"(?i)<(br|/p|/div|/li|/h[1-6]|/tr|/ul|/ol)[^>]*>").unwrap();
    let s = re_break.replace_all(&s, "\n");
    let re_li = regex::Regex::new(r"(?i)<li[^>]*>").unwrap();
    let s = re_li.replace_all(&s, "- ");
    let re_tag = regex::Regex::new(r"(?s)<[^>]+>").unwrap();
    let s = decode_entities(&re_tag.replace_all(&s, " "));
    let mut out = String::new();
    let mut blank = 0;
    for line in s.lines() {
        let l = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if l.is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        out.push_str(&l);
        out.push('\n');
    }
    out.trim().to_string()
}

pub fn decode_entities(s: &str) -> String {
    let mut out = s
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&rsquo;", "'")
        .replace("&lsquo;", "'")
        .replace("&ldquo;", "\"")
        .replace("&rdquo;", "\"")
        .replace("&ndash;", "-")
        .replace("&mdash;", ", ")
        .replace("&eacute;", "é")
        .replace("&egrave;", "è")
        .replace("&agrave;", "à")
        .replace("&euro;", "€")
        .replace("&pound;", "£");
    let re = regex::Regex::new(r"&#(x?)([0-9a-fA-F]+);").unwrap();
    out = re
        .replace_all(&out, |c: &regex::Captures| {
            let n = if &c[1] == "x" { u32::from_str_radix(&c[2], 16) } else { c[2].parse::<u32>() };
            n.ok().and_then(char::from_u32).map(|ch| ch.to_string()).unwrap_or_default()
        })
        .to_string();
    out.replace("&amp;", "&")
}

fn has_word(hay: &str, needle: &str) -> bool {
    let mut from = 0;
    while let Some(i) = hay[from..].find(needle) {
        let start = from + i;
        let end = start + needle.len();
        let before = hay[..start].chars().next_back().map(|c| c.is_alphanumeric()).unwrap_or(false);
        let after = hay[end..].chars().next().map(|c| c.is_alphanumeric()).unwrap_or(false);
        if !before && !after {
            return true;
        }
        from = end;
    }
    false
}

/// FR, UK or US from a free-text location; None when it is somewhere else or
/// cannot be told (a bare "Remote").
pub fn country_of(location: &str) -> Option<&'static str> {
    let l = location.to_lowercase();
    const FR: &[&str] = &["france", "paris", "lyon", "marseille", "bordeaux", "lille", "nantes", "toulouse", "montreuil",
        "boulogne-billancourt", "neuilly", "île-de-france", "ile-de-france", "strasbourg", "rennes", "montpellier"];
    const UK: &[&str] = &["united kingdom", "uk", "england", "london", "londres", "manchester", "bristol", "brighton",
        "glasgow", "scotland", "leeds", "liverpool", "birmingham", "edinburgh", "cardiff", "wales", "royaume-uni", "gb"];
    const US: &[&str] = &["united states", "usa", "us", "u.s.", "new york", "nyc", "los angeles", "nashville", "miami",
        "san francisco", "chicago", "austin", "atlanta", "brooklyn", "santa monica", "burbank", "hollywood", "culver city",
        "beverly hills", "seattle", "boston", "denver", "états-unis", "etats-unis",
        "ca", "ny", "tn", "fl", "tx", "il", "ga", "wa", "ma", "co", "nj"];
    // Order matters little: the lists do not overlap, except two-letter codes
    // which only count when written in capitals after a comma ("Austin, TX").
    for w in FR {
        if has_word(&l, w) {
            return Some("FR");
        }
    }
    for w in UK.iter().filter(|w| w.len() > 2) {
        if has_word(&l, w) {
            return Some("UK");
        }
    }
    for w in US.iter().filter(|w| w.len() > 2) {
        if has_word(&l, w) {
            return Some("US");
        }
    }
    let codes = regex::Regex::new(r"(?:^|[,(/\s])\s*(UK|GB|US|CA|NY|TN|FL|TX|IL|GA|WA|MA|CO|NJ)\b").unwrap();
    match codes.captures(location).map(|c| c[1].to_string()).as_deref() {
        Some("UK") | Some("GB") => Some("UK"),
        Some(_) => Some("US"),
        None => None,
    }
}

/// Words that make a title worth analysing, per domain. A company board lists
/// every opening (engineers, finance, legal): only these go further.
const TITLE_TERMS: &[(&str, &[&str])] = &[
    ("sync", &["sync", "synch", "licensing", "license", "licence", "music supervis", "publishing", "copyright",
        "catalog", "catalogue", "creative", "clearance", "éditions", "edition", "production music", "library"]),
    ("label", &["label", "a&r", "artist", "artiste", "day-to-day", "day to day", "chef de projet",
        "release", "campaign", "repertoire"]),
    ("booking", &["booking", "booker", "agent", "touring", "tour", "live", "talent", "promoter", "programmat"]),
];

/// Titles that match a term by accident on a company board.
const TITLE_NOISE: &[&str] = &["engineer", "developer", "software", "data scientist", "devops", "accountant", "payroll",
    "finance", "legal counsel", "attorney", "lawyer", "paralegal", "it support", "security", "designer", "ux ",
    "intern", "internship", "stage", "stagiaire", "alternance", "apprenti", "machine learning", "backend", "frontend",
    "full stack", "fullstack", "sre", "qa ", "analyst", "comptable", "juriste"];

/// Domain of a title, or None when the title is not one of the user's roles.
pub fn domain_of_title(title: &str) -> Option<&'static str> {
    let t = format!(" {} ", title.to_lowercase());
    if TITLE_NOISE.iter().any(|n| t.contains(n)) {
        return None;
    }
    TITLE_TERMS.iter().find(|(_, terms)| terms.iter().any(|w| t.contains(w))).map(|(d, _)| *d)
}

fn day(s: &str) -> Option<String> {
    let s = s.trim();
    if s.len() >= 10 && s.as_bytes()[4] == b'-' {
        return Some(s[..10].to_string());
    }
    chrono::DateTime::parse_from_rfc2822(s).ok().map(|d| d.format("%Y-%m-%d").to_string())
}

fn from_millis(ms: i64) -> Option<String> {
    chrono::DateTime::from_timestamp_millis(ms).map(|d| d.format("%Y-%m-%d").to_string())
}

fn txt(v: &Value, path: &[&str]) -> String {
    let mut cur = v;
    for p in path {
        cur = &cur[*p];
    }
    cur.as_str().unwrap_or("").trim().to_string()
}

// ---------------- Parsers (pure) ----------------

/// Keep a board posting only when the title is one of the user's roles and
/// the place is France, the UK or the USA.
fn board_lead(platform: &str, token: &str, company: &str, id: &str, title: &str, location: &str, url: &str,
    posted: Option<String>, description: String) -> Option<Lead> {
    let domain = domain_of_title(title)?;
    let country = country_of(location)?;
    Some(Lead {
        source: platform.to_string(),
        key: format!("{platform}:{token}:{id}"),
        url: url.to_string(),
        title: title.trim().to_string(),
        company: company.to_string(),
        location: location.trim().to_string(),
        country: country.to_string(),
        domain: domain.to_string(),
        posted_at: posted,
        description,
        ..Default::default()
    })
}

pub fn parse_board(platform: &str, token: &str, company: &str, v: &Value) -> Vec<Lead> {
    let empty = Vec::new();
    match platform {
        "greenhouse" => v["jobs"].as_array().unwrap_or(&empty).iter().filter_map(|j| {
            board_lead(platform, token, company, &j["id"].to_string(), &txt(j, &["title"]), &txt(j, &["location", "name"]),
                &txt(j, &["absolute_url"]), day(&txt(j, &["updated_at"])), html_to_text(&txt(j, &["content"])))
        }).collect(),
        "lever" => v.as_array().unwrap_or(&empty).iter().filter_map(|j| {
            let mut d = txt(j, &["descriptionPlain"]);
            for l in j["lists"].as_array().unwrap_or(&empty) {
                d.push_str(&format!("\n\n{}\n{}", txt(l, &["text"]), html_to_text(&txt(l, &["content"]))));
            }
            d.push_str(&format!("\n\n{}", txt(j, &["additionalPlain"])));
            board_lead(platform, token, company, &txt(j, &["id"]), &txt(j, &["text"]), &txt(j, &["categories", "location"]),
                &txt(j, &["hostedUrl"]), j["createdAt"].as_i64().and_then(from_millis), d.trim().to_string())
        }).collect(),
        "ashby" => v["jobs"].as_array().unwrap_or(&empty).iter().filter_map(|j| {
            board_lead(platform, token, company, &txt(j, &["id"]), &txt(j, &["title"]), &txt(j, &["location"]),
                &txt(j, &["jobUrl"]), day(&txt(j, &["publishedAt"])), txt(j, &["descriptionPlain"]))
        }).collect(),
        "smartrecruiters" => v["content"].as_array().unwrap_or(&empty).iter().filter_map(|j| {
            let id = txt(j, &["id"]);
            let loc = [txt(j, &["location", "city"]), txt(j, &["location", "country"]).to_uppercase()]
                .into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(", ");
            let loc = loc.replace(", GB", ", UK");
            let url = format!("https://jobs.smartrecruiters.com/{token}/{id}");
            board_lead(platform, token, company, &id, &txt(j, &["name"]), &loc, &url, day(&txt(j, &["releasedDate"])), String::new())
        }).collect(),
        "workable" => v["jobs"].as_array().unwrap_or(&empty).iter().filter_map(|j| {
            let loc = [txt(j, &["city"]), txt(j, &["country"])].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(", ");
            board_lead(platform, token, company, &txt(j, &["shortcode"]), &txt(j, &["title"]), &loc, &txt(j, &["url"]),
                day(&txt(j, &["published_on"])), html_to_text(&txt(j, &["description"])))
        }).collect(),
        "recruitee" => v["offers"].as_array().unwrap_or(&empty).iter().filter_map(|j| {
            let loc = [txt(j, &["city"]), txt(j, &["country"])].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(", ");
            let d = format!("{}\n\n{}", html_to_text(&txt(j, &["description"])), html_to_text(&txt(j, &["requirements"])));
            board_lead(platform, token, company, &j["id"].to_string(), &txt(j, &["title"]), &loc, &txt(j, &["careers_url"]),
                day(&txt(j, &["created_at"])), d.trim().to_string())
        }).collect(),
        _ => Vec::new(),
    }
}

pub fn board_url(platform: &str, token: &str) -> Option<String> {
    Some(match platform {
        "greenhouse" => format!("https://boards-api.greenhouse.io/v1/boards/{token}/jobs?content=true"),
        "lever" => format!("https://api.lever.co/v0/postings/{token}?mode=json"),
        "ashby" => format!("https://api.ashbyhq.com/posting-api/job-board/{token}"),
        "smartrecruiters" => format!("https://api.smartrecruiters.com/v1/companies/{token}/postings?limit=100"),
        "workable" => format!("https://apply.workable.com/api/v1/widget/accounts/{token}?details=true"),
        "recruitee" => format!("https://{token}.recruitee.com/api/offers/"),
        "rss" => token.to_string(),
        _ => return None,
    })
}

fn tag(item: &str, name: &str) -> String {
    let re = regex::Regex::new(&format!(r"(?is)<{name}(?:\s[^>]*)?>(.*?)</{name}>")).unwrap();
    let raw = re.captures(item).map(|c| c[1].to_string()).unwrap_or_default();
    let raw = raw.trim();
    let raw = raw.strip_prefix("<![CDATA[").and_then(|r| r.strip_suffix("]]>")).unwrap_or(raw);
    raw.trim().to_string()
}

/// Items of an RSS or Atom feed of a job board. The feed is a music job board,
/// so every item is kept when its place is in scope; the company is read from
/// "Role at Company" or "Company: Role" titles when the feed has no field.
pub fn parse_feed(feed_url: &str, board: &str, xml: &str) -> Vec<Lead> {
    let re = regex::Regex::new(r"(?is)<(item|entry)[\s>].*?</(item|entry)>").unwrap();
    re.find_iter(xml).filter_map(|m| {
        let item = m.as_str();
        let title = decode_entities(&tag(item, "title"));
        let mut url = decode_entities(&tag(item, "link"));
        if url.is_empty() {
            url = regex::Regex::new(r#"(?i)<link[^>]*href="([^"]+)""#).unwrap().captures(item).map(|c| c[1].to_string()).unwrap_or_default();
        }
        if title.is_empty() || url.is_empty() {
            return None;
        }
        let body = [tag(item, "content:encoded"), tag(item, "description"), tag(item, "summary"), tag(item, "content")]
            .into_iter().find(|s| !s.is_empty()).unwrap_or_default();
        let description = html_to_text(&decode_entities(&body));
        let mut company = decode_entities(&tag(item, "dc:creator"));
        let mut role = title.clone();
        for sep in [" at ", " @ ", " chez ", " - ", " | ", " – "] {
            if let Some((a, b)) = title.split_once(sep) {
                role = a.trim().to_string();
                if company.is_empty() || company.len() > 60 {
                    company = b.trim().to_string();
                }
                break;
            }
        }
        let location = [tag(item, "location"), tag(item, "job:location"), tag(item, "georss:featurename")]
            .into_iter().find(|s| !s.is_empty()).map(|s| decode_entities(&s)).unwrap_or_default();
        let place = if location.is_empty() { format!("{title} {}", description.chars().take(400).collect::<String>()) } else { location.clone() };
        let country = country_of(&place)?;
        let posted = [tag(item, "pubDate"), tag(item, "published"), tag(item, "updated"), tag(item, "dc:date")]
            .into_iter().find_map(|d| day(&d));
        Some(Lead {
            source: "rss".into(),
            key: format!("rss:{url}"),
            url,
            domain: domain_of_title(&role).unwrap_or("").to_string(),
            title: role,
            company: if company.is_empty() { board.to_string() } else { company },
            location,
            country: country.to_string(),
            posted_at: posted,
            description,
            ..Default::default()
        })
    }).filter(|_| !feed_url.is_empty()).collect()
}

pub fn adzuna_country(country: &str) -> &'static str {
    match country {
        "FR" => "fr",
        "UK" => "gb",
        _ => "us",
    }
}

pub fn parse_adzuna(country: &str, domain: &str, v: &Value) -> Vec<Lead> {
    let empty = Vec::new();
    v["results"].as_array().unwrap_or(&empty).iter().filter_map(|j| {
        let title = html_to_text(&txt(j, &["title"]));
        let id = j["id"].as_str().map(str::to_string).unwrap_or_else(|| j["id"].to_string());
        if title.is_empty() || id == "null" {
            return None;
        }
        let salary = match (j["salary_min"].as_f64(), j["salary_max"].as_f64()) {
            (Some(a), Some(b)) if a > 0.0 && (b - a).abs() > 1.0 => Some(format!("{:.0} à {:.0} {}", a, b, currency(country))),
            (Some(a), _) if a > 0.0 => Some(format!("{:.0} {}", a, currency(country))),
            _ => None,
        };
        let contract = [txt(j, &["contract_time"]), txt(j, &["contract_type"])].into_iter().filter(|s| !s.is_empty())
            .map(|s| s.replace('_', " ")).collect::<Vec<_>>().join(", ");
        Some(Lead {
            source: "adzuna".into(),
            key: format!("adzuna:{id}"),
            url: txt(j, &["redirect_url"]),
            title,
            company: txt(j, &["company", "display_name"]),
            location: txt(j, &["location", "display_name"]),
            country: country.to_string(),
            domain: domain.to_string(),
            contract: (!contract.is_empty()).then_some(contract),
            salary,
            posted_at: day(&txt(j, &["created"])),
            description: html_to_text(&txt(j, &["description"])),
        })
    }).collect()
}

fn currency(country: &str) -> &'static str {
    match country {
        "FR" => "EUR / an",
        "UK" => "GBP / an",
        _ => "USD / an",
    }
}

/// What a page says when the opening is gone, in English and French.
const CLOSED_MARKERS: &[&str] = &[
    "no longer accepting applications", "no longer available", "no longer open", "job has expired", "job ad has expired",
    "posting has expired", "this job is closed", "this position is closed", "position has been filled", "job has been filled",
    "role has been filled", "this vacancy has now closed", "vacancy has closed", "applications are now closed",
    "applications have closed", "application deadline has passed", "this job is not available", "job not found",
    "job posting is no longer", "this listing has expired", "this opportunity has closed", "sorry, this job",
    "offre expirée", "offre n'est plus disponible", "offre n’est plus disponible", "annonce n'est plus disponible",
    "annonce n’est plus disponible", "offre a expiré", "annonce a expiré", "poste a été pourvu", "poste est pourvu",
    "offre pourvue", "offre clôturée", "offre est clôturée", "candidatures sont closes", "n'est plus en ligne", "n’est plus en ligne",
    "cette offre n'existe plus", "cette offre n’existe plus",
];

/// The sentence of a page that says the opening is closed, if any.
pub fn closed_marker(text: &str) -> Option<&'static str> {
    let low = text.to_lowercase();
    CLOSED_MARKERS.iter().copied().find(|m| low.contains(m))
}

/// Pages this app never fetches (their terms forbid automated reading).
pub fn no_fetch(url: &str) -> bool {
    let host = crate::commands::enrich::host_of(url);
    ["linkedin.", "indeed.", "glassdoor."].iter().any(|h| host.contains(h))
}

/// Is the opening behind this address still there? Some(reason) when it is
/// gone (page removed, or the page says so), None when it stands or cannot be
/// told (network error, page that refuses robots): doubt never closes a line.
pub fn gone(http: &Http, url: &str) -> Option<String> {
    if no_fetch(url) {
        return None;
    }
    match http.text(url) {
        Ok(html) => closed_marker(&html_to_text(&html)).map(|m| format!("la page indique « {m} »")),
        Err(e) if e == "HTTP 404" || e == "HTTP 410" => Some("la page a été retirée".into()),
        Err(_) => None,
    }
}

/// Hosts a web search may return that are not a posting to apply to.
const SEARCH_SKIP: &[&str] = &["youtube.", "facebook.", "instagram.", "wikipedia.", "reddit.", "quora.", "pinterest.",
    "tiktok.", "twitter.", "x.com", "linkedin.", "glassdoor.", "salary.", "payscale.", "ziprecruiter.", "indeed.", "talent.com",
    "jooble.", "simplyhired.", "careerbuilder.", "monster.", "coursera.", "udemy.", "berklee.", "careersinmusic.",
    "betterteam.", "zippia."];

pub fn parse_search(country: &str, domain: &str, query: &str, v: &Value) -> Vec<Lead> {
    let empty = Vec::new();
    let words: Vec<String> = query.to_lowercase().split_whitespace().filter(|w| w.len() > 2).map(str::to_string).collect();
    v["organic"].as_array().unwrap_or(&empty).iter().filter_map(|r| {
        let url = txt(r, &["link"]);
        let title = txt(r, &["title"]);
        let snippet = txt(r, &["snippet"]);
        let host = crate::commands::enrich::host_of(&url);
        if url.is_empty() || SEARCH_SKIP.iter().any(|s| host.contains(s)) {
            return None;
        }
        // A posting names the role in its title; guides and salary pages do not
        // pass the next test either.
        let lt = title.to_lowercase();
        if !words.iter().any(|w| lt.contains(w)) {
            return None;
        }
        if ["how to", "what is", "what does", "salary", "salaire", "job description template", "fiche métier", "jobs in", "emplois"]
            .iter().any(|w| lt.contains(w)) {
            return None;
        }
        let (role, company) = split_search_title(&title);
        Some(Lead {
            source: "web".into(),
            key: format!("web:{}", url.split(['?', '#']).next().unwrap_or(&url).trim_end_matches('/')),
            url,
            title: role,
            company,
            location: String::new(),
            country: country.to_string(),
            domain: domain.to_string(),
            posted_at: None,
            description: snippet,
            ..Default::default()
        })
    }).collect()
}

/// "Sync Manager - Acme Music | LinkedIn" → ("Sync Manager", "Acme Music").
pub fn split_search_title(title: &str) -> (String, String) {
    let t = title.replace(" | LinkedIn", "").replace(" - LinkedIn", "");
    if let Some((company, role)) = t.split_once(" hiring ") {
        let role = role.split(" in ").next().unwrap_or(role);
        return (role.trim().to_string(), company.trim().to_string());
    }
    for sep in [" at ", " chez ", " - ", " | ", " – ", " @ "] {
        if let Some((a, b)) = t.split_once(sep) {
            let company = b.split(" - ").next().unwrap_or(&b).split(" | ").next().unwrap_or(&b).trim().to_string();
            return (a.trim().to_string(), company);
        }
    }
    (t.trim().to_string(), String::new())
}

// ---------------- Network ----------------

pub struct Http {
    client: reqwest::blocking::Client,
}

impl Http {
    pub fn new() -> Result<Self, String> {
        let client = reqwest::blocking::Client::builder()
            .user_agent(UA)
            .timeout(Duration::from_secs(20))
            .connect_timeout(Duration::from_secs(8))
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self { client })
    }

    pub fn text(&self, url: &str) -> Result<String, String> {
        let r = self.client.get(url).send().map_err(|e| e.to_string())?;
        if !r.status().is_success() {
            return Err(format!("HTTP {}", r.status().as_u16()));
        }
        let mut body = r.text().map_err(|e| e.to_string())?;
        if body.len() > 4_000_000 {
            let mut cut = 4_000_000;
            while !body.is_char_boundary(cut) {
                cut -= 1;
            }
            body.truncate(cut);
        }
        Ok(body)
    }

    pub fn json(&self, url: &str) -> Result<Value, String> {
        serde_json::from_str(&self.text(url)?).map_err(|e| format!("réponse illisible : {e}"))
    }

    /// Readable text of a web page, None when it cannot be read.
    pub fn page_text(&self, url: &str) -> Option<String> {
        let html = self.text(url).ok()?;
        let t = html_to_text(&html);
        (t.len() > 200).then_some(t)
    }

    pub fn page_html(&self, url: &str) -> Option<String> {
        self.text(url).ok()
    }

    pub fn search(&self, key: &str, q: &str, country: &str, recent: bool) -> Result<Value, String> {
        let gl = match country {
            "FR" => "fr",
            "UK" => "gb",
            _ => "us",
        };
        let mut body = serde_json::json!({ "q": q, "gl": gl, "num": 20 });
        if recent {
            body["tbs"] = "qdr:w".into();
        }
        let r = self
            .client
            .post("https://google.serper.dev/search")
            .header("X-API-KEY", key)
            .json(&body)
            .send()
            .map_err(|e| e.to_string())?;
        if !r.status().is_success() {
            let code = r.status().as_u16();
            crate::commands::enrich::serper_health::failed(Some(code), &r.text().unwrap_or_default());
            return Err(format!("recherche web : HTTP {code}"));
        }
        crate::commands::enrich::serper_health::ok();
        r.json().map_err(|e| e.to_string())
    }
}

#[derive(Default)]
pub struct Collected {
    pub leads: Vec<Lead>,
    pub notes: Vec<String>,
    /// Lines closed because their opening left the company's board.
    pub closed: usize,
}

/// One pass over every active source. Errors of one source never stop the
/// others; they are reported in the run notes.
pub fn collect(conn: &Connection, http: &Http, progress: &mut dyn FnMut(&str)) -> Collected {
    let mut out = Collected::default();

    // Company boards and feeds.
    let boards: Vec<(i64, String, String, String)> = conn
        .prepare("SELECT id, platform, token, company FROM job_boards WHERE active = 1")
        .and_then(|mut st| st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).map(|rows| rows.filter_map(Result::ok).collect()))
        .unwrap_or_default();
    let mut board_errors = 0;
    for (id, platform, token, company) in &boards {
        progress(&format!("{company}"));
        let Some(url) = board_url(platform, token) else { continue };
        let res = if platform == "rss" {
            http.text(&url).map(|xml| parse_feed(&url, company, &xml))
        } else {
            http.json(&url).map(|v| parse_board(platform, token, company, &v))
        };
        match res {
            Ok(leads) => {
                let _ = conn.execute("UPDATE job_boards SET last_ok = datetime('now'), last_error = NULL WHERE id = ?1", params![id]);
                // A board lists its open positions only: a line of ours that
                // left the list is closed.
                if platform != "rss" {
                    let live: std::collections::HashSet<&str> = leads.iter().map(|l| l.key.as_str()).collect();
                    let ours: Vec<(i64, String)> = conn
                        .prepare("SELECT id, source_key FROM job_posts WHERE status IN ('new', 'shortlisted') AND source_key LIKE ?1")
                        .and_then(|mut st| st.query_map(params![format!("{platform}:{token}:%")], |r| Ok((r.get(0)?, r.get(1)?))).map(|rows| rows.filter_map(Result::ok).collect()))
                        .unwrap_or_default();
                    for (pid, key) in ours {
                        if !live.contains(key.as_str()) {
                            super::close_post(conn, pid, "l'offre n'est plus sur le site carrière de la société");
                            out.closed += 1;
                        }
                    }
                }
                out.leads.extend(leads);
            }
            Err(e) => {
                board_errors += 1;
                let _ = conn.execute("UPDATE job_boards SET last_error = ?2 WHERE id = ?1", params![id, e]);
            }
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    if board_errors > 0 {
        out.notes.push(format!("{board_errors} source(s) d'entreprise en erreur (détail dans Réglages)"));
    }

    let searches: Vec<(String, String, String)> = conn
        .prepare("SELECT domain, query, country FROM job_searches WHERE active = 1")
        .and_then(|mut st| st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).map(|rows| rows.filter_map(Result::ok).collect()))
        .unwrap_or_default();

    // Adzuna, with the user's own key.
    match (super::setting(conn, "adzuna_app_id"), super::setting(conn, "adzuna_app_key")) {
        (Some(id), Some(key)) => {
            let mut failed = None;
            for (domain, query, country) in &searches {
                progress(&format!("Adzuna · {query} · {country}"));
                let url = format!(
                    "https://api.adzuna.com/v1/api/jobs/{}/search/1?app_id={}&app_key={}&results_per_page=30&what_phrase={}&max_days_old=7&sort_by=date&content-type=application/json",
                    adzuna_country(country), id.trim(), key.trim(), urlencode(query)
                );
                match http.json(&url) {
                    Ok(v) => out.leads.extend(parse_adzuna(country, domain, &v)),
                    Err(e) => {
                        failed = Some(e);
                        break;
                    }
                }
                std::thread::sleep(Duration::from_millis(1200));
            }
            if let Some(e) = failed {
                out.notes.push(format!("Adzuna : {e}"));
            }
        }
        _ => out.notes.push("Adzuna non réglé (clé gratuite à créer, Réglages de l'onglet Emploi)".into()),
    }

    // Web search, postings of the last week.
    match super::setting(conn, "serper_api_key") {
        Some(key) => {
            for (domain, query, country) in &searches {
                progress(&format!("Web · {query} · {country}"));
                let place = match country.as_str() {
                    "FR" => "France",
                    "UK" => "UK",
                    _ => "USA",
                };
                let q = format!("\"{query}\" (job OR hiring OR \"apply now\" OR recrute OR \"offre d'emploi\") music {place}");
                match http.search(&key, &q, country, true) {
                    Ok(v) => out.leads.extend(parse_search(country, domain, query, &v)),
                    Err(e) => {
                        out.notes.push(e);
                        break;
                    }
                }
                std::thread::sleep(Duration::from_millis(400));
            }
        }
        None => out.notes.push("Recherche web non réglée (clé Serper, Réglages, Intégrations)".into()),
    }
    out
}

pub fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{:02X}", b),
        })
        .collect()
}

/// Insert the leads not seen before. Returns the ids of the new postings.
pub fn store(conn: &Connection, leads: &[Lead]) -> Vec<i64> {
    let mut ids = Vec::new();
    for l in leads {
        if l.title.trim().is_empty() {
            continue;
        }
        // The same opening often shows on several sources: one line per
        // (company, title, country).
        let dup: bool = !l.company.trim().is_empty()
            && conn
                .query_row(
                    "SELECT 1 FROM job_posts WHERE lower(trim(company)) = lower(trim(?1)) AND lower(trim(title)) = lower(trim(?2)) AND country = ?3",
                    params![l.company, l.title, l.country],
                    |_| Ok(true),
                )
                .unwrap_or(false);
        if dup {
            continue;
        }
        let n = conn
            .execute(
                "INSERT OR IGNORE INTO job_posts (source, source_key, url, title, company, location, country, domain, contract,
                    salary, posted_at, description, apply_url)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?3)",
                params![l.source, l.key, l.url, l.title, l.company, l.location, l.country, l.domain, l.contract,
                    l.salary, l.posted_at, l.description],
            )
            .unwrap_or(0);
        if n > 0 {
            ids.push(conn.last_insert_rowid());
        }
    }
    ids
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn countries_from_locations() {
        assert_eq!(country_of("Paris, Île-de-France"), Some("FR"));
        assert_eq!(country_of("London, UK"), Some("UK"));
        assert_eq!(country_of("Los Angeles, CA"), Some("US"));
        assert_eq!(country_of("Austin, TX (hybrid)"), Some("US"));
        assert_eq!(country_of("Miami, USA (sur site)"), Some("US"));
        assert_eq!(country_of("Berlin, Germany"), None);
        assert_eq!(country_of("Remote"), None);
        assert_eq!(country_of("Cologne"), None, "a two-letter code only counts in capitals");
    }

    #[test]
    fn titles_in_scope() {
        assert_eq!(domain_of_title("Senior Sync Manager"), Some("sync"));
        assert_eq!(domain_of_title("Day-to-Day Manager"), Some("label"));
        assert_eq!(domain_of_title("Booking Agent Assistant"), Some("booking"));
        assert_eq!(domain_of_title("Software Engineer, Licensing"), None);
        assert_eq!(domain_of_title("Office Manager"), None);
        assert_eq!(domain_of_title("A&R Intern"), None);
    }

    #[test]
    fn html_becomes_readable_text() {
        let t = html_to_text("<div><h2>Role</h2><p>Pitch &amp; clear music.</p><ul><li>Sync</li><li>Briefs</li></ul><script>x()</script></div>");
        assert_eq!(t, "Role\nPitch & clear music.\n- Sync\n- Briefs");
        assert!(html_to_text("&lt;p&gt;Escaped&lt;/p&gt;").contains("Escaped"));
    }

    #[test]
    fn greenhouse_and_lever_boards_keep_roles_in_scope() {
        let gh = json!({"jobs": [
            {"id": 1, "title": "Sync Licensing Manager", "location": {"name": "London, UK"}, "absolute_url": "https://x/1", "updated_at": "2026-11-10T10:00:00Z", "content": "&lt;p&gt;Clear rights&lt;/p&gt;"},
            {"id": 2, "title": "Backend Engineer", "location": {"name": "London, UK"}, "absolute_url": "https://x/2"},
            {"id": 3, "title": "Label Manager", "location": {"name": "Berlin"}, "absolute_url": "https://x/3"}
        ]});
        let leads = parse_board("greenhouse", "acme", "Acme", &gh);
        assert_eq!(leads.len(), 1);
        assert_eq!((leads[0].key.as_str(), leads[0].country.as_str(), leads[0].domain.as_str()), ("greenhouse:acme:1", "UK", "sync"));
        assert_eq!(leads[0].posted_at.as_deref(), Some("2026-11-10"));
        assert_eq!(leads[0].description, "Clear rights");

        let lv = json!([{"id": "ab", "text": "Artist Manager", "categories": {"location": "New York, NY"}, "hostedUrl": "https://l/ab", "createdAt": 1794000000000i64, "descriptionPlain": "Day to day"}]);
        let leads = parse_board("lever", "mgmt", "Mgmt", &lv);
        assert_eq!((leads[0].title.as_str(), leads[0].country.as_str()), ("Artist Manager", "US"));
    }

    #[test]
    fn feed_items() {
        let xml = r#"<rss><channel><item><title>Sync Manager at Acme Music</title><link>https://jobs.test/1</link>
            <pubDate>Mon, 16 Nov 2026 09:00:00 +0000</pubDate><description><![CDATA[<p>Based in London. Pitch music.</p>]]></description></item>
            <item><title>Tour Manager at Other</title><link>https://jobs.test/2</link><description>Berlin office</description></item></channel></rss>"#;
        let leads = parse_feed("https://jobs.test/feed", "Board", xml);
        assert_eq!(leads.len(), 1);
        assert_eq!((leads[0].title.as_str(), leads[0].company.as_str(), leads[0].country.as_str()), ("Sync Manager", "Acme Music", "UK"));
        assert_eq!(leads[0].posted_at.as_deref(), Some("2026-11-16"));
    }

    #[test]
    fn adzuna_results() {
        let v = json!({"results": [{"id": "77", "title": "<strong>Sync</strong> Manager", "company": {"display_name": "Acme"},
            "location": {"display_name": "Paris"}, "redirect_url": "https://a/77", "created": "2026-11-17T08:00:00Z",
            "salary_min": 40000.0, "salary_max": 50000.0, "contract_time": "full_time", "description": "Pitch…"}]});
        let l = &parse_adzuna("FR", "sync", &v)[0];
        assert_eq!((l.title.as_str(), l.key.as_str()), ("Sync Manager", "adzuna:77"));
        assert_eq!(l.salary.as_deref(), Some("40000 à 50000 EUR / an"));
        assert_eq!(l.contract.as_deref(), Some("full time"));
    }

    #[test]
    fn search_results_drop_guides_and_aggregators() {
        let v = json!({"organic": [
            {"title": "Sync Manager - Acme Music | LinkedIn", "link": "https://uk.linkedin.com/jobs/view/1?x=1", "snippet": "Acme is hiring"},
            {"title": "Sync Manager - Acme Music | Music Week Jobs", "link": "https://jobs.musicweek.test/view/1?x=1", "snippet": "Acme is hiring"},
            {"title": "What does a Sync Manager do?", "link": "https://blog.test/a", "snippet": ""},
            {"title": "Sync Manager jobs", "link": "https://www.indeed.com/q", "snippet": ""}
        ]});
        let leads = parse_search("UK", "sync", "sync manager", &v);
        assert_eq!(leads.len(), 1);
        assert_eq!((leads[0].title.as_str(), leads[0].company.as_str()), ("Sync Manager", "Acme Music"));
        assert_eq!(leads[0].key, "web:https://jobs.musicweek.test/view/1", "a page the app may not read cannot be checked, so it is left out");
        assert_eq!(split_search_title("Kobalt hiring Sync Manager in London | LinkedIn"), ("Sync Manager".into(), "Kobalt".into()));
    }

    #[test]
    fn store_skips_known_and_duplicate_openings() {
        let conn = crate::commands::jobs::test_db();
        let a = Lead { source: "lever".into(), key: "lever:x:1".into(), title: "Sync Manager".into(), company: "Acme".into(), country: "UK".into(), ..Default::default() };
        let b = Lead { key: "web:https://x".into(), source: "web".into(), ..a.clone() };
        assert_eq!(store(&conn, &[a.clone(), b]).len(), 1, "same company, title and country is one opening");
        assert_eq!(store(&conn, &[a]).len(), 0);
    }
}
