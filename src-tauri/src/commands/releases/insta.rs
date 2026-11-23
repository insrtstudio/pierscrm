//! Instagram DMs of the release campaigns, sent by the app itself.
//!
//! Instagram's official messaging API cannot open a conversation, so a
//! validated DM is sent the way the user would: in a visible window of the
//! app, signed in to their own account, the page opens the profile, clicks
//! "Message", types the text and sends it (insta_dm.js). One message at a
//! time, a random pause between two, a daily cap (setting
//! `insta_dm_daily_cap`, 15 by default). Nothing is hidden or disguised: no
//! fingerprint tricks, no proxy. Automating the site is against Instagram's
//! terms of use and the account can be restricted for it; the pace and the cap
//! keep the volume at what a person does by hand.
//!
//! Only messages validated in a card are ever sent (status 'queued').

use super::{advance, Progress, EVENT};
use crate::commands::beatport::parse_handover;
use crate::AppState;
use rusqlite::{params, Connection};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

const WINDOW: &str = "instagram";
const DM_JS: &str = include_str!("insta_dm.js");
const DEFAULT_CAP: i64 = 15;

#[derive(Debug, Clone)]
struct Job {
    target_id: i64,
    card_id: i64,
    followup: bool,
    name: String,
    handle: String,
    text: String,
    /// A trial on a profile: the text is typed then erased, nothing is sent.
    dry: bool,
}

static JOB: Mutex<Option<Job>> = Mutex::new(None);
static NEED_LOGIN: AtomicBool = AtomicBool::new(false);
static PAUSED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Serialize, Default)]
pub struct Status {
    pub queued: i64,
    pub sent_today: i64,
    pub cap: i64,
    pub busy: bool,
    pub need_login: bool,
    pub paused: bool,
}

fn cap(conn: &Connection) -> i64 {
    conn.query_row("SELECT value FROM settings WHERE key = 'insta_dm_daily_cap'", [], |r| r.get::<_, String>(0))
        .ok()
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|n| (1..=40).contains(n))
        .unwrap_or(DEFAULT_CAP)
}

fn sent_today(conn: &Connection) -> i64 {
    conn.query_row(
        "SELECT COALESCE(SUM(status = 'sent' AND date(sent_at, 'localtime') = date('now', 'localtime')), 0)
              + COALESCE(SUM(fu_status = 'sent' AND date(fu_sent_at, 'localtime') = date('now', 'localtime')), 0)
         FROM release_targets WHERE channel = 'instagram'",
        [],
        |r| r.get(0),
    )
    .unwrap_or(0)
}

pub fn status(conn: &Connection) -> Status {
    let queued = conn
        .query_row(
            "SELECT COUNT(*) FROM release_targets WHERE channel = 'instagram' AND (status = 'queued' OR (status = 'sent' AND fu_status = 'queued'))",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    Status {
        queued,
        sent_today: sent_today(conn),
        cap: cap(conn),
        busy: JOB.lock().unwrap_or_else(|e| e.into_inner()).is_some(),
        need_login: NEED_LOGIN.load(Ordering::SeqCst),
        paused: PAUSED.load(Ordering::SeqCst),
    }
}

fn close(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(WINDOW) {
        let _ = w.close();
    }
}

fn pause_seconds() -> u64 {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0) as u64;
    50 + nanos % 70
}

/// Starts the queue if it is idle.
pub fn kick(app: &AppHandle) {
    PAUSED.store(false, Ordering::SeqCst);
    let app = app.clone();
    std::thread::spawn(move || next(&app));
}

/// A deleted or withdrawn target must not be sent by a window already open.
pub fn drop_if_orphan(app: &AppHandle, conn: &Connection) {
    let current = JOB.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let Some(job) = current.filter(|j| !j.dry) else { return };
    let still: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM release_targets WHERE id = ?1 AND (status IN ('queued', 'sending') OR fu_status IN ('queued', 'sending'))",
            [job.target_id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if still == 0 {
        *JOB.lock().unwrap_or_else(|e| e.into_inner()) = None;
        let app = app.clone();
        std::thread::spawn(move || close(&app));
    }
}

fn emit(app: &AppHandle, card_id: i64, label: &str, finished: bool, error: Option<String>) {
    let _ = app.emit(EVENT, Progress { card_id, stage: "insta".into(), label: label.to_string(), finished, error, ..Default::default() });
}

fn next(app: &AppHandle) {
    if PAUSED.load(Ordering::SeqCst) {
        return;
    }
    let job = {
        let mut slot = JOB.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_some() {
            return;
        }
        let Ok(conn) = app.state::<AppState>().pool.get() else { return };
        if sent_today(&conn) >= cap(&conn) {
            emit(app, 0, "", true, Some("Plafond de DM du jour atteint, la suite partira demain.".into()));
            return;
        }
        let found = conn
            .query_row(
                "SELECT id, card_id, name, instagram, CASE WHEN status = 'queued' THEN dm ELSE followup END, status <> 'queued'
                 FROM release_targets
                 WHERE channel = 'instagram' AND instagram IS NOT NULL AND (status = 'queued' OR (status = 'sent' AND fu_status = 'queued'))
                 ORDER BY id LIMIT 1",
                [],
                |r| {
                    Ok(Job {
                        target_id: r.get(0)?,
                        card_id: r.get(1)?,
                        name: r.get(2)?,
                        handle: r.get(3)?,
                        text: super::one_line(&r.get::<_, Option<String>>(4)?.unwrap_or_default()),
                        followup: r.get::<_, i64>(5)? != 0,
                        dry: false,
                    })
                },
            )
            .ok();
        let Some(job) = found else { return };
        // From here its outcome is unknown until the page answers: an
        // interrupted message is failed, never queued again on its own.
        let _ = conn.execute(
            if job.followup { "UPDATE release_targets SET fu_status = 'sending' WHERE id = ?1" } else { "UPDATE release_targets SET status = 'sending' WHERE id = ?1" },
            [job.target_id],
        );
        *slot = Some(job.clone());
        job
    };
    if job.text.is_empty() {
        finish(app, &job, Some("Message vide.".into()));
        return;
    }
    emit(app, job.card_id, &job.name, false, None);
    if let Err(e) = open(app, Some(&job)) {
        finish(app, &job, Some(e));
    }
}

/// Records the outcome of the current job and moves on after a pause.
fn finish(app: &AppHandle, job: &Job, error: Option<String>) {
    *JOB.lock().unwrap_or_else(|e| e.into_inner()) = None;
    if job.dry {
        let info = error.is_none().then(|| format!("Essai réussi sur @{} : profil ouvert, message saisi puis effacé, rien n'a été envoyé.", job.handle));
        let _ = app.emit(EVENT, Progress { stage: "insta".into(), finished: true, error, info, ..Default::default() });
        let app = app.clone();
        std::thread::spawn(move || close(&app));
        return;
    }
    if let Ok(conn) = app.state::<AppState>().pool.get() {
        let ok = error.is_none();
        let _ = if job.followup {
            conn.execute(
                "UPDATE release_targets SET fu_status = ?2, fu_sent_at = CASE WHEN ?2 = 'sent' THEN datetime('now') END, error = ?3 WHERE id = ?1",
                params![job.target_id, if ok { "sent" } else { "failed" }, error],
            )
        } else {
            conn.execute(
                "UPDATE release_targets SET status = ?2, sent_at = CASE WHEN ?2 = 'sent' THEN datetime('now') END, error = ?3 WHERE id = ?1",
                params![job.target_id, if ok { "sent" } else { "failed" }, error],
            )
        };
        if ok {
            let _ = conn.execute(
                "INSERT INTO social_touches (channel, handle, body) VALUES ('instagram', ?1, ?2)",
                params![job.handle, job.text],
            );
        }
        advance(&conn, job.card_id);
    }
    emit(app, job.card_id, &job.name, true, error);
    let app = app.clone();
    std::thread::spawn(move || {
        close(&app);
        std::thread::sleep(Duration::from_secs(pause_seconds()));
        next(&app);
    });
}

fn on_handover(app: &AppHandle, kind: &str, json: &str) {
    let current = JOB.lock().unwrap_or_else(|e| e.into_inner()).clone();
    match kind {
        "ig_login" => {
            NEED_LOGIN.store(true, Ordering::SeqCst);
            emit(app, current.map(|j| j.card_id).unwrap_or(0), "", false, Some("Connectez-vous à Instagram dans la fenêtre ouverte, l'envoi reprendra tout seul.".into()));
        }
        "ig_sent" | "ig_failed" | "ig_dry" => {
            NEED_LOGIN.store(false, Ordering::SeqCst);
            let Some(job) = current else { return };
            let error = (kind == "ig_failed").then(|| {
                let v = serde_json::from_str::<serde_json::Value>(json).unwrap_or_default();
                let field = |k: &str| v.get(k).and_then(|r| r.as_str()).unwrap_or_default().to_string();
                let reason = Some(field("reason")).filter(|r| !r.is_empty()).unwrap_or_else(|| "Envoi échoué.".into());
                match field("seen") {
                    seen if seen.is_empty() => reason,
                    seen => format!("{reason} Page {} ; boutons vus : {}", field("path"), seen.chars().take(400).collect::<String>()),
                }
            });
            let app = app.clone();
            // (Off the navigation callback: database work and window closing.)
            std::thread::spawn(move || finish(&app, &job, error));
        }
        _ => {}
    }
}

/// Opens the Instagram window: on a profile with a message to send, or on the
/// home page for the user to sign in.
fn open(app: &AppHandle, job: Option<&Job>) -> Result<(), String> {
    if let Some(w) = app.get_webview_window(WINDOW) {
        let _ = w.close();
        std::thread::sleep(Duration::from_millis(600));
    }
    let url = match job {
        Some(j) => format!("https://www.instagram.com/{}/", j.handle),
        None => "https://www.instagram.com/".to_string(),
    };
    let url = url.parse().map_err(|_| "Adresse Instagram invalide.".to_string())?;
    let nav_app = app.clone();
    let mut builder = WebviewWindowBuilder::new(app, WINDOW, WebviewUrl::External(url))
        .title(if job.is_some() { "Instagram : envoi d'un DM validé" } else { "Instagram : connexion" })
        .inner_size(520.0, 780.0)
        .on_navigation(move |u| match parse_handover(u.as_str()) {
            Some((kind, json)) => {
                on_handover(&nav_app, &kind, &json);
                false
            }
            None => true,
        });
    if let Some(j) = job {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let id = if j.dry { format!("dry{stamp}") } else { format!("{}{}", j.target_id, if j.followup { "f" } else { "" }) };
        let task = serde_json::json!({ "id": id, "handle": j.handle, "text": j.text, "dry": j.dry });
        builder = builder.initialization_script(DM_JS.replace("__TASK__", &task.to_string()));
    }
    let window = builder.build().map_err(|e| e.to_string())?;
    let app = app.clone();
    let owner = job.map(|j| (j.target_id, j.followup, j.dry));
    window.on_window_event(move |e| {
        if !matches!(e, tauri::WindowEvent::Destroyed) {
            return;
        }
        // Only the window of the message being sent may end that message: an
        // older window (sign-in, previous message) closing changes nothing.
        let current = JOB.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let mine = current.filter(|j| Some((j.target_id, j.followup, j.dry)) == owner);
        if let Some(job) = mine.clone().filter(|j| !j.dry && NEED_LOGIN.load(Ordering::SeqCst)) {
            // Closed while Instagram was asking to sign in: nothing was typed,
            // the message goes back to the queue and waits for "Reprendre".
            PAUSED.store(true, Ordering::SeqCst);
            *JOB.lock().unwrap_or_else(|e| e.into_inner()) = None;
            if let Ok(conn) = app.state::<AppState>().pool.get() {
                let _ = conn.execute(
                    if job.followup { "UPDATE release_targets SET fu_status = 'queued' WHERE id = ?1" } else { "UPDATE release_targets SET status = 'queued' WHERE id = ?1" },
                    [job.target_id],
                );
            }
            let _ = app.emit(EVENT, Progress { stage: "insta".into(), finished: true, ..Default::default() });
        } else if let Some(job) = mine {
            // Closed by hand in the middle of a message: the queue stops and
            // the message is failed, to be checked in Instagram before any retry.
            PAUSED.store(true, Ordering::SeqCst);
            let app = app.clone();
            std::thread::spawn(move || finish(&app, &job, Some("Fenêtre fermée pendant l'envoi : vérifiez dans Instagram avant de renvoyer.".into())));
        } else {
            let _ = app.emit(EVENT, Progress { stage: "insta".into(), finished: true, ..Default::default() });
        }
    });
    Ok(())
}

/// "Connecter Instagram": the window on the home page, to sign in once.
#[tauri::command]
pub async fn insta_login(app: AppHandle) -> Result<(), String> {
    if JOB.lock().unwrap_or_else(|e| e.into_inner()).is_some() {
        return Err("Un DM est en cours d'envoi.".into());
    }
    NEED_LOGIN.store(false, Ordering::SeqCst);
    open(&app, None)
}

/// "Reprendre" / "Suspendre" the queue of validated DMs.
#[tauri::command]
pub async fn insta_queue(app: AppHandle, run: bool) -> Result<(), String> {
    if run {
        kick(&app);
    } else {
        PAUSED.store(true, Ordering::SeqCst);
        // (Closing the window ends the message in hand, see `open`.)
        close(&app);
    }
    Ok(())
}

#[tauri::command(async)]
pub fn insta_set_cap(state: tauri::State<'_, AppState>, cap: i64) -> Result<(), String> {
    if !(1..=40).contains(&cap) {
        return Err("Entre 1 et 40 DM par jour.".into());
    }
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO settings (key, value) VALUES ('insta_dm_daily_cap', ?1) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [cap.to_string()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// "Tester" : opens a profile, clicks Message, types a text and erases it.
/// Nothing is sent. Says whether the page still works the way the sender expects.
#[tauri::command]
pub async fn insta_test(app: AppHandle, handle: String) -> Result<(), String> {
    let handle = crate::commands::social::normalize_handle_input(&handle).ok_or("Compte Instagram invalide.")?;
    let job = Job { target_id: 0, card_id: 0, followup: false, name: handle.clone(), handle, text: "Essai de saisie, ce message ne sera pas envoyé.".into(), dry: true };
    {
        let mut slot = JOB.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_some() {
            return Err("Un DM est en cours d'envoi.".into());
        }
        *slot = Some(job.clone());
    }
    if let Err(e) = open(&app, Some(&job)) {
        *JOB.lock().unwrap_or_else(|e| e.into_inner()) = None;
        return Err(e);
    }
    Ok(())
}
