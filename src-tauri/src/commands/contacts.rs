use crate::models::Contact;
use crate::AppState;
use rusqlite::params;
use tauri::State;

#[tauri::command]
pub fn list_contacts(
    state: State<AppState>,
    category: Option<String>,
    status: Option<String>,
    search: Option<String>,
    artist_id: Option<i64>,
) -> Result<Vec<Contact>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let mut sql = String::from("SELECT * FROM contacts WHERE 1=1");
    let mut args: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    if let Some(a) = artist_id {
        sql.push_str(" AND artist_id = ?");
        args.push(Box::new(a));
    }
    if let Some(c) = category.filter(|s| !s.is_empty() && s != "all") {
        sql.push_str(" AND category = ?");
        args.push(Box::new(c));
    }
    if let Some(s) = status.filter(|s| !s.is_empty() && s != "all") {
        sql.push_str(" AND status = ?");
        args.push(Box::new(s));
    }
    if let Some(q) = search.filter(|s| !s.trim().is_empty()) {
        sql.push_str(
            " AND (name LIKE ? OR promoter LIKE ? OR venue LIKE ? OR email LIKE ? OR area LIKE ? OR notes LIKE ?)",
        );
        let like = format!("%{}%", q);
        for _ in 0..6 {
            args.push(Box::new(like.clone()));
        }
    }
    sql.push_str(
        " ORDER BY CASE priority WHEN 'P1' THEN 1 WHEN 'A' THEN 1 WHEN 'P2' THEN 2 WHEN 'B' THEN 2 WHEN 'P3' THEN 3 WHEN 'C' THEN 3 ELSE 4 END, name COLLATE NOCASE",
    );

    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let param_refs: Vec<&dyn rusqlite::types::ToSql> = args.iter().map(|b| b.as_ref()).collect();
    let rows = stmt
        .query_map(param_refs.as_slice(), Contact::from_row)
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

#[tauri::command]
pub fn get_contact(state: State<AppState>, id: i64) -> Result<Option<Contact>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT * FROM contacts WHERE id = ?")
        .map_err(|e| e.to_string())?;
    let mut rows = stmt
        .query_map(params![id], Contact::from_row)
        .map_err(|e| e.to_string())?;
    match rows.next() {
        Some(r) => Ok(Some(r.map_err(|e| e.to_string())?)),
        None => Ok(None),
    }
}

#[tauri::command]
pub fn create_contact(state: State<AppState>, contact: Contact) -> Result<i64, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO contacts
            (artist_id, category, priority, name, promoter, venue, type, area, scale, date, time, format,
             reason, contact_channel, email, email_status, status, first_contact, follow_up, notes, website, tags)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22)",
        params![
            contact.artist_id, contact.category, contact.priority, contact.name, contact.promoter, contact.venue,
            contact.type_, contact.area, contact.scale, contact.date, contact.time, contact.format,
            contact.reason, contact.contact_channel, contact.email, contact.email_status,
            contact.status, contact.first_contact, contact.follow_up, contact.notes, contact.website, contact.tags
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(conn.last_insert_rowid())
}

#[tauri::command]
pub fn update_contact(state: State<AppState>, contact: Contact) -> Result<(), String> {
    let id = contact.id.ok_or("missing id")?;
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE contacts SET
            category=?2, priority=?3, name=?4, promoter=?5, venue=?6, type=?7, area=?8, scale=?9,
            date=?10, time=?11, format=?12, reason=?13, contact_channel=?14, email=?15, email_status=?16,
            status=?17, first_contact=?18, follow_up=?19, notes=?20, website=?21, tags=?22, artist_id=?23,
            updated_at=datetime('now')
         WHERE id=?1",
        params![
            id, contact.category, contact.priority, contact.name, contact.promoter, contact.venue,
            contact.type_, contact.area, contact.scale, contact.date, contact.time, contact.format,
            contact.reason, contact.contact_channel, contact.email, contact.email_status,
            contact.status, contact.first_contact, contact.follow_up, contact.notes, contact.website, contact.tags,
            contact.artist_id
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn update_contact_status(state: State<AppState>, id: i64, status: String) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE contacts SET status=?2, updated_at=datetime('now') WHERE id=?1",
        params![id, status],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn delete_contact(state: State<AppState>, id: i64) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM contacts WHERE id=?1", params![id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn delete_contacts(state: State<AppState>, ids: Vec<i64>) -> Result<(), String> {
    let mut conn = state.pool.get().map_err(|e| e.to_string())?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    for id in ids {
        tx.execute("DELETE FROM contacts WHERE id=?1", params![id])
            .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(())
}
