use crate::models::{VisaCountry, VisaDossier};
use crate::AppState;
use rusqlite::params;
use tauri::State;

// ---------- Country knowledge base ----------

#[tauri::command]
pub fn list_visa_countries(state: State<AppState>) -> Result<Vec<VisaCountry>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT * FROM visa_countries ORDER BY name COLLATE NOCASE")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], VisaCountry::from_row)
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

#[tauri::command]
pub fn save_visa_country(state: State<AppState>, country: VisaCountry) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO visa_countries (code, name, work_rules, visa_types, processing_time, required_docs, notes, official_link, updated_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,datetime('now'))
         ON CONFLICT(code) DO UPDATE SET
            name=excluded.name, work_rules=excluded.work_rules, visa_types=excluded.visa_types,
            processing_time=excluded.processing_time, required_docs=excluded.required_docs,
            notes=excluded.notes, official_link=excluded.official_link, updated_at=datetime('now')",
        params![
            country.code, country.name, country.work_rules, country.visa_types,
            country.processing_time, country.required_docs, country.notes, country.official_link
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn delete_visa_country(state: State<AppState>, code: String) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM visa_countries WHERE code=?1", params![code])
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ---------- Dossiers ----------

#[tauri::command]
pub fn list_dossiers(state: State<AppState>) -> Result<Vec<VisaDossier>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT * FROM visa_dossiers ORDER BY updated_at DESC, id DESC")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], VisaDossier::from_row)
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

#[tauri::command]
pub fn save_dossier(state: State<AppState>, dossier: VisaDossier) -> Result<i64, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    match dossier.id {
        Some(id) => {
            conn.execute(
                "UPDATE visa_dossiers SET
                    artist_id=?2, country_code=?3, country_name=?4, title=?5, purpose=?6,
                    event_date=?7, entry_date=?8, status=?9, checklist=?10, notes=?11,
                    updated_at=datetime('now')
                 WHERE id=?1",
                params![
                    id, dossier.artist_id, dossier.country_code, dossier.country_name,
                    dossier.title, dossier.purpose, dossier.event_date, dossier.entry_date,
                    dossier.status, dossier.checklist, dossier.notes
                ],
            )
            .map_err(|e| e.to_string())?;
            Ok(id)
        }
        None => {
            conn.execute(
                "INSERT INTO visa_dossiers
                    (artist_id, country_code, country_name, title, purpose, event_date, entry_date, status, checklist, notes)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                params![
                    dossier.artist_id, dossier.country_code, dossier.country_name, dossier.title,
                    dossier.purpose, dossier.event_date, dossier.entry_date, dossier.status,
                    dossier.checklist, dossier.notes
                ],
            )
            .map_err(|e| e.to_string())?;
            Ok(conn.last_insert_rowid())
        }
    }
}

#[tauri::command]
pub fn delete_dossier(state: State<AppState>, id: i64) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM visa_dossiers WHERE id=?1", params![id])
        .map_err(|e| e.to_string())?;
    Ok(())
}
