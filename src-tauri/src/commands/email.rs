use crate::models::SmtpConfig;
use crate::AppState;
use lettre::message::{Mailbox, MultiPart, SinglePart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{Message, SmtpTransport, Transport};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use tauri::{AppHandle, Emitter, State};

use super::{campaigns, deliverability, funnel, organisations, outbox, status};

static TOKEN_COUNTER: AtomicU64 = AtomicU64::new(1);

fn setting(conn: &rusqlite::Connection, key: &str) -> Option<String> {
    // Routed through the Keychain for secret keys (see crate::secrets).
    crate::secrets::read_setting(conn, key).filter(|s| !s.trim().is_empty())
}

fn load_smtp(state: &AppState) -> Result<SmtpConfig, String> {
    load_smtp_from(&state.pool)
}

/// Pool-based variant, for work that runs off the command thread (bulk send,
/// reply sync) and so cannot hold a `State`.
pub(crate) fn load_smtp_from(pool: &crate::db::DbPool) -> Result<SmtpConfig, String> {
    let conn = pool.get().map_err(|e| e.to_string())?;
    let mut cfg: SmtpConfig = match setting(&conn, "smtp") {
        Some(j) => serde_json::from_str(&j).map_err(|e| e.to_string())?,
        None => SmtpConfig::default(),
    };
    // The password lives in the Keychain, not in the `smtp` JSON.
    crate::secrets::smtp_fill(&mut cfg);
    Ok(cfg)
}

/// Editable email signature, stored as JSON under the `email_signature` setting
/// and appended to every outgoing message (both the plain-text and HTML parts).
#[derive(Deserialize, Default)]
struct SignatureData {
    #[serde(default)]
    name: String,
    #[serde(default)]
    role: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    phone: String,
    #[serde(default)]
    booking_email: String,
    #[serde(default)]
    website: String,
    #[serde(default)]
    instagram: String,
    #[serde(default)]
    soundcloud: String,
}

impl SignatureData {
    fn is_empty(&self) -> bool {
        [
            &self.name,
            &self.role,
            &self.label,
            &self.phone,
            &self.booking_email,
            &self.website,
            &self.instagram,
            &self.soundcloud,
        ]
        .iter()
        .all(|s| s.trim().is_empty())
    }
}

fn load_signature(conn: &rusqlite::Connection) -> Option<SignatureData> {
    let raw = setting(conn, "email_signature")?;
    let s: SignatureData = serde_json::from_str(&raw).ok()?;
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn norm_url(val: &str) -> String {
    let v = val.trim();
    if v.starts_with("http://") || v.starts_with("https://") {
        v.to_string()
    } else {
        format!("https://{}", v)
    }
}

/// Accept either a full URL or a bare handle (with or without a leading @).
fn social_url(host: &str, val: &str) -> String {
    let v = val.trim().trim_start_matches('@');
    if v.starts_with("http://") || v.starts_with("https://") {
        v.to_string()
    } else {
        format!("https://{}/{}", host, v.trim_start_matches(&format!("{}/", host)))
    }
}

/// Render the signature block as (plain_text, html). Only non-empty fields show.
fn render_signature(s: &SignatureData) -> (String, String) {
    fn f(v: &str) -> &str {
        v.trim()
    }
    // Plain text.
    let mut plain: Vec<String> = Vec::new();
    let title = match (f(&s.name).is_empty(), f(&s.role).is_empty()) {
        (false, false) => format!("{} · {}", f(&s.name), f(&s.role)),
        (false, true) => f(&s.name).to_string(),
        (true, false) => f(&s.role).to_string(),
        _ => String::new(),
    };
    if !title.is_empty() {
        plain.push(title);
    }
    if !f(&s.label).is_empty() {
        plain.push(f(&s.label).to_string());
    }
    let contact: Vec<String> = [f(&s.booking_email), f(&s.phone)]
        .iter()
        .filter(|v| !v.is_empty())
        .map(|v| v.to_string())
        .collect();
    if !contact.is_empty() {
        plain.push(contact.join(" · "));
    }
    let mut links: Vec<String> = Vec::new();
    if !f(&s.website).is_empty() {
        links.push(norm_url(&s.website));
    }
    if !f(&s.instagram).is_empty() {
        links.push(social_url("instagram.com", &s.instagram));
    }
    if !f(&s.soundcloud).is_empty() {
        links.push(social_url("soundcloud.com", &s.soundcloud));
    }
    if !links.is_empty() {
        plain.push(links.join(" · "));
    }
    let plain_out = if plain.is_empty() {
        String::new()
    } else {
        format!("--\n{}", plain.join("\n"))
    };

    // HTML.
    let mut rows: Vec<String> = Vec::new();
    if !f(&s.name).is_empty() {
        let role = if f(&s.role).is_empty() {
            String::new()
        } else {
            format!(
                " <span style=\"color:#777\">· {}</span>",
                html_escape(f(&s.role))
            )
        };
        rows.push(format!(
            "<div style=\"font-weight:bold;color:#111\">{}{}</div>",
            html_escape(f(&s.name)),
            role
        ));
    } else if !f(&s.role).is_empty() {
        rows.push(format!("<div>{}</div>", html_escape(f(&s.role))));
    }
    if !f(&s.label).is_empty() {
        rows.push(format!(
            "<div style=\"font-weight:600;color:#333\">{}</div>",
            html_escape(f(&s.label))
        ));
    }
    let mut contact_html: Vec<String> = Vec::new();
    if !f(&s.booking_email).is_empty() {
        contact_html.push(format!(
            "<a href=\"mailto:{0}\" style=\"color:#555;text-decoration:none\">{0}</a>",
            html_escape(f(&s.booking_email))
        ));
    }
    if !f(&s.phone).is_empty() {
        contact_html.push(html_escape(f(&s.phone)));
    }
    if !contact_html.is_empty() {
        rows.push(format!("<div>{}</div>", contact_html.join(" · ")));
    }
    let mut link_html: Vec<String> = Vec::new();
    let mut add_link = |url: String, label: &str| {
        link_html.push(format!(
            "<a href=\"{}\" style=\"color:#555\">{}</a>",
            html_escape(&url),
            label
        ));
    };
    if !f(&s.website).is_empty() {
        let url = norm_url(&s.website);
        let disp = url
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .trim_end_matches('/')
            .to_string();
        add_link(url.clone(), &html_escape(&disp));
    }
    if !f(&s.instagram).is_empty() {
        add_link(social_url("instagram.com", &s.instagram), "Instagram");
    }
    if !f(&s.soundcloud).is_empty() {
        add_link(social_url("soundcloud.com", &s.soundcloud), "SoundCloud");
    }
    if !link_html.is_empty() {
        rows.push(format!("<div>{}</div>", link_html.join(" · ")));
    }
    let html_out = if rows.is_empty() {
        String::new()
    } else {
        format!(
            "<div style=\"margin-top:22px;padding-top:14px;border-top:1px solid #e6e6e6;\
             font-family:-apple-system,Helvetica,Arial,sans-serif;font-size:13px;line-height:1.55;color:#555\">{}</div>",
            rows.join("")
        )
    };

    (plain_out, html_out)
}

pub(crate) fn build_transport(cfg: &SmtpConfig) -> Result<SmtpTransport, String> {
    // Sanitize inputs — stray whitespace / a pasted protocol prefix is a very
    // common cause of "535 authentication rejected".
    let host = cfg
        .host
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_string();
    if host.is_empty() {
        return Err("SMTP host is not configured.".into());
    }
    let username = cfg.username.trim().to_string();
    let password = cfg.password.trim().to_string();
    let creds = Credentials::new(username, password);
    let builder = match cfg.encryption.as_str() {
        "tls" => SmtpTransport::relay(&host).map_err(|e| e.to_string())?,
        "none" => SmtpTransport::builder_dangerous(&host),
        _ => SmtpTransport::starttls_relay(&host).map_err(|e| e.to_string())?,
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
pub(crate) fn render_template(input: &str, vars: &HashMap<String, String>) -> String {
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
        // Push the whole char, not a single byte: `bytes[i] as char` turned every
        // accented letter into mojibake ("é" -> "Ã©").
        let ch = input[i..].chars().next().unwrap_or('\u{FFFD}');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Build a single message (HTML + tracking pixel when configured, else plain text).
/// Build a proper multipart/alternative body: a text/plain part (what filters
/// and text-only clients read) plus a well-formed text/html part wrapped in a
/// real <html> document. Sending both, with a plain-text alternative, avoids the
/// SpamAssassin penalties for HTML-only / no-<html>-tag / image-heavy mail. The
/// invisible tracking pixel goes only in the HTML part, only when configured.
fn message_body(
    body: &str,
    signature: &Option<SignatureData>,
    tracking_base: &Option<String>,
    token: &str,
) -> MultiPart {
    let (sig_plain, sig_html) = match signature {
        Some(s) => render_signature(s),
        None => (String::new(), String::new()),
    };
    let plain = if sig_plain.is_empty() {
        body.to_string()
    } else {
        format!("{}\n\n{}", body, sig_plain)
    };
    let pixel = match tracking_base {
        Some(base) => format!(
            "<img src=\"{}/o/{}.gif\" width=\"1\" height=\"1\" alt=\"\" style=\"display:none\"/>",
            base.trim_end_matches('/'),
            token
        ),
        None => String::new(),
    };
    let html = format!(
        "<!doctype html><html lang=\"fr\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"></head>\
         <body style=\"margin:0;padding:0;background:#ffffff\">\
         <div style=\"max-width:560px;padding:8px 4px;font-family:-apple-system,Helvetica,Arial,sans-serif;\
         font-size:15px;line-height:1.55;color:#111111\">{}{}</div>{}\
         </body></html>",
        body_html(body),
        sig_html,
        pixel
    );
    MultiPart::alternative()
        .singlepart(SinglePart::plain(plain))
        .singlepart(SinglePart::html(html))
}

/// The body as a plain, personal-looking email: one <p> per paragraph, line
/// breaks kept, every URL clickable. No banner, no button, no template look:
/// it should read like a message typed in a mail app.
fn body_html(body: &str) -> String {
    body.trim()
        .split("\n\n")
        .filter(|p| !p.trim().is_empty())
        .map(|p| {
            let lines: Vec<String> = p.trim().lines().map(|l| linkify(&html_escape(l))).collect();
            format!("<p style=\"margin:0 0 14px 0\">{}</p>", lines.join("<br/>"))
        })
        .collect()
}

/// Wrap http(s) URLs of an already-escaped line in links. Trailing punctuation
/// stays outside the link.
fn linkify(escaped: &str) -> String {
    let mut out = String::with_capacity(escaped.len());
    let mut rest = escaped;
    while let Some(i) = rest.find("http://").into_iter().chain(rest.find("https://")).min() {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let end = tail.find(|c: char| c.is_whitespace() || c == '<').unwrap_or(tail.len());
        let mut url = &tail[..end];
        while url.ends_with(['.', ',', ')', ';', ':', '!', '?']) {
            url = &url[..url.len() - 1];
        }
        out.push_str(&format!("<a href=\"{0}\" style=\"color:#ff5a1f\">{0}</a>", url));
        rest = &tail[url.len()..];
    }
    out.push_str(rest);
    out
}

/// Sender as a structured mailbox: formatting "Name <addr>" by hand broke (and
/// silently dropped the name) when the display name held a comma or quotes.
fn sender_mailbox(cfg: &SmtpConfig) -> Result<Mailbox, String> {
    let addr = cfg
        .from_email
        .trim()
        .parse()
        .map_err(|_| "Invalid sender email address".to_string())?;
    let name = cfg.from_name.trim();
    Ok(Mailbox::new(if name.is_empty() { None } else { Some(name.to_string()) }, addr))
}

/// Parse a recipient field that may hold several addresses or a note
/// ("a@x.nl / b@x.nl", "info@x.com (secondary)"): the whole string first, then
/// the first token that is a valid address. These used to fail as "invalid".
fn parse_recipient(raw: &str) -> Option<Mailbox> {
    if let Ok(m) = raw.trim().parse::<Mailbox>() {
        return Some(m);
    }
    raw.split(|c: char| c == '/' || c == ',' || c == ';' || c.is_whitespace())
        .map(|t| t.trim().trim_matches(|c| c == '<' || c == '>' || c == '(' || c == ')'))
        .filter(|t| t.contains('@'))
        .find_map(|t| t.parse::<Mailbox>().ok())
}

/// The contact a free-typed address belongs to (lowest id when several share it).
fn contact_for_address(conn: &rusqlite::Connection, addr: &str) -> Option<i64> {
    conn.query_row(
        "SELECT id FROM contacts WHERE email IS NOT NULL AND lower(trim(email)) = lower(trim(?1))
         ORDER BY id LIMIT 1",
        params![addr],
        |r| r.get(0),
    )
    .ok()
}

/// Artist name for {{artist}}: the campaign's, else the contact's, else the
/// only artist in the roster (a one-artist CRM sending without a campaign used
/// to mail a literal "{{artist}}").
pub(crate) fn sole_artist(conn: &rusqlite::Connection) -> Option<String> {
    let mut stmt = conn.prepare("SELECT name FROM artists LIMIT 2").ok()?;
    let names: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .ok()?
        .filter_map(|r| r.ok())
        .collect();
    if names.len() == 1 {
        names.into_iter().next()
    } else {
        None
    }
}

/// Only one bulk send at a time: a second one (page left and re-opened mid-send)
/// would mail the same recipients twice.
static BULK_RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Set by `cancel_bulk_send`: the running batch stops before its next send.
static BULK_CANCEL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Stop the running bulk send before its next email (what was sent stays sent).
#[tauri::command]
pub fn cancel_bulk_send() -> bool {
    let running = BULK_RUNNING.load(Ordering::SeqCst);
    if running {
        BULK_CANCEL.store(true, Ordering::SeqCst);
    }
    running
}

struct BulkGuard;
impl Drop for BulkGuard {
    fn drop(&mut self) {
        BULK_RUNNING.store(false, Ordering::SeqCst);
    }
}

/// One outgoing message. The Message-ID is ours (stored with the email, so a
/// reply's In-Reply-To finds it), and every message carries List-Unsubscribe.
#[allow(clippy::too_many_arguments)]
fn build_message(
    from: &Mailbox,
    to: &Mailbox,
    subject: &str,
    body: &str,
    signature: &Option<SignatureData>,
    tracking_base: &Option<String>,
    token: &str,
    message_id: &str,
    unsubscribe_base: Option<&str>,
) -> Result<Message, String> {
    build_message_full(from, to, subject, body, signature, tracking_base, token, message_id, unsubscribe_base, None, &[])
}

/// Where an answer sits in an existing conversation: the Message-ID it answers
/// (In-Reply-To) and the chain before it (References), so the programmer's
/// mail app shows it inside the same thread. Used by the reply desk.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ThreadRef {
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
}

#[allow(clippy::too_many_arguments)]
fn build_message_in_thread(
    from: &Mailbox,
    to: &Mailbox,
    subject: &str,
    body: &str,
    signature: &Option<SignatureData>,
    tracking_base: &Option<String>,
    token: &str,
    message_id: &str,
    unsubscribe_base: Option<&str>,
    thread: Option<&ThreadRef>,
) -> Result<Message, String> {
    build_message_full(from, to, subject, body, signature, tracking_base, token, message_id, unsubscribe_base, thread, &[])
}

/// Every outgoing message: optional thread headers (reply desk) and file
/// attachments (generated booking documents, see commands/offers/docs.rs,
/// as multipart/mixed around the usual alternative body).
#[allow(clippy::too_many_arguments)]
fn build_message_full(
    from: &Mailbox,
    to: &Mailbox,
    subject: &str,
    body: &str,
    signature: &Option<SignatureData>,
    tracking_base: &Option<String>,
    token: &str,
    message_id: &str,
    unsubscribe_base: Option<&str>,
    thread: Option<&ThreadRef>,
    attachments: &[(String, Vec<u8>)],
) -> Result<Message, String> {
    let alt = message_body(body, signature, tracking_base, token);
    let content = if attachments.is_empty() {
        alt
    } else {
        let pdf = lettre::message::header::ContentType::parse("application/pdf").map_err(|e| e.to_string())?;
        attachments.iter().fold(MultiPart::mixed().multipart(alt), |m, (name, bytes)| {
            m.singlepart(lettre::message::Attachment::new(name.clone()).body(bytes.clone(), pdf.clone()))
        })
    };
    let mut b = Message::builder()
        .from(from.clone())
        .to(to.clone())
        .subject(subject.to_string())
        .message_id(Some(message_id.to_string()));
    if let Some(t) = thread {
        if let Some(id) = t.in_reply_to.as_deref().filter(|s| !s.trim().is_empty()) {
            b = b.in_reply_to(id.trim().to_string());
        }
        if !t.references.is_empty() {
            b = b.references(t.references.join(" "));
        }
    }
    // List-Unsubscribe (mailto, plus the HTTPS one-click link and
    // List-Unsubscribe-Post when an endpoint is configured).
    deliverability::with_unsubscribe(b, &from.email.to_string(), unsubscribe_base, token)
        .multipart(content)
        .map_err(|e| e.to_string())
}

/// Hand a message to SMTP: (status, error, hard_bounce). A permanent "no such
/// mailbox" rejection is a hard bounce: the caller suppresses the address so it
/// is never retried.
fn deliver(transport: &SmtpTransport, msg: &Message) -> (&'static str, Option<String>, bool) {
    match transport.send(msg) {
        Ok(_) => ("sent", None, false),
        Err(e) => {
            let code = e.status().and_then(|c| c.to_string().parse::<u16>().ok());
            let text = e.to_string();
            let hard = e.is_permanent() && outbox::is_permanent_recipient_failure(code, &text);
            ("failed", Some(text), hard)
        }
    }
}

#[tauri::command(async)]
pub fn get_smtp_config(state: State<'_, AppState>) -> Result<SmtpConfig, String> {
    // The UI gets a masked password, never the real one.
    load_smtp(&state).map(crate::secrets::smtp_masked)
}

#[tauri::command(async)]
pub fn save_smtp_config(state: State<'_, AppState>, mut config: SmtpConfig) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    // Password to the Keychain (an unchanged mask keeps the stored one).
    crate::secrets::smtp_store(&mut config)?;
    let json = serde_json::to_string(&config).map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO settings (key, value) VALUES ('smtp', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![json],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command(async)]
pub fn test_smtp(state: State<'_, AppState>) -> Result<bool, String> {
    let cfg = load_smtp(&state)?;
    let transport = build_transport(&cfg)?;
    transport.test_connection().map_err(|e| e.to_string())
}

#[derive(Serialize)]
pub struct SendResult {
    pub ok: bool,
    pub error: Option<String>,
    pub tracked: bool,
    /// Why nothing was sent, for the UI: "suppressed" (address on the
    /// suppression list), "quota" (daily cap reached), "bounced" (hard SMTP
    /// rejection, the address is now suppressed).
    pub code: Option<String>,
}

#[tauri::command(async)]
pub fn send_email(
    app: AppHandle,
    state: State<'_, AppState>,
    contact_id: Option<i64>,
    campaign_id: Option<i64>,
    to: String,
    subject: String,
    body: String,
    attachments: Option<Vec<String>>,
) -> Result<SendResult, String> {
    // Only documents PiersCRM generated (app data dir) can be attached.
    let files = attachments
        .unwrap_or_default()
        .iter()
        .map(|p| super::offers::docs::load_attachment(&app, p))
        .collect::<Result<Vec<_>, _>>()?;
    send_one_with(&state.pool, contact_id, campaign_id, &to, &subject, &body, &files).map(|(r, _)| r)
}

/// One email through every guardrail (suppression list, daily cap), with our
/// Message-ID and List-Unsubscribe, logged in `emails` and applied to the
/// pipeline and the campaign audience. Shared by the Compose tab and the
/// residency autopilot. Returns the result and the id of the logged email.
pub(crate) fn send_one(
    pool: &crate::db::DbPool,
    contact_id: Option<i64>,
    campaign_id: Option<i64>,
    to: &str,
    subject: &str,
    body: &str,
) -> Result<(SendResult, Option<i64>), String> {
    send_one_full(pool, contact_id, campaign_id, to, subject, body, None, &[])
}

/// `send_one` for an answer inside an existing conversation (reply desk):
/// same guardrails and logging, plus In-Reply-To / References.
pub(crate) fn send_one_in_thread(
    pool: &crate::db::DbPool,
    contact_id: Option<i64>,
    campaign_id: Option<i64>,
    to: &str,
    subject: &str,
    body: &str,
    thread: Option<&ThreadRef>,
) -> Result<(SendResult, Option<i64>), String> {
    send_one_full(pool, contact_id, campaign_id, to, subject, body, thread, &[])
}

/// `send_one` with file attachments (name, bytes).
pub(crate) fn send_one_with(
    pool: &crate::db::DbPool,
    contact_id: Option<i64>,
    campaign_id: Option<i64>,
    to: &str,
    subject: &str,
    body: &str,
    attachments: &[(String, Vec<u8>)],
) -> Result<(SendResult, Option<i64>), String> {
    send_one_full(pool, contact_id, campaign_id, to, subject, body, None, attachments)
}

#[allow(clippy::too_many_arguments)]
fn send_one_full(
    pool: &crate::db::DbPool,
    contact_id: Option<i64>,
    campaign_id: Option<i64>,
    to: &str,
    subject: &str,
    body: &str,
    thread: Option<&ThreadRef>,
    attachments: &[(String, Vec<u8>)],
) -> Result<(SendResult, Option<i64>), String> {
    let cfg = load_smtp_from(pool)?;
    if cfg.from_email.trim().is_empty() {
        return Err("Sender email is not configured (Settings → Email).".into());
    }

    let from_mbox = sender_mailbox(&cfg)?;
    let to_mbox: Mailbox =
        parse_recipient(to).ok_or_else(|| format!("Invalid recipient address: {}", to))?;
    let addr = to_mbox.email.to_string();

    // Guardrails first: a suppressed address is never mailed, and the daily cap
    // holds for single sends too.
    let (tracking_base, signature, unsub_base) = {
        let conn = pool.get().map_err(|e| e.to_string())?;
        if let Some(reason) = outbox::suppression_reason(&conn, &addr) {
            return Ok((
                SendResult {
                    ok: false,
                    error: Some(format!("{} : adresse en liste de suppression ({})", addr, reason)),
                    tracked: false,
                    code: Some("suppressed".into()),
                },
                None,
            ));
        }
        let q = outbox::quota(&conn);
        if q.remaining <= 0 {
            return Ok((
                SendResult {
                    ok: false,
                    error: Some(format!("Quota journalier atteint ({}/{}).", q.sent_today, q.cap)),
                    tracked: false,
                    code: Some("quota".into()),
                },
                None,
            ));
        }
        // The shared sending budget (warm-up ramp, hourly cap, per-domain cap).
        if let Some((code, reason)) = deliverability::send_block(&conn, &addr, chrono::Utc::now()) {
            return Ok((SendResult { ok: false, error: Some(reason), tracked: false, code: Some(code) }, None));
        }
        // Open-tracking: only if a public tracking base URL is configured.
        (setting(&conn, "tracking_base_url"), load_signature(&conn), deliverability::unsubscribe_base(&conn))
    };
    let token = gen_token();
    let tracked = tracking_base.is_some();
    let message_id = outbox::gen_message_id(&cfg.from_email);

    let email = build_message_full(
        &from_mbox, &to_mbox, subject, body, &signature, &tracking_base, &token, &message_id,
        unsub_base.as_deref(), thread, attachments,
    )?;

    let transport = build_transport(&cfg)?;
    let (outcome, error, hard_bounce) = deliver(&transport, &email);
    if outcome == "sent" {
        outbox::save_to_sent(pool, &cfg, email.formatted());
    }

    let conn = pool.get().map_err(|e| e.to_string())?;
    // A free-typed address (Compose tab) is tied to its contact, so the email
    // shows in the contact's history, counts for follow-ups and moves the pipeline.
    let contact_id = contact_id.or_else(|| contact_for_address(&conn, &addr));
    conn.execute(
        "INSERT INTO emails (contact_id, campaign_id, to_addr, subject, body, status, error, track_token, message_id)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![contact_id, campaign_id, addr, subject, body, outcome, error, token,
            (outcome == "sent").then_some(&message_id)],
    )
    .map_err(|e| e.to_string())?;
    let email_id = conn.last_insert_rowid();
    deliverability::record_send(&conn, Some(email_id), &cfg.from_email, &addr, outcome == "sent");

    if outcome == "sent" {
        status::on_email_sent(&conn, contact_id, &addr);
        if let (Some(camp), Some(cid)) = (campaign_id, contact_id) {
            campaigns::mark_audience(&conn, camp, cid, "sent");
        }
    } else if hard_bounce {
        outbox::suppress(&conn, &addr, "bounce", error.as_deref());
    }

    Ok((
        SendResult {
            ok: outcome == "sent",
            error,
            tracked,
            code: hard_bounce.then(|| "bounced".to_string()),
        },
        Some(email_id),
    ))
}

#[tauri::command(async)]
pub fn list_emails(
    state: State<'_, AppState>,
    contact_id: Option<i64>,
) -> Result<Vec<crate::models::EmailLog>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let cols = outbox::EMAIL_COLS;
    let map = outbox::email_from_row;
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
    /// Waiting for the anti-spam spacing until this UTC time (shared budget).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waiting_until: Option<String>,
}

#[derive(Serialize)]
pub struct BulkResult {
    pub sent: usize,
    pub failed: usize,
    pub skipped: usize,
    pub errors: Vec<String>,
    /// Addresses skipped because they are on the suppression list.
    pub suppressed: Vec<String>,
    /// Addresses suppressed during this send (hard SMTP rejection).
    pub bounced: Vec<String>,
    /// Recipients left unsent because the daily cap was reached.
    pub deferred: usize,
    pub cap: i64,
    /// Skipped because their organisation was already mailed (in this batch,
    /// or earlier in the same campaign). Counted in `skipped` too.
    pub grouped: usize,
    /// Left for another day: their domain had its daily share (shared
    /// sending budget). Counted in `skipped` too.
    pub throttled: Vec<String>,
    /// Why the batch stopped early (sending budget, health pause), if it did.
    pub stop_reason: Option<String>,
}

/// {{date}} / {{target_date}} for humans: an ISO day ("2026-10-21") becomes
/// "21 Oct" (with the year when it is not the current one); anything else (a
/// free-text date) is kept as typed. Mirrored by humanDate() in ComposeModal.tsx
/// so the preview matches what is sent.
pub(crate) fn human_date(raw: &str) -> String {
    let t = raw.trim();
    let Some(d) = t
        .get(..10)
        .and_then(|p| chrono::NaiveDate::parse_from_str(p, "%Y-%m-%d").ok())
    else {
        return t.to_string();
    };
    if t.len() > 10 && !t[10..].starts_with(['T', ' ']) {
        return t.to_string();
    }
    use chrono::Datelike;
    if d.year() == chrono::Local::now().year() {
        d.format("%-d %b").to_string()
    } else {
        d.format("%-d %b %Y").to_string()
    }
}

fn non_empty(s: Option<String>) -> Option<String> {
    s.filter(|v| !v.trim().is_empty())
}

/// Send one email per contact in `contact_ids`, rendering {{variables}} from the
/// contact + its campaign (event/artist/target date). One SMTP connection is
/// reused; progress is streamed via the `bulk-progress` event.
///
/// A bulk send lasts minutes (throttled sends, blocking SMTP), so it runs on
/// the blocking thread pool: as a plain `(async)` command it used to occupy an
/// async runtime worker for the whole batch.
#[tauri::command]
pub async fn send_bulk(
    app: AppHandle,
    state: State<'_, AppState>,
    campaign_id: Option<i64>,
    contact_ids: Vec<i64>,
    subject: String,
    body: String,
    extra_vars: Option<HashMap<String, String>>,
    subject_variants: Option<Vec<String>>,
    template_id: Option<i64>,
) -> Result<BulkResult, String> {
    let pool = state.pool.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let ab = (subject_variants.unwrap_or_default(), template_id);
        send_bulk_blocking(app, &pool, campaign_id, contact_ids, subject, body, extra_vars, ab)
    })
    .await
    .map_err(|e| e.to_string())?
}

fn send_bulk_blocking(
    app: AppHandle,
    pool: &crate::db::DbPool,
    campaign_id: Option<i64>,
    contact_ids: Vec<i64>,
    subject: String,
    body: String,
    extra_vars: Option<HashMap<String, String>>,
    (subject_variants, template_id): (Vec<String>, Option<i64>),
) -> Result<BulkResult, String> {
    if BULK_RUNNING.swap(true, Ordering::SeqCst) {
        return Err("Un envoi groupé est déjà en cours.".into());
    }
    let _bulk = BulkGuard;
    BULK_CANCEL.store(false, Ordering::SeqCst);
    let cfg = load_smtp_from(pool)?;
    if cfg.from_email.trim().is_empty() {
        return Err("Sender email is not configured (Settings → Email).".into());
    }
    let from_mbox = sender_mailbox(&cfg)?;
    let transport = build_transport(&cfg)?;

    let conn = pool.get().map_err(|e| e.to_string())?;
    // Daily cap: what is left of today's quota bounds this batch; the rest is
    // reported as deferred instead of failing the whole send.
    let quota = outbox::quota(&conn);
    if quota.remaining <= 0 {
        return Err(format!(
            "Quota journalier atteint ({}/{}). Réessayez demain ou augmentez le plafond.",
            quota.sent_today, quota.cap
        ));
    }
    let mut budget = quota.remaining;
    // Sender health: above the bounce / complaint thresholds every automated
    // send is paused until the user resumes (banner).
    if let Some(p) = deliverability::health::refresh(&conn, chrono::Utc::now()) {
        return Err(format!("Envois automatiques en pause : {}", p.reason));
    }
    let unsub_base = deliverability::unsubscribe_base(&conn);
    let mut throttled: Vec<String> = Vec::new();
    let mut stop_reason: Option<String> = None;
    let tracking_base = setting(&conn, "tracking_base_url");
    let signature = load_signature(&conn);
    // Spacing between sends: spreading mail out is the single biggest lever
    // for staying out of spam. A bulk send the user started uses the shorter
    // `bulk_spacing_*` gap of the sending budget (20 to 60 s by default); the
    // residency autopilot keeps the longer shared gap.
    let budget_cfg = deliverability::budget::load_config(&conn);
    let mut next_bulk_at: Option<chrono::DateTime<chrono::Utc>> = None;

    // Campaign-level variables ({{event}}, {{artist}}, {{target_date}}) plus any
    // per-send link variables the user supplied ({{mix}}, {{listen}}, …).
    let mut base_vars: HashMap<String, String> = HashMap::new();
    if let Some(extra) = extra_vars {
        for (k, v) in extra {
            if !v.trim().is_empty() {
                base_vars.insert(k, v);
            }
        }
    }
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
                base_vars.insert("target_date".into(), human_date(&td));
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

    let fallback_artist = sole_artist(&conn);
    // Subject A/B test (commands/funnel.rs): `subject` is arm A, each recipient
    // gets one arm from a hash of its contact id.
    let variants = funnel::clean_variants(&subject, &subject_variants);
    funnel::record_variants(&conn, campaign_id, template_id, &variants);
    let ab_salt = funnel::salt(campaign_id, template_id.unwrap_or(0));

    let total = contact_ids.len();
    let (mut sent, mut failed, mut skipped, mut deferred) = (0usize, 0usize, 0usize, 0usize);
    let mut errors: Vec<String> = Vec::new();
    let mut suppressed: Vec<String> = Vec::new();
    let mut bounced: Vec<String> = Vec::new();
    // Several contacts can share one address (a promoter booking two venues):
    // mail it once per batch.
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    // Same for an organisation (one programmer behind several venues, see
    // commands/organisations.rs): its first reachable contact gets the email.
    let mut seen_orgs: std::collections::HashSet<i64> = std::collections::HashSet::new();
    let mut grouped = 0usize;

    for (idx, cid) in contact_ids.iter().enumerate() {
        if budget <= 0 {
            // Daily cap reached: the rest waits for tomorrow.
            deferred = total - idx;
            let _ = app.emit(
                "bulk-progress",
                BulkProgress { done: total, total, sent, failed, skipped, current: None, waiting_until: None },
            );
            break;
        }
        let row = conn
            .query_row(
                "SELECT c.name, c.promoter, c.venue, c.area, c.date, c.email, a.name, c.organisation_id
                 FROM contacts c LEFT JOIN artists a ON a.id = c.artist_id WHERE c.id = ?1",
                params![cid],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, Option<String>>(3)?,
                        r.get::<_, Option<String>>(4)?,
                        r.get::<_, Option<String>>(5)?,
                        r.get::<_, Option<String>>(6)?,
                        r.get::<_, Option<i64>>(7)?,
                    ))
                },
            )
            .ok();

        let emit = |done, sent, failed, skipped, current| {
            let _ = app.emit(
                "bulk-progress",
                BulkProgress { done, total, sent, failed, skipped, current, waiting_until: None },
            );
        };

        let Some((name, promoter, venue, area, date, email, contact_artist, org_id)) = row else {
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
        if !vars.contains_key("artist") {
            if let Some(a) = non_empty(contact_artist).or_else(|| fallback_artist.clone()) {
                vars.insert("artist".into(), a);
            }
        }
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
        vars.insert(
            "date".into(),
            non_empty(date).map(|d| human_date(&d)).unwrap_or(fallback_date),
        );

        let picked = funnel::pick(*cid, ab_salt, &variants);
        let subj = render_template(picked.map_or(subject.as_str(), |p| p.1), &vars);
        let bod = render_template(&body, &vars);

        let to_mbox: Mailbox = match parse_recipient(&email) {
            Some(m) => m,
            None => {
                failed += 1;
                errors.push(format!("{}: adresse invalide", email));
                emit(idx + 1, sent, failed, skipped, Some(email.clone()));
                continue;
            }
        };
        let email = to_mbox.email.to_string();

        if !seen.insert(email.trim().to_lowercase()) {
            // That mailbox was just reached through another contact.
            if let Some(camp) = campaign_id {
                campaigns::mark_audience(&conn, camp, *cid, "sent");
            }
            skipped += 1;
            emit(idx + 1, sent, failed, skipped, Some(email.clone()));
            continue;
        }
        if outbox::suppression_reason(&conn, &email).is_some() {
            skipped += 1;
            suppressed.push(email.clone());
            emit(idx + 1, sent, failed, skipped, Some(email.clone()));
            continue;
        }
        if let Some(org) = org_id {
            let reached = seen_orgs.contains(&org)
                || campaign_id.is_some_and(|camp| organisations::org_reached_in_campaign(&conn, camp, org));
            if reached {
                // The organisation has the email: this venue counts as contacted
                // through it, so the next batch does not cold-mail it again.
                if let Some(camp) = campaign_id {
                    campaigns::mark_audience(&conn, camp, *cid, "grouped");
                }
                status::on_email_sent(&conn, Some(*cid), "");
                grouped += 1;
                skipped += 1;
                emit(idx + 1, sent, failed, skipped, Some(email.clone()));
                continue;
            }
        }

        // The shared sending budget (deliverability): health pause, daily and
        // hourly caps, per-domain cap, and the random spacing between sends.
        let mut skip_domain = false;
        loop {
            if BULK_CANCEL.load(Ordering::SeqCst) {
                stop_reason = Some("Envoi annulé.".into());
                break;
            }
            if let Some(p) = deliverability::health::current_pause(&conn) {
                stop_reason = Some(p.reason);
                break;
            }
            // Bulk spacing (shorter than the autopilot's), waited in 1 s steps
            // so a cancel takes effect at once.
            if let Some(until) = next_bulk_at.filter(|t| *t > chrono::Utc::now()) {
                let _ = app.emit(
                    "bulk-progress",
                    BulkProgress {
                        done: idx, total, sent, failed, skipped,
                        current: Some(email.clone()),
                        waiting_until: Some(deliverability::fmt(until)),
                    },
                );
                while chrono::Utc::now() < until && !BULK_CANCEL.load(Ordering::SeqCst) {
                    std::thread::sleep(std::time::Duration::from_secs(1));
                }
                continue;
            }
            match deliverability::budget::check(&conn, &email, chrono::Utc::now(), false) {
                deliverability::budget::Decision::Allow => break,
                deliverability::budget::Decision::Wait { reason, .. }
                | deliverability::budget::Decision::Defer { reason, .. } => {
                    stop_reason = Some(reason);
                    break;
                }
                deliverability::budget::Decision::SkipDomain { .. } => {
                    skip_domain = true;
                    break;
                }
            }
        }
        if stop_reason.is_some() {
            deferred = total - idx;
            emit(total, sent, failed, skipped, None);
            break;
        }
        if skip_domain {
            throttled.push(email.clone());
            skipped += 1;
            emit(idx + 1, sent, failed, skipped, Some(email.clone()));
            continue;
        }

        let token = gen_token();
        let message_id = outbox::gen_message_id(&cfg.from_email);
        // One malformed message must not abort the batch (and hide what was sent).
        let msg = match build_message(
            &from_mbox, &to_mbox, &subj, &bod, &signature, &tracking_base, &token, &message_id,
            unsub_base.as_deref(),
        ) {
            Ok(m) => m,
            Err(e) => {
                failed += 1;
                errors.push(format!("{}: {}", email, e));
                emit(idx + 1, sent, failed, skipped, Some(email.clone()));
                continue;
            }
        };
        let (outcome, err, hard_bounce) = deliver(&transport, &msg);
        if outcome == "sent" {
            outbox::save_to_sent(pool, &cfg, msg.formatted());
        }
        let inserted = conn.execute(
            "INSERT INTO emails (contact_id, campaign_id, to_addr, subject, body, status, error, track_token, message_id)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![cid, campaign_id, email, subj, bod, outcome, err, token,
                (outcome == "sent").then_some(&message_id)],
        );
        let inserted_id = inserted.is_ok().then(|| conn.last_insert_rowid());
        if let Some(id) = inserted_id {
            funnel::tag_email(&conn, id, template_id, None, picked.map(|p| p.0));
        }
        deliverability::record_send(&conn, inserted_id, &cfg.from_email, &email, outcome == "sent");
        if outcome == "sent" {
            sent += 1;
            budget -= 1;
            status::on_email_sent(&conn, Some(*cid), &email);
            if let Some(camp) = campaign_id {
                campaigns::mark_audience(&conn, camp, *cid, "sent");
            }
            if let Some(org) = org_id {
                seen_orgs.insert(org);
            }
        } else {
            failed += 1;
            if hard_bounce {
                outbox::suppress(&conn, &email, "bounce", err.as_deref());
                bounced.push(email.clone());
            }
            if let Some(e) = err {
                errors.push(format!("{}: {}", email, e));
            }
        }
        emit(idx + 1, sent, failed, skipped, Some(email.clone()));

        // Space out the next send.
        next_bulk_at = Some(chrono::Utc::now() + chrono::Duration::seconds(deliverability::budget::bulk_gap_secs(&budget_cfg)));
    }

    Ok(BulkResult {
        sent, failed, skipped, errors, suppressed, bounced, deferred, cap: quota.cap, grouped, throttled, stop_reason,
    })
}

/// Apply a batch of open events (fetched by the frontend from the tracking
/// endpoint) to the local email log. Returns how many rows were updated.
#[tauri::command(async)]
pub fn apply_opens(state: State<'_, AppState>, opens: Vec<OpenRecord>) -> Result<usize, String> {
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

#[cfg(test)]
mod body_html_tests {
    use super::{body_html, linkify};

    #[test]
    fn paragraphs_and_links() {
        let h = body_html("Hello,\n\nTout est là :\nhttps://insrt.studio/fr/p/abc\n\nÀ bientôt,\nThibault");
        assert_eq!(h.matches("<p ").count(), 3);
        assert!(h.contains("<a href=\"https://insrt.studio/fr/p/abc\""));
        assert!(h.contains("À bientôt,<br/>Thibault"));
        assert_eq!(linkify("voir https://a.fr/x."), "voir <a href=\"https://a.fr/x\" style=\"color:#ff5a1f\">https://a.fr/x</a>.");
    }

    #[test]
    fn threaded_message_headers() {
        use super::{build_message_in_thread, ThreadRef};
        let from: lettre::message::Mailbox = "Thibault <booking@insrt.fr>".parse().unwrap();
        let to: lettre::message::Mailbox = "booking@club.fr".parse().unwrap();
        let t = ThreadRef { in_reply_to: Some("<r1@club.fr>".into()), references: vec!["<s1@insrt.fr>".into(), "<r1@club.fr>".into()] };
        let m = build_message_in_thread(&from, &to, "Re: date", "Hello", &None, &None, "tok", "<n1@insrt.fr>", None, Some(&t)).unwrap();
        let raw = String::from_utf8(m.formatted()).unwrap();
        assert!(raw.contains("In-Reply-To: <r1@club.fr>"));
        assert!(raw.contains("References: <s1@insrt.fr> <r1@club.fr>"));
        let plain = build_message_in_thread(&from, &to, "s", "b", &None, &None, "tok", "<n2@insrt.fr>", None, None).unwrap();
        assert!(!String::from_utf8(plain.formatted()).unwrap().contains("In-Reply-To"));
    }
}

#[cfg(test)]
mod tests {
    use super::human_date;
    use chrono::Datelike;

    #[test]
    fn date_variable_formatting() {
        let y = chrono::Local::now().year();
        assert_eq!(human_date(&format!("{}-10-21", y)), "21 Oct");
        assert_eq!(human_date("2019-03-05"), "5 Mar 2019");
        assert_eq!(human_date("Wed Oct 21"), "Wed Oct 21");
        assert_eq!(human_date("2026-10-21abc"), "2026-10-21abc");
    }
}
