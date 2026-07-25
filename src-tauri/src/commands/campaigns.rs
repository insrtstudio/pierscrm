use crate::models::Campaign;
use crate::AppState;
use rusqlite::params;
use tauri::State;

#[tauri::command]
pub fn list_campaigns(state: State<AppState>) -> Result<Vec<Campaign>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT c.id, c.name, c.purpose, c.event_name, c.artist_id, c.target_date, c.status, c.color,
                    c.created_at, c.updated_at,
                    COALESCE((SELECT COUNT(*) FROM emails e WHERE e.campaign_id = c.id AND e.status='sent'), 0) AS sent_count,
                    COALESCE((SELECT COUNT(*) FROM emails e WHERE e.campaign_id = c.id AND e.opened_at IS NOT NULL), 0) AS opened_count
             FROM campaigns c
             ORDER BY (c.target_date IS NULL), c.target_date, c.id DESC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], Campaign::from_row)
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

#[tauri::command]
pub fn save_campaign(state: State<AppState>, campaign: Campaign) -> Result<i64, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    match campaign.id {
        Some(id) => {
            conn.execute(
                "UPDATE campaigns SET name=?2, purpose=?3, event_name=?4, artist_id=?5, target_date=?6, status=?7, color=?8, updated_at=datetime('now') WHERE id=?1",
                params![id, campaign.name, campaign.purpose, campaign.event_name, campaign.artist_id, campaign.target_date, campaign.status, campaign.color],
            )
            .map_err(|e| e.to_string())?;
            Ok(id)
        }
        None => {
            conn.execute(
                "INSERT INTO campaigns (name, purpose, event_name, artist_id, target_date, status, color) VALUES (?1,?2,?3,?4,?5,?6,?7)",
                params![campaign.name, campaign.purpose, campaign.event_name, campaign.artist_id, campaign.target_date, campaign.status, campaign.color],
            )
            .map_err(|e| e.to_string())?;
            Ok(conn.last_insert_rowid())
        }
    }
}

#[tauri::command]
pub fn delete_campaign(state: State<AppState>, id: i64) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM campaigns WHERE id=?1", params![id])
        .map_err(|e| e.to_string())?;
    Ok(())
}
