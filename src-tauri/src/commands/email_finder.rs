//! Residency "find missing emails": the clubs of a campaign zone that the
//! target builder leaves out for want of an address, crawled once more with
//! what first-pass enrichment (enrich.rs) misses.
//!
//! On top of enrich's mailto / plain-text extraction: HTML-entity encoded
//! addresses, Cloudflare email protection, "booking [at] club [dot] fr"
//! spellings and `<span>@</span>` splits, the RA blurb already stored, the
//! venue's Instagram bio, and the role an address plays from where it sits on
//! the page (an info@ printed under "Billetterie" is the ticket desk's). Every
//! find goes through enrich's write-time rules (deny list, placeholders, RGPD
//! exclusions, "already on another venue") and is kept only when its domain has
//! an MX record (deliverability::dns). Nothing is ever invented.
//!
//! Politeness: one request at a time (a single worker thread), at least
//! HOST_GAP between two hits on the same host, short timeouts, the same browser
//! User-Agent as enrichment, at most MAX_PAGES fetches per venue.

use crate::commands::deliverability::dns::{mx_hosts, DohResolver, RType, Resolver};
use crate::commands::enrich;
use crate::commands::residency::targeting::{self, email_rank, BuildOptions};
use crate::AppState;
use regex::Regex;
use rusqlite::{params, Connection};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, State};

/// Fetches per venue for its own site (homepage and www/apex retries included).
pub const MAX_PAGES: usize = 10;
/// Least time between two requests to the same host.
const HOST_GAP: Duration = Duration::from_millis(900);
/// A venue the pass went over is not crawled again before this many days,
/// unless forced: sites change slowly.
pub const FIND_COOLDOWN_DAYS: i64 = 14;

/// Guessed pages, after the contact-ish links actually found on the homepage.
const FINDER_PATHS: &[&str] = &[
    "/contact", "/booking", "/programmation", "/contact-us", "/contactez-nous", "/nous-contacter", "/infos",
    "/infos-pratiques", "/about", "/a-propos", "/mentions-legales", "/impressum", "/kontakt", "/legal", "/imprint",
    "/en/contact", "/fr/contact",
];

// ---------------- Network seams ----------------

/// One page GET. Behind a trait so tests run on fixtures, never the network.
pub trait Fetcher {
    /// HTML of the page, None on any failure or a non-HTML answer.
    fn get(&mut self, url: &str) -> Option<String>;
}

pub struct HttpFetcher {
    client: reqwest::blocking::Client,
    last_hit: HashMap<String, Instant>,
}

impl HttpFetcher {
    pub fn new() -> Result<Self, String> {
        let client = reqwest::blocking::Client::builder()
            .user_agent(enrich::CRAWL_UA)
            .timeout(Duration::from_secs(8))
            .connect_timeout(Duration::from_secs(5))
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self { client, last_hit: HashMap::new() })
    }
}

impl Fetcher for HttpFetcher {
    fn get(&mut self, url: &str) -> Option<String> {
        let host = enrich::host_of(url);
        if let Some(t) = self.last_hit.get(&host) {
            let since = t.elapsed();
            if since < HOST_GAP {
                std::thread::sleep(HOST_GAP - since);
            }
        }
        let out = fetch_html_blocking(&self.client, url);
        self.last_hit.insert(host, Instant::now());
        out
    }
}

/// enrich::fetch_html for the worker thread: 2xx HTML only, capped at 300 KB.
fn fetch_html_blocking(client: &reqwest::blocking::Client, url: &str) -> Option<String> {
    let resp = client.get(url).send().ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let ct = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    if !ct.contains("html") && !ct.is_empty() {
        return None;
    }
    let mut body = resp.text().ok()?;
    if body.len() > 300_000 {
        let mut cut = 300_000;
        while !body.is_char_boundary(cut) {
            cut -= 1;
        }
        body.truncate(cut);
    }
    Some(body)
}

/// MX lookups, one per domain for the whole run.
pub struct MxCheck<'a> {
    resolver: &'a dyn Resolver,
    cache: HashMap<String, Option<bool>>,
}

impl<'a> MxCheck<'a> {
    pub fn new(resolver: &'a dyn Resolver) -> Self {
        Self { resolver, cache: HashMap::new() }
    }

    /// Some(true) when the domain can receive mail, Some(false) when it has no
    /// MX (or a null MX), None when the lookup itself failed.
    pub fn has_mx(&mut self, domain: &str) -> Option<bool> {
        let d = domain.trim().trim_end_matches('.').to_lowercase();
        if let Some(v) = self.cache.get(&d) {
            return *v;
        }
        let v = self.resolver.lookup(&d, RType::Mx).ok().map(|a| !mx_hosts(&a).is_empty());
        self.cache.insert(d, v);
        v
    }
}

// ---------------- Extraction (pure) ----------------

#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub email: String,
    /// "mailto" | "regex" | "obfuscated" | "cfemail"
    pub method: &'static str,
    /// Role read from the words just before the address (context_role).
    pub role: Option<&'static str>,
}

fn re(cell: &'static OnceLock<Regex>, pat: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pat).unwrap())
}

/// Numeric entities and the named ones obfuscators use for "@" and ".":
/// `&#98;&#111;...`, `&#x40;`, `&commat;`, `&period;`, plus `%40`.
pub fn decode_entities(html: &str) -> String {
    static NUM: OnceLock<Regex> = OnceLock::new();
    let s = re(&NUM, r"&#([xX]?)([0-9a-fA-F]{1,6});").replace_all(html, |c: &regex::Captures| {
        let n = if c[1].is_empty() { c[2].parse::<u32>().ok() } else { u32::from_str_radix(&c[2], 16).ok() };
        n.and_then(char::from_u32).map(|ch| ch.to_string()).unwrap_or_default()
    });
    s.replace("&commat;", "@")
        .replace("&period;", ".")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("%40", "@")
}

/// Visible text of a page: scripts and styles dropped, every tag a space (so
/// `booking<span>@</span>club.fr` reads "booking @ club.fr"), lowercased.
pub fn page_text(html: &str) -> String {
    static SCRIPT: OnceLock<Regex> = OnceLock::new();
    static TAG: OnceLock<Regex> = OnceLock::new();
    static WS: OnceLock<Regex> = OnceLock::new();
    let s = re(&SCRIPT, r"(?is)<(script|style|noscript)\b.*?</(script|style|noscript)\s*>").replace_all(html, " ");
    let s = re(&TAG, r"<[^>]*>").replace_all(&s, " ");
    re(&WS, r"\s+").replace_all(&s, " ").to_lowercase()
}

/// Cloudflare's email protection: `data-cfemail="hex"` or
/// `/cdn-cgi/l/email-protection#hex`, the first byte XOR-ing the rest.
pub fn cf_decode(hex: &str) -> Option<String> {
    if hex.len() < 4 || hex.len() % 2 != 0 {
        return None;
    }
    let bytes: Vec<u8> = (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok()).collect::<Option<_>>()?;
    let key = bytes[0];
    String::from_utf8(bytes[1..].iter().map(|b| b ^ key).collect()).ok()
}

fn cf_emails(html: &str) -> Vec<String> {
    static CF: OnceLock<Regex> = OnceLock::new();
    re(&CF, r#"(?:data-cfemail=["']|email-protection#)([0-9a-fA-F]{4,})"#)
        .captures_iter(html)
        .filter_map(|c| cf_decode(&c[1]))
        .filter_map(|e| enrich::clean_email(&e))
        .collect()
}

/// "booking [at] club [dot] fr", "prog(at)club.fr", "info @ club.fr",
/// "booking at club dot com". A bare word "at" (English prose: "meet us at
/// club.com") only counts with a spelled-out "dot" too.
pub fn deobfuscate(text: &str) -> Vec<String> {
    const LOCAL: &str = r"[a-z0-9][a-z0-9._%+\-]*";
    const LABEL: &str = r"[a-z0-9][a-z0-9\-]*";
    const STRONG_AT: &str = r"(?:\s*[\[\(\{]\s*(?:at|@|arobase|chez)\s*[\]\)\}]\s*|\s+@\s*|\s*@\s+)";
    const WEAK_AT: &str = r"\s+(?:at|arobase)\s+";
    const WORD_DOT: &str = r"(?:\s*[\[\(\{]\s*(?:dot|point|punkt|\.)\s*[\]\)\}]\s*|\s+(?:dot|point|punkt)\s+)";
    static STRONG: OnceLock<Regex> = OnceLock::new();
    static WEAK: OnceLock<Regex> = OnceLock::new();
    static AT_NORM: OnceLock<Regex> = OnceLock::new();
    static DOT_NORM: OnceLock<Regex> = OnceLock::new();
    let strong = re(&STRONG, &format!(r"(?i)\b{LOCAL}{STRONG_AT}{LABEL}(?:(?:{WORD_DOT}|\.){LABEL})+"));
    let weak = re(&WEAK, &format!(r"(?i)\b{LOCAL}{WEAK_AT}{LABEL}(?:{WORD_DOT}{LABEL})+"));
    let at_norm = re(&AT_NORM, &format!(r"(?i){STRONG_AT}|{WEAK_AT}"));
    let dot_norm = re(&DOT_NORM, &format!(r"(?i){WORD_DOT}"));
    let mut out: Vec<String> = Vec::new();
    for m in strong.find_iter(text).chain(weak.find_iter(text)) {
        let s = at_norm.replace(m.as_str(), "@");
        let s = dot_norm.replace_all(&s, ".");
        let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
        if let Some(e) = enrich::clean_email(&s) {
            if !out.contains(&e) {
                out.push(e);
            }
        }
    }
    out
}

const TICKETING_WORDS: &[&str] = &[
    "billetterie", "billeterie", "ticket", "tickets", "ticketing", "ticketshop", "boxoffice", "kassa", "vorverkauf",
    "shop", "webshop", "boutique", "merch", "jobs", "job", "emploi", "recrutement", "candidature", "candidatures",
    "careers", "bewerbung", "vacatures", "privatisation", "privatisations", "privatiser", "rental", "rentals",
    "verhuur", "vermietung", "reservation", "reservations", "réservation", "réservations", "vestiaire",
];
const PRESS_WORDS: &[&str] = &["presse", "press", "media", "médias", "medias"];

/// Role the page gives an address: the words between the previous address (or
/// 90 characters back) and this one. "reservation" for a ticket desk, a shop,
/// jobs or room hire, "presse" for press; None otherwise, and always None for
/// a booking address.
pub fn context_role(text: &str, email: &str) -> Option<&'static str> {
    if email_rank(email, None) == 0 {
        return None;
    }
    let pos = text.find(email)?;
    let mut start = pos.saturating_sub(90);
    while !text.is_char_boundary(start) {
        start -= 1;
    }
    let mut window = &text[start..pos];
    if let Some(at) = window.rfind('@') {
        let rest = &window[at..];
        window = rest.find(char::is_whitespace).map(|w| &rest[w..]).unwrap_or("");
    }
    let words: Vec<&str> = window.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    if words.iter().any(|w| TICKETING_WORDS.contains(w)) || window.contains("box office") {
        return Some("reservation");
    }
    if words.iter().any(|w| PRESS_WORDS.contains(w)) {
        return Some("presse");
    }
    None
}

/// Every address on a page, each with how it was found and its context role.
pub fn find_emails(html: &str) -> Vec<Found> {
    let decoded = decode_entities(html);
    let text = page_text(&decoded);
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<Found> = Vec::new();
    let mut push = |email: String, method: &'static str, out: &mut Vec<Found>| {
        if seen.insert(email.clone()) {
            let role = context_role(&text, &email);
            out.push(Found { email, method, role });
        }
    };
    for (e, mailto) in enrich::extract_contacts(html).0 {
        push(e, if mailto { "mailto" } else { "regex" }, &mut out);
    }
    if decoded != html {
        for (e, _) in enrich::extract_contacts(&decoded).0 {
            push(e, "obfuscated", &mut out);
        }
    }
    for e in cf_emails(html) {
        push(e, "cfemail", &mut out);
    }
    for e in deobfuscate(&text) {
        push(e, "obfuscated", &mut out);
    }
    out
}

/// (score, role) of a find: enrich::score_email, lowered by its context role.
pub fn score_found(f: &Found) -> (i64, &'static str) {
    let (score, role) = enrich::score_email(&f.email, matches!(f.method, "mailto" | "cfemail"));
    match f.role {
        Some("reservation") if role != "booking" => (score.min(5), "reservation"),
        Some("presse") if role != "booking" => (score.min(60), "presse"),
        _ => (score, role),
    }
}

// ---------------- One venue ----------------

#[derive(Debug, Clone, Default)]
pub struct VenueJob {
    pub id: i64,
    pub nom: String,
    pub ville: Option<String>,
    pub site_web: Option<String>,
    pub page_contact: Option<String>,
    pub instagram: Option<String>,
    /// RA blurb stored by enrichment (vi_venues.notes).
    pub notes: Option<String>,
    pub ra_url: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct VenueOutcome {
    pub kept: i64,
    pub dropped_no_mx: i64,
    pub fetches: usize,
    /// A site resolved through the web search (stored as site_web).
    pub site_found: Option<String>,
    /// First page other than the homepage an address was kept from.
    pub contact_page: Option<String>,
    /// A booking-grade address (rank 0) was kept.
    pub booking: bool,
}

/// Web search for a venue's site (name, city) -> origin. Prod: Serper.
pub type SiteSearch<'a> = &'a dyn Fn(&str, Option<&str>) -> Option<String>;

/// Store the finds of one page through the write-time rules and the MX filter.
#[allow(clippy::too_many_arguments)]
fn store_found(
    conn: &Connection,
    venue_id: i64,
    found: Vec<Found>,
    url: &str,
    site_host: Option<&str>,
    searched: Option<(&str, Option<&str>)>,
    method: Option<&'static str>,
    mx: &mut MxCheck,
    out: &mut VenueOutcome,
) -> i64 {
    let mut kept = 0;
    for f in found {
        if !enrich::venue_email_allowed(conn, venue_id, &f.email, site_host, searched) {
            continue;
        }
        match mx.has_mx(enrich::email_domain(&f.email)) {
            Some(true) => {}
            Some(false) => {
                out.dropped_no_mx += 1;
                continue;
            }
            // Unverified (resolver down): not kept, the next run retries it.
            None => continue,
        }
        let (score, role) = score_found(&f);
        let confiance = match f.method {
            "mailto" | "cfemail" => 1.0,
            "regex" => 0.85,
            _ => 0.8,
        };
        let ins = conn
            .execute(
                "INSERT OR IGNORE INTO vi_contacts
                    (venue_id, type, valeur, role_devine, score, source_url, source_method, confiance, verifie)
                 VALUES (?1,'email',?2,?3,?4,?5,?6,?7,0)",
                params![venue_id, f.email, role, score, url, method.unwrap_or(f.method), confiance],
            )
            .unwrap_or(0);
        if ins > 0 {
            kept += 1;
            if targeting::email_rank_with_role(&f.email, None, Some(role)) == 0 {
                out.booking = true;
            }
        }
    }
    out.kept += kept;
    kept
}

/// Look for a venue's missing address: the RA blurb on file, its own site
/// (homepage, stored contact page, contact-ish links, FINDER_PATHS; at most
/// MAX_PAGES fetches), then its Instagram bio when nothing turned up.
pub fn find_for_venue(
    conn: &Connection,
    job: &VenueJob,
    fetch: &mut dyn Fetcher,
    mx: &mut MxCheck,
    search: Option<SiteSearch>,
) -> VenueOutcome {
    let mut out = VenueOutcome::default();
    let mut insta = job.instagram.clone().filter(|s| !s.trim().is_empty());

    // The stored site, unless it is a third party's (Facebook, a ticketing page).
    let mut site = job.site_web.as_deref().and_then(enrich::normalize_site);
    if let Some(s) = site.clone() {
        if enrich::is_foreign_venue_site(&s) {
            insta = insta.or_else(|| enrich::instagram_handle(&s));
            site = None;
        }
    }
    let mut searched = false;
    if site.is_none() && !job.nom.trim().is_empty() {
        if let Some(found) = search.and_then(|f| f(&job.nom, job.ville.as_deref())) {
            out.site_found = Some(found.clone());
            site = Some(found);
            searched = true;
        }
    }
    let rule = searched.then_some((job.nom.as_str(), job.ville.as_deref()));
    let own_host = site.as_deref().map(enrich::host_of);

    // 1. The RA blurb already on file (no request).
    if let Some(notes) = job.notes.as_deref().filter(|n| n.contains('@') || n.contains("[at]") || n.contains("(at)")) {
        let src = job.ra_url.clone().unwrap_or_else(|| "ra".into());
        store_found(conn, job.id, find_emails(notes), &src, own_host.as_deref(), rule, Some("ra_notes"), mx, &mut out);
    }

    // 2. The venue's own site.
    if let Some(raw) = &site {
        let mut base = enrich::origin_of(raw);
        let mut home = String::new();
        for cand in enrich::base_candidates(raw) {
            if out.fetches >= MAX_PAGES {
                break;
            }
            out.fetches += 1;
            if let Some(body) = fetch.get(&cand) {
                base = cand;
                home = body;
                break;
            }
        }
        let host = enrich::host_of(&base);
        let mut urls: Vec<String> = Vec::new();
        if let Some(pc) = job.page_contact.as_deref().and_then(enrich::normalize_site) {
            if enrich::same_site(&enrich::host_of(&pc), &host) {
                urls.push(pc);
            }
        }
        urls.extend(enrich::internal_links(&home, &base));
        urls.extend(FINDER_PATHS.iter().map(|p| format!("{base}{p}")));
        let mut done: HashSet<String> = HashSet::from([base.clone()]);
        let mut pages: Vec<(String, String)> = Vec::new();
        if !home.is_empty() {
            pages.push((base.clone(), home));
        }
        let mut queue = urls.into_iter();
        loop {
            // Homepage first (already fetched), then one fetch at a time.
            let (url, body) = match pages.pop() {
                Some(p) => p,
                None => {
                    if out.booking || out.fetches >= MAX_PAGES {
                        break;
                    }
                    let Some(u) = queue.find(|u| !done.contains(u)) else { break };
                    done.insert(u.clone());
                    out.fetches += 1;
                    match fetch.get(&u) {
                        Some(b) => (u, b),
                        None => continue,
                    }
                }
            };
            let kept = store_found(conn, job.id, find_emails(&body), &url, Some(&host), rule, None, mx, &mut out);
            if kept > 0 && url != base && out.contact_page.is_none() {
                out.contact_page = Some(url);
            }
        }
    }

    // 3. Nothing on the site: the Instagram bio (its meta description only, the
    //    rest of the page is Instagram's own boilerplate).
    if out.kept == 0 {
        if let Some(h) = insta {
            let url = format!("https://www.instagram.com/{}/", h.trim_start_matches('@'));
            out.fetches += 1;
            if let Some(body) = fetch.get(&url) {
                let bio: String =
                    body.split("<meta").filter(|m| m.contains("description")).collect::<Vec<_>>().join(" ");
                store_found(conn, job.id, find_emails(&bio), &url, None, None, Some("instagram"), mx, &mut out);
            }
        }
    }
    out
}

fn load_jobs(conn: &Connection, ids: &[i64], force: bool) -> Vec<VenueJob> {
    let mut st = match conn.prepare(&format!(
        "SELECT id, nom, ville, site_web, page_contact, instagram, notes, ra_url FROM vi_venues
         WHERE id = ?1 AND (?2 OR email_find_at IS NULL OR email_find_at < datetime('now', '-{FIND_COOLDOWN_DAYS} days'))"
    )) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    ids.iter()
        .filter_map(|id| {
            st.query_row(params![id, force], |r| {
                Ok(VenueJob {
                    id: r.get(0)?,
                    nom: r.get(1)?,
                    ville: r.get(2)?,
                    site_web: r.get(3)?,
                    page_contact: r.get(4)?,
                    instagram: r.get(5)?,
                    notes: r.get(6)?,
                    ra_url: r.get(7)?,
                })
            })
            .ok()
        })
        .collect()
}

/// Record the pass on the venue: when, the contact page and a searched site.
fn mark_done(conn: &Connection, venue_id: i64, o: &VenueOutcome) {
    let _ = conn.execute(
        "UPDATE vi_venues SET email_find_at = datetime('now'),
            page_contact = COALESCE(page_contact, ?2),
            site_web = CASE WHEN ?3 IS NULL THEN site_web
                            WHEN site_web IS NULL OR trim(site_web) = '' THEN ?3
                            ELSE site_web END,
            updated_at = datetime('now')
         WHERE id = ?1",
        params![venue_id, o.contact_page, o.site_found],
    );
}

// ---------------- Command ----------------

static RUNNING: AtomicBool = AtomicBool::new(false);

struct RunGuard;
impl Drop for RunGuard {
    fn drop(&mut self) {
        RUNNING.store(false, Ordering::SeqCst);
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct FinderStart {
    /// Venues the pass goes over.
    pub queued: usize,
    /// Venues left out because the pass went over them in the last FIND_COOLDOWN_DAYS.
    pub skipped_recent: usize,
}

/// Payload of the `residency:email-finder` event, sent after each venue.
#[derive(Debug, Clone, Serialize)]
pub struct FinderProgress {
    pub campaign_id: i64,
    pub done: usize,
    pub total: usize,
    /// Venues that got at least one usable address.
    pub venues_found: usize,
    pub emails_found: i64,
    pub dropped_no_mx: i64,
    pub finished: bool,
}

/// Serper search through the blocking client (the worker is a plain thread).
fn serper_search(key: &str, nom: &str, ville: Option<&str>) -> Option<String> {
    let client = reqwest::blocking::Client::builder()
        .user_agent(enrich::CRAWL_UA)
        .timeout(Duration::from_secs(10))
        .build()
        .ok()?;
    let query = format!("{} {} nightclub venue official website", nom, ville.unwrap_or_default());
    let resp = client
        .post("https://google.serper.dev/search")
        .header("X-API-KEY", key)
        .json(&serde_json::json!({ "q": query, "num": 8 }))
        .send();
    let resp = match resp {
        Ok(r) => r,
        Err(e) => {
            enrich::serper_health::failed(None, &e.to_string());
            return None;
        }
    };
    if !resp.status().is_success() {
        let code = resp.status().as_u16();
        enrich::serper_health::failed(Some(code), &resp.text().unwrap_or_default());
        return None;
    }
    enrich::serper_health::ok();
    let d: serde_json::Value = resp.json().ok()?;
    std::thread::sleep(Duration::from_millis(400));
    enrich::pick_site_from_search(&d, nom, ville, enrich::THIRD_PARTY_HOSTS)
}

/// "Chercher les emails manquants": the clubs this campaign's target leaves out
/// for want of an address (same options as the builder), crawled in the
/// background. Returns at once; progress comes as `residency:email-finder`
/// events, the last one with `finished: true`. `force` ignores the cooldown.
#[tauri::command(async)]
pub fn residency_find_missing_emails(
    app: AppHandle,
    state: State<'_, AppState>,
    campaign_id: i64,
    options: Option<BuildOptions>,
    force: Option<bool>,
) -> Result<FinderStart, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let r = crate::commands::residency::load(&conn, campaign_id)
        .ok_or_else(|| "Campagne résidence introuvable.".to_string())?;
    let built = targeting::build(&conn, campaign_id, &r.config, &options.unwrap_or_default())?;
    let ids = built.no_email_venue_ids;
    let jobs = load_jobs(&conn, &ids, force.unwrap_or(false));
    let start = FinderStart { queued: jobs.len(), skipped_recent: ids.len() - jobs.len() };
    if jobs.is_empty() {
        return Ok(start);
    }
    if RUNNING.swap(true, Ordering::SeqCst) {
        return Err("Une recherche d'emails est déjà en cours.".into());
    }
    let serper = crate::secrets::read_setting(&conn, "serper_api_key").filter(|k| !k.trim().is_empty());
    drop(conn);
    let pool = state.pool.clone();
    std::thread::spawn(move || {
        let _guard = RunGuard;
        let total = jobs.len();
        let mut p = FinderProgress {
            campaign_id,
            done: 0,
            total,
            venues_found: 0,
            emails_found: 0,
            dropped_no_mx: 0,
            finished: false,
        };
        let (Ok(mut fetcher), Ok(resolver)) = (HttpFetcher::new(), DohResolver::new()) else {
            p.finished = true;
            let _ = app.emit("residency:email-finder", p);
            return;
        };
        let mut mx = MxCheck::new(&resolver);
        let search = serper.map(|key| move |nom: &str, ville: Option<&str>| serper_search(&key, nom, ville));
        for job in &jobs {
            if let Ok(conn) = pool.get() {
                let o = find_for_venue(&conn, job, &mut fetcher, &mut mx, search.as_ref().map(|f| f as SiteSearch));
                mark_done(&conn, job.id, &o);
                p.emails_found += o.kept;
                p.dropped_no_mx += o.dropped_no_mx;
                if o.kept > 0 {
                    p.venues_found += 1;
                }
            }
            p.done += 1;
            let _ = app.emit("residency:email-finder", p.clone());
        }
        p.finished = true;
        if let Ok(conn) = pool.get() {
            crate::commands::residency::log(
                &conn,
                Some(campaign_id),
                "info",
                "emails_found",
                &format!(
                    "Recherche d'emails : {} adresse(s) trouvée(s) pour {} club(s) sur {} ({} écartée(s) sans MX)",
                    p.emails_found, p.venues_found, total, p.dropped_no_mx
                ),
            );
        }
        let _ = app.emit("residency:email-finder", p);
    });
    Ok(start)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::residency::testdb;

    /// Pages by URL; records every request so tests can count them.
    struct FakeFetcher {
        pages: HashMap<String, String>,
        hits: Vec<String>,
    }

    impl FakeFetcher {
        fn new(pages: &[(&str, &str)]) -> Self {
            Self { pages: pages.iter().map(|(u, b)| (u.to_string(), b.to_string())).collect(), hits: Vec::new() }
        }
    }

    impl Fetcher for FakeFetcher {
        fn get(&mut self, url: &str) -> Option<String> {
            self.hits.push(url.to_string());
            self.pages.get(url).cloned()
        }
    }

    /// MX answers by domain; anything else has none, "down.test" fails.
    struct FakeResolver(HashMap<String, Vec<String>>);

    impl Resolver for FakeResolver {
        fn lookup(&self, name: &str, rtype: RType) -> Result<Vec<String>, String> {
            assert_eq!(rtype, RType::Mx);
            if name == "down.test" {
                return Err("offline".into());
            }
            Ok(self.0.get(name).cloned().unwrap_or_default())
        }
    }

    fn resolver(domains: &[&str]) -> FakeResolver {
        FakeResolver(domains.iter().map(|d| (d.to_string(), vec![format!("10 mx.{d}.")])).collect())
    }

    fn emails(found: &[Found]) -> Vec<&str> {
        found.iter().map(|f| f.email.as_str()).collect()
    }

    #[test]
    fn mailto_and_plain_text() {
        let f = find_emails(r#"<a href="mailto:Booking@Club.fr?subject=hi">x</a> or write prog@club.fr"#);
        assert_eq!(f[0], Found { email: "booking@club.fr".into(), method: "mailto", role: None });
        assert_eq!(f[1].email, "prog@club.fr");
        assert_eq!(f[1].method, "regex");
    }

    #[test]
    fn obfuscated_forms() {
        let html = r#"
            <p>Booking : booking [at] lesucre [dot] eu</p>
            <p>Programmation: prog(at)club-lille.fr</p>
            <p>Contact : contact<span>@</span>rennes-club.fr</p>
            <p>Write to hello at ubu dot bzh</p>
            <a href="mailto:&#105;&#110;&#102;&#111;&#64;&#108;&#97;&#99;&#97;&#115;&#98;&#97;&#104;&#46;&#102;&#114;">mail</a>
            <p>info&#x40;bikini&#46;net</p>
        "#;
        let f = find_emails(html);
        let v = emails(&f);
        for e in [
            "booking@lesucre.eu", "prog@club-lille.fr", "contact@rennes-club.fr", "hello@ubu.bzh",
            "info@lacasbah.fr", "info@bikini.net",
        ] {
            assert!(v.contains(&e), "{e} missing from {v:?}");
        }
        assert!(f.iter().any(|x| x.email == "booking@lesucre.eu" && x.method == "obfuscated"));
        // prose with a bare "at" and no spelled-out dot is not an address
        assert!(deobfuscate("meet us at club.com tonight").is_empty());
        // a sentence end does not glue the next word onto the domain
        assert_eq!(deobfuscate("booking [at] club.fr. nous"), vec!["booking@club.fr".to_string()]);
    }

    #[test]
    fn cloudflare_protection() {
        // key 0x42 XOR "booking@club.fr"
        let key = 0x42u8;
        let hex: String = std::iter::once(key)
            .chain("booking@club.fr".bytes().map(|b| b ^ key))
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(cf_decode(&hex).as_deref(), Some("booking@club.fr"));
        let f = find_emails(&format!(r#"<a href="/cdn-cgi/l/email-protection#{hex}">[email protected]</a>"#));
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].method, "cfemail");
        assert_eq!(cf_decode("zz"), None);
    }

    #[test]
    fn context_marks_ticket_desk_and_press() {
        let html = "<h3>Billetterie</h3><p>info@abconcerts.be</p>\
                    <h3>Programmation</h3><p>Stéphane, steph@abconcerts.be</p>\
                    <h3>Presse</h3><p>media.contact@abconcerts.be</p>\
                    <h3>Booking</h3><p>booking@abconcerts.be</p>";
        let f = find_emails(html);
        let role = |e: &str| f.iter().find(|x| x.email == e).unwrap().role;
        assert_eq!(role("info@abconcerts.be"), Some("reservation"));
        // the heading before the previous address does not leak onto the next one
        assert_eq!(role("steph@abconcerts.be"), None);
        assert_eq!(role("media.contact@abconcerts.be"), Some("presse"));
        assert_eq!(role("booking@abconcerts.be"), None);
        // "Impressum" is not "press"
        assert_eq!(find_emails("<h2>Impressum</h2> info@club.de")[0].role, None);
        let info = f.iter().find(|x| x.email == "info@abconcerts.be").unwrap();
        assert_eq!(score_found(info), (5, "reservation"));
    }

    #[test]
    fn ranking_prefers_booking_and_drops_ticketing() {
        use targeting::{best_email, email_rank_with_role, is_never_pitch, EmailCand};
        assert_eq!(email_rank("booking@x.fr", None), 0);
        assert_eq!(email_rank("programmation@x.fr", None), 0);
        assert_eq!(email_rank("prog@x.fr", None), 0);
        assert_eq!(email_rank("marie.dupont@x.fr", None), 1);
        assert_eq!(email_rank("info@x.fr", None), 2);
        for e in ["ticketshop@x.be", "billetterie@x.fr", "location@x.fr", "jobs@x.fr", "presse@x.fr", "webshop@x.nl"] {
            assert_eq!(email_rank(e, None), 4, "{e}");
        }
        assert!(is_never_pitch("ticketshop@abconcerts.be"));
        assert!(is_never_pitch("jobs@club.fr"));
        assert!(!is_never_pitch("booking.tickets@club.fr"));
        assert!(!is_never_pitch("presse@club.fr"));
        assert_eq!(email_rank_with_role("info@ab.be", None, Some("reservation")), 4);
        assert_eq!(email_rank_with_role("booking@ab.be", None, Some("reservation")), 0);
        let c = |a: &str, role: Option<&str>| EmailCand { addr: a.into(), score: 30, person: None, role: role.map(String::from) };
        // the ticket desk's info@ loses to the generic contact@, the shop is never picked
        let cands = vec![c("info@ab.be", Some("reservation")), c("contact@ab.be", None), c("ticketshop@ab.be", None)];
        assert_eq!(best_email(&cands, Some("ab.be")).unwrap().addr, "contact@ab.be");
        assert!(best_email(&[c("ticketshop@ab.be", None)], None).is_none());
        // a named programmer beats info@
        let cands = vec![c("info@ab.be", None), c("marie.dupont@ab.be", None)];
        assert_eq!(best_email(&cands, None).unwrap().addr, "marie.dupont@ab.be");
    }

    #[test]
    fn mx_filter_is_cached() {
        let r = resolver(&["club.fr"]);
        let mut mx = MxCheck::new(&r);
        assert_eq!(mx.has_mx("club.fr"), Some(true));
        assert_eq!(mx.has_mx("dead-club.fr"), Some(false));
        assert_eq!(mx.has_mx("down.test"), None);
        // a null MX ("0 .") is no MX
        let null = FakeResolver(HashMap::from([("nullmx.fr".to_string(), vec!["0 .".to_string()])]));
        assert_eq!(MxCheck::new(&null).has_mx("nullmx.fr"), Some(false));
    }

    fn venue(c: &Connection, id: i64, nom: &str, site: Option<&str>) {
        c.execute(
            "INSERT INTO vi_venues (id, nom, nom_normalise, ville, ville_norm, pays, statut, site_web)
             VALUES (?1, ?2, lower(?2), 'Lille', 'lille', 'FR', 'qualifie', ?3)",
            params![id, nom, site],
        )
        .unwrap();
    }

    fn stored(c: &Connection, id: i64) -> Vec<(String, String, String)> {
        let mut st = c
            .prepare("SELECT valeur, role_devine, source_method FROM vi_contacts WHERE venue_id = ?1 AND type = 'email' ORDER BY valeur")
            .unwrap();
        st.query_map(params![id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().flatten().collect()
    }

    #[test]
    fn crawls_a_venue_and_stores_with_rules() {
        let c = testdb::conn();
        venue(&c, 1, "La Bulle", Some("https://www.labulle-lille.fr/"));
        venue(&c, 2, "Other", None);
        c.execute(
            "INSERT INTO vi_contacts (venue_id, type, valeur, score, source_url) VALUES (2, 'email', 'shared@agency.io', 30, 'x')",
            [],
        )
        .unwrap();
        c.execute("INSERT INTO vi_exclusions (valeur_normalisee) VALUES ('erased@labulle-lille.fr')", []).ok();
        let home = r#"<a href="/infos-pratiques">Infos</a> <footer>contact [at] labulle-lille [dot] fr</footer>"#;
        let infos = r#"<h3>Billetterie</h3> billetterie@labulle-lille.fr
            your@email.com shared@agency.io olivia.stock@djmag.com erased@labulle-lille.fr
            prog@dead-domain.fr"#;
        let contact = r#"<a href="mailto:programmation@labulle-lille.fr">Programmation</a>"#;
        let mut f = FakeFetcher::new(&[
            ("https://www.labulle-lille.fr", home),
            ("https://www.labulle-lille.fr/infos-pratiques", infos),
            ("https://www.labulle-lille.fr/contact", contact),
        ]);
        let r = resolver(&["labulle-lille.fr", "agency.io", "djmag.com"]);
        let mut mx = MxCheck::new(&r);
        let job = VenueJob { id: 1, nom: "La Bulle".into(), site_web: Some("https://www.labulle-lille.fr/".into()), ..Default::default() };
        let o = find_for_venue(&c, &job, &mut f, &mut mx, None);
        let s = stored(&c, 1);
        let addrs: Vec<&str> = s.iter().map(|x| x.0.as_str()).collect();
        assert_eq!(addrs, vec!["billetterie@labulle-lille.fr", "contact@labulle-lille.fr", "programmation@labulle-lille.fr"]);
        assert_eq!(s[0].1, "reservation");
        assert_eq!(s[1].2, "obfuscated");
        assert_eq!(s[2].2, "mailto");
        // dead-domain.fr has no MX: dropped and counted
        assert_eq!(o.dropped_no_mx, 1);
        assert!(o.booking);
        assert_eq!(o.contact_page.as_deref(), Some("https://www.labulle-lille.fr/infos-pratiques"));
        // stops after the booking address: no request past /contact
        assert_eq!(f.hits.last().map(String::as_str), Some("https://www.labulle-lille.fr/contact"));
        mark_done(&c, 1, &o);
        let pc: Option<String> = c.query_row("SELECT page_contact FROM vi_venues WHERE id = 1", [], |r| r.get(0)).unwrap();
        assert!(pc.is_some());
        // the cooldown now skips it, force does not
        assert!(load_jobs(&c, &[1], false).is_empty());
        assert_eq!(load_jobs(&c, &[1], true).len(), 1);
    }

    #[test]
    fn page_cap_notes_instagram_and_search() {
        let c = testdb::conn();
        venue(&c, 1, "Le Bikini", Some("https://lebikini.com"));
        // A dead site: every guess 404s, the cap holds.
        let mut f = FakeFetcher::new(&[("https://www.instagram.com/lebikini/", r#"<meta name="description" content="Club. Booking: booking@lebikini.com">"#)]);
        let r = resolver(&["lebikini.com"]);
        let mut mx = MxCheck::new(&r);
        let job = VenueJob {
            id: 1,
            nom: "Le Bikini".into(),
            site_web: Some("https://lebikini.com".into()),
            instagram: Some("lebikini".into()),
            ..Default::default()
        };
        let o = find_for_venue(&c, &job, &mut f, &mut mx, None);
        assert_eq!(o.fetches, MAX_PAGES + 1, "{:?}", f.hits);
        assert_eq!(stored(&c, 1), vec![("booking@lebikini.com".into(), "booking".into(), "instagram".into())]);

        // RA blurb on file, no site at all: no request, the search hook is used.
        venue(&c, 2, "Le Pont", None);
        let job = VenueJob {
            id: 2,
            nom: "Le Pont".into(),
            notes: Some("Booking enquiries: booking (at) lepont-club.fr".into()),
            ra_url: Some("https://ra.co/clubs/1".into()),
            ..Default::default()
        };
        let r = resolver(&["lepont-club.fr"]);
        let mut mx = MxCheck::new(&r);
        let mut f = FakeFetcher::new(&[]);
        let search = |_: &str, _: Option<&str>| -> Option<String> { None };
        let o = find_for_venue(&c, &job, &mut f, &mut mx, Some(&search));
        assert_eq!(o.kept, 1);
        assert!(f.hits.is_empty());
        assert_eq!(stored(&c, 2)[0].2, "ra_notes");

        // A searched site only yields addresses on a name-matching domain.
        venue(&c, 3, "Le Flow", None);
        let mut f = FakeFetcher::new(&[("https://leflow.fr", "booking@leflow.fr someone@gmail.com")]);
        let r = resolver(&["leflow.fr", "gmail.com"]);
        let mut mx = MxCheck::new(&r);
        let search = |_: &str, _: Option<&str>| Some("https://leflow.fr".to_string());
        let job = VenueJob { id: 3, nom: "Le Flow".into(), ..Default::default() };
        let o = find_for_venue(&c, &job, &mut f, &mut mx, Some(&search));
        assert_eq!(o.site_found.as_deref(), Some("https://leflow.fr"));
        assert_eq!(stored(&c, 3).len(), 1);
        // resolver down: nothing kept, nothing counted as no-MX
        venue(&c, 4, "Down", Some("https://down.test"));
        let mut f = FakeFetcher::new(&[("https://down.test", "booking@down.test")]);
        let r = resolver(&[]);
        let mut mx = MxCheck::new(&r);
        let job = VenueJob { id: 4, nom: "Down".into(), site_web: Some("https://down.test".into()), ..Default::default() };
        let o = find_for_venue(&c, &job, &mut f, &mut mx, None);
        assert_eq!((o.kept, o.dropped_no_mx), (0, 0));
    }
}
