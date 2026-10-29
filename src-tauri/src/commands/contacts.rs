use crate::models::Contact;
use crate::AppState;
use rusqlite::params;
use tauri::State;

/// Columns of a Pipeline row. The heavy free-text fields (`notes`, `reason`)
/// are left out of lists, a list of 3 000 contacts carried them all on every
/// filter change; the editor loads the full row with `get_contact`.
const LIST_COLUMNS: &str = "c.id, c.artist_id, c.category, c.priority, c.name, c.promoter, c.venue,
    c.type, c.area, c.scale, c.date, c.time, c.format, NULL AS reason, c.contact_channel, c.email,
    c.email_status, c.status, c.first_contact, c.follow_up, NULL AS notes, c.website, c.tags,
    c.organisation_id, o.name AS organisation_name, c.created_at, c.updated_at";

/// Filters shared by `list_contacts` and `count_contacts`, so the count is
/// exactly the size of the list.
#[derive(Default)]
struct Filter {
    category: Option<String>,
    status: Option<String>,
    search: Option<String>,
    artist_id: Option<i64>,
    organisation_id: Option<i64>,
}

fn where_clause(f: Filter) -> (String, Vec<Box<dyn rusqlite::types::ToSql>>) {
    let mut sql = String::from(" WHERE 1=1");
    let mut args: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    if let Some(a) = f.artist_id {
        sql.push_str(" AND c.artist_id = ?");
        args.push(Box::new(a));
    }
    if let Some(o) = f.organisation_id {
        sql.push_str(" AND c.organisation_id = ?");
        args.push(Box::new(o));
    }
    if let Some(c) = f.category.filter(|s| !s.is_empty() && s != "all") {
        sql.push_str(" AND c.category = ?");
        args.push(Box::new(c));
    }
    if let Some(s) = f.status.filter(|s| !s.is_empty() && s != "all") {
        sql.push_str(" AND c.status = ?");
        args.push(Box::new(s));
    }
    if let Some(q) = f.search.filter(|s| !s.trim().is_empty()) {
        sql.push_str(
            " AND (c.name LIKE ? OR c.promoter LIKE ? OR c.venue LIKE ? OR c.email LIKE ? OR c.area LIKE ? OR c.notes LIKE ? OR c.tags LIKE ? OR c.type LIKE ? OR o.name LIKE ?)",
        );
        let like = format!("%{}%", q.trim());
        for _ in 0..9 {
            args.push(Box::new(like.clone()));
        }
    }
    (sql, args)
}

#[tauri::command(async)]
#[allow(clippy::too_many_arguments)]
pub fn list_contacts(
    state: State<'_, AppState>,
    category: Option<String>,
    status: Option<String>,
    search: Option<String>,
    artist_id: Option<i64>,
    organisation_id: Option<i64>,
    sort: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
) -> Result<Vec<Contact>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let (filter, mut args) = where_clause(Filter { category, status, search, artist_id, organisation_id });
    let mut sql = format!(
        "SELECT {LIST_COLUMNS} FROM contacts c LEFT JOIN organisations o ON o.id = c.organisation_id{filter}"
    );
    // Whitelisted orderings only (the value comes from the UI). `c.id` last so
    // paging is stable between pages.
    sql.push_str(match sort.as_deref() {
        Some("name") => " ORDER BY c.name COLLATE NOCASE, c.id",
        Some("recent") => " ORDER BY c.updated_at DESC, c.id DESC",
        Some("created") => " ORDER BY c.created_at DESC, c.id DESC",
        Some("date") => " ORDER BY c.date IS NULL OR c.date = '', c.date, c.name COLLATE NOCASE, c.id",
        Some("area") => " ORDER BY COALESCE(NULLIF(c.area, ''), c.venue) IS NULL, COALESCE(NULLIF(c.area, ''), c.venue) COLLATE NOCASE, c.name COLLATE NOCASE, c.id",
        Some("organisation") => " ORDER BY o.name IS NULL, o.name COLLATE NOCASE, c.name COLLATE NOCASE, c.id",
        _ => " ORDER BY CASE UPPER(TRIM(c.priority)) WHEN 'P1' THEN 1 WHEN 'A' THEN 1 WHEN 'P2' THEN 2 WHEN 'B' THEN 2 WHEN 'P3' THEN 3 WHEN 'C' THEN 3 ELSE 4 END, c.name COLLATE NOCASE, c.id",
    });
    // No limit = the whole result (older callers: Agenda picker, BulkSend...).
    if let Some(l) = limit {
        sql.push_str(" LIMIT ? OFFSET ?");
        args.push(Box::new(l.clamp(1, 5000)));
        args.push(Box::new(offset.unwrap_or(0).max(0)));
    }

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

/// Same filters as `list_contacts`: the Pipeline pages its rows in, so the
/// size of the result set comes from here.
#[tauri::command(async)]
pub fn count_contacts(
    state: State<'_, AppState>,
    category: Option<String>,
    status: Option<String>,
    search: Option<String>,
    artist_id: Option<i64>,
    organisation_id: Option<i64>,
) -> Result<i64, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let (filter, args) = where_clause(Filter { category, status, search, artist_id, organisation_id });
    let sql = format!(
        "SELECT COUNT(*) FROM contacts c LEFT JOIN organisations o ON o.id = c.organisation_id{filter}"
    );
    let param_refs: Vec<&dyn rusqlite::types::ToSql> = args.iter().map(|b| b.as_ref()).collect();
    conn.query_row(&sql, param_refs.as_slice(), |r| r.get(0))
        .map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub fn get_contact(state: State<'_, AppState>, id: i64) -> Result<Option<Contact>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT c.*, o.name AS organisation_name FROM contacts c
             LEFT JOIN organisations o ON o.id = c.organisation_id WHERE c.id = ?",
        )
        .map_err(|e| e.to_string())?;
    let mut rows = stmt
        .query_map(params![id], Contact::from_row)
        .map_err(|e| e.to_string())?;
    match rows.next() {
        Some(r) => Ok(Some(r.map_err(|e| e.to_string())?)),
        None => Ok(None),
    }
}

#[tauri::command(async)]
pub fn create_contact(state: State<'_, AppState>, contact: Contact) -> Result<i64, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO contacts
            (artist_id, category, priority, name, promoter, venue, type, area, scale, date, time, format,
             reason, contact_channel, email, email_status, status, first_contact, follow_up, notes, website, tags,
             organisation_id)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23)",
        params![
            contact.artist_id, contact.category, contact.priority, contact.name, contact.promoter, contact.venue,
            contact.type_, contact.area, contact.scale, contact.date, contact.time, contact.format,
            contact.reason, contact.contact_channel, contact.email, contact.email_status,
            contact.status, contact.first_contact, contact.follow_up, contact.notes, contact.website, contact.tags,
            contact.organisation_id
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(conn.last_insert_rowid())
}

#[tauri::command(async)]
pub fn update_contact(state: State<'_, AppState>, contact: Contact) -> Result<(), String> {
    let id = contact.id.ok_or("missing id")?;
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE contacts SET
            category=?2, priority=?3, name=?4, promoter=?5, venue=?6, type=?7, area=?8, scale=?9,
            date=?10, time=?11, format=?12, reason=?13, contact_channel=?14, email=?15, email_status=?16,
            status=?17, first_contact=?18, follow_up=?19, notes=?20, website=?21, tags=?22, artist_id=?23,
            organisation_id=?24, updated_at=datetime('now')
         WHERE id=?1",
        params![
            id, contact.category, contact.priority, contact.name, contact.promoter, contact.venue,
            contact.type_, contact.area, contact.scale, contact.date, contact.time, contact.format,
            contact.reason, contact.contact_channel, contact.email, contact.email_status,
            contact.status, contact.first_contact, contact.follow_up, contact.notes, contact.website, contact.tags,
            contact.artist_id, contact.organisation_id
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command(async)]
pub fn update_contact_status(state: State<'_, AppState>, id: i64, status: String) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE contacts SET status=?2, updated_at=datetime('now') WHERE id=?1",
        params![id, status],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

const STATUSES: &[&str] = &[
    "to_contact", "contacted", "followed_up", "in_discussion", "confirmed", "declined",
    "no_answer", "to_evaluate", "low_priority",
];

/// Pipeline selection bar: one status for many contacts, in one transaction.
/// Returns how many rows changed.
#[tauri::command(async)]
pub fn update_contacts_status(state: State<'_, AppState>, ids: Vec<i64>, status: String) -> Result<usize, String> {
    if !STATUSES.contains(&status.as_str()) {
        return Err(format!("unknown status: {status}"));
    }
    let mut conn = state.pool.get().map_err(|e| e.to_string())?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let mut n = 0;
    {
        let mut stmt = tx
            .prepare("UPDATE contacts SET status=?2, updated_at=datetime('now') WHERE id=?1 AND status <> ?2")
            .map_err(|e| e.to_string())?;
        for id in ids {
            n += stmt.execute(params![id, status]).map_err(|e| e.to_string())?;
        }
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(n)
}

/// Pipeline selection bar: attach many contacts to one artist (None detaches).
#[tauri::command(async)]
pub fn assign_contacts_artist(state: State<'_, AppState>, ids: Vec<i64>, artist_id: Option<i64>) -> Result<usize, String> {
    let mut conn = state.pool.get().map_err(|e| e.to_string())?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let mut n = 0;
    {
        let mut stmt = tx
            .prepare("UPDATE contacts SET artist_id=?2, updated_at=datetime('now') WHERE id=?1 AND artist_id IS NOT ?2")
            .map_err(|e| e.to_string())?;
        for id in ids {
            n += stmt.execute(params![id, artist_id]).map_err(|e| e.to_string())?;
        }
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(n)
}

#[tauri::command(async)]
pub fn delete_contact(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM contacts WHERE id=?1", params![id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command(async)]
pub fn delete_contacts(state: State<'_, AppState>, ids: Vec<i64>) -> Result<(), String> {
    let mut conn = state.pool.get().map_err(|e| e.to_string())?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    for id in ids {
        tx.execute("DELETE FROM contacts WHERE id=?1", params![id])
            .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(())
}

// ---------------- Organisations (audit item 9) ----------------

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Organisation {
    #[serde(default)]
    pub id: Option<i64>,
    pub name: String,
    #[serde(default)]
    pub email_domain: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
    /// Number of contacts attached (read-only).
    #[serde(default)]
    pub contact_count: i64,
}

#[tauri::command(async)]
pub fn list_organisations(state: State<'_, AppState>) -> Result<Vec<Organisation>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT o.id, o.name, o.email_domain, o.notes,
                    (SELECT COUNT(*) FROM contacts c WHERE c.organisation_id = o.id)
             FROM organisations o ORDER BY o.name COLLATE NOCASE",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(Organisation {
                id: r.get(0)?,
                name: r.get(1)?,
                email_domain: r.get(2)?,
                notes: r.get(3)?,
                contact_count: r.get(4)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(|e| e.to_string())
}

/// Create (no id) or rename an organisation. Returns its id.
#[tauri::command(async)]
pub fn save_organisation(state: State<'_, AppState>, organisation: Organisation) -> Result<i64, String> {
    let name = organisation.name.trim().to_string();
    if name.is_empty() {
        return Err("name required".into());
    }
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    match organisation.id {
        Some(id) => {
            conn.execute(
                "UPDATE organisations SET name=?2, email_domain=?3, notes=?4 WHERE id=?1",
                params![id, name, organisation.email_domain, organisation.notes],
            )
            .map_err(|e| e.to_string())?;
            Ok(id)
        }
        None => {
            conn.execute(
                "INSERT INTO organisations (name, email_domain, notes) VALUES (?1, ?2, ?3)",
                params![name, organisation.email_domain, organisation.notes],
            )
            .map_err(|e| e.to_string())?;
            Ok(conn.last_insert_rowid())
        }
    }
}

/// Contacts are detached (ON DELETE SET NULL), never deleted.
#[tauri::command(async)]
pub fn delete_organisation(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.execute("UPDATE contacts SET organisation_id=NULL WHERE organisation_id=?1", params![id])
        .map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM organisations WHERE id=?1", params![id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ---------------- Follow-ups (booking cadence) ----------------

#[derive(serde::Serialize)]
pub struct Followup {
    pub contact_id: i64,
    pub name: String,
    pub email: Option<String>,
    pub venue: Option<String>,
    pub area: Option<String>,
    pub status: String,
    pub last_email: String,
    pub days_since: i64,
    pub email_count: i64,
    pub opened: bool,
    /// When the follow-up was due (YYYY-MM-DD).
    pub due: String,
}

/// Contacts due for a follow-up. An email counts for a contact when it carries
/// its id or went to its exact address (emails typed by hand, or a promoter
/// shared by several venues). Due on the contact's `follow_up` date when it is a
/// real date not before the last email, else 7 days after the last email. Still
/// under the 3-email ceiling, with no reply since the last email, and not
/// dismissed (a dismissal is lifted by the next send, see status.rs). Oldest
/// first, so the most overdue surface at the top.
#[tauri::command(async)]
pub fn list_followups(state: State<'_, AppState>) -> Result<Vec<Followup>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    // Close the sequences that ran dry before listing (3 emails, no reply).
    crate::commands::status::apply_no_answer(&conn);
    let sql = format!(
        "WITH m AS (
             SELECT e.contact_id AS cid, e.id AS eid FROM emails e
             WHERE e.status = 'sent' AND e.contact_id IS NOT NULL
             UNION
             SELECT c.id, e.id FROM emails e
             JOIN contacts c ON lower(trim(c.email)) = lower(trim(e.to_addr))
             WHERE e.status = 'sent' AND c.email IS NOT NULL AND trim(c.email) <> ''
         ),
         agg AS (
             SELECT m.cid,
                    MAX(e.sent_at) AS last_email,
                    COUNT(DISTINCT e.id) AS cnt,
                    MAX(CASE WHEN e.opened_at IS NOT NULL THEN 1 ELSE 0 END) AS opened,
                    MAX(e.replied_at) AS last_reply
             FROM m JOIN emails e ON e.id = m.eid
             GROUP BY m.cid
         )
         SELECT c.id, c.name, c.email, c.venue, c.area, c.status, a.last_email, a.cnt, a.opened,
                CAST(julianday('now') - julianday(a.last_email) AS INTEGER) AS days,
                CASE WHEN c.follow_up GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]*'
                          AND date(c.follow_up) >= date(a.last_email)
                     THEN date(c.follow_up)
                     ELSE date(a.last_email, '+{days} days') END AS due
         FROM agg a JOIN contacts c ON c.id = a.cid
         WHERE COALESCE(c.followup_dismissed, 0) = 0
           AND c.status NOT IN ('confirmed', 'declined', 'no_answer')
           AND a.cnt < {ceiling}
           AND (a.last_reply IS NULL OR a.last_reply < a.last_email)
           AND NOT EXISTS (SELECT 1 FROM email_replies r
                           WHERE r.contact_id = c.id AND r.kind = 'reply' AND r.received_at >= a.last_email)
           AND NOT EXISTS (SELECT 1 FROM suppressions s WHERE s.address = lower(trim(c.email)))
           AND due <= date('now')
         ORDER BY a.last_email ASC
         LIMIT 200",
        days = crate::commands::status::FOLLOWUP_DAYS,
        ceiling = crate::commands::status::NO_ANSWER_AFTER_EMAILS,
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(Followup {
                contact_id: r.get(0)?,
                name: r.get(1)?,
                email: r.get(2)?,
                venue: r.get(3)?,
                area: r.get(4)?,
                status: r.get(5)?,
                last_email: r.get(6)?,
                email_count: r.get(7)?,
                opened: r.get::<_, i64>(8)? != 0,
                days_since: r.get(9)?,
                due: r.get(10)?,
            })
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// Stop suggesting follow-ups for a contact (the "no response is a response" case).
#[tauri::command(async)]
pub fn dismiss_followup(state: State<'_, AppState>, contact_id: i64) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE contacts SET followup_dismissed = 1, updated_at = datetime('now') WHERE id = ?1",
        params![contact_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
