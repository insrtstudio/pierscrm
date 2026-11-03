//! Protecting the sending address ("délivrabilité").
//!
//! - `dns`: SPF / DKIM / DMARC / MX check of the sending domain, with the
//!   exact record to add. SPF or DKIM failing blocks the residency autopilot.
//! - `budget`: one sending budget shared by every automated path (daily and
//!   hourly caps, warm-up ramp, randomized spacing, per-domain daily cap).
//! - `health`: 7-day bounce / complaint / unsubscribe / reply rates per
//!   sending address; above the thresholds, automated sending pauses.
//! - One-click unsubscribe (RFC 8058): the HTTPS endpoint is the user's
//!   `unsubscribe_base_url`, else insrt.studio (`/u/<token>`) as soon as the
//!   insrt API is configured. Tokens are then signed with the insrt API token
//!   (insrt drops forged ones) and the recorded ones are polled back from
//!   `GET /api/crm/v1/unsubscribes` (see `poll_insrt_unsubscribes`).
//!
//! Hooks into the shared send paths are single calls: `send_block` (email.rs,
//! before SMTP), `record_send` (after), `automated_block` / `domain_capped`
//! (residency engine's pick_next, bulk loop) and `on_tick` (engine tick).

pub mod budget;
pub mod dns;
pub mod health;
pub mod verify;

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
/// Cursor (`next`) of the insrt unsubscribe feed.
pub const INSRT_UNSUB_CURSOR_KEY: &str = "insrt_unsubscribe_cursor";
/// Note of the suppressions made by a one-click unsubscribe (push notices key on it).
pub const ONE_CLICK_NOTE: &str = "lien de désinscription";

/// Additive migration: the sending address of each email (health per sender).
pub fn migrate(conn: &Connection) {
    let _ = conn.execute("ALTER TABLE emails ADD COLUMN from_addr TEXT", []);
    verify::migrate(conn);
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

/// Engine tick (every 5 minutes): health re-check, and the poll of a custom
/// unsubscribe endpoint (`{base}/unsubscribes.json`). The insrt endpoint is
/// polled with the EPK activity (`poll_insrt_unsubscribes`).
pub fn on_tick(conn: &Connection, now: DateTime<Utc>) {
    health::refresh(conn, now);
    let due = get(conn, UNSUB_POLL_KEY)
        .and_then(|s| parse_utc(&s))
        .map_or(true, |t| now - t >= chrono::Duration::minutes(30));
    if due && custom_unsubscribe_base(conn).is_some() {
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

fn https_base(s: &str) -> Option<String> {
    let s = s.trim().trim_end_matches('/');
    (s.starts_with("https://") && s.len() > 8).then(|| s.to_string())
}

/// The user's own HTTPS unsubscribe endpoint (`unsubscribe_base_url`).
pub fn custom_unsubscribe_base(conn: &Connection) -> Option<String> {
    crate::secrets::read_setting(conn, UNSUB_BASE_KEY).and_then(|s| https_base(&s))
}

/// Where the one-click link points and whether that is insrt. The user's
/// endpoint wins; otherwise insrt.studio (or `insrt_api_base` when it is
/// HTTPS) as soon as an insrt API token is configured.
pub fn effective_unsubscribe_base(custom: Option<&str>, insrt_token: Option<&str>, insrt_base: Option<&str>) -> Option<(String, bool)> {
    if let Some(c) = custom.and_then(https_base) {
        return Some((c, false));
    }
    insrt_token.filter(|t| !t.trim().is_empty())?;
    let base = insrt_base
        .filter(|b| b.trim().starts_with("http"))
        .unwrap_or(crate::commands::residency::insrt::DEFAULT_BASE);
    https_base(base).map(|b| (b, true))
}

fn insrt_token(conn: &Connection) -> Option<String> {
    crate::secrets::read_setting(conn, "insrt_api_token").filter(|t| !t.trim().is_empty())
}

/// What a send needs for List-Unsubscribe: the HTTPS endpoint (None: mailto
/// only) and the key that signs the tokens (the insrt API token).
#[derive(Debug, Clone, Default)]
pub struct UnsubConfig {
    pub base: Option<String>,
    pub via_insrt: bool,
    pub key: Option<String>,
}

impl UnsubConfig {
    /// The token of one email (also its open-tracking token).
    pub fn new_token(&self) -> String {
        new_token(self.key.as_deref())
    }
}

pub fn unsubscribe_config(conn: &Connection) -> UnsubConfig {
    let key = insrt_token(conn);
    let custom = crate::secrets::read_setting(conn, UNSUB_BASE_KEY);
    let insrt_base = crate::secrets::read_setting(conn, "insrt_api_base");
    let eff = effective_unsubscribe_base(custom.as_deref(), key.as_deref(), insrt_base.as_deref());
    UnsubConfig { via_insrt: eff.as_ref().is_some_and(|e| e.1), base: eff.map(|e| e.0), key }
}

// ---------------- Tokens ----------------

/// HMAC-SHA256 (RFC 2104), written out: sha2 is already in the tree, a
/// dedicated hmac crate is not.
fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let pad = |b: u8| k.iter().map(|x| x ^ b).collect::<Vec<u8>>();
    let inner = Sha256::new().chain_update(pad(0x36)).chain_update(msg).finalize();
    Sha256::new().chain_update(pad(0x5c)).chain_update(inner).finalize().into()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Signature half of a token: HMAC-SHA256(key, "unsub:v1:" + nonce), first 16
/// bytes in hex. Same computation on insrt (src/features/mailing/token.ts).
pub fn unsub_signature(nonce: &str, key: &str) -> String {
    hex(&hmac_sha256(key.trim().as_bytes(), format!("unsub:v1:{}", nonce).as_bytes())[..16])
}

fn random_nonce() -> String {
    let mut b = [0u8; 12];
    if getrandom::fill(&mut b).is_err() {
        // No OS randomness (never seen on macOS): time + counter, still unique.
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let t = Utc::now().timestamp_nanos_opt().unwrap_or_default() as u64;
        let c = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        b[..8].copy_from_slice(&t.to_be_bytes());
        b[8..].copy_from_slice(&(c as u32).to_be_bytes());
    }
    hex(&b)
}

/// A fresh email token: 24 hex chars of random nonce, followed by 32 hex
/// chars of signature when a key is set (56 in all, the format insrt checks).
pub fn new_token(key: Option<&str>) -> String {
    let nonce = random_nonce();
    match key.filter(|k| !k.trim().is_empty()) {
        Some(k) => {
            let sig = unsub_signature(&nonce, k);
            nonce + &sig
        }
        None => nonce,
    }
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

/// Suppress the addresses behind one-click unsubscribe tokens (the token of
/// an email is its `track_token`). Returns how many addresses were newly
/// suppressed; unknown tokens and addresses already suppressed are skipped.
pub fn apply_unsubscribes(conn: &Connection, tokens: &[String]) -> usize {
    let mut n = 0;
    for t in tokens {
        let addr: Option<String> = conn
            .query_row("SELECT to_addr FROM emails WHERE track_token = ?1 LIMIT 1", [t.trim()], |r| r.get(0))
            .ok();
        if let Some(a) = addr {
            if crate::commands::outbox::suppress(conn, &a, "unsubscribe", Some(ONE_CLICK_NOTE)) {
                crate::commands::status::on_refused(conn, None, &a);
                n += 1;
            }
        }
    }
    n
}

/// Read the one-click unsubscribes recorded on insrt since the stored cursor
/// (all of them the first time) and suppress their addresses, so every
/// sequence stops. Runs with the EPK activity poll (engine, every 30 min, and
/// the contact card's "Actualiser"). Returns how many addresses were newly
/// suppressed. A failure keeps the cursor: the next poll asks again.
pub fn poll_insrt_unsubscribes(conn: &Connection, client: &dyn crate::commands::residency::insrt::Insrt) -> Result<usize, String> {
    let mut since = get(conn, INSRT_UNSUB_CURSOR_KEY).unwrap_or_else(|| "1970-01-01T00:00:00.000Z".to_string());
    let mut n = 0;
    for _ in 0..20 {
        let page = client.unsubscribes(&since)?;
        let tokens: Vec<String> = page.unsubscribes.iter().map(|u| u.token.clone()).collect();
        n += apply_unsubscribes(conn, &tokens);
        let next = page.next.filter(|s| !s.trim().is_empty());
        let advanced = next.as_ref().is_some_and(|x| *x != since);
        if let Some(x) = next {
            since = x;
        }
        put(conn, INSRT_UNSUB_CURSOR_KEY, &since);
        if tokens.is_empty() || !advanced {
            break;
        }
    }
    Ok(n)
}

fn poll_unsubscribes(conn: &Connection) -> Result<usize, String> {
    let Some(base) = custom_unsubscribe_base(conn) else { return Ok(0) };
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
    /// The one-click endpoint in use (the user's, or insrt's by default).
    pub unsubscribe_endpoint: Option<String>,
    /// The endpoint is insrt.studio (no custom URL set, insrt API configured).
    pub unsubscribe_via_insrt: bool,
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
    let unsub = unsubscribe_config(&conn);
    Ok(DeliverabilityStatus {
        sender: health::current_sender(&conn),
        budget: budget::status(&conn, now),
        health: health::stats(&conn, now),
        thresholds: health::load_thresholds(&conn),
        pause,
        dns: stored_report(&conn),
        one_click_unsubscribe: unsub.base.is_some(),
        unsubscribe_endpoint: unsub.base,
        unsubscribe_via_insrt: unsub.via_insrt,
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
    fn signed_tokens() {
        // RFC 4231 test cases 2 and 6 (key longer than a block).
        assert_eq!(hex(&hmac_sha256(b"Jefe", b"what do ya want for nothing?")), "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
        assert_eq!(
            hex(&hmac_sha256(&[0xaa; 131], b"Test Using Larger Than Block-Size Key - Hash Key First")),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
        // Vector shared with insrt (tests/unsubscribe.spec.ts).
        assert_eq!(unsub_signature("0123456789abcdef01234567", "test-key"), "2d669e2ca689da42fee1b4f4663e15c4");
        assert_eq!(unsub_signature("0123456789abcdef01234567", " test-key\n"), "2d669e2ca689da42fee1b4f4663e15c4");
        let t = new_token(Some("k"));
        assert_eq!(t.len(), 56);
        assert!(t.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_eq!(&t[24..], unsub_signature(&t[..24], "k"));
        assert_ne!(new_token(Some("k")), t);
        assert_eq!(new_token(None).len(), 24);
        assert_eq!(new_token(Some("  ")).len(), 24);
    }

    #[test]
    fn insrt_is_the_default_endpoint() {
        let insrt = Some(("https://insrt.studio".to_string(), true));
        assert_eq!(effective_unsubscribe_base(None, Some("tok"), None), insrt);
        assert_eq!(effective_unsubscribe_base(Some("  "), Some("tok"), Some("")), insrt);
        assert_eq!(effective_unsubscribe_base(Some("http://nope"), Some("tok"), None), insrt);
        assert_eq!(
            effective_unsubscribe_base(None, Some("tok"), Some("https://preview.insrt.studio/")),
            Some(("https://preview.insrt.studio".to_string(), true))
        );
        // A local dev base is not HTTPS: mailto only.
        assert_eq!(effective_unsubscribe_base(None, Some("tok"), Some("http://localhost:3000")), None);
        assert_eq!(effective_unsubscribe_base(None, None, None), None);
        assert_eq!(effective_unsubscribe_base(None, Some(" "), None), None);
        // The user's own endpoint wins.
        assert_eq!(
            effective_unsubscribe_base(Some("https://t.insrt.fr/"), Some("tok"), None),
            Some(("https://t.insrt.fr".to_string(), false))
        );
    }

    struct FakeInsrt {
        pages: std::sync::Mutex<Vec<Result<crate::commands::residency::insrt::UnsubscribePage, String>>>,
        calls: std::sync::Mutex<Vec<String>>,
    }

    impl crate::commands::residency::insrt::Insrt for FakeInsrt {
        fn press_kits(&self, _: &str) -> Result<Vec<crate::commands::residency::insrt::PressKit>, String> {
            Err("unused".into())
        }
        fn press_link(
            &self,
            _: &str,
            _: i64,
            _: &crate::commands::residency::insrt::Recipient,
            _: &str,
        ) -> Result<crate::commands::residency::insrt::PressLink, String> {
            Err("unused".into())
        }
        fn activity(&self, _: &str, _: &str) -> Result<crate::commands::residency::insrt::ActivityPage, String> {
            Err("unused".into())
        }
        fn unsubscribes(&self, since: &str) -> Result<crate::commands::residency::insrt::UnsubscribePage, String> {
            self.calls.lock().unwrap().push(since.to_string());
            let mut p = self.pages.lock().unwrap();
            if p.is_empty() {
                Ok(crate::commands::residency::insrt::UnsubscribePage { unsubscribes: vec![], next: Some(since.to_string()) })
            } else {
                p.remove(0)
            }
        }
    }

    fn page(tokens: &[&str], next: &str) -> Result<crate::commands::residency::insrt::UnsubscribePage, String> {
        Ok(crate::commands::residency::insrt::UnsubscribePage {
            unsubscribes: tokens
                .iter()
                .map(|t| crate::commands::residency::insrt::Unsubscribe { token: t.to_string(), source: Some("one_click".into()), at: Some(next.into()) })
                .collect(),
            next: Some(next.into()),
        })
    }

    #[test]
    fn insrt_unsubscribes_suppress_and_advance() {
        let c = crate::commands::residency::testdb::conn();
        c.execute_batch(
            "INSERT INTO emails (to_addr, subject, body, status, track_token) VALUES ('a@club.fr','s','b','sent','ta');
             INSERT INTO emails (to_addr, subject, body, status, track_token) VALUES ('b@club.fr','s','b','sent','tb');
             INSERT INTO emails (to_addr, subject, body, status, track_token) VALUES ('b@club.fr','s2','b','sent','tb2');",
        )
        .unwrap();
        let fake = FakeInsrt {
            pages: std::sync::Mutex::new(vec![
                page(&["ta", "unknown"], "2026-10-31T10:00:00.000Z"),
                page(&["tb", "tb2"], "2026-10-31T11:00:00.000Z"),
            ]),
            calls: std::sync::Mutex::new(vec![]),
        };
        assert_eq!(poll_insrt_unsubscribes(&c, &fake).unwrap(), 2);
        assert_eq!(
            *fake.calls.lock().unwrap(),
            vec!["1970-01-01T00:00:00.000Z", "2026-10-31T10:00:00.000Z", "2026-10-31T11:00:00.000Z"]
        );
        assert_eq!(crate::commands::outbox::suppression_reason(&c, "a@club.fr").as_deref(), Some("unsubscribe"));
        assert_eq!(crate::commands::outbox::suppression_reason(&c, "b@club.fr").as_deref(), Some("unsubscribe"));
        assert_eq!(get(&c, INSRT_UNSUB_CURSOR_KEY).as_deref(), Some("2026-10-31T11:00:00.000Z"));
        // A failure keeps the cursor; the same tokens again change nothing.
        let failing = FakeInsrt { pages: std::sync::Mutex::new(vec![Err("insrt: HTTP 500".into())]), calls: std::sync::Mutex::new(vec![]) };
        assert!(poll_insrt_unsubscribes(&c, &failing).is_err());
        assert_eq!(get(&c, INSRT_UNSUB_CURSOR_KEY).as_deref(), Some("2026-10-31T11:00:00.000Z"));
        let replay = FakeInsrt { pages: std::sync::Mutex::new(vec![page(&["ta"], "2026-10-31T12:00:00.000Z")]), calls: std::sync::Mutex::new(vec![]) };
        assert_eq!(poll_insrt_unsubscribes(&c, &replay).unwrap(), 0);
        assert_eq!(replay.calls.lock().unwrap()[0], "2026-10-31T11:00:00.000Z");
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
