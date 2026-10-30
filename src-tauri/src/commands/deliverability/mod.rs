//! Protecting the sending address ("délivrabilité").
//!
//! - `dns`: SPF / DKIM / DMARC / MX check of the sending domain, with the
//!   exact record to add. SPF or DKIM failing blocks the residency autopilot.
//! - `budget`: one sending budget shared by every automated path (daily and
//!   hourly caps, warm-up ramp, randomized spacing, per-domain daily cap).
//! - `health`: 7-day bounce / complaint / unsubscribe / reply rates per
//!   sending address; above the thresholds, automated sending pauses.
//! - One-click unsubscribe (RFC 8058) when an HTTPS unsubscribe endpoint is
//!   configured.
//!
//! Hooks into the shared send paths are single calls: `send_block` (email.rs,
//! before SMTP), `record_send` (after), `automated_block` / `domain_capped`
//! (residency engine's pick_next, bulk loop) and `on_tick` (engine tick).

pub mod budget;
pub mod dns;
pub mod health;

use chrono::{DateTime, Utc};
use lettre::message::header::{Header, HeaderName, HeaderValue};
use rusqlite::Connection;
use serde::Serialize;
use tauri::State;

use crate::AppState;

pub(crate) use crate::commands::residency::engine::{fmt, get, parse_utc, put};

pub const DNS_REPORT_KEY: &str = "deliverability_dns";
pub const UNSUB_BASE_KEY: &str = "unsubscribe_base_url";
const UNSUB_POLL_KEY: &str = "unsubscribe_last_poll";

/// Additive migration: the sending address of each email (health per sender).
pub fn migrate(conn: &Connection) {
    let _ = conn.execute("ALTER TABLE emails ADD COLUMN from_addr TEXT", []);
}

// ---------------- Gates for the send paths ----------------

/// Why a send must not go out now (code for SendResult, French reason).
/// Hard caps only (daily, hourly, domain): the random spacing is the
/// schedulers' business, so a one-off Compose send never waits for it.
pub fn send_block(conn: &Connection, addr: &str, now: DateTime<Utc>) -> Option<(String, String)> {
    match budget::check(conn, addr, now, false) {
        budget::Decision::Allow => None,
        budget::Decision::Wait { code, reason, .. } | budget::Decision::Defer { code, reason } => Some((code.into(), reason)),
        budget::Decision::SkipDomain { reason } => Some(("budget_domain".into(), reason)),
    }
}

/// May an automated send (autopilot, bulk) go out now, whoever the recipient?
/// Health pause first, then the shared budget with its spacing.
pub fn automated_block(conn: &Connection, now: DateTime<Utc>) -> Option<String> {
    if let Some(p) = health::current_pause(conn) {
        return Some(p.reason);
    }
    match budget::check_global(conn, now) {
        budget::Decision::Allow => None,
        budget::Decision::Wait { reason, .. } | budget::Decision::Defer { reason, .. } | budget::Decision::SkipDomain { reason } => Some(reason),
    }
}

/// This recipient's organisation domain had its share today.
pub fn domain_capped(conn: &Connection, addr: &str, now: DateTime<Utc>) -> bool {
    matches!(budget::check(conn, addr, now, false), budget::Decision::SkipDomain { .. })
}

pub fn record_send(conn: &Connection, email_id: Option<i64>, from_email: &str, to: &str, sent: bool) {
    budget::record_send(conn, email_id, from_email, to, sent, Utc::now());
}

/// Engine tick (every 5 minutes): health re-check, unsubscribe poll.
pub fn on_tick(conn: &Connection, now: DateTime<Utc>) {
    health::refresh(conn, now);
    let due = get(conn, UNSUB_POLL_KEY)
        .and_then(|s| parse_utc(&s))
        .map_or(true, |t| now - t >= chrono::Duration::minutes(30));
    if due && unsubscribe_base(conn).is_some() {
        if let Err(e) = poll_unsubscribes(conn) {
            eprintln!("[deliverability] {}", e);
        }
        put(conn, UNSUB_POLL_KEY, &fmt(now));
    }
}

// ---------------- List-Unsubscribe ----------------

/// `List-Unsubscribe`: the HTTPS one-click link first when configured, the
/// mailto always (read by the reply sync).
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

/// `List-Unsubscribe-Post: List-Unsubscribe=One-Click` (RFC 8058). Only valid
/// next to an HTTPS List-Unsubscribe URI, so only sent with one.
#[derive(Clone)]
pub struct ListUnsubscribePost;

impl Header for ListUnsubscribePost {
    fn name() -> HeaderName {
        HeaderName::new_from_ascii_str("List-Unsubscribe-Post")
    }
    fn parse(_: &str) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        Ok(Self)
    }
    fn display(&self) -> HeaderValue {
        HeaderValue::new(Self::name(), "List-Unsubscribe=One-Click".to_string())
    }
}

/// The configured HTTPS unsubscribe endpoint (`unsubscribe_base_url`).
pub fn unsubscribe_base(conn: &Connection) -> Option<String> {
    crate::secrets::read_setting(conn, UNSUB_BASE_KEY)
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| s.starts_with("https://") && s.len() > 8)
}

/// Header value: `<https://base/u/token>, <mailto:from?subject=unsubscribe>`.
pub fn list_unsubscribe_value(from_email: &str, base: Option<&str>, token: &str) -> String {
    let mailto = format!("<mailto:{}?subject=unsubscribe>", from_email.trim());
    match base {
        Some(b) => format!("<{}/u/{}>, {}", b.trim_end_matches('/'), token, mailto),
        None => mailto,
    }
}

/// Add both headers to a message being built.
pub fn with_unsubscribe(
    b: lettre::message::MessageBuilder,
    from_email: &str,
    base: Option<&str>,
    token: &str,
) -> lettre::message::MessageBuilder {
    let b = b.header(ListUnsubscribe(list_unsubscribe_value(from_email, base, token)));
    if base.is_some() {
        b.header(ListUnsubscribePost)
    } else {
        b
    }
}

/// Tokens in the endpoint's `unsubscribes.json`: a list of tokens, or of
/// objects with a `token` field.
pub fn parse_unsubscribes(raw: &str) -> Vec<String> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) else { return Vec::new() };
    let arr = v.as_array().cloned().or_else(|| v.get("unsubscribes").and_then(|u| u.as_array()).cloned()).unwrap_or_default();
    arr.iter()
        .filter_map(|e| e.as_str().map(String::from).or_else(|| e.get("token").and_then(|t| t.as_str()).map(String::from)))
        .filter(|t| !t.trim().is_empty())
        .collect()
}

/// Suppress the addresses behind one-click unsubscribe tokens.
pub fn apply_unsubscribes(conn: &Connection, tokens: &[String]) -> usize {
    let mut n = 0;
    for t in tokens {
        let addr: Option<String> = conn
            .query_row("SELECT to_addr FROM emails WHERE track_token = ?1", [t], |r| r.get(0))
            .ok();
        if let Some(a) = addr {
            if crate::commands::outbox::suppress(conn, &a, "unsubscribe", Some("lien de désinscription")) {
                crate::commands::status::on_refused(conn, None, &a);
                n += 1;
            }
        }
    }
    n
}

fn poll_unsubscribes(conn: &Connection) -> Result<usize, String> {
    let Some(base) = unsubscribe_base(conn) else { return Ok(0) };
    let raw = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?
        .get(format!("{}/unsubscribes.json", base))
        .send()
        .and_then(|r| r.error_for_status())
        .and_then(|r| r.text())
        .map_err(|e| format!("désinscriptions : {}", e))?;
    Ok(apply_unsubscribes(conn, &parse_unsubscribes(&raw)))
}

// ---------------- Commands ----------------

#[derive(Serialize)]
pub struct DeliverabilityStatus {
    pub sender: String,
    pub budget: budget::BudgetStatus,
    pub health: Vec<health::Stats>,
    pub thresholds: health::Thresholds,
    pub pause: Option<health::Pause>,
    /// Last DNS report (for the current sending domain only).
    pub dns: Option<dns::DnsReport>,
    pub one_click_unsubscribe: bool,
}

fn conn(state: &State<'_, AppState>) -> Result<r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>, String> {
    state.pool.get().map_err(|e| e.to_string())
}

pub fn stored_report(conn: &Connection) -> Option<dns::DnsReport> {
    let r: dns::DnsReport = serde_json::from_str(&get(conn, DNS_REPORT_KEY)?).ok()?;
    let me = health::current_sender(conn);
    (dns::domain_of(&me).as_deref() == Some(r.domain.as_str())).then_some(r)
}

#[tauri::command(async)]
pub fn deliverability_status(state: State<'_, AppState>) -> Result<DeliverabilityStatus, String> {
    let conn = conn(&state)?;
    let now = Utc::now();
    let pause = health::refresh(&conn, now);
    Ok(DeliverabilityStatus {
        sender: health::current_sender(&conn),
        budget: budget::status(&conn, now),
        health: health::stats(&conn, now),
        thresholds: health::load_thresholds(&conn),
        pause,
        dns: stored_report(&conn),
        one_click_unsubscribe: unsubscribe_base(&conn).is_some(),
    })
}

/// Look the sending domain up (DNS over HTTPS) and store the report.
#[tauri::command]
pub async fn deliverability_check_dns(state: State<'_, AppState>) -> Result<dns::DnsReport, String> {
    let pool = state.pool.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let cfg = crate::commands::email::load_smtp_from(&pool)?;
        if cfg.from_email.trim().is_empty() {
            return Err("Adresse d'envoi non configurée (Réglages, Email).".to_string());
        }
        let resolver = dns::DohResolver::new()?;
        let report = dns::check_domain(&resolver, &cfg.from_email, &cfg.host, &fmt(Utc::now()))?;
        let conn = pool.get().map_err(|e| e.to_string())?;
        put(&conn, DNS_REPORT_KEY, &serde_json::to_string(&report).map_err(|e| e.to_string())?);
        Ok(report)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(async)]
pub fn deliverability_save_budget(state: State<'_, AppState>, config: budget::BudgetConfig) -> Result<budget::BudgetConfig, String> {
    let c = conn(&state)?;
    budget::save_config(&c, config)
}

#[tauri::command(async)]
pub fn deliverability_save_thresholds(state: State<'_, AppState>, thresholds: health::Thresholds) -> Result<health::Thresholds, String> {
    let c = conn(&state)?;
    health::save_thresholds(&c, thresholds)
}

#[tauri::command(async)]
pub fn deliverability_resume(state: State<'_, AppState>) -> Result<(), String> {
    let c = conn(&state)?;
    health::resume(&c, Utc::now());
    crate::commands::residency::engine::wake();
    Ok(())
}

/// Why the residency autopilot may not be switched on: SPF or DKIM failing
/// on the last check of the sending domain, or a health pause.
pub fn autopilot_block(conn: &Connection) -> Option<String> {
    if let Some(p) = health::current_pause(conn) {
        return Some(format!("Envois automatiques en pause : {}", p.reason));
    }
    dns::autopilot_block(stored_report(conn).as_ref(), &health::current_sender(conn))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsubscribe_headers() {
        assert_eq!(
            list_unsubscribe_value("booking@insrt.fr", None, "tok"),
            "<mailto:booking@insrt.fr?subject=unsubscribe>"
        );
        assert_eq!(
            list_unsubscribe_value("booking@insrt.fr", Some("https://t.insrt.fr/"), "tok"),
            "<https://t.insrt.fr/u/tok>, <mailto:booking@insrt.fr?subject=unsubscribe>"
        );
        let from: lettre::message::Mailbox = "booking@insrt.fr".parse().unwrap();
        let msg = |base: Option<&str>| {
            let b = lettre::Message::builder().from(from.clone()).to(from.clone()).subject("s");
            String::from_utf8(with_unsubscribe(b, "booking@insrt.fr", base, "tok").body("x".to_string()).unwrap().formatted()).unwrap()
        };
        let plain = msg(None);
        assert!(plain.contains("List-Unsubscribe: <mailto:") && !plain.contains("List-Unsubscribe-Post"));
        let one_click = msg(Some("https://t.insrt.fr"));
        assert!(one_click.contains("List-Unsubscribe-Post: List-Unsubscribe=One-Click"));
        assert!(one_click.contains("<https://t.insrt.fr/u/tok>"));
    }

    #[test]
    fn unsubscribe_tokens() {
        assert_eq!(parse_unsubscribes(r#"["a","b"]"#), vec!["a", "b"]);
        assert_eq!(parse_unsubscribes(r#"{"unsubscribes":[{"token":"c","at":"x"}]}"#), vec!["c"]);
        assert!(parse_unsubscribes("nope").is_empty());
        let c = crate::commands::residency::testdb::conn();
        c.execute("INSERT INTO emails (to_addr, subject, body, status, track_token) VALUES ('x@club.fr','s','b','sent','t1')", []).unwrap();
        assert_eq!(apply_unsubscribes(&c, &["t1".into(), "zz".into()]), 1);
        assert_eq!(crate::commands::outbox::suppression_reason(&c, "x@club.fr").as_deref(), Some("unsubscribe"));
        assert_eq!(apply_unsubscribes(&c, &["t1".into()]), 0);
    }

    #[test]
    fn autopilot_gate_reads_stored_report() {
        let c = crate::commands::residency::testdb::conn();
        put(&c, "smtp", r#"{"from_email":"booking@insrt.fr"}"#);
        assert_eq!(autopilot_block(&c), None);
        let r = dns::DnsReport {
            domain: "insrt.fr".into(),
            provider: None,
            checks: vec![dns::Check {
                key: "dkim".into(),
                status: dns::Status::Fail,
                summary: String::new(),
                detail: None,
                found: None,
                fix: None,
            }],
            checked_at: String::new(),
            ready: false,
        };
        put(&c, DNS_REPORT_KEY, &serde_json::to_string(&r).unwrap());
        assert!(autopilot_block(&c).unwrap().contains("DKIM"));
        put(&c, "smtp", r#"{"from_email":"booking@other.fr"}"#);
        assert_eq!(autopilot_block(&c), None);
        health::resume(&c, Utc::now());
        put(&c, health::PAUSE_KEY, r#"{"reason":"rebonds","since":"x","sender":"y"}"#);
        assert!(autopilot_block(&c).unwrap().contains("rebonds"));
    }
}
