//! Releases on the insrt label, run as a board.
//!
//! A release starts as a profile (name, genre, mastered or not). Validating it
//! with a release date lays out its plan: one card per step, dated backwards
//! from the release day. A card sits on a calendar day and in a board column,
//! and is worked from its own pop-up:
//!  - `meta`: the metadata sheet the distributor asks for;
//!  - checklist cards (mastering, distributor upload, editorial pitch, Groover,
//!    release day, recap, custom): steps to tick, a text the AI can write;
//!  - campaign cards (`dj`, `playlists`): people found by the agent, one draft
//!    each, sent on validation only, by email from the personal address or by
//!    Instagram DM (see `insta`).
//!
//! Nothing leaves without a click on "Valider et envoyer", follow-ups included.
//! Deleting a card or a release deletes its targets, so nothing queued stays.
//!
//! The agent never invents a contact: names come from the AI or from the user,
//! addresses, handles and Trackstack links only from search results, and an
//! address is kept only when its domain takes mail.

use crate::commands::deliverability::dns::DohResolver;
use crate::commands::email::{send_personal, ThreadRef};
use crate::commands::email_finder::MxCheck;
use crate::commands::enrich;
use crate::commands::residency::claude::{self, extract_json, LlmRequest, SystemBlock};
use crate::commands::residency::content::HOUSE_VOICE;
use crate::commands::social;
use crate::db::DbPool;
use crate::AppState;
use regex::Regex;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Duration;
use tauri::{AppHandle, Emitter, State};

pub mod insta;

pub const EVENT: &str = "releases:progress";
/// People looked up in one pass of the agent (two searches each at most).
const FIND_BATCH: usize = 15;

// ---------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------

pub fn migrate(conn: &Connection) {
    for stmt in [
        "CREATE TABLE IF NOT EXISTS releases (
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            name         TEXT NOT NULL,
            genre        TEXT NOT NULL DEFAULT '',
            master_state TEXT NOT NULL DEFAULT 'unmastered', -- master | unmastered
            artist       TEXT NOT NULL DEFAULT '',
            listen_url   TEXT,
            notes        TEXT,
            release_date TEXT,                               -- NULL until validated
            created_at   TEXT NOT NULL DEFAULT (datetime('now'))
        )",
        "CREATE TABLE IF NOT EXISTS release_cards (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            release_id INTEGER NOT NULL REFERENCES releases(id) ON DELETE CASCADE,
            kind       TEXT NOT NULL,
            title      TEXT NOT NULL,
            due_date   TEXT NOT NULL,
            col        TEXT NOT NULL DEFAULT 'todo',         -- todo | doing | waiting | done
            pos        INTEGER NOT NULL DEFAULT 0,
            data       TEXT NOT NULL DEFAULT '{}',
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        )",
        "CREATE TABLE IF NOT EXISTS release_targets (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            card_id     INTEGER NOT NULL REFERENCES release_cards(id) ON DELETE CASCADE,
            name        TEXT NOT NULL,
            why         TEXT,
            email       TEXT,
            instagram   TEXT,
            trackstack  TEXT,
            source_url  TEXT,
            channel     TEXT NOT NULL DEFAULT 'none',        -- email | instagram | none
            subject     TEXT,
            body        TEXT,
            dm          TEXT,
            followup    TEXT,
            status      TEXT NOT NULL DEFAULT 'found',       -- found | draft | queued | sent | failed | skipped
            error       TEXT,
            email_id    INTEGER,
            sent_at     TEXT,
            fu_status   TEXT,                                -- NULL | queued | sent | failed
            fu_sent_at  TEXT,
            replied     INTEGER NOT NULL DEFAULT 0,
            created_at  TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE (card_id, name)
        )",
        "CREATE INDEX IF NOT EXISTS idx_release_cards_release ON release_cards(release_id)",
        "CREATE INDEX IF NOT EXISTS idx_release_targets_card ON release_targets(card_id)",
    ] {
        let _ = conn.execute(stmt, []);
    }
}

// ---------------------------------------------------------------------------
// The plan
// ---------------------------------------------------------------------------

pub const CAMPAIGN_KINDS: &[&str] = &["dj", "playlists"];
pub const KINDS: &[&str] = &["mastering", "meta", "distri", "pitch", "dj", "playlists", "groover", "release", "recap", "custom"];

/// (kind, days from the release day, French title).
const PLAN: &[(&str, i64, &str)] = &[
    ("mastering", -42, "Mastering"),
    ("meta", -35, "Collecter les métadonnées"),
    ("distri", -28, "Upload au distributeur"),
    ("pitch", -28, "Pitch éditorial Spotify"),
    ("dj", -21, "Campagne DJ"),
    ("playlists", -14, "Campagne playlists"),
    ("groover", -14, "Campagne Groover"),
    ("release", 0, "Jour de sortie"),
    ("recap", 7, "Bilan de la sortie"),
];

fn default_title(kind: &str) -> &'static str {
    PLAN.iter().find(|p| p.0 == kind).map(|p| p.2).unwrap_or("Étape")
}

fn steps(labels: &[&str]) -> Value {
    Value::Array(labels.iter().map(|l| json!({ "label": l, "done": false })).collect())
}

/// What a new card starts with.
fn default_data(kind: &str, r: &Release) -> Value {
    match kind {
        "meta" => json!({ "fields": {
            "title": r.name, "artists": r.artist, "label": "insrt", "genre": r.genre,
            "release_date": r.release_date.clone().unwrap_or_default(),
        }}),
        "mastering" => json!({ "checklist": steps(&[
            "Mix validé et exporté (WAV 24 bits, sans limiteur)",
            "Envoyé au mastering",
            "Master reçu et écouté sur plusieurs systèmes",
            "Master final rangé (WAV 16 bits 44,1 kHz et 24 bits)",
        ])}),
        "distri" => json!({ "checklist": steps(&[
            "Métadonnées complètes (carte Métadonnées terminée)",
            "Master WAV envoyé",
            "Pochette 3000 x 3000 px envoyée",
            "Date de sortie et plateformes réglées",
            "Crédits et parts déclarés",
            "Sortie acceptée par le distributeur",
        ])}),
        "pitch" => json!({ "checklist": steps(&[
            "Sortie visible dans Spotify for Artists",
            "Texte du pitch rédigé",
            "Pitch envoyé (au moins 7 jours avant la sortie)",
        ])}),
        "groover" => json!({ "checklist": steps(&[
            "Lien d'écoute prêt",
            "Texte de présentation rédigé",
            "Curateurs choisis sur Groover",
            "Campagne lancée",
            "Retours reportés ici",
        ]), "url": "https://groover.co" }),
        "release" => json!({ "checklist": steps(&[
            "Liens vérifiés sur toutes les plateformes",
            "Annonce publiée",
            "Lien envoyé à ceux qui ont répondu",
        ])}),
        "recap" => json!({ "checklist": steps(&[
            "Écoutes de la première semaine notées",
            "Playlists obtenues notées",
            "Retours des DJs notés",
        ])}),
        "dj" | "playlists" => json!({ "lang": "en" }),
        _ => json!({ "checklist": [] }),
    }
}

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Release {
    #[serde(default)]
    pub id: i64,
    pub name: String,
    #[serde(default)]
    pub genre: String,
    #[serde(default)]
    pub master_state: String,
    #[serde(default)]
    pub artist: String,
    #[serde(default)]
    pub listen_url: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub release_date: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Card {
    pub id: i64,
    pub release_id: i64,
    pub release_name: String,
    pub kind: String,
    pub title: String,
    pub due_date: String,
    pub col: String,
    pub pos: i64,
    pub data: Value,
    /// Campaign cards: people found, drafts waiting for validation, sent,
    /// answers, follow-ups that can be validated today.
    pub targets: i64,
    pub drafts: i64,
    pub sent: i64,
    pub replies: i64,
    pub followups_due: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Target {
    pub id: i64,
    pub card_id: i64,
    pub name: String,
    pub why: Option<String>,
    pub email: Option<String>,
    pub instagram: Option<String>,
    pub trackstack: Option<String>,
    pub source_url: Option<String>,
    pub channel: String,
    pub subject: Option<String>,
    pub body: Option<String>,
    pub dm: Option<String>,
    pub followup: Option<String>,
    pub status: String,
    pub error: Option<String>,
    pub sent_at: Option<String>,
    pub fu_status: Option<String>,
    pub fu_sent_at: Option<String>,
    pub replied: bool,
    pub opened_at: Option<String>,
    pub open_count: i64,
    pub followup_due: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Board {
    pub releases: Vec<Release>,
    pub cards: Vec<Card>,
    pub insta: insta::Status,
    pub sender: Option<String>,
    pub ai_ready: bool,
    pub search_ready: bool,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Progress {
    pub card_id: i64,
    /// find | draft | send | insta
    pub stage: String,
    pub label: String,
    pub done: usize,
    pub total: usize,
    pub finished: bool,
    pub error: Option<String>,
}

fn release_from(row: &rusqlite::Row) -> rusqlite::Result<Release> {
    Ok(Release {
        id: row.get(0)?,
        name: row.get(1)?,
        genre: row.get(2)?,
        master_state: row.get(3)?,
        artist: row.get(4)?,
        listen_url: row.get(5)?,
        notes: row.get(6)?,
        release_date: row.get(7)?,
    })
}
const RELEASE_COLS: &str = "id, name, genre, master_state, artist, listen_url, notes, release_date";

fn load_release(conn: &Connection, id: i64) -> Result<Release, String> {
    conn.query_row(&format!("SELECT {RELEASE_COLS} FROM releases WHERE id = ?1"), [id], release_from)
        .map_err(|_| "Sortie introuvable.".to_string())
}

/// A sent message with no answer, old enough for a follow-up.
const FOLLOWUP_DUE_SQL: &str = "(t.status = 'sent' AND t.replied = 0 AND t.fu_status IS NULL \
    AND COALESCE(e.replied_at, '') = '' AND t.followup IS NOT NULL AND trim(t.followup) <> '' \
    AND date(t.sent_at) <= date('now', 'localtime', '-7 days'))";
const REPLIED_SQL: &str = "(t.replied = 1 OR COALESCE(e.replied_at, '') <> '')";

fn load_cards(conn: &Connection, only: Option<i64>) -> Result<Vec<Card>, String> {
    let sql = format!(
        "SELECT c.id, c.release_id, r.name, c.kind, c.title, c.due_date, c.col, c.pos, c.data,
                COUNT(t.id),
                COALESCE(SUM(t.status = 'draft'), 0),
                COALESCE(SUM(t.status = 'sent'), 0),
                COALESCE(SUM(t.status = 'sent' AND {REPLIED_SQL}), 0),
                COALESCE(SUM({FOLLOWUP_DUE_SQL}), 0)
         FROM release_cards c
         JOIN releases r ON r.id = c.release_id
         LEFT JOIN release_targets t ON t.card_id = c.id AND t.status <> 'skipped'
         LEFT JOIN emails e ON e.id = t.email_id
         WHERE (?1 IS NULL OR c.id = ?1)
         GROUP BY c.id
         ORDER BY c.due_date, c.pos, c.id"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([only], |row| {
            let data: String = row.get(8)?;
            Ok(Card {
                id: row.get(0)?,
                release_id: row.get(1)?,
                release_name: row.get(2)?,
                kind: row.get(3)?,
                title: row.get(4)?,
                due_date: row.get(5)?,
                col: row.get(6)?,
                pos: row.get(7)?,
                data: serde_json::from_str(&data).unwrap_or_else(|_| json!({})),
                targets: row.get(9)?,
                drafts: row.get(10)?,
                sent: row.get(11)?,
                replies: row.get(12)?,
                followups_due: row.get(13)?,
            })
        })
        .map_err(|e| e.to_string())?;
    Ok(rows.filter_map(Result::ok).collect())
}

fn load_targets(conn: &Connection, card_id: i64) -> Result<Vec<Target>, String> {
    let sql = format!(
        "SELECT t.id, t.card_id, t.name, t.why, t.email, t.instagram, t.trackstack, t.source_url, t.channel,
                t.subject, t.body, t.dm, t.followup, t.status, t.error, t.sent_at, t.fu_status, t.fu_sent_at,
                {REPLIED_SQL}, e.opened_at, COALESCE(e.open_count, 0), {FOLLOWUP_DUE_SQL}
         FROM release_targets t
         LEFT JOIN emails e ON e.id = t.email_id
         WHERE t.card_id = ?1
         ORDER BY CASE t.status WHEN 'draft' THEN 0 WHEN 'found' THEN 1 WHEN 'queued' THEN 2 WHEN 'failed' THEN 3
                                WHEN 'sent' THEN 4 ELSE 5 END,
                  CASE t.channel WHEN 'email' THEN 0 WHEN 'instagram' THEN 1 ELSE 2 END, t.name"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([card_id], |row| {
            Ok(Target {
                id: row.get(0)?,
                card_id: row.get(1)?,
                name: row.get(2)?,
                why: row.get(3)?,
                email: row.get(4)?,
                instagram: row.get(5)?,
                trackstack: row.get(6)?,
                source_url: row.get(7)?,
                channel: row.get(8)?,
                subject: row.get(9)?,
                body: row.get(10)?,
                dm: row.get(11)?,
                followup: row.get(12)?,
                status: row.get(13)?,
                error: row.get(14)?,
                sent_at: row.get(15)?,
                fu_status: row.get(16)?,
                fu_sent_at: row.get(17)?,
                replied: row.get::<_, i64>(18)? != 0,
                opened_at: row.get(19)?,
                open_count: row.get(20)?,
                followup_due: row.get::<_, i64>(21)? != 0,
            })
        })
        .map_err(|e| e.to_string())?;
    Ok(rows.filter_map(Result::ok).collect())
}

fn col_rank(col: &str) -> i64 {
    match col {
        "todo" => 0,
        "doing" => 1,
        "waiting" => 2,
        _ => 3,
    }
}

/// A campaign card moves forward on its own with its work, never backwards.
pub(crate) fn advance(conn: &Connection, card_id: i64) {
    let Ok((col, kind)) = conn.query_row("SELECT col, kind FROM release_cards WHERE id = ?1", [card_id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    }) else {
        return;
    };
    if !CAMPAIGN_KINDS.contains(&kind.as_str()) {
        return;
    }
    let (total, open, sent): (i64, i64, i64) = conn
        .query_row(
            "SELECT COUNT(*), COALESCE(SUM(status IN ('found', 'draft', 'queued')), 0), COALESCE(SUM(status = 'sent'), 0)
             FROM release_targets WHERE card_id = ?1 AND status <> 'skipped'",
            [card_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap_or((0, 0, 0));
    let want = if total == 0 {
        "todo"
    } else if sent > 0 && open == 0 {
        "waiting"
    } else {
        "doing"
    };
    if col_rank(want) > col_rank(&col) {
        let _ = conn.execute("UPDATE release_cards SET col = ?2 WHERE id = ?1", params![card_id, want]);
    }
}

fn valid_date(d: &str) -> bool {
    chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").is_ok()
}

// ---------------------------------------------------------------------------
// Board commands
// ---------------------------------------------------------------------------

#[tauri::command(async)]
pub fn releases_board(state: State<'_, AppState>) -> Result<Board, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(&format!("SELECT {RELEASE_COLS} FROM releases ORDER BY COALESCE(release_date, '9999'), id"))
        .map_err(|e| e.to_string())?;
    let releases = stmt.query_map([], release_from).map_err(|e| e.to_string())?.filter_map(Result::ok).collect();
    Ok(Board {
        releases,
        cards: load_cards(&conn, None)?,
        insta: insta::status(&conn),
        sender: crate::commands::email::labels_sender_address(&conn),
        ai_ready: claude::client(&conn).is_some(),
        search_ready: crate::secrets::read_setting(&conn, "serper_api_key").is_some_and(|k| !k.trim().is_empty()),
    })
}

/// Creates or edits the profile of a release.
#[tauri::command(async)]
pub fn release_save(state: State<'_, AppState>, release: Release) -> Result<i64, String> {
    let name = release.name.trim();
    if name.is_empty() {
        return Err("Le nom de la sortie est vide.".into());
    }
    let master = if release.master_state == "master" { "master" } else { "unmastered" };
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let artist = match release.artist.trim() {
        "" => crate::commands::email::sole_artist(&conn).unwrap_or_default(),
        a => a.to_string(),
    };
    let clean = |v: &Option<String>| v.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    if release.id > 0 {
        conn.execute(
            "UPDATE releases SET name = ?2, genre = ?3, master_state = ?4, artist = ?5, listen_url = ?6, notes = ?7 WHERE id = ?1",
            params![release.id, name, release.genre.trim(), master, artist, clean(&release.listen_url), clean(&release.notes)],
        )
        .map_err(|e| e.to_string())?;
        Ok(release.id)
    } else {
        conn.execute(
            "INSERT INTO releases (name, genre, master_state, artist, listen_url, notes) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![name, release.genre.trim(), master, artist, clean(&release.listen_url), clean(&release.notes)],
        )
        .map_err(|e| e.to_string())?;
        Ok(conn.last_insert_rowid())
    }
}

/// Validates a release with its date. The first time, lays out the plan and
/// sets the agent on the campaign cards. Later, moves every card that is not
/// done by the same number of days.
#[tauri::command(async)]
pub fn release_validate(app: AppHandle, state: State<'_, AppState>, id: i64, date: String) -> Result<(), String> {
    let day = chrono::NaiveDate::parse_from_str(&date, "%Y-%m-%d").map_err(|_| "Date de sortie invalide.".to_string())?;
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let mut r = load_release(&conn, id)?;
    let before = r.release_date.as_deref().and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok());
    conn.execute("UPDATE releases SET release_date = ?2 WHERE id = ?1", params![id, date]).map_err(|e| e.to_string())?;
    r.release_date = Some(date.clone());
    let existing: i64 = conn.query_row("SELECT COUNT(*) FROM release_cards WHERE release_id = ?1", [id], |x| x.get(0)).unwrap_or(0);
    if existing > 0 {
        if let Some(b) = before {
            let shift = (day - b).num_days();
            if shift != 0 {
                conn.execute(
                    "UPDATE release_cards SET due_date = date(due_date, ?2) WHERE release_id = ?1 AND col <> 'done'",
                    params![id, format!("{shift:+} days")],
                )
                .map_err(|e| e.to_string())?;
            }
        }
        return Ok(());
    }
    let today = chrono::Local::now().date_naive();
    let mut campaigns = Vec::new();
    for (pos, (kind, offset, title)) in PLAN.iter().enumerate() {
        if *kind == "mastering" && r.master_state == "master" {
            continue;
        }
        // (A step whose day is already past is due today.)
        let due = (day + chrono::Duration::days(*offset)).max(if *offset < 0 { today } else { day });
        conn.execute(
            "INSERT INTO release_cards (release_id, kind, title, due_date, pos, data) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, kind, title, due.format("%Y-%m-%d").to_string(), pos as i64, default_data(kind, &r).to_string()],
        )
        .map_err(|e| e.to_string())?;
        if CAMPAIGN_KINDS.contains(kind) {
            campaigns.push(conn.last_insert_rowid());
        }
    }
    drop(conn);
    // The agent starts on its own, one campaign after the other.
    let pool = state.pool.clone();
    std::thread::spawn(move || {
        for card in campaigns {
            agent::run(&app, &pool, card, Vec::new());
        }
    });
    Ok(())
}

#[tauri::command(async)]
pub fn release_delete(app: AppHandle, state: State<'_, AppState>, id: i64) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM release_targets WHERE card_id IN (SELECT id FROM release_cards WHERE release_id = ?1)", [id])
        .map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM release_cards WHERE release_id = ?1", [id]).map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM releases WHERE id = ?1", [id]).map_err(|e| e.to_string())?;
    insta::drop_if_orphan(&app, &conn);
    Ok(())
}

/// Adds a card to a release on a given day.
#[tauri::command(async)]
pub fn card_add(
    app: AppHandle,
    state: State<'_, AppState>,
    release_id: i64,
    kind: String,
    title: Option<String>,
    due_date: String,
) -> Result<i64, String> {
    if !KINDS.contains(&kind.as_str()) {
        return Err("Type de carte inconnu.".into());
    }
    if !valid_date(&due_date) {
        return Err("Date invalide.".into());
    }
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let r = load_release(&conn, release_id)?;
    let title = title.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).unwrap_or_else(|| default_title(&kind).to_string());
    conn.execute(
        "INSERT INTO release_cards (release_id, kind, title, due_date, pos, data)
         VALUES (?1, ?2, ?3, ?4, (SELECT COALESCE(MAX(pos), 0) + 1 FROM release_cards WHERE release_id = ?1), ?5)",
        params![release_id, kind, title, due_date, default_data(&kind, &r).to_string()],
    )
    .map_err(|e| e.to_string())?;
    let id = conn.last_insert_rowid();
    drop(conn);
    if CAMPAIGN_KINDS.contains(&kind.as_str()) {
        let pool = state.pool.clone();
        std::thread::spawn(move || agent::run(&app, &pool, id, Vec::new()));
    }
    Ok(id)
}

/// Moves a card to another column, another day, or renames it.
#[tauri::command(async)]
pub fn card_move(
    state: State<'_, AppState>,
    id: i64,
    col: Option<String>,
    due_date: Option<String>,
    title: Option<String>,
) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    if let Some(c) = col.filter(|c| ["todo", "doing", "waiting", "done"].contains(&c.as_str())) {
        conn.execute("UPDATE release_cards SET col = ?2 WHERE id = ?1", params![id, c]).map_err(|e| e.to_string())?;
    }
    if let Some(d) = due_date.filter(|d| valid_date(d)) {
        conn.execute("UPDATE release_cards SET due_date = ?2 WHERE id = ?1", params![id, d]).map_err(|e| e.to_string())?;
    }
    if let Some(t) = title.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()) {
        conn.execute("UPDATE release_cards SET title = ?2 WHERE id = ?1", params![id, t]).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Saves what a card holds (metadata sheet, checklist, notes, text).
#[tauri::command(async)]
pub fn card_set_data(state: State<'_, AppState>, id: i64, data: Value, col: Option<String>) -> Result<(), String> {
    if !data.is_object() {
        return Err("Contenu de carte invalide.".into());
    }
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.execute("UPDATE release_cards SET data = ?2 WHERE id = ?1", params![id, data.to_string()]).map_err(|e| e.to_string())?;
    if let Some(c) = col.filter(|c| ["todo", "doing", "waiting", "done"].contains(&c.as_str())) {
        conn.execute("UPDATE release_cards SET col = ?2 WHERE id = ?1", params![id, c]).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command(async)]
pub fn card_delete(app: AppHandle, state: State<'_, AppState>, id: i64) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM release_targets WHERE card_id = ?1", [id]).map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM release_cards WHERE id = ?1", [id]).map_err(|e| e.to_string())?;
    insta::drop_if_orphan(&app, &conn);
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct CardDetail {
    pub card: Card,
    pub release: Release,
    pub targets: Vec<Target>,
}

#[tauri::command(async)]
pub fn card_detail(state: State<'_, AppState>, id: i64) -> Result<CardDetail, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let card = load_cards(&conn, Some(id))?.into_iter().next().ok_or("Carte introuvable.")?;
    let release = load_release(&conn, card.release_id)?;
    let targets = load_targets(&conn, id)?;
    Ok(CardDetail { card, release, targets })
}

// ---------------------------------------------------------------------------
// Targets
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct TargetEdit {
    pub id: i64,
    pub channel: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub instagram: Option<String>,
    #[serde(default)]
    pub subject: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub dm: Option<String>,
    #[serde(default)]
    pub followup: Option<String>,
}

/// A draft corrected by hand. Only what has not left yet can change channel or text.
#[tauri::command(async)]
pub fn target_save(state: State<'_, AppState>, edit: TargetEdit) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let clean = |v: &Option<String>| v.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    let email = match clean(&edit.email) {
        Some(e) => Some(enrich::clean_email(&e).ok_or("Adresse email invalide.")?),
        None => None,
    };
    let handle = match clean(&edit.instagram) {
        Some(h) => Some(social::normalize_handle_input(&h).ok_or("Compte Instagram invalide.")?),
        None => None,
    };
    let channel = match edit.channel.as_str() {
        "email" if email.is_some() => "email",
        "instagram" if handle.is_some() => "instagram",
        "email" => return Err("Pas d'adresse email pour ce contact.".into()),
        "instagram" => return Err("Pas de compte Instagram pour ce contact.".into()),
        _ => "none",
    };
    let n = conn
        .execute(
            "UPDATE release_targets SET channel = ?2, email = ?3, instagram = ?4, subject = ?5, body = ?6, dm = ?7, followup = ?8
             WHERE id = ?1 AND status IN ('found', 'draft', 'failed')",
            params![edit.id, channel, email, handle, clean(&edit.subject), clean(&edit.body), clean(&edit.dm).map(|d| one_line(&d)), clean(&edit.followup)],
        )
        .map_err(|e| e.to_string())?;
    if n == 0 {
        // Already sent: only the follow-up text can still change.
        conn.execute(
            "UPDATE release_targets SET followup = ?2 WHERE id = ?1 AND fu_status IS NULL",
            params![edit.id, clean(&edit.followup)],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Leaves people out of a campaign (they are not proposed again), or marks an answer.
#[tauri::command(async)]
pub fn targets_mark(app: AppHandle, state: State<'_, AppState>, ids: Vec<i64>, mark: String) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    for id in ids {
        match mark.as_str() {
            "skip" => conn.execute("UPDATE release_targets SET status = 'skipped' WHERE id = ?1 AND status <> 'sent'", [id]),
            "restore" => conn.execute(
                "UPDATE release_targets SET status = CASE WHEN body IS NOT NULL OR dm IS NOT NULL THEN 'draft' ELSE 'found' END
                 WHERE id = ?1 AND status IN ('skipped', 'queued')",
                [id],
            ),
            "replied" => conn.execute("UPDATE release_targets SET replied = 1 WHERE id = ?1", [id]),
            "not_replied" => conn.execute("UPDATE release_targets SET replied = 0 WHERE id = ?1", [id]),
            "no_followup" => conn.execute("UPDATE release_targets SET fu_status = 'skipped' WHERE id = ?1 AND fu_status IS NULL", [id]),
            _ => return Err("Action inconnue.".into()),
        }
        .map_err(|e| e.to_string())?;
    }
    insta::drop_if_orphan(&app, &conn);
    Ok(())
}

pub(crate) fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// What a text still lacks before it can leave.
fn unfinished(text: &str) -> bool {
    text.contains("[LIEN]") || text.contains("[LINK]") || text.trim().is_empty()
}

static SENDING: AtomicBool = AtomicBool::new(false);

/// "Valider et envoyer": the chosen drafts leave. Emails at once, one after the
/// other, from the personal address. DMs join the Instagram queue.
#[tauri::command(async)]
pub fn card_send(app: AppHandle, state: State<'_, AppState>, card_id: i64, ids: Vec<i64>, followup: bool) -> Result<usize, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let targets: Vec<Target> = load_targets(&conn, card_id)?.into_iter().filter(|t| ids.contains(&t.id)).collect();
    let mut emails = Vec::new();
    let mut dms = 0usize;
    for t in &targets {
        let ready = if followup { t.status == "sent" && t.fu_status.is_none() } else { t.status == "draft" || t.status == "failed" };
        if !ready {
            continue;
        }
        let text = if followup {
            t.followup.clone()
        } else if t.channel == "email" {
            t.body.clone()
        } else {
            t.dm.clone()
        }
        .unwrap_or_default();
        if unfinished(&text) {
            return Err(format!("Le message pour {} n'est pas prêt (texte vide ou lien d'écoute manquant).", t.name));
        }
        match t.channel.as_str() {
            "email" => emails.push(t.clone()),
            "instagram" => {
                let sql = if followup {
                    "UPDATE release_targets SET fu_status = 'queued', error = NULL WHERE id = ?1"
                } else {
                    "UPDATE release_targets SET status = 'queued', error = NULL WHERE id = ?1"
                };
                conn.execute(sql, [t.id]).map_err(|e| e.to_string())?;
                dms += 1;
            }
            _ => {}
        }
    }
    if !emails.is_empty() {
        if !crate::commands::email::labels_sender_ready(&conn) {
            return Err("Adresse personnelle d'envoi non réglée (Réglages, Email, compte des labels).".into());
        }
        if SENDING.swap(true, Ordering::SeqCst) {
            return Err("Un envoi est déjà en cours.".into());
        }
    }
    advance(&conn, card_id);
    drop(conn);
    let total = emails.len() + dms;
    if dms > 0 {
        insta::kick(&app);
    }
    if !emails.is_empty() {
        let pool = state.pool.clone();
        std::thread::spawn(move || {
            let n = emails.len();
            for (i, t) in emails.iter().enumerate() {
                let _ = app.emit(EVENT, Progress { card_id, stage: "send".into(), label: t.name.clone(), done: i, total: n, ..Default::default() });
                send_email(&pool, t, followup);
                // (A human gap between two mails, never a burst.)
                if i + 1 < n {
                    std::thread::sleep(Duration::from_secs(20));
                }
            }
            if let Ok(conn) = pool.get() {
                advance(&conn, card_id);
            }
            SENDING.store(false, Ordering::SeqCst);
            let _ = app.emit(EVENT, Progress { card_id, stage: "send".into(), done: n, total: n, finished: true, ..Default::default() });
        });
    }
    Ok(total)
}

fn send_email(pool: &DbPool, t: &Target, followup: bool) {
    let Some(to) = t.email.as_deref() else { return };
    let subject = t.subject.clone().unwrap_or_default();
    let result = if followup {
        // In the thread of the first mail.
        let first: Option<String> = pool.get().ok().and_then(|c| {
            c.query_row(
                "SELECT e.message_id FROM release_targets t JOIN emails e ON e.id = t.email_id WHERE t.id = ?1",
                [t.id],
                |r| r.get::<_, Option<String>>(0),
            )
            .ok()
            .flatten()
        });
        let thread = first.map(|id| ThreadRef { in_reply_to: Some(id.clone()), references: vec![id] });
        let subject = if subject.to_lowercase().starts_with("re:") { subject } else { format!("Re: {subject}") };
        send_personal(pool, to, &subject, t.followup.as_deref().unwrap_or_default(), thread.as_ref(), &[])
    } else {
        send_personal(pool, to, &subject, t.body.as_deref().unwrap_or_default(), None, &[])
    };
    let Ok(conn) = pool.get() else { return };
    let (ok, error, email_id) = match result {
        Ok((r, id, _)) => (r.ok, r.error, id),
        Err(e) => (false, Some(e), None),
    };
    let _ = if followup {
        conn.execute(
            "UPDATE release_targets SET fu_status = ?2, fu_sent_at = CASE WHEN ?2 = 'sent' THEN datetime('now') END, error = ?3 WHERE id = ?1",
            params![t.id, if ok { "sent" } else { "failed" }, error],
        )
    } else {
        conn.execute(
            "UPDATE release_targets SET status = ?2, sent_at = CASE WHEN ?2 = 'sent' THEN datetime('now') END, error = ?3, email_id = ?4 WHERE id = ?1",
            params![t.id, if ok { "sent" } else { "failed" }, error, email_id],
        )
    };
}

// ---------------------------------------------------------------------------
// Texts the AI writes for a checklist card
// ---------------------------------------------------------------------------

/// Writes the text a card needs (editorial pitch, Groover presentation) into its `text`.
#[tauri::command(async)]
pub fn card_write(state: State<'_, AppState>, id: i64) -> Result<String, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let card = load_cards(&conn, Some(id))?.into_iter().next().ok_or("Carte introuvable.")?;
    let r = load_release(&conn, card.release_id)?;
    let llm = claude::client(&conn).ok_or("Clé Claude absente (Réglages).")?;
    let what = match card.kind.as_str() {
        "pitch" => "the pitch typed into the Spotify for Artists editorial form (at most 480 characters, English): what the track is, its mood and where it fits, one fact about the artist or the label. Plain and factual.",
        "groover" => "the presentation shown to curators on Groover (at most 600 characters, English, first person): what the track is, who made it, why it suits their selection. Plain and factual.",
        "release" => "a short release-day post for Instagram (French, at most 300 characters), no hashtag wall, no slogan.",
        _ => "a short working note for this step (French, at most 400 characters).",
    };
    let req = LlmRequest {
        model: claude::drafting_model(&conn),
        max_tokens: 700,
        system: vec![SystemBlock { text: format!("You write short working texts for an independent electronic music label. Write {what}\nRules:\n{HOUSE_VOICE}\nNever invent a fact, a figure, a name or a support. Answer with the text only."), cache: false }],
        user: format!(
            "Release: {}\nArtist: {}\nLabel: insrt\nGenre: {}\nRelease date: {}\nNotes from the artist: {}\nStep: {}",
            r.name,
            r.artist,
            r.genre,
            r.release_date.clone().unwrap_or_default(),
            r.notes.clone().unwrap_or_else(|| "none".into()),
            card.title
        ),
        schema: None,
    };
    let resp = llm.complete(&req)?;
    claude::log_usage(&conn, "release_text", None, &resp.model, &resp.usage);
    let text = claude::no_dashes(resp.text.trim());
    let mut data = card.data.clone();
    data["text"] = json!(text);
    conn.execute("UPDATE release_cards SET data = ?2 WHERE id = ?1", params![id, data.to_string()]).map_err(|e| e.to_string())?;
    Ok(text)
}

/// "Chercher": sets the agent on a campaign card, optionally on names typed by hand.
#[tauri::command(async)]
pub fn card_find(app: AppHandle, state: State<'_, AppState>, id: i64, names: Vec<String>) -> Result<(), String> {
    if agent::RUNNING.load(Ordering::SeqCst) {
        return Err("L'agent travaille déjà sur une carte.".into());
    }
    let pool = state.pool.clone();
    std::thread::spawn(move || agent::run(&app, &pool, id, names));
    Ok(())
}

// ---------------------------------------------------------------------------
// The agent
// ---------------------------------------------------------------------------

pub mod agent {
    use super::*;

    pub static RUNNING: AtomicBool = AtomicBool::new(false);

    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            RUNNING.store(false, Ordering::SeqCst);
        }
    }

    fn emit(app: &AppHandle, p: Progress) {
        let _ = app.emit(EVENT, p);
    }

    /// Finds people for a campaign card, then writes one draft each. Runs on
    /// its own thread; a second call waits for the first to end.
    pub fn run(app: &AppHandle, pool: &DbPool, card_id: i64, names: Vec<String>) {
        while RUNNING.swap(true, Ordering::SeqCst) {
            std::thread::sleep(Duration::from_secs(2));
        }
        let _guard = Guard;
        let outcome = work(app, pool, card_id, names);
        if let Ok(conn) = pool.get() {
            advance(&conn, card_id);
        }
        emit(app, Progress { card_id, stage: "draft".into(), finished: true, error: outcome.err(), ..Default::default() });
    }

    fn work(app: &AppHandle, pool: &DbPool, card_id: i64, names: Vec<String>) -> Result<(), String> {
        let (card, release) = {
            let conn = pool.get().map_err(|e| e.to_string())?;
            let card = load_cards(&conn, Some(card_id))?.into_iter().next().ok_or("Carte introuvable.")?;
            let release = load_release(&conn, card.release_id)?;
            (card, release)
        };
        match card.kind.as_str() {
            "playlists" => {
                let added = curators(pool, &card, &release)?;
                let named = lookup(app, pool, &card, &release, names, "playlist curator")?;
                if added + named == 0 && card.targets == 0 {
                    return Err("Aucun curateur avec une adresse dans le Radar. Lancez une recherche dans le Radar, ou ajoutez des noms ici.".into());
                }
            }
            "dj" => {
                let names = if names.is_empty() { suggest_djs(pool, &card, &release)? } else { names };
                lookup(app, pool, &card, &release, names, "dj")?;
            }
            _ => return Ok(()),
        }
        drafts(app, pool, &card, &release)
    }

    /// Playlist curators the Radar already holds with a usable address, closest genre first.
    fn curators(pool: &DbPool, card: &Card, release: &Release) -> Result<usize, String> {
        let conn = pool.get().map_err(|e| e.to_string())?;
        let words: Vec<String> = release.genre.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() > 2).map(str::to_string).collect();
        let sql = format!(
            "SELECT c.nom, c.owner_name, c.url, c.followers, COALESCE(c.genre, '') || ' ' || COALESCE(c.description, '') || ' ' || c.nom,
                    (SELECT rc.valeur FROM radar_contacts rc WHERE rc.curator_id = c.id AND rc.type = 'email' AND {}
                     ORDER BY rc.verifie DESC, rc.score DESC LIMIT 1)
             FROM radar_curators c
             WHERE c.kind = 'playlist' AND c.editorial = 0 AND c.statut NOT IN ('rejete', 'rejected')
             ORDER BY c.score DESC, c.followers DESC LIMIT 600",
            crate::commands::radar::pitchable_sql("rc.valeur")
        );
        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let mut rows: Vec<(i64, String, Option<String>, Option<String>, Option<i64>, String)> = stmt
            .query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, Option<i64>>(3)?, r.get::<_, String>(4)?, r.get::<_, Option<String>>(5)?))
            })
            .map_err(|e| e.to_string())?
            .filter_map(Result::ok)
            .filter_map(|(nom, owner, url, followers, text, email)| {
                let email = email?;
                let text = text.to_lowercase();
                let fit = words.iter().filter(|w| text.contains(w.as_str())).count() as i64;
                Some((fit, nom, owner, url, followers, email))
            })
            .collect();
        // Only playlists that name the genre, unless none does.
        if rows.iter().any(|r| r.0 > 0) {
            rows.retain(|r| r.0 > 0);
        }
        rows.sort_by(|a, b| b.0.cmp(&a.0));
        let mut added = 0;
        let mut seen_mail = std::collections::HashSet::new();
        for (_, nom, owner, url, followers, email) in rows {
            if added >= 25 {
                break;
            }
            let Some(email) = enrich::clean_email(&email) else { continue };
            if !seen_mail.insert(email.clone()) || crate::commands::outbox::suppression_reason(&conn, &email).is_some() {
                continue;
            }
            // One mail per address across the whole release.
            let already: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM release_targets t JOIN release_cards c ON c.id = t.card_id WHERE c.release_id = ?1 AND lower(t.email) = ?2",
                    params![card.release_id, email],
                    |r| r.get(0),
                )
                .unwrap_or(0);
            if already > 0 {
                continue;
            }
            let why = match (owner, followers) {
                (Some(o), Some(f)) => format!("Playlist de {o}, {f} abonnés"),
                (Some(o), None) => format!("Playlist de {o}"),
                (None, Some(f)) => format!("{f} abonnés"),
                _ => "Playlist du Radar".into(),
            };
            let n = conn
                .execute(
                    "INSERT OR IGNORE INTO release_targets (card_id, name, why, email, source_url, channel) VALUES (?1, ?2, ?3, ?4, ?5, 'email')",
                    params![card.id, nom, why, email, url],
                )
                .unwrap_or(0);
            added += n;
        }
        Ok(added)
    }

    /// DJs worth sending this release to. Names only: every contact detail
    /// comes from the search that follows.
    fn suggest_djs(pool: &DbPool, card: &Card, release: &Release) -> Result<Vec<String>, String> {
        let conn = pool.get().map_err(|e| e.to_string())?;
        let llm = claude::client(&conn).ok_or("Clé Claude absente (Réglages) : ajoutez des noms de DJs à la main.")?;
        let known: Vec<String> = conn
            .prepare("SELECT t.name FROM release_targets t JOIN release_cards c ON c.id = t.card_id WHERE c.release_id = ?1")
            .and_then(|mut s| s.query_map([card.release_id], |r| r.get::<_, String>(0)).map(|rows| rows.filter_map(Result::ok).collect()))
            .unwrap_or_default();
        let req = LlmRequest {
            model: claude::drafting_model(&conn),
            max_tokens: 900,
            system: vec![SystemBlock {
                text: "You help an independent label pick DJs for a promo mailing. Name working club DJs and DJ/producers who really play the given genre and who are within reach of a small label: active residents, rising names, respected selectors, radio show hosts. No superstar, no one retired, no invented name. \
Answer with JSON only: {\"djs\":[{\"name\":\"...\",\"why\":\"one short factual clause in French\"}]}".into(),
                cache: false,
            }],
            user: format!(
                "Genre: {}\nRelease: {} by {}\nNotes: {}\nAlready on the list (do not repeat): {}\nGive {} names.",
                release.genre,
                release.name,
                release.artist,
                release.notes.clone().unwrap_or_else(|| "none".into()),
                if known.is_empty() { "nobody".into() } else { known.join(", ") },
                FIND_BATCH
            ),
            schema: None,
        };
        let resp = llm.complete(&req)?;
        claude::log_usage(&conn, "release_djs", None, &resp.model, &resp.usage);
        let v = extract_json(&resp.text).ok_or("Réponse de l'IA illisible.")?;
        let mut out = Vec::new();
        for d in v.get("djs").and_then(|d| d.as_array()).cloned().unwrap_or_default() {
            let Some(name) = d.get("name").and_then(|n| n.as_str()).map(str::trim).filter(|n| !n.is_empty() && n.len() < 60) else { continue };
            if known.iter().any(|k| k.eq_ignore_ascii_case(name)) {
                continue;
            }
            let why = d.get("why").and_then(|w| w.as_str()).map(|w| format!("Suggestion de l'IA : {}", w.trim()));
            let _ = conn.execute(
                "INSERT OR IGNORE INTO release_targets (card_id, name, why) VALUES (?1, ?2, ?3)",
                params![card.id, name, why],
            );
            out.push(name.to_string());
        }
        Ok(out)
    }

    fn search(key: &str, query: &str) -> Option<Value> {
        let client = reqwest::blocking::Client::builder().user_agent(enrich::CRAWL_UA).timeout(Duration::from_secs(10)).build().ok()?;
        let resp = match client.post("https://google.serper.dev/search").header("X-API-KEY", key).json(&json!({ "q": query, "num": 10 })).send() {
            Ok(r) => r,
            Err(e) => {
                enrich::serper_health::failed(None, &e.to_string());
                return None;
            }
        };
        if !resp.status().is_success() {
            let code = resp.status().as_u16();
            enrich::serper_health::failed(Some(code), &resp.text().unwrap_or_default());
            return None;
        }
        enrich::serper_health::ok();
        std::thread::sleep(Duration::from_millis(400));
        resp.json().ok()
    }

    /// (link, title, snippet) of every result of a search answer.
    pub(crate) fn results(d: &Value) -> Vec<(String, String, String)> {
        let mut out = Vec::new();
        if let Some(profiles) = d.pointer("/knowledgeGraph/profiles").and_then(|p| p.as_array()) {
            for p in profiles {
                out.push((p.get("link").and_then(|x| x.as_str()).unwrap_or_default().to_string(), String::new(), String::new()));
            }
        }
        for it in d.get("organic").and_then(|o| o.as_array()).cloned().unwrap_or_default() {
            let s = |k: &str| it.get(k).and_then(|x| x.as_str()).unwrap_or_default().to_string();
            out.push((s("link"), s("title"), s("snippet")));
        }
        out
    }

    /// A Trackstack inbox link in search results (tstack.app/name or trackstack.app/name).
    pub(crate) fn trackstack_in(items: &[(String, String, String)]) -> Option<String> {
        static RE: OnceLock<Regex> = OnceLock::new();
        let re = RE.get_or_init(|| Regex::new(r"(?i)\b(?:https?://)?(?:www\.)?(tstack\.app|trackstack\.app)/([a-z0-9][a-z0-9._-]{1,40})\b").unwrap());
        const NOT_INBOX: &[&str] = &["products", "pricing", "blog", "manifesto", "terms", "privacy", "login", "signup", "help", "about", "download", "discover"];
        for (link, title, snippet) in items {
            for text in [link, title, snippet] {
                for c in re.captures_iter(text) {
                    let slug = c[2].trim_end_matches('.').to_lowercase();
                    if !NOT_INBOX.contains(&slug.as_str()) {
                        return Some(format!("https://tstack.app/{slug}"));
                    }
                }
            }
        }
        None
    }

    /// The first usable address printed in results that name the person.
    pub(crate) fn email_in(items: &[(String, String, String)], name: &str, mx: &mut MxCheck) -> Option<(String, String)> {
        static RE: OnceLock<Regex> = OnceLock::new();
        let re = RE.get_or_init(|| Regex::new(r"(?i)[a-z0-9][a-z0-9._%+\-]*@[a-z0-9][a-z0-9.\-]*\.[a-z]{2,}").unwrap());
        let key: String = name.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();
        for (link, title, snippet) in items {
            let hay: String = format!("{title} {snippet} {link}").to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();
            if key.len() < 3 || !hay.contains(&key) {
                continue;
            }
            for m in re.find_iter(snippet) {
                let Some(email) = enrich::clean_email(m.as_str()) else { continue };
                if enrich::is_placeholder_email(&email) || enrich::is_non_pitch_email(&email) || enrich::is_denied_email_domain(enrich::email_domain(&email)) {
                    continue;
                }
                if mx.has_mx(enrich::email_domain(&email)) == Some(true) {
                    return Some((email, link.clone()));
                }
            }
        }
        None
    }

    /// Looks up the contact details of named people: Instagram, Trackstack, a public address.
    fn lookup(app: &AppHandle, pool: &DbPool, card: &Card, release: &Release, names: Vec<String>, role: &str) -> Result<usize, String> {
        let names: Vec<String> = names.iter().map(|n| n.trim().to_string()).filter(|n| !n.is_empty() && n.len() < 60).take(FIND_BATCH).collect();
        if names.is_empty() {
            return Ok(0);
        }
        let key = {
            let conn = pool.get().map_err(|e| e.to_string())?;
            for n in &names {
                let _ = conn.execute("INSERT OR IGNORE INTO release_targets (card_id, name, why) VALUES (?1, ?2, 'Ajouté à la main')", params![card.id, n]);
            }
            crate::secrets::read_setting(&conn, "serper_api_key").filter(|k| !k.trim().is_empty())
        };
        let Some(key) = key else {
            return Err("Clé de recherche (Serper) absente : les contacts ne peuvent pas être cherchés. Saisissez-les à la main.".into());
        };
        let resolver = DohResolver::new().map_err(|e| e.to_string())?;
        let mut mx = MxCheck::new(&resolver);
        let total = names.len();
        let mut found = 0;
        for (i, name) in names.iter().enumerate() {
            emit(app, Progress { card_id: card.id, stage: "find".into(), label: name.clone(), done: i, total, ..Default::default() });
            let mut handle = None;
            let mut stack = None;
            let mut email = None;
            if let Some(d) = search(&key, &format!("\"{name}\" {role} {} instagram", release.genre)) {
                let items = results(&d);
                handle = social::instagram_from_search(&d, name, None);
                stack = trackstack_in(&items);
                email = email_in(&items, name, &mut mx);
            }
            if email.is_none() || stack.is_none() {
                if let Some(d) = search(&key, &format!("\"{name}\" {role} promo OR demo OR contact email OR trackstack")) {
                    let items = results(&d);
                    stack = stack.or_else(|| trackstack_in(&items));
                    email = email.or_else(|| email_in(&items, name, &mut mx));
                    handle = handle.or_else(|| social::instagram_from_search(&d, name, None));
                }
            }
            let conn = pool.get().map_err(|e| e.to_string())?;
            let email = email.filter(|(e, _)| crate::commands::outbox::suppression_reason(&conn, e).is_none());
            let channel = if email.is_some() {
                "email"
            } else if handle.is_some() {
                "instagram"
            } else {
                "none"
            };
            if channel != "none" || stack.is_some() {
                found += 1;
            }
            let _ = conn.execute(
                "UPDATE release_targets SET email = COALESCE(?3, email), instagram = COALESCE(?4, instagram), trackstack = COALESCE(?5, trackstack),
                        source_url = COALESCE(?6, source_url), channel = CASE WHEN channel = 'none' THEN ?7 ELSE channel END
                 WHERE card_id = ?1 AND name = ?2 AND status IN ('found', 'draft')",
                params![card.id, name, email.as_ref().map(|e| &e.0), handle, stack, email.as_ref().map(|e| &e.1), channel],
            );
        }
        Ok(found)
    }

    /// One draft per person who can be written to and has none yet.
    fn drafts(app: &AppHandle, pool: &DbPool, card: &Card, release: &Release) -> Result<(), String> {
        let conn = pool.get().map_err(|e| e.to_string())?;
        let todo: Vec<Target> = load_targets(&conn, card.id)?.into_iter().filter(|t| t.status == "found" && t.channel != "none").collect();
        if todo.is_empty() {
            return Ok(());
        }
        let llm = claude::client(&conn).ok_or("Clé Claude absente (Réglages) : les messages ne peuvent pas être rédigés.")?;
        let model = claude::drafting_model(&conn);
        let lang = if card.data.get("lang").and_then(|l| l.as_str()) == Some("fr") { "French" } else { "English" };
        let link = release.listen_url.clone().unwrap_or_else(|| "[LIEN]".into());
        let (who, ask) = if card.kind == "dj" {
            ("a DJ", "Offer the track as a promo before its release, in case it fits their sets. No request for feedback forms, no pressure.")
        } else {
            ("a playlist curator", "Suggest the track for their playlist, naming the playlist once. No pressure.")
        };
        let system = format!(
            "You write a short first message from an artist who releases on his own label to {who}. {ask}\n\
It is typed by the artist himself (Thibault, who plays as Piers). Write in {lang}.\nRules:\n{HOUSE_VOICE}\n\
Never invent a fact, a support, a figure or a past contact. Do not pretend to know the person's work beyond what is given.\n\
Answer with JSON only:\n\
{{\"subject\":\"email subject, 3 to 6 words, no capitals shouting\",\
\"email\":\"the email, 50 to 90 words, greeting with the first name or the name, the listening link alone on its own line, signed Thibault (Piers)\",\
\"dm\":\"the same as one Instagram message of 30 to 50 words in a single paragraph, the link at the very end\",\
\"followup\":\"a follow-up of one or two sentences for a week later, with the link again on its own line\"}}"
        );
        let total = todo.len();
        for (i, t) in todo.iter().enumerate() {
            emit(app, Progress { card_id: card.id, stage: "draft".into(), label: t.name.clone(), done: i, total, ..Default::default() });
            let req = LlmRequest {
                model: model.clone(),
                max_tokens: 900,
                system: vec![SystemBlock { text: system.clone(), cache: true }],
                user: format!(
                    "Recipient: {}\nWhat is known: {}\nTrack: {}\nArtist: {}\nLabel: insrt\nGenre: {}\nState: {}\nRelease date: {}\nListening link: {}\nNotes from the artist: {}",
                    t.name,
                    t.why.clone().unwrap_or_else(|| "nothing more".into()),
                    release.name,
                    release.artist,
                    release.genre,
                    if release.master_state == "master" { "mastered" } else { "not mastered yet" },
                    release.release_date.clone().unwrap_or_default(),
                    link,
                    release.notes.clone().unwrap_or_else(|| "none".into()),
                ),
                schema: None,
            };
            let resp = match llm.complete(&req) {
                Ok(r) => r,
                Err(e) => return Err(format!("Rédaction interrompue : {e}")),
            };
            claude::log_usage(&conn, "release_draft", None, &resp.model, &resp.usage);
            let Some(v) = extract_json(&resp.text) else { continue };
            let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(|x| claude::no_dashes(x.trim())).filter(|x| !x.is_empty());
            let _ = conn.execute(
                "UPDATE release_targets SET subject = ?2, body = ?3, dm = ?4, followup = ?5, status = 'draft' WHERE id = ?1 AND status = 'found'",
                params![t.id, s("subject"), s("email"), s("dm").map(|d| one_line(&d)), s("followup")],
            );
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn trackstack_links() {
            let items = vec![
                ("https://trackstack.app/pricing".to_string(), "Pricing".to_string(), String::new()),
                ("https://www.instagram.com/someone".to_string(), "Someone".to_string(), "Promos via tstack.app/someone-dj. Bookings elsewhere".to_string()),
            ];
            assert_eq!(trackstack_in(&items).as_deref(), Some("https://tstack.app/someone-dj"));
            assert_eq!(trackstack_in(&items[..1]), None);
        }

        #[test]
        fn dm_is_one_paragraph() {
            assert_eq!(one_line("Hello\n\nthere  you\nlink"), "Hello there you link");
            assert!(unfinished("écoute ici [LIEN]"));
            assert!(!unfinished("écoute ici https://insrt.studio/p/abc"));
        }
    }
}
