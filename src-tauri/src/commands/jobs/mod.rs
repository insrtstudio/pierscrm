//! Job search desk ("Emploi"): the app looks for new postings every day,
//! condenses each one into a line, prepares a CV adapted to the posting and an
//! application email, and waits for one click before anything leaves.
//!
//! Rules of the module:
//! - nothing is sent without the user's click (applications and follow-ups);
//! - a CV only ever states facts held in the profile (`job_profile` setting):
//!   dates, employers and figures are copied from it, never written by the AI;
//! - an address is only ever copied from a page and MX-checked, never guessed;
//! - postings come from public APIs and feeds, no site protection is bypassed;
//! - personal data lives in the local database only, never in this repository.

pub mod apply;
pub mod ats;
pub mod brief;
pub mod cv;
pub mod daily;
pub mod sources;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::AppState;

pub const EVENT: &str = "jobs:progress";

pub fn migrate(conn: &Connection) {
    let _ = conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS job_posts (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            source        TEXT NOT NULL DEFAULT 'manual',
            source_key    TEXT NOT NULL UNIQUE,
            url           TEXT,
            title         TEXT NOT NULL,
            company       TEXT NOT NULL DEFAULT '',
            location      TEXT NOT NULL DEFAULT '',
            country       TEXT NOT NULL DEFAULT '',
            domain        TEXT NOT NULL DEFAULT '',
            contract      TEXT,
            salary        TEXT,
            posted_at     TEXT,
            found_at      TEXT NOT NULL DEFAULT (datetime('now')),
            description   TEXT NOT NULL DEFAULT '',
            apply_kind    TEXT NOT NULL DEFAULT 'unknown',
            apply_email   TEXT,
            apply_url     TEXT,
            status        TEXT NOT NULL DEFAULT 'new',
            score         INTEGER,
            line          TEXT,
            fit           TEXT,
            company_brief TEXT,
            company_kind  TEXT,
            company_site  TEXT,
            roster        TEXT,
            contact_name  TEXT,
            contact_role  TEXT,
            contact_email TEXT,
            contact_source TEXT,
            lang          TEXT,
            next_action   TEXT,
            next_action_at TEXT,
            notes         TEXT,
            analysed_at   TEXT,
            briefed_at    TEXT,
            updated_at    TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_job_posts_status ON job_posts(status);
        CREATE TABLE IF NOT EXISTS job_applications (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            post_id       INTEGER NOT NULL UNIQUE REFERENCES job_posts(id) ON DELETE CASCADE,
            cv_json       TEXT,
            cv_path       TEXT,
            cv_warnings   TEXT,
            ats           TEXT,
            subject       TEXT NOT NULL DEFAULT '',
            body          TEXT NOT NULL DEFAULT '',
            followup_body TEXT NOT NULL DEFAULT '',
            state         TEXT NOT NULL DEFAULT 'draft',
            email_id      INTEGER,
            message_id    TEXT,
            sent_at       TEXT,
            followup_due  TEXT,
            followup_email_id INTEGER,
            followup_sent_at TEXT,
            created_at    TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE TABLE IF NOT EXISTS job_searches (
            id      INTEGER PRIMARY KEY AUTOINCREMENT,
            domain  TEXT NOT NULL,
            query   TEXT NOT NULL,
            country TEXT NOT NULL,
            active  INTEGER NOT NULL DEFAULT 1,
            UNIQUE(query, country)
        );
        CREATE TABLE IF NOT EXISTS job_boards (
            id       INTEGER PRIMARY KEY AUTOINCREMENT,
            platform TEXT NOT NULL,
            token    TEXT NOT NULL,
            company  TEXT NOT NULL DEFAULT '',
            active   INTEGER NOT NULL DEFAULT 1,
            last_ok  TEXT,
            last_error TEXT,
            UNIQUE(platform, token)
        );
        CREATE TABLE IF NOT EXISTS job_runs (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            started_at  TEXT NOT NULL DEFAULT (datetime('now')),
            finished_at TEXT,
            found       INTEGER NOT NULL DEFAULT 0,
            added       INTEGER NOT NULL DEFAULT 0,
            analysed    INTEGER NOT NULL DEFAULT 0,
            prepared    INTEGER NOT NULL DEFAULT 0,
            notes       TEXT
        );",
    );
    sources::seed(conn);
}

// ---------------- Settings ----------------

pub(crate) fn setting(conn: &Connection, key: &str) -> Option<String> {
    crate::secrets::read_setting(conn, key).filter(|v| !v.trim().is_empty())
}

pub(crate) fn put_setting(conn: &Connection, key: &str, value: &str) {
    let _ = crate::secrets::write_setting(conn, key, value);
}

pub(crate) fn setting_i64(conn: &Connection, key: &str, default: i64) -> i64 {
    setting(conn, key).and_then(|v| v.trim().parse().ok()).unwrap_or(default)
}

/// Lowest score at which an application is prepared without being asked.
pub(crate) fn min_score(conn: &Connection) -> i64 {
    setting_i64(conn, "jobs_min_score", 65).clamp(0, 100)
}

/// Applications prepared per daily run at most (AI cost and reading time).
pub(crate) fn daily_prepare(conn: &Connection) -> i64 {
    setting_i64(conn, "jobs_daily_prepare", 6).clamp(0, 30)
}

pub(crate) fn followup_days(conn: &Connection) -> i64 {
    setting_i64(conn, "jobs_followup_days", 7).clamp(2, 30)
}

// ---------------- Rows ----------------

#[derive(Debug, Clone, Serialize, Default)]
pub struct JobRow {
    pub id: i64,
    pub source: String,
    pub url: Option<String>,
    pub title: String,
    pub company: String,
    pub location: String,
    pub country: String,
    pub domain: String,
    pub contract: Option<String>,
    pub salary: Option<String>,
    pub posted_at: Option<String>,
    pub found_at: String,
    pub apply_kind: String,
    pub apply_email: Option<String>,
    pub apply_url: Option<String>,
    pub status: String,
    pub score: Option<i64>,
    pub line: Option<String>,
    pub company_brief: Option<String>,
    pub company_kind: Option<String>,
    pub company_site: Option<String>,
    pub roster: Option<String>,
    pub contact_name: Option<String>,
    pub contact_role: Option<String>,
    pub contact_email: Option<String>,
    pub contact_source: Option<String>,
    pub lang: Option<String>,
    pub next_action: Option<String>,
    pub next_action_at: Option<String>,
    pub notes: Option<String>,
    // Application, when one exists.
    pub app_state: Option<String>,
    pub ats_score: Option<i64>,
    pub sent_at: Option<String>,
    pub opened_at: Option<String>,
    pub open_count: i64,
    pub tracked: bool,
    pub replied_at: Option<String>,
    pub followup_due: Option<String>,
    pub followup_sent_at: Option<String>,
}

const ROW_SQL: &str = "SELECT p.id, p.source, p.url, p.title, p.company, p.location, p.country, p.domain, p.contract, p.salary,
        p.posted_at, p.found_at, p.apply_kind, p.apply_email, p.apply_url, p.status, p.score, p.line, p.company_brief,
        p.company_kind, p.company_site, p.roster, p.contact_name, p.contact_role, p.contact_email, p.contact_source,
        p.lang, p.next_action, p.next_action_at, p.notes,
        a.state, CAST(json_extract(a.ats, '$.score') AS INTEGER), a.sent_at,
        (SELECT MIN(e.opened_at) FROM emails e WHERE e.id IN (a.email_id, a.followup_email_id)),
        COALESCE((SELECT SUM(e.open_count) FROM emails e WHERE e.id IN (a.email_id, a.followup_email_id)), 0),
        EXISTS (SELECT 1 FROM emails e WHERE e.id = a.email_id AND e.track_token IS NOT NULL),
        (SELECT MIN(COALESCE(r.received_at, r.created_at)) FROM email_replies r
           WHERE r.email_id IN (a.email_id, a.followup_email_id) AND r.kind = 'reply'),
        a.followup_due, a.followup_sent_at
     FROM job_posts p LEFT JOIN job_applications a ON a.post_id = p.id";

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<JobRow> {
    Ok(JobRow {
        id: r.get(0)?,
        source: r.get(1)?,
        url: r.get(2)?,
        title: r.get(3)?,
        company: r.get(4)?,
        location: r.get(5)?,
        country: r.get(6)?,
        domain: r.get(7)?,
        contract: r.get(8)?,
        salary: r.get(9)?,
        posted_at: r.get(10)?,
        found_at: r.get(11)?,
        apply_kind: r.get(12)?,
        apply_email: r.get(13)?,
        apply_url: r.get(14)?,
        status: r.get(15)?,
        score: r.get(16)?,
        line: r.get(17)?,
        company_brief: r.get(18)?,
        company_kind: r.get(19)?,
        company_site: r.get(20)?,
        roster: r.get(21)?,
        contact_name: r.get(22)?,
        contact_role: r.get(23)?,
        contact_email: r.get(24)?,
        contact_source: r.get(25)?,
        lang: r.get(26)?,
        next_action: r.get(27)?,
        next_action_at: r.get(28)?,
        notes: r.get(29)?,
        app_state: r.get(30)?,
        ats_score: r.get(31)?,
        sent_at: r.get(32)?,
        opened_at: r.get(33)?,
        open_count: r.get(34)?,
        tracked: r.get(35)?,
        replied_at: r.get(36)?,
        followup_due: r.get(37)?,
        followup_sent_at: r.get(38)?,
    })
}

pub(crate) fn list(conn: &Connection) -> Result<Vec<JobRow>, String> {
    let sql = format!("{ROW_SQL} ORDER BY COALESCE(p.score, -1) DESC, p.found_at DESC, p.id DESC");
    let mut st = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = st.query_map([], row).map_err(|e| e.to_string())?;
    Ok(rows.filter_map(Result::ok).collect())
}

pub(crate) fn one(conn: &Connection, id: i64) -> Result<JobRow, String> {
    let sql = format!("{ROW_SQL} WHERE p.id = ?1");
    conn.query_row(&sql, params![id], row).map_err(|e| e.to_string())
}

/// Everything shown in the card of one posting.
#[derive(Serialize)]
pub struct JobDetail {
    pub job: JobRow,
    pub description: String,
    pub fit: Option<serde_json::Value>,
    pub application: Option<ApplicationView>,
}

#[derive(Serialize)]
pub struct ApplicationView {
    pub state: String,
    pub cv_path: Option<String>,
    pub cv: Option<serde_json::Value>,
    pub cv_warnings: Vec<String>,
    pub ats: Option<serde_json::Value>,
    pub subject: String,
    pub body: String,
    pub followup_body: String,
}

fn json(s: Option<String>) -> Option<serde_json::Value> {
    s.and_then(|s| serde_json::from_str(&s).ok())
}

pub(crate) fn detail(conn: &Connection, id: i64) -> Result<JobDetail, String> {
    let job = one(conn, id)?;
    let (description, fit): (String, Option<String>) = conn
        .query_row("SELECT description, fit FROM job_posts WHERE id = ?1", params![id], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(|e| e.to_string())?;
    let application = conn
        .query_row(
            "SELECT state, cv_path, cv_json, cv_warnings, ats, subject, body, followup_body FROM job_applications WHERE post_id = ?1",
            params![id],
            |r| {
                Ok(ApplicationView {
                    state: r.get(0)?,
                    cv_path: r.get(1)?,
                    cv: json(r.get(2)?),
                    cv_warnings: json(r.get(3)?).and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default(),
                    ats: json(r.get(4)?),
                    subject: r.get(5)?,
                    body: r.get(6)?,
                    followup_body: r.get(7)?,
                })
            },
        )
        .optional()
        .map_err(|e| e.to_string())?;
    Ok(JobDetail { job, description, fit: json(fit), application })
}

// ---------------- Dashboard ----------------

#[derive(Serialize, Default)]
pub struct Board {
    pub jobs: Vec<JobRow>,
    pub new_today: i64,
    pub to_validate: i64,
    pub sent: i64,
    pub opened: i64,
    pub replies: i64,
    pub followups_due: i64,
    pub last_run: Option<String>,
    pub last_run_notes: Option<String>,
    pub running: bool,
    pub ai_ready: bool,
    pub sender: Option<String>,
    pub profile_ready: bool,
    pub pdf_ready: bool,
    pub adzuna_ready: bool,
    pub search_ready: bool,
}

pub(crate) fn board(conn: &Connection) -> Result<Board, String> {
    let jobs = list(conn)?;
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let mut b = Board { ..Default::default() };
    for j in &jobs {
        // found_at is UTC; close enough for a "today" counter.
        if j.found_at.starts_with(&today) && j.status != "dismissed" {
            b.new_today += 1;
        }
        let sent = matches!(j.app_state.as_deref(), Some("sent") | Some("manual"));
        if j.app_state.as_deref() == Some("draft") && !matches!(j.status.as_str(), "dismissed" | "closed") {
            b.to_validate += 1;
        }
        if sent {
            b.sent += 1;
            b.opened += j.opened_at.is_some() as i64;
            b.replies += j.replied_at.is_some() as i64;
            if followup_is_due(j, &today) {
                b.followups_due += 1;
            }
        }
    }
    let run: Option<(Option<String>, Option<String>)> = conn
        .query_row("SELECT finished_at, notes FROM job_runs ORDER BY id DESC LIMIT 1", [], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()
        .map_err(|e| e.to_string())?;
    if let Some((at, notes)) = run {
        b.last_run = at;
        b.last_run_notes = notes;
    }
    b.running = daily::is_running();
    b.ai_ready = setting(conn, "anthropic_api_key").is_some();
    b.sender = crate::commands::email::labels_sender_address(conn).filter(|_| crate::commands::email::labels_sender_ready(conn));
    b.profile_ready = cv::load_profile(conn).is_some();
    b.pdf_ready = cv::browser_path().is_some();
    b.adzuna_ready = setting(conn, "adzuna_app_id").is_some() && setting(conn, "adzuna_app_key").is_some();
    b.search_ready = setting(conn, "serper_api_key").is_some();
    b.jobs = jobs;
    Ok(b)
}

/// A sent application with no answer, past its follow-up date, not yet nudged.
pub(crate) fn followup_is_due(j: &JobRow, today: &str) -> bool {
    matches!(j.app_state.as_deref(), Some("sent") | Some("manual"))
        && j.replied_at.is_none()
        && j.followup_sent_at.is_none()
        && !matches!(j.status.as_str(), "rejected" | "closed" | "dismissed" | "interview" | "offer")
        && j.followup_due.as_deref().map(|d| d <= today).unwrap_or(false)
}

// ---------------- Edits ----------------

#[derive(Deserialize)]
pub struct JobInput {
    pub id: Option<i64>,
    pub title: String,
    #[serde(default)]
    pub company: String,
    #[serde(default)]
    pub location: String,
    #[serde(default)]
    pub country: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub contract: Option<String>,
    #[serde(default)]
    pub salary: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub apply_email: Option<String>,
    #[serde(default)]
    pub contact_name: Option<String>,
    #[serde(default)]
    pub contact_email: Option<String>,
    #[serde(default)]
    pub next_action: Option<String>,
    #[serde(default)]
    pub next_action_at: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
}

fn clean(s: &Option<String>) -> Option<String> {
    s.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

pub(crate) fn save(conn: &Connection, input: &JobInput) -> Result<i64, String> {
    let title = input.title.trim();
    if title.is_empty() {
        return Err("Intitulé du poste manquant.".into());
    }
    let apply_email = clean(&input.apply_email).map(|e| e.to_lowercase());
    let country = match input.country.trim().to_uppercase().as_str() {
        c @ ("FR" | "UK" | "US") => c.to_string(),
        _ => sources::country_of(&input.location).unwrap_or("").to_string(),
    };
    match input.id {
        Some(id) => {
            conn.execute(
                "UPDATE job_posts SET title=?2, company=?3, location=?4, country=?5, url=?6, contract=?7, salary=?8,
                    description=COALESCE(?9, description), apply_email=?10,
                    apply_kind=CASE WHEN ?10 IS NOT NULL THEN 'email' WHEN apply_kind='email' THEN 'unknown' ELSE apply_kind END,
                    contact_name=?11, contact_email=?12, next_action=?13, next_action_at=?14, notes=?15,
                    updated_at=datetime('now')
                 WHERE id=?1",
                params![id, title, input.company.trim(), input.location.trim(), country, clean(&input.url),
                    clean(&input.contract), clean(&input.salary), clean(&input.description), apply_email,
                    clean(&input.contact_name), clean(&input.contact_email), clean(&input.next_action),
                    clean(&input.next_action_at), clean(&input.notes)],
            )
            .map_err(|e| e.to_string())?;
            Ok(id)
        }
        None => {
            let key = format!("manual:{}", crate::commands::deliverability::new_token(None));
            conn.execute(
                "INSERT INTO job_posts (source, source_key, title, company, location, country, url, contract, salary,
                    description, apply_email, apply_kind, contact_name, contact_email, next_action, next_action_at, notes, status)
                 VALUES ('manual', ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, COALESCE(?9, ''), ?10,
                    CASE WHEN ?10 IS NOT NULL THEN 'email' ELSE 'unknown' END, ?11, ?12, ?13, ?14, ?15, 'shortlisted')",
                params![key, title, input.company.trim(), input.location.trim(), country, clean(&input.url),
                    clean(&input.contract), clean(&input.salary), clean(&input.description), apply_email,
                    clean(&input.contact_name), clean(&input.contact_email), clean(&input.next_action),
                    clean(&input.next_action_at), clean(&input.notes)],
            )
            .map_err(|e| e.to_string())?;
            Ok(conn.last_insert_rowid())
        }
    }
}

pub const STATUSES: &[&str] = &["new", "shortlisted", "applied", "interview", "offer", "rejected", "dismissed", "closed"];

pub(crate) fn set_status(conn: &Connection, id: i64, status: &str) -> Result<(), String> {
    if !STATUSES.contains(&status) {
        return Err(format!("Statut inconnu : {status}"));
    }
    conn.execute("UPDATE job_posts SET status=?2, updated_at=datetime('now') WHERE id=?1", params![id, status])
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ---------------- Import of a tracking sheet ----------------

fn status_from_sheet(s: &str) -> &'static str {
    let s = s.to_lowercase();
    if s.contains("envoy") || s.contains("sent") || s.contains("postul") {
        "applied"
    } else if s.contains("entretien") || s.contains("interview") {
        "interview"
    } else if s.contains("refus") || s.contains("reject") {
        "rejected"
    } else {
        "shortlisted"
    }
}

/// Import a tracking sheet (CSV with the columns of `suivi_candidatures.csv`:
/// date, entreprise, poste, lieu, type_contrat, remuneration, statut,
/// cv_fichier, email_candidature, contact, langue, prochaine_action, notes).
/// A line already imported (same company and role) is left alone.
pub(crate) fn import_sheet(conn: &Connection, text: &str) -> Result<usize, String> {
    let mut rdr = csv::ReaderBuilder::new().flexible(true).from_reader(text.trim_start_matches('\u{feff}').as_bytes());
    let headers: Vec<String> = rdr.headers().map_err(|e| e.to_string())?.iter().map(|h| h.trim().to_lowercase()).collect();
    let col = |name: &str| headers.iter().position(|h| h == name);
    let (Some(c_company), Some(c_title)) = (col("entreprise"), col("poste")) else {
        return Err("Colonnes « entreprise » et « poste » introuvables.".into());
    };
    let mut added = 0;
    for rec in rdr.records().filter_map(Result::ok) {
        let get = |i: Option<usize>| i.and_then(|i| rec.get(i)).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
        let (Some(company), Some(title)) = (get(Some(c_company)), get(Some(c_title))) else { continue };
        let key = format!("sheet:{}:{}", company.to_lowercase(), title.to_lowercase());
        let location = get(col("lieu")).unwrap_or_default();
        let status = status_from_sheet(&get(col("statut")).unwrap_or_default());
        let contact = get(col("contact"));
        let email = contact.as_deref().filter(|c| c.contains('@')).map(|c| c.to_lowercase());
        let mut notes: Vec<String> = Vec::new();
        if let Some(n) = get(col("notes")) {
            notes.push(n);
        }
        if let Some(f) = get(col("cv_fichier")) {
            notes.push(format!("CV : {f}"));
        }
        if let Some(f) = get(col("lettre_fichier")) {
            notes.push(format!("Lettre : {f}"));
        }
        let n = conn
            .execute(
                "INSERT OR IGNORE INTO job_posts (source, source_key, title, company, location, country, contract, salary,
                    status, apply_email, apply_kind, contact_email, contact_name, lang, next_action, notes, found_at)
                 VALUES ('sheet', ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, CASE WHEN ?9 IS NOT NULL THEN 'email' ELSE 'unknown' END,
                    ?9, ?10, ?11, ?12, ?13, COALESCE(?14, datetime('now')))",
                params![key, title, company, location, sources::country_of(&location).unwrap_or(""),
                    get(col("type_contrat")), get(col("remuneration")), status, email,
                    contact.filter(|c| !c.contains('@')), get(col("langue")), get(col("prochaine_action")),
                    (!notes.is_empty()).then(|| notes.join("\n")), get(col("date"))],
            )
            .map_err(|e| e.to_string())?;
        added += n;
    }
    Ok(added)
}

// ---------------- Commands ----------------

#[tauri::command(async)]
pub fn jobs_board(state: State<'_, AppState>) -> Result<Board, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    board(&conn)
}

#[tauri::command(async)]
pub fn job_detail(state: State<'_, AppState>, id: i64) -> Result<JobDetail, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    detail(&conn, id)
}

#[tauri::command(async)]
pub fn job_save(state: State<'_, AppState>, input: JobInput) -> Result<i64, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    save(&conn, &input)
}

#[tauri::command(async)]
pub fn job_set_status(state: State<'_, AppState>, id: i64, status: String) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    set_status(&conn, id, &status)
}

#[tauri::command(async)]
pub fn job_delete(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM job_posts WHERE id = ?1", params![id]).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command(async)]
pub fn jobs_import_sheet(state: State<'_, AppState>, path: String) -> Result<usize, String> {
    let text = std::fs::read_to_string(&path).map_err(|e| format!("Lecture impossible : {e}"))?;
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    import_sheet(&conn, &text)
}

#[derive(Serialize, Deserialize)]
pub struct SearchRow {
    pub id: Option<i64>,
    pub domain: String,
    pub query: String,
    pub country: String,
    pub active: bool,
}

#[derive(Serialize, Deserialize)]
pub struct BoardRow {
    pub id: Option<i64>,
    pub platform: String,
    pub token: String,
    pub company: String,
    pub active: bool,
    #[serde(default)]
    pub last_ok: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
}

#[derive(Serialize)]
pub struct JobsConfig {
    pub searches: Vec<SearchRow>,
    pub boards: Vec<BoardRow>,
    pub enabled: bool,
    pub min_score: i64,
    pub daily_prepare: i64,
    pub followup_days: i64,
    pub profile: String,
    pub adzuna_app_id: String,
    pub adzuna_app_key: String,
}

#[tauri::command(async)]
pub fn jobs_config(state: State<'_, AppState>) -> Result<JobsConfig, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let mut st = conn
        .prepare("SELECT id, domain, query, country, active FROM job_searches ORDER BY domain, country, query")
        .map_err(|e| e.to_string())?;
    let searches = st
        .query_map([], |r| {
            Ok(SearchRow { id: r.get(0)?, domain: r.get(1)?, query: r.get(2)?, country: r.get(3)?, active: r.get(4)? })
        })
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();
    let mut st = conn
        .prepare("SELECT id, platform, token, company, active, last_ok, last_error FROM job_boards ORDER BY company, token")
        .map_err(|e| e.to_string())?;
    let boards = st
        .query_map([], |r| {
            Ok(BoardRow {
                id: r.get(0)?,
                platform: r.get(1)?,
                token: r.get(2)?,
                company: r.get(3)?,
                active: r.get(4)?,
                last_ok: r.get(5)?,
                last_error: r.get(6)?,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();
    Ok(JobsConfig {
        searches,
        boards,
        enabled: setting(&conn, "jobs_enabled").as_deref() != Some("0"),
        min_score: min_score(&conn),
        daily_prepare: daily_prepare(&conn),
        followup_days: followup_days(&conn),
        profile: setting(&conn, "job_profile").unwrap_or_else(|| cv::PROFILE_SKELETON.to_string()),
        adzuna_app_id: crate::secrets::ui_setting(&conn, "adzuna_app_id").unwrap_or_default(),
        adzuna_app_key: crate::secrets::ui_setting(&conn, "adzuna_app_key").unwrap_or_default(),
    })
}

#[tauri::command(async)]
pub fn jobs_save_search(state: State<'_, AppState>, row: SearchRow) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let country = row.country.trim().to_uppercase();
    if row.query.trim().is_empty() || !matches!(country.as_str(), "FR" | "UK" | "US") {
        return Err("Mots-clés et pays (FR, UK, US) requis.".into());
    }
    match row.id {
        Some(id) => conn.execute(
            "UPDATE job_searches SET domain=?2, query=?3, country=?4, active=?5 WHERE id=?1",
            params![id, row.domain.trim(), row.query.trim(), country, row.active],
        ),
        None => conn.execute(
            "INSERT OR IGNORE INTO job_searches (domain, query, country, active) VALUES (?1, ?2, ?3, ?4)",
            params![row.domain.trim(), row.query.trim(), country, row.active],
        ),
    }
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command(async)]
pub fn jobs_delete_search(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM job_searches WHERE id = ?1", params![id]).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command(async)]
pub fn jobs_save_board(state: State<'_, AppState>, row: BoardRow) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let platform = row.platform.trim().to_lowercase();
    if !sources::PLATFORMS.contains(&platform.as_str()) || row.token.trim().is_empty() {
        return Err("Plateforme ou identifiant invalide.".into());
    }
    match row.id {
        Some(id) => conn.execute(
            "UPDATE job_boards SET platform=?2, token=?3, company=?4, active=?5 WHERE id=?1",
            params![id, platform, row.token.trim(), row.company.trim(), row.active],
        ),
        None => conn.execute(
            "INSERT OR IGNORE INTO job_boards (platform, token, company, active) VALUES (?1, ?2, ?3, ?4)",
            params![platform, row.token.trim(), row.company.trim(), row.active],
        ),
    }
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command(async)]
pub fn jobs_delete_board(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM job_boards WHERE id = ?1", params![id]).map_err(|e| e.to_string())?;
    Ok(())
}

#[derive(Deserialize)]
pub struct JobsPrefs {
    pub enabled: bool,
    pub min_score: i64,
    pub daily_prepare: i64,
    pub followup_days: i64,
    #[serde(default)]
    pub adzuna_app_id: Option<String>,
    #[serde(default)]
    pub adzuna_app_key: Option<String>,
}

#[tauri::command(async)]
pub fn jobs_save_prefs(state: State<'_, AppState>, prefs: JobsPrefs) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    put_setting(&conn, "jobs_enabled", if prefs.enabled { "1" } else { "0" });
    put_setting(&conn, "jobs_min_score", &prefs.min_score.clamp(0, 100).to_string());
    put_setting(&conn, "jobs_daily_prepare", &prefs.daily_prepare.clamp(0, 30).to_string());
    put_setting(&conn, "jobs_followup_days", &prefs.followup_days.clamp(2, 30).to_string());
    if let Some(v) = prefs.adzuna_app_id {
        put_setting(&conn, "adzuna_app_id", v.trim());
    }
    if let Some(v) = prefs.adzuna_app_key {
        put_setting(&conn, "adzuna_app_key", v.trim());
    }
    Ok(())
}

/// Save the profile (the only source of facts for every CV). Refused when it
/// is not valid JSON of the expected shape, so a typo never breaks tailoring.
#[tauri::command(async)]
pub fn jobs_save_profile(state: State<'_, AppState>, profile: String) -> Result<(), String> {
    let parsed: cv::Profile = serde_json::from_str(&profile).map_err(|e| format!("Profil illisible : {e}"))?;
    if parsed.identity.name.trim().is_empty() || parsed.experience.is_empty() {
        return Err("Le profil doit au moins porter un nom et une expérience.".into());
    }
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    put_setting(&conn, "job_profile", &profile);
    Ok(())
}

#[cfg(test)]
pub(crate) fn test_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);
         CREATE TABLE emails (id INTEGER PRIMARY KEY, contact_id INTEGER, campaign_id INTEGER, to_addr TEXT, subject TEXT,
            body TEXT, status TEXT, error TEXT, track_token TEXT, opened_at TEXT, open_count INTEGER NOT NULL DEFAULT 0,
            sent_at TEXT, message_id TEXT);
         CREATE TABLE email_replies (id INTEGER PRIMARY KEY, email_id INTEGER, kind TEXT, received_at TEXT, created_at TEXT);",
    )
    .unwrap();
    migrate(&conn);
    conn
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHEET: &str = "\u{feff}id,date,entreprise,poste,lieu,type_contrat,remuneration,statut,cv_fichier,lettre_fichier,email_candidature,contact,langue,prochaine_action,notes\n\
1,2026-10-29,Acme Publishing,Sync Manager,Paris,,,CV prêt,cv_a.pdf,,,,FR/EN,Envoyer la candidature,Maison de publishing\n\
3,2026-11-18,Agency Corp,Talent Agent,\"Miami, USA (sur site)\",Temps plein,Fixe + commission,Candidature envoyée,cv_b.pdf,,Oui,jobs@agency.test,EN,Relancer,Visa à prévoir\n";

    #[test]
    fn sheet_import_maps_status_country_and_contact_once() {
        let conn = test_db();
        assert_eq!(import_sheet(&conn, SHEET).unwrap(), 2);
        assert_eq!(import_sheet(&conn, SHEET).unwrap(), 0, "a second import adds nothing");
        let jobs = list(&conn).unwrap();
        let agent = jobs.iter().find(|j| j.title == "Talent Agent").unwrap();
        assert_eq!(agent.status, "applied");
        assert_eq!(agent.country, "US");
        assert_eq!(agent.apply_email.as_deref(), Some("jobs@agency.test"));
        assert_eq!(agent.apply_kind, "email");
        let sync = jobs.iter().find(|j| j.title == "Sync Manager").unwrap();
        assert_eq!(sync.status, "shortlisted");
        assert_eq!(sync.country, "FR");
        assert!(sync.notes.as_deref().unwrap().contains("cv_a.pdf"));
    }

    #[test]
    fn manual_posting_and_status() {
        let conn = test_db();
        let id = save(
            &conn,
            &JobInput {
                id: None,
                title: "Label Manager".into(),
                company: "Indie".into(),
                location: "London".into(),
                country: String::new(),
                url: None,
                contract: None,
                salary: None,
                description: Some("Run releases".into()),
                apply_email: Some("Jobs@Indie.test".into()),
                contact_name: None,
                contact_email: None,
                next_action: None,
                next_action_at: None,
                notes: None,
            },
        )
        .unwrap();
        let j = one(&conn, id).unwrap();
        assert_eq!((j.country.as_str(), j.apply_kind.as_str()), ("UK", "email"));
        assert_eq!(j.apply_email.as_deref(), Some("jobs@indie.test"));
        assert!(set_status(&conn, id, "nope").is_err());
        set_status(&conn, id, "interview").unwrap();
        assert_eq!(one(&conn, id).unwrap().status, "interview");
    }

    #[test]
    fn followup_due_only_when_sent_unanswered_and_past_date() {
        let mut j = JobRow { app_state: Some("sent".into()), followup_due: Some("2026-11-20".into()), status: "applied".into(), ..Default::default() };
        assert!(followup_is_due(&j, "2026-11-20"));
        assert!(!followup_is_due(&j, "2026-11-19"));
        j.replied_at = Some("2026-11-19".into());
        assert!(!followup_is_due(&j, "2026-11-25"));
        j.replied_at = None;
        j.followup_sent_at = Some("2026-11-21".into());
        assert!(!followup_is_due(&j, "2026-11-25"));
    }
}
