use crate::models::Event;
use crate::AppState;
use rusqlite::params;
use tauri::State;

use super::status;

#[tauri::command(async)]
pub fn list_events(
    state: State<'_, AppState>,
    from: Option<String>,
    to: Option<String>,
    artist_id: Option<i64>,
) -> Result<Vec<Event>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let mut sql = String::from("SELECT * FROM events WHERE 1=1");
    let mut args: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    if let Some(f) = from {
        sql.push_str(" AND date >= ?");
        args.push(Box::new(f));
    }
    if let Some(t) = to {
        sql.push_str(" AND date <= ?");
        args.push(Box::new(t));
    }
    if let Some(a) = artist_id {
        sql.push_str(" AND artist_id = ?");
        args.push(Box::new(a));
    }
    sql.push_str(" ORDER BY date, start_time");
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let refs: Vec<&dyn rusqlite::types::ToSql> = args.iter().map(|b| b.as_ref()).collect();
    let rows = stmt
        .query_map(refs.as_slice(), Event::from_row)
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

#[tauri::command(async)]
pub fn save_event(state: State<'_, AppState>, event: Event) -> Result<i64, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    // What the event was before this save, to undo a confirmation that no
    // longer holds (status changed, or the gig moved to another contact).
    let before: Option<(Option<i64>, String)> = event.id.and_then(|id| {
        conn.query_row(
            "SELECT contact_id, status FROM events WHERE id = ?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok()
    });
    let saved = save_event_row(&conn, &event)?;
    // Pipeline transitions (see status.rs): a confirmed gig closes the booking;
    // a confirmation withdrawn reopens it unless another gig is still confirmed.
    if let (Some(cid), "confirmed") = (event.contact_id, event.status.as_str()) {
        status::on_event_confirmed(&conn, cid);
    }
    if let Some((Some(prev_cid), prev_status)) = before {
        if prev_status == "confirmed"
            && (event.status != "confirmed" || event.contact_id != Some(prev_cid))
        {
            status::on_event_cancelled(&conn, prev_cid);
        }
    }
    crate::commands::insrt_gigs::after_event_change(&state.pool, saved); // insrt dates (audit #19)
    Ok(saved)
}

/// Insert or update an event row and realign its budget line (also used by
/// commands/offers, which applies its own pipeline transitions).
pub(crate) fn save_event_row(conn: &rusqlite::Connection, event: &Event) -> Result<i64, String> {
    match event.id {
        Some(id) => {
            conn.execute(
                "UPDATE events SET artist_id=?2, contact_id=?3, title=?4, venue=?5, city=?6, date=?7,
                    start_time=?8, end_time=?9, status=?10, fee=?11, notes=?12, updated_at=datetime('now')
                 WHERE id=?1",
                params![id, event.artist_id, event.contact_id, event.title, event.venue, event.city,
                    event.date, event.start_time, event.end_time, event.status, event.fee, event.notes],
            )
            .map_err(|e| e.to_string())?;
            let _ = crate::commands::data::sync_event_budget(&conn, id); // budget line (audit #7)
            Ok(id)
        }
        None => {
            conn.execute(
                "INSERT INTO events (artist_id, contact_id, title, venue, city, date, start_time, end_time, status, fee, notes)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
                params![event.artist_id, event.contact_id, event.title, event.venue, event.city,
                    event.date, event.start_time, event.end_time, event.status, event.fee, event.notes],
            )
            .map_err(|e| e.to_string())?;
            let id = conn.last_insert_rowid();
            let _ = crate::commands::data::sync_event_budget(&conn, id); // budget line (audit #7)
            Ok(id)
        }
    }
}

#[tauri::command(async)]
pub fn delete_event(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let before: Option<(Option<i64>, String)> = conn
        .query_row(
            "SELECT contact_id, status FROM events WHERE id = ?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();
    conn.execute("DELETE FROM events WHERE id=?1", params![id])
        .map_err(|e| e.to_string())?;
    // Deleting a confirmed gig withdraws the confirmation (status.rs).
    if let Some((Some(cid), st)) = before {
        if st == "confirmed" {
            status::on_event_cancelled(&conn, cid);
        }
    }
    let _ = crate::commands::data::sync_event_budget(&conn, id); // budget line (audit #7)
    crate::commands::insrt_gigs::after_event_change(&state.pool, id); // insrt dates (audit #19)
    Ok(())
}
