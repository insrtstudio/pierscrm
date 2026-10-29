//! Venue Intelligence, J2 harvesting engine.
//!
//! A run (`vi_runs`) is split into atomic tasks (`vi_tasks`), one per area over a
//! 90-day window. A single background worker (spawned on the Tauri async runtime)
//! drains the queue: fetch RA listings, upsert venues / evidence / promoters, then
//! qualify. Everything is persisted, so closing the app mid-run loses nothing:
//! "Reprendre" reclaims unfinished tasks and continues. Progress streams to the UI
//! via the `vi:run-progress` event.
//!
//! Politeness: 1.5 s between page requests, exponential backoff on 429/5xx, at most
//! `max_tentatives` attempts per task, then the task is marked `echec` without
//! blocking the run.

use crate::commands::ra;
use crate::db::{normalise, DbPool};
use crate::{AppState, CancelSet};
use chrono::{Duration, NaiveDate, Utc};
use rusqlite::params;
use serde::Serialize;
use serde_json::json;
use tauri::{AppHandle, Emitter, State};

const PAGE_SIZE: i64 = 100;
const POLITENESS_MS: u64 = 1500;
const WINDOW_DAYS: i64 = 90;

/// Runs that currently have live workers. Guards against a second set of workers
/// on the same run (double-click on Resume, Resume before Stop has drained), which
/// would process in-flight tasks twice.
static ACTIVE_RUNS: std::sync::LazyLock<std::sync::Mutex<std::collections::HashSet<i64>>> =
    std::sync::LazyLock::new(Default::default);

fn run_is_active(run_id: i64) -> bool {
    ACTIVE_RUNS.lock().unwrap_or_else(|e| e.into_inner()).contains(&run_id)
}

/// Removes the run from ACTIVE_RUNS when the coordinator ends, even on panic.
struct ActiveRunGuard(i64);
impl Drop for ActiveRunGuard {
    fn drop(&mut self) {
        ACTIVE_RUNS.lock().unwrap_or_else(|e| e.into_inner()).remove(&self.0);
    }
}

// ---------------- Progress event ----------------

#[derive(Serialize, Clone)]
pub struct RunProgress {
    pub run_id: i64,
    pub statut: String,
    pub tasks_total: i64,
    pub tasks_done: i64,
    pub tasks_echec: i64,
    pub venues: i64,
    pub evidence: i64,
    /// English fallback of `code`, for anything that does not translate.
    pub message: Option<String>,
    /// Stable key the UI translates (`vi_progress.<code>` in the i18n files),
    /// with its interpolation values in `args`. The worker used to send French
    /// sentences only, so the English UI showed French progress lines.
    pub code: Option<String>,
    pub args: Option<serde_json::Value>,
}

/// One progress line: a translatable code, its values, and the English text.
pub(crate) struct ProgressMsg {
    pub code: &'static str,
    pub args: serde_json::Value,
    pub text: String,
}

impl ProgressMsg {
    fn new(code: &'static str, args: serde_json::Value, text: String) -> Self {
        Self { code, args, text }
    }
}

fn run_stats(pool: &DbPool, run_id: i64) -> RunProgress {
    let conn = pool.get().unwrap();
    let one = |sql: &str, p: &[&dyn rusqlite::ToSql]| -> i64 {
        conn.query_row(sql, p, |r| r.get(0)).unwrap_or(0)
    };
    let total = one("SELECT COUNT(*) FROM vi_tasks WHERE run_id=?1", &[&run_id]);
    let done = one(
        "SELECT COUNT(*) FROM vi_tasks WHERE run_id=?1 AND statut='termine'",
        &[&run_id],
    );
    let echec = one(
        "SELECT COUNT(*) FROM vi_tasks WHERE run_id=?1 AND statut='echec'",
        &[&run_id],
    );
    let statut: String = conn
        .query_row("SELECT statut FROM vi_runs WHERE id=?1", params![run_id], |r| r.get(0))
        .unwrap_or_else(|_| "en_cours".into());
    RunProgress {
        run_id,
        statut,
        tasks_total: total,
        tasks_done: done,
        tasks_echec: echec,
        venues: one("SELECT COUNT(*) FROM vi_venues", &[]),
        evidence: one("SELECT COUNT(*) FROM vi_evidence", &[]),
        message: None,
        code: None,
        args: None,
    }
}

fn emit(app: &AppHandle, pool: &DbPool, run_id: i64, msg: Option<ProgressMsg>) {
    let mut p = run_stats(pool, run_id);
    if let Some(m) = msg {
        p.message = Some(m.text);
        p.code = Some(m.code.to_string());
        p.args = Some(m.args);
    }
    let _ = app.emit("vi:run-progress", p);
}

// ---------------- Date windows ----------------

fn windows(year_from: i32, year_to: i32) -> Vec<(NaiveDate, NaiveDate)> {
    let start = NaiveDate::from_ymd_opt(year_from, 1, 1).unwrap_or_else(|| Utc::now().date_naive());
    let today = Utc::now().date_naive();
    let mut end = NaiveDate::from_ymd_opt(year_to, 12, 31).unwrap_or(today);
    if end > today {
        end = today;
    }
    let mut out = Vec::new();
    let mut cur = start;
    while cur <= end {
        let wend = (cur + Duration::days(WINDOW_DAYS - 1)).min(end);
        out.push((cur, wend));
        cur = wend + Duration::days(1);
    }
    out
}

fn iso_start(d: NaiveDate) -> String {
    format!("{}T00:00:00.000Z", d.format("%Y-%m-%d"))
}
fn iso_end(d: NaiveDate) -> String {
    format!("{}T23:59:59.999Z", d.format("%Y-%m-%d"))
}

// ---------------- Public commands ----------------

/// Resolve one seeded area's RA id (via the GraphQL `area` query) and store it.
#[tauri::command]
pub async fn vi_resolve_area(state: State<'_, AppState>, id: i64) -> Result<Option<i64>, String> {
    let (slug, pays): (String, Option<String>) = {
        let conn = state.pool.get().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT slug, pays FROM vi_ra_areas WHERE id=?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|e| e.to_string())?
    };
    let (country, url_name) = slug.split_once('/').unwrap_or(("", slug.as_str()));
    let country = pays.unwrap_or_else(|| country.to_uppercase());
    let client = ra::client();
    let resolved = ra::resolve_area(&client, &country, url_name)
        .await
        .map_err(|e| e.message)?;
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE vi_ra_areas SET ra_area_id=?2, resolved_at=CASE WHEN ?2 IS NOT NULL THEN datetime('now') ELSE resolved_at END WHERE id=?1",
        params![id, resolved],
    )
    .map_err(|e| e.to_string())?;
    Ok(resolved)
}

/// Resolve every active, unresolved area. Returns how many got an id.
#[tauri::command]
pub async fn vi_resolve_all_areas(state: State<'_, AppState>) -> Result<i64, String> {
    let todo: Vec<(i64, String, Option<String>)> = {
        let conn = state.pool.get().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare("SELECT id, slug, pays FROM vi_ra_areas WHERE actif=1 AND ra_area_id IS NULL")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .map_err(|e| e.to_string())?;
        rows.filter_map(|r| r.ok()).collect()
    };
    let client = ra::client();
    let mut n = 0i64;
    for (id, slug, pays) in todo {
        let (country, url_name) = slug.split_once('/').unwrap_or(("", slug.as_str()));
        let country = pays.unwrap_or_else(|| country.to_uppercase());
        if let Ok(Some(area_id)) = ra::resolve_area(&client, &country, url_name).await {
            let conn = state.pool.get().map_err(|e| e.to_string())?;
            let _ = conn.execute(
                "UPDATE vi_ra_areas SET ra_area_id=?2, resolved_at=datetime('now') WHERE id=?1",
                params![id, area_id],
            );
            n += 1;
        }
        tokio::time::sleep(std::time::Duration::from_millis(POLITENESS_MS)).await;
    }
    Ok(n)
}

/// Start a harvest run over the given areas (vi_ra_areas ids; empty = all active
/// resolved) and a year range. Creates the task queue and spawns the worker.
#[tauri::command]
pub async fn vi_start_harvest(
    app: AppHandle,
    state: State<'_, AppState>,
    area_ids: Vec<i64>,
    year_from: i32,
    year_to: i32,
) -> Result<i64, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;

    // Resolve target areas (must have a ra_area_id).
    let mut areas: Vec<(i64, i64, String, String)> = Vec::new(); // vi_id, ra_id, name, country
    {
        let base = "SELECT id, ra_area_id, COALESCE(libelle, slug), COALESCE(pays,'') FROM vi_ra_areas WHERE actif=1 AND ra_area_id IS NOT NULL";
        if area_ids.is_empty() {
            let mut stmt = conn.prepare(base).map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
                .map_err(|e| e.to_string())?;
            areas = rows.filter_map(|r| r.ok()).collect();
        } else {
            for aid in &area_ids {
                if let Ok(row) = conn.query_row(
                    &format!("{} AND id=?1", base),
                    params![aid],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                ) {
                    areas.push(row);
                }
            }
        }
    }
    if areas.is_empty() {
        return Err("vi.no_resolved_areas".into());
    }

    let wins = windows(year_from, year_to);
    let params_json = json!({ "area_ids": area_ids, "year_from": year_from, "year_to": year_to,
        "areas": areas.len(), "windows": wins.len() });

    conn.execute(
        "INSERT INTO vi_runs (type, params, statut, started_at) VALUES ('harvest', ?1, 'en_cours', datetime('now'))",
        params![params_json.to_string()],
    )
    .map_err(|e| e.to_string())?;
    let run_id = conn.last_insert_rowid();

    // One task per area x window.
    let mut stmt = conn
        .prepare("INSERT INTO vi_tasks (run_id, type, payload, statut) VALUES (?1,'harvest_area_window',?2,'en_attente')")
        .map_err(|e| e.to_string())?;
    for (vi_id, ra_id, name, country) in &areas {
        for (ws, we) in &wins {
            let payload = json!({
                "ra_area_id": ra_id, "vi_area_id": vi_id, "area_name": name, "country": country,
                "gte": iso_start(*ws), "lte": iso_end(*we)
            });
            stmt.execute(params![run_id, payload.to_string()])
                .map_err(|e| e.to_string())?;
        }
    }
    drop(stmt);

    spawn_worker(state.pool.clone(), state.cancels.clone(), app, run_id);
    Ok(run_id)
}

/// Resume an interrupted run: reclaim stale/locked tasks, spawn the worker again.
#[tauri::command]
pub async fn vi_resume_run(
    app: AppHandle,
    state: State<'_, AppState>,
    run_id: i64,
) -> Result<(), String> {
    if run_is_active(run_id) {
        return Err("vi.run_still_active".into());
    }
    // Clear any stale stop flag first, otherwise the resumed run would inherit the
    // cancel and immediately exit doing nothing.
    state
        .cancels
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&run_id);
    {
        let conn = state.pool.get().map_err(|e| e.to_string())?;
        // Reclaim tasks stuck 'en_cours' (crash mid-task) back to 'en_attente'.
        conn.execute(
            "UPDATE vi_tasks SET statut='en_attente', locked_at=NULL
             WHERE run_id=?1 AND statut='en_cours'",
            params![run_id],
        )
        .map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE vi_runs SET statut='en_cours' WHERE id=?1",
            params![run_id],
        )
        .map_err(|e| e.to_string())?;
    }
    spawn_worker(state.pool.clone(), state.cancels.clone(), app, run_id);
    Ok(())
}

// ---------------- Worker ----------------

fn setting_i64(conn: &rusqlite::Connection, key: &str) -> Option<i64> {
    conn.query_row("SELECT value FROM settings WHERE key=?1", params![key], |r| {
        r.get::<_, String>(0)
    })
    .ok()
    .and_then(|s| s.trim().parse().ok())
}

/// A run's completion stats, stored as JSON in vi_runs.stats and shown as a bento.
/// "new" counts use created_at >= started_at (one run at a time in practice).
fn run_final_stats(pool: &DbPool, run_id: i64) -> String {
    let Ok(conn) = pool.get() else {
        return "{}".into();
    };
    let one = |sql: &str, p: &[&dyn rusqlite::ToSql]| -> i64 {
        conn.query_row(sql, p, |r| r.get(0)).unwrap_or(0)
    };
    let (started, rtype): (Option<String>, String) = conn
        .query_row(
            "SELECT started_at, type FROM vi_runs WHERE id=?1",
            params![run_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap_or((None, String::new()));
    let st: &str = started.as_deref().unwrap_or("");
    let duration = one(
        "SELECT CAST((julianday('now') - julianday(?1)) * 86400 AS INTEGER)",
        &[&st],
    )
    .max(0);
    json!({
        "type": rtype,
        "duration_secs": duration,
        "tasks_total": one("SELECT COUNT(*) FROM vi_tasks WHERE run_id=?1", &[&run_id]),
        "tasks_done": one("SELECT COUNT(*) FROM vi_tasks WHERE run_id=?1 AND statut='termine'", &[&run_id]),
        "tasks_echec": one("SELECT COUNT(*) FROM vi_tasks WHERE run_id=?1 AND statut='echec'", &[&run_id]),
        "venues_total": one("SELECT COUNT(*) FROM vi_venues", &[]),
        "venues_new": one("SELECT COUNT(*) FROM vi_venues WHERE created_at >= ?1", &[&st]),
        "evidence_total": one("SELECT COUNT(*) FROM vi_evidence", &[]),
        "evidence_new": one("SELECT COUNT(*) FROM vi_evidence WHERE created_at >= ?1", &[&st]),
        "qualified": one("SELECT COUNT(*) FROM vi_venues WHERE statut='qualifie'", &[]),
        "emails_total": one("SELECT COUNT(*) FROM vi_contacts WHERE type='email'", &[]),
        "venues_with_email": one("SELECT COUNT(DISTINCT venue_id) FROM vi_contacts WHERE type='email'", &[]),
    })
    .to_string()
}

/// Human-readable "what am I doing right now" line for the current task, shown
/// live in the UI (area + window for harvest, venue name for enrichment).
fn describe_task(pool: &DbPool, task_type: &str, payload: &str) -> ProgressMsg {
    let v: serde_json::Value = serde_json::from_str(payload).unwrap_or(serde_json::Value::Null);
    if task_type == "radar_harvest" {
        let (genre, source) = (v["genre"].as_str().unwrap_or("?"), v["source"].as_str().unwrap_or("?"));
        return ProgressMsg::new(
            "radar_harvest",
            json!({ "genre": genre, "source": source }),
            format!("Radar: {} on {}", genre, source),
        );
    }
    if task_type == "radar_enrich" {
        let id = v["curator_id"].as_i64().unwrap_or(0);
        let nom: Option<String> = pool.get().ok().and_then(|c| {
            c.query_row("SELECT nom FROM radar_curators WHERE id=?1", params![id], |r| r.get(0))
                .ok()
        });
        let name = nom.unwrap_or_else(|| format!("#{}", id));
        return ProgressMsg::new("radar_enrich", json!({ "name": name }), format!("Email search: {}", name));
    }
    if task_type == "enrich_venue" {
        let vid = v["venue_id"].as_i64().unwrap_or(0);
        let nom: Option<String> = pool.get().ok().and_then(|c| {
            c.query_row("SELECT nom FROM vi_venues WHERE id=?1", params![vid], |r| r.get(0))
                .ok()
        });
        let name = nom.unwrap_or_else(|| format!("#{}", vid));
        let retry = v["retry"].as_bool().unwrap_or(false);
        ProgressMsg::new(
            if retry { "enrich_retry" } else { "enrich" },
            json!({ "name": name }),
            format!("Enrichment: {}", name),
        )
    } else {
        let area = v["area_name"].as_str().unwrap_or("?");
        let gte = v["gte"].as_str().unwrap_or("").get(0..7).unwrap_or("");
        let lte = v["lte"].as_str().unwrap_or("").get(0..7).unwrap_or("");
        ProgressMsg::new(
            "harvest",
            json!({ "area": area, "from": gte, "to": lte }),
            format!("Harvest: {} ({} → {})", area, gte, lte),
        )
    }
}

/// One worker: drains pending tasks for the run until empty or the run is stopped.
/// The claim is a single atomic UPDATE ... RETURNING, so several workers never
/// grab the same task (SQLite serialises the writes; WAL + busy_timeout handle
/// contention).
async fn drain(pool: DbPool, cancels: CancelSet, app: AppHandle, run_id: i64, politeness: u64) {
    let client = ra::client();
    loop {
        if cancels.lock().unwrap_or_else(|e| e.into_inner()).contains(&run_id) {
            break;
        }
        // Never unwrap a starved pool: back off and retry instead of panicking.
        let conn = match pool.get() {
            Ok(c) => c,
            Err(_) => {
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                continue;
            }
        };
        let task: rusqlite::Result<(i64, String, String, i64, i64)> = conn
            .query_row(
                "UPDATE vi_tasks SET statut='en_cours', locked_at=datetime('now'), updated_at=datetime('now')
                 WHERE id = (SELECT id FROM vi_tasks WHERE run_id=?1 AND statut='en_attente' ORDER BY id LIMIT 1)
                 RETURNING id, type, payload, tentatives, max_tentatives",
                params![run_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            );
        drop(conn);
        let (task_id, task_type, payload, tentatives, max_tentatives) = match task {
            Ok(t) => t,
            Err(rusqlite::Error::QueryReturnedNoRows) => break, // no more pending tasks
            // SQLITE_BUSY under write contention is not "queue empty": retry.
            Err(_) => {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                continue;
            }
        };

        // Tell the UI exactly what this worker is doing right now.
        emit(&app, &pool, run_id, Some(describe_task(&pool, &task_type, &payload)));

        let result = match task_type.as_str() {
            "enrich_venue" => {
                crate::commands::enrich::process_enrich_task(&client, &pool, &payload).await
            }
            "radar_harvest" => crate::commands::radar::process_harvest_task(&pool, &payload).await,
            "radar_enrich" => crate::commands::radar::process_enrich_task(&pool, &payload).await,
            _ => process_harvest_task(&client, &pool, &payload).await,
        };
        match result {
            Ok(()) => {
                if let Ok(conn) = pool.get() {
                    let _ = conn.execute(
                        "UPDATE vi_tasks SET statut='termine', locked_at=NULL, updated_at=datetime('now') WHERE id=?1",
                        params![task_id],
                    );
                }
            }
            Err(e) => {
                let attempts = tentatives + 1;
                if let Ok(conn) = pool.get() {
                    if !e.retryable || attempts >= max_tentatives {
                        let _ = conn.execute(
                            "UPDATE vi_tasks SET statut='echec', tentatives=?2, derniere_erreur=?3, locked_at=NULL, updated_at=datetime('now') WHERE id=?1",
                            params![task_id, attempts, e.message],
                        );
                    } else {
                        let _ = conn.execute(
                            "UPDATE vi_tasks SET statut='en_attente', tentatives=?2, derniere_erreur=?3, locked_at=NULL, updated_at=datetime('now') WHERE id=?1",
                            params![task_id, attempts, e.message],
                        );
                    }
                }
                if e.retryable && attempts < max_tentatives {
                    let backoff = 1500u64 * (1 << attempts.min(4)) as u64;
                    tokio::time::sleep(std::time::Duration::from_millis(backoff)).await;
                }
            }
        }

        emit(&app, &pool, run_id, None);
        tokio::time::sleep(std::time::Duration::from_millis(politeness)).await;
    }
}

/// Spawn the run: N concurrent workers (setting `vi_workers`, default 3) drain the
/// task queue, then a single finalisation marks the run and stores its stats.
pub fn spawn_worker(pool: DbPool, cancels: CancelSet, app: AppHandle, run_id: i64) {
    if !ACTIVE_RUNS.lock().unwrap_or_else(|e| e.into_inner()).insert(run_id) {
        return; // workers already draining this run
    }
    tauri::async_runtime::spawn(async move {
        let _active = ActiveRunGuard(run_id);
        emit(&app, &pool, run_id, Some(ProgressMsg::new("starting", json!({}), "Starting".into())));
        let (workers, politeness) = match pool.get() {
            Ok(conn) => (
                setting_i64(&conn, "vi_workers").unwrap_or(3).clamp(1, 6),
                setting_i64(&conn, "vi_politeness_ms").unwrap_or(1000).clamp(200, 5000) as u64,
            ),
            Err(_) => (3, 1000),
        };

        let spawn_drains = || {
            let mut handles = Vec::new();
            for _ in 0..workers {
                let (pool, cancels, app) = (pool.clone(), cancels.clone(), app.clone());
                handles.push(tauri::async_runtime::spawn(drain(
                    pool, cancels, app, run_id, politeness,
                )));
            }
            handles
        };

        for h in spawn_drains() {
            let _ = h.await;
        }

        // A Radar scan chains straight into the email hunt: a curator with no
        // address is not actionable, so the second phase is queued into the same
        // run and drained by a second pass rather than left to a separate button.
        let is_radar_scan = pool
            .get()
            .ok()
            .and_then(|c| {
                c.query_row("SELECT type FROM vi_runs WHERE id=?1", params![run_id], |r| {
                    r.get::<_, String>(0)
                })
                .ok()
            })
            .as_deref()
            == Some("radar_harvest");
        let stopped = || cancels.lock().unwrap_or_else(|e| e.into_inner()).contains(&run_id);
        if is_radar_scan && !stopped() {
            let queued = crate::commands::radar::queue_enrich_tasks(&pool, run_id);
            if queued > 0 {
                emit(
                    &app,
                    &pool,
                    run_id,
                    Some(ProgressMsg::new("email_hunt", json!({ "count": queued }), format!("Email search ({queued})"))),
                );
                for h in spawn_drains() {
                    let _ = h.await;
                }
            }
        }

        let cancelled = cancels.lock().unwrap_or_else(|e| e.into_inner()).remove(&run_id);

        qualify(&pool);
        // Finalisation must not panic on a starved pool, or the run would stay
        // 'en_cours' forever: wait for a connection instead.
        let conn = loop {
            match pool.get() {
                Ok(c) => break c,
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(500)).await,
            }
        };
        let remaining: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM vi_tasks WHERE run_id=?1 AND statut IN ('en_attente','en_cours')",
                params![run_id],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let statut = if cancelled || remaining > 0 { "arrete" } else { "termine" };
        let stats = run_final_stats(&pool, run_id);
        let _ = conn.execute(
            "UPDATE vi_runs SET statut=?2, finished_at=datetime('now'), stats=?3 WHERE id=?1",
            params![run_id, statut, stats],
        );
        drop(conn);
        emit(
            &app,
            &pool,
            run_id,
            Some(if cancelled {
                ProgressMsg::new("stopped", json!({}), "Stopped".into())
            } else {
                ProgressMsg::new("finished", json!({}), "Finished".into())
            }),
        );
    });
}

#[derive(Serialize)]
pub struct TaskRow {
    pub id: i64,
    #[serde(rename = "type")]
    pub type_: String,
    pub statut: String,
    pub tentatives: i64,
    pub erreur: Option<String>,
    pub payload: Option<String>,
}

/// Task-level log for a run (failed tasks by default), so the UI can explain
/// exactly what went wrong on a run.
#[tauri::command(async)]
pub fn vi_run_tasks(
    state: State<'_, AppState>,
    run_id: i64,
    only_failed: Option<bool>,
) -> Result<Vec<TaskRow>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let sql = if only_failed.unwrap_or(true) {
        "SELECT id, type, statut, tentatives, derniere_erreur, payload FROM vi_tasks
         WHERE run_id=?1 AND statut='echec' ORDER BY id LIMIT 300"
    } else {
        "SELECT id, type, statut, tentatives, derniere_erreur, payload FROM vi_tasks
         WHERE run_id=?1 ORDER BY id LIMIT 500"
    };
    let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![run_id], |r| {
            Ok(TaskRow {
                id: r.get(0)?,
                type_: r.get(1)?,
                statut: r.get(2)?,
                tentatives: r.get(3)?,
                erreur: r.get(4)?,
                payload: r.get(5)?,
            })
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// Ask a running run to stop. Workers see the flag on their next loop and exit;
/// the run is marked 'arrete' and stays resumable.
#[tauri::command(async)]
pub fn vi_stop_run(state: State<'_, AppState>, run_id: i64) -> Result<(), String> {
    state.cancels.lock().unwrap_or_else(|e| e.into_inner()).insert(run_id);
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let _ = conn.execute(
        "UPDATE vi_runs SET statut='arrete' WHERE id=?1 AND statut='en_cours'",
        params![run_id],
    );
    Ok(())
}

async fn process_harvest_task(
    client: &reqwest::Client,
    pool: &DbPool,
    payload: &str,
) -> Result<(), ra::RaError> {
    let v: serde_json::Value = serde_json::from_str(payload)
        .map_err(|e| ra::RaError { message: e.to_string(), status: None, retryable: false })?;
    let area_id = v["ra_area_id"].as_i64().unwrap_or(0);
    let gte = v["gte"].as_str().unwrap_or("").to_string();
    let lte = v["lte"].as_str().unwrap_or("").to_string();
    let region = v["area_name"].as_str().unwrap_or("").to_string();

    let mut page = 1i64;
    loop {
        let listings = ra::fetch_listings(client, area_id, &gte, &lte, page, PAGE_SIZE).await?;
        let n = listings.data.len() as i64;
        {
            let conn = pool.get().unwrap();
            for listing in &listings.data {
                let Some(ev) = &listing.event else { continue };
                let Some(venue) = &ev.venue else { continue };
                let ra_venue_id: i64 = match venue.id.parse() {
                    Ok(x) => x,
                    Err(_) => continue,
                };
                // Excluded from the fiche: never re-added, never re-counted.
                if crate::commands::vi::is_excluded(&conn, &crate::commands::vi::venue_exclusion_key(ra_venue_id)) {
                    continue;
                }
                // "TBA", "Secret Location (Paris)"...: a label shared by every
                // unannounced party in the city, not a room to pitch.
                if crate::commands::vi::is_placeholder_venue(
                    &venue.name,
                    venue.area.as_ref().and_then(|a| a.name.as_deref()),
                ) {
                    continue;
                }
                // "TBA - Club Cello" is stored as "Club Cello".
                let display_name =
                    crate::commands::vi::strip_tba_prefix(&venue.name).unwrap_or_else(|| venue.name.clone());
                let norm = normalise(&display_name);
                let slug = format!("{}-{}", norm, ra_venue_id);
                let ville = venue.area.as_ref().and_then(|a| a.name.clone());
                // Accent-folded copy of the city: SQLite has no unaccent, and the
                // artist-targeting sort compares against it (see vi_list_venues).
                let ville_norm = ville.as_deref().map(normalise);
                let pays = venue
                    .area
                    .as_ref()
                    .and_then(|a| a.country.as_ref())
                    .and_then(|c| c.url_code.clone());
                let ra_url = venue
                    .content_url
                    .as_ref()
                    .map(|u| format!("https://ra.co{}", u));

                // No `nb_events_periode + 1` here any more: re-harvesting an
                // overlapping window counted the same events again (most areas
                // were harvested 2 to 4 times, see the v1 dedup migration). The
                // count is derived from vi_venue_events below instead.
                let _ = conn.execute(
                    "INSERT INTO vi_venues (slug, nom, nom_normalise, ville, ville_norm, pays, region_cible, ra_venue_id, ra_url, nb_events_periode)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,0)
                     ON CONFLICT(ra_venue_id) DO UPDATE SET
                        ville = COALESCE(vi_venues.ville, excluded.ville),
                        ville_norm = COALESCE(vi_venues.ville_norm, excluded.ville_norm),
                        pays = COALESCE(vi_venues.pays, excluded.pays),
                        region_cible = COALESCE(vi_venues.region_cible, excluded.region_cible),
                        ra_url = COALESCE(vi_venues.ra_url, excluded.ra_url),
                        updated_at = datetime('now')",
                    params![slug, display_name, norm, ville, ville_norm, pays, region, ra_venue_id, ra_url],
                );
                let venue_id: i64 = match conn.query_row(
                    "SELECT id FROM vi_venues WHERE ra_venue_id=?1",
                    params![ra_venue_id],
                    |r| r.get(0),
                ) {
                    Ok(x) => x,
                    Err(_) => continue,
                };

                let date_event = ev.date.as_deref().unwrap_or("").chars().take(10).collect::<String>();

                // Each RA event is counted once, however many runs see it.
                let new_event = match ev.id.parse::<i64>() {
                    Ok(eid) => conn
                        .execute(
                            "INSERT OR IGNORE INTO vi_venue_events (ra_event_id, venue_id, date_event) VALUES (?1,?2,?3)",
                            params![eid, venue_id, date_event],
                        )
                        .map(|n| n > 0)
                        .unwrap_or(false),
                    Err(_) => false,
                };
                if new_event {
                    let _ = conn.execute(
                        "UPDATE vi_venues SET nb_events_periode = MAX(nb_events_legacy,
                            (SELECT COUNT(*) FROM vi_venue_events e WHERE e.venue_id = ?1))
                         WHERE id = ?1",
                        params![venue_id],
                    );
                }
                let source_url = ev
                    .content_url
                    .as_ref()
                    .map(|u| format!("https://ra.co{}", u))
                    .or_else(|| ra_url.clone())
                    .unwrap_or_default();

                // Evidence: exact match of normalised artist name against the active reference list.
                for artist in &ev.artists {
                    let an = normalise(&artist.name);
                    if an.is_empty() {
                        continue;
                    }
                    let tier: Option<i64> = conn
                        .query_row(
                            "SELECT tier FROM vi_reference_artists WHERE nom_normalise=?1 AND actif=1",
                            params![an],
                            |r| r.get(0),
                        )
                        .ok();
                    if let Some(tier) = tier {
                        if !date_event.is_empty() && !source_url.is_empty() {
                            let _ = conn.execute(
                                "INSERT OR IGNORE INTO vi_evidence (venue_id, artiste, artiste_tier, date_event, titre_event, source_url, source_type)
                                 VALUES (?1,?2,?3,?4,?5,?6,'ra')",
                                params![venue_id, artist.name, tier, date_event, ev.title, source_url],
                            );
                        }
                    }
                }

                // Promoters aggregated per venue, counted once per event like the
                // venue itself. Their RA page is kept so the email retry can reach
                // the promoter's own site.
                for promo in &ev.promoters {
                    if promo.name.trim().is_empty() {
                        continue;
                    }
                    let purl = promo.id.as_deref().and_then(ra::promoter_url);
                    if new_event {
                        let _ = conn.execute(
                            "INSERT INTO vi_promoters (venue_id, nom, nb_events, ra_url) VALUES (?1,?2,1,?3)
                             ON CONFLICT(venue_id, nom) DO UPDATE SET nb_events = nb_events + 1,
                                ra_url = COALESCE(vi_promoters.ra_url, excluded.ra_url)",
                            params![venue_id, promo.name, purl],
                        );
                    } else if purl.is_some() {
                        let _ = conn.execute(
                            "UPDATE vi_promoters SET ra_url = COALESCE(ra_url, ?3) WHERE venue_id=?1 AND nom=?2",
                            params![venue_id, promo.name, purl],
                        );
                    }
                }
            }
        }

        if n < PAGE_SIZE {
            break; // last (incomplete) page
        }
        page += 1;
        tokio::time::sleep(std::time::Duration::from_millis(POLITENESS_MS)).await;
    }
    Ok(())
}

/// Compute score_qualif and statut for all non-decided venues.
fn qualify(pool: &DbPool) {
    let conn = pool.get().unwrap();
    let _ = conn.execute(
        "UPDATE vi_venues SET
            score_qualif = MIN(100,
                COALESCE((SELECT SUM(CASE artiste_tier WHEN 1 THEN 10 WHEN 2 THEN 6 WHEN 3 THEN 4 ELSE 4 END)
                          FROM vi_evidence e WHERE e.venue_id = vi_venues.id), 0)
                + CASE WHEN (SELECT COUNT(DISTINCT artiste) FROM vi_evidence e WHERE e.venue_id = vi_venues.id) >= 3
                       THEN 10 ELSE 0 END),
            statut = CASE
                WHEN EXISTS(SELECT 1 FROM vi_evidence e WHERE e.venue_id = vi_venues.id) THEN 'qualifie'
                WHEN nb_events_periode >= 12 THEN 'qualifie'
                -- Once qualified, never demoted automatically: the event-count
                -- dedup lowered inflated counts and would otherwise reject ~600
                -- venues on an estimate.
                WHEN statut = 'qualifie' THEN 'qualifie'
                ELSE 'rejete' END,
            updated_at = datetime('now')
         WHERE statut NOT IN ('valide','archive')",
        [],
    );
}

// ---------------- Read side (runs + venues) ----------------

#[derive(Serialize)]
pub struct RunRow {
    pub id: i64,
    pub type_: String,
    pub statut: String,
    pub params: Option<String>,
    pub stats: Option<String>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub erreur: Option<String>,
    pub created_at: Option<String>,
    pub tasks_total: i64,
    pub tasks_done: i64,
    pub tasks_echec: i64,
}

#[tauri::command(async)]
pub fn vi_list_runs(state: State<'_, AppState>) -> Result<Vec<RunRow>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT r.id, r.type, r.statut, r.params, r.stats, r.started_at, r.finished_at, r.erreur, r.created_at,
                    (SELECT COUNT(*) FROM vi_tasks t WHERE t.run_id=r.id),
                    (SELECT COUNT(*) FROM vi_tasks t WHERE t.run_id=r.id AND t.statut='termine'),
                    (SELECT COUNT(*) FROM vi_tasks t WHERE t.run_id=r.id AND t.statut='echec')
             FROM vi_runs r ORDER BY r.id DESC LIMIT 100",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(RunRow {
                id: r.get(0)?,
                type_: r.get(1)?,
                statut: r.get(2)?,
                params: r.get(3)?,
                stats: r.get(4)?,
                started_at: r.get(5)?,
                finished_at: r.get(6)?,
                erreur: r.get(7)?,
                created_at: r.get(8)?,
                tasks_total: r.get(9)?,
                tasks_done: r.get(10)?,
                tasks_echec: r.get(11)?,
            })
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// An address on this many distinct venues or more came off a ticketing or
/// listing page (Skiddle, Xceed, Fever...), not off the venue's own site: in the
/// real data custcare@skiddle.com sat on 136 venues. Four, not radar's three, so
/// a promoter group sharing one booking inbox across a few rooms still counts.
const VENUE_SHARED_EMAIL_MIN: i64 = 4;

/// SQL boolean over `col`: true when the address plausibly belongs to the venue
/// itself. Every place that reads a venue email (filter, list, counters,
/// promotion, repair) goes through this, so they all agree.
pub(crate) fn own_email_sql(col: &str) -> String {
    use super::radar::{NON_PITCH_LOCALS, PLACEHOLDER_SQL};
    // One IN lookup on the local part instead of one LIKE per mailbox name:
    // this runs for every address of every venue the email filter walks, and
    // the 20 LIKEs were a third of the filtered list's time.
    let locals = NON_PITCH_LOCALS.iter().map(|l| format!("'{l}'")).collect::<Vec<_>>().join(",");
    let mut parts: Vec<String> = vec![format!(
        "substr(lower({col}), 1, instr({col}, '@') - 1) IN ({locals})"
    )];
    parts.extend(PLACEHOLDER_SQL.iter().map(|p| format!("lower({col}) LIKE {p}")));
    // Scraping artefact: a JSON-escaped string left a trailing backslash.
    parts.push(format!("{col} LIKE '%\\'"));
    // Uncorrelated, so SQLite builds this set once per statement.
    parts.push(format!(
        "lower({col}) IN (SELECT lower(valeur) FROM vi_contacts WHERE type='email'
           GROUP BY lower(valeur) HAVING COUNT(DISTINCT venue_id) >= {VENUE_SHARED_EMAIL_MIN})"
    ));
    format!("NOT ({})", parts.join(" OR "))
}

// A venue is "contacted" when its CRM contact (or a contact matching it by
// venue/name) has actually been engaged: an email went out, a first contact is
// dated, or the status moved past "to contact". Merely being promoted into the
// pipeline is not contact; that is what crm_contact_id / "in pipeline" shows. "played" when one of our
// events happened there, or a matching booking is confirmed. Name-matched
// because RA venues aren't hard-linked to the pipeline yet (that is J6).
const CONTACTED_SQL: &str = "(EXISTS(SELECT 1 FROM contacts c WHERE c.id=v.crm_contact_id
      AND (c.first_contact IS NOT NULL OR c.status IN ('contacted','followed_up','in_discussion','confirmed','declined')))
    OR EXISTS(SELECT 1 FROM emails m WHERE m.contact_id=v.crm_contact_id AND m.status='sent')
    OR EXISTS(
    SELECT 1 FROM contacts c
    WHERE (lower(trim(c.venue))=lower(trim(v.nom)) OR lower(trim(c.name))=lower(trim(v.nom)))
      AND (c.first_contact IS NOT NULL OR c.status IN ('contacted','followed_up','in_discussion','confirmed','declined'))))";
const PLAYED_SQL: &str = "(EXISTS(SELECT 1 FROM events e WHERE lower(trim(e.venue))=lower(trim(v.nom)))
    OR EXISTS(SELECT 1 FROM contacts c
        WHERE (lower(trim(c.venue))=lower(trim(v.nom)) OR lower(trim(c.name))=lower(trim(v.nom)))
          AND c.status='confirmed'))";

#[derive(Serialize)]
pub struct VenueRow {
    pub id: i64,
    pub nom: String,
    pub ville: Option<String>,
    pub pays: Option<String>,
    pub region_cible: Option<String>,
    pub statut: String,
    pub priorite: String,
    pub score_qualif: i64,
    pub nb_events_periode: i64,
    pub nb_evidence: i64,
    pub top_promoter: Option<String>,
    pub ra_url: Option<String>,
    pub site_web: Option<String>,
    pub telephone: Option<String>,
    pub best_email: Option<String>,
    pub nb_emails: i64,
    pub enriched: bool,
    pub crm_contact_id: Option<i64>,
    pub contacted: bool,
    pub played: bool,
    /// The venue's city is one of the targeted artist's audience cities.
    pub is_target: bool,
}

/// Normalised, de-duplicated audience cities (same folding as vi_venues.ville_norm).
fn norm_cities(cities: Option<Vec<String>>) -> Vec<String> {
    let mut out: Vec<String> = cities
        .unwrap_or_default()
        .iter()
        .map(|c| normalise(c))
        .filter(|c| c.len() >= 2)
        .collect();
    out.sort();
    out.dedup();
    out
}

/// SQL boolean "this venue is in one of `cities`" over the accent-folded
/// vi_venues.ville_norm, plus its bind values. Containment both ways, as the
/// page did client-side: "London" matches "Greater London" and the reverse.
/// None when no city is targeted.
fn target_sql(cities: &[String]) -> Option<(String, Vec<Box<dyn rusqlite::ToSql>>)> {
    if cities.is_empty() {
        return None;
    }
    let mut parts = Vec::new();
    let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    for c in cities {
        parts.push("instr(v.ville_norm, ?) > 0 OR instr(?, v.ville_norm) > 0");
        args.push(Box::new(c.clone()));
        args.push(Box::new(c.clone()));
    }
    Some((
        format!("(COALESCE(v.ville_norm,'') != '' AND ({}))", parts.join(" OR ")),
        args,
    ))
}

/// The WHERE clause shared by the venue list and its count, so the number shown
/// is always the size of the list being filtered.
#[allow(clippy::too_many_arguments)]
fn venue_filters(
    statut: Option<String>,
    pays: Option<String>,
    search: Option<String>,
    has_email: Option<bool>,
    has_phone: Option<bool>,
    has_website: Option<bool>,
    contacted: Option<bool>,
    played: Option<bool>,
    only_in: &[String],
) -> (String, Vec<Box<dyn rusqlite::ToSql>>) {
    let mut filters = String::new();
    let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    // Restrict to the targeted cities (the "N venues in target cities" count).
    if let Some((cond, a)) = target_sql(only_in) {
        filters.push_str(&format!(" AND {cond}"));
        args.extend(a);
    }
    // "actif" = everything still in play (not rejected). An IN list keeps the
    // statut index usable, unlike a != test.
    if statut.as_deref() == Some("actif") {
        filters.push_str(" AND v.statut IN ('candidat','qualifie','valide')");
    } else if let Some(s) = statut.filter(|s| !s.is_empty() && s != "all") {
        filters.push_str(" AND v.statut=?");
        args.push(Box::new(s));
    }
    if let Some(p) = pays.filter(|s| !s.is_empty() && s != "all") {
        filters.push_str(" AND v.pays=?");
        args.push(Box::new(p));
    }
    if let Some(q) = search.filter(|s| !s.trim().is_empty()) {
        filters.push_str(" AND (v.nom LIKE ? OR v.ville LIKE ? OR v.adresse LIKE ?)");
        let like = format!("%{}%", q.trim());
        args.push(Box::new(like.clone()));
        args.push(Box::new(like.clone()));
        args.push(Box::new(like));
    }
    if has_email == Some(true) {
        filters.push_str(&format!(
            " AND EXISTS(SELECT 1 FROM vi_contacts c WHERE c.venue_id=v.id AND c.type='email' AND {})",
            own_email_sql("c.valeur")
        ));
    }
    if has_phone == Some(true) {
        filters.push_str(
            " AND ((v.telephone IS NOT NULL AND trim(v.telephone) != '')
                   OR EXISTS(SELECT 1 FROM vi_contacts c WHERE c.venue_id=v.id AND c.type='telephone'))",
        );
    }
    if has_website == Some(true) {
        filters.push_str(" AND v.site_web IS NOT NULL AND trim(v.site_web) != ''");
    }
    if contacted == Some(true) {
        filters.push_str(&format!(" AND {}", CONTACTED_SQL));
    }
    if played == Some(true) {
        filters.push_str(&format!(" AND {}", PLAYED_SQL));
    }

    (filters, args)
}

#[tauri::command(async)]
#[allow(clippy::too_many_arguments)]
pub fn vi_count_venues(
    state: State<'_, AppState>,
    statut: Option<String>,
    pays: Option<String>,
    search: Option<String>,
    has_email: Option<bool>,
    has_phone: Option<bool>,
    has_website: Option<bool>,
    contacted: Option<bool>,
    played: Option<bool>,
    target_cities: Option<Vec<String>>,
    target_only: Option<bool>,
) -> Result<i64, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    count_venues(
        &conn, statut, pays, search, has_email, has_phone, has_website, contacted, played,
        target_cities, target_only,
    )
}

#[allow(clippy::too_many_arguments)]
fn count_venues(
    conn: &rusqlite::Connection,
    statut: Option<String>,
    pays: Option<String>,
    search: Option<String>,
    has_email: Option<bool>,
    has_phone: Option<bool>,
    has_website: Option<bool>,
    contacted: Option<bool>,
    played: Option<bool>,
    target_cities: Option<Vec<String>>,
    target_only: Option<bool>,
) -> Result<i64, String> {
    let only_in = if target_only == Some(true) { norm_cities(target_cities) } else { Vec::new() };
    let (filters, args) = venue_filters(
        statut, pays, search, has_email, has_phone, has_website, contacted, played, &only_in,
    );
    let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();
    conn.query_row(
        &format!("SELECT COUNT(*) FROM vi_venues v WHERE 1=1{}", filters),
        refs.as_slice(),
        |r| r.get(0),
    )
    .map_err(|e| e.to_string())
}

#[tauri::command(async)]
#[allow(clippy::too_many_arguments)]
pub fn vi_list_venues(
    state: State<'_, AppState>,
    statut: Option<String>,
    pays: Option<String>,
    search: Option<String>,
    has_email: Option<bool>,
    has_phone: Option<bool>,
    has_website: Option<bool>,
    contacted: Option<bool>,
    played: Option<bool>,
    target_cities: Option<Vec<String>>,
    target_only: Option<bool>,
    limit: Option<i64>,
    offset: Option<i64>,
) -> Result<Vec<VenueRow>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    list_venues(
        &conn, statut, pays, search, has_email, has_phone, has_website, contacted, played,
        target_cities, target_only, limit, offset,
    )
}

#[allow(clippy::too_many_arguments)]
fn list_venues(
    conn: &rusqlite::Connection,
    statut: Option<String>,
    pays: Option<String>,
    search: Option<String>,
    has_email: Option<bool>,
    has_phone: Option<bool>,
    has_website: Option<bool>,
    contacted: Option<bool>,
    played: Option<bool>,
    target_cities: Option<Vec<String>>,
    target_only: Option<bool>,
    limit: Option<i64>,
    offset: Option<i64>,
) -> Result<Vec<VenueRow>, String> {
    // Filters and paging first, decoration second. Selecting the page in an inner
    // query means the six correlated subqueries below run for the rows actually
    // returned, not for every row the paging walks past: at OFFSET 20 000 that is
    // the difference between 0.46 s and 0.06 s. The ORDER BY matches
    // idx_vi_venues_sort so the inner query neither sorts nor scans.
    let cities = norm_cities(target_cities);
    let only_in: &[String] = if target_only == Some(true) { &cities } else { &[] };
    let (filters, filter_args) = venue_filters(
        statut, pays, search, has_email, has_phone, has_website, contacted, played, only_in,
    );
    // Artist targeting sorts in SQL, so venues in the audience cities come first
    // across every page, not just within the rows already loaded. Without a
    // target the ORDER BY stays the index's and SQLite stops after one page.
    let (target_expr, target_args) = target_sql(&cities).unwrap_or_else(|| ("0".into(), Vec::new()));
    // Bind order follows the text: the target expression (select list) comes
    // before the WHERE filters.
    let mut args = target_args;
    args.extend(filter_args);

    let sql = format!(
        "SELECT v.id, v.nom, v.ville, v.pays, v.region_cible, v.statut, v.priorite, v.score_qualif,
                v.nb_events_periode,
                (SELECT COUNT(*) FROM vi_evidence e WHERE e.venue_id=v.id),
                (SELECT nom FROM vi_promoters p WHERE p.venue_id=v.id ORDER BY nb_events DESC LIMIT 1),
                v.ra_url, v.site_web, v.telephone,
                (SELECT valeur FROM vi_contacts c WHERE c.venue_id=v.id AND c.type='email' AND {own} ORDER BY c.score DESC, c.id LIMIT 1),
                (SELECT COUNT(*) FROM vi_contacts c WHERE c.venue_id=v.id AND c.type='email' AND {own}),
                (v.enriched_at IS NOT NULL),
                v.crm_contact_id,
                {contacted} AS is_contacted,
                {played} AS has_played,
                v.is_target
         FROM (SELECT v.*, {target_expr} AS is_target FROM vi_venues v WHERE 1=1{filters}
               ORDER BY {order}v.score_qualif DESC, v.nb_events_periode DESC, v.nom COLLATE NOCASE
               LIMIT {limit} OFFSET {offset}) v
         ORDER BY v.is_target DESC, v.score_qualif DESC, v.nb_events_periode DESC, v.nom COLLATE NOCASE",
        contacted = CONTACTED_SQL,
        played = PLAYED_SQL,
        own = own_email_sql("c.valeur"),
        filters = filters,
        order = if cities.is_empty() { "" } else { "is_target DESC, " },
        limit = limit.unwrap_or(200).clamp(1, 1000),
        offset = offset.unwrap_or(0).max(0),
    );

    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();
    let rows = stmt
        .query_map(refs.as_slice(), |r| {
            Ok(VenueRow {
                id: r.get(0)?,
                nom: r.get(1)?,
                ville: r.get(2)?,
                pays: r.get(3)?,
                region_cible: r.get(4)?,
                statut: r.get(5)?,
                priorite: r.get(6)?,
                score_qualif: r.get(7)?,
                nb_events_periode: r.get(8)?,
                nb_evidence: r.get(9)?,
                top_promoter: r.get(10)?,
                ra_url: r.get(11)?,
                site_web: r.get(12)?,
                telephone: r.get(13)?,
                best_email: r.get(14)?,
                nb_emails: r.get(15)?,
                enriched: r.get::<_, i64>(16)? != 0,
                crm_contact_id: r.get(17)?,
                contacted: r.get::<_, i64>(18)? != 0,
                played: r.get::<_, i64>(19)? != 0,
                is_target: r.get::<_, i64>(20)? != 0,
            })
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

// ---------------- Promote to CRM contacts ----------------

/// Promote one venue into the contacts pipeline (category 'venue') so it shows up
/// in email campaigns. Idempotent: returns the existing contact if already linked.
#[tauri::command(async)]
pub fn vi_promote_venue(state: State<'_, AppState>, id: i64) -> Result<i64, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    promote_venue_inner(&conn, id)
}

pub(crate) fn promote_venue_inner(conn: &rusqlite::Connection, id: i64) -> Result<i64, String> {
    if let Ok(Some(cid)) = conn.query_row(
        "SELECT crm_contact_id FROM vi_venues WHERE id=?1",
        params![id],
        |r| r.get::<_, Option<i64>>(0),
    ) {
        return Ok(cid);
    }
    let (nom, ville, pays, site, tel): (String, Option<String>, Option<String>, Option<String>, Option<String>) =
        conn.query_row(
            "SELECT nom, ville, pays, site_web, telephone FROM vi_venues WHERE id=?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .map_err(|e| e.to_string())?;
    let email: Option<String> = conn
        .query_row(
            &format!(
                "SELECT valeur FROM vi_contacts c WHERE c.venue_id=?1 AND c.type='email' AND {}
                 ORDER BY c.score DESC, c.id LIMIT 1",
                own_email_sql("c.valeur")
            ),
            params![id],
            |r| r.get(0),
        )
        .ok();
    conn.execute(
        "INSERT INTO contacts (category, name, venue, area, email, website, notes, contact_channel, status, reason, artist_id)
         VALUES ('venue', ?1, ?1, ?2, ?3, ?4, ?5, 'email', 'to_contact', 'Venue Intelligence', ?6)",
        params![
            nom,
            ville,
            email,
            site,
            tel.map(|t| format!("Tel: {} · {}", t, pays.unwrap_or_default())),
            crate::commands::artists::default_artist_id(conn)
        ],
    )
    .map_err(|e| e.to_string())?;
    let contact_id = conn.last_insert_rowid();
    conn.execute(
        "UPDATE vi_venues SET crm_contact_id=?2, statut='valide', updated_at=datetime('now') WHERE id=?1",
        params![id, contact_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(contact_id)
}

/// Promote every not-yet-linked qualified/validated venue (with an email by
/// default) into contacts. Returns how many were added.
#[tauri::command(async)]
pub fn vi_promote_venues_bulk(state: State<'_, AppState>, with_email_only: Option<bool>) -> Result<i64, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let mut sql = String::from(
        "SELECT id FROM vi_venues WHERE statut IN ('qualifie','valide') AND crm_contact_id IS NULL",
    );
    if with_email_only != Some(false) {
        sql.push_str(&format!(
            " AND EXISTS(SELECT 1 FROM vi_contacts c WHERE c.venue_id=vi_venues.id AND c.type='email' AND {})",
            own_email_sql("c.valeur")
        ));
    }
    let ids: Vec<i64> = {
        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let rows = stmt.query_map([], |r| r.get::<_, i64>(0)).map_err(|e| e.to_string())?;
        let mut v = Vec::new();
        for r in rows {
            if let Ok(x) = r {
                v.push(x);
            }
        }
        v
    };
    // One transaction for the whole batch: thousands of autocommits took
    // seconds and left a half-promoted list if the app closed midway.
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    let mut n = 0;
    for id in ids {
        if promote_venue_inner(&tx, id).is_ok() {
            n += 1;
        }
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(n)
}

/// Promoted venue contacts still waiting for a first email whose address is not
/// the venue's own (a ticketing desk, a tourism office...). Rows: (venue, contact).
fn stale_venue_contacts_sql() -> String {
    format!(
        "SELECT v.id, ct.id FROM vi_venues v JOIN contacts ct ON ct.id=v.crm_contact_id
         WHERE ct.status='to_contact' AND ct.first_contact IS NULL
           AND ct.email IS NOT NULL AND trim(ct.email) != ''
           AND NOT EXISTS(SELECT 1 FROM emails m WHERE m.contact_id=ct.id)
           AND NOT ({})",
        own_email_sql("ct.email")
    )
}

/// Swap the borrowed address of each stale venue contact for the venue's best own
/// address, or clear it when there is none, so a campaign can't write to the
/// same ticketing desk a hundred times. Contacts already engaged are left alone.
#[tauri::command(async)]
pub fn vi_repair_contacts(state: State<'_, AppState>) -> Result<i64, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let pairs: Vec<(i64, i64)> = {
        let mut stmt = conn.prepare(&stale_venue_contacts_sql()).map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))
            .map_err(|e| e.to_string())?;
        rows.flatten().collect()
    };
    let best_sql = format!(
        "SELECT valeur FROM vi_contacts c WHERE c.venue_id=?1 AND c.type='email' AND {}
         ORDER BY c.score DESC, c.id LIMIT 1",
        own_email_sql("c.valeur")
    );
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    let mut n = 0;
    for (venue_id, contact_id) in pairs {
        let best: Option<String> = tx.query_row(&best_sql, params![venue_id], |r| r.get(0)).ok();
        if tx
            .execute(
                "UPDATE contacts SET email=?2, updated_at=datetime('now') WHERE id=?1",
                params![contact_id, best],
            )
            .is_ok()
        {
            n += 1;
        }
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(n)
}

// ---------------- Venue dashboard stats ----------------

#[derive(Serialize)]
pub struct CountryCount {
    pub pays: String,
    pub n: i64,
}

#[derive(Serialize)]
pub struct VenueStats {
    pub total: i64,
    pub qualifie: i64,
    pub valide: i64,
    pub candidat: i64,
    pub rejete: i64,
    pub enriched: i64,
    pub with_email: i64,
    pub contacted: i64,
    pub played: i64,
    pub countries: i64,
    /// Venues the "add all to pipeline" button would promote right now.
    pub promotable: i64,
    /// Promoted venue contacts, never written to, whose email is not the venue's own.
    pub stale_contacts: i64,
    pub top_countries: Vec<CountryCount>,
}

#[tauri::command(async)]
pub fn vi_venue_stats(state: State<'_, AppState>) -> Result<VenueStats, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let one = |sql: &str| -> i64 { conn.query_row(sql, [], |r| r.get(0)).unwrap_or(0) };
    let by_statut = |s: &str| -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM vi_venues WHERE statut=?1",
            params![s],
            |r| r.get(0),
        )
        .unwrap_or(0)
    };

    let mut top_countries = Vec::new();
    if let Ok(mut stmt) = conn.prepare(
        "SELECT COALESCE(pays,'?') AS p, COUNT(*) n FROM vi_venues GROUP BY p ORDER BY n DESC",
    ) {
        if let Ok(rows) = stmt.query_map([], |r| {
            Ok(CountryCount { pays: r.get(0)?, n: r.get(1)? })
        }) {
            for r in rows.flatten() {
                top_countries.push(r);
            }
        }
    }

    Ok(VenueStats {
        total: one("SELECT COUNT(*) FROM vi_venues"),
        qualifie: by_statut("qualifie"),
        valide: by_statut("valide"),
        candidat: by_statut("candidat"),
        rejete: by_statut("rejete"),
        enriched: one("SELECT COUNT(*) FROM vi_venues WHERE enriched_at IS NOT NULL"),
        with_email: one(&format!(
            "SELECT COUNT(DISTINCT venue_id) FROM vi_contacts c WHERE c.type='email' AND {}",
            own_email_sql("c.valeur")
        )),
        contacted: one(&format!("SELECT COUNT(*) FROM vi_venues v WHERE {}", CONTACTED_SQL)),
        played: one(&format!("SELECT COUNT(*) FROM vi_venues v WHERE {}", PLAYED_SQL)),
        countries: one("SELECT COUNT(DISTINCT pays) FROM vi_venues WHERE pays IS NOT NULL"),
        promotable: one(&format!(
            "SELECT COUNT(*) FROM vi_venues v WHERE statut IN ('qualifie','valide') AND crm_contact_id IS NULL
               AND EXISTS(SELECT 1 FROM vi_contacts c WHERE c.venue_id=v.id AND c.type='email' AND {})",
            own_email_sql("c.valeur")
        )),
        stale_contacts: one(&format!("SELECT COUNT(*) FROM ({})", stale_venue_contacts_sql())),
        top_countries,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_cities_fold_accents_and_match_both_ways() {
        let cities = norm_cities(Some(vec!["Zürich".into(), " Paris ".into(), "".into(), "paris".into()]));
        assert_eq!(cities, vec!["paris", "zurich"]);
        let (cond, args) = target_sql(&cities).unwrap();
        assert_eq!(args.len(), 4);
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE vi_venues (id INTEGER PRIMARY KEY, ville_norm TEXT);
             INSERT INTO vi_venues VALUES (1,'zurich'),(2,'paris'),(3,'berlin'),(4,''),(5,NULL),(6,'greaterparis');",
        )
        .unwrap();
        let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();
        let mut st = conn
            .prepare(&format!("SELECT id FROM vi_venues v WHERE {cond} ORDER BY id"))
            .unwrap();
        let ids: Vec<i64> = st.query_map(refs.as_slice(), |r| r.get(0)).unwrap().flatten().collect();
        assert_eq!(ids, vec![1, 2, 6]);
        assert!(target_sql(&[]).is_none());
    }

    /// Timing on a copy of the real database (never the live one):
    /// VI_MIGRATION_DB=/path/copy.db cargo test --release venue_list_perf -- --ignored --nocapture
    #[test]
    #[ignore]
    fn venue_list_perf() {
        let path = std::env::var("VI_MIGRATION_DB").expect("VI_MIGRATION_DB");
        assert!(!path.contains("Application Support"));
        let pool = crate::db::init_pool(std::path::Path::new(&path)).unwrap();
        let conn = pool.get().unwrap();
        let cities = || Some(vec!["Paris".to_string(), "Amsterdam".into(), "Berlin".into(), "Lyon".into()]);
        let s = || Some("actif".to_string());
        let time = |label: &str, f: &dyn Fn() -> usize| {
            f(); // warm
            let t = std::time::Instant::now();
            let n = f();
            println!("{label}: {n} rows in {:?}", t.elapsed());
        };
        time("list p1 no target", &|| list_venues(&conn, s(), None, None, None, None, None, None, None, None, None, Some(200), Some(0)).unwrap().len());
        time("list p1 target", &|| list_venues(&conn, s(), None, None, None, None, None, None, None, cities(), None, Some(200), Some(0)).unwrap().len());
        time("list p20 target", &|| list_venues(&conn, s(), None, None, None, None, None, None, None, cities(), None, Some(200), Some(4000)).unwrap().len());
        time("list all statuts target", &|| list_venues(&conn, Some("all".into()), None, None, None, None, None, None, None, cities(), None, Some(200), Some(0)).unwrap().len());
        time("list email no target", &|| list_venues(&conn, s(), None, None, Some(true), None, None, None, None, None, None, Some(200), Some(0)).unwrap().len());
        time("list email target", &|| list_venues(&conn, s(), None, None, Some(true), None, None, None, None, cities(), None, Some(200), Some(0)).unwrap().len());
        time("count target_only", &|| count_venues(&conn, s(), None, None, None, None, None, None, None, cities(), Some(true)).unwrap() as usize);
        time("count no target", &|| count_venues(&conn, s(), None, None, None, None, None, None, None, None, None).unwrap() as usize);
        let rows = list_venues(&conn, s(), None, None, None, None, None, None, None, cities(), None, Some(200), Some(0)).unwrap();
        let first_non = rows.iter().position(|r| !r.is_target).unwrap_or(rows.len());
        assert!(rows[first_non..].iter().all(|r| !r.is_target), "targets first");
        println!("first page: {} targets then {}", first_non, rows.len() - first_non);
    }
}
