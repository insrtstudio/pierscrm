//! The daily pass: collect new postings, analyse them, read up on the
//! companies worth it and prepare the applications to validate. Runs once a
//! day while the app is open (and on demand); it never sends anything.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use rusqlite::{params, Connection};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use super::sources::Http;
use super::{apply, brief, cv, sources};
use crate::commands::deliverability::dns::DohResolver;
use crate::commands::email_finder::MxCheck;
use crate::commands::residency::claude;
use crate::db::DbPool;
use crate::AppState;

/// Company job boards read every day, checked to exist on 2026-11-18. The
/// user adds or removes boards in the settings of the tab.
pub const DEFAULT_BOARDS: &[(&str, &str, &str)] = &[
    ("greenhouse", "theorchard", "The Orchard"),
    ("smartrecruiters", "believe", "Believe"),
    ("ashby", "reservoir", "Reservoir Media"),
    ("lever", "spotify", "Spotify"),
    ("lever", "vevo", "Vevo"),
    ("greenhouse", "splice", "Splice"),
];

/// Postings analysed per run at most: the rest waits for the next run.
const ANALYSE_CAP: usize = 60;

static RUNNING: AtomicBool = AtomicBool::new(false);
struct RunGuard;
impl Drop for RunGuard {
    fn drop(&mut self) {
        RUNNING.store(false, Ordering::SeqCst);
    }
}

pub fn is_running() -> bool {
    RUNNING.load(Ordering::SeqCst)
}

#[derive(Serialize, Clone, Default)]
pub struct Progress {
    /// collect, analyse, company, prepare, done
    pub stage: String,
    pub label: String,
    pub done: usize,
    pub total: usize,
    pub finished: bool,
    pub found: usize,
    pub added: usize,
    pub analysed: usize,
    pub prepared: usize,
    pub notes: Vec<String>,
}

fn ids(conn: &Connection, sql: &str, limit: i64) -> Vec<i64> {
    conn.prepare(sql)
        .and_then(|mut st| st.query_map(params![limit], |r| r.get(0)).map(|rows| rows.filter_map(Result::ok).collect()))
        .unwrap_or_default()
}

/// Analysis and company brief of one posting when they are missing (a posting
/// typed by hand, or prepared before the daily pass reached it).
pub fn complete_one(conn: &Connection, id: i64) {
    let (Some(p), Some(llm), Ok(http), Ok(resolver)) = (cv::load_profile(conn), claude::client(conn), Http::new(), DohResolver::new()) else { return };
    let mut mx = MxCheck::new(&resolver);
    let mut check = |d: &str| mx.has_mx(d);
    let (analysed, briefed, company, site): (bool, bool, String, Option<String>) = conn
        .query_row(
            "SELECT analysed_at IS NOT NULL, briefed_at IS NOT NULL, company, company_site FROM job_posts WHERE id = ?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap_or((true, true, String::new(), None));
    if !analysed {
        let _ = brief::analyse(conn, &http, llm.as_ref(), &p, id, &mut check);
    }
    if !briefed && !company.trim().is_empty() {
        let key = super::setting(conn, "serper_api_key");
        let mut log = |m: &str, u: &claude::Usage| claude::log_usage(conn, "jobs_company", None, m, u);
        let finds = brief::company(&http, llm.as_ref(), key.as_deref(), &company, site.as_deref(), &mut check, &mut log);
        brief::store_company(conn, id, &finds);
    }
}

/// How many openings are looked at again per pass.
const VERIFY_CAP: i64 = 40;

/// Close what is no longer open: postings older than the age limit, then the
/// lines that matter most (a draft waiting, a good score, a line the user
/// kept), whose page is read again every two days. Returns how many closed.
pub fn verify_open(conn: &Connection, http: &Http, progress: &mut dyn FnMut(&str)) -> usize {
    let mut closed = 0;
    let old = ids(
        conn,
        &format!(
            "SELECT id FROM job_posts WHERE status = 'new' AND source NOT IN ('sheet', 'manual')
               AND posted_at IS NOT NULL AND date(posted_at) < date('now', '-{} days') LIMIT ?1",
            super::MAX_AGE_DAYS
        ),
        1000,
    );
    for id in old {
        // A company board still listing an old opening keeps it: the board is the proof.
        let on_board: bool = conn
            .query_row("SELECT source IN ('greenhouse', 'lever', 'ashby', 'smartrecruiters', 'workable', 'recruitee') FROM job_posts WHERE id = ?1", params![id], |r| r.get(0))
            .unwrap_or(false);
        if !on_board {
            super::close_post(conn, id, &format!("publiée il y a plus de {} jours", super::MAX_AGE_DAYS));
            closed += 1;
        }
    }
    let todo: Vec<(i64, String, String)> = conn
        .prepare(
            "SELECT p.id, p.url, p.title FROM job_posts p LEFT JOIN job_applications a ON a.post_id = p.id
             WHERE p.status IN ('new', 'shortlisted') AND p.url LIKE 'http%' AND p.source NOT IN ('sheet')
               AND (a.state IS NULL OR a.state = 'draft')
               AND (a.id IS NOT NULL OR p.status = 'shortlisted' OR COALESCE(p.score, 0) >= 50)
               AND (p.checked_at IS NULL OR p.checked_at < datetime('now', '-2 days'))
             ORDER BY (a.id IS NOT NULL) DESC, p.score DESC LIMIT ?1",
        )
        .and_then(|mut st| st.query_map(params![VERIFY_CAP], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).map(|rows| rows.filter_map(Result::ok).collect()))
        .unwrap_or_default();
    for (id, url, title) in todo {
        progress(&format!("Vérification · {title}"));
        match sources::gone(http, &url) {
            Some(why) => {
                super::close_post(conn, id, &why);
                closed += 1;
            }
            None => {
                let _ = conn.execute("UPDATE job_posts SET checked_at = datetime('now') WHERE id = ?1", params![id]);
            }
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    closed
}

pub fn run(app: &AppHandle, pool: &DbPool) -> Result<Progress, String> {
    if RUNNING.swap(true, Ordering::SeqCst) {
        return Err("Une recherche est déjà en cours.".into());
    }
    let _guard = RunGuard;
    let conn = pool.get().map_err(|e| e.to_string())?;
    let http = Http::new()?;
    let mut p = Progress { stage: "collect".into(), ..Default::default() };
    let _ = conn.execute("INSERT INTO job_runs DEFAULT VALUES", []);
    let run_id = conn.last_insert_rowid();
    let emit = |p: &Progress| {
        let _ = app.emit(super::EVENT, p.clone());
    };

    // 1. Collect.
    let collected = {
        let mut on = |label: &str| {
            let mut q = p.clone();
            q.label = label.to_string();
            emit(&q);
        };
        sources::collect(&conn, &http, &mut on)
    };
    p.found = collected.leads.len();
    p.notes = collected.notes;
    p.added = sources::store(&conn, &collected.leads).len();

    // 1b. Only open positions stay in the list.
    let closed = collected.closed + verify_open(&conn, &http, &mut |label: &str| {
        let mut q = p.clone();
        q.label = label.to_string();
        emit(&q);
    });
    if closed > 0 {
        p.notes.push(format!("{closed} offre(s) fermée(s) retirée(s) de la liste"));
    }

    // 2. Analyse what has not been read yet.
    let profile = cv::load_profile(&conn);
    let llm = claude::client(&conn);
    match (&profile, &llm) {
        (Some(profile), Some(llm)) => {
            let resolver = DohResolver::new()?;
            let mut mx = MxCheck::new(&resolver);
            let mut check = |d: &str| mx.has_mx(d);
            let todo = ids(&conn, "SELECT id FROM job_posts WHERE analysed_at IS NULL AND status = 'new' ORDER BY id DESC LIMIT ?1", ANALYSE_CAP as i64);
            p.stage = "analyse".into();
            p.total = todo.len();
            for (i, id) in todo.iter().enumerate() {
                p.done = i;
                emit(&p);
                match brief::analyse(&conn, &http, llm.as_ref(), profile, *id, &mut check) {
                    Ok(_) => p.analysed += 1,
                    Err(e) => {
                        let fatal = e.contains("crédit") || e.contains("Clé API");
                        if fatal || !p.notes.iter().any(|n| n.starts_with("Analyse")) {
                            p.notes.push(format!("Analyse : {e}"));
                        }
                        if fatal {
                            break;
                        }
                    }
                }
            }

            // 3. Companies and applications for the postings that fit.
            let min = super::min_score(&conn);
            let cap = super::daily_prepare(&conn);
            let sql = format!(
                "SELECT p.id FROM job_posts p LEFT JOIN job_applications a ON a.post_id = p.id
                 WHERE a.id IS NULL AND p.status IN ('new', 'shortlisted') AND p.source <> 'sheet' AND COALESCE(p.score, 0) >= {min}
                 ORDER BY p.score DESC, p.id DESC LIMIT ?1"
            );
            let best = ids(&conn, &sql, cap);
            p.stage = "prepare".into();
            p.total = best.len();
            if cv::browser_path().is_none() && !best.is_empty() {
                p.notes.push("Aucun navigateur Chromium (Chrome, Brave, Edge) : les CV ne peuvent pas être produits en PDF".into());
            }
            let dir = apply::jobs_dir(app)?;
            let key = super::setting(&conn, "serper_api_key");
            for (i, id) in best.iter().enumerate() {
                p.done = i;
                p.label = conn.query_row("SELECT company || ' · ' || title FROM job_posts WHERE id = ?1", params![id], |r| r.get(0)).unwrap_or_default();
                emit(&p);
                let (company, site, briefed): (String, Option<String>, bool) = conn
                    .query_row("SELECT company, company_site, briefed_at IS NOT NULL FROM job_posts WHERE id = ?1", params![id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                    .unwrap_or_default();
                if !briefed && !company.trim().is_empty() {
                    let mut log = |m: &str, u: &claude::Usage| claude::log_usage(&conn, "jobs_company", None, m, u);
                    let finds = brief::company(&http, llm.as_ref(), key.as_deref(), &company, site.as_deref(), &mut check, &mut log);
                    brief::store_company(&conn, *id, &finds);
                }
                match apply::prepare(&conn, &dir, Some(llm.as_ref()), *id) {
                    Ok(()) => p.prepared += 1,
                    Err(e) => p.notes.push(format!("{} : {e}", p.label)),
                }
            }
        }
        (None, _) => p.notes.push("Profil non renseigné : les annonces sont collectées mais pas analysées".into()),
        (_, None) => p.notes.push("Clé Claude non réglée : les annonces sont collectées mais pas analysées".into()),
    }

    p.stage = "done".into();
    p.finished = true;
    p.label.clear();
    let _ = conn.execute(
        "UPDATE job_runs SET finished_at = datetime('now'), found = ?2, added = ?3, analysed = ?4, prepared = ?5, notes = ?6 WHERE id = ?1",
        params![run_id, p.found as i64, p.added as i64, p.analysed as i64, p.prepared as i64, (!p.notes.is_empty()).then(|| p.notes.join(" · "))],
    );
    super::put_setting(&conn, "jobs_last_run_day", &chrono::Local::now().format("%Y-%m-%d").to_string());
    emit(&p);
    Ok(p)
}

fn notify(app: &AppHandle, p: &Progress) {
    if p.prepared == 0 && p.added == 0 {
        return;
    }
    use tauri_plugin_notification::NotificationExt;
    let body = if p.prepared > 0 {
        format!("{} candidature(s) prête(s) à valider, {} nouvelle(s) annonce(s)", p.prepared, p.added)
    } else {
        format!("{} nouvelle(s) annonce(s)", p.added)
    };
    let _ = app.notification().builder().title("Recherche d'emploi").body(&body).show();
}

/// Once a day while the app is open: first check a minute after launch, then
/// every twenty minutes, so a Mac that slept through the morning still runs.
pub fn start(pool: DbPool, app: AppHandle) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(60));
        loop {
            let due = pool
                .get()
                .map(|conn| {
                    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
                    super::setting(&conn, "jobs_enabled").as_deref() != Some("0")
                        && cv::load_profile(&conn).is_some()
                        && super::setting(&conn, "jobs_last_run_day").as_deref() != Some(today.as_str())
                })
                .unwrap_or(false);
            if due {
                if let Ok(p) = run(&app, &pool) {
                    notify(&app, &p);
                }
            }
            std::thread::sleep(Duration::from_secs(20 * 60));
        }
    });
}

/// Run the pass now (button). Progress comes as `jobs:progress` events.
#[tauri::command]
pub fn jobs_run(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    if is_running() {
        return Err("Une recherche est déjà en cours.".into());
    }
    let pool = state.pool.clone();
    std::thread::spawn(move || {
        if let Err(e) = run(&app, &pool) {
            let _ = app.emit(super::EVENT, Progress { stage: "done".into(), finished: true, notes: vec![e], ..Default::default() });
        }
    });
    Ok(())
}

#[cfg(test)]
mod live {
    use super::*;

    /// Reads the default company boards for real (no key needed):
    /// `cargo test --lib jobs::daily::live -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn boards_answer() {
        let conn = crate::commands::jobs::test_db();
        let http = Http::new().unwrap();
        let c = sources::collect(&conn, &http, &mut |_| {});
        for l in &c.leads {
            println!("{} | {} | {} | {} | {} | {} chars", l.source, l.company, l.title, l.location, l.country, l.description.len());
        }
        println!("notes: {:?}", c.notes);
        let errs: Vec<(String, String)> = conn
            .prepare("SELECT token, last_error FROM job_boards WHERE last_error IS NOT NULL")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        println!("errors: {:?}", errs);
        println!("stored: {}", sources::store(&conn, &c.leads).len());
    }
}
