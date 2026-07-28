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

#[tauri::command]
pub fn vi_list_reference_artists(state: State<AppState>) -> Result<Vec<ViReferenceArtist>, String> {
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

#[tauri::command]
pub fn vi_save_reference_artist(
    state: State<AppState>,
    artist: ViReferenceArtist,
) -> Result<i64, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let norm = normalise(&artist.nom);
    if norm.is_empty() {
        return Err("Nom d'artiste vide.".into());
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

#[tauri::command]
pub fn vi_delete_reference_artist(state: State<AppState>, id: i64) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM vi_reference_artists WHERE id=?1", params![id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ---------------- RA areas ----------------

#[tauri::command]
pub fn vi_list_areas(state: State<AppState>) -> Result<Vec<ViArea>, String> {
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

#[tauri::command]
pub fn vi_save_area(state: State<AppState>, area: ViArea) -> Result<i64, String> {
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

#[tauri::command]
pub fn vi_overview(state: State<AppState>) -> Result<ViOverview, String> {
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
