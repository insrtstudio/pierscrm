//! Outbox guardrails and reply detection.
//!
//! - Suppression list: addresses never mailed again (hard bounce, unsubscribe,
//!   refusal, added by hand). Every send path checks it.
//! - Daily send cap (`daily_send_cap` setting, default 150) across single and
//!   bulk sends, counted from the emails sent since local midnight.
//! - Message-ID generation and the List-Unsubscribe header.
//! - IMAP reply sync: a read-only scan of INBOX (EXAMINE + BODY.PEEK, so nothing
//!   is ever marked read on the server) that matches replies to sent emails by
//!   In-Reply-To / References, else by sender address, and turns bounces and
//!   unsubscribe requests into suppressions.
//! - The paginated, filtered email history.

use crate::commands::status;
use crate::db::DbPool;
use crate::models::EmailLog;
use crate::AppState;
use lettre::message::header::{Header, HeaderName, HeaderValue};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tauri::State;

// ---------------- Message-ID + List-Unsubscribe ----------------

static MID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Domain part of the sender address, if it is a plausible host name.
fn sender_domain(from_email: &str) -> Option<String> {
    let (_, d) = from_email.trim().rsplit_once('@')?;
    let d = d.trim().trim_end_matches('>').to_ascii_lowercase();
    let ok = !d.is_empty()
        && d.contains('.')
        && d.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    ok.then_some(d)
}

/// A globally unique Message-ID on the sender's own domain, `<id@domain>`:
/// timestamp + process counter + a random component. Stored with the email so
/// IMAP replies (In-Reply-To / References) can be matched back to it.
pub fn gen_message_id(from_email: &str) -> String {
    let domain = sender_domain(from_email).unwrap_or_else(|| "pierscrm.local".into());
    let ts = chrono::Utc::now().timestamp_micros() as u64;
    let n = MID_COUNTER.fetch_add(1, Ordering::Relaxed);
    // RandomState is seeded randomly per process and per instance.
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(ts ^ n);
    format!("<{:x}.{:x}.{:016x}@{}>", ts, n, h.finish(), domain)
}

/// `List-Unsubscribe: <mailto:from?subject=unsubscribe>`. Gmail and Outlook show
/// an "unsubscribe" link for it, which is far better for the sender's reputation
/// than a spam report. The resulting email lands in our inbox and the reply sync
/// turns it into a suppression.
#[derive(Clone)]
pub struct ListUnsubscribe(String);

impl Header for ListUnsubscribe {
    fn name() -> HeaderName {
        HeaderName::new_from_ascii_str("List-Unsubscribe")
    }
    fn parse(s: &str) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        Ok(Self(s.to_string()))
    }
    fn display(&self) -> HeaderValue {
        HeaderValue::new(Self::name(), self.0.clone())
    }
}

pub fn list_unsubscribe(from_email: &str) -> ListUnsubscribe {
    ListUnsubscribe(format!("<mailto:{}?subject=unsubscribe>", from_email.trim()))
}

// ---------------- Suppressions ----------------

pub const SUPPRESSION_REASONS: [&str; 4] = ["bounce", "unsubscribe", "manual", "refused"];

pub fn norm_addr(a: &str) -> String {
    a.trim().trim_matches(|c| c == '<' || c == '>').trim().to_lowercase()
}

/// Why this address must not be mailed, if it is suppressed.
pub fn suppression_reason(conn: &Connection, addr: &str) -> Option<String> {
    conn.query_row(
        "SELECT reason FROM suppressions WHERE address = ?1",
        params![norm_addr(addr)],
        |r| r.get(0),
    )
    .ok()
}

/// Add an address to the suppression list (the first reason recorded wins).
/// Returns true when it was not suppressed yet.
pub fn suppress(conn: &Connection, addr: &str, reason: &str, note: Option<&str>) -> bool {
    let a = norm_addr(addr);
    if a.is_empty() || !a.contains('@') {
        return false;
    }
    let inserted = conn
        .execute(
            "INSERT OR IGNORE INTO suppressions (address, reason, note) VALUES (?1, ?2, ?3)",
            params![a, reason, note],
        )
        .unwrap_or(0)
        > 0;
    if inserted {
        match reason {
            "bounce" => crate::commands::campaigns::mark_audience_by_address(conn, &a, "bounced"),
            "unsubscribe" | "refused" => {
                crate::commands::campaigns::mark_audience_by_address(conn, &a, "unsubscribed")
            }
            _ => {}
        }
    }
    inserted
}

/// SMTP rejections that mean "this mailbox does not exist / cannot receive",
/// as opposed to a policy or spam block (5.7.x), an auth failure (535) or a
/// temporary problem (4xx). Only these auto-suppress the address: suppressing on
/// a 5.7.1 spam block would silently drop good contacts.
pub fn is_permanent_recipient_failure(code: Option<u16>, msg: &str) -> bool {
    let m = msg.to_lowercase();
    if m.contains("5.7.") || m.contains("spam") || m.contains("policy") || m.contains("blocked") {
        return false;
    }
    if ["5.1.1", "5.1.0", "5.1.2", "5.1.3", "5.1.6", "5.1.10", "5.2.1"]
        .iter()
        .any(|c| m.contains(c))
    {
        return true;
    }
    let mailbox_words = [
        "user unknown",
        "unknown user",
        "no such user",
        "does not exist",
        "mailbox unavailable",
        "mailbox not found",
        "recipient address rejected",
        "invalid recipient",
        "address rejected",
        "account disabled",
    ];
    match code {
        Some(550) | Some(551) | Some(553) => true,
        Some(c) if (500..600).contains(&c) => mailbox_words.iter().any(|w| m.contains(w)),
        _ => false,
    }
}

#[derive(Serialize)]
pub struct Suppression {
    pub id: i64,
    pub address: String,
    pub reason: String,
    pub note: Option<String>,
    pub created_at: String,
}

#[tauri::command(async)]
pub fn list_suppressions(state: State<'_, AppState>) -> Result<Vec<Suppression>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT id, address, reason, note, created_at FROM suppressions ORDER BY created_at DESC, id DESC")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(Suppression {
                id: r.get(0)?,
                address: r.get(1)?,
                reason: r.get(2)?,
                note: r.get(3)?,
                created_at: r.get(4)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

/// Add addresses by hand (one or several, separated by commas, spaces or new
/// lines). A refusal or unsubscribe also closes the matching contacts.
#[tauri::command(async)]
pub fn add_suppression(
    state: State<'_, AppState>,
    address: String,
    reason: String,
    note: Option<String>,
) -> Result<usize, String> {
    let reason = if SUPPRESSION_REASONS.contains(&reason.as_str()) { reason } else { "manual".into() };
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let mut n = 0;
    for a in address
        .split(|c: char| c == ',' || c == ';' || c.is_whitespace())
        .filter(|a| a.contains('@'))
    {
        if suppress(&conn, a, &reason, note.as_deref().filter(|s| !s.trim().is_empty())) {
            n += 1;
        }
        if reason == "refused" || reason == "unsubscribe" {
            status::on_refused(&conn, None, &norm_addr(a));
        }
    }
    Ok(n)
}

#[tauri::command(async)]
pub fn remove_suppression(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM suppressions WHERE id = ?1", params![id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ---------------- Daily send cap ----------------

pub const DEFAULT_DAILY_CAP: i64 = 150;
const MAX_DAILY_CAP: i64 = 5000;

/// Parse the `daily_send_cap` setting; anything missing or out of range falls
/// back to the default (a cap can be raised, never switched off).
pub fn parse_cap(raw: Option<&str>) -> i64 {
    raw.and_then(|s| s.trim().parse::<i64>().ok())
        .filter(|v| (1..=MAX_DAILY_CAP).contains(v))
        .unwrap_or(DEFAULT_DAILY_CAP)
}

pub fn remaining(cap: i64, sent_today: i64) -> i64 {
    (cap - sent_today).max(0)
}

pub fn daily_cap(conn: &Connection) -> i64 {
    let raw: Option<String> = conn
        .query_row("SELECT value FROM settings WHERE key = 'daily_send_cap'", [], |r| r.get(0))
        .ok();
    parse_cap(raw.as_deref())
}

/// Emails sent since local midnight (sent_at is stored in UTC).
pub fn sent_today(conn: &Connection) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM emails WHERE status = 'sent'
           AND sent_at >= datetime('now', 'localtime', 'start of day', 'utc')",
        [],
        |r| r.get(0),
    )
    .unwrap_or(0)
}

#[derive(Serialize)]
pub struct Quota {
    pub cap: i64,
    pub sent_today: i64,
    pub remaining: i64,
}

pub fn quota(conn: &Connection) -> Quota {
    let cap = daily_cap(conn);
    let sent = sent_today(conn);
    Quota { cap, sent_today: sent, remaining: remaining(cap, sent) }
}

#[tauri::command(async)]
pub fn send_quota(state: State<'_, AppState>) -> Result<Quota, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    Ok(quota(&conn))
}

// ---------------- Email history (paged) ----------------

#[derive(Deserialize, Default)]
pub struct EmailFilter {
    #[serde(default)]
    pub campaign_id: Option<i64>,
    /// "sent" or "failed".
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub opened: Option<bool>,
    #[serde(default)]
    pub replied: Option<bool>,
    #[serde(default)]
    pub search: Option<String>,
}

#[derive(Serialize)]
pub struct EmailPage {
    pub rows: Vec<EmailLog>,
    pub total: i64,
}

pub const EMAIL_COLS: &str = "id, contact_id, campaign_id, to_addr, subject, body, status, error, \
     track_token, opened_at, open_count, sent_at, message_id, replied_at";

pub fn email_from_row(row: &rusqlite::Row) -> rusqlite::Result<EmailLog> {
    Ok(EmailLog {
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
        message_id: row.get(12)?,
        replied_at: row.get(13)?,
    })
}

#[tauri::command(async)]
pub fn list_emails_page(
    state: State<'_, AppState>,
    filter: Option<EmailFilter>,
    limit: Option<i64>,
    offset: Option<i64>,
) -> Result<EmailPage, String> {
    let f = filter.unwrap_or_default();
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let mut wh = String::from(" WHERE 1=1");
    let mut args: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    if let Some(c) = f.campaign_id {
        wh.push_str(" AND campaign_id = ?");
        args.push(Box::new(c));
    }
    match f.status.as_deref() {
        Some("sent") => wh.push_str(" AND status = 'sent'"),
        Some("failed") => wh.push_str(" AND status <> 'sent'"),
        _ => {}
    }
    match f.opened {
        Some(true) => wh.push_str(" AND opened_at IS NOT NULL"),
        Some(false) => wh.push_str(" AND opened_at IS NULL"),
        None => {}
    }
    match f.replied {
        Some(true) => wh.push_str(" AND replied_at IS NOT NULL"),
        Some(false) => wh.push_str(" AND replied_at IS NULL"),
        None => {}
    }
    if let Some(q) = f.search.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        wh.push_str(" AND (to_addr LIKE ? OR subject LIKE ?)");
        let like = format!("%{}%", q);
        args.push(Box::new(like.clone()));
        args.push(Box::new(like));
    }
    let refs: Vec<&dyn rusqlite::types::ToSql> = args.iter().map(|b| b.as_ref()).collect();
    let total: i64 = conn
        .query_row(&format!("SELECT COUNT(*) FROM emails{}", wh), refs.as_slice(), |r| r.get(0))
        .map_err(|e| e.to_string())?;
    let limit = limit.unwrap_or(100).clamp(1, 500);
    let offset = offset.unwrap_or(0).max(0);
    let sql = format!(
        "SELECT {} FROM emails{} ORDER BY sent_at DESC, id DESC LIMIT {} OFFSET {}",
        EMAIL_COLS, wh, limit, offset
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(refs.as_slice(), email_from_row)
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    Ok(EmailPage { rows, total })
}

// ---------------- Inbound mail parsing (pure) ----------------

/// Raw header block -> (lowercased name, unfolded value) pairs.
pub fn parse_headers(raw: &[u8]) -> Vec<(String, String)> {
    let text = String::from_utf8_lossy(raw);
    let mut out: Vec<(String, String)> = Vec::new();
    for line in text.split('\n') {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            if let Some(last) = out.last_mut() {
                last.1.push(' ');
                last.1.push_str(line.trim());
            }
            continue;
        }
        if let Some((k, v)) = line.split_once(':') {
            out.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
        }
    }
    out
}

fn header<'a>(h: &'a [(String, String)], name: &str) -> Option<&'a str> {
    h.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut buf, mut bits) = (0u32, 0u32);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            b' ' | b'\r' | b'\n' => continue,
            _ => return None,
        } as u32;
        buf = (buf << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Some(out)
}

fn decode_charset(bytes: Vec<u8>, charset: &str) -> String {
    let cs = charset.to_ascii_lowercase();
    if cs.starts_with("iso-8859-1") || cs.starts_with("latin1") || cs.starts_with("windows-1252") {
        bytes.into_iter().map(|b| b as char).collect()
    } else {
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

/// Decode RFC 2047 encoded words (`=?UTF-8?B?...?=`, `=?utf-8?q?...?=`), as used
/// by most clients for non-ASCII subjects and display names.
pub fn decode_words(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    let mut last_was_word = false;
    while let Some(start) = rest.find("=?") {
        let before = &rest[..start];
        // Whitespace between two encoded words is dropped (RFC 2047 §6.2).
        if !(last_was_word && before.trim().is_empty()) {
            out.push_str(before);
        }
        let after = &rest[start + 2..];
        let parts: Vec<&str> = after.splitn(3, '?').collect();
        if parts.len() < 3 {
            out.push_str(&rest[start..]);
            return out;
        }
        let (charset, enc, tail) = (parts[0], parts[1], parts[2]);
        let Some(end) = tail.find("?=") else {
            out.push_str(&rest[start..]);
            return out;
        };
        let text = &tail[..end];
        let decoded = match enc {
            "B" | "b" => b64_decode(text).map(|b| decode_charset(b, charset)),
            "Q" | "q" => {
                let mut bytes = Vec::new();
                let tb = text.as_bytes();
                let mut i = 0;
                while i < tb.len() {
                    match tb[i] {
                        b'_' => bytes.push(b' '),
                        b'=' if i + 2 < tb.len() => {
                            match u8::from_str_radix(&text[i + 1..i + 3], 16) {
                                Ok(b) => {
                                    bytes.push(b);
                                    i += 2;
                                }
                                Err(_) => bytes.push(b'='),
                            }
                        }
                        c => bytes.push(c),
                    }
                    i += 1;
                }
                Some(decode_charset(bytes, charset))
            }
            _ => None,
        };
        match decoded {
            Some(d) => {
                out.push_str(&d);
                last_was_word = true;
            }
            None => {
                out.push_str(&rest[start..start + 2 + parts[0].len() + parts[1].len() + 2 + end + 2]);
                last_was_word = false;
            }
        }
        rest = &tail[end + 2..];
    }
    out.push_str(rest);
    out
}

/// The address inside a From/To value: `"Name" <a@b.c>` or a bare `a@b.c`.
pub fn extract_addr(v: &str) -> Option<String> {
    if let (Some(s), Some(e)) = (v.rfind('<'), v.rfind('>')) {
        if s < e {
            let a = norm_addr(&v[s + 1..e]);
            if a.contains('@') {
                return Some(a);
            }
        }
    }
    v.split(|c: char| c.is_whitespace() || c == ',' || c == ';' || c == '"')
        .map(|t| t.trim_matches(|c| c == '<' || c == '>' || c == '(' || c == ')'))
        .find(|t| t.contains('@') && t.contains('.'))
        .map(norm_addr)
}

/// Every `<message-id>` token of an In-Reply-To / References value.
pub fn extract_ids(v: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = v;
    while let Some(s) = rest.find('<') {
        let Some(e) = rest[s..].find('>') else { break };
        let id = &rest[s..s + e + 1];
        if id.contains('@') {
            out.push(id.to_string());
        }
        rest = &rest[s + e + 1..];
    }
    out
}

/// Every email-looking token of a text (used to read bounce reports).
pub fn addresses_in(text: &str) -> Vec<String> {
    text.split(|c: char| {
        c.is_whitespace() || matches!(c, '<' | '>' | '(' | ')' | '"' | '\'' | ',' | ';' | ':' | '[' | ']')
    })
    .map(|t| t.trim_end_matches('.'))
    .filter(|t| {
        let mut parts = t.split('@');
        matches!((parts.next(), parts.next(), parts.next()), (Some(l), Some(d), None)
            if !l.is_empty() && d.contains('.') && !d.starts_with('.'))
    })
    .map(norm_addr)
    .collect()
}

#[derive(Debug, PartialEq, Eq)]
pub enum Inbound {
    /// A human answer.
    Reply,
    /// Out-of-office and other automatic answers: not a reply.
    AutoReply,
    /// Delivery failure report.
    Bounce,
    /// "unsubscribe" email triggered by the List-Unsubscribe header.
    Unsubscribe,
    /// Our own messages and anything else to leave alone.
    Ignore,
}

pub struct InboundHeaders {
    pub from: Option<String>,
    pub subject: String,
    pub message_id: Option<String>,
    pub refs: Vec<String>,
    pub content_type: String,
    /// Content-Transfer-Encoding of a single-part message (base64, quoted-printable...).
    pub transfer_encoding: String,
    pub auto_submitted: bool,
    pub failed_recipients: Vec<String>,
}

pub fn read_inbound(raw: &[u8]) -> InboundHeaders {
    let h = parse_headers(raw);
    let get = |n: &str| header(&h, n).unwrap_or("");
    let mut refs = extract_ids(get("in-reply-to"));
    for id in extract_ids(get("references")) {
        if !refs.contains(&id) {
            refs.push(id);
        }
    }
    let auto_submitted = {
        let auto = get("auto-submitted").to_ascii_lowercase();
        let prec = get("precedence").to_ascii_lowercase();
        (!auto.is_empty() && auto != "no")
            || header(&h, "x-autoreply").is_some()
            || header(&h, "x-autorespond").is_some()
            || prec.contains("auto_reply")
            || prec.contains("bulk")
            || prec.contains("junk")
    };
    InboundHeaders {
        from: extract_addr(&decode_words(get("from"))),
        subject: decode_words(get("subject")),
        message_id: extract_ids(get("message-id")).into_iter().next(),
        refs,
        content_type: get("content-type").to_ascii_lowercase(),
        transfer_encoding: get("content-transfer-encoding").trim().to_ascii_lowercase(),
        auto_submitted,
        failed_recipients: addresses_in(get("x-failed-recipients")),
    }
}

const BOUNCE_SUBJECTS: [&str; 12] = [
    "undeliverable",
    "undelivered mail",
    "delivery status notification (failure)",
    "mail delivery failed",
    "delivery failure",
    "delivery has failed",
    "returned mail",
    "failure notice",
    "mail delivery subsystem",
    "non remis",
    "échec de la remise",
    "message non distribué",
];

const AUTO_SUBJECTS: [&str; 8] = [
    "out of office",
    "automatic reply",
    "auto reply",
    "autoreply",
    "réponse automatique",
    "absence",
    "abwesenheit",
    "afwezig",
];

pub fn classify(h: &InboundHeaders, own_addr: &str) -> Inbound {
    let from = h.from.clone().unwrap_or_default();
    if from.is_empty() || from == norm_addr(own_addr) {
        return Inbound::Ignore;
    }
    let local = from.split('@').next().unwrap_or("");
    let subj = h.subject.trim().to_lowercase();
    let daemon = ["mailer-daemon", "mailerdaemon", "mail-daemon", "postmaster"].contains(&local);
    if daemon
        || h.content_type.contains("report-type=delivery-status")
        || h.content_type.contains("report-type=\"delivery-status\"")
        || !h.failed_recipients.is_empty()
        || BOUNCE_SUBJECTS.iter().any(|s| subj.starts_with(s))
    {
        return Inbound::Bounce;
    }
    if subj.starts_with("unsubscribe") || subj.starts_with("désinscri") || subj.starts_with("desinscri") {
        return Inbound::Unsubscribe;
    }
    let stripped = subj
        .trim_start_matches("re:")
        .trim_start_matches("aw:")
        .trim_start_matches("tr:")
        .trim();
    if h.auto_submitted || AUTO_SUBJECTS.iter().any(|s| subj.starts_with(s) || stripped.starts_with(s)) {
        return Inbound::AutoReply;
    }
    Inbound::Reply
}

/// The recipients a bounce report says failed, restricted to addresses we
/// actually mailed (`known`), so an address merely quoted in the report is never
/// suppressed. A report that only announces a delay is not a bounce.
pub fn bounced_recipients(
    header_failed: &[String],
    body: &str,
    known: &dyn Fn(&str) -> bool,
) -> Vec<String> {
    let lower = body.to_lowercase();
    if lower.contains("action: delayed") && !lower.contains("action: failed") {
        return vec![];
    }
    let mut found: Vec<String> = header_failed.iter().map(|a| norm_addr(a)).collect();
    for line in body.lines() {
        let l = line.trim().to_lowercase();
        if l.starts_with("final-recipient:") || l.starts_with("original-recipient:") {
            found.extend(addresses_in(&l[l.find(':').unwrap_or(0) + 1..]));
        }
    }
    if found.is_empty() {
        found = addresses_in(body);
    }
    let mut seen = HashSet::new();
    found
        .into_iter()
        .filter(|a| known(a) && seen.insert(a.clone()))
        .collect()
}

// ---------------- Reply body text (pure) ----------------
//
// The residency agent reads what a club answered (to classify it), so the sync
// keeps a short plain-text excerpt of each reply. Only what is needed: the
// first text/plain part (else the HTML one, tags stripped), transfer encoding
// and charset decoded, the quoted original cut off.

/// Max characters kept per reply excerpt.
pub const EXCERPT_MAX: usize = 3000;

fn qp_decode(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'=' {
            // Soft line break.
            if i + 1 < b.len() && (b[i + 1] == b'\n' || b[i + 1] == b'\r') {
                i += 1;
                if b[i] == b'\r' && i + 1 < b.len() && b[i + 1] == b'\n' {
                    i += 1;
                }
                i += 1;
                continue;
            }
            if i + 2 < b.len() {
                let hex = |c: u8| (c as char).to_digit(16);
                if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                    out.push((h * 16 + l) as u8);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

/// A header parameter (`boundary`, `charset`) out of a Content-Type value.
fn ct_param(ct: &str, name: &str) -> Option<String> {
    let low = ct.to_ascii_lowercase();
    let key = format!("{}=", name);
    let pos = low.find(&key)?;
    let rest = &ct[pos + key.len()..];
    let v = if let Some(r) = rest.strip_prefix('"') {
        r.split('"').next().unwrap_or("")
    } else {
        rest.split(|c: char| c == ';' || c.is_whitespace()).next().unwrap_or("")
    };
    (!v.is_empty()).then(|| v.to_string())
}

fn decode_part(body: &str, transfer_encoding: &str, content_type: &str) -> String {
    let charset = ct_param(content_type, "charset").unwrap_or_else(|| "utf-8".into());
    let te = transfer_encoding.trim().to_ascii_lowercase();
    let bytes = if te.contains("base64") {
        b64_decode(body).unwrap_or_else(|| body.as_bytes().to_vec())
    } else if te.contains("quoted-printable") {
        qp_decode(body)
    } else {
        body.as_bytes().to_vec()
    };
    decode_charset(bytes, &charset)
}

fn strip_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    let lower = html.to_ascii_lowercase();
    // Drop <style>/<script> contents and the quoted original (Gmail/Outlook).
    let cut = ["<blockquote", "class=\"gmail_quote", "id=\"divrplyfwdmsg", "id=\"appendonsend"]
        .iter()
        .filter_map(|m| lower.find(m))
        .min()
        .unwrap_or(html.len());
    for ch in html[..cut].chars() {
        match ch {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&#39;", "'")
        .replace("&quot;", "\"")
}

/// Cut the quoted original ("> ...", "On ... wrote:", "Le ... a écrit :",
/// "-----Original Message-----", an Outlook "From:/De :" block) and squeeze blank lines.
pub fn strip_quoted(text: &str) -> String {
    let mut kept: Vec<&str> = Vec::new();
    for line in text.lines() {
        let l = line.trim();
        let low = l.to_lowercase();
        let marker = l.starts_with('>')
            || low.starts_with("-----original message")
            || low.starts_with("----- message d'origine")
            || low.starts_with("-------- message original")
            || ((low.starts_with("on ") && low.ends_with("wrote:"))
                || (low.starts_with("le ") && (low.ends_with("a écrit :") || low.ends_with("a écrit:"))))
            || ((low.starts_with("from:") || low.starts_with("de :") || low.starts_with("de:"))
                && kept.iter().rev().take(3).any(|k| k.trim().is_empty() || k.trim().starts_with("___")));
        if marker {
            break;
        }
        kept.push(line);
    }
    let mut out = String::new();
    let mut blank = 0;
    for l in kept {
        if l.trim().is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        out.push_str(l.trim_end());
        out.push('\n');
    }
    out.trim().to_string()
}

fn split_part(part: &str) -> (Vec<(String, String)>, &str) {
    let idx = part
        .find("\r\n\r\n")
        .map(|i| (i, 4))
        .or_else(|| part.find("\n\n").map(|i| (i, 2)));
    match idx {
        Some((i, n)) => (parse_headers(part[..i].as_bytes()), &part[i + n..]),
        None => (Vec::new(), part),
    }
}

/// (plain, html) candidates of a MIME body, depth-first.
fn walk_mime(content_type: &str, transfer_encoding: &str, body: &str, depth: u8, out: &mut (Option<String>, Option<String>)) {
    let ct = content_type.to_ascii_lowercase();
    if ct.starts_with("multipart/") && depth < 4 {
        let Some(boundary) = ct_param(content_type, "boundary") else { return };
        let delim = format!("--{}", boundary);
        for part in body.split(&delim).skip(1) {
            if part.starts_with("--") {
                break;
            }
            let part = part.trim_start_matches(['\r', '\n']);
            let (h, b) = split_part(part);
            let pct = header(&h, "content-type").unwrap_or("text/plain").to_string();
            let pte = header(&h, "content-transfer-encoding").unwrap_or("").to_string();
            let disp = header(&h, "content-disposition").unwrap_or("").to_ascii_lowercase();
            if disp.starts_with("attachment") {
                continue;
            }
            walk_mime(&pct, &pte, b, depth + 1, out);
        }
        return;
    }
    if ct.is_empty() || ct.starts_with("text/plain") {
        if out.0.is_none() {
            out.0 = Some(decode_part(body, transfer_encoding, content_type));
        }
    } else if ct.starts_with("text/html") && out.1.is_none() {
        out.1 = Some(strip_html(&decode_part(body, transfer_encoding, content_type)));
    }
}

/// The new text of a reply, as plain text, quoted original removed, at most
/// EXCERPT_MAX characters.
pub fn reply_text(content_type: &str, transfer_encoding: &str, body: &str) -> String {
    let mut found = (None, None);
    walk_mime(content_type, transfer_encoding, body, 0, &mut found);
    let text = found.0.filter(|t| !t.trim().is_empty()).or(found.1).unwrap_or_default();
    let clean = strip_quoted(&text.replace("\r\n", "\n"));
    clean.chars().take(EXCERPT_MAX).collect()
}

/// IMAP host to use: the explicit `imap_host` setting, else derived from the
/// SMTP host ("smtp.x" -> "imap.x", "smtp-x" -> "imap-x").
/// IMAP host guesses for an SMTP host, most likely first.
pub(crate) fn imap_host_candidates(smtp_host: &str) -> Vec<String> {
    let first = derive_imap_host(smtp_host);
    let h = smtp_host.trim();
    let mut out = vec![first];
    for (pre, alt) in [("smtp.", "mail."), ("smtp-", "mail-")] {
        if let Some(rest) = h.strip_prefix(pre) {
            out.push(format!("{alt}{rest}"));
        }
    }
    out.push(h.to_string());
    out.dedup();
    out
}

pub fn derive_imap_host(smtp_host: &str) -> String {
    let h = smtp_host
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/');
    if let Some(rest) = h.strip_prefix("smtp.") {
        format!("imap.{}", rest)
    } else if let Some(rest) = h.strip_prefix("smtp-") {
        format!("imap-{}", rest)
    } else {
        h.to_string()
    }
}

/// IMAP SEARCH date format ("1-Sep-2026").
fn imap_date(d: chrono::NaiveDate) -> String {
    d.format("%-d-%b-%Y").to_string()
}

// ---------------- Reply sync (IMAP) ----------------

/// Oldest unanswered email considered, and the hard cap on the scan window.
const SYNC_WINDOW_DAYS: i64 = 60;
/// At most this many inbox messages per sync (newest first).
const MAX_SCAN: usize = 1500;

static SYNC_RUNNING: AtomicBool = AtomicBool::new(false);

struct SyncGuard;
impl Drop for SyncGuard {
    fn drop(&mut self) {
        SYNC_RUNNING.store(false, Ordering::SeqCst);
    }
}

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct SyncResult {
    pub scanned: usize,
    pub replies: usize,
    pub bounces: usize,
    pub unsubscribes: usize,
    pub no_answer: usize,
    #[serde(default)]
    pub skipped: bool,
}

#[derive(Serialize, Deserialize, Default)]
struct SyncMark {
    uid_validity: u32,
    last_uid: u32,
}

fn get_setting(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM settings WHERE key = ?1", params![key], |r| r.get(0))
        .ok()
        .filter(|s: &String| !s.trim().is_empty())
}

fn put_setting(conn: &Connection, key: &str, value: &str) {
    let _ = conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    );
}

type ImapSession = imap::Session<native_tls::TlsStream<std::net::TcpStream>>;

fn imap_connect(host: &str, port: u16, user: &str, pass: &str) -> Result<ImapSession, String> {
    use std::net::ToSocketAddrs;
    use std::time::Duration;
    let addr = (host, port)
        .to_socket_addrs()
        .map_err(|e| format!("IMAP {}: {}", host, e))?
        .next()
        .ok_or_else(|| format!("IMAP {}: adresse introuvable", host))?;
    let tcp = std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(15))
        .map_err(|e| format!("IMAP {}:{}: {}", host, port, e))?;
    let _ = tcp.set_read_timeout(Some(Duration::from_secs(60)));
    let _ = tcp.set_write_timeout(Some(Duration::from_secs(30)));
    let tls = native_tls::TlsConnector::new().map_err(|e| e.to_string())?;
    let stream = tls.connect(host, tcp).map_err(|e| format!("IMAP TLS: {}", e))?;
    let mut client = imap::Client::new(stream);
    client.read_greeting().map_err(|e| e.to_string())?;
    client
        .login(user, pass)
        .map_err(|(e, _)| format!("IMAP login: {}", e))
}

struct SentRef {
    email_id: i64,
    contact_id: Option<i64>,
    campaign_id: Option<i64>,
    to_addr: String,
}

fn sent_by_message_ids(conn: &Connection, ids: &[String]) -> Option<SentRef> {
    for id in ids.iter().rev() {
        if let Ok(r) = conn.query_row(
            "SELECT id, contact_id, campaign_id, to_addr FROM emails WHERE message_id = ?1 AND status = 'sent'",
            params![id],
            |r| Ok(SentRef { email_id: r.get(0)?, contact_id: r.get(1)?, campaign_id: r.get(2)?, to_addr: r.get(3)? }),
        ) {
            return Some(r);
        }
    }
    None
}

/// Fallback when the reply carries no usable reference: the latest email sent
/// to that address before the reply arrived, else a contact with that address
/// that we have mailed.
fn sent_by_address(conn: &Connection, addr: &str, received: &str) -> Option<SentRef> {
    conn.query_row(
        "SELECT id, contact_id, campaign_id, to_addr FROM emails
         WHERE status = 'sent' AND lower(trim(to_addr)) = ?1 AND sent_at <= ?2
         ORDER BY sent_at DESC LIMIT 1",
        params![addr, received],
        |r| Ok(SentRef { email_id: r.get(0)?, contact_id: r.get(1)?, campaign_id: r.get(2)?, to_addr: r.get(3)? }),
    )
    .ok()
    .or_else(|| {
        conn.query_row(
            "SELECT m.id, c.id, m.campaign_id, c.email FROM contacts c
             JOIN emails m ON m.contact_id = c.id AND m.status = 'sent' AND m.sent_at <= ?2
             WHERE lower(trim(c.email)) = ?1
             ORDER BY m.sent_at DESC LIMIT 1",
            params![addr, received],
            |r| Ok(SentRef { email_id: r.get(0)?, contact_id: r.get(1)?, campaign_id: r.get(2)?, to_addr: r.get(3)? }),
        )
        .ok()
    })
}

/// Record one inbound message. Returns false when it was already recorded
/// (re-scans are idempotent thanks to the UNIQUE message_id).
fn record_inbound(
    conn: &Connection,
    sent: Option<&SentRef>,
    from: &str,
    subject: &str,
    message_id: &str,
    kind: &str,
    matched_by: &str,
    received: &str,
) -> bool {
    conn.execute(
        "INSERT OR IGNORE INTO email_replies
            (email_id, contact_id, from_addr, subject, message_id, kind, matched_by, received_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            sent.map(|s| s.email_id),
            sent.and_then(|s| s.contact_id),
            from,
            subject,
            message_id,
            kind,
            matched_by,
            received
        ],
    )
    .unwrap_or(0)
        > 0
}

fn apply_reply(conn: &Connection, sent: &SentRef, from: &str, received: &str) {
    let _ = conn.execute(
        "UPDATE emails SET replied_at = COALESCE(replied_at, ?2) WHERE id = ?1",
        params![sent.email_id, received],
    );
    status::on_reply(conn, sent.contact_id, from);
    if let (Some(camp), Some(cid)) = (sent.campaign_id, sent.contact_id) {
        crate::commands::campaigns::mark_audience(conn, camp, cid, "replied");
    }
}

/// A plain "no thanks" closes the booking (status.rs). Residency replies are
/// left to the residency agent, which reads them with more context.
fn refusal_from_text(conn: &Connection, message_id: &str, text: &str) {
    let row: Option<(Option<i64>, String)> = conn
        .query_row(
            "SELECT r.contact_id, r.from_addr FROM email_replies r
             LEFT JOIN emails m ON m.id = r.email_id
             LEFT JOIN campaigns c ON c.id = m.campaign_id
             WHERE r.message_id = ?1 AND r.kind = 'reply' AND COALESCE(c.kind, '') <> 'residency'",
            params![message_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();
    if let Some((cid, from)) = row {
        status::on_reply_text(conn, cid, &from, text);
    }
}

pub fn sync_replies_blocking(pool: &DbPool) -> Result<SyncResult, String> {
    if SYNC_RUNNING.swap(true, Ordering::SeqCst) {
        return Ok(SyncResult { skipped: true, ..Default::default() });
    }
    let _guard = SyncGuard;
    let cfg = crate::commands::email::load_smtp_from(pool)?;
    let conn = pool.get().map_err(|e| e.to_string())?;
    let res = run_sync(&conn, &cfg);
    put_setting(&conn, "imap_last_sync", &chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string());
    match &res {
        Ok(r) => {
            put_setting(&conn, "imap_last_result", &serde_json::to_string(r).unwrap_or_default());
            // Last *successful* scan: the residency autopilot refuses to send
            // when this is stale (stop-on-reply would be blind).
            put_setting(&conn, "imap_last_ok", &chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string());
            let _ = conn.execute("DELETE FROM settings WHERE key = 'imap_last_error'", []);
        }
        Err(e) => put_setting(&conn, "imap_last_error", e),
    }
    res
}

/// IMAP (host, port, user) of the sending account, derived like the reply sync.
fn imap_target(conn: &Connection, cfg: &crate::models::SmtpConfig) -> Result<(String, u16, String), String> {
    let own = cfg.from_email.trim().to_string();
    let user = if cfg.username.trim().is_empty() { own } else { cfg.username.trim().to_string() };
    if user.is_empty() || cfg.password.trim().is_empty() {
        return Err("Identifiants email non configurés (Réglages, Email).".into());
    }
    let port: u16 = get_setting(conn, "imap_port")
        .and_then(|p| p.trim().parse().ok())
        .unwrap_or(993);
    let host = match get_setting(conn, "imap_host").filter(|h| !h.trim().is_empty()) {
        Some(h) => h,
        // Guessed names do not always exist (securemail: smtp-fr resolves,
        // imap-fr does not, the IMAP server is mail-fr): keep the first that resolves.
        None => imap_host_candidates(&cfg.host)
            .into_iter()
            .find(|h| {
                use std::net::ToSocketAddrs;
                (h.as_str(), port).to_socket_addrs().map(|mut a| a.next().is_some()).unwrap_or(false)
            })
            .unwrap_or_else(|| derive_imap_host(&cfg.host)),
    };
    if host.is_empty() {
        return Err("Serveur IMAP non configuré.".into());
    }
    Ok((host, port, user))
}

/// The Sent mailbox: the one flagged \Sent (RFC 6154), else a usual name.
pub(crate) fn pick_sent_folder(folders: &[(String, Vec<String>)]) -> Option<String> {
    if let Some((name, _)) = folders.iter().find(|(_, attrs)| attrs.iter().any(|a| a.eq_ignore_ascii_case("\\Sent"))) {
        return Some(name.clone());
    }
    const NAMES: &[&str] = &["sent", "inbox.sent", "sent items", "sent messages", "envoyés", "inbox.envoyés", "éléments envoyés", "messages envoyés"];
    NAMES.iter().find_map(|n| folders.iter().find(|(f, _)| f.to_lowercase() == *n).map(|(f, _)| f.clone()))
}

/// Keep a copy of a sent email in the account's Sent folder, as a mail client
/// does after SMTP (SMTP alone leaves nothing there). Best effort, off the
/// sending thread; turned off with the setting `imap_save_sent` = "0".
pub(crate) fn save_to_sent(pool: &DbPool, cfg: &crate::models::SmtpConfig, raw: Vec<u8>) {
    let Ok(conn) = pool.get() else { return };
    if get_setting(&conn, "imap_save_sent").as_deref() == Some("0") {
        return;
    }
    let Ok((host, port, user)) = imap_target(&conn, cfg) else { return };
    let cached = get_setting(&conn, "imap_sent_folder");
    let pass = cfg.password.trim().to_string();
    let pool = pool.clone();
    drop(conn);
    std::thread::spawn(move || {
        let res = (|| -> Result<String, String> {
            let mut session = imap_connect(&host, port, &user, &pass)?;
            let folder = match cached {
                Some(f) => f,
                None => {
                    let names = session.list(Some(""), Some("*")).map_err(|e| e.to_string())?;
                    let folders: Vec<(String, Vec<String>)> = names
                        .iter()
                        .map(|n| {
                            let attrs = n
                                .attributes()
                                .iter()
                                .map(|a| match a {
                                    imap::types::NameAttribute::Custom(c) => c.to_string(),
                                    other => format!("{:?}", other),
                                })
                                .collect();
                            (n.name().to_string(), attrs)
                        })
                        .collect();
                    pick_sent_folder(&folders).ok_or("dossier Envoyés introuvable")?
                }
            };
            session.append_with_flags(&folder, &raw, &[imap::types::Flag::Seen]).map_err(|e| e.to_string())?;
            let _ = session.logout();
            Ok(folder)
        })();
        if let Ok(conn) = pool.get() {
            match res {
                Ok(folder) => put_setting(&conn, "imap_sent_folder", &folder),
                Err(e) => put_setting(&conn, "imap_sent_error", &e),
            }
        }
    });
}

fn run_sync(conn: &Connection, cfg: &crate::models::SmtpConfig) -> Result<SyncResult, String> {
    let mut out = SyncResult::default();
    let own = cfg.from_email.trim().to_string();
    let (host, port, user) = imap_target(conn, cfg)?;

    // Window: from the oldest unanswered email of the last SYNC_WINDOW_DAYS.
    let oldest: Option<String> = conn
        .query_row(
            &format!(
                "SELECT MIN(sent_at) FROM emails WHERE status = 'sent' AND replied_at IS NULL
                   AND sent_at >= datetime('now', '-{} days')",
                SYNC_WINDOW_DAYS
            ),
            [],
            |r| r.get(0),
        )
        .ok()
        .flatten();
    let Some(oldest) = oldest else {
        out.no_answer = status::apply_no_answer(conn);
        return Ok(out);
    };
    let since = chrono::NaiveDateTime::parse_from_str(&oldest, "%Y-%m-%d %H:%M:%S")
        .map(|d| d.date())
        .unwrap_or_else(|_| chrono::Utc::now().date_naive())
        - chrono::Duration::days(1);

    let mut session = imap_connect(&host, port, &user, cfg.password.trim())?;
    // EXAMINE = read-only: the server never sets \Seen on what we fetch.
    let mbox = session.examine("INBOX").map_err(|e| e.to_string())?;
    let validity = mbox.uid_validity.unwrap_or(0);
    let mark: SyncMark = get_setting(conn, "imap_sync_mark")
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_default();
    let min_uid = if mark.uid_validity == validity { mark.last_uid } else { 0 };

    let mut uids: Vec<u32> = session
        .uid_search(format!("SINCE {}", imap_date(since)))
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|u| *u > min_uid)
        .collect();
    uids.sort_unstable();
    if uids.len() > MAX_SCAN {
        uids.drain(..uids.len() - MAX_SCAN);
    }

    let sent_addrs: HashSet<String> = {
        let mut stmt = conn
            .prepare(&format!(
                "SELECT DISTINCT lower(trim(to_addr)) FROM emails
                 WHERE status = 'sent' AND sent_at >= datetime('now', '-{} days')",
                SYNC_WINDOW_DAYS + 30
            ))
            .map_err(|e| e.to_string())?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0)).map_err(|e| e.to_string())?;
        rows.filter_map(|r| r.ok()).collect()
    };

    let mut bounce_uids: Vec<(u32, InboundHeaders, String)> = Vec::new();
    // Replies (and matched auto-replies) whose text is fetched afterwards, for
    // the residency agent: (uid, message-id, content-type, transfer-encoding).
    let mut text_uids: Vec<(u32, String, String, String)> = Vec::new();
    let mut max_uid = min_uid;
    for chunk in uids.chunks(50) {
        let set = chunk.iter().map(|u| u.to_string()).collect::<Vec<_>>().join(",");
        let fetches = session
            .uid_fetch(&set, "(UID INTERNALDATE BODY.PEEK[HEADER])")
            .map_err(|e| e.to_string())?;
        for f in fetches.iter() {
            let uid = f.uid.unwrap_or(0);
            max_uid = max_uid.max(uid);
            out.scanned += 1;
            let Some(raw) = f.header() else { continue };
            let h = read_inbound(raw);
            let received = f
                .internal_date()
                .map(|d| d.with_timezone(&chrono::Utc).format("%Y-%m-%d %H:%M:%S").to_string())
                .unwrap_or_else(|| chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string());
            let mid = h
                .message_id
                .clone()
                .unwrap_or_else(|| format!("<imap-{}-{}@local>", validity, uid));
            let from = h.from.clone().unwrap_or_default();
            match classify(&h, &own) {
                Inbound::Reply => {
                    let (sent, by) = match sent_by_message_ids(conn, &h.refs) {
                        Some(s) => (Some(s), "message_id"),
                        None => (sent_by_address(conn, &from, &received), "address"),
                    };
                    let Some(sent) = sent else { continue };
                    if record_inbound(conn, Some(&sent), &from, &h.subject, &mid, "reply", by, &received) {
                        apply_reply(conn, &sent, &from, &received);
                        out.replies += 1;
                        text_uids.push((uid, mid.clone(), h.content_type.clone(), h.transfer_encoding.clone()));
                    }
                }
                // An out-of-office to one of our emails is not a reply (nothing
                // moves in the pipeline) but the residency sequence reschedules
                // around it, so it is recorded when it points at a sent email.
                Inbound::AutoReply => {
                    let Some(sent) = sent_by_message_ids(conn, &h.refs) else { continue };
                    if record_inbound(conn, Some(&sent), &from, &h.subject, &mid, "auto_reply", "message_id", &received) {
                        text_uids.push((uid, mid.clone(), h.content_type.clone(), h.transfer_encoding.clone()));
                    }
                }
                Inbound::Unsubscribe => {
                    if !sent_addrs.contains(&from) && sent_by_address(conn, &from, &received).is_none() {
                        continue;
                    }
                    if record_inbound(conn, None, &from, &h.subject, &mid, "unsubscribe", "address", &received) {
                        suppress(conn, &from, "unsubscribe", Some(&h.subject));
                        status::on_refused(conn, None, &from);
                        out.unsubscribes += 1;
                    }
                }
                Inbound::Bounce => bounce_uids.push((uid, h, received)),
                Inbound::Ignore => {}
            }
        }
    }

    // Bounce reports need their body (Final-Recipient lines, quoted original).
    for (uid, h, received) in bounce_uids {
        let mid = h
            .message_id
            .clone()
            .unwrap_or_else(|| format!("<imap-{}-{}@local>", validity, uid));
        let already: bool = conn
            .query_row("SELECT 1 FROM email_replies WHERE message_id = ?1", params![mid], |_| Ok(true))
            .unwrap_or(false);
        if already {
            continue;
        }
        let fetches = session
            .uid_fetch(uid.to_string(), "(UID BODY.PEEK[TEXT])")
            .map_err(|e| e.to_string())?;
        let body = fetches
            .iter()
            .find_map(|f| f.text().map(|t| String::from_utf8_lossy(t).into_owned()))
            .unwrap_or_default();
        // The original Message-ID quoted in the report pins the exact email.
        let quoted = sent_by_message_ids(conn, &extract_ids(&body));
        let mut failed = bounced_recipients(&h.failed_recipients, &body, &|a| sent_addrs.contains(a));
        if failed.is_empty() {
            if let Some(q) = &quoted {
                failed.push(norm_addr(&q.to_addr));
            }
        }
        if failed.is_empty() {
            continue;
        }
        let from = h.from.clone().unwrap_or_default();
        if record_inbound(conn, quoted.as_ref(), &from, &h.subject, &mid, "bounce", "report", &received) {
            for a in &failed {
                if suppress(conn, a, "bounce", Some(&h.subject)) {
                    out.bounces += 1;
                }
            }
        }
    }

    // Text of the new replies (best effort: a failure here keeps the reply).
    for (uid, mid, ct, te) in text_uids {
        let Ok(fetches) = session.uid_fetch(uid.to_string(), "(UID BODY.PEEK[TEXT])") else { continue };
        let Some(raw) = fetches.iter().find_map(|f| f.text().map(|t| String::from_utf8_lossy(t).into_owned())) else {
            continue;
        };
        let text = reply_text(&ct, &te, &raw);
        let _ = conn.execute(
            "UPDATE email_replies SET body_excerpt = ?2 WHERE message_id = ?1",
            params![mid, text],
        );
        refusal_from_text(conn, &mid, &text);
    }

    let _ = session.logout();
    put_setting(
        conn,
        "imap_sync_mark",
        &serde_json::to_string(&SyncMark { uid_validity: validity, last_uid: max_uid }).unwrap_or_default(),
    );
    out.no_answer = status::apply_no_answer(conn);
    Ok(out)
}

/// Scan the inbox for replies, bounces and unsubscribe requests. Runs off the
/// async runtime (network + DB work can take a while). A second call while one
/// is running returns at once with `skipped`.
#[tauri::command]
pub async fn sync_replies(state: State<'_, AppState>) -> Result<SyncResult, String> {
    let pool = state.pool.clone();
    let p2 = pool.clone();
    let res = tauri::async_runtime::spawn_blocking(move || sync_replies_blocking(&pool))
        .await
        .map_err(|e| e.to_string())?;
    // New replies to residency campaigns get classified in the background.
    if matches!(&res, Ok(r) if !r.skipped) {
        crate::commands::residency::engine::classify_in_background(p2);
    }
    res
}

#[derive(Serialize)]
pub struct ReplySyncStatus {
    pub host: String,
    pub derived: bool,
    pub last_sync: Option<String>,
    pub last_error: Option<String>,
    pub last_result: Option<SyncResult>,
}

#[tauri::command(async)]
pub fn reply_sync_status(state: State<'_, AppState>) -> Result<ReplySyncStatus, String> {
    let cfg = crate::commands::email::load_smtp_from(&state.pool)?;
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let explicit = get_setting(&conn, "imap_host");
    Ok(ReplySyncStatus {
        derived: explicit.is_none(),
        host: explicit.unwrap_or_else(|| derive_imap_host(&cfg.host)),
        last_sync: get_setting(&conn, "imap_last_sync"),
        last_error: get_setting(&conn, "imap_last_error"),
        last_result: get_setting(&conn, "imap_last_result").and_then(|j| serde_json::from_str(&j).ok()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imap_candidates_try_mail_prefix() {
        assert_eq!(
            imap_host_candidates("smtp-fr.securemail.pro"),
            vec!["imap-fr.securemail.pro", "mail-fr.securemail.pro", "smtp-fr.securemail.pro"]
        );
    }

    #[test]
    fn sent_folder_prefers_special_use_flag() {
        let f = |v: &[(&str, &[&str])]| v.iter().map(|(n, a)| (n.to_string(), a.iter().map(|x| x.to_string()).collect())).collect::<Vec<_>>();
        assert_eq!(pick_sent_folder(&f(&[("INBOX", &[]), ("Mes envois", &["\\HasNoChildren", "\\Sent"])])).as_deref(), Some("Mes envois"));
        assert_eq!(pick_sent_folder(&f(&[("INBOX", &[]), ("INBOX.Sent", &[])])).as_deref(), Some("INBOX.Sent"));
        assert_eq!(pick_sent_folder(&f(&[("INBOX", &[]), ("Envoyés", &[])])).as_deref(), Some("Envoyés"));
        assert_eq!(pick_sent_folder(&f(&[("INBOX", &[])])), None);
    }

    #[test]
    fn message_id_shape_and_uniqueness() {
        let a = gen_message_id("Booking <booking@Insrt.fr>");
        let b = gen_message_id("booking@insrt.fr");
        assert!(a.starts_with('<') && a.ends_with("@insrt.fr>"), "{}", a);
        assert_ne!(a, b);
        assert!(gen_message_id("not-an-address").ends_with("@pierscrm.local>"));
        assert_eq!(extract_ids(&a), vec![a.clone()]);
    }

    #[test]
    fn quota_math() {
        assert_eq!(parse_cap(None), 150);
        assert_eq!(parse_cap(Some(" 40 ")), 40);
        assert_eq!(parse_cap(Some("0")), 150);
        assert_eq!(parse_cap(Some("abc")), 150);
        assert_eq!(parse_cap(Some("999999")), 150);
        assert_eq!(remaining(150, 20), 130);
        assert_eq!(remaining(150, 200), 0);
    }

    #[test]
    fn smtp_failure_classification() {
        assert!(is_permanent_recipient_failure(Some(550), "5.1.1 <x@y.nl>: Recipient address rejected: User unknown"));
        assert!(is_permanent_recipient_failure(Some(554), "5.1.1 mailbox does not exist"));
        assert!(!is_permanent_recipient_failure(Some(550), "5.7.1 Message rejected as spam"));
        assert!(!is_permanent_recipient_failure(Some(535), "5.7.8 Authentication credentials invalid"));
        assert!(!is_permanent_recipient_failure(Some(451), "4.7.1 try again later"));
        assert!(!is_permanent_recipient_failure(None, "connection refused"));
    }

    #[test]
    fn headers_and_reply_matching() {
        let raw = b"From: =?UTF-8?Q?Jos=C3=A9_P=C3=A9rez?= <Jose@Club.NL>\r\n\
Subject: =?utf-8?B?UmU6IEJvb2tpbmcgUGllcnM=?=\r\n\
Message-ID: <abc@club.nl>\r\n\
In-Reply-To: <18f.2.ff@insrt.fr>\r\n\
References: <17a.1.aa@insrt.fr>\r\n <18f.2.ff@insrt.fr>\r\n\r\n";
        let h = read_inbound(raw);
        assert_eq!(h.from.as_deref(), Some("jose@club.nl"));
        assert_eq!(h.subject, "Re: Booking Piers");
        assert_eq!(h.message_id.as_deref(), Some("<abc@club.nl>"));
        assert_eq!(h.refs, vec!["<18f.2.ff@insrt.fr>".to_string(), "<17a.1.aa@insrt.fr>".to_string()]);
        assert_eq!(classify(&h, "booking@insrt.fr"), Inbound::Reply);
        // Our own sent copy is ignored.
        assert_eq!(classify(&h, "jose@club.nl"), Inbound::Ignore);
        assert_eq!(decode_words("=?ISO-8859-1?Q?caf=E9?= ok"), "café ok");
    }

    #[test]
    fn reply_matching_against_db() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE emails (id INTEGER PRIMARY KEY, contact_id INTEGER, campaign_id INTEGER,
                to_addr TEXT, status TEXT, sent_at TEXT, message_id TEXT, replied_at TEXT);
             CREATE TABLE contacts (id INTEGER PRIMARY KEY, email TEXT);
             INSERT INTO emails VALUES (1, 5, 2, 'Jose@club.nl', 'sent', '2026-10-01 10:00:00', '<m1@insrt.fr>', NULL);
             INSERT INTO emails VALUES (2, 5, NULL, 'jose@club.nl', 'sent', '2026-10-08 10:00:00', '<m2@insrt.fr>', NULL);",
        )
        .unwrap();
        let by_id = sent_by_message_ids(&c, &["<zz@x>".into(), "<m1@insrt.fr>".into()]).unwrap();
        assert_eq!(by_id.email_id, 1);
        assert_eq!(by_id.campaign_id, Some(2));
        // Address fallback: the latest email sent before the reply arrived.
        assert_eq!(sent_by_address(&c, "jose@club.nl", "2026-10-05 00:00:00").unwrap().email_id, 1);
        assert_eq!(sent_by_address(&c, "jose@club.nl", "2026-10-09 00:00:00").unwrap().email_id, 2);
        assert!(sent_by_address(&c, "jose@club.nl", "2026-09-01 00:00:00").is_none());
    }

    #[test]
    fn bounce_and_auto_reply_detection() {
        let raw = b"From: Mail Delivery System <MAILER-DAEMON@mx.example.com>\r\n\
Subject: Undelivered Mail Returned to Sender\r\n\
Content-Type: multipart/report; report-type=delivery-status; boundary=\"x\"\r\n\r\n";
        let h = read_inbound(raw);
        assert_eq!(classify(&h, "booking@insrt.fr"), Inbound::Bounce);
        let body = "Reporting-MTA: dns; mx.example.com\r\n\
Final-Recipient: rfc822; Gone@Venue.nl\r\nAction: failed\r\nStatus: 5.1.1\r\n\
Also quoting someone@else.com in the text.";
        let known = |a: &str| a == "gone@venue.nl";
        assert_eq!(bounced_recipients(&[], body, &known), vec!["gone@venue.nl".to_string()]);
        // Delay notices are not bounces.
        let delayed = "Final-Recipient: rfc822; gone@venue.nl\nAction: delayed\n";
        assert!(bounced_recipients(&[], delayed, &known).is_empty());
        // Unknown addresses are never suppressed.
        assert!(bounced_recipients(&[], "Final-Recipient: rfc822; other@x.nl\nAction: failed", &known).is_empty());

        let ooo = read_inbound(b"From: a@club.nl\r\nSubject: Automatic reply: Booking\r\nAuto-Submitted: auto-replied\r\n\r\n");
        assert_eq!(classify(&ooo, "booking@insrt.fr"), Inbound::AutoReply);
        let unsub = read_inbound(b"From: a@club.nl\r\nSubject: unsubscribe\r\n\r\n");
        assert_eq!(classify(&unsub, "booking@insrt.fr"), Inbound::Unsubscribe);
    }

    #[test]
    fn reply_text_extraction() {
        let body = "--b1\r\nContent-Type: text/plain; charset=\"utf-8\"\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\nBonjour, tr=C3=A8s int=C3=A9ress=C3=A9s !\r\nOn peut en parler =\r\njeudi.\r\n\r\nLe 28 oct. 2026 =C3=A0 10:00, Piers <booking@insrt.fr> a =C3=A9crit :\r\n> Bonjour\r\n--b1\r\nContent-Type: text/html\r\n\r\n<p>ignored</p>\r\n--b1--";
        let t = reply_text("multipart/alternative; boundary=\"b1\"", "", body);
        assert_eq!(t, "Bonjour, très intéressés !\nOn peut en parler jeudi.");
        // HTML only, base64.
        let html = "PHA+SGVsbG8gPGI+d29ybGQ8L2I+PC9wPjxibG9ja3F1b3RlPm9sZDwvYmxvY2txdW90ZT4=";
        assert_eq!(reply_text("text/html; charset=utf-8", "base64", html), "Hello  world");
        // Plain, with a quoted tail.
        assert_eq!(reply_text("", "", "Not now, maybe in March.\n\nOn Tue, Piers wrote:\n> hi"), "Not now, maybe in March.");
    }

    #[test]
    fn imap_host_derivation() {
        assert_eq!(derive_imap_host("smtp.gmail.com"), "imap.gmail.com");
        assert_eq!(derive_imap_host("smtp-fr.securemail.pro"), "imap-fr.securemail.pro");
        assert_eq!(derive_imap_host("mail.example.com"), "mail.example.com");
        assert_eq!(imap_date(chrono::NaiveDate::from_ymd_opt(2026, 9, 1).unwrap()), "1-Sep-2026");
    }
}
