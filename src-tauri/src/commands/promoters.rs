//! Promoters and collectives as first-class targets.
//!
//! RA events carry their promoters. Until now they were only aggregated per
//! venue (vi_promoters: one row per venue and promoter name, with a count).
//! This module gives each promoter one identity across every room it uses
//! (vi_promoter_orgs, keyed on its RA id, else its normalised name), links it
//! to its nights (vi_event_promoters), finds its booking address with the
//! enrichment pipeline's quality rules, and promotes it into the pipeline as a
//! contact of category 'promoter'.
//!
//! Also stores each RA event's lineup (vi_event_artists), title and the date
//! we first saw it (vi_venue_events.titre / first_seen_at), which feed the fit
//! score (fit.rs) and the booking lead time (timing.rs).

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::json;
use tauri::{AppHandle, State};

use super::enrich::{
    base_candidates, domain_matches_name, email_domain, extract_contacts, host_of, instagram_handle,
    internal_links, is_denied_email_domain, is_foreign_venue_site, is_non_pitch_email, is_placeholder_email,
    normalize_site, origin_of, same_site, score_email, CRAWL_PATHS,
};
use super::{ra, timing};
use crate::db::{normalise, DbPool};
use crate::AppState;

// ---------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------

/// Additive and idempotent, run on every launch (db::migrate). Also runs the
/// fit schema (fit::migrate) so db.rs has a single entry point for this work.
pub(crate) fn migrate(conn: &Connection) {
    for stmt in [
        "ALTER TABLE vi_venue_events ADD COLUMN titre TEXT",
        "ALTER TABLE vi_venue_events ADD COLUMN first_seen_at TEXT",
        "CREATE INDEX IF NOT EXISTS idx_vi_venue_events_date ON vi_venue_events(date_event)",
        "CREATE TABLE IF NOT EXISTS vi_event_artists (
            ra_event_id   INTEGER NOT NULL,
            nom           TEXT NOT NULL,
            nom_normalise TEXT NOT NULL,
            PRIMARY KEY (ra_event_id, nom_normalise)
        ) WITHOUT ROWID",
        "CREATE INDEX IF NOT EXISTS idx_vi_event_artists_norm ON vi_event_artists(nom_normalise)",
        "CREATE TABLE IF NOT EXISTS vi_promoter_orgs (
            id             INTEGER PRIMARY KEY AUTOINCREMENT,
            cle            TEXT NOT NULL UNIQUE,
            nom            TEXT NOT NULL,
            nom_normalise  TEXT NOT NULL,
            ra_promoter_id INTEGER,
            ra_url         TEXT,
            site_web       TEXT,
            instagram      TEXT,
            ville          TEXT,
            ville_norm     TEXT,
            pays           TEXT,
            statut         TEXT NOT NULL DEFAULT 'actif',
            house_promoter INTEGER NOT NULL DEFAULT 0,
            nb_events_total INTEGER NOT NULL DEFAULT 0,
            nb_events_12m  INTEGER NOT NULL DEFAULT 0,
            nb_venues      INTEGER NOT NULL DEFAULT 0,
            last_event     TEXT,
            enriched_at    TEXT,
            crm_contact_id INTEGER REFERENCES contacts(id) ON DELETE SET NULL,
            fit_score      INTEGER,
            fit_detail     TEXT,
            fit_at         TEXT,
            lead_weeks     INTEGER,
            lead_source    TEXT,
            month_profile  TEXT,
            created_at     TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at     TEXT NOT NULL DEFAULT (datetime('now'))
        )",
        "CREATE INDEX IF NOT EXISTS idx_vi_promoter_orgs_norm ON vi_promoter_orgs(nom_normalise)",
        "CREATE INDEX IF NOT EXISTS idx_vi_promoter_orgs_crm ON vi_promoter_orgs(crm_contact_id)",
        "CREATE INDEX IF NOT EXISTS idx_vi_promoter_orgs_fit ON vi_promoter_orgs(fit_score DESC)",
        "CREATE TABLE IF NOT EXISTS vi_event_promoters (
            ra_event_id INTEGER NOT NULL,
            promoter_id INTEGER NOT NULL REFERENCES vi_promoter_orgs(id) ON DELETE CASCADE,
            PRIMARY KEY (ra_event_id, promoter_id)
        ) WITHOUT ROWID",
        "CREATE INDEX IF NOT EXISTS idx_vi_event_promoters_p ON vi_event_promoters(promoter_id)",
        "CREATE TABLE IF NOT EXISTS vi_promoter_contacts (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            promoter_id   INTEGER NOT NULL REFERENCES vi_promoter_orgs(id) ON DELETE CASCADE,
            type          TEXT NOT NULL DEFAULT 'email',
            valeur        TEXT NOT NULL,
            role_devine   TEXT,
            score         INTEGER NOT NULL DEFAULT 0,
            source_url    TEXT,
            source_method TEXT,
            verifie       INTEGER NOT NULL DEFAULT 0,
            created_at    TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE (promoter_id, type, valeur)
        )",
        "CREATE INDEX IF NOT EXISTS idx_vi_promoter_contacts_p ON vi_promoter_contacts(promoter_id, type, score DESC)",
        "CREATE INDEX IF NOT EXISTS idx_vi_promoter_contacts_val ON vi_promoter_contacts(valeur, promoter_id)",
        // Per-venue promoter rows -> their promoter identity.
        "ALTER TABLE vi_promoters ADD COLUMN org_id INTEGER REFERENCES vi_promoter_orgs(id) ON DELETE SET NULL",
        "CREATE INDEX IF NOT EXISTS idx_vi_promoters_org ON vi_promoters(org_id)",
    ] {
        let _ = conn.execute(stmt, []);
    }
    crate::commands::fit::migrate(conn);
    if sync_orgs(conn) > 0 {
        refresh_aggregates(conn);
    }
}

/// Stable identity of a promoter: its RA id, else its normalised name.
pub(crate) fn org_key(nom: &str, ra_id: Option<i64>) -> Option<String> {
    match ra_id {
        Some(id) => Some(format!("ra:{id}")),
        None => {
            let n = normalise(nom);
            (!n.is_empty()).then(|| format!("n:{n}"))
        }
    }
}

/// Insert (or find) a promoter identity, returns its id.
pub(crate) fn upsert_org(conn: &Connection, nom: &str, ra_id: Option<i64>) -> Option<i64> {
    let nom = nom.trim();
    let cle = org_key(nom, ra_id)?;
    let ra_url = ra_id.and_then(|i| ra::promoter_url(&i.to_string()));
    conn.execute(
        "INSERT INTO vi_promoter_orgs (cle, nom, nom_normalise, ra_promoter_id, ra_url) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(cle) DO UPDATE SET ra_url = COALESCE(vi_promoter_orgs.ra_url, excluded.ra_url)",
        params![cle, nom, normalise(nom), ra_id, ra_url],
    )
    .ok()?;
    conn.query_row("SELECT id FROM vi_promoter_orgs WHERE cle = ?1", [cle], |r| r.get(0)).ok()
}

/// Give every per-venue promoter row its identity. Returns how many rows were
/// linked (0 once everything is linked, so the launch cost is one query).
pub(crate) fn sync_orgs(conn: &Connection) -> usize {
    let rows: Vec<(i64, String, Option<String>)> = match conn
        .prepare("SELECT id, nom, ra_url FROM vi_promoters WHERE org_id IS NULL")
    {
        Ok(mut st) => st
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .map(|it| it.flatten().collect())
            .unwrap_or_default(),
        Err(_) => return 0,
    };
    if rows.is_empty() {
        return 0;
    }
    let Ok(tx) = conn.unchecked_transaction() else { return 0 };
    let mut n = 0;
    for (id, nom, url) in rows {
        let ra_id = url.as_deref().and_then(ra::promoter_id_from_url);
        if let Some(org) = upsert_org(&tx, &nom, ra_id) {
            if tx.execute("UPDATE vi_promoters SET org_id = ?2 WHERE id = ?1", params![id, org]).is_ok() {
                n += 1;
            }
        }
    }
    let _ = tx.commit();
    n
}

/// Everything record_listings stores beyond the venue counts: the event's
/// lineup, title and promoters. Idempotent (INSERT OR IGNORE throughout), so a
/// re-harvest of an event seen before the lineup table existed fills it in.
pub(crate) fn record_event_extras(conn: &Connection, ev: &ra::EventNode, venue_id: i64) {
    let Ok(eid) = ev.id.parse::<i64>() else { return };
    if let Some(t) = ev.title.as_deref().filter(|t| !t.trim().is_empty()) {
        let _ = conn.execute(
            "UPDATE vi_venue_events SET titre = COALESCE(titre, ?2) WHERE ra_event_id = ?1",
            params![eid, t.trim()],
        );
    }
    for a in &ev.artists {
        let norm = normalise(&a.name);
        if norm.is_empty() {
            continue;
        }
        let _ = conn.execute(
            "INSERT OR IGNORE INTO vi_event_artists (ra_event_id, nom, nom_normalise) VALUES (?1, ?2, ?3)",
            params![eid, a.name.trim(), norm],
        );
    }
    for p in &ev.promoters {
        if p.name.trim().is_empty() {
            continue;
        }
        let ra_id = p.id.as_deref().and_then(|s| s.parse::<i64>().ok());
        let Some(org) = upsert_org(conn, &p.name, ra_id) else { continue };
        let _ = conn.execute(
            "INSERT OR IGNORE INTO vi_event_promoters (ra_event_id, promoter_id) VALUES (?1, ?2)",
            params![eid, org],
        );
        let _ = conn.execute(
            "UPDATE vi_promoters SET org_id = ?3 WHERE venue_id = ?1 AND nom = ?2 AND org_id IS NULL",
            params![venue_id, p.name, org],
        );
    }
}

/// Set-based refresh of each promoter's totals, venues, main city and "house
/// promoter" flag (the venue itself listed as promoter of its own nights).
pub(crate) fn refresh_aggregates(conn: &Connection) {
    let _ = conn.execute_batch(
        "UPDATE vi_promoter_orgs SET
            nb_events_total = COALESCE((SELECT SUM(p.nb_events) FROM vi_promoters p WHERE p.org_id = vi_promoter_orgs.id), 0),
            nb_venues = (SELECT COUNT(DISTINCT p.venue_id) FROM vi_promoters p WHERE p.org_id = vi_promoter_orgs.id),
            last_event = (SELECT MAX(e.date_event) FROM vi_event_promoters ep
                          JOIN vi_venue_events e ON e.ra_event_id = ep.ra_event_id
                          WHERE ep.promoter_id = vi_promoter_orgs.id),
            house_promoter = EXISTS(SELECT 1 FROM vi_promoters p JOIN vi_venues v ON v.id = p.venue_id
                                    WHERE p.org_id = vi_promoter_orgs.id AND v.nom_normalise = vi_promoter_orgs.nom_normalise);
         UPDATE vi_promoter_orgs SET
            ville = (SELECT v.ville FROM vi_promoters p JOIN vi_venues v ON v.id = p.venue_id
                     WHERE p.org_id = vi_promoter_orgs.id AND v.ville IS NOT NULL ORDER BY p.nb_events DESC LIMIT 1),
            ville_norm = (SELECT v.ville_norm FROM vi_promoters p JOIN vi_venues v ON v.id = p.venue_id
                     WHERE p.org_id = vi_promoter_orgs.id AND v.ville IS NOT NULL ORDER BY p.nb_events DESC LIMIT 1),
            pays = (SELECT v.pays FROM vi_promoters p JOIN vi_venues v ON v.id = p.venue_id
                     WHERE p.org_id = vi_promoter_orgs.id AND v.pays IS NOT NULL ORDER BY p.nb_events DESC LIMIT 1);",
    );
}

// ---------------------------------------------------------------------------
// Email quality (pure)
// ---------------------------------------------------------------------------

/// Free mailbox providers: collectives often have no domain of their own.
const FREE_MAIL: &[&str] = &[
    "gmail.com", "googlemail.com", "hotmail.com", "hotmail.fr", "hotmail.co.uk", "outlook.com", "outlook.fr",
    "live.com", "live.fr", "yahoo.com", "yahoo.fr", "yahoo.co.uk", "icloud.com", "me.com", "proton.me",
    "protonmail.com", "gmx.de", "gmx.net", "web.de", "orange.fr", "free.fr", "sfr.fr", "laposte.net", "wanadoo.fr",
];

pub(crate) fn is_free_mail(domain: &str) -> bool {
    let d = domain.trim().to_lowercase();
    FREE_MAIL.contains(&d.as_str())
}

/// Write-time rule for a promoter address, stricter than the venue one (a
/// collective has no room whose page we crawl; anything we read is someone's
/// page). Kept when it is not a placeholder / non-pitch / denied-domain
/// address and:
/// - sits on the promoter's own site domain (RA-listed, or found by search
///   with a name-matching domain), or
/// - its domain matches the promoter's name (domain_matches_name), or
/// - it is a free mailbox whose local part carries the name
///   (wearecollectifx@gmail.com for "Collectif X").
pub(crate) fn keep_promoter_email(email: &str, site_host: Option<&str>, name: &str, city: Option<&str>) -> bool {
    if is_placeholder_email(email) || is_non_pitch_email(email) {
        return false;
    }
    let d = email_domain(email);
    if is_denied_email_domain(d) {
        return false;
    }
    if site_host.is_some_and(|h| !h.is_empty() && same_site(d, h)) {
        return true;
    }
    if is_free_mail(d) {
        let local = normalise(email.split('@').next().unwrap_or(""));
        return super::enrich::name_tokens(name)
            .iter()
            .filter(|t| city.map_or(true, |c| normalise(c) != **t))
            .any(|t| t.len() >= 4 && local.contains(t.as_str()));
    }
    domain_matches_name(d, name, city)
}

// ---------------------------------------------------------------------------
// Read side
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct PromoterRow {
    pub id: i64,
    pub nom: String,
    pub ville: Option<String>,
    pub pays: Option<String>,
    pub ra_url: Option<String>,
    pub site_web: Option<String>,
    pub nb_events_12m: i64,
    pub nb_events_total: i64,
    pub nb_venues: i64,
    /// Top venues by number of nights there.
    pub venues: Vec<String>,
    pub last_event: Option<String>,
    pub best_email: Option<String>,
    pub nb_emails: i64,
    pub enriched: bool,
    pub crm_contact_id: Option<i64>,
    pub house_promoter: bool,
    pub fit_score: Option<i64>,
    pub fit_detail: Option<String>,
    pub timing: timing::Timing,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct PromoterFilter {
    #[serde(default)]
    pub search: Option<String>,
    #[serde(default)]
    pub pays: Option<String>,
    #[serde(default)]
    pub has_email: Option<bool>,
    #[serde(default)]
    pub fit_min: Option<i64>,
    /// Include venues listed as the promoter of their own nights.
    #[serde(default)]
    pub include_house: Option<bool>,
    #[serde(default)]
    pub in_pipeline: Option<bool>,
    /// Promoters with at least this many nights (12 months if known, else total).
    #[serde(default)]
    pub min_events: Option<i64>,
}

fn filters(f: &PromoterFilter) -> (String, Vec<Box<dyn rusqlite::ToSql>>) {
    let mut w = String::from(" AND o.statut = 'actif'");
    let mut a: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    if f.include_house != Some(true) {
        w.push_str(" AND o.house_promoter = 0");
    }
    if let Some(q) = f.search.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        w.push_str(" AND (o.nom LIKE ? OR o.ville LIKE ?)");
        let like = format!("%{q}%");
        a.push(Box::new(like.clone()));
        a.push(Box::new(like));
    }
    if let Some(p) = f.pays.clone().filter(|s| !s.is_empty() && s != "all") {
        w.push_str(" AND o.pays = ?");
        a.push(Box::new(p));
    }
    if f.has_email == Some(true) {
        w.push_str(" AND EXISTS(SELECT 1 FROM vi_promoter_contacts c WHERE c.promoter_id = o.id AND c.type = 'email')");
    }
    if let Some(m) = f.fit_min.filter(|m| *m > 0) {
        w.push_str(" AND o.fit_score >= ?");
        a.push(Box::new(m));
    }
    if f.in_pipeline == Some(true) {
        w.push_str(" AND o.crm_contact_id IS NOT NULL");
    }
    if let Some(m) = f.min_events.filter(|m| *m > 0) {
        w.push_str(" AND MAX(o.nb_events_12m, o.nb_events_total) >= ?");
        a.push(Box::new(m));
    }
    (w, a)
}

pub fn count_promoters(conn: &Connection, f: &PromoterFilter) -> Result<i64, String> {
    let (w, a) = filters(f);
    let refs: Vec<&dyn rusqlite::ToSql> = a.iter().map(|b| b.as_ref()).collect();
    conn.query_row(&format!("SELECT COUNT(*) FROM vi_promoter_orgs o WHERE 1=1{w}"), refs.as_slice(), |r| r.get(0))
        .map_err(|e| e.to_string())
}

const ROW_SELECT: &str = "SELECT o.id, o.nom, o.ville, o.pays, o.ra_url, o.site_web, o.nb_events_12m, o.nb_events_total,
        o.nb_venues, o.last_event,
        (SELECT valeur FROM vi_promoter_contacts c WHERE c.promoter_id = o.id AND c.type = 'email'
          ORDER BY c.score DESC, c.id LIMIT 1),
        (SELECT COUNT(*) FROM vi_promoter_contacts c WHERE c.promoter_id = o.id AND c.type = 'email'),
        o.enriched_at IS NOT NULL, o.crm_contact_id, o.house_promoter, o.fit_score, o.fit_detail,
        o.lead_weeks, o.month_profile
 FROM vi_promoter_orgs o";

/// Rows of ROW_SELECT + `tail` (WHERE / ORDER / LIMIT), with their top venues.
fn query_rows(
    conn: &Connection,
    tail: &str,
    args: &[Box<dyn rusqlite::ToSql>],
    today: chrono::NaiveDate,
) -> Result<Vec<PromoterRow>, String> {
    let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();
    let mut st = conn.prepare(&format!("{ROW_SELECT} {tail}")).map_err(|e| e.to_string())?;
    let mut venues_st = conn
        .prepare(
            "SELECT v.nom FROM vi_promoters p JOIN vi_venues v ON v.id = p.venue_id
             WHERE p.org_id = ?1 ORDER BY p.nb_events DESC, v.nom LIMIT 3",
        )
        .map_err(|e| e.to_string())?;
    let rows = st
        .query_map(refs.as_slice(), |r| {
            let lead: Option<i64> = r.get(17)?;
            let prof: Option<String> = r.get(18)?;
            Ok(PromoterRow {
                id: r.get(0)?,
                nom: r.get(1)?,
                ville: r.get(2)?,
                pays: r.get(3)?,
                ra_url: r.get(4)?,
                site_web: r.get(5)?,
                nb_events_12m: r.get(6)?,
                nb_events_total: r.get(7)?,
                nb_venues: r.get(8)?,
                venues: Vec::new(),
                last_event: r.get(9)?,
                best_email: r.get(10)?,
                nb_emails: r.get(11)?,
                enriched: r.get::<_, i64>(12)? != 0,
                crm_contact_id: r.get(13)?,
                house_promoter: r.get::<_, i64>(14)? != 0,
                fit_score: r.get(15)?,
                fit_detail: r.get(16)?,
                timing: timing::window_for_row(today, lead, prof.as_deref()),
            })
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        let mut r = r.map_err(|e| e.to_string())?;
        r.venues = venues_st
            .query_map([r.id], |x| x.get::<_, String>(0))
            .map(|it| it.flatten().collect())
            .unwrap_or_default();
        out.push(r);
    }
    Ok(out)
}

/// `sort`: "fit" (default), "events", "recent" or "name".
pub fn list_promoters(
    conn: &Connection,
    f: &PromoterFilter,
    sort: Option<&str>,
    limit: i64,
    offset: i64,
    today: chrono::NaiveDate,
) -> Result<Vec<PromoterRow>, String> {
    let (w, a) = filters(f);
    let order = match sort.unwrap_or("fit") {
        "events" => "MAX(o.nb_events_12m, o.nb_events_total) DESC, o.nom COLLATE NOCASE",
        "recent" => "o.last_event IS NULL, o.last_event DESC, o.nom COLLATE NOCASE",
        "name" => "o.nom COLLATE NOCASE",
        _ => "o.fit_score IS NULL, o.fit_score DESC, MAX(o.nb_events_12m, o.nb_events_total) DESC, o.nom COLLATE NOCASE",
    };
    let tail = format!("WHERE 1=1{w} ORDER BY {order} LIMIT {} OFFSET {}", limit.clamp(1, 1000), offset.max(0));
    query_rows(conn, &tail, &a, today)
}

#[derive(Debug, Clone, Serialize)]
pub struct PromoterVenue {
    pub venue_id: i64,
    pub nom: String,
    pub ville: Option<String>,
    pub nb_events: i64,
    pub capacite_est: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PromoterEvent {
    pub date_event: Option<String>,
    pub titre: Option<String>,
    pub venue: String,
    pub artists: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PromoterContact {
    pub id: i64,
    pub valeur: String,
    pub role_devine: Option<String>,
    pub score: i64,
    pub source_url: Option<String>,
    pub source_method: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PromoterFiche {
    pub row: PromoterRow,
    pub instagram: Option<String>,
    pub lead_source: Option<String>,
    pub venues: Vec<PromoterVenue>,
    pub events: Vec<PromoterEvent>,
    pub contacts: Vec<PromoterContact>,
}

pub fn promoter_fiche(conn: &Connection, id: i64, today: chrono::NaiveDate) -> Result<PromoterFiche, String> {
    // Same row as the list (house promoters and archived ones included).
    let row = query_rows(conn, "WHERE o.id = ?1", &[Box::new(id)], today)?
        .pop()
        .ok_or_else(|| "promoter not found".to_string())?;
    let (instagram, lead_source): (Option<String>, Option<String>) = conn
        .query_row("SELECT instagram, lead_source FROM vi_promoter_orgs WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(|e| e.to_string())?;
    let venues = {
        let mut st = conn
            .prepare(
                "SELECT v.id, v.nom, v.ville, p.nb_events, v.capacite_est FROM vi_promoters p
                 JOIN vi_venues v ON v.id = p.venue_id WHERE p.org_id = ?1 ORDER BY p.nb_events DESC, v.nom LIMIT 50",
            )
            .map_err(|e| e.to_string())?;
        let rows = st
            .query_map([id], |r| {
                Ok(PromoterVenue { venue_id: r.get(0)?, nom: r.get(1)?, ville: r.get(2)?, nb_events: r.get(3)?, capacite_est: r.get(4)? })
            })
            .map_err(|e| e.to_string())?;
        rows.flatten().collect()
    };
    let events = {
        let mut st = conn
            .prepare(
                "SELECT e.ra_event_id, e.date_event, e.titre, v.nom FROM vi_event_promoters ep
                 JOIN vi_venue_events e ON e.ra_event_id = ep.ra_event_id
                 JOIN vi_venues v ON v.id = e.venue_id
                 WHERE ep.promoter_id = ?1 ORDER BY e.date_event DESC LIMIT 25",
            )
            .map_err(|e| e.to_string())?;
        let rows: Vec<(i64, Option<String>, Option<String>, String)> = st
            .query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .map_err(|e| e.to_string())?
            .flatten()
            .collect();
        let mut art = conn
            .prepare("SELECT nom FROM vi_event_artists WHERE ra_event_id = ?1 ORDER BY nom LIMIT 8")
            .map_err(|e| e.to_string())?;
        rows.into_iter()
            .map(|(eid, date_event, titre, venue)| PromoterEvent {
                date_event,
                titre,
                venue,
                artists: art.query_map([eid], |r| r.get::<_, String>(0)).map(|it| it.flatten().collect()).unwrap_or_default(),
            })
            .collect()
    };
    let contacts = {
        let mut st = conn
            .prepare(
                "SELECT id, valeur, role_devine, score, source_url, source_method FROM vi_promoter_contacts
                 WHERE promoter_id = ?1 AND type = 'email' ORDER BY score DESC, id",
            )
            .map_err(|e| e.to_string())?;
        let rows = st
            .query_map([id], |r| {
                Ok(PromoterContact {
                    id: r.get(0)?,
                    valeur: r.get(1)?,
                    role_devine: r.get(2)?,
                    score: r.get(3)?,
                    source_url: r.get(4)?,
                    source_method: r.get(5)?,
                })
            })
            .map_err(|e| e.to_string())?;
        rows.flatten().collect()
    };
    Ok(PromoterFiche { row, instagram, lead_source, venues, events, contacts })
}

// ---------------------------------------------------------------------------
// Promote to the pipeline
// ---------------------------------------------------------------------------

/// Best stored address of a promoter.
pub(crate) fn best_promoter_email(conn: &Connection, id: i64) -> Option<String> {
    conn.query_row(
        "SELECT valeur FROM vi_promoter_contacts WHERE promoter_id = ?1 AND type = 'email' ORDER BY score DESC, id LIMIT 1",
        [id],
        |r| r.get(0),
    )
    .ok()
}

/// Promote a promoter into the contacts pipeline (category 'promoter').
/// Idempotent: returns the linked contact when there is one.
pub(crate) fn promote_promoter_inner(conn: &Connection, id: i64) -> Result<i64, String> {
    if let Some(cid) = conn
        .query_row("SELECT crm_contact_id FROM vi_promoter_orgs WHERE id = ?1", [id], |r| r.get::<_, Option<i64>>(0))
        .optional()
        .map_err(|e| e.to_string())?
        .flatten()
    {
        // Still there? (ON DELETE SET NULL covers deletes with FKs on.)
        let alive: bool = conn
            .query_row("SELECT EXISTS(SELECT 1 FROM contacts WHERE id = ?1)", [cid], |r| r.get(0))
            .unwrap_or(false);
        if alive {
            return Ok(cid);
        }
    }
    let (nom, ville, site, ra_url): (String, Option<String>, Option<String>, Option<String>) = conn
        .query_row("SELECT nom, ville, site_web, ra_url FROM vi_promoter_orgs WHERE id = ?1", [id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })
        .map_err(|e| e.to_string())?;
    let venues: Vec<String> = {
        let mut st = conn
            .prepare(
                "SELECT v.nom FROM vi_promoters p JOIN vi_venues v ON v.id = p.venue_id
                 WHERE p.org_id = ?1 ORDER BY p.nb_events DESC LIMIT 5",
            )
            .map_err(|e| e.to_string())?;
        let rows = st.query_map([id], |r| r.get::<_, String>(0)).map_err(|e| e.to_string())?;
        rows.flatten().collect()
    };
    let email = best_promoter_email(conn, id);
    let mut notes = Vec::new();
    if !venues.is_empty() {
        notes.push(format!("Salles : {}", venues.join(", ")));
    }
    if let Some(u) = ra_url {
        notes.push(format!("RA : {u}"));
    }
    conn.execute(
        "INSERT INTO contacts (category, name, venue, area, email, website, notes, contact_channel, status, reason, artist_id)
         VALUES ('promoter', ?1, ?1, ?2, ?3, ?4, ?5, 'email', 'to_contact', 'Venue Intelligence (collectif)', ?6)",
        params![
            nom,
            ville,
            email,
            site,
            (!notes.is_empty()).then(|| notes.join(" · ")),
            crate::commands::artists::default_artist_id(conn)
        ],
    )
    .map_err(|e| e.to_string())?;
    let cid = conn.last_insert_rowid();
    conn.execute(
        "UPDATE vi_promoter_orgs SET crm_contact_id = ?2, updated_at = datetime('now') WHERE id = ?1",
        params![id, cid],
    )
    .map_err(|e| e.to_string())?;
    Ok(cid)
}

// ---------------------------------------------------------------------------
// Enrichment (network: RA promoter page, Serper, the promoter's own site)
// ---------------------------------------------------------------------------

/// Store addresses found on one page for a promoter, through keep_promoter_email
/// and the RGPD exclusions. Returns how many were kept.
pub(crate) fn store_promoter_emails(
    conn: &Connection,
    promoter_id: i64,
    emails: Vec<(String, bool)>,
    url: &str,
    site_host: Option<&str>,
    name: &str,
    city: Option<&str>,
    method: &str,
) -> i64 {
    let mut kept = 0;
    for (email, from_mailto) in emails {
        if super::vi::is_excluded(conn, &email) || !keep_promoter_email(&email, site_host, name, city) {
            continue;
        }
        let (score, role) = score_email(&email, from_mailto);
        if conn
            .execute(
                "INSERT OR IGNORE INTO vi_promoter_contacts (promoter_id, type, valeur, role_devine, score, source_url, source_method)
                 VALUES (?1, 'email', ?2, ?3, ?4, ?5, ?6)",
                params![promoter_id, email, role, score, url, method],
            )
            .map(|n| n > 0)
            .unwrap_or(false)
        {
            kept += 1;
        }
    }
    kept
}

pub async fn process_enrich_task(ra_client: &reqwest::Client, pool: &DbPool, payload: &str) -> Result<(), ra::RaError> {
    let perr = |m: String| ra::RaError { message: m, status: None, retryable: false };
    let v: serde_json::Value = serde_json::from_str(payload).map_err(|e| perr(e.to_string()))?;
    let id = v["promoter_id"].as_i64().unwrap_or(0);
    let (nom, ville, ra_id, stored_site): (String, Option<String>, Option<i64>, Option<String>) = {
        let conn = pool.get().map_err(|e| perr(e.to_string()))?;
        conn.query_row(
            "SELECT nom, ville, ra_promoter_id, site_web FROM vi_promoter_orgs WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .map_err(|e| perr(e.to_string()))?
    };
    let mut site = stored_site.as_deref().and_then(normalize_site).filter(|s| !is_foreign_venue_site(s));
    let mut insta: Option<String> = None;
    let mut searched = false;

    // 1. The website RA lists for the promoter.
    if site.is_none() {
        if let Some(pid) = ra_id {
            if let Ok(Some(w)) = ra::fetch_promoter_website(ra_client, pid).await {
                match normalize_site(&w) {
                    Some(s) if is_foreign_venue_site(&s) => insta = instagram_handle(&s),
                    Some(s) => site = Some(s),
                    None => {}
                }
            }
        }
    }
    // 2. A web search, kept only on a domain matching the name.
    if site.is_none() {
        let key = pool
            .get()
            .ok()
            .and_then(|c| crate::secrets::read_setting(&c, "serper_api_key"))
            .filter(|k| !k.trim().is_empty());
        if let Some(key) = key {
            let q = format!("{} {} collective promoter", nom, ville.clone().unwrap_or_default());
            site = super::enrich::resolve_website_via_search(
                &super::enrich::crawl_client(),
                &key,
                &q,
                &nom,
                ville.as_deref(),
                super::enrich::THIRD_PARTY_HOSTS,
            )
            .await;
            searched = site.is_some();
            tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        }
    }
    if let Ok(conn) = pool.get() {
        let _ = conn.execute(
            "UPDATE vi_promoter_orgs SET site_web = COALESCE(site_web, ?2), instagram = COALESCE(instagram, ?3),
                updated_at = datetime('now') WHERE id = ?1",
            params![id, site, insta],
        );
    }

    // 3. Crawl the site (homepage, contact / legal paths, contact links).
    let client = super::enrich::crawl_client();
    if let Some(raw) = &site {
        let mut base = origin_of(raw);
        let mut home = String::new();
        for cand in base_candidates(raw) {
            if let Some(b) = super::enrich::fetch_html(&client, &cand).await {
                base = cand;
                home = b;
                break;
            }
        }
        let host = host_of(&base);
        let method = if searched { "search" } else { "promoter_site" };
        let mut urls = vec![base.clone()];
        for p in CRAWL_PATHS.iter().filter(|p| !p.is_empty()) {
            urls.push(format!("{base}{p}"));
        }
        for l in internal_links(&home, &base) {
            if !urls.contains(&l) {
                urls.push(l);
            }
        }
        let mut fetched = 0;
        for url in urls {
            if fetched >= 8 {
                break;
            }
            let body = if url == base {
                if home.is_empty() {
                    continue;
                }
                home.clone()
            } else {
                fetched += 1;
                match super::enrich::fetch_html(&client, &url).await {
                    Some(b) => b,
                    None => continue,
                }
            };
            let (emails, _) = extract_contacts(&body);
            let kept = match pool.get() {
                Ok(conn) => store_promoter_emails(&conn, id, emails, &url, Some(&host), &nom, ville.as_deref(), method),
                Err(_) => 0,
            };
            if kept > 0 && url != base {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
    }
    if let Ok(conn) = pool.get() {
        let _ = conn.execute(
            "UPDATE vi_promoter_orgs SET enriched_at = datetime('now'), updated_at = datetime('now') WHERE id = ?1",
            [id],
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[tauri::command(async)]
pub fn vi_list_promoters(
    state: State<'_, AppState>,
    filter: Option<PromoterFilter>,
    sort: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
) -> Result<Vec<PromoterRow>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    list_promoters(
        &conn,
        &filter.unwrap_or_default(),
        sort.as_deref(),
        limit.unwrap_or(100),
        offset.unwrap_or(0),
        chrono::Utc::now().date_naive(),
    )
}

#[tauri::command(async)]
pub fn vi_count_promoters(state: State<'_, AppState>, filter: Option<PromoterFilter>) -> Result<i64, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    count_promoters(&conn, &filter.unwrap_or_default())
}

#[tauri::command(async)]
pub fn vi_promoter_detail(state: State<'_, AppState>, id: i64) -> Result<PromoterFiche, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    promoter_fiche(&conn, id, chrono::Utc::now().date_naive())
}

#[tauri::command(async)]
pub fn vi_promote_promoter(state: State<'_, AppState>, id: i64) -> Result<i64, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    promote_promoter_inner(&conn, id)
}

/// Queue an email search over promoters (`ids`, else every non-house promoter
/// with at least `min_events` nights and no stored address, not enriched yet
/// unless `force`). Error codes the UI translates: "vi.enrich_none".
#[tauri::command]
pub async fn vi_start_promoter_enrich(
    app: AppHandle,
    state: State<'_, AppState>,
    ids: Option<Vec<i64>>,
    force: Option<bool>,
    min_events: Option<i64>,
) -> Result<i64, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let ids: Vec<i64> = match ids.filter(|v| !v.is_empty()) {
        Some(v) => v,
        None => {
            let mut sql = format!(
                "SELECT o.id FROM vi_promoter_orgs o WHERE o.statut = 'actif' AND o.house_promoter = 0
                   AND MAX(o.nb_events_12m, o.nb_events_total) >= {}
                   AND NOT EXISTS(SELECT 1 FROM vi_promoter_contacts c WHERE c.promoter_id = o.id AND c.type = 'email')",
                min_events.unwrap_or(3).max(1)
            );
            if !force.unwrap_or(false) {
                sql.push_str(" AND o.enriched_at IS NULL");
            }
            sql.push_str(" ORDER BY o.fit_score IS NULL, o.fit_score DESC, o.nb_events_total DESC LIMIT 2000");
            let mut st = conn.prepare(&sql).map_err(|e| e.to_string())?;
            let rows = st.query_map([], |r| r.get::<_, i64>(0)).map_err(|e| e.to_string())?;
            rows.flatten().collect()
        }
    };
    if ids.is_empty() {
        return Err("vi.enrich_none".into());
    }
    conn.execute(
        "INSERT INTO vi_runs (type, params, statut, started_at) VALUES ('enrich_promoters', ?1, 'en_cours', datetime('now'))",
        [json!({ "promoters": ids.len() }).to_string()],
    )
    .map_err(|e| e.to_string())?;
    let run_id = conn.last_insert_rowid();
    {
        let mut st = conn
            .prepare("INSERT INTO vi_tasks (run_id, type, payload, statut) VALUES (?1, 'enrich_promoter', ?2, 'en_attente')")
            .map_err(|e| e.to_string())?;
        for id in &ids {
            st.execute(params![run_id, json!({ "promoter_id": id }).to_string()]).map_err(|e| e.to_string())?;
        }
    }
    crate::commands::harvest::spawn_worker(state.pool.clone(), state.cancels.clone(), app, run_id);
    Ok(run_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        crate::db::migrate_for_tests(&c).unwrap();
        c
    }

    #[test]
    fn org_keys() {
        assert_eq!(org_key("Collectif X", Some(12)).as_deref(), Some("ra:12"));
        assert_eq!(org_key("Collectif X", None).as_deref(), Some("n:collectifx"));
        assert_eq!(org_key(" !! ", None), None);
    }

    #[test]
    fn promoter_email_rules() {
        let site = Some("collectifx.fr");
        // Own site domain.
        assert!(keep_promoter_email("booking@collectifx.fr", site, "Collectif X", Some("Paris")));
        // Name-matching domain without a site.
        assert!(keep_promoter_email("hello@pardonmyfrench.paris", None, "Pardon My French", Some("Paris")));
        // Free mailbox carrying the name.
        assert!(keep_promoter_email("pardonmyfrench.booking@gmail.com", None, "Pardon My French", None));
        assert!(!keep_promoter_email("jean.dupont@gmail.com", None, "Pardon My French", None));
        // City word alone proves nothing.
        assert!(!keep_promoter_email("paris.events@gmail.com", None, "Paris Underground", Some("Paris")));
        // Third party on the promoter's page (agency, ticketing).
        assert!(!keep_promoter_email("info@ticketmaster.fr", site, "Collectif X", None));
        assert!(!keep_promoter_email("contact@someagency.com", site, "Collectif X", None));
        // Placeholder / non-pitch.
        assert!(!keep_promoter_email("noreply@collectifx.fr", site, "Collectif X", None));
    }

    #[test]
    fn harvest_links_events_lineups_and_promoters() {
        let c = conn();
        c.execute_batch(
            "INSERT INTO vi_venues (id, nom, nom_normalise, ville, ville_norm, pays, statut, ra_venue_id)
               VALUES (1, 'Club A', 'cluba', 'Paris', 'paris', 'FR', 'qualifie', 77),
                      (2, 'Club B', 'clubb', 'Lyon', 'lyon', 'FR', 'qualifie', 78);
             INSERT INTO vi_venue_events (ra_event_id, venue_id, date_event) VALUES (1001, 1, '2026-09-01');",
        )
        .unwrap();
        let ev: ra::EventNode = serde_json::from_value(json!({
            "id": "1001", "title": "House Sessions",
            "artists": [{ "id": "1", "name": "Prunk" }, { "id": "2", "name": "Piers" }],
            "promoters": [{ "id": "5", "name": "Collectif X" }, { "name": "Club A" }]
        }))
        .unwrap();
        c.execute("INSERT INTO vi_promoters (venue_id, nom, nb_events, ra_url) VALUES (1, 'Collectif X', 4, 'https://ra.co/promoters/5')", [])
            .unwrap();
        c.execute("INSERT INTO vi_promoters (venue_id, nom, nb_events) VALUES (1, 'Club A', 9)", []).unwrap();
        c.execute("INSERT INTO vi_promoters (venue_id, nom, nb_events, ra_url) VALUES (2, 'Collectif X', 1, 'https://ra.co/promoters/5')", [])
            .unwrap();
        record_event_extras(&c, &ev, 1);
        record_event_extras(&c, &ev, 1); // idempotent
        let n: i64 = c.query_row("SELECT COUNT(*) FROM vi_event_artists WHERE ra_event_id = 1001", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 2);
        let t: String = c.query_row("SELECT titre FROM vi_venue_events WHERE ra_event_id = 1001", [], |r| r.get(0)).unwrap();
        assert_eq!(t, "House Sessions");
        // The Lyon row of the same RA promoter gets the same identity on sync.
        assert_eq!(sync_orgs(&c), 1);
        assert_eq!(sync_orgs(&c), 0);
        refresh_aggregates(&c);
        let orgs: i64 = c.query_row("SELECT COUNT(*) FROM vi_promoter_orgs", [], |r| r.get(0)).unwrap();
        assert_eq!(orgs, 2);
        let (total, venues, ville, house, last): (i64, i64, String, i64, String) = c
            .query_row(
                "SELECT nb_events_total, nb_venues, ville, house_promoter, last_event FROM vi_promoter_orgs WHERE cle = 'ra:5'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();
        assert_eq!((total, venues, ville.as_str(), house, last.as_str()), (5, 2, "Paris", 0, "2026-09-01"));
        let house: i64 = c.query_row("SELECT house_promoter FROM vi_promoter_orgs WHERE cle = 'n:cluba'", [], |r| r.get(0)).unwrap();
        assert_eq!(house, 1);

        // List: house promoters hidden by default; email + fit filters.
        let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 30).unwrap();
        let f = PromoterFilter::default();
        let rows = list_promoters(&c, &f, None, 50, 0, today).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].nom, "Collectif X");
        assert_eq!(rows[0].venues, vec!["Club A", "Club B"]);
        assert_eq!(count_promoters(&c, &PromoterFilter { include_house: Some(true), ..Default::default() }).unwrap(), 2);
        assert_eq!(count_promoters(&c, &PromoterFilter { has_email: Some(true), ..Default::default() }).unwrap(), 0);
        let pid = rows[0].id;
        let kept = store_promoter_emails(
            &c,
            pid,
            vec![("booking@collectifx.fr".into(), true), ("info@ticketmaster.fr".into(), false)],
            "https://collectifx.fr/contact",
            Some("collectifx.fr"),
            "Collectif X",
            Some("Paris"),
            "promoter_site",
        );
        assert_eq!(kept, 1);
        assert_eq!(count_promoters(&c, &PromoterFilter { has_email: Some(true), ..Default::default() }).unwrap(), 1);

        // Fiche, then promotion (idempotent) as a 'promoter' contact.
        let fiche = promoter_fiche(&c, pid, today).unwrap();
        assert_eq!(fiche.venues.len(), 2);
        assert_eq!(fiche.events.len(), 1);
        assert_eq!(fiche.events[0].artists, vec!["Piers", "Prunk"]);
        assert_eq!(fiche.contacts.len(), 1);
        assert_eq!(fiche.row.timing.state, timing::State::Unknown);
        let cid = promote_promoter_inner(&c, pid).unwrap();
        assert_eq!(promote_promoter_inner(&c, pid).unwrap(), cid);
        let (cat, email, notes): (String, String, String) = c
            .query_row("SELECT category, email, notes FROM contacts WHERE id = ?1", [cid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap();
        assert_eq!(cat, "promoter");
        assert_eq!(email, "booking@collectifx.fr");
        assert!(notes.starts_with("Salles : Club A, Club B"));
        assert!(!notes.contains('\u{2014}'));
    }
}
