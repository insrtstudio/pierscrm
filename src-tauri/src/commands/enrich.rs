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

const CRAWL_PATHS: &[&str] = &[
    "", "/contact", "/contact-us", "/contactez-nous", "/contacts", "/contatti", "/contacto",
    "/booking", "/bookings", "/press", "/presse", "/mentions-legales", "/impressum", "/legal",
    "/about", "/info",
];

fn crawl_client() -> reqwest::Client {
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

fn is_false_positive(email: &str) -> bool {
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
fn score_email(email: &str, from_mailto: bool) -> (i64, &'static str) {
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
fn extract_contacts(html: &str) -> (Vec<(String, bool)>, Option<String>) {
    use regex::Regex;
    let mut emails: Vec<(String, bool)> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    // mailto: links first, they are always real addresses.
    for cap in html.split("mailto:").skip(1) {
        let addr: String = cap
            .chars()
            .take_while(|c| !matches!(c, '"' | '\'' | '?' | '>' | ' ' | '<' | ')' | '&'))
            .collect();
        let addr = addr.trim().to_lowercase();
        if addr.contains('@') && !is_false_positive(&addr) && seen.insert(addr.clone()) {
            emails.push((addr, true));
        }
    }
    // then plain-text addresses.
    let re = Regex::new(r"[A-Za-z0-9._%+\-]+@[A-Za-z0-9.\-]+\.[A-Za-z]{2,}").unwrap();
    for m in re.find_iter(html) {
        let addr = m.as_str().trim_matches('.').to_lowercase();
        if !is_false_positive(&addr) && seen.insert(addr.clone()) {
            emails.push((addr, false));
        }
    }

    // one phone from tel: if present (numbers keep their spaces).
    let phone = html.split("tel:").nth(1).map(|c| {
        c.chars()
            .take_while(|ch| !matches!(ch, '"' | '\'' | '>' | '<'))
            .collect::<String>()
            .trim()
            .to_string()
    });

    (emails, phone)
}

fn normalize_site(raw: &str) -> Option<String> {
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
fn origin_of(base: &str) -> String {
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
fn base_candidates(base: &str) -> Vec<String> {
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
fn internal_links(html: &str, origin: &str) -> Vec<String> {
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

pub async fn process_enrich_task(
    ra_client: &reqwest::Client,
    pool: &DbPool,
    payload: &str,
) -> Result<(), ra::RaError> {
    let v: serde_json::Value = serde_json::from_str(payload)
        .map_err(|e| ra::RaError { message: e.to_string(), status: None, retryable: false })?;
    let venue_id = v["venue_id"].as_i64().unwrap_or(0);
    let ra_venue_id: i64 = {
        let conn = pool.get().unwrap();
        conn.query_row(
            "SELECT ra_venue_id FROM vi_venues WHERE id=?1",
            params![venue_id],
            |r| r.get::<_, Option<i64>>(0),
        )
        .ok()
        .flatten()
        .unwrap_or(0)
    };

    // 1. RA venue detail (website / phone / address / capacity / blurb).
    let mut site: Option<String> = None;
    if ra_venue_id > 0 {
        if let Ok(d) = ra::fetch_venue_detail(ra_client, ra_venue_id).await {
            site = d.website.as_deref().and_then(normalize_site);
            let capacite: Option<i64> = d
                .capacity
                .as_deref()
                .and_then(|c| c.chars().filter(|ch| ch.is_ascii_digit()).collect::<String>().parse().ok());
            let phone = d.phone.filter(|p| !p.trim().is_empty());
            let conn = pool.get().unwrap();
            let _ = conn.execute(
                "UPDATE vi_venues SET
                    site_web = COALESCE(site_web, ?2),
                    telephone = COALESCE(telephone, ?3),
                    adresse = COALESCE(adresse, ?4),
                    capacite_est = COALESCE(capacite_est, ?5),
                    notes = COALESCE(NULLIF(notes,''), ?6),
                    updated_at = datetime('now')
                 WHERE id = ?1",
                params![venue_id, site, phone, d.address, capacite, d.blurb],
            );
        }
    }
    if site.is_none() {
        let conn = pool.get().unwrap();
        site = conn
            .query_row("SELECT site_web FROM vi_venues WHERE id=?1", params![venue_id], |r| {
                r.get::<_, Option<String>>(0)
            })
            .ok()
            .flatten()
            .and_then(|s| normalize_site(&s));
    }

    // 2. Crawl the venue site for emails (and a phone / contact page).
    if let Some(raw) = site {
        let client = crawl_client();

        // Pick a base whose homepage actually responds (handles www/apex + stale http).
        let mut base = origin_of(&raw);
        let mut home_html = String::new();
        for cand in base_candidates(&raw) {
            if let Ok(resp) = client.get(cand.as_str()).send().await {
                if resp.status().is_success() {
                    if let Ok(body) = resp.text().await {
                        base = cand;
                        home_html = body;
                        break;
                    }
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        }

        // Visit list: the guessed contact/legal paths + real links found on the homepage.
        let mut urls: Vec<String> = CRAWL_PATHS.iter().map(|p| format!("{}{}", base, p)).collect();
        for link in internal_links(&home_html, &base) {
            if !urls.contains(&link) {
                urls.push(link);
            }
        }

        let mut best_email_score = 0i64;
        let mut contact_page: Option<String> = None;
        let mut fetches = 0;

        for url in &urls {
            if fetches >= 12 {
                break;
            }
            // Reuse the homepage body we already downloaded instead of refetching it.
            let body = if url == &base && !home_html.is_empty() {
                home_html.clone()
            } else {
                let resp = match client.get(url.as_str()).send().await {
                    Ok(r) => r,
                    Err(_) => continue,
                };
                if !resp.status().is_success() {
                    continue;
                }
                let ct = resp
                    .headers()
                    .get("content-type")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_string();
                if !ct.contains("html") && !ct.is_empty() {
                    continue;
                }
                fetches += 1;
                match resp.text().await {
                    Ok(b) => b,
                    Err(_) => continue,
                }
            };
            let body = if body.len() > 300_000 { body[..300_000].to_string() } else { body };
            let (emails, phone) = extract_contacts(&body);

            if url != &base && !emails.is_empty() && contact_page.is_none() {
                contact_page = Some(url.clone());
            }

            let conn = pool.get().unwrap();
            for (email, from_mailto) in emails {
                let (score, role) = score_email(&email, from_mailto);
                best_email_score = best_email_score.max(score);
                let _ = conn.execute(
                    "INSERT OR IGNORE INTO vi_contacts
                        (venue_id, type, valeur, role_devine, score, source_url, source_method, confiance, verifie)
                     VALUES (?1,'email',?2,?3,?4,?5,?6,?7,0)",
                    params![
                        venue_id, email, role, score, url,
                        if from_mailto { "mailto" } else { "regex" },
                        if from_mailto { 1.0 } else { 0.85 }
                    ],
                );
            }
            // store a phone from the site only if RA gave none
            if let Some(ph) = phone {
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

            // enough signal: a booking-grade address plus a contact page, stop early.
            if best_email_score >= 100 && contact_page.is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(900)).await;
        }

        let conn = pool.get().unwrap();
        let _ = conn.execute(
            "UPDATE vi_venues SET page_contact = COALESCE(page_contact, ?2), enriched_at = datetime('now'), updated_at = datetime('now') WHERE id=?1",
            params![venue_id, contact_page],
        );
    } else {
        let conn = pool.get().unwrap();
        let _ = conn.execute(
            "UPDATE vi_venues SET enriched_at = datetime('now') WHERE id=?1",
            params![venue_id],
        );
    }

    Ok(())
}

// ---------------- Commands ----------------

/// Queue an enrichment run over qualified/validated venues (optionally re-run).
#[tauri::command]
pub async fn vi_start_enrich(
    app: AppHandle,
    state: State<'_, AppState>,
    force: Option<bool>,
) -> Result<i64, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let mut sql = String::from(
        "SELECT id FROM vi_venues WHERE statut IN ('qualifie','valide')",
    );
    if !force.unwrap_or(false) {
        sql.push_str(" AND enriched_at IS NULL");
    }
    sql.push_str(" ORDER BY score_qualif DESC");
    let venue_ids: Vec<i64> = {
        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| r.get::<_, i64>(0))
            .map_err(|e| e.to_string())?;
        rows.filter_map(|r| r.ok()).collect()
    };
    if venue_ids.is_empty() {
        return Err("Aucune salle à enrichir (lance d'abord un moissonnage).".into());
    }

    conn.execute(
        "INSERT INTO vi_runs (type, params, statut, started_at) VALUES ('enrich', ?1, 'en_cours', datetime('now'))",
        params![json!({ "venues": venue_ids.len() }).to_string()],
    )
    .map_err(|e| e.to_string())?;
    let run_id = conn.last_insert_rowid();

    let mut stmt = conn
        .prepare("INSERT INTO vi_tasks (run_id, type, payload, statut) VALUES (?1,'enrich_venue',?2,'en_attente')")
        .map_err(|e| e.to_string())?;
    for vid in &venue_ids {
        stmt.execute(params![run_id, json!({ "venue_id": vid }).to_string()])
            .map_err(|e| e.to_string())?;
    }
    drop(stmt);

    crate::commands::harvest::spawn_worker(state.pool.clone(), app, run_id);
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

#[tauri::command]
pub fn vi_venue_detail(state: State<AppState>, id: i64) -> Result<ViVenueFiche, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;

    let mut fiche = conn
        .query_row(
            "SELECT id, nom, ville, pays, adresse, capacite_est, site_web, page_contact, telephone,
                    ra_url, statut, score_qualif, nb_events_periode, notes, enriched_at
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
