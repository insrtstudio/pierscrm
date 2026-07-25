use crate::models::SmtpConfig;
use crate::AppState;
use lettre::message::{header::ContentType, Mailbox};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{Message, SmtpTransport, Transport};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use tauri::State;

static TOKEN_COUNTER: AtomicU64 = AtomicU64::new(1);

fn setting(conn: &rusqlite::Connection, key: &str) -> Option<String> {
    conn.query_row(
        "SELECT value FROM settings WHERE key = ?1",
        params![key],
        |r| r.get::<_, String>(0),
    )
    .ok()
    .filter(|s| !s.trim().is_empty())
}

fn load_smtp(state: &AppState) -> Result<SmtpConfig, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    match setting(&conn, "smtp") {
        Some(j) => serde_json::from_str(&j).map_err(|e| e.to_string()),
        None => Ok(SmtpConfig::default()),
    }
}

fn build_transport(cfg: &SmtpConfig) -> Result<SmtpTransport, String> {
    if cfg.host.trim().is_empty() {
        return Err("SMTP host is not configured.".into());
    }
    let creds = Credentials::new(cfg.username.clone(), cfg.password.clone());
    let builder = match cfg.encryption.as_str() {
        "tls" => SmtpTransport::relay(&cfg.host).map_err(|e| e.to_string())?,
        "none" => SmtpTransport::builder_dangerous(&cfg.host),
        _ => SmtpTransport::starttls_relay(&cfg.host).map_err(|e| e.to_string())?,
    };
    Ok(builder.port(cfg.port).credentials(creds).build())
}

fn gen_token() -> String {
    let n = chrono::Utc::now().timestamp_micros() as u64;
    let c = TOKEN_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{:x}{:x}", n, c)
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[tauri::command]
pub fn get_smtp_config(state: State<AppState>) -> Result<SmtpConfig, String> {
    load_smtp(&state)
}

#[tauri::command]
pub fn save_smtp_config(state: State<AppState>, config: SmtpConfig) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let json = serde_json::to_string(&config).map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO settings (key, value) VALUES ('smtp', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![json],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn test_smtp(state: State<AppState>) -> Result<bool, String> {
    let cfg = load_smtp(&state)?;
    let transport = build_transport(&cfg)?;
    transport.test_connection().map_err(|e| e.to_string())
}

#[derive(Serialize)]
pub struct SendResult {
    pub ok: bool,
    pub error: Option<String>,
    pub tracked: bool,
}

#[tauri::command]
pub fn send_email(
    state: State<AppState>,
    contact_id: Option<i64>,
    to: String,
    subject: String,
    body: String,
) -> Result<SendResult, String> {
    let cfg = load_smtp(&state)?;
    if cfg.from_email.trim().is_empty() {
        return Err("Sender email is not configured (Settings → Email).".into());
    }

    let from_mbox: Mailbox = format!("{} <{}>", cfg.from_name, cfg.from_email)
        .parse()
        .or_else(|_| cfg.from_email.parse())
        .map_err(|_| "Invalid sender email address".to_string())?;
    let to_mbox: Mailbox = to
        .trim()
        .parse()
        .map_err(|_| format!("Invalid recipient address: {}", to))?;

    // Open-tracking: only if a public tracking base URL is configured.
    let token = gen_token();
    let tracking_base = {
        let conn = state.pool.get().map_err(|e| e.to_string())?;
        setting(&conn, "tracking_base_url")
    };
    let tracked = tracking_base.is_some();

    let builder = Message::builder()
        .from(from_mbox)
        .to(to_mbox)
        .subject(subject.clone());

    let email = if let Some(base) = &tracking_base {
        let base = base.trim_end_matches('/');
        let pixel = format!(
            "<img src=\"{}/o/{}.gif\" width=\"1\" height=\"1\" alt=\"\" style=\"display:none\"/>",
            base, token
        );
        let html = format!(
            "<div style=\"font-family:Arial,Helvetica,sans-serif;font-size:14px;line-height:1.5;color:#111\">{}</div>{}",
            html_escape(&body).replace('\n', "<br/>"),
            pixel
        );
        builder
            .header(ContentType::TEXT_HTML)
            .body(html)
            .map_err(|e| e.to_string())?
    } else {
        builder
            .header(ContentType::TEXT_PLAIN)
            .body(body.clone())
            .map_err(|e| e.to_string())?
    };

    let transport = build_transport(&cfg)?;
    let (status, error) = match transport.send(&email) {
        Ok(_) => ("sent", None),
        Err(e) => ("failed", Some(e.to_string())),
    };

    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO emails (contact_id, to_addr, subject, body, status, error, track_token)
         VALUES (?1,?2,?3,?4,?5,?6,?7)",
        params![contact_id, to, subject, body, status, error, token],
    )
    .map_err(|e| e.to_string())?;

    if status == "sent" {
        if let Some(cid) = contact_id {
            conn.execute(
                "UPDATE contacts
                 SET status = CASE WHEN status IN ('to_contact','to_evaluate','low_priority') THEN 'contacted' ELSE status END,
                     first_contact = COALESCE(first_contact, date('now')),
                     updated_at = datetime('now')
                 WHERE id = ?1",
                params![cid],
            )
            .map_err(|e| e.to_string())?;
        }
    }

    Ok(SendResult {
        ok: status == "sent",
        error,
        tracked,
    })
}

#[tauri::command]
pub fn list_emails(
    state: State<AppState>,
    contact_id: Option<i64>,
) -> Result<Vec<crate::models::EmailLog>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let cols = "id, contact_id, to_addr, subject, body, status, error, track_token, opened_at, open_count, sent_at";
    let map = |row: &rusqlite::Row| -> rusqlite::Result<crate::models::EmailLog> {
        Ok(crate::models::EmailLog {
            id: row.get(0)?,
            contact_id: row.get(1)?,
            to_addr: row.get(2)?,
            subject: row.get(3)?,
            body: row.get(4)?,
            status: row.get(5)?,
            error: row.get(6)?,
            track_token: row.get(7)?,
            opened_at: row.get(8)?,
            open_count: row.get(9)?,
            sent_at: row.get(10)?,
        })
    };
    let mut out = Vec::new();
    match contact_id {
        Some(cid) => {
            let sql = format!(
                "SELECT {} FROM emails WHERE contact_id = ?1 ORDER BY sent_at DESC",
                cols
            );
            let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
            let rows = stmt.query_map(params![cid], map).map_err(|e| e.to_string())?;
            for r in rows {
                out.push(r.map_err(|e| e.to_string())?);
            }
        }
        None => {
            let sql = format!("SELECT {} FROM emails ORDER BY sent_at DESC LIMIT 300", cols);
            let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
            let rows = stmt.query_map([], map).map_err(|e| e.to_string())?;
            for r in rows {
                out.push(r.map_err(|e| e.to_string())?);
            }
        }
    }
    Ok(out)
}

#[derive(Deserialize)]
pub struct OpenRecord {
    pub token: String,
    pub opened_at: Option<String>,
    #[serde(default)]
    pub count: Option<i64>,
}

/// Apply a batch of open events (fetched by the frontend from the tracking
/// endpoint) to the local email log. Returns how many rows were updated.
#[tauri::command]
pub fn apply_opens(state: State<AppState>, opens: Vec<OpenRecord>) -> Result<usize, String> {
    let mut conn = state.pool.get().map_err(|e| e.to_string())?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let mut updated = 0usize;
    for o in &opens {
        let count = o.count.unwrap_or(1).max(1);
        let n = tx
            .execute(
                "UPDATE emails
                 SET opened_at = COALESCE(opened_at, ?2),
                     open_count = MAX(open_count, ?3)
                 WHERE track_token = ?1",
                params![o.token, o.opened_at, count],
            )
            .map_err(|e| e.to_string())?;
        updated += n;
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(updated)
}
