use crate::models::{BudgetItem, Kpi, Task, Template};
use crate::AppState;
use rusqlite::params;
use serde::Serialize;
use std::collections::HashMap;
use tauri::State;

// ---------- Templates ----------

#[tauri::command]
pub fn list_templates(state: State<AppState>) -> Result<Vec<Template>, String> {
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

#[tauri::command]
pub fn save_template(state: State<AppState>, template: Template) -> Result<i64, String> {
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

#[tauri::command]
pub fn delete_template(state: State<AppState>, id: i64) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM templates WHERE id=?1", params![id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ---------- Budget ----------

#[tauri::command]
pub fn list_budget(state: State<AppState>) -> Result<Vec<BudgetItem>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT id, category, item, min_cost, max_cost, actual, kind, notes, sort FROM budget_items ORDER BY kind, sort, id")
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
            })
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

#[tauri::command]
pub fn save_budget_item(state: State<AppState>, item: BudgetItem) -> Result<i64, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    match item.id {
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

#[tauri::command]
pub fn delete_budget_item(state: State<AppState>, id: i64) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM budget_items WHERE id=?1", params![id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ---------- Tasks ----------

#[tauri::command]
pub fn list_tasks(state: State<AppState>) -> Result<Vec<Task>, String> {
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

#[tauri::command]
pub fn save_task(state: State<AppState>, task: Task) -> Result<i64, String> {
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

#[tauri::command]
pub fn delete_task(state: State<AppState>, id: i64) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM tasks WHERE id=?1", params![id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ---------- KPIs ----------

#[tauri::command]
pub fn list_kpis(state: State<AppState>, artist_id: Option<i64>) -> Result<Vec<Kpi>, String> {
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

#[tauri::command]
pub fn save_kpi(state: State<AppState>, kpi: Kpi) -> Result<i64, String> {
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

#[tauri::command]
pub fn delete_kpi(state: State<AppState>, id: i64) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM kpis WHERE id=?1", params![id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ---------- Settings (generic key/value) ----------

#[tauri::command]
pub fn get_setting(state: State<AppState>, key: String) -> Result<Option<String>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let v: Option<String> = conn
        .query_row("SELECT value FROM settings WHERE key=?1", params![key], |r| r.get(0))
        .ok();
    Ok(v)
}

#[tauri::command]
pub fn set_setting(state: State<AppState>, key: String, value: String) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )
    .map_err(|e| e.to_string())?;
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
    pub budget_min: f64,
    pub budget_max: f64,
    pub budget_actual: f64,
    pub revenue_actual: f64,
    pub tasks_done: i64,
    pub tasks_total: i64,
}

#[tauri::command]
pub fn dashboard_stats(state: State<AppState>) -> Result<DashboardStats, String> {
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

    Ok(DashboardStats {
        by_status,
        by_category,
        total_contacts,
        emails_sent,
        budget_min,
        budget_max,
        budget_actual,
        revenue_actual,
        tasks_done,
        tasks_total,
    })
}
