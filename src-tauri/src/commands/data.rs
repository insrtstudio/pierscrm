use crate::models::{BudgetItem, Kpi, Task, Template};
use crate::AppState;
use rusqlite::params;
use serde::Serialize;
use std::collections::HashMap;
use tauri::State;

// ---------- Templates ----------

#[tauri::command(async)]
pub fn list_templates(state: State<'_, AppState>) -> Result<Vec<Template>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT id, name, subject, body, created_at FROM templates ORDER BY id")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(Template {
                id: r.get(0)?,
                name: r.get(1)?,
                subject: r.get(2)?,
                body: r.get(3)?,
                created_at: r.get(4)?,
            })
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

#[tauri::command(async)]
pub fn save_template(state: State<'_, AppState>, template: Template) -> Result<i64, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    match template.id {
        Some(id) => {
            conn.execute(
                "UPDATE templates SET name=?2, subject=?3, body=?4 WHERE id=?1",
                params![id, template.name, template.subject, template.body],
            )
            .map_err(|e| e.to_string())?;
            Ok(id)
        }
        None => {
            conn.execute(
                "INSERT INTO templates (name, subject, body) VALUES (?1,?2,?3)",
                params![template.name, template.subject, template.body],
            )
            .map_err(|e| e.to_string())?;
            Ok(conn.last_insert_rowid())
        }
    }
}

#[tauri::command(async)]
pub fn delete_template(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM templates WHERE id=?1", params![id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ---------- Budget ----------

#[tauri::command(async)]
pub fn list_budget(state: State<'_, AppState>) -> Result<Vec<BudgetItem>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT id, category, item, min_cost, max_cost, actual, kind, notes, sort, event_id FROM budget_items ORDER BY kind, sort, id")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(BudgetItem {
                id: r.get(0)?,
                category: r.get(1)?,
                item: r.get(2)?,
                min_cost: r.get(3)?,
                max_cost: r.get(4)?,
                actual: r.get(5)?,
                kind: r.get(6)?,
                notes: r.get(7)?,
                sort: r.get(8)?,
                event_id: r.get(9)?,
            })
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

#[tauri::command(async)]
pub fn save_budget_item(state: State<'_, AppState>, item: BudgetItem) -> Result<i64, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    match item.id {
        // A gig's line belongs to its event: amount and kind stay what the
        // Agenda says (only the label, category and notes are editable), and
        // event_id is never written from here.
        Some(id) if linked_event(&conn, id).is_some() => {
            conn.execute(
                "UPDATE budget_items SET category=?2, item=?3, notes=?4, sort=?5 WHERE id=?1",
                params![id, item.category, item.item, item.notes, item.sort],
            )
            .map_err(|e| e.to_string())?;
            Ok(id)
        }
        Some(id) => {
            conn.execute(
                "UPDATE budget_items SET category=?2, item=?3, min_cost=?4, max_cost=?5, actual=?6, kind=?7, notes=?8, sort=?9 WHERE id=?1",
                params![id, item.category, item.item, item.min_cost, item.max_cost, item.actual, item.kind, item.notes, item.sort],
            )
            .map_err(|e| e.to_string())?;
            Ok(id)
        }
        None => {
            conn.execute(
                "INSERT INTO budget_items (category, item, min_cost, max_cost, actual, kind, notes, sort) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                params![item.category, item.item, item.min_cost, item.max_cost, item.actual, item.kind, item.notes, item.sort],
            )
            .map_err(|e| e.to_string())?;
            Ok(conn.last_insert_rowid())
        }
    }
}

#[tauri::command(async)]
pub fn delete_budget_item(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    // Deleting a gig's line would only last until the next sync: the fee is
    // removed by cancelling or editing the event in the Agenda.
    if linked_event(&conn, id).is_some() {
        return Err("This line follows an Agenda event: change the event instead.".into());
    }
    conn.execute("DELETE FROM budget_items WHERE id=?1", params![id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Attach a hand-typed revenue line to the Agenda gig it duplicates (audit
/// item 7). The line becomes that gig's line: the automatic one is dropped,
/// the amount follows the event from now on, label and notes are kept.
#[tauri::command(async)]
pub fn link_budget_item(state: State<'_, AppState>, id: i64, event_id: i64) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    link_budget_line(&conn, id, event_id)
}

pub(crate) fn link_budget_line(conn: &rusqlite::Connection, id: i64, event_id: i64) -> Result<(), String> {
    use rusqlite::OptionalExtension;
    let current: Option<Option<i64>> = conn
        .query_row("SELECT event_id FROM budget_items WHERE id=?1", params![id], |r| r.get(0))
        .optional()
        .map_err(|e| e.to_string())?;
    match current {
        None => return Err("Budget line not found.".into()),
        Some(Some(e)) if e == event_id => return Ok(()),
        Some(Some(_)) => return Err("This line already follows another Agenda event.".into()),
        Some(None) => {}
    }
    // Only a confirmed gig with a fee owns a line (sync_event_budget would
    // delete it otherwise, taking the user's line with it).
    let owns_line: bool = conn
        .query_row(
            "SELECT status='confirmed' AND COALESCE(fee,0) > 0 FROM events WHERE id=?1",
            params![event_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .unwrap_or(false);
    if !owns_line {
        return Err("Only a confirmed gig with a fee can carry a budget line.".into());
    }
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    tx.execute(
        "DELETE FROM budget_items WHERE event_id=?1 AND id<>?2",
        params![event_id, id],
    )
    .map_err(|e| e.to_string())?;
    tx.execute("UPDATE budget_items SET event_id=?2 WHERE id=?1", params![id, event_id])
        .map_err(|e| e.to_string())?;
    sync_event_budget(&tx, event_id).map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())
}

fn linked_event(conn: &rusqlite::Connection, budget_id: i64) -> Option<i64> {
    conn.query_row(
        "SELECT event_id FROM budget_items WHERE id=?1",
        params![budget_id],
        |r| r.get::<_, Option<i64>>(0),
    )
    .ok()
    .flatten()
}

/// Keep the revenue line of one Agenda event in step with it (audit item 7).
///
/// A confirmed gig with a fee owns exactly one `kind='revenue'` line carrying
/// its `event_id`; any other state (hold, cancelled, no fee, event deleted)
/// owns none. The fee therefore exists once, in budget_items, and every total
/// (Budget page, dashboard net) reads it from there: there is no separate
/// "gig fees" sum to add on top any more, so it cannot be counted twice.
///
/// Call it after any write to an event (save, delete, status change).
pub(crate) fn sync_event_budget(conn: &rusqlite::Connection, event_id: i64) -> rusqlite::Result<()> {
    use rusqlite::OptionalExtension;
    let ev: Option<(String, String, Option<String>, Option<String>, String, Option<f64>)> = conn
        .query_row(
            "SELECT title, date, venue, city, status, fee FROM events WHERE id=?1",
            params![event_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )
        .optional()?;
    match ev {
        Some((title, date, venue, city, status, Some(fee))) if status == "confirmed" && fee > 0.0 => {
            let updated = conn.execute(
                "UPDATE budget_items SET actual=?2, kind='revenue', min_cost=0, max_cost=0,
                    item=CASE WHEN trim(item)='' THEN ?3 ELSE item END
                 WHERE event_id=?1",
                params![event_id, fee, title],
            )?;
            if updated == 0 {
                let notes = [Some(date), venue, city]
                    .into_iter()
                    .flatten()
                    .filter(|s| !s.trim().is_empty())
                    .collect::<Vec<_>>()
                    .join(" · ");
                conn.execute(
                    "INSERT INTO budget_items (category, item, min_cost, max_cost, actual, kind, notes, sort, event_id)
                     VALUES (NULL, ?1, 0, 0, ?2, 'revenue', ?3,
                             (SELECT COALESCE(MAX(sort), 0) + 1 FROM budget_items), ?4)",
                    params![title, fee, notes, event_id],
                )?;
            }
        }
        _ => {
            conn.execute("DELETE FROM budget_items WHERE event_id=?1", params![event_id])?;
        }
    }
    Ok(())
}

/// `sync_event_budget` for every event, plus cleanup of lines whose event is
/// gone. Run at startup so any path that changed an event is caught up.
pub(crate) fn sync_all_event_budgets(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    let ids: Vec<i64> = {
        let mut stmt = conn.prepare(
            "SELECT id FROM events
             UNION SELECT event_id FROM budget_items WHERE event_id IS NOT NULL",
        )?;
        let it = stmt.query_map([], |r| r.get(0))?;
        it.collect::<rusqlite::Result<_>>()?
    };
    for id in ids {
        sync_event_budget(conn, id)?;
    }
    Ok(())
}

/// Total Meta Ads spend recorded by Audience (read-only, shown as an expense).
pub(crate) fn ad_spend_total(conn: &rusqlite::Connection) -> f64 {
    conn.query_row("SELECT COALESCE(SUM(spend), 0) FROM pulse_meta_spend", [], |r| r.get(0))
        .unwrap_or(0.0)
}

// ---------- Tasks ----------

#[tauri::command(async)]
pub fn list_tasks(state: State<'_, AppState>) -> Result<Vec<Task>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT id, period, title, done, owner, due_date, sort FROM tasks ORDER BY sort, id")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(Task {
                id: r.get(0)?,
                period: r.get(1)?,
                title: r.get(2)?,
                done: r.get::<_, i64>(3)? != 0,
                owner: r.get(4)?,
                due_date: r.get(5)?,
                sort: r.get(6)?,
            })
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

#[tauri::command(async)]
pub fn save_task(state: State<'_, AppState>, task: Task) -> Result<i64, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    match task.id {
        Some(id) => {
            conn.execute(
                "UPDATE tasks SET period=?2, title=?3, done=?4, owner=?5, due_date=?6, sort=?7 WHERE id=?1",
                params![id, task.period, task.title, task.done as i64, task.owner, task.due_date, task.sort],
            )
            .map_err(|e| e.to_string())?;
            Ok(id)
        }
        None => {
            conn.execute(
                "INSERT INTO tasks (period, title, done, owner, due_date, sort) VALUES (?1,?2,?3,?4,?5,?6)",
                params![task.period, task.title, task.done as i64, task.owner, task.due_date, task.sort],
            )
            .map_err(|e| e.to_string())?;
            Ok(conn.last_insert_rowid())
        }
    }
}

#[tauri::command(async)]
pub fn delete_task(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM tasks WHERE id=?1", params![id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ---------- KPIs ----------

#[tauri::command(async)]
pub fn list_kpis(state: State<'_, AppState>, artist_id: Option<i64>) -> Result<Vec<Kpi>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let map = |r: &rusqlite::Row| -> rusqlite::Result<Kpi> {
        Ok(Kpi {
            id: r.get(0)?,
            artist_id: r.get(1)?,
            goal: r.get(2)?,
            kpi: r.get(3)?,
            target: r.get(4)?,
            actual: r.get(5)?,
            sort: r.get(6)?,
        })
    };
    let mut out = Vec::new();
    match artist_id {
        Some(a) => {
            let mut stmt = conn
                .prepare("SELECT id, artist_id, goal, kpi, target, actual, sort FROM kpis WHERE artist_id=?1 ORDER BY sort, id")
                .map_err(|e| e.to_string())?;
            let rows = stmt.query_map(params![a], map).map_err(|e| e.to_string())?;
            for r in rows {
                out.push(r.map_err(|e| e.to_string())?);
            }
        }
        None => {
            let mut stmt = conn
                .prepare("SELECT id, artist_id, goal, kpi, target, actual, sort FROM kpis ORDER BY sort, id")
                .map_err(|e| e.to_string())?;
            let rows = stmt.query_map([], map).map_err(|e| e.to_string())?;
            for r in rows {
                out.push(r.map_err(|e| e.to_string())?);
            }
        }
    }
    Ok(out)
}

#[tauri::command(async)]
pub fn save_kpi(state: State<'_, AppState>, kpi: Kpi) -> Result<i64, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    match kpi.id {
        Some(id) => {
            conn.execute(
                "UPDATE kpis SET artist_id=?2, goal=?3, kpi=?4, target=?5, actual=?6, sort=?7 WHERE id=?1",
                params![id, kpi.artist_id, kpi.goal, kpi.kpi, kpi.target, kpi.actual, kpi.sort],
            )
            .map_err(|e| e.to_string())?;
            Ok(id)
        }
        None => {
            conn.execute(
                "INSERT INTO kpis (artist_id, goal, kpi, target, actual, sort) VALUES (?1,?2,?3,?4,?5,?6)",
                params![kpi.artist_id, kpi.goal, kpi.kpi, kpi.target, kpi.actual, kpi.sort],
            )
            .map_err(|e| e.to_string())?;
            Ok(conn.last_insert_rowid())
        }
    }
}

#[tauri::command(async)]
pub fn delete_kpi(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM kpis WHERE id=?1", params![id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ---------- Settings (generic key/value) ----------

#[tauri::command(async)]
pub fn get_setting(state: State<'_, AppState>, key: String) -> Result<Option<String>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    // Secrets come back masked (see crate::secrets), never in clear.
    Ok(crate::secrets::ui_setting(&conn, &key))
}

#[tauri::command(async)]
pub fn set_setting(state: State<'_, AppState>, key: String, value: String) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    // Secret keys go to the Keychain; an unchanged masked value is a no-op.
    crate::secrets::write_setting(&conn, &key, &value)?;
    // New Spotify credentials must not keep using a token minted for the old ones.
    if key.starts_with("spotify_") {
        crate::commands::spotify::invalidate_token_cache();
    }
    Ok(())
}

// ---------- Printing ----------

/// Trigger the native print dialog (macOS → "Save as PDF") for the main webview.
/// More reliable than the webview's own `window.print()` in some setups.
#[tauri::command]
pub fn print_page(window: tauri::WebviewWindow) -> Result<(), String> {
    window.print().map_err(|e| e.to_string())
}

// ---------- Dashboard ----------

#[derive(Serialize)]
pub struct DashboardStats {
    pub by_status: HashMap<String, i64>,
    pub by_category: HashMap<String, i64>,
    pub total_contacts: i64,
    pub emails_sent: i64,
    /// Sent emails with at least one tracked open (open rate numerator).
    pub emails_opened: i64,
    pub budget_min: f64,
    pub budget_max: f64,
    pub budget_actual: f64,
    /// All revenue lines, gig fees included (they are budget lines linked to
    /// their event, see `sync_event_budget`).
    pub revenue_actual: f64,
    /// Part of `revenue_actual` coming from confirmed Agenda gigs. Informative
    /// only: never add it to `revenue_actual`.
    pub gig_fees: f64,
    /// Meta Ads spend recorded by Audience. Not in `budget_actual`; `net`
    /// accounts for it.
    pub ad_spend: f64,
    /// revenue_actual - budget_actual - ad_spend.
    pub net: f64,
    pub tasks_done: i64,
    pub tasks_total: i64,
    /// Open tasks whose due date is today or earlier.
    pub tasks_overdue: i64,
}

#[tauri::command(async)]
pub fn dashboard_stats(state: State<'_, AppState>) -> Result<DashboardStats, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;

    let mut by_status = HashMap::new();
    {
        let mut stmt = conn
            .prepare("SELECT status, COUNT(*) FROM contacts GROUP BY status")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
            .map_err(|e| e.to_string())?;
        for r in rows {
            let (k, v) = r.map_err(|e| e.to_string())?;
            by_status.insert(k, v);
        }
    }

    let mut by_category = HashMap::new();
    {
        let mut stmt = conn
            .prepare("SELECT category, COUNT(*) FROM contacts GROUP BY category")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
            .map_err(|e| e.to_string())?;
        for r in rows {
            let (k, v) = r.map_err(|e| e.to_string())?;
            by_category.insert(k, v);
        }
    }

    let total_contacts: i64 = conn
        .query_row("SELECT COUNT(*) FROM contacts", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    let emails_sent: i64 = conn
        .query_row("SELECT COUNT(*) FROM emails WHERE status='sent'", [], |r| r.get(0))
        .unwrap_or(0);

    let emails_opened: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM emails WHERE status='sent' AND opened_at IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let gig_fees: f64 = conn
        .query_row(
            "SELECT COALESCE(SUM(actual),0) FROM budget_items WHERE kind='revenue' AND event_id IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0.0);
    let ad_spend = ad_spend_total(&conn);

    let (budget_min, budget_max, budget_actual): (f64, f64, f64) = conn
        .query_row(
            "SELECT COALESCE(SUM(min_cost),0), COALESCE(SUM(max_cost),0), COALESCE(SUM(actual),0) FROM budget_items WHERE kind='expense'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap_or((0.0, 0.0, 0.0));
    let revenue_actual: f64 = conn
        .query_row(
            "SELECT COALESCE(SUM(actual),0) FROM budget_items WHERE kind='revenue'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0.0);

    let tasks_total: i64 = conn
        .query_row("SELECT COUNT(*) FROM tasks", [], |r| r.get(0))
        .unwrap_or(0);
    let tasks_done: i64 = conn
        .query_row("SELECT COUNT(*) FROM tasks WHERE done=1", [], |r| r.get(0))
        .unwrap_or(0);
    let tasks_overdue: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM tasks WHERE done=0 AND due_date IS NOT NULL AND due_date <> ''
               AND due_date <= date('now','localtime')",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    Ok(DashboardStats {
        by_status,
        by_category,
        total_contacts,
        emails_sent,
        emails_opened,
        budget_min,
        budget_max,
        budget_actual,
        revenue_actual,
        gig_fees,
        ad_spend,
        net: revenue_actual - budget_actual - ad_spend,
        tasks_done,
        tasks_total,
        tasks_overdue,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> rusqlite::Connection {
        let c = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::migrate_for_tests(&c).unwrap();
        c
    }

    fn gig(c: &rusqlite::Connection, status: &str, fee: f64) -> i64 {
        c.execute(
            "INSERT INTO events (title, date, status, fee) VALUES ('Gig', '2026-11-20', ?1, ?2)",
            params![status, fee],
        )
        .unwrap();
        let id = c.last_insert_rowid();
        sync_event_budget(c, id).unwrap();
        id
    }

    fn revenue(c: &rusqlite::Connection) -> f64 {
        c.query_row("SELECT COALESCE(SUM(actual),0) FROM budget_items WHERE kind='revenue'", [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn linking_a_manual_line_removes_the_double_count() {
        let c = conn();
        let ev = gig(&c, "confirmed", 800.0);
        c.execute(
            "INSERT INTO budget_items (item, actual, kind, notes) VALUES ('Cachet Paradiso', 750, 'revenue', 'virement')",
            [],
        )
        .unwrap();
        let manual = c.last_insert_rowid();
        assert_eq!(revenue(&c), 1550.0);

        link_budget_line(&c, manual, ev).unwrap();
        assert_eq!(revenue(&c), 800.0, "counted once, at the Agenda fee");
        let (item, notes, event_id): (String, String, i64) = c
            .query_row("SELECT item, notes, event_id FROM budget_items WHERE kind='revenue'", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap();
        assert_eq!((item.as_str(), notes.as_str(), event_id), ("Cachet Paradiso", "virement", ev));
        // Idempotent, and a re-sync keeps the user's line.
        link_budget_line(&c, manual, ev).unwrap();
        sync_all_event_budgets(&c).unwrap();
        assert_eq!(revenue(&c), 800.0);
    }

    #[test]
    fn only_confirmed_paid_gigs_take_a_line() {
        let c = conn();
        let hold = gig(&c, "hold", 500.0);
        c.execute("INSERT INTO budget_items (item, actual, kind) VALUES ('x', 500, 'revenue')", []).unwrap();
        let manual = c.last_insert_rowid();
        assert!(link_budget_line(&c, manual, hold).is_err());
        assert!(link_budget_line(&c, 9999, hold).is_err());
        let n: i64 = c.query_row("SELECT COUNT(*) FROM budget_items", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1, "nothing touched");
    }
}
