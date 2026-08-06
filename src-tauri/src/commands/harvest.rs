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
use chrono::{Datelike, Duration, NaiveDate, Utc};
use rusqlite::params;
use serde::Serialize;
use serde_json::json;
use tauri::{AppHandle, Emitter, State};

const PAGE_SIZE: i64 = 100;
const POLITENESS_MS: u64 = 1500;
const WINDOW_DAYS: i64 = 90;
const STALE_LOCK_SECS: i64 = 300;

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
    pub message: Option<String>,
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
    }
}

fn emit(app: &AppHandle, pool: &DbPool, run_id: i64, message: Option<String>) {
    let mut p = run_stats(pool, run_id);
    p.message = message;
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
        return Err("Aucune zone résolue à moissonner. Résolvez d'abord les area ids.".into());
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
    let conn = pool.get().unwrap();
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
fn describe_task(pool: &DbPool, task_type: &str, payload: &str) -> String {
    let v: serde_json::Value = serde_json::from_str(payload).unwrap_or(serde_json::Value::Null);
    if task_type == "radar_harvest" {
        return format!("Radar : {} sur {}", v["genre"].as_str().unwrap_or("?"), v["source"].as_str().unwrap_or("?"));
    }
    if task_type == "radar_enrich" {
        return format!("Radar enrichissement : curateur #{}", v["curator_id"].as_i64().unwrap_or(0));
    }
    if task_type == "enrich_venue" {
        let vid = v["venue_id"].as_i64().unwrap_or(0);
        let nom: Option<String> = pool.get().ok().and_then(|c| {
            c.query_row("SELECT nom FROM vi_venues WHERE id=?1", params![vid], |r| r.get(0))
                .ok()
        });
        format!("Enrichissement : {}", nom.unwrap_or_else(|| format!("lieu #{}", vid)))
    } else {
        let area = v["area_name"].as_str().unwrap_or("?");
        let gte = v["gte"].as_str().unwrap_or("").get(0..7).unwrap_or("");
        let lte = v["lte"].as_str().unwrap_or("").get(0..7).unwrap_or("");
        format!("Moissonnage : {} ({} → {})", area, gte, lte)
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
        let task: Option<(i64, String, String, i64, i64)> = conn
            .query_row(
                "UPDATE vi_tasks SET statut='en_cours', locked_at=datetime('now'), updated_at=datetime('now')
                 WHERE id = (SELECT id FROM vi_tasks WHERE run_id=?1 AND statut='en_attente' ORDER BY id LIMIT 1)
                 RETURNING id, type, payload, tentatives, max_tentatives",
                params![run_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .ok();
        drop(conn);
        let Some((task_id, task_type, payload, tentatives, max_tentatives)) = task else {
            break; // no more pending tasks
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
    tauri::async_runtime::spawn(async move {
        emit(&app, &pool, run_id, Some("Démarrage".into()));
        let (workers, politeness) = {
            let conn = pool.get().unwrap();
            let w = setting_i64(&conn, "vi_workers").unwrap_or(3).clamp(1, 6);
            let p = setting_i64(&conn, "vi_politeness_ms").unwrap_or(1000).clamp(200, 5000) as u64;
            (w, p)
        };

        let mut handles = Vec::new();
        for _ in 0..workers {
            let (pool, cancels, app) = (pool.clone(), cancels.clone(), app.clone());
            handles.push(tauri::async_runtime::spawn(drain(
                pool, cancels, app, run_id, politeness,
            )));
        }
        for h in handles {
            let _ = h.await;
        }

        let cancelled = cancels.lock().unwrap_or_else(|e| e.into_inner()).remove(&run_id);

        qualify(&pool);
        let conn = pool.get().unwrap();
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
            Some(if cancelled { "Arrêté".into() } else { "Terminé".into() }),
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
#[tauri::command]
pub fn vi_run_tasks(
    state: State<AppState>,
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
#[tauri::command]
pub fn vi_stop_run(state: State<AppState>, run_id: i64) -> Result<(), String> {
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
                let norm = normalise(&venue.name);
                let slug = format!("{}-{}", norm, ra_venue_id);
                let ville = venue.area.as_ref().and_then(|a| a.name.clone());
                let pays = venue
                    .area
                    .as_ref()
                    .and_then(|a| a.country.as_ref())
                    .and_then(|c| c.url_code.clone());
                let ra_url = venue
                    .content_url
                    .as_ref()
                    .map(|u| format!("https://ra.co{}", u));

                let _ = conn.execute(
                    "INSERT INTO vi_venues (slug, nom, nom_normalise, ville, pays, region_cible, ra_venue_id, ra_url, nb_events_periode)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,1)
                     ON CONFLICT(ra_venue_id) DO UPDATE SET
                        nb_events_periode = nb_events_periode + 1,
                        ville = COALESCE(vi_venues.ville, excluded.ville),
                        pays = COALESCE(vi_venues.pays, excluded.pays),
                        region_cible = COALESCE(vi_venues.region_cible, excluded.region_cible),
                        ra_url = COALESCE(vi_venues.ra_url, excluded.ra_url),
                        updated_at = datetime('now')",
                    params![slug, venue.name, norm, ville, pays, region, ra_venue_id, ra_url],
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

                // Promoters aggregated per venue.
                for promo in &ev.promoters {
                    if promo.name.trim().is_empty() {
                        continue;
                    }
                    let _ = conn.execute(
                        "INSERT INTO vi_promoters (venue_id, nom, nb_events) VALUES (?1,?2,1)
                         ON CONFLICT(venue_id, nom) DO UPDATE SET nb_events = nb_events + 1",
                        params![venue_id, promo.name],
                    );
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

#[tauri::command]
pub fn vi_list_runs(state: State<AppState>) -> Result<Vec<RunRow>, String> {
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

// A venue is "contacted" when it's linked to a CRM contact, or a matching
// contact (by venue/name) has actually been engaged. "played" when one of our
// events happened there, or a matching booking is confirmed. Name-matched
// because RA venues aren't hard-linked to the pipeline yet (that is J6).
const CONTACTED_SQL: &str = "(v.crm_contact_id IS NOT NULL OR EXISTS(
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
}

#[tauri::command]
pub fn vi_list_venues(
    state: State<AppState>,
    statut: Option<String>,
    pays: Option<String>,
    search: Option<String>,
    has_email: Option<bool>,
    has_phone: Option<bool>,
    has_website: Option<bool>,
    contacted: Option<bool>,
    played: Option<bool>,
    limit: Option<i64>,
    offset: Option<i64>,
) -> Result<Vec<VenueRow>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let mut sql = format!(
        "SELECT v.id, v.nom, v.ville, v.pays, v.region_cible, v.statut, v.priorite, v.score_qualif,
                v.nb_events_periode,
                (SELECT COUNT(*) FROM vi_evidence e WHERE e.venue_id=v.id),
                (SELECT nom FROM vi_promoters p WHERE p.venue_id=v.id ORDER BY nb_events DESC LIMIT 1),
                v.ra_url, v.site_web, v.telephone,
                (SELECT valeur FROM vi_contacts c WHERE c.venue_id=v.id AND c.type='email' ORDER BY c.score DESC, c.id LIMIT 1),
                (SELECT COUNT(*) FROM vi_contacts c WHERE c.venue_id=v.id AND c.type='email'),
                (v.enriched_at IS NOT NULL),
                v.crm_contact_id,
                {contacted} AS is_contacted,
                {played} AS has_played
         FROM vi_venues v WHERE 1=1",
        contacted = CONTACTED_SQL,
        played = PLAYED_SQL,
    );
    let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    if let Some(s) = statut.filter(|s| !s.is_empty() && s != "all") {
        sql.push_str(" AND v.statut=?");
        args.push(Box::new(s));
    }
    if let Some(p) = pays.filter(|s| !s.is_empty() && s != "all") {
        sql.push_str(" AND v.pays=?");
        args.push(Box::new(p));
    }
    if let Some(q) = search.filter(|s| !s.trim().is_empty()) {
        sql.push_str(" AND (v.nom LIKE ? OR v.ville LIKE ? OR v.adresse LIKE ?)");
        let like = format!("%{}%", q.trim());
        args.push(Box::new(like.clone()));
        args.push(Box::new(like.clone()));
        args.push(Box::new(like));
    }
    if has_email == Some(true) {
        sql.push_str(" AND EXISTS(SELECT 1 FROM vi_contacts c WHERE c.venue_id=v.id AND c.type='email')");
    }
    if has_phone == Some(true) {
        sql.push_str(
            " AND ((v.telephone IS NOT NULL AND trim(v.telephone) != '')
                   OR EXISTS(SELECT 1 FROM vi_contacts c WHERE c.venue_id=v.id AND c.type='telephone'))",
        );
    }
    if has_website == Some(true) {
        sql.push_str(" AND v.site_web IS NOT NULL AND trim(v.site_web) != ''");
    }
    if contacted == Some(true) {
        sql.push_str(&format!(" AND {}", CONTACTED_SQL));
    }
    if played == Some(true) {
        sql.push_str(&format!(" AND {}", PLAYED_SQL));
    }
    sql.push_str(" ORDER BY v.score_qualif DESC, v.nb_events_periode DESC, v.nom COLLATE NOCASE");
    sql.push_str(&format!(" LIMIT {} OFFSET {}", limit.unwrap_or(200).clamp(1, 1000), offset.unwrap_or(0).max(0)));

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
#[tauri::command]
pub fn vi_promote_venue(state: State<AppState>, id: i64) -> Result<i64, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    promote_venue_inner(&conn, id)
}

fn promote_venue_inner(conn: &rusqlite::Connection, id: i64) -> Result<i64, String> {
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
            "SELECT valeur FROM vi_contacts WHERE venue_id=?1 AND type='email' ORDER BY score DESC LIMIT 1",
            params![id],
            |r| r.get(0),
        )
        .ok();
    conn.execute(
        "INSERT INTO contacts (category, name, venue, area, email, website, notes, contact_channel, status, reason)
         VALUES ('venue', ?1, ?1, ?2, ?3, ?4, ?5, 'email', 'to_contact', 'Venue Intelligence')",
        params![nom, ville, email, site, tel.map(|t| format!("Tel: {} · {}", t, pays.unwrap_or_default()))],
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
#[tauri::command]
pub fn vi_promote_venues_bulk(state: State<AppState>, with_email_only: Option<bool>) -> Result<i64, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let mut sql = String::from(
        "SELECT id FROM vi_venues WHERE statut IN ('qualifie','valide') AND crm_contact_id IS NULL",
    );
    if with_email_only != Some(false) {
        sql.push_str(" AND EXISTS(SELECT 1 FROM vi_contacts c WHERE c.venue_id=vi_venues.id AND c.type='email')");
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
    let mut n = 0;
    for id in ids {
        if promote_venue_inner(&conn, id).is_ok() {
            n += 1;
        }
    }
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
    pub top_countries: Vec<CountryCount>,
}

#[tauri::command]
pub fn vi_venue_stats(state: State<AppState>) -> Result<VenueStats, String> {
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
        "SELECT COALESCE(pays,'?') AS p, COUNT(*) n FROM vi_venues GROUP BY p ORDER BY n DESC LIMIT 8",
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
        with_email: one("SELECT COUNT(DISTINCT venue_id) FROM vi_contacts WHERE type='email'"),
        contacted: one(&format!("SELECT COUNT(*) FROM vi_venues v WHERE {}", CONTACTED_SQL)),
        played: one(&format!("SELECT COUNT(*) FROM vi_venues v WHERE {}", PLAYED_SQL)),
        countries: one("SELECT COUNT(DISTINCT pays) FROM vi_venues WHERE pays IS NOT NULL"),
        top_countries,
    })
}
