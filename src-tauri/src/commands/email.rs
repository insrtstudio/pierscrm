use crate::models::SmtpConfig;
use crate::AppState;
use lettre::message::{header::ContentType, Mailbox};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{Message, SmtpTransport, Transport};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use tauri::{AppHandle, Emitter, State};

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

/// Substitute {{ key }} placeholders from a map; unknown keys are left intact.
fn render_template(input: &str, vars: &HashMap<String, String>) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if i + 1 < bytes.len() && bytes[i] == b'{' && bytes[i + 1] == b'{' {
            if let Some(end) = input[i + 2..].find("}}") {
                let key = input[i + 2..i + 2 + end].trim();
                match vars.get(key) {
                    Some(v) => out.push_str(v),
                    None => out.push_str(&format!("{{{{{}}}}}", key)),
                }
                i = i + 2 + end + 2;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

/// Build a single message (HTML + tracking pixel when configured, else plain text).
fn build_message(
    from: &Mailbox,
    to: &Mailbox,
    subject: &str,
    body: &str,
    tracking_base: &Option<String>,
    token: &str,
) -> Result<Message, String> {
    let builder = Message::builder()
        .from(from.clone())
        .to(to.clone())
        .subject(subject.to_string());
    if let Some(base) = tracking_base {
        let base = base.trim_end_matches('/');
        let pixel = format!(
            "<img src=\"{}/o/{}.gif\" width=\"1\" height=\"1\" alt=\"\" style=\"display:none\"/>",
            base, token
        );
        let html = format!(
            "<div style=\"font-family:Arial,Helvetica,sans-serif;font-size:14px;line-height:1.5;color:#111\">{}</div>{}",
            html_escape(body).replace('\n', "<br/>"),
            pixel
        );
        builder
            .header(ContentType::TEXT_HTML)
            .body(html)
            .map_err(|e| e.to_string())
    } else {
        builder
            .header(ContentType::TEXT_PLAIN)
            .body(body.to_string())
            .map_err(|e| e.to_string())
    }
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
    campaign_id: Option<i64>,
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
        "INSERT INTO emails (contact_id, campaign_id, to_addr, subject, body, status, error, track_token)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        params![contact_id, campaign_id, to, subject, body, status, error, token],
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
    let cols = "id, contact_id, campaign_id, to_addr, subject, body, status, error, track_token, opened_at, open_count, sent_at";
    let map = |row: &rusqlite::Row| -> rusqlite::Result<crate::models::EmailLog> {
        Ok(crate::models::EmailLog {
            id: row.get(0)?,
            contact_id: row.get(1)?,
            campaign_id: row.get(2)?,
            to_addr: row.get(3)?,
            subject: row.get(4)?,
            body: row.get(5)?,
            status: row.get(6)?,
            error: row.get(7)?,
            track_token: row.get(8)?,
            opened_at: row.get(9)?,
            open_count: row.get(10)?,
            sent_at: row.get(11)?,
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

#[derive(Serialize, Clone)]
pub struct BulkProgress {
    pub done: usize,
    pub total: usize,
    pub sent: usize,
    pub failed: usize,
    pub skipped: usize,
    pub current: Option<String>,
}

#[derive(Serialize)]
pub struct BulkResult {
    pub sent: usize,
    pub failed: usize,
    pub skipped: usize,
    pub errors: Vec<String>,
}

fn non_empty(s: Option<String>) -> Option<String> {
    s.filter(|v| !v.trim().is_empty())
}

/// Send one email per contact in `contact_ids`, rendering {{variables}} from the
/// contact + its campaign (event/artist/target date). One SMTP connection is
/// reused; progress is streamed via the `bulk-progress` event.
#[tauri::command]
pub fn send_bulk(
    app: AppHandle,
    state: State<AppState>,
    campaign_id: Option<i64>,
    contact_ids: Vec<i64>,
    subject: String,
    body: String,
) -> Result<BulkResult, String> {
    let cfg = load_smtp(&state)?;
    if cfg.from_email.trim().is_empty() {
        return Err("Sender email is not configured (Settings → Email).".into());
    }
    let from_mbox: Mailbox = format!("{} <{}>", cfg.from_name, cfg.from_email)
        .parse()
        .or_else(|_| cfg.from_email.parse())
        .map_err(|_| "Invalid sender email address".to_string())?;
    let transport = build_transport(&cfg)?;

    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let tracking_base = setting(&conn, "tracking_base_url");

    // Campaign-level variables ({{event}}, {{artist}}, {{target_date}}).
    let mut base_vars: HashMap<String, String> = HashMap::new();
    if let Some(cid) = campaign_id {
        if let Ok((event_name, target_date, artist_id)) = conn.query_row(
            "SELECT event_name, target_date, artist_id FROM campaigns WHERE id = ?1",
            params![cid],
            |r| {
                Ok((
                    r.get::<_, Option<String>>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<i64>>(2)?,
                ))
            },
        ) {
            if let Some(e) = non_empty(event_name) {
                base_vars.insert("event".into(), e);
            }
            if let Some(td) = non_empty(target_date) {
                base_vars.insert("target_date".into(), td);
            }
            if let Some(aid) = artist_id {
                if let Ok(name) = conn.query_row(
                    "SELECT name FROM artists WHERE id = ?1",
                    params![aid],
                    |r| r.get::<_, String>(0),
                ) {
                    base_vars.insert("artist".into(), name);
                }
            }
        }
    }

    let total = contact_ids.len();
    let (mut sent, mut failed, mut skipped) = (0usize, 0usize, 0usize);
    let mut errors: Vec<String> = Vec::new();

    for (idx, cid) in contact_ids.iter().enumerate() {
        let row = conn
            .query_row(
                "SELECT name, promoter, venue, area, date, email FROM contacts WHERE id = ?1",
                params![cid],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, Option<String>>(3)?,
                        r.get::<_, Option<String>>(4)?,
                        r.get::<_, Option<String>>(5)?,
                    ))
                },
            )
            .ok();

        let emit = |done, sent, failed, skipped, current| {
            let _ = app.emit(
                "bulk-progress",
                BulkProgress { done, total, sent, failed, skipped, current },
            );
        };

        let Some((name, promoter, venue, area, date, email)) = row else {
            skipped += 1;
            emit(idx + 1, sent, failed, skipped, None);
            continue;
        };
        let Some(email) = non_empty(email) else {
            skipped += 1;
            emit(idx + 1, sent, failed, skipped, None);
            continue;
        };

        let mut vars = base_vars.clone();
        let display = non_empty(promoter.clone()).unwrap_or_else(|| name.clone());
        vars.insert("name".into(), display);
        vars.insert(
            "venue".into(),
            non_empty(venue.clone()).unwrap_or_else(|| name.clone()),
        );
        vars.entry("event".into()).or_insert_with(|| name.clone());
        vars.insert("promoter".into(), promoter.clone().unwrap_or_default());
        vars.insert("city".into(), area.clone().unwrap_or_default());
        let fallback_date = base_vars.get("target_date").cloned().unwrap_or_default();
        vars.insert("date".into(), non_empty(date).unwrap_or(fallback_date));

        let subj = render_template(&subject, &vars);
        let bod = render_template(&body, &vars);

        let to_mbox: Mailbox = match email.trim().parse() {
            Ok(m) => m,
            Err(_) => {
                failed += 1;
                errors.push(format!("{}: adresse invalide", email));
                emit(idx + 1, sent, failed, skipped, Some(email.clone()));
                continue;
            }
        };

        let token = gen_token();
        let msg = build_message(&from_mbox, &to_mbox, &subj, &bod, &tracking_base, &token)?;
        let (status, err) = match transport.send(&msg) {
            Ok(_) => ("sent", None),
            Err(e) => ("failed", Some(e.to_string())),
        };
        let _ = conn.execute(
            "INSERT INTO emails (contact_id, campaign_id, to_addr, subject, body, status, error, track_token)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![cid, campaign_id, email, subj, bod, status, err, token],
        );
        if status == "sent" {
            sent += 1;
            let _ = conn.execute(
                "UPDATE contacts
                 SET status = CASE WHEN status IN ('to_contact','to_evaluate','low_priority') THEN 'contacted' ELSE status END,
                     first_contact = COALESCE(first_contact, date('now')),
                     updated_at = datetime('now')
                 WHERE id = ?1",
                params![cid],
            );
        } else {
            failed += 1;
            if let Some(e) = err {
                errors.push(format!("{}: {}", email, e));
            }
        }
        emit(idx + 1, sent, failed, skipped, Some(email.clone()));
    }

    Ok(BulkResult { sent, failed, skipped, errors })
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
