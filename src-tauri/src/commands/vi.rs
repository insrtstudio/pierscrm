//! Venue Intelligence commands.
//!
//! J1 scope: read/verify the seeded reference data and let the reference-artist
//! list and RA areas be edited (the artist list is the main precision lever, and
//! RA area ids sometimes need manual entry). Harvest / qualification / enrichment
//! land in later milestones.

use crate::db::normalise;
use crate::models::{ViArea, ViOverview, ViReferenceArtist};
use crate::AppState;
use rusqlite::params;
use tauri::State;

// ---------------- Reference artists ----------------

#[tauri::command(async)]
pub fn vi_list_reference_artists(state: State<'_, AppState>) -> Result<Vec<ViReferenceArtist>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT id, nom, nom_normalise, tier, genres, actif
             FROM vi_reference_artists ORDER BY tier, nom COLLATE NOCASE",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], ViReferenceArtist::from_row)
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

#[tauri::command(async)]
pub fn vi_save_reference_artist(
    state: State<'_, AppState>,
    artist: ViReferenceArtist,
) -> Result<i64, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let norm = normalise(&artist.nom);
    if norm.is_empty() {
        return Err("vi.artist_name_empty".into());
    }
    match artist.id {
        Some(id) => {
            conn.execute(
                "UPDATE vi_reference_artists SET nom=?2, nom_normalise=?3, tier=?4, genres=?5, actif=?6 WHERE id=?1",
                params![id, artist.nom, norm, artist.tier, artist.genres, artist.actif as i64],
            )
            .map_err(|e| e.to_string())?;
            Ok(id)
        }
        None => {
            conn.execute(
                "INSERT INTO vi_reference_artists (nom, nom_normalise, tier, genres, actif)
                 VALUES (?1,?2,?3,?4,?5)
                 ON CONFLICT(nom_normalise) DO UPDATE SET nom=excluded.nom, tier=excluded.tier,
                    genres=excluded.genres, actif=excluded.actif",
                params![artist.nom, norm, artist.tier, artist.genres, artist.actif as i64],
            )
            .map_err(|e| e.to_string())?;
            let id: i64 = conn
                .query_row(
                    "SELECT id FROM vi_reference_artists WHERE nom_normalise=?1",
                    params![norm],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            Ok(id)
        }
    }
}

#[tauri::command(async)]
pub fn vi_delete_reference_artist(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM vi_reference_artists WHERE id=?1", params![id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ---------------- RA areas ----------------

#[tauri::command(async)]
pub fn vi_list_areas(state: State<'_, AppState>) -> Result<Vec<ViArea>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT id, slug, ra_area_id, libelle, pays, actif, resolved_at
             FROM vi_ra_areas ORDER BY pays, slug",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt.query_map([], ViArea::from_row).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

#[tauri::command(async)]
pub fn vi_save_area(state: State<'_, AppState>, area: ViArea) -> Result<i64, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    // Manual entry of a resolved ra_area_id stamps resolved_at.
    let resolved = if area.ra_area_id.is_some() {
        "datetime('now')"
    } else {
        "resolved_at"
    };
    match area.id {
        Some(id) => {
            let sql = format!(
                "UPDATE vi_ra_areas SET slug=?2, ra_area_id=?3, libelle=?4, pays=?5, actif=?6, resolved_at={} WHERE id=?1",
                resolved
            );
            conn.execute(
                &sql,
                params![id, area.slug, area.ra_area_id, area.libelle, area.pays, area.actif as i64],
            )
            .map_err(|e| e.to_string())?;
            Ok(id)
        }
        None => {
            conn.execute(
                "INSERT INTO vi_ra_areas (slug, ra_area_id, libelle, pays, actif) VALUES (?1,?2,?3,?4,?5)
                 ON CONFLICT(slug) DO UPDATE SET ra_area_id=excluded.ra_area_id, libelle=excluded.libelle,
                    pays=excluded.pays, actif=excluded.actif",
                params![area.slug, area.ra_area_id, area.libelle, area.pays, area.actif as i64],
            )
            .map_err(|e| e.to_string())?;
            Ok(conn.last_insert_rowid())
        }
    }
}

// ---------------- Overview (J1 verification) ----------------

#[tauri::command(async)]
pub fn vi_overview(state: State<'_, AppState>) -> Result<ViOverview, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let one = |sql: &str| -> i64 { conn.query_row(sql, [], |r| r.get(0)).unwrap_or(0) };
    Ok(ViOverview {
        venues_total: one("SELECT COUNT(*) FROM vi_venues"),
        venues_qualifie: one("SELECT COUNT(*) FROM vi_venues WHERE statut='qualifie'"),
        venues_valide: one("SELECT COUNT(*) FROM vi_venues WHERE statut='valide'"),
        evidence_total: one("SELECT COUNT(*) FROM vi_evidence"),
        contacts_total: one("SELECT COUNT(*) FROM vi_contacts"),
        promoters_total: one("SELECT COUNT(*) FROM vi_promoters"),
        reference_artists_total: one("SELECT COUNT(*) FROM vi_reference_artists"),
        reference_artists_actifs: one("SELECT COUNT(*) FROM vi_reference_artists WHERE actif=1"),
        areas_total: one("SELECT COUNT(*) FROM vi_ra_areas"),
        areas_resolues: one("SELECT COUNT(*) FROM vi_ra_areas WHERE ra_area_id IS NOT NULL"),
        runs_total: one("SELECT COUNT(*) FROM vi_runs"),
    })
}

// ---------------- Exclusions (vi_exclusions) ----------------
//
// The table existed from J0 ("RGPD: contacts explicitly erased land here so a
// later run never re-adds them") but nothing wrote or read it. Two kinds of rows
// now: an email address (lowercased) that enrichment must never store again, and
// a venue, keyed `ra:<ra_venue_id>`, that harvest skips and the list archives.

/// Exclusion key of a venue (stable across re-harvests, unlike our own row id).
pub(crate) fn venue_exclusion_key(ra_venue_id: i64) -> String {
    format!("ra:{}", ra_venue_id)
}

/// Whether `valeur` (an email or a venue key) is excluded.
pub(crate) fn is_excluded(conn: &rusqlite::Connection, valeur: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM vi_exclusions WHERE valeur_normalisee=?1)",
        params![valeur.trim().to_lowercase()],
        |r| r.get::<_, bool>(0),
    )
    .unwrap_or(false)
}

/// Exclude a venue: it is archived (out of the list, out of qualification) and
/// later harvests skip it. Contacts already in the pipeline are left alone.
#[tauri::command(async)]
pub fn vi_exclude_venue(state: State<'_, AppState>, id: i64, raison: Option<String>) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let ra_id: Option<i64> = conn
        .query_row("SELECT ra_venue_id FROM vi_venues WHERE id=?1", params![id], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if let Some(ra_id) = ra_id {
        conn.execute(
            "INSERT OR IGNORE INTO vi_exclusions (valeur_normalisee, type, raison) VALUES (?1,'venue',?2)",
            params![venue_exclusion_key(ra_id), raison],
        )
        .map_err(|e| e.to_string())?;
    }
    conn.execute(
        "UPDATE vi_venues SET statut='archive', updated_at=datetime('now') WHERE id=?1",
        params![id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Undo vi_exclude_venue: back to 'candidat', re-qualified on the next run.
#[tauri::command(async)]
pub fn vi_restore_venue(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    if let Ok(Some(ra_id)) =
        conn.query_row("SELECT ra_venue_id FROM vi_venues WHERE id=?1", params![id], |r| r.get::<_, Option<i64>>(0))
    {
        conn.execute(
            "DELETE FROM vi_exclusions WHERE valeur_normalisee=?1",
            params![venue_exclusion_key(ra_id)],
        )
        .map_err(|e| e.to_string())?;
    }
    conn.execute(
        "UPDATE vi_venues SET statut = CASE WHEN crm_contact_id IS NOT NULL THEN 'valide' ELSE 'candidat' END,
            updated_at=datetime('now') WHERE id=?1 AND statut='archive'",
        params![id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Erase an address everywhere Venue Intelligence and Radar hold it and make
/// sure no later run stores it again. Contacts in the pipeline are untouched
/// (that is the contact's own page to decide).
#[tauri::command(async)]
pub fn vi_exclude_email(state: State<'_, AppState>, email: String, raison: Option<String>) -> Result<i64, String> {
    let e = email.trim().to_lowercase();
    if !e.contains('@') {
        return Err("vi.invalid_email".into());
    }
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT OR IGNORE INTO vi_exclusions (valeur_normalisee, type, raison) VALUES (?1,'email',?2)",
        params![e, raison],
    )
    .map_err(|e| e.to_string())?;
    let n = conn
        .execute("DELETE FROM vi_contacts WHERE type='email' AND lower(valeur)=?1", params![e])
        .map_err(|e| e.to_string())?;
    let m = conn
        .execute("DELETE FROM radar_contacts WHERE type='email' AND lower(valeur)=?1", params![e])
        .map_err(|e| e.to_string())?;
    Ok((n + m) as i64)
}

// ---------------- Data-quality migrations (audit 2026-10-29) ----------------

/// Venue Intelligence / Radar schema and data fixes, called once per launch from
/// db::migrate. Every step is idempotent; the one-shot data rewrites are guarded
/// by a settings flag. Best-effort: a failure here never blocks startup.
pub(crate) fn migrate_quality(conn: &rusqlite::Connection) {
    for stmt in [
        // #18: one row per RA event, so a venue's event count is a COUNT, not
        // an increment that every overlapping re-harvest inflated.
        "CREATE TABLE IF NOT EXISTS vi_venue_events (
            ra_event_id INTEGER PRIMARY KEY,
            venue_id    INTEGER NOT NULL REFERENCES vi_venues(id) ON DELETE CASCADE,
            date_event  TEXT
         )",
        "CREATE INDEX IF NOT EXISTS idx_vi_venue_events_venue ON vi_venue_events(venue_id)",
        // Floor kept from the counts harvested before vi_venue_events existed.
        "ALTER TABLE vi_venues ADD COLUMN nb_events_legacy INTEGER NOT NULL DEFAULT 0",
        // #14: accent-folded city, SQLite having no unaccent().
        "ALTER TABLE vi_venues ADD COLUMN ville_norm TEXT",
        // #1: the write-time "already on another venue" check looks addresses up by value.
        "CREATE INDEX IF NOT EXISTS idx_vi_contacts_val ON vi_contacts(valeur, venue_id)",
        // #12: YouTube channels were stored as kind 'playlist'.
        "UPDATE radar_curators SET kind='channel' WHERE source='youtube' AND kind != 'channel'",
    ] {
        let _ = conn.execute(stmt, []);
    }

    fill_ville_norm(conn);
    fill_instagram_from_site(conn);
    once(conn, "vi_events_dedup_v1", dedup_legacy_event_counts);
    once(conn, "vi_artefact_cleanup_v1", |c| {
        clean_artefact_emails(c, "vi_contacts");
        clean_artefact_emails(c, "radar_contacts");
    });
    once(conn, "vi_phone_cleanup_v1", clean_artefact_phones);
}

/// One-shot: phones scraped after a "tel:" inside a script came back as JS
/// fragments or with a trailing backslash. Clean what is a number, drop the rest.
fn clean_artefact_phones(conn: &rusqlite::Connection) {
    let rows: Vec<(i64, String)> = match conn.prepare("SELECT id, valeur FROM vi_contacts WHERE type='telephone'") {
        Ok(mut st) => st
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .map(|it| it.flatten().collect())
            .unwrap_or_default(),
        Err(_) => return,
    };
    let Ok(tx) = conn.unchecked_transaction() else { return };
    for (id, v) in rows {
        match super::enrich::clean_phone(&v) {
            Some(p) if p == v => {}
            Some(p) => {
                if tx.execute("UPDATE vi_contacts SET valeur=?2 WHERE id=?1", params![id, p]).is_err() {
                    let _ = tx.execute("DELETE FROM vi_contacts WHERE id=?1", params![id]);
                }
            }
            None => {
                let _ = tx.execute("DELETE FROM vi_contacts WHERE id=?1", params![id]);
            }
        }
    }
    let _ = tx.commit();
}

fn once(conn: &rusqlite::Connection, flag: &str, f: impl FnOnce(&rusqlite::Connection)) {
    let done: bool = conn
        .query_row("SELECT EXISTS(SELECT 1 FROM settings WHERE key=?1)", params![flag], |r| r.get(0))
        .unwrap_or(true);
    if done {
        return;
    }
    f(conn);
    let _ = conn.execute(
        "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, datetime('now'))",
        params![flag],
    );
}

/// Fill vi_venues.ville_norm wherever a city is known and the fold is missing.
fn fill_ville_norm(conn: &rusqlite::Connection) {
    let rows: Vec<(i64, String)> = match conn
        .prepare("SELECT id, ville FROM vi_venues WHERE ville IS NOT NULL AND ville_norm IS NULL")
    {
        Ok(mut st) => st
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .map(|it| it.flatten().collect())
            .unwrap_or_default(),
        Err(_) => return,
    };
    if rows.is_empty() {
        return;
    }
    let Ok(tx) = conn.unchecked_transaction() else { return };
    for (id, ville) in rows {
        let _ = tx.execute("UPDATE vi_venues SET ville_norm=?2 WHERE id=?1", params![id, normalise(&ville)]);
    }
    let _ = tx.commit();
}

/// An Instagram profile stored as the venue's website becomes its Instagram
/// handle, which the email retry can then read.
fn fill_instagram_from_site(conn: &rusqlite::Connection) {
    let rows: Vec<(i64, String)> = match conn.prepare(
        "SELECT id, site_web FROM vi_venues WHERE instagram IS NULL AND site_web LIKE '%instagram.com/%'",
    ) {
        Ok(mut st) => st
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .map(|it| it.flatten().collect())
            .unwrap_or_default(),
        Err(_) => return,
    };
    for (id, site) in rows {
        if let Some(h) = super::enrich::instagram_handle(&site) {
            let _ = conn.execute("UPDATE vi_venues SET instagram=?2 WHERE id=?1", params![id, h]);
        }
    }
}

/// How many times, on average, each day of an area was harvested, from the
/// finished harvest tasks (their payload holds the area and the window).
pub(crate) fn area_harvest_factors(payloads: &[String]) -> std::collections::HashMap<String, f64> {
    use chrono::NaiveDate;
    use std::collections::HashMap;
    let mut per_area: HashMap<String, HashMap<NaiveDate, u32>> = HashMap::new();
    for p in payloads {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(p) else { continue };
        let area = v["area_name"].as_str().unwrap_or("").to_string();
        let d = |k: &str| v[k].as_str().and_then(|s| NaiveDate::parse_from_str(s.get(0..10)?, "%Y-%m-%d").ok());
        let (Some(mut cur), Some(end)) = (d("gte"), d("lte")) else { continue };
        if area.is_empty() || end < cur {
            continue;
        }
        let days = per_area.entry(area).or_default();
        while cur <= end {
            *days.entry(cur).or_default() += 1;
            cur = cur.succ_opt().unwrap_or(end + chrono::Duration::days(1));
        }
    }
    per_area
        .into_iter()
        .filter(|(_, days)| !days.is_empty())
        .map(|(a, days)| {
            let total: u32 = days.values().sum();
            (a, total as f64 / days.len() as f64)
        })
        .collect()
}

/// One-shot: deflate the event counts harvested before vi_venue_events existed.
/// Every listing used to add 1, so an area harvested 4 times over the same dates
/// counted each event 4 times. The exact events are not stored, so each area's
/// counts are divided by that area's average harvest overlap; the result is
/// kept as nb_events_legacy, the floor new distinct counts build on.
fn dedup_legacy_event_counts(conn: &rusqlite::Connection) {
    let payloads: Vec<String> = match conn.prepare(
        "SELECT payload FROM vi_tasks WHERE type='harvest_area_window' AND statut='termine' AND payload IS NOT NULL",
    ) {
        Ok(mut st) => st
            .query_map([], |r| r.get(0))
            .map(|it| it.flatten().collect())
            .unwrap_or_default(),
        Err(_) => return,
    };
    let factors = area_harvest_factors(&payloads);
    let Ok(tx) = conn.unchecked_transaction() else { return };
    for (area, f) in &factors {
        if *f <= 1.01 {
            continue;
        }
        let _ = tx.execute(
            "UPDATE vi_venues SET nb_events_periode = MAX(1, CAST(ROUND(nb_events_periode / ?2) AS INTEGER))
             WHERE region_cible = ?1 AND nb_events_periode > 0",
            params![area, f],
        );
        let _ = tx.execute(
            "UPDATE vi_promoters SET nb_events = MAX(1, CAST(ROUND(nb_events / ?2) AS INTEGER))
             WHERE nb_events > 0 AND venue_id IN (SELECT id FROM vi_venues WHERE region_cible = ?1)",
            params![area, f],
        );
    }
    let _ = tx.execute("UPDATE vi_venues SET nb_events_legacy = nb_events_periode", []);
    let _ = tx.commit();
}

/// One-shot: rewrite scraping artefacts in stored addresses (`a@b.com\`,
/// `u003ea@b.com`) to the clean address, and drop template addresses
/// (`your@email.com`). Addresses shared across venues/curators are not touched
/// here: the read side filters them, and a real promoter inbox can be shared.
fn clean_artefact_emails(conn: &rusqlite::Connection, table: &str) {
    use super::enrich::{clean_email, is_placeholder_email};
    let owner = if table == "vi_contacts" { "venue_id" } else { "curator_id" };
    let rows: Vec<(i64, i64, String)> = match conn.prepare(&format!(
        "SELECT id, {owner}, valeur FROM {table} WHERE type='email'"
    )) {
        Ok(mut st) => st
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .map(|it| it.flatten().collect())
            .unwrap_or_default(),
        Err(_) => return,
    };
    let Ok(tx) = conn.unchecked_transaction() else { return };
    let copy_cols = if table == "vi_contacts" {
        "type, role_devine, score, personne_nom, personne_role, source_url, source_method, confiance, verifie"
    } else {
        "type, role_devine, score, source_url, source_method, verifie"
    };
    for (id, owner_id, valeur) in rows {
        let cleaned = clean_email(&valeur);
        let keep_as_is = cleaned.as_deref() == Some(valeur.as_str()) && !is_placeholder_email(&valeur);
        if keep_as_is {
            continue;
        }
        match cleaned.filter(|c| !is_placeholder_email(c)) {
            Some(c) => {
                // The clean twin may already exist for this owner: then the
                // artefact row simply goes.
                let _ = tx.execute(
                    &format!(
                        "INSERT OR IGNORE INTO {table} ({owner}, valeur, {copy_cols})
                         SELECT {owner}, ?2, {copy_cols} FROM {table} WHERE id=?1"
                    ),
                    params![id, c],
                );
                let _ = owner_id;
                let _ = tx.execute(&format!("DELETE FROM {table} WHERE id=?1"), params![id]);
            }
            None => {
                let _ = tx.execute(&format!("DELETE FROM {table} WHERE id=?1"), params![id]);
            }
        }
    }
    let _ = tx.commit();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harvest_factor_counts_overlap() {
        let p = |a: &str, g: &str, l: &str| {
            serde_json::json!({ "area_name": a, "gte": format!("{g}T00:00:00.000Z"), "lte": format!("{l}T23:59:59.999Z") })
                .to_string()
        };
        let f = area_harvest_factors(&[
            p("Paris", "2025-01-01", "2025-01-10"),
            p("Paris", "2025-01-01", "2025-01-10"),
            p("Paris", "2025-01-06", "2025-01-15"),
            p("Berlin", "2025-01-01", "2025-01-10"),
        ]);
        // Paris: 30 day-harvests over 15 distinct days.
        assert!((f["Paris"] - 2.0).abs() < 1e-9);
        assert!((f["Berlin"] - 1.0).abs() < 1e-9);
    }

    /// Runs the full startup migration on a copy of a real database, never on
    /// the live one: VI_MIGRATION_DB=/path/to/copy.db cargo test migrate_on_copy -- --ignored
    #[test]
    #[ignore]
    fn migrate_on_copy() {
        let path = std::env::var("VI_MIGRATION_DB").expect("VI_MIGRATION_DB");
        assert!(!path.contains("Application Support"), "never on the live database");
        let t = std::time::Instant::now();
        let pool = crate::db::init_pool(std::path::Path::new(&path)).expect("migrate");
        println!("init_pool + migrations: {:?}", t.elapsed());
        let _ = pool.get().unwrap();
    }

    #[test]
    fn venue_key_is_stable() {
        assert_eq!(venue_exclusion_key(42), "ra:42");
    }
}
