//! Instagram as a prospecting channel.
//!
//! Bookers say a DM works well with collectives, so the app collects their
//! handles and helps write the DM. It never sends one: Instagram's official
//! API cannot open a conversation and unofficial automation would put
//! @madebypiers at risk of a ban. The flow is "assisted": the app drafts a
//! short DM in Thibault's voice, copies it and opens the conversation, he
//! pastes and sends, then clicks "DM envoyé" and the touch is recorded like a
//! sent email (status, first contact, follow-up date).
//!
//! - Handles: read from the pages enrichment already crawls (enrich.rs for
//!   venues, promoters.rs for collectives), from the Serper results those runs
//!   already pay for, and by the "Chercher les Instagram" pass over collectives
//!   enriched before this existed (task `instagram_promoter`, see harvest.rs).
//! - social_touches: one row per DM sent, listed by `list_social_touches`
//!   (week agenda, contact history).

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::json;
use tauri::{AppHandle, State};

use super::enrich::{base_candidates, core_matches_name, crawl_client, fetch_html, is_foreign_venue_site, normalize_site, origin_of};
use super::ra;
use super::residency::claude::{self, Llm, LlmRequest, SystemBlock};
use super::residency::content::HOUSE_VOICE;
use super::residency::insrt::{Insrt, Recipient};
use super::residency::zone::{iso_country, language_for};
use crate::db::{normalise, DbPool};
use crate::AppState;

// ---------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------

/// Additive and idempotent, run on every launch (db::migrate, after the
/// promoters schema).
pub(crate) fn migrate(conn: &Connection) {
    for stmt in [
        "ALTER TABLE contacts ADD COLUMN instagram TEXT",
        // When the Instagram pass last looked at a collective (found or not),
        // so a second click resumes instead of starting over.
        "ALTER TABLE vi_promoter_orgs ADD COLUMN instagram_checked_at TEXT",
        "CREATE TABLE IF NOT EXISTS social_touches (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            contact_id  INTEGER REFERENCES contacts(id) ON DELETE CASCADE,
            venue_id    INTEGER,
            promoter_id INTEGER,
            channel     TEXT NOT NULL DEFAULT 'instagram',
            handle      TEXT,
            body        TEXT,
            sent_at     TEXT NOT NULL DEFAULT (datetime('now')),
            created_at  TEXT NOT NULL DEFAULT (datetime('now'))
        )",
        "CREATE INDEX IF NOT EXISTS idx_social_touches_sent ON social_touches(sent_at)",
        "CREATE INDEX IF NOT EXISTS idx_social_touches_contact ON social_touches(contact_id)",
        // A venue or collective promoted into the pipeline (or given a handle
        // later) hands its handle to its contact. Triggers, so every promote
        // path (Lieux, Collectifs, residency targeting) carries it.
        "CREATE TRIGGER IF NOT EXISTS trg_vi_venues_instagram_contact
         AFTER UPDATE OF crm_contact_id, instagram ON vi_venues
         WHEN NEW.crm_contact_id IS NOT NULL AND trim(COALESCE(NEW.instagram, '')) <> ''
         BEGIN
            UPDATE contacts SET instagram = NEW.instagram
            WHERE id = NEW.crm_contact_id AND trim(COALESCE(instagram, '')) = '';
         END",
        "CREATE TRIGGER IF NOT EXISTS trg_vi_promoter_orgs_instagram_contact
         AFTER UPDATE OF crm_contact_id, instagram ON vi_promoter_orgs
         WHEN NEW.crm_contact_id IS NOT NULL AND trim(COALESCE(NEW.instagram, '')) <> ''
         BEGIN
            UPDATE contacts SET instagram = NEW.instagram
            WHERE id = NEW.crm_contact_id AND trim(COALESCE(instagram, '')) = '';
         END",
        // Contacts promoted before the column existed.
        "UPDATE contacts SET instagram = (SELECT v.instagram FROM vi_venues v
             WHERE v.crm_contact_id = contacts.id AND trim(COALESCE(v.instagram, '')) <> '' LIMIT 1)
         WHERE trim(COALESCE(instagram, '')) = ''
           AND EXISTS (SELECT 1 FROM vi_venues v WHERE v.crm_contact_id = contacts.id AND trim(COALESCE(v.instagram, '')) <> '')",
        "UPDATE contacts SET instagram = (SELECT o.instagram FROM vi_promoter_orgs o
             WHERE o.crm_contact_id = contacts.id AND trim(COALESCE(o.instagram, '')) <> '' LIMIT 1)
         WHERE trim(COALESCE(instagram, '')) = ''
           AND EXISTS (SELECT 1 FROM vi_promoter_orgs o WHERE o.crm_contact_id = contacts.id AND trim(COALESCE(o.instagram, '')) <> '')",
    ] {
        let _ = conn.execute(stmt, []);
    }
    // An Instagram link typed in the imported sheet's "channel" column.
    let rows: Vec<(i64, String)> = conn
        .prepare(
            "SELECT id, contact_channel FROM contacts
             WHERE trim(COALESCE(instagram, '')) = '' AND (contact_channel LIKE '%instagram.com/%' OR contact_channel LIKE '%instagr.am/%')",
        )
        .and_then(|mut st| st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map(|it| it.flatten().collect()))
        .unwrap_or_default();
    for (id, ch) in rows {
        if let Some(h) = handle_from_url(&ch) {
            let _ = conn.execute("UPDATE contacts SET instagram = ?2 WHERE id = ?1", params![id, h]);
        }
    }
}

// ---------------------------------------------------------------------------
// Handles: extraction, normalisation, choice
// ---------------------------------------------------------------------------

/// First path segments of instagram.com that are not a profile.
const RESERVED_PATHS: &[&str] = &[
    "p", "reel", "reels", "tv", "explore", "stories", "accounts", "share", "direct", "about", "legal",
    "developer", "developers", "web", "privacy", "help", "press", "api", "oauth", "challenge", "emails",
    "session", "s", "tags", "locations", "invites", "static", "graphql", "login", "signup", "sharer",
    "embed", "terms", "ar", "creators", "business", "download", "lite", "topics", "popular",
];

/// Accounts that are never the venue's or collective's: Instagram itself, our
/// own accounts, platforms whose widgets put their footer links on club sites,
/// and the placeholders of site templates.
const GENERIC_HANDLES: &[&str] = &[
    "instagram", "madebypiers", "piers", "insrt", "insrt.studio", "insrtstudio", "meta", "facebook",
    "username", "yourusername", "your_username", "yourname", "youraccount", "yourhandle", "handle",
    "user", "profile", "example", "mystore", "wix", "squarespace", "shopify", "wordpress", "elementor",
    "residentadvisor", "ra.co", "dicefm", "dice.fm", "shotgun.live", "shotgunlive", "shotgun", "eventbrite",
    "songkick", "bandsintown", "soundcloud", "spotify", "beatport", "boilerroom", "boilerroomtv", "mixmag",
    "djmag", "xceed", "xceed.me", "feverup", "weezevent", "linktree", "linktr.ee", "tiktok", "youtube",
];

/// A plausible profile handle: 1-30 of [a-z0-9._], no leading, trailing or
/// doubled dot, not only digits, not a reserved path or a generic account.
fn valid_handle(h: &str) -> bool {
    !h.is_empty()
        && h.len() <= 30
        && h.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.')
        && !h.starts_with('.')
        && !h.ends_with('.')
        && !h.contains("..")
        && !h.chars().all(|c| c.is_ascii_digit() || c == '.' || c == '_')
        && !RESERVED_PATHS.contains(&h)
        && !GENERIC_HANDLES.contains(&h)
}

fn profile_re() -> regex::Regex {
    // Host not preceded by another label (help.instagram.com is not a profile),
    // "\/" tolerated for links inside inline JSON.
    regex::Regex::new(r"(?i)(?:^|[^a-z0-9.\-])(?:www\.|m\.)?(?:instagram\.com|instagr\.am)((?:\\?/[a-z0-9_.]+)+)")
        .expect("valid regex")
}

/// The handle out of a profile URL's path ("/_u/club.name/" -> "club.name").
fn handle_from_path(path: &str) -> Option<String> {
    let path = path.replace("\\/", "/").to_lowercase();
    let mut segs = path.split('/').filter(|s| !s.is_empty());
    let mut first = segs.next()?;
    if first == "_u" {
        first = segs.next()?;
    }
    // A sentence ending right after the link ("instagram.com/club.").
    let h = first.trim_end_matches('.').to_string();
    valid_handle(&h).then_some(h)
}

/// Normalised handle from an Instagram profile URL, None for posts, reels,
/// share links, other sites and generic accounts.
pub(crate) fn handle_from_url(url: &str) -> Option<String> {
    let caps = profile_re().captures(url.trim())?;
    handle_from_path(caps.get(1)?.as_str())
}

/// A handle typed by hand ("@Club.Name", "club.name", or a profile URL).
/// Generic names are allowed here: the user knows whom he means.
pub fn normalize_handle_input(raw: &str) -> Option<String> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    if s.to_lowercase().contains("instagram.com") || s.to_lowercase().contains("instagr.am") {
        return handle_from_url(s);
    }
    let h: String = s.trim_start_matches('@').trim().trim_end_matches('/').to_lowercase();
    let ok = !h.is_empty()
        && h.len() <= 30
        && h.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.')
        && !h.starts_with('.')
        && !h.ends_with('.');
    ok.then_some(h)
}

/// Add the profiles a page links to, counting repeats (header + footer).
pub(crate) fn tally_handles(acc: &mut Vec<(String, usize)>, html: &str) {
    for caps in profile_re().captures_iter(html) {
        let Some(h) = caps.get(1).and_then(|m| handle_from_path(m.as_str())) else { continue };
        match acc.iter_mut().find(|(x, _)| *x == h) {
            Some((_, n)) => *n += 1,
            None => acc.push((h, 1)),
        }
    }
}

/// Whether a handle plausibly belongs to `name`: the domain rule of
/// enrich::domain_matches_name on the handle with dots and underscores folded
/// ("lesucre_lyon" for Le Sucre, "possession.paris" for Possession).
pub(crate) fn handle_matches_name(handle: &str, name: &str, city: Option<&str>) -> bool {
    core_matches_name(&normalise(handle), name, city)
}

/// The most plausible handle among those found on a venue's or collective's
/// pages. One matching the name wins (the closest match, then the most linked);
/// with none matching, a lone handle is taken only when `trust_single` (their
/// own site: a single footer link is theirs, several links are partners and
/// artists).
pub(crate) fn pick_instagram(cands: &[(String, usize)], name: &str, city: Option<&str>, trust_single: bool) -> Option<String> {
    let compact = normalise(name);
    let rank = |h: &str| -> u8 {
        let core = normalise(h);
        if !compact.is_empty() && core == compact {
            3
        } else if !compact.is_empty() && (core.starts_with(&compact) || core.ends_with(&compact)) {
            2
        } else {
            1
        }
    };
    cands
        .iter()
        .filter(|(h, _)| handle_matches_name(h, name, city))
        .max_by_key(|(h, n)| (rank(h), *n, std::cmp::Reverse(h.len())))
        .map(|(h, _)| h.clone())
        .or_else(|| (trust_single && cands.len() == 1).then(|| cands[0].0.clone()))
}

/// "Club Name (@clubname) • Instagram photos and videos": the account name of
/// an Instagram result title equals `name` (folded).
fn title_is_name(title: &str, name: &str) -> bool {
    let cut = ["(@", " • ", " | ", " on Instagram", " sur Instagram"]
        .iter()
        .filter_map(|m| title.find(m))
        .min()
        .unwrap_or(title.len());
    let t = normalise(&title[..cut]);
    t.len() >= 3 && t == normalise(name)
}

/// The Instagram profile in a Serper response, when one plausibly belongs to
/// `name`: a profile link (knowledge graph profiles, organic results) whose
/// handle matches the name or whose result title is the name, else a handle
/// quoted in a title or snippet that matches the name.
pub(crate) fn instagram_from_search(d: &serde_json::Value, name: &str, city: Option<&str>) -> Option<String> {
    let mut items: Vec<(String, String, String)> = Vec::new();
    if let Some(profiles) = d.pointer("/knowledgeGraph/profiles").and_then(|p| p.as_array()) {
        for p in profiles {
            let link = p.get("link").and_then(|x| x.as_str()).unwrap_or_default();
            items.push((link.to_string(), String::new(), String::new()));
        }
    }
    if let Some(org) = d.get("organic").and_then(|o| o.as_array()) {
        for it in org {
            let s = |k: &str| it.get(k).and_then(|x| x.as_str()).unwrap_or_default().to_string();
            items.push((s("link"), s("title"), s("snippet")));
        }
    }
    for (link, title, _) in &items {
        if let Some(h) = handle_from_url(link) {
            if handle_matches_name(&h, name, city) || title_is_name(title, name) {
                return Some(h);
            }
        }
    }
    let at = regex::Regex::new(r"@([A-Za-z0-9_.]{2,30})").expect("valid regex");
    for (_, title, snippet) in &items {
        let text = format!("{title} {snippet}");
        let mut found: Vec<(String, usize)> = Vec::new();
        tally_handles(&mut found, &text);
        if text.to_lowercase().contains("instagram") {
            for c in at.captures_iter(&text) {
                let h = c[1].to_lowercase().trim_end_matches('.').to_string();
                if valid_handle(&h) && !found.iter().any(|(x, _)| *x == h) {
                    found.push((h, 1));
                }
            }
        }
        if let Some(h) = found.into_iter().map(|(h, _)| h).find(|h| handle_matches_name(h, name, city)) {
            return Some(h);
        }
    }
    None
}

/// Store a collective's handle unless it already has one.
pub(crate) fn store_promoter_instagram(conn: &Connection, id: i64, handle: &str) {
    let _ = conn.execute(
        "UPDATE vi_promoter_orgs SET instagram = ?2, updated_at = datetime('now')
         WHERE id = ?1 AND trim(COALESCE(instagram, '')) = ''",
        params![id, handle],
    );
}

// ---------------------------------------------------------------------------
// "Chercher les Instagram" over collectives
// ---------------------------------------------------------------------------

/// Serper queries one Instagram pass may spend (a collective whose site gave
/// no handle costs one). The rest are left unchecked for the next click.
pub const IG_SEARCH_CAP: usize = 500;

/// Pages of a collective's site read for its Instagram link (the homepage
/// first: the link is almost always in the header or footer).
const IG_PATHS: &[&str] = &["/contact", "/about"];

/// One collective: its site's pages, then (when the payload allows it and a
/// Serper key is set) a `"<nom>" <ville> instagram` search. Marked checked
/// when a handle was found or the search could run, so an interrupted or
/// capped pass resumes with the rest.
pub async fn process_instagram_task(pool: &DbPool, payload: &str) -> Result<(), ra::RaError> {
    let perr = |m: String| ra::RaError { message: m, status: None, retryable: false };
    let v: serde_json::Value = serde_json::from_str(payload).map_err(|e| perr(e.to_string()))?;
    let id = v["promoter_id"].as_i64().unwrap_or(0);
    let may_search = v["ig_search"].as_bool().unwrap_or(false);
    let (nom, ville, site, insta): (String, Option<String>, Option<String>, Option<String>) = {
        let conn = pool.get().map_err(|e| perr(e.to_string()))?;
        conn.query_row(
            "SELECT nom, ville, site_web, instagram FROM vi_promoter_orgs WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .map_err(|e| perr(e.to_string()))?
    };
    let mark = |found: Option<&str>| {
        if let Ok(conn) = pool.get() {
            if let Some(h) = found {
                store_promoter_instagram(&conn, id, h);
            }
            let _ = conn.execute("UPDATE vi_promoter_orgs SET instagram_checked_at = datetime('now') WHERE id = ?1", [id]);
        }
    };
    if insta.as_deref().is_some_and(|h| !h.trim().is_empty()) {
        mark(None);
        return Ok(());
    }

    let client = crawl_client();
    let mut handles: Vec<(String, usize)> = Vec::new();
    let mut found: Option<String> = None;
    // A site that is itself an Instagram profile.
    if let Some(h) = site.as_deref().and_then(handle_from_url) {
        found = Some(h);
    }
    if let Some(raw) = site.as_deref().and_then(normalize_site).filter(|s| !is_foreign_venue_site(s)).filter(|_| found.is_none()) {
        let mut base = origin_of(&raw);
        for cand in base_candidates(&raw) {
            if let Some(b) = fetch_html(&client, &cand).await {
                base = cand;
                tally_handles(&mut handles, &b);
                break;
            }
        }
        found = pick_instagram(&handles, &nom, ville.as_deref(), true);
        for p in IG_PATHS {
            if found.is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            if let Some(b) = fetch_html(&client, &format!("{base}{p}")).await {
                tally_handles(&mut handles, &b);
                found = pick_instagram(&handles, &nom, ville.as_deref(), true);
            }
        }
    }
    let mut searched = false;
    if found.is_none() && may_search && !nom.trim().is_empty() {
        let key = pool
            .get()
            .ok()
            .and_then(|c| crate::secrets::read_setting(&c, "serper_api_key"))
            .filter(|k| !k.trim().is_empty());
        if let Some(key) = key {
            let q = format!("\"{}\" {} instagram", nom.trim(), ville.clone().unwrap_or_default()).trim().to_string();
            if let Some(d) = super::enrich::serper_search(&client, &key, &q).await {
                found = instagram_from_search(&d, &nom, ville.as_deref());
                searched = true;
            }
            // Politeness / rate-limit courtesy toward the search API.
            tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        }
    }
    if found.is_some() || searched {
        mark(found.as_deref());
    }
    Ok(())
}

/// Queue the Instagram pass: `ids`, else the enriched active collectives with
/// at least `min_events` nights, no handle and not checked yet (best fit
/// first, 2000 at most). The first IG_SEARCH_CAP may use a web search.
/// Error code the UI translates: "social.instagram_none".
#[tauri::command]
pub async fn vi_start_promoter_instagram(
    app: AppHandle,
    state: State<'_, AppState>,
    ids: Option<Vec<i64>>,
    min_events: Option<i64>,
) -> Result<i64, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let ids = match ids.filter(|v| !v.is_empty()) {
        Some(v) => v,
        None => instagram_candidates(&conn, min_events.unwrap_or(3))?,
    };
    if ids.is_empty() {
        return Err("social.instagram_none".into());
    }
    conn.execute(
        "INSERT INTO vi_runs (type, params, statut, started_at) VALUES ('instagram_promoters', ?1, 'en_cours', datetime('now'))",
        [json!({ "promoters": ids.len() }).to_string()],
    )
    .map_err(|e| e.to_string())?;
    let run_id = conn.last_insert_rowid();
    {
        let mut st = conn
            .prepare("INSERT INTO vi_tasks (run_id, type, payload, statut) VALUES (?1, 'instagram_promoter', ?2, 'en_attente')")
            .map_err(|e| e.to_string())?;
        for (i, id) in ids.iter().enumerate() {
            let payload = json!({ "promoter_id": id, "ig_search": i < IG_SEARCH_CAP });
            st.execute(params![run_id, payload.to_string()]).map_err(|e| e.to_string())?;
        }
    }
    crate::commands::harvest::spawn_worker(state.pool.clone(), state.cancels.clone(), app, run_id);
    Ok(run_id)
}

pub(crate) fn instagram_candidates(conn: &Connection, min_events: i64) -> Result<Vec<i64>, String> {
    let sql = format!(
        "SELECT o.id FROM vi_promoter_orgs o
         WHERE o.statut = 'actif' AND o.house_promoter = 0 AND o.enriched_at IS NOT NULL
           AND MAX(o.nb_events_12m, o.nb_events_total) >= {}
           AND trim(COALESCE(o.instagram, '')) = '' AND o.instagram_checked_at IS NULL
         ORDER BY o.fit_score IS NULL, o.fit_score DESC, o.nb_events_total DESC LIMIT 2000",
        min_events.max(1)
    );
    let mut st = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = st.query_map([], |r| r.get::<_, i64>(0)).map_err(|e| e.to_string())?;
    Ok(rows.flatten().collect())
}

// ---------------------------------------------------------------------------
// Assisted DM: target, draft, record
// ---------------------------------------------------------------------------

/// Who a DM goes to, with the true facts it may cite.
#[derive(Debug, Clone, Default, Serialize)]
pub struct DmTarget {
    pub contact_id: Option<i64>,
    pub venue_id: Option<i64>,
    pub promoter_id: Option<i64>,
    pub name: String,
    pub city: String,
    pub country: String,
    pub lang: String,
    pub handle: Option<String>,
    /// "venue" or "promoter" (collective).
    pub kind: String,
    /// Artists close to ours they booked (vi_evidence / lineups), recent first.
    pub artists: Vec<String>,
    /// Names of their recent nights (collectives).
    pub nights: Vec<String>,
}

fn opt_handle(h: Option<String>) -> Option<String> {
    h.and_then(|h| normalize_handle_input(&h))
}

fn venue_facts(conn: &Connection, t: &mut DmTarget, vid: i64) {
    if let Ok((nom, ville, pays, ig, cid)) = conn.query_row(
        "SELECT nom, ville, pays, instagram, crm_contact_id FROM vi_venues WHERE id = ?1",
        [vid],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, Option<String>>(3)?, r.get::<_, Option<i64>>(4)?)),
    ) {
        if t.name.is_empty() {
            t.name = nom;
        }
        if let Some(v) = ville.filter(|v| !v.trim().is_empty()) {
            t.city = v;
        }
        if let Some(p) = pays.filter(|p| !p.trim().is_empty()) {
            t.country = iso_country(&p);
        }
        t.handle = t.handle.clone().or(opt_handle(ig));
        t.contact_id = t.contact_id.or(cid);
        t.venue_id = Some(vid);
        t.kind = "venue".into();
    }
    if let Ok(mut st) = conn.prepare(
        "SELECT artiste FROM vi_evidence WHERE venue_id = ?1
         GROUP BY lower(artiste) ORDER BY MAX(date_event) DESC, COUNT(*) DESC LIMIT 3",
    ) {
        t.artists = st.query_map([vid], |r| r.get(0)).map(|it| it.flatten().collect()).unwrap_or_default();
    }
}

fn promoter_facts(conn: &Connection, t: &mut DmTarget, pid: i64) {
    if let Ok((nom, ville, pays, ig, cid)) = conn.query_row(
        "SELECT nom, ville, pays, instagram, crm_contact_id FROM vi_promoter_orgs WHERE id = ?1",
        [pid],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, Option<String>>(3)?, r.get::<_, Option<i64>>(4)?)),
    ) {
        if t.name.is_empty() {
            t.name = nom;
        }
        if let Some(v) = ville.filter(|v| !v.trim().is_empty()) {
            t.city = v;
        }
        if let Some(p) = pays.filter(|p| !p.trim().is_empty()) {
            t.country = iso_country(&p);
        }
        t.handle = t.handle.clone().or(opt_handle(ig));
        t.contact_id = t.contact_id.or(cid);
        t.promoter_id = Some(pid);
        t.kind = "promoter".into();
    }
    // Artists of their lineups, those on our comparables list first.
    if let Ok(mut st) = conn.prepare(
        "SELECT MIN(a.nom) FROM vi_event_promoters ep
         JOIN vi_event_artists a ON a.ra_event_id = ep.ra_event_id
         LEFT JOIN vi_venue_events e ON e.ra_event_id = ep.ra_event_id
         WHERE ep.promoter_id = ?1
         GROUP BY a.nom_normalise
         ORDER BY EXISTS (SELECT 1 FROM artist_comparables c WHERE c.nom_normalise = a.nom_normalise) DESC,
                  MAX(e.date_event) DESC, COUNT(*) DESC
         LIMIT 3",
    ) {
        t.artists = st.query_map([pid], |r| r.get(0)).map(|it| it.flatten().collect()).unwrap_or_default();
    }
    if let Ok(mut st) = conn.prepare(
        "SELECT e.titre FROM vi_event_promoters ep JOIN vi_venue_events e ON e.ra_event_id = ep.ra_event_id
         WHERE ep.promoter_id = ?1 AND trim(COALESCE(e.titre, '')) <> ''
         GROUP BY lower(e.titre) ORDER BY MAX(e.date_event) DESC LIMIT 2",
    ) {
        t.nights = st.query_map([pid], |r| r.get(0)).map(|it| it.flatten().collect()).unwrap_or_default();
    }
}

/// Resolve the target of a DM from a contact, a venue or a collective. A
/// contact linked to a venue or collective of Discovery gets its facts.
pub fn dm_target(conn: &Connection, contact_id: Option<i64>, venue_id: Option<i64>, promoter_id: Option<i64>) -> Result<DmTarget, String> {
    let mut t = DmTarget { kind: "contact".into(), ..Default::default() };
    let (mut vid, mut pid) = (venue_id, promoter_id);
    if let Some(cid) = contact_id {
        let (name, venue, area, ig, category): (String, Option<String>, Option<String>, Option<String>, Option<String>) = conn
            .query_row("SELECT name, venue, area, instagram, category FROM contacts WHERE id = ?1", [cid], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })
            .map_err(|e| e.to_string())?;
        t.contact_id = Some(cid);
        t.name = venue.filter(|v| !v.trim().is_empty()).unwrap_or(name);
        t.city = area.unwrap_or_default();
        t.handle = opt_handle(ig);
        if category.as_deref() == Some("promoter") {
            t.kind = "promoter".into();
        }
        if vid.is_none() && pid.is_none() {
            vid = conn.query_row("SELECT id FROM vi_venues WHERE crm_contact_id = ?1 LIMIT 1", [cid], |r| r.get(0)).optional().ok().flatten();
            pid = conn
                .query_row("SELECT id FROM vi_promoter_orgs WHERE crm_contact_id = ?1 LIMIT 1", [cid], |r| r.get(0))
                .optional()
                .ok()
                .flatten();
        }
    }
    if let Some(v) = vid {
        venue_facts(conn, &mut t, v);
    } else if let Some(p) = pid {
        promoter_facts(conn, &mut t, p);
    }
    if t.name.trim().is_empty() {
        return Err("social.no_target".into());
    }
    if t.country.is_empty() {
        // Unknown country (a contact typed by hand): the residency rule.
        t.country = super::residency::zone::Zone::default().default_country();
    }
    t.lang = language_for(&t.country, Some(&t.city), "auto").to_string();
    Ok(t)
}

/// The link a DM carries: the contact's personal press link (already used in
/// a residency email, else created on insrt when a press kit is configured),
/// else the artist's mix, else the residency campaign's lead asset.
pub fn dm_link(conn: &Connection, t: &DmTarget, insrt: Option<&dyn Insrt>) -> Option<String> {
    let nonempty = |s: Option<String>| s.map(|x| x.trim().to_string()).filter(|x| x.starts_with("http"));
    if let Some(cid) = t.contact_id {
        let existing: Option<String> = conn
            .query_row(
                "SELECT press_url FROM campaign_messages WHERE contact_id = ?1 AND trim(COALESCE(press_url, '')) <> ''
                 ORDER BY id DESC LIMIT 1",
                [cid],
                |r| r.get(0),
            )
            .optional()
            .ok()
            .flatten();
        if let Some(u) = nonempty(existing) {
            return Some(u);
        }
    }
    let cfg = latest_residency_config(conn);
    if let (Some(cid), Some(client), Some(kit)) =
        (t.contact_id, insrt, cfg.as_ref().and_then(|c| c.press_kit_id.clone()).filter(|k| !k.trim().is_empty()))
    {
        let email: String = conn
            .query_row("SELECT COALESCE(email, '') FROM contacts WHERE id = ?1", [cid], |r| r.get(0))
            .unwrap_or_default();
        let rec = Recipient { name: t.name.clone(), email, organisation: t.name.clone() };
        if let Ok(l) = client.press_link(&kit, cid, &rec, &t.lang) {
            if let Some(u) = nonempty(Some(l.url)) {
                return Some(u);
            }
        }
    }
    let mix: Option<String> = conn
        .query_row(
            "SELECT mix_url FROM artists WHERE id = COALESCE(?1, (SELECT MIN(id) FROM artists))",
            [crate::commands::artists::default_artist_id(conn)],
            |r| r.get(0),
        )
        .optional()
        .ok()
        .flatten()
        .flatten();
    if let Some(u) = nonempty(mix) {
        return Some(u);
    }
    let cfg = cfg.unwrap_or_default();
    cfg.assets.iter().map(|a| a.url.trim().to_string()).find(|u| u.starts_with("http"))
}

fn latest_residency_config(conn: &Connection) -> Option<super::residency::Config> {
    let raw: String = conn
        .query_row(
            "SELECT r.config FROM residency_campaigns r JOIN campaigns c ON c.id = r.campaign_id
             ORDER BY (c.status = 'active') DESC, r.updated_at DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()
        .ok()
        .flatten()?;
    serde_json::from_str(&raw).ok()
}

/// Artist name and genres for the DM ("Piers", "house, tech house").
fn artist_line(conn: &Connection) -> (String, String) {
    let row: Option<(String, Option<String>)> = conn
        .query_row(
            "SELECT name, genres FROM artists WHERE id = COALESCE(?1, (SELECT MIN(id) FROM artists))",
            [crate::commands::artists::default_artist_id(conn)],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .ok()
        .flatten();
    let (name, genres) = row.unwrap_or_else(|| ("Piers".into(), None));
    let genres = genres
        .map(|g| g.trim().to_lowercase())
        .filter(|g| !g.is_empty() && g.len() <= 40)
        .unwrap_or_else(|| "house, tech house".into());
    (name, genres)
}

/// The DM without AI: 2-3 sentences, one true detail when there is one.
pub fn template_dm(t: &DmTarget, artist: &str, genres: &str, link: Option<&str>) -> String {
    let fr = t.lang != "en";
    let at_venue = t.kind == "venue";
    let fact = match (t.artists.first(), t.nights.first()) {
        (Some(a), _) if fr => format!(
            " J'ai vu passer {a} {}, ça m'a donné envie de t'écrire.",
            if at_venue { "chez vous" } else { "à une de vos soirées" }
        ),
        (Some(a), _) => format!(" Saw {a} played {}, which is why I'm writing.", if at_venue { "at yours" } else { "one of your nights" }),
        (None, Some(n)) if fr => format!(" J'ai vu passer vos soirées {n}, ça m'a donné envie de t'écrire."),
        (None, Some(n)) => format!(" Been following {n} for a bit, which is why I'm writing."),
        (None, None) if fr => " Je suis tombé sur votre programmation et ça m'a donné envie de t'écrire.".to_string(),
        (None, None) => " Came across your programming and wanted to say hi.".to_string(),
    };
    let ask = match (link.filter(|l| !l.trim().is_empty()), fr) {
        (Some(l), true) => format!(" Si un jour vous cherchez quelqu'un pour une date, je te laisse mon dernier mix : {}", l.trim()),
        (Some(l), false) => format!(" If you're ever looking for someone for a night, here's my latest mix: {}", l.trim()),
        (None, true) => " Si un jour vous cherchez quelqu'un pour une date, ce serait avec plaisir.".to_string(),
        (None, false) => " If you're ever looking for someone for a night, I'd be up for it.".to_string(),
    };
    if fr {
        format!("Salut ! Moi c'est Thibault, je mixe sous le nom de {artist} ({genres}).{fact}{ask}")
    } else {
        format!("Hey! I'm Thibault, I play as {artist} ({genres}).{fact}{ask}")
    }
}

const DM_RULES: &str = "You write a first Instagram direct message from a DJ/producer to a club or a party collective. \
It is sent by hand from the artist's own account, so it must read like a real DM typed on a phone: short, warm, casual, \
confident without selling. Nothing like an email.

Voice (French example, adapt, do not copy):
---
Salut ! Moi c'est Thibault, je mixe sous le nom de Piers (house, tech house). J'ai vu passer DJ Seinfeld chez vous, \
ça m'a donné envie de t'écrire. Si un jour vous cherchez quelqu'un pour une date, je te laisse mon dernier mix : https://example.link
---

Rules:
- 2 to 4 short sentences, under 70 words. One paragraph. No subject, no greeting line on its own, no signature \
(at most \"Thibault\" at the very end), no hashtags, no emojis.
- The sender is Thibault, who plays as the artist: say it once, simply.
- At most one concrete, true detail taken only from the facts given (an artist they booked, the name of their night). \
Never invent a fact, a name or a number. If no fact is usable, skip it rather than flatter.
- The link exactly once, copied verbatim, at the end of a sentence. If no link is given, no link.
- No question that pressures, no \"let me know\". A light, open ending.
";

const DM_TAIL: &str = "- Write in the language requested. In French, \"vous\" for the crew and \"tu\" for whoever reads is fine, never formal.

Answer with JSON only: {\"body\": \"...\"}.";

/// The DM rules with the house voice shared by every text written under
/// Thibault's name (residency/content.rs).
fn dm_system() -> String {
    format!("{DM_RULES}{HOUSE_VOICE}{DM_TAIL}")
}

fn dm_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {"body": {"type": "string"}},
        "required": ["body"],
        "additionalProperties": false
    })
}

fn dm_facts(t: &DmTarget) -> String {
    let mut f = vec![format!("{}: {}", if t.kind == "venue" { "Club" } else { "Collective / promoter" }, t.name)];
    if !t.city.trim().is_empty() {
        f.push(format!("City: {} ({})", t.city, t.country));
    }
    if !t.artists.is_empty() {
        f.push(format!("Artists close to ours they booked recently: {}", t.artists.join(", ")));
    }
    if !t.nights.is_empty() {
        f.push(format!("Their recent nights: {}", t.nights.join(", ")));
    }
    f.join("\n")
}

/// A DM body the model wrote, cleaned, or None when unusable (empty, an
/// email in disguise).
pub fn parse_dm(text: &str, link: Option<&str>) -> Option<String> {
    let v = claude::extract_json(text)?;
    let mut body = claude::no_dashes(v.get("body")?.as_str()?.trim());
    if body.is_empty() || body.chars().count() > 600 || body.contains("Cordialement") {
        return None;
    }
    if let Some(l) = link.map(str::trim).filter(|l| !l.is_empty()) {
        if !body.contains(l) {
            body.push(' ');
            body.push_str(l);
        }
    }
    Some(body)
}

#[derive(Debug, Clone, Serialize)]
pub struct DmDraft {
    pub body: String,
    pub lang: String,
    pub handle: Option<String>,
    pub link: Option<String>,
    /// The model that wrote it, or "template".
    pub generated_by: String,
    pub name: String,
    pub contact_id: Option<i64>,
    pub venue_id: Option<i64>,
    pub promoter_id: Option<i64>,
    /// Why the template was used although an AI key is set.
    pub note: Option<String>,
}

/// Draft the DM for a target: Claude when a key is set (one retry), else the
/// template.
pub fn build_dm(conn: &Connection, t: &DmTarget, llm: Option<&dyn Llm>, insrt: Option<&dyn Insrt>) -> DmDraft {
    let link = dm_link(conn, t, insrt);
    let (artist, genres) = artist_line(conn);
    let mut note = None;
    let mut out = DmDraft {
        body: String::new(),
        lang: t.lang.clone(),
        handle: t.handle.clone(),
        link: link.clone(),
        generated_by: "template".into(),
        name: t.name.clone(),
        contact_id: t.contact_id,
        venue_id: t.venue_id,
        promoter_id: t.promoter_id,
        note: None,
    };
    if let Some(llm) = llm {
        let model = claude::drafting_model(conn);
        let req = LlmRequest {
            model: model.clone(),
            max_tokens: 400,
            system: vec![SystemBlock { text: dm_system(), cache: false }],
            user: format!(
                "Language: {}\nArtist: {} ({})\nLink to include: {}\n\nFacts:\n{}",
                if t.lang == "en" { "English" } else { "French" },
                artist,
                genres,
                link.clone().unwrap_or_else(|| "none".into()),
                dm_facts(t)
            ),
            schema: Some(dm_schema()),
        };
        for _ in 0..2 {
            match llm.complete(&req) {
                Ok(resp) => {
                    claude::log_usage(conn, "dm", None, &resp.model, &resp.usage);
                    if let Some(body) = parse_dm(&resp.text, link.as_deref()) {
                        out.body = body;
                        out.generated_by = if resp.model.is_empty() { model.clone() } else { resp.model };
                        return out;
                    }
                    note = Some("réponse inattendue".to_string());
                }
                Err(e) => {
                    note = Some(e);
                    break;
                }
            }
        }
    }
    out.body = template_dm(t, &artist, &genres, link.as_deref());
    out.note = note;
    out
}

// ---------------- Recording a DM ----------------

#[derive(Debug, Clone, Serialize)]
pub struct SocialTouch {
    pub id: i64,
    pub contact_id: Option<i64>,
    pub venue_id: Option<i64>,
    pub promoter_id: Option<i64>,
    pub channel: String,
    pub handle: Option<String>,
    pub body: Option<String>,
    pub sent_at: String,
    /// The contact's name (else the venue's or collective's).
    pub name: String,
    pub status: Option<String>,
}

/// "DM envoyé": the venue or collective joins the pipeline when it is not in
/// it yet (a DM is outreach), the touch is stored, the handle kept on the
/// contact, and the contact moves like after a first email
/// (status::on_dm_sent). Returns the touch.
pub fn record_dm(
    conn: &Connection,
    contact_id: Option<i64>,
    venue_id: Option<i64>,
    promoter_id: Option<i64>,
    handle: Option<&str>,
    body: Option<&str>,
) -> Result<SocialTouch, String> {
    let t = dm_target(conn, contact_id, venue_id, promoter_id)?;
    let cid = match t.contact_id {
        Some(c) => c,
        None => match (t.venue_id, t.promoter_id) {
            (Some(v), _) => crate::commands::harvest::promote_venue_inner(conn, v)?,
            (None, Some(p)) => crate::commands::promoters::promote_promoter_inner(conn, p)?,
            _ => return Err("social.no_target".into()),
        },
    };
    let handle = handle.and_then(normalize_handle_input).or(t.handle.clone());
    if let Some(h) = &handle {
        let _ = conn.execute(
            "UPDATE contacts SET instagram = ?2 WHERE id = ?1 AND trim(COALESCE(instagram, '')) = ''",
            params![cid, h],
        );
    }
    conn.execute(
        "INSERT INTO social_touches (contact_id, venue_id, promoter_id, channel, handle, body) VALUES (?1, ?2, ?3, 'instagram', ?4, ?5)",
        params![cid, t.venue_id, t.promoter_id, handle, body.map(str::trim).filter(|b| !b.is_empty())],
    )
    .map_err(|e| e.to_string())?;
    let id = conn.last_insert_rowid();
    crate::commands::status::on_dm_sent(conn, cid);
    touches(conn, "WHERE s.id = ?", vec![Box::new(id)])?.pop().ok_or_else(|| "touch not found".into())
}

fn touches(conn: &Connection, filter: &str, args: Vec<Box<dyn rusqlite::ToSql>>) -> Result<Vec<SocialTouch>, String> {
    let sql = format!(
        "SELECT s.id, s.contact_id, s.venue_id, s.promoter_id, s.channel, s.handle, s.body, s.sent_at,
                COALESCE(c.name, v.nom, o.nom, s.handle, ''), c.status
         FROM social_touches s
         LEFT JOIN contacts c ON c.id = s.contact_id
         LEFT JOIN vi_venues v ON v.id = s.venue_id
         LEFT JOIN vi_promoter_orgs o ON o.id = s.promoter_id
         {filter} ORDER BY s.sent_at DESC, s.id DESC LIMIT 500"
    );
    let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();
    let mut st = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = st
        .query_map(refs.as_slice(), |r| {
            Ok(SocialTouch {
                id: r.get(0)?,
                contact_id: r.get(1)?,
                venue_id: r.get(2)?,
                promoter_id: r.get(3)?,
                channel: r.get(4)?,
                handle: r.get(5)?,
                body: r.get(6)?,
                sent_at: r.get(7)?,
                name: r.get(8)?,
                status: r.get(9)?,
            })
        })
        .map_err(|e| e.to_string())?;
    Ok(rows.flatten().collect())
}

/// DMs sent, newest first. `since` / `until` are days (YYYY-MM-DD, both
/// inclusive) on sent_at; `contact_id` / `venue_id` / `promoter_id` narrow to
/// one target (a venue or collective also matches through its contact).
pub fn list_touches(
    conn: &Connection,
    since: Option<&str>,
    until: Option<&str>,
    contact_id: Option<i64>,
    venue_id: Option<i64>,
    promoter_id: Option<i64>,
) -> Result<Vec<SocialTouch>, String> {
    let mut w = String::from("WHERE 1=1");
    let mut a: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    if let Some(s) = since.filter(|s| !s.trim().is_empty()) {
        w.push_str(" AND date(s.sent_at) >= date(?)");
        a.push(Box::new(s.trim().to_string()));
    }
    if let Some(u) = until.filter(|s| !s.trim().is_empty()) {
        w.push_str(" AND date(s.sent_at) <= date(?)");
        a.push(Box::new(u.trim().to_string()));
    }
    if let Some(c) = contact_id {
        w.push_str(" AND s.contact_id = ?");
        a.push(Box::new(c));
    }
    if let Some(v) = venue_id {
        w.push_str(" AND (s.venue_id = ? OR s.contact_id = (SELECT crm_contact_id FROM vi_venues WHERE id = ?))");
        a.push(Box::new(v));
        a.push(Box::new(v));
    }
    if let Some(p) = promoter_id {
        w.push_str(" AND (s.promoter_id = ? OR s.contact_id = (SELECT crm_contact_id FROM vi_promoter_orgs WHERE id = ?))");
        a.push(Box::new(p));
        a.push(Box::new(p));
    }
    touches(conn, &w, a)
}

// ---------------- Commands ----------------

/// Draft a DM for a contact, a venue or a collective (exactly one id is
/// enough). Calls Claude when a key is set, hence off the main thread.
#[tauri::command]
pub async fn social_dm_draft(
    state: State<'_, AppState>,
    contact_id: Option<i64>,
    venue_id: Option<i64>,
    promoter_id: Option<i64>,
) -> Result<DmDraft, String> {
    let pool = state.pool.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let conn = pool.get().map_err(|e| e.to_string())?;
        let t = dm_target(&conn, contact_id, venue_id, promoter_id)?;
        let llm = claude::client(&conn);
        let insrt = super::residency::insrt::client(&conn);
        Ok(build_dm(&conn, &t, llm.as_deref(), insrt.as_deref()))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// "DM envoyé" (see record_dm).
#[tauri::command(async)]
pub fn social_record_dm(
    state: State<'_, AppState>,
    contact_id: Option<i64>,
    venue_id: Option<i64>,
    promoter_id: Option<i64>,
    handle: Option<String>,
    body: Option<String>,
) -> Result<SocialTouch, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    record_dm(&conn, contact_id, venue_id, promoter_id, handle.as_deref(), body.as_deref())
}

/// DMs sent between two days (inclusive), or for one target. Meant for the
/// week agenda ("done" actions) and the contact history.
#[tauri::command(async)]
pub fn list_social_touches(
    state: State<'_, AppState>,
    since: Option<String>,
    until: Option<String>,
    contact_id: Option<i64>,
    venue_id: Option<i64>,
    promoter_id: Option<i64>,
) -> Result<Vec<SocialTouch>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    list_touches(&conn, since.as_deref(), until.as_deref(), contact_id, venue_id, promoter_id)
}

/// Set (or clear) the handle of a venue or collective by hand.
#[tauri::command(async)]
pub fn social_set_instagram(
    state: State<'_, AppState>,
    venue_id: Option<i64>,
    promoter_id: Option<i64>,
    handle: Option<String>,
) -> Result<Option<String>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let h = handle.as_deref().and_then(normalize_handle_input);
    if let Some(v) = venue_id {
        conn.execute("UPDATE vi_venues SET instagram = ?2, updated_at = datetime('now') WHERE id = ?1", params![v, h])
            .map_err(|e| e.to_string())?;
    }
    if let Some(p) = promoter_id {
        conn.execute(
            "UPDATE vi_promoter_orgs SET instagram = ?2, instagram_checked_at = datetime('now'), updated_at = datetime('now') WHERE id = ?1",
            params![p, h],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::residency::claude::fake::FakeLlm;

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        crate::db::migrate_for_tests(&c).unwrap();
        c
    }

    #[test]
    fn handles_from_urls() {
        let ok = [
            ("https://www.instagram.com/b21_brussels?igsh=MX", "b21_brussels"),
            ("http://instagram.com/Club.Name/", "club.name"),
            ("instagram.com/possession.paris", "possession.paris"),
            ("https://m.instagram.com/concrete_paris/#", "concrete_paris"),
            ("https://www.instagram.com/_u/rexclub", "rexclub"),
            ("https://instagr.am/lesucre_lyon", "lesucre_lyon"),
            ("https:\\/\\/www.instagram.com\\/fold_london\\/", "fold_london"),
            ("Suivez-nous : instagram.com/badaboum.paris.", "badaboum.paris"),
        ];
        for (u, h) in ok {
            assert_eq!(handle_from_url(u).as_deref(), Some(h), "{u}");
        }
        for bad in [
            "https://instagram.com/p/abc",
            "https://www.instagram.com/reel/Cx1/",
            "https://www.instagram.com/explore/tags/techno/",
            "https://www.instagram.com/stories/club/123",
            "https://www.instagram.com/accounts/login/",
            "https://www.instagram.com/share/abc",
            "https://www.instagram.com/instagram",
            "https://www.instagram.com/madebypiers/",
            "https://instagram.com/yourusername",
            "https://help.instagram.com/581066165581870",
            "https://notinstagram.com/club",
            "https://www.instagram.com/",
            "https://club.com",
        ] {
            assert_eq!(handle_from_url(bad), None, "{bad}");
        }
        // The venue enrichment's helper is the same rule.
        assert_eq!(crate::commands::enrich::instagram_handle("https://instagram.com/p/abc"), None);
        assert_eq!(normalize_handle_input("@Possession.Paris"), Some("possession.paris".into()));
        assert_eq!(normalize_handle_input(" https://www.instagram.com/rexclub/ "), Some("rexclub".into()));
        assert_eq!(normalize_handle_input("pas un handle !"), None);
        assert_eq!(normalize_handle_input(""), None);
    }

    #[test]
    fn handles_from_pages_and_choice() {
        let html = r#"<a href="https://www.instagram.com/lesucre_lyon/">IG</a>
            <a href="https://instagram.com/p/xyz">post</a>
            <script>{"ig":"https:\/\/www.instagram.com\/lesucre_lyon\/"}</script>
            <a href="https://instagram.com/dj_guest">guest</a>
            <a href="https://instagram.com/instagram">app</a>"#;
        let mut acc = Vec::new();
        tally_handles(&mut acc, html);
        assert_eq!(acc, vec![("lesucre_lyon".to_string(), 2), ("dj_guest".to_string(), 1)]);
        assert_eq!(pick_instagram(&acc, "Le Sucre", Some("Lyon"), true).as_deref(), Some("lesucre_lyon"));
        // Several handles, none matching: no guess.
        let other = vec![("dj_guest".to_string(), 3), ("label_x".to_string(), 1)];
        assert_eq!(pick_instagram(&other, "Le Sucre", Some("Lyon"), true), None);
        // A lone footer link on their own site is theirs; not from elsewhere.
        let lone = vec![("lsclub".to_string(), 1)];
        assert_eq!(pick_instagram(&lone, "Le Sucre", Some("Lyon"), true).as_deref(), Some("lsclub"));
        assert_eq!(pick_instagram(&lone, "Le Sucre", Some("Lyon"), false), None);
        // The closest name wins over a looser match.
        let two = vec![("possessionfriends".to_string(), 3), ("possession".to_string(), 1)];
        assert_eq!(pick_instagram(&two, "Possession", Some("Paris"), false).as_deref(), Some("possession"));
        // A city word alone proves nothing.
        assert!(!handle_matches_name("paris_nights", "Vault Paris", Some("Paris")));
        assert!(handle_matches_name("rexclub", "Rex Club", Some("Paris")));
    }

    #[test]
    fn instagram_in_search_results() {
        let d = json!({
            "organic": [
                {"link": "https://www.instagram.com/djmag/", "title": "DJ Mag (@djmag)", "snippet": "Possession tonight"},
                {"link": "https://www.instagram.com/p/C1/", "title": "Possession (@possession.paris) • Instagram photos and videos", "snippet": ""},
                {"link": "https://possession.fr", "title": "Possession", "snippet": ""}
            ]
        });
        assert_eq!(instagram_from_search(&d, "Possession", Some("Paris")).as_deref(), Some("possession.paris"));
        // An account named exactly like the collective, with an unrelated handle.
        let d = json!({"organic": [{"link": "https://www.instagram.com/xyz_crew/", "title": "La Mamie's (@xyz_crew) • Instagram", "snippet": ""}]});
        assert_eq!(instagram_from_search(&d, "La Mamie's", Some("Paris")).as_deref(), Some("xyz_crew"));
        let d = json!({"organic": [{"link": "https://www.instagram.com/someone_else/", "title": "Someone", "snippet": ""}]});
        assert_eq!(instagram_from_search(&d, "Possession", Some("Paris")), None);
    }

    fn seed(c: &Connection) {
        c.execute_batch(
            "INSERT INTO artists (id, name, genres) VALUES (1, 'Piers', 'House, Tech House');
             INSERT INTO vi_promoter_orgs (id, cle, nom, nom_normalise, ville, pays, instagram)
                VALUES (7, 'n:possession', 'Possession', 'possession', 'Paris', 'FR', 'possession.paris');",
        )
        .unwrap();
    }

    #[test]
    fn dm_template_fallback() {
        let c = conn();
        seed(&c);
        let t = dm_target(&c, None, None, Some(7)).unwrap();
        assert_eq!((t.lang.as_str(), t.kind.as_str(), t.handle.as_deref()), ("fr", "promoter", Some("possession.paris")));
        let d = build_dm(&c, &t, None, None);
        assert_eq!(d.generated_by, "template");
        assert!(d.body.starts_with("Salut ! Moi c'est Thibault, je mixe sous le nom de Piers (house, tech house)."), "{}", d.body);
        // The residency lead asset is the link when nothing better is set.
        assert!(d.body.ends_with("https://www.youtube.com/watch?v=dnURpBKDE7w"), "{}", d.body);
        assert!(!d.body.contains('—'));
        assert!(d.body.split(". ").count() <= 4);
        // English by country, with a fact when there is one.
        let mut en = t.clone();
        en.lang = "en".into();
        en.kind = "venue".into();
        en.artists = vec!["DJ Seinfeld".into()];
        let b = template_dm(&en, "Piers", "house", Some("https://x.y/mix"));
        assert_eq!(b, "Hey! I'm Thibault, I play as Piers (house). Saw DJ Seinfeld played at yours, which is why I'm writing. If you're ever looking for someone for a night, here's my latest mix: https://x.y/mix");
        // Claude is used when available; a bad answer then a good one (link appended, dashes removed).
        let llm = FakeLlm::new(&["pas du json", r#"{"body":"Salut ! Vos soirées — top. Thibault"}"#]);
        let d = build_dm(&c, &t, Some(&llm), None);
        assert_eq!(d.generated_by, "claude-haiku-5-5");
        assert!(d.body.starts_with("Salut ! Vos soirées, top.") && d.body.ends_with("dnURpBKDE7w"), "{}", d.body);
        let seen = llm.seen.lock().unwrap();
        assert!(seen[0].user.contains("Language: French") && seen[0].user.contains("Possession"));
        drop(seen);
        // Claude failing: the template, with a note.
        let d = build_dm(&c, &t, Some(&FakeLlm::new(&[])), None);
        assert_eq!(d.generated_by, "template");
        assert!(d.note.is_some());
    }

    #[test]
    fn recording_a_dm_moves_the_pipeline() {
        let c = conn();
        seed(&c);
        // A collective not in the pipeline yet: promoted, touch stored, contacted.
        let t = record_dm(&c, None, None, Some(7), Some("@Possession.Paris"), Some("Salut !")).unwrap();
        let cid = t.contact_id.unwrap();
        assert_eq!((t.promoter_id, t.handle.as_deref(), t.status.as_deref()), (Some(7), Some("possession.paris"), Some("contacted")));
        let (ig, first, follow): (Option<String>, Option<String>, Option<String>) = c
            .query_row("SELECT instagram, first_contact, follow_up FROM contacts WHERE id = ?1", [cid], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap();
        assert_eq!(ig.as_deref(), Some("possession.paris"));
        let today = chrono::Utc::now().date_naive();
        assert_eq!(first, Some(today.to_string()));
        assert_eq!(follow, Some((today + chrono::Duration::days(crate::commands::status::FOLLOWUP_DAYS)).to_string()));
        // A second DM (through the contact) is a follow-up.
        let t2 = record_dm(&c, Some(cid), None, None, None, Some("Relance")).unwrap();
        assert_eq!((t2.status.as_deref(), t2.promoter_id), (Some("followed_up"), Some(7)));
        // Listed by day range and by target.
        let day = today.to_string();
        assert_eq!(list_touches(&c, Some(&day), Some(&day), None, None, None).unwrap().len(), 2);
        assert_eq!(list_touches(&c, None, None, None, None, Some(7)).unwrap().len(), 2);
        assert!(list_touches(&c, Some("2000-01-01"), Some("2000-01-02"), None, None, None).unwrap().is_empty());
        // A DM'd contact comes back in the follow-up queue when the date is due.
        c.execute("UPDATE contacts SET follow_up = date('now', '-1 day') WHERE id = ?1", [cid]).unwrap();
        let due = crate::commands::contacts::followups(&c).unwrap();
        assert!(due.iter().any(|f| f.contact_id == cid));
    }

    #[test]
    fn promoting_carries_the_handle() {
        let c = conn();
        c.execute_batch(
            "INSERT INTO vi_venues (id, nom, nom_normalise, ville, pays, instagram, statut)
                VALUES (3, 'Rex Club', 'rexclub', 'Paris', 'FR', 'rexclub', 'qualifie');",
        )
        .unwrap();
        let cid = crate::commands::harvest::promote_venue_inner(&c, 3).unwrap();
        let ig: Option<String> = c.query_row("SELECT instagram FROM contacts WHERE id = ?1", [cid], |r| r.get(0)).unwrap();
        assert_eq!(ig.as_deref(), Some("rexclub"));
        // A handle found later reaches the contact too, without overwriting one typed by hand.
        c.execute("UPDATE contacts SET instagram = 'typed_by_hand' WHERE id = ?1", [cid]).unwrap();
        c.execute("UPDATE vi_venues SET instagram = 'rex_other' WHERE id = 3", []).unwrap();
        let ig: Option<String> = c.query_row("SELECT instagram FROM contacts WHERE id = ?1", [cid], |r| r.get(0)).unwrap();
        assert_eq!(ig.as_deref(), Some("typed_by_hand"));
        // The candidates of the Instagram pass: enriched, no handle, not checked.
        c.execute_batch(
            "INSERT INTO vi_promoter_orgs (id, cle, nom, nom_normalise, nb_events_total, enriched_at)
                VALUES (1, 'a', 'A', 'a', 5, datetime('now')), (2, 'b', 'B', 'b', 5, NULL);
             INSERT INTO vi_promoter_orgs (id, cle, nom, nom_normalise, nb_events_total, enriched_at, instagram_checked_at)
                VALUES (4, 'd', 'D', 'd', 5, datetime('now'), datetime('now'));",
        )
        .unwrap();
        assert_eq!(instagram_candidates(&c, 3).unwrap(), vec![1]);
    }
}
