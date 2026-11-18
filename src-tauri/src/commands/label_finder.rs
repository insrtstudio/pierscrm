//! "Chercher les contacts" of a label: where its demos go, its site, its
//! Instagram.
//!
//! Three places, in this order, each only for what is still missing:
//! 1. LabelsBase (labelsbase.net), a public directory of labels with their
//!    demo address. A page is trusted when it links the same Beatport page as
//!    the label (or, failing a Beatport link on either side, bears its name).
//! 2. A web search (Serper, when a key is set) for a label with no site.
//! 3. The label's own site: homepage, then its demo / contact pages. Addresses
//!    on the site's domain or a personal mailbox, demo desks first; a link to
//!    a demo platform (LabelRadar, Trackstack, a form) is kept as the form.
//!
//! Same rules as the club finder: an address is copied from a page, never
//! built, and kept only when its domain takes mail (MX). What the user typed
//! is never overwritten: only empty fields are filled.

use crate::commands::deliverability::dns::DohResolver;
use crate::commands::email_finder::{find_emails, Fetcher, HttpFetcher, MxCheck};
use crate::commands::enrich;
use crate::db::normalise;
use crate::AppState;
use regex::Regex;
use rusqlite::{params, Connection};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Duration;
use tauri::{AppHandle, Emitter, State};

pub const EVENT: &str = "labels:finder";
const DIRECTORY: &str = "https://labelsbase.net";
/// Pages read on the label's own site.
const SITE_PAGES: usize = 6;
const SITE_PATHS: &[&str] = &["/demos", "/demo", "/contact", "/submit", "/about"];
/// Hosts that are never the label's own site.
const PLATFORMS: &[&str] = &[
    "beatport.", "soundcloud.", "facebook.", "instagram.", "twitter.", "x.com", "youtube.", "youtu.be", "spotify.",
    "apple.com", "bandcamp.", "discogs.", "traxsource.", "junodownload.", "tiktok.", "linktr.ee", "labelsbase.",
    "residentadvisor.", "ra.co", "mixcloud.", "deezer.", "wikipedia.", "google.", "t.me", "threads.",
];
const FREE_MAIL: &[&str] =
    &["gmail.com", "googlemail.com", "outlook.com", "hotmail.com", "live.com", "yahoo.com", "icloud.com", "proton.me", "protonmail.com", "gmx.de", "gmx.net", "web.de"];
/// Demo platforms and form hosts: a link there is the label's demo form.
const FORM_HOSTS: &[&str] = &["labelradar.com/", "tstack.app/", "trackstack.", "forms.gle/", "docs.google.com/forms", "typeform.com/", "demodrop.", "jotform.com/"];

#[derive(Debug, Clone, Default)]
pub struct LabelJob {
    pub id: i64,
    pub name: String,
    pub beatport_id: Option<i64>,
    pub beatport_url: Option<String>,
    pub website: Option<String>,
    pub instagram: Option<String>,
    pub email: Option<String>,
    pub demo_url: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Finds {
    pub website: Option<String>,
    pub instagram: Option<String>,
    pub email: Option<String>,
    /// The page the address was read on.
    pub email_source: Option<String>,
    pub demo_url: Option<String>,
    pub country: Option<String>,
}

fn re(cell: &'static OnceLock<Regex>, pat: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pat).unwrap())
}

fn hrefs(html: &str) -> Vec<String> {
    static HREF: OnceLock<Regex> = OnceLock::new();
    re(&HREF, r#"(?i)href\s*=\s*["']([^"'\s>]+)["']"#).captures_iter(html).map(|c| c[1].replace("&amp;", "&")).collect()
}

/// Visible lines of a page, tags as line breaks.
fn lines(html: &str) -> Vec<String> {
    static SCRIPT: OnceLock<Regex> = OnceLock::new();
    static TAG: OnceLock<Regex> = OnceLock::new();
    let s = re(&SCRIPT, r"(?is)<(script|style)\b.*?</(script|style)\s*>").replace_all(html, "\n");
    let s = re(&TAG, r"<[^>]*>").replace_all(&s, "\n");
    s.lines()
        .map(|l| l.trim().replace("&amp;", "&").replace("&#039;", "'"))
        .filter(|l| !l.is_empty())
        .collect()
}

fn is_platform(url: &str) -> bool {
    let host = enrich::host_of(url);
    host.is_empty() || PLATFORMS.iter().any(|p| enrich::host_hit(&host, p))
}

fn instagram_url(handle: &str) -> String {
    format!("https://www.instagram.com/{}/", handle.trim_start_matches('@'))
}

/// Slugs to try on the directory: the Beatport one, then the usual variants.
pub fn slug_candidates(name: &str, beatport_url: Option<&str>) -> Vec<String> {
    let from_name: String = {
        let mut s = String::new();
        for ch in name.to_lowercase().chars() {
            if ch.is_ascii_alphanumeric() {
                s.push(ch);
            } else if !s.ends_with('-') && !s.is_empty() {
                s.push('-');
            }
        }
        s.trim_end_matches('-').to_string()
    };
    let bp = beatport_url
        .and_then(|u| u.split("/label/").nth(1))
        .and_then(|r| r.split('/').next())
        .map(|s| s.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect::<String>())
        .filter(|s| !s.is_empty());
    let mut out: Vec<String> = Vec::new();
    let mut push = |s: String| {
        if !s.is_empty() && !out.contains(&s) {
            out.push(s);
        }
    };
    let first = bp.unwrap_or_else(|| from_name.clone());
    let mut base = first.clone();
    for suffix in ["-records", "-recordings", "-music", "-label"] {
        if let Some(b) = base.strip_suffix(suffix) {
            base = b.to_string();
            break;
        }
    }
    push(first);
    push(from_name);
    push(base.clone());
    push(format!("{base}-records"));
    push(format!("{base}-music"));
    out
}

#[derive(Debug, Default, PartialEq)]
pub struct DirectoryPage {
    pub title: String,
    pub beatport_id: Option<i64>,
    pub demo_email: Option<String>,
    pub general_email: Option<String>,
    pub website: Option<String>,
    pub instagram: Option<String>,
    pub country: Option<String>,
}

/// What a LabelsBase page says.
pub fn parse_directory(html: &str) -> DirectoryPage {
    static BP: OnceLock<Regex> = OnceLock::new();
    static TITLE: OnceLock<Regex> = OnceLock::new();
    let mut p = DirectoryPage::default();
    if let Some(c) = re(&TITLE, r"(?is)<title>\s*(.*?)\s*</title>").captures(html) {
        let t = c[1].replace("&amp;", "&").replace("&#039;", "'");
        p.title = t.split(" Demo Submission").next().unwrap_or("").trim().to_string();
    }
    p.beatport_id = re(&BP, r"beatport\.com/label/[^/\x22']+/(\d+)").captures(html).and_then(|c| c[1].parse().ok());
    let ls = lines(html);
    let after = |key: &str| -> Option<String> {
        let i = ls.iter().position(|l| l.to_lowercase().starts_with(key))?;
        ls.get(i + 1).and_then(|v| enrich::clean_email(v))
    };
    p.demo_email = after("demo email");
    p.general_email = after("general contact");
    if let Some(i) = ls.iter().position(|l| l == "Location") {
        let parts: Vec<&str> = ls[i + 1..]
            .iter()
            .take(2)
            .map(|s| s.trim_matches(|c: char| c == ',' || c.is_whitespace()))
            .take_while(|s| !["Contacts", "Links in web", "Genres", "Founded"].contains(s))
            .collect();
        p.country = parts.last().map(|s| s.to_string()).filter(|s| !s.is_empty() && s.len() < 60);
    }
    for h in hrefs(html) {
        if !h.starts_with("http") {
            continue;
        }
        if p.instagram.is_none() {
            if let Some(handle) = enrich::instagram_handle(&h) {
                p.instagram = Some(instagram_url(&handle));
                continue;
            }
        }
        if p.website.is_none() && !is_platform(&h) && !h.contains("chrome.google") && !h.contains("cdn") && !h.contains("googleapis") {
            p.website = enrich::normalize_site(&h);
        }
    }
    p
}

fn rank(email: &str) -> u8 {
    let local = email.split('@').next().unwrap_or("");
    if ["demo", "promo", "submit", "anr", "a&r", "music", "tracks"].iter().any(|k| local.contains(k)) || local == "ar" {
        0
    } else if ["info", "contact", "hello", "hi", "label", "office", "mail"].iter().any(|k| local.starts_with(k)) {
        1
    } else {
        2
    }
}

/// Addresses of a page of the label's site that may be the label's: on its
/// domain or a personal mailbox, best desk first.
pub fn site_emails(html: &str, host: &str) -> Vec<String> {
    let mut out: Vec<String> = find_emails(html)
        .into_iter()
        .filter_map(|f| enrich::clean_email(&f.email))
        .filter(|e| !enrich::is_placeholder_email(e) && !enrich::is_non_pitch_email(e))
        .filter(|e| {
            let d = enrich::email_domain(e);
            enrich::same_site(d, host) || FREE_MAIL.contains(&d)
        })
        .collect();
    out.sort_by_key(|e| rank(e));
    out.dedup();
    out
}

fn form_link(html: &str) -> Option<String> {
    hrefs(html).into_iter().find(|h| h.starts_with("http") && FORM_HOSTS.iter().any(|f| h.to_lowercase().contains(f)))
}

fn demo_links(html: &str, base: &str) -> Vec<String> {
    let host = enrich::host_of(base);
    let mut out = Vec::new();
    for h in hrefs(html) {
        let low = h.to_lowercase();
        if !["demo", "submit", "contact", "about"].iter().any(|k| low.contains(k)) {
            continue;
        }
        let url = if h.starts_with("http") {
            if !enrich::same_site(&enrich::host_of(&h), &host) {
                continue;
            }
            h
        } else if h.starts_with('/') {
            format!("{base}{h}")
        } else {
            continue;
        };
        let url = url.split('#').next().unwrap_or("").trim_end_matches('/').to_string();
        if !url.is_empty() && !out.contains(&url) {
            out.push(url);
        }
    }
    // Demo pages before contact pages.
    out.sort_by_key(|u| if u.to_lowercase().contains("demo") || u.to_lowercase().contains("submit") { 0 } else { 1 });
    out
}

/// Web search result -> (site, instagram) of the label. Prod: Serper.
pub type LabelSearch<'a> = &'a dyn Fn(&str) -> Option<serde_json::Value>;

fn name_core(name: &str) -> String {
    let n = normalise(name);
    for suffix in ["records", "recordings", "music", "label"] {
        if let Some(b) = n.strip_suffix(suffix) {
            if b.len() >= 4 {
                return b.to_string();
            }
        }
    }
    n
}

fn instagram_from_search(d: &serde_json::Value, name: &str) -> Option<String> {
    let core = name_core(name);
    if core.len() < 4 {
        return None;
    }
    d.get("organic")?.as_array()?.iter().find_map(|it| {
        let handle = enrich::instagram_handle(it.get("link")?.as_str()?)?;
        normalise(&handle).contains(&core).then(|| instagram_url(&handle))
    })
}

pub fn find_for_label(job: &LabelJob, fetch: &mut dyn Fetcher, mx: &mut MxCheck, search: Option<LabelSearch>) -> Finds {
    let mut f = Finds::default();
    let has = |v: &Option<String>| v.as_deref().map_or(false, |s| !s.trim().is_empty());
    // (address, page) candidates, best first.
    let mut emails: Vec<(String, String)> = Vec::new();

    // 1. The directory.
    for slug in slug_candidates(&job.name, job.beatport_url.as_deref()).into_iter().take(4) {
        let url = format!("{DIRECTORY}/{slug}");
        let Some(html) = fetch.get(&url) else { continue };
        let p = parse_directory(&html);
        let same = match (p.beatport_id, job.beatport_id) {
            (Some(a), Some(b)) => a == b,
            _ => normalise(&p.title) == normalise(&job.name),
        };
        if !same {
            continue;
        }
        if let Some(e) = p.demo_email {
            emails.push((e, url.clone()));
        }
        if let Some(e) = p.general_email {
            emails.push((e, url.clone()));
        }
        f.website = p.website;
        f.instagram = p.instagram;
        f.country = p.country;
        break;
    }

    // 2. No site yet: the web search.
    let mut site = job.website.as_deref().filter(|s| !s.trim().is_empty()).and_then(enrich::normalize_site).filter(|s| !is_platform(s));
    if site.is_none() {
        site = f.website.clone();
    } else {
        f.website = None;
    }
    if site.is_none() || (!has(&job.instagram) && f.instagram.is_none()) {
        if let Some(d) = search.and_then(|s| s(&job.name)) {
            if site.is_none() {
                site = enrich::pick_site_from_search(&d, &job.name, None, PLATFORMS);
                f.website = site.clone();
            }
            if f.instagram.is_none() {
                f.instagram = instagram_from_search(&d, &job.name);
            }
        }
    }

    // 3. The label's own site.
    if let Some(raw) = &site {
        let mut fetched = 0;
        let mut base = enrich::origin_of(raw);
        let mut home = None;
        for cand in enrich::base_candidates(raw) {
            fetched += 1;
            if let Some(body) = fetch.get(&cand) {
                base = cand;
                home = Some(body);
                break;
            }
        }
        if let Some(home) = home {
            let host = enrich::host_of(&base);
            let mut queue = demo_links(&home, &base);
            queue.extend(SITE_PATHS.iter().map(|p| format!("{base}{p}")));
            let mut pages = vec![(base.clone(), home)];
            let mut done = vec![base.clone()];
            let mut queue = queue.into_iter();
            let mut site_found: Vec<(String, String)> = Vec::new();
            while let Some((url, body)) = pages.pop() {
                for e in site_emails(&body, &host) {
                    if !site_found.iter().any(|(x, _)| *x == e) {
                        site_found.push((e, url.clone()));
                    }
                }
                if f.demo_url.is_none() {
                    f.demo_url = form_link(&body);
                }
                if f.instagram.is_none() {
                    f.instagram = hrefs(&body).iter().find_map(|h| enrich::instagram_handle(h)).map(|h| instagram_url(&h));
                }
                let enough = site_found.iter().any(|(e, _)| rank(e) == 0) || f.demo_url.is_some();
                while pages.is_empty() && !enough && fetched < SITE_PAGES {
                    let Some(u) = queue.find(|u| !done.contains(u)) else { break };
                    done.push(u.clone());
                    fetched += 1;
                    if let Some(b) = fetch.get(&u) {
                        pages.push((u, b));
                    }
                }
            }
            // A demo desk on the site beats the directory; the directory beats
            // a general address of the site.
            site_found.sort_by_key(|(e, _)| rank(e));
            let (desk, other): (Vec<_>, Vec<_>) = site_found.into_iter().partition(|(e, _)| rank(e) == 0);
            emails = desk.into_iter().chain(emails).chain(other).collect();
        }
    }

    // The first candidate whose domain takes mail.
    if !has(&job.email) {
        for (e, src) in emails {
            if mx.has_mx(enrich::email_domain(&e)) == Some(true) {
                f.email = Some(e);
                f.email_source = Some(src);
                break;
            }
        }
    }
    if has(&job.instagram) {
        f.instagram = None;
    }
    if has(&job.demo_url) {
        f.demo_url = None;
    }
    f
}

/// Write what was found: empty fields only. A label with no address but a
/// form becomes a "form" label. Returns whether a way to send a demo came out.
pub fn apply(conn: &Connection, id: i64, f: &Finds) -> bool {
    let _ = conn.execute(
        "UPDATE labels SET
            website = COALESCE(NULLIF(trim(website), ''), ?2),
            instagram = COALESCE(NULLIF(trim(instagram), ''), ?3),
            contact_source = CASE WHEN COALESCE(trim(email), '') = '' AND ?4 IS NOT NULL THEN ?5 ELSE contact_source END,
            email = COALESCE(NULLIF(trim(email), ''), ?4),
            demo_url = COALESCE(NULLIF(trim(demo_url), ''), ?6),
            country = COALESCE(NULLIF(trim(country), ''), ?7),
            contact_find_at = datetime('now'), updated_at = datetime('now')
         WHERE id = ?1",
        params![id, f.website, f.instagram, f.email, f.email_source, f.demo_url, f.country],
    );
    let _ = conn.execute(
        "UPDATE labels SET demo_policy = 'form'
         WHERE id = ?1 AND demo_policy = 'email' AND COALESCE(trim(email), '') = '' AND COALESCE(trim(demo_url), '') <> ''",
        params![id],
    );
    // The contact behind the label follows, as in labels::save_label.
    if let Some(e) = &f.email {
        let _ = conn.execute(
            "UPDATE contacts SET email = ?2, updated_at = datetime('now')
             WHERE id = (SELECT contact_id FROM labels WHERE id = ?1) AND COALESCE(trim(email), '') = ''",
            params![id, e],
        );
    }
    f.email.is_some() || f.demo_url.is_some()
}

fn load_jobs(conn: &Connection, ids: &[i64]) -> Vec<LabelJob> {
    let Ok(mut st) = conn.prepare("SELECT id, name, beatport_id, beatport_url, website, instagram, email, demo_url FROM labels WHERE id = ?1")
    else {
        return Vec::new();
    };
    ids.iter()
        .filter_map(|id| {
            st.query_row(params![id], |r| {
                Ok(LabelJob {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    beatport_id: r.get(2)?,
                    beatport_url: r.get(3)?,
                    website: r.get(4)?,
                    instagram: r.get(5)?,
                    email: r.get(6)?,
                    demo_url: r.get(7)?,
                })
            })
            .ok()
        })
        .collect()
}

fn serper(key: &str, name: &str) -> Option<serde_json::Value> {
    let client = reqwest::blocking::Client::builder().user_agent(enrich::CRAWL_UA).timeout(Duration::from_secs(10)).build().ok()?;
    let resp = client
        .post("https://google.serper.dev/search")
        .header("X-API-KEY", key)
        .json(&serde_json::json!({ "q": format!("\"{name}\" record label demo", ), "num": 10 }))
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
    std::thread::sleep(Duration::from_millis(400));
    resp.json().ok()
}

static RUNNING: AtomicBool = AtomicBool::new(false);
struct RunGuard;
impl Drop for RunGuard {
    fn drop(&mut self) {
        RUNNING.store(false, Ordering::SeqCst);
    }
}

/// Payload of `labels:finder`, sent after each label.
#[derive(Debug, Clone, Serialize)]
pub struct FinderProgress {
    pub done: usize,
    pub total: usize,
    /// Labels that got an address or a form.
    pub found: usize,
    pub finished: bool,
}

/// "Chercher les contacts" for the given labels, in the background. Returns
/// how many are queued; progress comes as `labels:finder` events.
#[tauri::command(async)]
pub fn labels_find_contacts(app: AppHandle, state: State<'_, AppState>, ids: Vec<i64>) -> Result<usize, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let jobs = load_jobs(&conn, &ids);
    if jobs.is_empty() {
        return Ok(0);
    }
    if RUNNING.swap(true, Ordering::SeqCst) {
        return Err("Une recherche de contacts est déjà en cours.".into());
    }
    let key = crate::secrets::read_setting(&conn, "serper_api_key").filter(|k| !k.trim().is_empty());
    drop(conn);
    let pool = state.pool.clone();
    let total = jobs.len();
    std::thread::spawn(move || {
        let _guard = RunGuard;
        let mut p = FinderProgress { done: 0, total, found: 0, finished: false };
        let (Ok(mut fetcher), Ok(resolver)) = (HttpFetcher::new(), DohResolver::new()) else {
            p.finished = true;
            let _ = app.emit(EVENT, p);
            return;
        };
        let mut mx = MxCheck::new(&resolver);
        let search = key.map(|k| move |name: &str| serper(&k, name));
        for job in &jobs {
            let finds = find_for_label(job, &mut fetcher, &mut mx, search.as_ref().map(|f| f as LabelSearch));
            if let Ok(conn) = pool.get() {
                if apply(&conn, job.id, &finds) {
                    p.found += 1;
                }
            }
            p.done += 1;
            let _ = app.emit(EVENT, p.clone());
        }
        p.finished = true;
        let _ = app.emit(EVENT, p);
    });
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::deliverability::dns::{RType, Resolver};
    use crate::commands::residency::testdb;
    use std::collections::HashMap;

    struct Pages(HashMap<String, String>, Vec<String>);
    impl Pages {
        fn new(p: &[(&str, &str)]) -> Self {
            Self(p.iter().map(|(u, b)| (u.to_string(), b.to_string())).collect(), Vec::new())
        }
    }
    impl Fetcher for Pages {
        fn get(&mut self, url: &str) -> Option<String> {
            self.1.push(url.to_string());
            self.0.get(url).cloned()
        }
    }
    /// Every domain takes mail except "nomx.test".
    struct Dns;
    impl Resolver for Dns {
        fn lookup(&self, name: &str, _: RType) -> Result<Vec<String>, String> {
            Ok(if name == "nomx.test" { Vec::new() } else { vec![format!("10 mx.{name}.")] })
        }
    }

    const DIRECTORY_PAGE: &str = r#"<html><head><title>Solid Grooves Records Demo Submission, Contacts, A&amp;R, Links &amp; More.</title></head><body>
        <div>Location</div><div>London,</div><div>United Kingdom</div>
        <div>Contacts</div><div>General contact:</div><a href="mailto:info@solidgrooves.test">info@solidgrooves.test</a>
        <div>Demo email to A&R:</div><a href="mailto:demos@solidgrooves.test?subject=x">demos@solidgrooves.test</a>
        <a href="https://www.instagram.com/solidgroovesrecords/">ig</a>
        <a href="https://soundcloud.com/solidgrooves-records">sc</a>
        <a href="https://solidgrooves.test/">site</a>
        <a href="https://www.beatport.com/label/solid-grooves-records/50983">bp</a></body></html>"#;

    fn job() -> LabelJob {
        LabelJob {
            id: 1,
            name: "Solid Grooves Records".into(),
            beatport_id: Some(50983),
            beatport_url: Some("https://www.beatport.com/label/solid-grooves-records/50983".into()),
            ..Default::default()
        }
    }

    #[test]
    fn slugs_start_with_the_beatport_one() {
        let s = slug_candidates("Solid Grooves Records", Some("https://www.beatport.com/label/solid-grooves-records/50983"));
        assert_eq!(s, vec!["solid-grooves-records", "solid-grooves", "solid-grooves-music"]);
        assert_eq!(slug_candidates("range.", None)[0], "range");
    }

    #[test]
    fn the_directory_page_of_the_same_beatport_label_is_read() {
        let p = parse_directory(DIRECTORY_PAGE);
        assert_eq!(p.title, "Solid Grooves Records");
        assert_eq!(p.beatport_id, Some(50983));
        assert_eq!(p.demo_email.as_deref(), Some("demos@solidgrooves.test"));
        assert_eq!(p.general_email.as_deref(), Some("info@solidgrooves.test"));
        assert_eq!(p.website.as_deref(), Some("https://solidgrooves.test"));
        assert_eq!(p.instagram.as_deref(), Some("https://www.instagram.com/solidgroovesrecords/"));
        assert_eq!(p.country.as_deref(), Some("United Kingdom"));
    }

    #[test]
    fn demo_address_comes_from_the_directory_and_the_site_adds_the_form() {
        let mut pages = Pages::new(&[
            ("https://labelsbase.net/solid-grooves-records", DIRECTORY_PAGE),
            ("https://solidgrooves.test", r#"<a href="/demos">Demos</a> <a href="mailto:shop@solidgrooves.test">shop</a>"#),
            ("https://solidgrooves.test/demos", r#"<a href="https://www.labelradar.com/labels/solidgrooves/portal">Send</a>"#),
        ]);
        let f = find_for_label(&job(), &mut pages, &mut MxCheck::new(&Dns), None);
        assert_eq!(f.email.as_deref(), Some("demos@solidgrooves.test"));
        assert_eq!(f.email_source.as_deref(), Some("https://labelsbase.net/solid-grooves-records"));
        assert_eq!(f.demo_url.as_deref(), Some("https://www.labelradar.com/labels/solidgrooves/portal"));
        assert_eq!(f.website.as_deref(), Some("https://solidgrooves.test"));
    }

    #[test]
    fn a_directory_page_of_another_label_is_ignored() {
        let mut j = job();
        j.beatport_id = Some(1);
        let mut pages = Pages::new(&[("https://labelsbase.net/solid-grooves-records", DIRECTORY_PAGE)]);
        let f = find_for_label(&j, &mut pages, &mut MxCheck::new(&Dns), None);
        assert_eq!(f, Finds::default());
    }

    #[test]
    fn an_address_without_mx_is_dropped_and_typed_fields_are_kept() {
        let mut pages = Pages::new(&[
            ("https://label.test", r#"<a href="mailto:demos@nomx.test">x</a> write to hello@label.test <a href="https://instagram.com/label_test">ig</a>"#),
        ]);
        let j = LabelJob { id: 1, name: "Label".into(), website: Some("label.test".into()), ..Default::default() };
        let f = find_for_label(&j, &mut pages, &mut MxCheck::new(&Dns), None);
        assert_eq!(f.email.as_deref(), Some("hello@label.test"));
        assert_eq!(f.website, None);

        let c = testdb::conn();
        c.execute("INSERT INTO labels (name, name_norm, website, email) VALUES ('Label', 'label', 'https://typed.test', '')", []).unwrap();
        let id = c.last_insert_rowid();
        assert!(apply(&c, id, &Finds { website: Some("https://other.test".into()), email: f.email.clone(), email_source: Some("https://label.test".into()), ..f.clone() }));
        let (site, email, src, ig): (String, String, String, String) = c
            .query_row("SELECT website, email, contact_source, instagram FROM labels WHERE id = ?1", params![id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap();
        assert_eq!((site.as_str(), email.as_str(), src.as_str()), ("https://typed.test", "hello@label.test", "https://label.test"));
        assert_eq!(ig, "https://www.instagram.com/label_test/");
    }

    #[test]
    fn a_label_with_only_a_form_becomes_a_form_label() {
        let c = testdb::conn();
        c.execute("INSERT INTO labels (name, name_norm) VALUES ('Trick', 'trick')", []).unwrap();
        let id = c.last_insert_rowid();
        assert!(apply(&c, id, &Finds { demo_url: Some("https://tstack.app/trick".into()), ..Default::default() }));
        let policy: String = c.query_row("SELECT demo_policy FROM labels WHERE id = ?1", params![id], |r| r.get(0)).unwrap();
        assert_eq!(policy, "form");
    }
}
