use crate::models::{Campaign, Contact};
use crate::AppState;
use rusqlite::{params, Connection};
use serde::Serialize;
use tauri::State;

#[tauri::command(async)]
pub fn list_campaigns(state: State<'_, AppState>) -> Result<Vec<Campaign>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT c.id, c.name, c.purpose, c.event_name, c.artist_id, c.target_date, c.status, c.color,
                    c.created_at, c.updated_at,
                    COALESCE((SELECT COUNT(*) FROM emails e WHERE e.campaign_id = c.id AND e.status='sent'), 0) AS sent_count,
                    COALESCE((SELECT COUNT(*) FROM emails e WHERE e.campaign_id = c.id AND e.status='sent' AND e.opened_at IS NOT NULL), 0) AS opened_count,
                    COALESCE((SELECT COUNT(DISTINCT e.contact_id) FROM emails e WHERE e.campaign_id = c.id AND e.status='sent' AND e.contact_id IS NOT NULL), 0) AS reached_count,
                    -- Real replies: detected by the IMAP sync (emails.replied_at) or
                    -- recorded on the audience row.
                    COALESCE((SELECT COUNT(*) FROM (
                        SELECT e.contact_id FROM emails e
                        WHERE e.campaign_id = c.id AND e.replied_at IS NOT NULL AND e.contact_id IS NOT NULL
                        UNION
                        SELECT cc.contact_id FROM campaign_contacts cc
                        WHERE cc.campaign_id = c.id AND cc.state = 'replied')), 0) AS replied_count,
                    COALESCE((SELECT COUNT(*) FROM campaign_contacts cc WHERE cc.campaign_id = c.id AND cc.state = 'planned'), 0) AS planned_count,
                    COALESCE((SELECT COUNT(*) FROM campaign_contacts cc WHERE cc.campaign_id = c.id), 0) AS audience_count
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

#[tauri::command(async)]
pub fn save_campaign(state: State<'_, AppState>, campaign: Campaign) -> Result<i64, String> {
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

#[tauri::command(async)]
pub fn delete_campaign(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM campaigns WHERE id=?1", params![id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Contacts already mailed in a campaign (a sent email carries its id, or goes to
/// their exact address). Bulk send excludes them so re-running a campaign after
/// adding contacts does not mail the same people twice.
#[tauri::command(async)]
pub fn list_campaign_recipients(state: State<'_, AppState>, campaign_id: i64) -> Result<Vec<i64>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT k.id FROM contacts k
             WHERE EXISTS (SELECT 1 FROM emails e WHERE e.campaign_id = ?1 AND e.status = 'sent'
                 AND (e.contact_id = k.id
                      OR (k.email IS NOT NULL AND trim(k.email) <> '' AND lower(trim(e.to_addr)) = lower(trim(k.email)))))",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![campaign_id], |r| r.get::<_, i64>(0))
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

// ---------------- Audience (campaign_contacts) ----------------
//
// A campaign's planned recipients, independent of what was already sent. Each
// row moves planned -> sent -> replied (or bounced) as the outbox works through
// it; emails.campaign_id is still written on every send, for the history.

/// Upsert one audience row to `state`. A reply is never downgraded back to
/// "sent" by a later follow-up.
pub fn mark_audience(conn: &Connection, campaign_id: i64, contact_id: i64, state: &str) {
    let _ = conn.execute(
        "INSERT INTO campaign_contacts (campaign_id, contact_id, state) VALUES (?1, ?2, ?3)
         ON CONFLICT(campaign_id, contact_id) DO UPDATE SET
             state = CASE WHEN campaign_contacts.state = 'replied' AND excluded.state = 'sent'
                          THEN campaign_contacts.state ELSE excluded.state END,
             updated_at = datetime('now')",
        params![campaign_id, contact_id, state],
    );
}

/// Same, for every audience row of the contacts using this address (a bounce
/// reports an address, not a contact).
pub fn mark_audience_by_address(conn: &Connection, addr: &str, state: &str) {
    let _ = conn.execute(
        "UPDATE campaign_contacts SET state = ?2, updated_at = datetime('now')
         WHERE contact_id IN (SELECT id FROM contacts
                              WHERE email IS NOT NULL AND lower(trim(email)) = lower(trim(?1)))",
        params![addr, state],
    );
}

/// Add contacts to a campaign's audience. Contacts already mailed in that
/// campaign come in as "sent", the others as "planned"; rows that already exist
/// are left alone. Returns how many were added.
#[tauri::command(async)]
pub fn add_to_campaign(
    state: State<'_, AppState>,
    campaign_id: i64,
    contact_ids: Vec<i64>,
) -> Result<usize, String> {
    let mut conn = state.pool.get().map_err(|e| e.to_string())?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let mut n = 0;
    for cid in contact_ids {
        n += tx
            .execute(
                "INSERT OR IGNORE INTO campaign_contacts (campaign_id, contact_id, state)
                 SELECT ?1, k.id,
                        CASE WHEN EXISTS (SELECT 1 FROM emails e WHERE e.campaign_id = ?1 AND e.status = 'sent'
                                AND (e.contact_id = k.id OR (k.email IS NOT NULL AND trim(k.email) <> ''
                                     AND lower(trim(e.to_addr)) = lower(trim(k.email)))))
                             THEN 'sent' ELSE 'planned' END
                 FROM contacts k WHERE k.id = ?2 AND EXISTS (SELECT 1 FROM campaigns WHERE id = ?1)",
                params![campaign_id, cid],
            )
            .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(n)
}

#[tauri::command(async)]
pub fn remove_from_campaign(
    state: State<'_, AppState>,
    campaign_id: i64,
    contact_ids: Vec<i64>,
) -> Result<usize, String> {
    let mut conn = state.pool.get().map_err(|e| e.to_string())?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let mut n = 0;
    for cid in contact_ids {
        n += tx
            .execute(
                "DELETE FROM campaign_contacts WHERE campaign_id = ?1 AND contact_id = ?2",
                params![campaign_id, cid],
            )
            .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(n)
}

#[derive(Serialize)]
pub struct AudienceRow {
    #[serde(flatten)]
    pub contact: Contact,
    pub state: String,
    pub added_at: String,
    /// The address is on the suppression list.
    pub suppressed: bool,
}

/// A campaign's audience with each recipient's state, planned first.
#[tauri::command(async)]
pub fn list_campaign_audience(
    state: State<'_, AppState>,
    campaign_id: i64,
) -> Result<Vec<AudienceRow>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT k.*, cc.state AS cc_state, cc.added_at AS cc_added_at,
                    EXISTS (SELECT 1 FROM suppressions s WHERE s.address = lower(trim(k.email))) AS cc_suppressed
             FROM campaign_contacts cc JOIN contacts k ON k.id = cc.contact_id
             WHERE cc.campaign_id = ?1
             ORDER BY CASE cc.state WHEN 'planned' THEN 0 WHEN 'sent' THEN 1 WHEN 'replied' THEN 2 ELSE 3 END,
                      k.name COLLATE NOCASE",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![campaign_id], |r| {
            Ok(AudienceRow {
                contact: Contact::from_row(r)?,
                state: r.get("cc_state")?,
                added_at: r.get("cc_added_at")?,
                suppressed: r.get::<_, i64>("cc_suppressed")? != 0,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}
