//! Venue Intelligence, J5 contact enrichment.
//!
//! Per venue: pull website / phone / address / capacity straight from RA, then
//! crawl the venue's own website (homepage, contact, legal, footer) to extract
//! emails. Legal notices and footers are the best filon for a real booking
//! address. No address is ever invented: only mailto: links and addresses found
//! verbatim in the page are stored, each with its source_url.

use crate::commands::ra;
use crate::db::DbPool;
use crate::AppState;
use rusqlite::params;
use serde::Serialize;
use serde_json::json;
use std::collections::HashSet;
use tauri::{AppHandle, State};

pub(crate) const CRAWL_PATHS: &[&str] = &[
    "", "/contact", "/contact-us", "/contactez-nous", "/contacts", "/contatti", "/contacto",
    "/booking", "/bookings", "/press", "/presse", "/mentions-legales", "/impressum", "/legal",
    "/about", "/info",
];

pub(crate) fn crawl_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36",
        )
        .timeout(std::time::Duration::from_secs(12))
        .redirect(reqwest::redirect::Policy::limited(6))
        .build()
        .expect("crawl client")
}

pub(crate) fn is_false_positive(email: &str) -> bool {
    const BAD: &[&str] = &[
        "sentry", "wixpress", "example.com", "example.org", "noreply", "no-reply", "@2x",
        "sample", "godaddy", "your-email", "email@", "domain.com", "yourdomain",
    ];
    let e = email.to_lowercase();
    if BAD.iter().any(|b| e.contains(b)) {
        return true;
    }
    [".png", ".jpg", ".jpeg", ".gif", ".webp", ".svg"]
        .iter()
        .any(|ext| e.ends_with(ext))
}

/// (score, role) from the local part before the @.
pub(crate) fn score_email(email: &str, from_mailto: bool) -> (i64, &'static str) {
    let local = email.split('@').next().unwrap_or("").to_lowercase();
    let has = |list: &[&str]| list.iter().any(|p| local == *p || local.starts_with(p));
    let (mut score, role): (i64, &'static str) = if has(&[
        "booking", "bookings", "talent", "talents", "artist", "artists", "programmation",
        "programme", "programming", "prog", "lineup", "music", "musica", "musique", "dj", "demo",
        "demos", "promo", "ar",
    ]) {
        (100, "booking")
    } else if has(&[
        "management", "direction", "manager", "pr", "press", "presse", "prensa", "comunicazione",
        "marketing", "partnership", "partenariat",
    ]) {
        let role = if has(&["press", "presse", "prensa", "pr"]) { "presse" } else { "management" };
        (60, role)
    } else if has(&["info", "hello", "contact", "hola", "ciao", "office", "admin"]) {
        (30, "general")
    } else if has(&[
        "reservation", "reservations", "reserva", "prenotazioni", "table", "vip", "guestlist",
        "shop", "job", "jobs", "recrutement", "rh", "lostandfound",
    ]) {
        (5, "reservation")
    } else {
        (20, "autre")
    };
    if from_mailto {
        score += 10;
    }
    (score, role)
}

/// Extract emails and a phone from raw HTML, tagging mailto vs regex.
pub(crate) fn extract_contacts(html: &str) -> (Vec<(String, bool)>, Option<String>) {
    use regex::Regex;
    let mut emails: Vec<(String, bool)> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    // mailto: links first, they are always real addresses.
    for cap in html.split("mailto:").skip(1) {
        let addr: String = cap
            .chars()
            .take_while(|c| !matches!(c, '"' | '\'' | '?' | '>' | ' ' | '<' | ')' | '&' | '\\'))
            .collect();
        // A JSON-escaped page ends the address on a backslash (`mailto:a@b.com\"`):
        // stop there and clean, or `a@b.com\` gets stored as a distinct address.
        let Some(addr) = clean_email(&addr) else { continue };
        if seen.insert(addr.clone()) {
            emails.push((addr, true));
        }
    }
    // then plain-text addresses.
    let re = Regex::new(r"[A-Za-z0-9._%+\-]+@[A-Za-z0-9.\-]+\.[A-Za-z]{2,}").unwrap();
    for m in re.find_iter(html) {
        // `\u003einfo@x.com` in an inline JSON blob matches as `u003einfo@x.com`.
        let Some(addr) = clean_email(m.as_str()) else { continue };
        if seen.insert(addr.clone()) {
            emails.push((addr, false));
        }
    }

    // one phone from tel: if present (numbers keep their spaces). Anything that
    // is not a phone number (JS after a "tel:" inside a script, an escaped
    // quote) is dropped rather than stored as one.
    let phone = html.split("tel:").skip(1).find_map(|c| {
        clean_phone(
            &c.chars()
                .take_while(|ch| !matches!(ch, '"' | '\'' | '>' | '<'))
                .collect::<String>(),
        )
    });

    (emails, phone)
}

/// A plausible phone number (digits, spaces, + - . ( ) /, 6 to 20 digits), with
/// JSON-escape debris trimmed; None otherwise.
pub(crate) fn clean_phone(raw: &str) -> Option<String> {
    let p = raw.trim().trim_end_matches('\\').trim();
    let p = p.strip_prefix("//").unwrap_or(p).trim();
    let digits = p.chars().filter(|c| c.is_ascii_digit()).count();
    let ok_chars = p
        .chars()
        .all(|c| c.is_ascii_digit() || matches!(c, ' ' | '+' | '-' | '.' | '(' | ')' | '/'));
    (ok_chars && (6..=20).contains(&digits)).then(|| p.to_string())
}

pub(crate) fn normalize_site(raw: &str) -> Option<String> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    let s = if s.starts_with("http://") || s.starts_with("https://") {
        s.to_string()
    } else {
        format!("https://{}", s)
    };
    Some(s.trim_end_matches('/').to_string())
}

/// scheme://host origin of a normalized base URL (drops any path).
pub(crate) fn origin_of(base: &str) -> String {
    match base.find("://") {
        Some(i) => {
            let rest = &base[i + 3..];
            let host = rest.split('/').next().unwrap_or(rest);
            format!("{}://{}", &base[..i], host)
        }
        None => base.to_string(),
    }
}

/// Candidate bases to try: the URL as-is plus the www<->apex swap.
pub(crate) fn base_candidates(base: &str) -> Vec<String> {
    let origin = origin_of(base);
    let mut out = vec![origin.clone()];
    if let Some(i) = origin.find("://") {
        let (scheme, host) = (&origin[..i], &origin[i + 3..]);
        let swapped = if let Some(rest) = host.strip_prefix("www.") {
            rest.to_string()
        } else {
            format!("www.{}", host)
        };
        let cand = format!("{}://{}", scheme, swapped);
        if !out.contains(&cand) {
            out.push(cand);
        }
    }
    out
}

/// Internal links from a homepage that look like contact / legal / about pages.
pub(crate) fn internal_links(html: &str, origin: &str) -> Vec<String> {
    const KW: &[&str] = &[
        "contact", "kontakt", "contatti", "contacto", "mention", "legal", "impressum",
        "about", "info", "book", "nous-contacter",
    ];
    let mut out: Vec<String> = Vec::new();
    for part in html.split("href=").skip(1) {
        let part = part.trim_start_matches(['"', '\'']);
        let href: String = part
            .chars()
            .take_while(|c| !matches!(c, '"' | '\'' | '>' | ' '))
            .collect();
        let low = href.to_lowercase();
        if !KW.iter().any(|k| low.contains(k)) {
            continue;
        }
        let url = if href.starts_with("http") {
            if !href.starts_with(origin) {
                continue; // external host
            }
            href.clone()
        } else if href.starts_with('/') {
            format!("{}{}", origin, href)
        } else {
            continue;
        };
        let url = url.trim_end_matches('/').to_string();
        if !out.contains(&url) && out.len() < 8 {
            out.push(url);
        }
    }
    out
}

// ---------------- Source-level email quality ----------------
//
// Before this, enrichment crawled whatever site RA or the search engine handed
// back. When that was a ticketing or listing page (Skiddle, Xceed, Fever...), a
// tourism office or a city hall, every address on it was filed as the venue's
// own: custcare@skiddle.com ended up on 136 venues. The read side filters those
// out, but they should never be stored in the first place.

/// Hosts that are never a venue's (or curator's) own site: ticketing and
/// listings, link hubs, socials, maps and search. Pattern syntax, see `host_hit`:
/// "name." matches that label anywhere in the host, "domain.tld" matches the
/// domain or any subdomain, ".suffix" matches a public suffix like ".gov".
pub(crate) const THIRD_PARTY_HOSTS: &[&str] = &[
    // ticketing / listings / guides
    "skiddle.", "xceed.", "feverup.", "fever.", "ra.co", "residentadvisor.", "dice.fm",
    "eventbrite.", "shotgun.", "songkick.", "bandsintown.", "nightlifeticket.", "festscanner.",
    "ticketmaster.", "seetickets.", "eventim.", "fnacspectacles.", "billetweb.", "weezevent.",
    "ticketswap.", "tixr.", "wegow.", "entradium.", "partyflock.", "allevents.", "clubbable.",
    "designmynight.", "timeout.", "tripadvisor.", "yelp.", "foursquare.", "lefooding.",
    "sceneseeker.", "musicofourdesire.", "goout.", "residentadvisor",
    // link hubs (their footer addresses belong to the hub, not the venue)
    "linktr.ee", "linktree.", "beacons.ai",
    // socials, platforms, search, maps
    "facebook.", "fb.com", "fb.me", "instagram.", "twitter.", "x.com", "tiktok.", "youtube.",
    "youtu.be", "soundcloud.", "spotify.", "mixcloud.", "wikipedia.", "google.", "goo.gl",
    "maps.", "apple.com", "bing.", "yandex.", "meetup.", "gmaps",
    // public bodies (city halls, ministries, the EU)
    ".gov", ".gouv.fr", ".gob.es", ".gv.at", ".admin.ch", "europa.eu", "irisnet.be",
    "comune.", "ayuntamiento.", "ajuntament.", "ville-", "mairie-", "stadt-",
    "esmadrid.com", "madrid-destino.com", "zuerich.com",
];

/// Substrings that mark a tourism office's host (visitberlin.de, turismo.it,
/// barcelonaturisme.com...). Checked on top of THIRD_PARTY_HOSTS.
const TOURISM_MARKERS: &[&str] = &["tourism", "tourisme", "turismo", "turisme", "toerisme"];

/// Lowercased host of a URL or bare domain, without scheme, port, path or "www.".
pub(crate) fn host_of(url: &str) -> String {
    let s = url.trim().to_lowercase();
    let s = s.split("://").nth(1).unwrap_or(&s).to_string();
    let host = s.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit('@').next().unwrap_or(host); // user@host
    let host = host.split(':').next().unwrap_or(host);
    host.trim_start_matches("www.").trim_end_matches('.').to_string()
}

/// One skip-list pattern against a host (see THIRD_PARTY_HOSTS for the syntax).
/// "x.com" therefore matches x.com and m.x.com but not netflix.com, and "ra.co"
/// not sierra.co.
pub(crate) fn host_hit(host: &str, pat: &str) -> bool {
    if let Some(sfx) = pat.strip_prefix('.') {
        // public suffix: ".gov" matches agency.gov and ncsa.gov.qa
        host.ends_with(pat) || host.contains(&format!(".{sfx}."))
    } else if pat.ends_with('.') || pat.ends_with('-') {
        host.starts_with(pat) || host.contains(&format!(".{pat}"))
    } else {
        host == pat || host.ends_with(&format!(".{pat}"))
    }
}

/// True when the host is in `list` or is a public body / tourism office.
pub(crate) fn host_in(host: &str, list: &[&str]) -> bool {
    if list.iter().any(|p| host_hit(host, p)) {
        return true;
    }
    if TOURISM_MARKERS.iter().any(|m| host.contains(m)) {
        return true;
    }
    // visitlondon.com, visit.brussels: a label starting with "visit".
    host.split('.').any(|label| label.starts_with("visit") && label.len() > 5 || label == "visit")
}

/// A venue site we must not crawl as if it were the venue's own.
pub(crate) fn is_third_party_host(host: &str) -> bool {
    host_in(host, THIRD_PARTY_HOSTS)
}

/// Instagram handle from a profile URL ("https://www.instagram.com/b21_brussels?igsh=..").
pub(crate) fn instagram_handle(url: &str) -> Option<String> {
    let low = url.trim().to_lowercase();
    let i = low.find("instagram.com/")?;
    let rest = &low[i + "instagram.com/".len()..];
    let h: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '.')
        .collect();
    let h = h.trim_matches('.').to_string();
    if h.is_empty() || ["p", "reel", "explore", "stories", "accounts"].contains(&h.as_str()) {
        None
    } else {
        Some(h)
    }
}

/// Minimal SQL LIKE matcher ('%' any run, '_' any char) so the Rust write path
/// and the SQL read path share one list of placeholder patterns
/// (radar::PLACEHOLDER_SQL), instead of two lists drifting apart.
fn like_match(pat: &str, s: &str) -> bool {
    let p: Vec<char> = pat.chars().collect();
    let t: Vec<char> = s.chars().collect();
    let (mut i, mut j) = (0usize, 0usize);
    let (mut star, mut mark) = (None::<usize>, 0usize);
    while j < t.len() {
        if i < p.len() && (p[i] == '_' || p[i] == t[j]) {
            i += 1;
            j += 1;
        } else if i < p.len() && p[i] == '%' {
            star = Some(i);
            mark = j;
            i += 1;
        } else if let Some(st) = star {
            i = st + 1;
            mark += 1;
            j = mark;
        } else {
            return false;
        }
    }
    while i < p.len() && p[i] == '%' {
        i += 1;
    }
    i == p.len()
}

/// Template address or scraping artefact (`your@email.com`, `u003einfo@x.com`,
/// a trailing backslash left by JSON escaping): never a real mailbox.
pub(crate) fn is_placeholder_email(email: &str) -> bool {
    let e = email.to_lowercase();
    if e.ends_with('\\') {
        return true;
    }
    super::radar::PLACEHOLDER_SQL
        .iter()
        .any(|p| like_match(p.trim_matches('\''), &e))
}

/// Compliance / support / automated mailbox (privacy@, dsa-authorities@...): a
/// real address, but never a route to someone who books music.
pub(crate) fn is_non_pitch_email(email: &str) -> bool {
    let local = email.split('@').next().unwrap_or("").to_lowercase();
    super::radar::NON_PITCH_LOCALS.iter().any(|l| {
        local == *l
            || local.starts_with(&format!("{l}-"))
            || local.starts_with(&format!("{l}."))
            || local.starts_with(&format!("{l}_"))
    })
}

/// Clean a scraped address: lowercase, drop JSON/HTML escape debris glued on
/// either side (`u003e`, `%20`, a trailing backslash or dot). None when what is
/// left is not a plausible address.
pub(crate) fn clean_email(raw: &str) -> Option<String> {
    let mut e = raw.trim().to_lowercase();
    loop {
        let before = e.len();
        for p in ["u003e", "u003c", "u0022", "u0027", "%20", "%3c", "%3e", "\\", "/", "<", ">"] {
            if let Some(rest) = e.strip_prefix(p) {
                e = rest.to_string();
            }
        }
        e = e
            .trim_end_matches(|c: char| matches!(c, '\\' | '.' | ',' | ';' | ')' | '/' | '>'))
            .to_string();
        if e.len() == before {
            break;
        }
    }
    let (local, domain) = e.split_once('@')?;
    if local.is_empty()
        || domain.contains('@')
        || !domain.contains('.')
        || domain.starts_with('.')
        || !e.is_ascii()
        || e.chars().any(|c| c.is_whitespace() || c == '\\' || c == '"')
    {
        return None;
    }
    let tld = domain.rsplit('.').next().unwrap_or("");
    if tld.len() < 2 || !tld.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    if is_false_positive(&e) {
        return None;
    }
    Some(e)
}

pub(crate) fn email_domain(email: &str) -> &str {
    email.rsplit_once('@').map(|(_, d)| d).unwrap_or("")
}

/// The address sits on the site's own domain (or a sub/parent domain of it).
pub(crate) fn same_site(email_domain: &str, site_host: &str) -> bool {
    let d = email_domain.trim_start_matches("www.");
    let h = site_host.trim_start_matches("www.");
    !d.is_empty()
        && (d == h || h.ends_with(&format!(".{d}")) || d.ends_with(&format!(".{h}")))
}

/// Write-time rule for a venue address. Keep it when it sits on the venue's own
/// site domain; otherwise only when it is not on a third-party domain and no
/// other venue already carries it (a second venue with the same address is how
/// a listing page's footer shows up).
pub(crate) fn keep_venue_email(email: &str, site_host: Option<&str>, seen_elsewhere: bool) -> bool {
    if is_placeholder_email(email) || is_non_pitch_email(email) {
        return false;
    }
    let d = email_domain(email);
    if let Some(h) = site_host.filter(|h| !h.is_empty()) {
        if same_site(d, h) {
            return true;
        }
    }
    if is_third_party_host(d) {
        return false;
    }
    !seen_elsewhere
}

/// Words that say nothing about which venue or curator a page is about.
const NAME_STOPWORDS: &[&str] = &[
    "the", "club", "clubs", "bar", "les", "los", "las", "del", "des", "der", "die", "das", "het",
    "een", "and", "und", "music", "musique", "musica", "records", "recordings", "playlist",
    "playlists", "official", "venue", "nightclub", "disco", "discoteca", "live", "sala", "salle",
    "cafe", "house", "room", "space", "studio", "festival", "radio", "channel", "label", "mix",
];

/// Normalised distinctive tokens of a name ("Le Sucre (Lyon)" -> ["sucre", "lyon"]).
/// Falls back to the whole normalised name when every word is a stopword.
pub(crate) fn name_tokens(name: &str) -> Vec<String> {
    let mut out: Vec<String> = name
        .split(|c: char| !c.is_alphanumeric())
        .map(crate::db::normalise)
        .filter(|t| t.len() >= 3 && !NAME_STOPWORDS.contains(&t.as_str()))
        .collect();
    out.dedup();
    if out.is_empty() {
        let whole = crate::db::normalise(name);
        if whole.len() >= 3 {
            out.push(whole);
        }
    }
    out
}

/// Pick a site out of a Serper response. A result is accepted only when it is
/// not skipped and its domain (first pass) or title (second pass) contains a
/// distinctive token of the name: "the first organic result" used to be a
/// directory page often enough to poison hundreds of rows.
pub(crate) fn pick_site_from_search(d: &serde_json::Value, name: &str, skip: &[&str]) -> Option<String> {
    let tokens = name_tokens(name);
    if tokens.is_empty() {
        return None;
    }
    let has_token = |s: &str| {
        let n = crate::db::normalise(s);
        tokens.iter().any(|t| n.contains(t.as_str()))
    };
    let usable = |link: &str| -> Option<(String, String)> {
        let n = normalize_site(link)?;
        let host = host_of(&n);
        if host.is_empty() || host_in(&host, skip) {
            return None;
        }
        Some((origin_of(&n), host))
    };
    if let Some(kg) = d.get("knowledgeGraph") {
        if let Some((origin, host)) = kg.get("website").and_then(|w| w.as_str()).and_then(usable) {
            let title = kg.get("title").and_then(|t| t.as_str()).unwrap_or("");
            if has_token(&host) || has_token(title) {
                return Some(origin);
            }
        }
    }
    let organic: Vec<(String, String, String)> = d
        .get("organic")
        .and_then(|o| o.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|it| {
                    let link = it.get("link")?.as_str()?;
                    let title = it.get("title").and_then(|t| t.as_str()).unwrap_or("").to_string();
                    let (origin, host) = usable(link)?;
                    Some((origin, host, title))
                })
                .collect()
        })
        .unwrap_or_default();
    organic
        .iter()
        .find(|(_, host, _)| has_token(host))
        .or_else(|| organic.iter().find(|(_, _, title)| has_token(title)))
        .map(|(o, _, _)| o.clone())
}

/// Resolve an official website via the Serper (Google) search API, used only
/// when no site is known. `name` is what the site must be about (see
/// pick_site_from_search) and `skip` the hosts never accepted (THIRD_PARTY_HOSTS
/// for venues, radar's own directory list for curators). Returns an origin.
pub(crate) async fn resolve_website_via_search(
    client: &reqwest::Client,
    api_key: &str,
    query: &str,
    name: &str,
    skip: &[&str],
) -> Option<String> {
    let resp = client
        .post("https://google.serper.dev/search")
        .header("X-API-KEY", api_key)
        .json(&json!({ "q": query, "num": 8 }))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let d: serde_json::Value = resp.json().await.ok()?;
    pick_site_from_search(&d, name, skip)
}

/// Deeper path list for the "retry the venues without an email" pass: the
/// localized contact / legal pages first-pass guessing misses (a German club's
/// address is on /impressum or /kontakt, a Spanish one's on /aviso-legal).
pub(crate) const RETRY_PATHS: &[&str] = &[
    "/contact", "/kontakt", "/booking", "/about", "/impressum", "/contact-us", "/about-us",
    "/imprint", "/en/contact", "/de/kontakt", "/fr/contact", "/es/contacto", "/nl/contact",
    "/it/contatti", "/info", "/legal", "/mentions-legales", "/aviso-legal", "/privacy",
    "/datenschutz", "/colofon", "/team",
];

/// GET a page and return its HTML (capped at 300 KB), or None on any failure or
/// a non-HTML response.
async fn fetch_html(client: &reqwest::Client, url: &str) -> Option<String> {
    let resp = client.get(url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    if !ct.contains("html") && !ct.is_empty() {
        return None;
    }
    let mut body = resp.text().await.ok()?;
    if body.len() > 300_000 {
        // Cut on a char boundary: slicing mid-codepoint panics the worker.
        let mut cut = 300_000;
        while !body.is_char_boundary(cut) {
            cut -= 1;
        }
        body.truncate(cut);
    }
    Some(body)
}

/// Store the addresses found on one page for a venue, applying the write-time
/// rules (keep_venue_email, RGPD exclusions). Returns how many were kept and the
/// best score among them.
fn store_venue_emails(
    conn: &rusqlite::Connection,
    venue_id: i64,
    emails: Vec<(String, bool)>,
    url: &str,
    site_host: Option<&str>,
    method: Option<&str>,
    score_delta: i64,
) -> (i64, i64) {
    let (mut kept, mut best) = (0i64, 0i64);
    for (email, from_mailto) in emails {
        if super::vi::is_excluded(conn, &email) {
            continue;
        }
        let seen_elsewhere: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM vi_contacts WHERE type='email' AND valeur=?1 AND venue_id != ?2)",
                params![email, venue_id],
                |r| r.get(0),
            )
            .unwrap_or(false);
        if !keep_venue_email(&email, site_host, seen_elsewhere) {
            continue;
        }
        let (score, role) = score_email(&email, from_mailto);
        let score = (score + score_delta).max(1);
        let m = method.unwrap_or(if from_mailto { "mailto" } else { "regex" });
        let ins = conn.execute(
            "INSERT OR IGNORE INTO vi_contacts
                (venue_id, type, valeur, role_devine, score, source_url, source_method, confiance, verifie)
             VALUES (?1,'email',?2,?3,?4,?5,?6,?7,0)",
            params![venue_id, email, role, score, url, m, if from_mailto { 1.0 } else { 0.85 }],
        );
        if ins.is_ok() {
            kept += 1;
            best = best.max(score);
        }
    }
    (kept, best)
}

struct CrawlOutcome {
    contact_page: Option<String>,
    kept: i64,
}

/// Crawl a site (homepage + `paths` + contact-ish links found on the homepage)
/// for addresses, at most `budget` fetches. `own_site` says whether the site is
/// the venue's own: only then does an address on its domain bypass the
/// "already on another venue" rule.
#[allow(clippy::too_many_arguments)]
async fn crawl_site(
    client: &reqwest::Client,
    pool: &DbPool,
    venue_id: i64,
    raw: &str,
    paths: &[&str],
    budget: usize,
    own_site: bool,
    method: Option<&str>,
    score_delta: i64,
) -> CrawlOutcome {
    // Pick a base whose homepage actually responds (handles www/apex + stale http).
    let mut base = origin_of(raw);
    let mut home_html = String::new();
    for cand in base_candidates(raw) {
        if let Some(body) = fetch_html(client, &cand).await {
            base = cand;
            home_html = body;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    }
    let host = host_of(&base);
    let site_host = if own_site { Some(host.as_str()) } else { None };

    // Visit list: homepage, the guessed paths, then real links found on the homepage.
    let mut urls: Vec<String> = vec![base.clone()];
    for p in paths {
        let u = format!("{}{}", base, p);
        if !urls.contains(&u) {
            urls.push(u);
        }
    }
    for link in internal_links(&home_html, &base) {
        if !urls.contains(&link) {
            urls.push(link);
        }
    }

    let mut out = CrawlOutcome { contact_page: None, kept: 0 };
    let mut best_score = 0i64;
    let mut fetches = 0usize;
    for url in &urls {
        if fetches >= budget {
            break;
        }
        // Reuse the homepage body we already downloaded instead of refetching it.
        let body = if url == &base {
            if home_html.is_empty() {
                continue;
            }
            home_html.clone()
        } else {
            fetches += 1;
            match fetch_html(client, url).await {
                Some(b) => b,
                None => continue,
            }
        };
        let (emails, phone) = extract_contacts(&body);
        if url != &base && !emails.is_empty() && out.contact_page.is_none() {
            out.contact_page = Some(url.clone());
        }
        let Ok(conn) = pool.get() else { continue };
        let (kept, best) = store_venue_emails(&conn, venue_id, emails, url, site_host, method, score_delta);
        out.kept += kept;
        best_score = best_score.max(best);
        // store a phone from the site only if RA gave none
        if let Some(ph) = phone.filter(|_| own_site) {
            let ph = ph.trim();
            if ph.len() >= 6 {
                let _ = conn.execute(
                    "INSERT OR IGNORE INTO vi_contacts (venue_id, type, valeur, role_devine, score, source_url, source_method, confiance, verifie)
                     SELECT ?1,'telephone',?2,'general',20,?3,'regex',0.8,0
                     WHERE NOT EXISTS (SELECT 1 FROM vi_contacts WHERE venue_id=?1 AND type='telephone')
                       AND (SELECT telephone FROM vi_venues WHERE id=?1) IS NULL",
                    params![venue_id, ph, url],
                );
            }
        }
        drop(conn);
        // enough signal: a booking-grade address plus a contact page, stop early.
        if best_score >= 100 && out.contact_page.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    out
}

fn ra_err(e: impl ToString) -> ra::RaError {
    ra::RaError { message: e.to_string(), status: None, retryable: true }
}

pub async fn process_enrich_task(
    ra_client: &reqwest::Client,
    pool: &DbPool,
    payload: &str,
) -> Result<(), ra::RaError> {
    let v: serde_json::Value = serde_json::from_str(payload)
        .map_err(|e| ra::RaError { message: e.to_string(), status: None, retryable: false })?;
    let venue_id = v["venue_id"].as_i64().unwrap_or(0);
    // Second pass over venues the first one left without an address.
    let retry = v["retry"].as_bool().unwrap_or(false);

    type Row = (Option<i64>, String, Option<String>, Option<String>, Option<String>, bool);
    let (ra_venue_id, nom, ville, stored_site, stored_insta, enriched_before): Row = {
        let conn = pool.get().map_err(ra_err)?;
        conn.query_row(
            "SELECT ra_venue_id, nom, ville, site_web, instagram, enriched_at IS NOT NULL FROM vi_venues WHERE id=?1",
            params![venue_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )
        .map_err(|e| ra::RaError { message: e.to_string(), status: None, retryable: false })?
    };
    let ra_venue_id = ra_venue_id.unwrap_or(0);
    {
        // Excluded since the run was queued: nothing to fetch, nothing to store.
        let conn = pool.get().map_err(ra_err)?;
        if ra_venue_id > 0 && super::vi::is_excluded(&conn, &super::vi::venue_exclusion_key(ra_venue_id)) {
            let _ = conn.execute(
                "UPDATE vi_venues SET enriched_at = datetime('now') WHERE id=?1",
                params![venue_id],
            );
            return Ok(());
        }
    }

    let mut site: Option<String> = stored_site.as_deref().and_then(normalize_site);
    let mut insta: Option<String> = stored_insta.filter(|s| !s.trim().is_empty());

    // 1. RA venue detail (website / phone / address / capacity / blurb). A retry
    //    skips it: the first pass already merged everything RA had.
    if ra_venue_id > 0 && !(retry && enriched_before) {
        if let Ok(d) = ra::fetch_venue_detail(ra_client, ra_venue_id).await {
            let ra_site = d.website.as_deref().and_then(normalize_site);
            // A Facebook page or a ticketing listing is not the venue's site; an
            // Instagram profile is kept for what it is.
            let (own, ig) = match ra_site {
                Some(s) if is_third_party_host(&host_of(&s)) => (None, instagram_handle(&s)),
                other => (other, None),
            };
            let capacite: Option<i64> = d
                .capacity
                .as_deref()
                .and_then(|c| c.chars().filter(|ch| ch.is_ascii_digit()).collect::<String>().parse().ok());
            let phone = d.phone.filter(|p| !p.trim().is_empty());
            if let Ok(conn) = pool.get() {
                let _ = conn.execute(
                    "UPDATE vi_venues SET
                        site_web = COALESCE(site_web, ?2),
                        telephone = COALESCE(telephone, ?3),
                        adresse = COALESCE(adresse, ?4),
                        capacite_est = COALESCE(capacite_est, ?5),
                        notes = COALESCE(NULLIF(notes,''), ?6),
                        instagram = COALESCE(instagram, ?7),
                        updated_at = datetime('now')
                     WHERE id = ?1",
                    params![venue_id, own, phone, d.address, capacite, d.blurb, ig],
                );
            }
            if own.is_some() {
                site = own;
            }
            insta = insta.or(ig);
        }
    }

    // The stored site may itself be a third-party page (1 152 venues had a
    // Facebook, Instagram or ticketing URL there): never crawl it as theirs.
    if let Some(s) = site.clone() {
        if is_third_party_host(&host_of(&s)) {
            if insta.is_none() {
                insta = instagram_handle(&s);
                if let (Some(h), Ok(conn)) = (&insta, pool.get()) {
                    let _ = conn.execute(
                        "UPDATE vi_venues SET instagram = COALESCE(instagram, ?2) WHERE id=?1",
                        params![venue_id, h],
                    );
                }
            }
            site = None;
        }
    }

    // 1b. Still no website? Resolve it with a web search (Serper), if a key is set.
    if site.is_none() {
        let api_key = pool.get().ok().and_then(|conn| {
            // Keychain-backed (see crate::secrets).
            crate::secrets::read_setting(&conn, "serper_api_key").filter(|k| !k.trim().is_empty())
        });
        if let Some(key) = api_key {
            if !nom.trim().is_empty() {
                let query = format!(
                    "{} {} nightclub venue official website",
                    nom,
                    ville.clone().unwrap_or_default()
                );
                if let Some(found) =
                    resolve_website_via_search(&crawl_client(), &key, &query, &nom, THIRD_PARTY_HOSTS).await
                {
                    if let Ok(conn) = pool.get() {
                        let _ = conn.execute(
                            "UPDATE vi_venues SET site_web = COALESCE(site_web, ?2), updated_at = datetime('now') WHERE id=?1",
                            params![venue_id, found],
                        );
                    }
                    site = Some(found);
                }
                // Politeness / rate-limit courtesy toward the search API.
                tokio::time::sleep(std::time::Duration::from_millis(400)).await;
            }
        }
    }

    // 2. Crawl the venue's own site.
    let client = crawl_client();
    let mut contact_page: Option<String> = None;
    let mut kept = 0i64;
    if let Some(raw) = &site {
        let (paths, budget) = if retry { (RETRY_PATHS, 14) } else { (CRAWL_PATHS, 8) };
        let o = crawl_site(&client, pool, venue_id, raw, paths, budget, true, None, 0).await;
        contact_page = o.contact_page;
        kept += o.kept;
    }

    // 3. Retry only: the venue's Instagram bio, then its main RA promoter's site.
    if retry && kept == 0 {
        if let Some(h) = &insta {
            let url = format!("https://www.instagram.com/{}/", h);
            if let Some(body) = fetch_html(&client, &url).await {
                // Only the profile's own meta description: the rest of the page
                // is Instagram's boilerplate (and its support addresses).
                let bio: String = body
                    .split("<meta")
                    .filter(|m| m.contains("description"))
                    .collect::<Vec<_>>()
                    .join(" ");
                let (emails, _) = extract_contacts(&bio);
                if let Ok(conn) = pool.get() {
                    kept += store_venue_emails(&conn, venue_id, emails, &url, None, Some("instagram"), 0).0;
                }
            }
        }
    }
    if retry && kept == 0 && ra_venue_id > 0 {
        let promoter: Option<String> = pool.get().ok().and_then(|conn| {
            conn.query_row(
                "SELECT ra_url FROM vi_promoters WHERE venue_id=?1 AND ra_url IS NOT NULL
                 ORDER BY nb_events DESC LIMIT 1",
                params![venue_id],
                |r| r.get(0),
            )
            .ok()
        });
        if let Some(pid) = promoter.as_deref().and_then(ra::promoter_id_from_url) {
            if let Ok(Some(w)) = ra::fetch_promoter_website(ra_client, pid).await {
                if let Some(ws) = normalize_site(&w).filter(|s| !is_third_party_host(&host_of(s))) {
                    // Not the venue's own site: a promoter running several rooms
                    // must not hand the same inbox to all of them.
                    let o = crawl_site(&client, pool, venue_id, &ws, CRAWL_PATHS, 6, false, Some("promoter"), -15).await;
                    kept += o.kept;
                }
            }
        }
    }
    let _ = kept;

    if let Ok(conn) = pool.get() {
        let _ = conn.execute(
            "UPDATE vi_venues SET page_contact = COALESCE(page_contact, ?2), enriched_at = datetime('now'), updated_at = datetime('now') WHERE id=?1",
            params![venue_id, contact_page],
        );
    }
    Ok(())
}

// ---------------- Commands ----------------

/// Queue an enrichment run over qualified/validated venues. `force` re-runs every
/// venue; `only_missing_email` queues the deeper retry (RETRY_PATHS, Instagram,
/// RA promoter) for venues already enriched that still have no usable address.
/// Errors are stable codes the UI translates ("vi.enrich_none", "vi.retry_none").
#[tauri::command]
pub async fn vi_start_enrich(
    app: AppHandle,
    state: State<'_, AppState>,
    force: Option<bool>,
    only_missing_email: Option<bool>,
) -> Result<i64, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let retry = only_missing_email.unwrap_or(false);
    let mut sql = String::from(
        "SELECT v.id FROM vi_venues v WHERE v.statut IN ('qualifie','valide')
           AND NOT EXISTS(SELECT 1 FROM vi_exclusions x WHERE x.valeur_normalisee = 'ra:' || v.ra_venue_id)",
    );
    if retry {
        sql.push_str(&format!(
            " AND v.enriched_at IS NOT NULL
              AND NOT EXISTS(SELECT 1 FROM vi_contacts c WHERE c.venue_id=v.id AND c.type='email' AND {})",
            super::harvest::own_email_sql("c.valeur")
        ));
    } else if !force.unwrap_or(false) {
        sql.push_str(" AND v.enriched_at IS NULL");
    }
    sql.push_str(" ORDER BY v.score_qualif DESC");
    let venue_ids: Vec<i64> = {
        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| r.get::<_, i64>(0))
            .map_err(|e| e.to_string())?;
        rows.filter_map(|r| r.ok()).collect()
    };
    if venue_ids.is_empty() {
        return Err(if retry { "vi.retry_none" } else { "vi.enrich_none" }.into());
    }

    conn.execute(
        "INSERT INTO vi_runs (type, params, statut, started_at) VALUES ('enrich', ?1, 'en_cours', datetime('now'))",
        params![json!({ "venues": venue_ids.len(), "retry": retry }).to_string()],
    )
    .map_err(|e| e.to_string())?;
    let run_id = conn.last_insert_rowid();

    let mut stmt = conn
        .prepare("INSERT INTO vi_tasks (run_id, type, payload, statut) VALUES (?1,'enrich_venue',?2,'en_attente')")
        .map_err(|e| e.to_string())?;
    for vid in &venue_ids {
        stmt.execute(params![run_id, json!({ "venue_id": vid, "retry": retry }).to_string()])
            .map_err(|e| e.to_string())?;
    }
    drop(stmt);

    crate::commands::harvest::spawn_worker(state.pool.clone(), state.cancels.clone(), app, run_id);
    Ok(run_id)
}

// ---------------- Venue fiche ----------------

#[derive(Serialize)]
pub struct ViContactRow {
    pub id: i64,
    pub type_: String,
    pub valeur: String,
    pub role_devine: Option<String>,
    pub score: i64,
    pub source_url: Option<String>,
    pub source_method: Option<String>,
    pub verifie: bool,
}

#[derive(Serialize)]
pub struct ViEvidenceRow {
    pub artiste: String,
    pub artiste_tier: Option<i64>,
    pub date_event: String,
    pub titre_event: Option<String>,
    pub source_url: String,
}

#[derive(Serialize)]
pub struct ViPromoterRow {
    pub nom: String,
    pub nb_events: i64,
}

#[derive(Serialize)]
pub struct ViVenueFiche {
    pub id: i64,
    pub nom: String,
    pub ville: Option<String>,
    pub pays: Option<String>,
    pub adresse: Option<String>,
    pub capacite_est: Option<i64>,
    pub site_web: Option<String>,
    pub page_contact: Option<String>,
    pub telephone: Option<String>,
    pub ra_url: Option<String>,
    pub statut: String,
    pub score_qualif: i64,
    pub nb_events_periode: i64,
    pub notes: Option<String>,
    pub enriched_at: Option<String>,
    pub instagram: Option<String>,
    /// Excluded from Venue Intelligence (vi_exclusions): archived, never re-harvested.
    pub excluded: bool,
    pub contacts: Vec<ViContactRow>,
    pub evidence: Vec<ViEvidenceRow>,
    pub promoters: Vec<ViPromoterRow>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_and_scores_emails() {
        let html = r#"
            <a href="mailto:booking@club.com">book us</a>
            <p>General: info@club.com or noreply@sentry.io</p>
            <img src="logo@2x.png">
            <a href="tel:+34 971 000 000">call</a>
        "#;
        let (emails, phone) = extract_contacts(html);
        let addrs: Vec<&str> = emails.iter().map(|(e, _)| e.as_str()).collect();
        assert!(addrs.contains(&"booking@club.com"));
        assert!(addrs.contains(&"info@club.com"));
        assert!(!addrs.iter().any(|e| e.contains("sentry"))); // false positive filtered
        assert!(!addrs.iter().any(|e| e.contains("2x.png"))); // image filtered
        assert!(phone.unwrap().contains("971"));

        // booking beats info, and a mailto gets the +10 bonus.
        assert_eq!(score_email("booking@club.com", true).0, 110);
        assert_eq!(score_email("booking@club.com", true).1, "booking");
        assert_eq!(score_email("info@club.com", false).0, 30);
        assert!(score_email("reservations@club.com", false).0 < 30);
    }

    #[test]
    fn base_candidates_swaps_www() {
        let c = base_candidates("https://www.club.com");
        assert_eq!(c[0], "https://www.club.com");
        assert_eq!(c[1], "https://club.com");
        let c2 = base_candidates("http://club.fr/path");
        assert_eq!(c2[0], "http://club.fr"); // path dropped
        assert_eq!(c2[1], "http://www.club.fr");
    }

    #[test]
    fn host_of_strips_scheme_www_path_port() {
        assert_eq!(host_of("https://www.Club.com/contact?x=1"), "club.com");
        assert_eq!(host_of("club.fr"), "club.fr");
        assert_eq!(host_of("http://sub.club.co.uk:8080/a"), "sub.club.co.uk");
    }

    #[test]
    fn third_party_hosts_are_skipped() {
        for h in [
            "skiddle.com", "www.xceed.me", "feverup.com", "ra.co", "de.ra.co", "dice.fm",
            "eventbrite.co.uk", "shotgun.live", "linktr.ee", "facebook.com", "m.facebook.com",
            "instagram.com", "maps.google.com", "nightlifeticket.nl", "festscanner.com",
            "ncsa.gov.qa", "agency.gov", "molenbeek.irisnet.be", "europarl.europa.eu",
            "esmadrid.com", "zuerich.com", "visitberlin.de", "barcelonaturisme.com",
            "turismo.roma.it",
        ] {
            assert!(is_third_party_host(h), "{h} should be third-party");
        }
        for h in ["berghain.berlin", "fabriclondon.com", "sierra.co", "netflix.com", "theavelive.com", "club.wixsite.com"] {
            assert!(!is_third_party_host(h), "{h} should be the venue's own");
        }
    }

    #[test]
    fn host_patterns() {
        assert!(host_hit("x.com", "x.com"));
        assert!(host_hit("m.x.com", "x.com"));
        assert!(!host_hit("netflix.com", "x.com"));
        assert!(host_hit("old.reddit.com", "reddit."));
        assert!(host_hit("a.gov.uk", ".gov"));
        assert!(!host_hit("govinda.com", ".gov"));
    }

    #[test]
    fn cleans_scraping_artefacts() {
        assert_eq!(clean_email("info@theavelive.com\\").as_deref(), Some("info@theavelive.com"));
        assert_eq!(clean_email("u003ea.lavanderos@vidy.ch").as_deref(), Some("a.lavanderos@vidy.ch"));
        assert_eq!(clean_email("u003e@vidy.ch"), None);
        assert_eq!(clean_email("%20Booking@Club.com.").as_deref(), Some("booking@club.com"));
        assert_eq!(clean_email("logo@2x.png"), None);
        assert_eq!(clean_email("a@b"), None);
        // a JSON-escaped mailto stops at the backslash
        let (emails, _) = extract_contacts(r#"<a href=\"mailto:info@club.com\">x</a> \u003ebook@club.com"#);
        let v: Vec<&str> = emails.iter().map(|(e, _)| e.as_str()).collect();
        assert!(v.contains(&"info@club.com"));
        assert!(v.contains(&"book@club.com"));
        assert!(!v.iter().any(|e| e.contains('\\') || e.starts_with("u003")));
    }

    #[test]
    fn phones_are_validated() {
        assert_eq!(clean_phone("+41 (0)21 619 45 45\\\\\\").as_deref(), Some("+41 (0)21 619 45 45"));
        assert_eq!(clean_phone("!1,text:!1,time:!1"), None);
        assert_eq!(clean_phone("123"), None);
    }

    #[test]
    fn like_matcher() {
        assert!(like_match("your@%", "your@email.com"));
        assert!(like_match("%@example.com", "a@example.com"));
        assert!(like_match("%@%sentry%", "x@o1.ingest.sentry.io"));
        assert!(!like_match("%@email.com", "a@gmail.com"));
        assert!(like_match("a_c", "abc"));
    }

    #[test]
    fn placeholder_and_non_pitch() {
        assert!(is_placeholder_email("your@email.com"));
        assert!(is_placeholder_email("info@club.com\\"));
        assert!(is_placeholder_email("u003ecommunity@ra.co"));
        assert!(!is_placeholder_email("booking@club.com"));
        assert!(is_non_pitch_email("privacy@club.com"));
        assert!(is_non_pitch_email("dsa-authorities@deezer.com"));
        assert!(!is_non_pitch_email("supporters@club.com"));
    }

    #[test]
    fn venue_email_write_rule() {
        // own domain: kept even when another venue already has it (shared group inbox)
        assert!(keep_venue_email("booking@club.com", Some("club.com"), true));
        assert!(keep_venue_email("booking@club.com", Some("www.club.com"), true));
        assert!(keep_venue_email("info@mail.club.com", Some("club.com"), true));
        // third-party domain: never, even if unseen
        assert!(!keep_venue_email("custcare@skiddle.com", Some("club.com"), false));
        assert!(!keep_venue_email("pointsinfo@ville-ge.ch", None, false));
        // foreign but unique: kept; foreign and already elsewhere: dropped
        assert!(keep_venue_email("clubname@gmail.com", Some("club.com"), false));
        assert!(!keep_venue_email("info@musicofourdesire.com", Some("club.com"), true));
        assert!(!keep_venue_email("someone@agency.io", Some("club.com"), true));
        // artefacts and compliance boxes: never
        assert!(!keep_venue_email("your@email.com", None, false));
        assert!(!keep_venue_email("privacy@club.com", Some("club.com"), false));
    }

    #[test]
    fn name_tokens_drop_noise() {
        assert_eq!(name_tokens("Le Sucre (Lyon)"), vec!["sucre", "lyon"]);
        assert_eq!(name_tokens("The Club"), vec!["theclub"]);
        assert_eq!(name_tokens("Zürich Club Hive"), vec!["zurich", "hive"]);
        assert!(name_tokens("DJ").is_empty());
    }

    #[test]
    fn search_result_must_be_about_the_name() {
        let d = serde_json::json!({
            "organic": [
                { "link": "https://www.skiddle.com/whats-on/fabric", "title": "Fabric tickets | Skiddle" },
                { "link": "https://www.someblog.com/top-10", "title": "Top 10 clubs in London" },
                { "link": "https://www.fabriclondon.com/", "title": "fabric London" }
            ]
        });
        assert_eq!(
            pick_site_from_search(&d, "Fabric", THIRD_PARTY_HOSTS).as_deref(),
            Some("https://www.fabriclondon.com")
        );
        // nothing carries the name: nothing accepted (the old code took the blog)
        let d2 = serde_json::json!({
            "organic": [{ "link": "https://www.someblog.com/top-10", "title": "Top 10 clubs" }]
        });
        assert_eq!(pick_site_from_search(&d2, "Fabric", THIRD_PARTY_HOSTS), None);
        // title match as a second pass
        let d3 = serde_json::json!({
            "organic": [{ "link": "https://www.k-club.de/", "title": "Kantine am Berghain" }]
        });
        assert_eq!(
            pick_site_from_search(&d3, "Kantine", THIRD_PARTY_HOSTS).as_deref(),
            Some("https://www.k-club.de")
        );
        // knowledge graph site is checked like any other result
        let d4 = serde_json::json!({
            "knowledgeGraph": { "title": "Fabric", "website": "https://www.facebook.com/fabric" },
            "organic": []
        });
        assert_eq!(pick_site_from_search(&d4, "Fabric", THIRD_PARTY_HOSTS), None);
        // a caller-specific skip list (radar passes its directories)
        let d5 = serde_json::json!({
            "organic": [{ "link": "https://playlistlookup.com/deep-vibes", "title": "Deep Vibes playlist" }]
        });
        assert_eq!(pick_site_from_search(&d5, "Deep Vibes", &["playlistlookup."]), None);
    }

    #[test]
    fn instagram_handles() {
        assert_eq!(instagram_handle("https://www.instagram.com/b21_brussels?igsh=MX").as_deref(), Some("b21_brussels"));
        assert_eq!(instagram_handle("http://instagram.com/club.name/"), Some("club.name".into()));
        assert_eq!(instagram_handle("https://instagram.com/p/abc"), None);
        assert_eq!(instagram_handle("https://club.com"), None);
    }

    #[test]
    fn finds_internal_contact_links() {
        let html = r#"<a href="/nous-contacter">Contact</a>
            <a href="https://club.com/mentions-legales">Legal</a>
            <a href="https://facebook.com/club">FB</a>
            <a href="/agenda">Events</a>"#;
        let links = internal_links(html, "https://club.com");
        assert!(links.contains(&"https://club.com/nous-contacter".to_string()));
        assert!(links.contains(&"https://club.com/mentions-legales".to_string()));
        assert!(!links.iter().any(|l| l.contains("facebook"))); // external skipped
        assert!(!links.iter().any(|l| l.contains("agenda"))); // no keyword
    }
}

#[tauri::command(async)]
pub fn vi_venue_detail(state: State<'_, AppState>, id: i64) -> Result<ViVenueFiche, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;

    let mut fiche = conn
        .query_row(
            "SELECT id, nom, ville, pays, adresse, capacite_est, site_web, page_contact, telephone,
                    ra_url, statut, score_qualif, nb_events_periode, notes, enriched_at, instagram,
                    EXISTS(SELECT 1 FROM vi_exclusions x WHERE x.valeur_normalisee = 'ra:' || vi_venues.ra_venue_id)
             FROM vi_venues WHERE id=?1",
            params![id],
            |r| {
                Ok(ViVenueFiche {
                    id: r.get(0)?,
                    nom: r.get(1)?,
                    ville: r.get(2)?,
                    pays: r.get(3)?,
                    adresse: r.get(4)?,
                    capacite_est: r.get(5)?,
                    site_web: r.get(6)?,
                    page_contact: r.get(7)?,
                    telephone: r.get(8)?,
                    ra_url: r.get(9)?,
                    statut: r.get(10)?,
                    score_qualif: r.get(11)?,
                    nb_events_periode: r.get(12)?,
                    notes: r.get(13)?,
                    enriched_at: r.get(14)?,
                    instagram: r.get(15)?,
                    excluded: r.get(16)?,
                    contacts: Vec::new(),
                    evidence: Vec::new(),
                    promoters: Vec::new(),
                })
            },
        )
        .map_err(|e| e.to_string())?;

    {
        let mut stmt = conn
            .prepare("SELECT id, type, valeur, role_devine, score, source_url, source_method, verifie FROM vi_contacts WHERE venue_id=?1 ORDER BY score DESC, id")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![id], |r| {
                Ok(ViContactRow {
                    id: r.get(0)?,
                    type_: r.get(1)?,
                    valeur: r.get(2)?,
                    role_devine: r.get(3)?,
                    score: r.get(4)?,
                    source_url: r.get(5)?,
                    source_method: r.get(6)?,
                    verifie: r.get::<_, i64>(7)? != 0,
                })
            })
            .map_err(|e| e.to_string())?;
        for row in rows {
            fiche.contacts.push(row.map_err(|e| e.to_string())?);
        }
    }
    {
        let mut stmt = conn
            .prepare("SELECT artiste, artiste_tier, date_event, titre_event, source_url FROM vi_evidence WHERE venue_id=?1 ORDER BY date_event DESC LIMIT 12")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![id], |r| {
                Ok(ViEvidenceRow {
                    artiste: r.get(0)?,
                    artiste_tier: r.get(1)?,
                    date_event: r.get(2)?,
                    titre_event: r.get(3)?,
                    source_url: r.get(4)?,
                })
            })
            .map_err(|e| e.to_string())?;
        for row in rows {
            fiche.evidence.push(row.map_err(|e| e.to_string())?);
        }
    }
    {
        let mut stmt = conn
            .prepare("SELECT nom, nb_events FROM vi_promoters WHERE venue_id=?1 ORDER BY nb_events DESC LIMIT 8")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![id], |r| {
                Ok(ViPromoterRow { nom: r.get(0)?, nb_events: r.get(1)? })
            })
            .map_err(|e| e.to_string())?;
        for row in rows {
            fiche.promoters.push(row.map_err(|e| e.to_string())?);
        }
    }

    Ok(fiche)
}
