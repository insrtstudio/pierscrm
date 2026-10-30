//! "Notifications iPhone": push notifications to Thibault's phone through
//! Pushover (https://pushover.net/api), sent by the desktop app while it runs.
//!
//! Nothing is threaded through the other modules: `tick` runs from the
//! residency engine (every 5 minutes, and after each reply sync) and scans
//! what the other modules already wrote:
//! - a reply (`email_replies`), read with the existing classification:
//!   residency `reply_insights`, the reply desk's intent, the rules
//!   (out-of-office, unsubscribe) and the outbox refusal detection;
//! - insrt EPK activity (`press_activity`): first view of a personal link,
//!   listens of 60 s or more (grouped per contact over 30 minutes), feedback;
//! - first opens of an email (`emails.opened_at`, tracking pixel), folded into
//!   the daily summary by default;
//! - problems: hard bounces (`suppressions`), automated sending paused
//!   (`residency_log` level 'pause': residency safety gates and sender
//!   health), IMAP sync failing for more than 2 hours;
//! - a daily summary at 18:00 Paris on days something was sent.
//!
//! Every notice is written once to `push_log`, UNIQUE on (kind, source_id), so
//! nothing is notified twice, across restarts too. The table is also the queue:
//! a notice waits there ('held') during quiet hours (default 22:00 to 08:00
//! Paris) unless it is urgent (priority 1), and a network failure leaves it
//! 'queued' for the next tick. A 4xx answer means a bad configuration: the
//! message is marked failed and sending stops until the keys are saved again
//! or a test succeeds.
//!
//! The network sits behind the `Pusher` trait; tests use a fake.

use chrono::{DateTime, Duration, NaiveTime, Timelike, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::State;

use crate::commands::residency::engine::{fmt, parse_utc};
use crate::commands::residency::replies::{self, Pending};
use crate::commands::residency::zone::Tz;
use crate::commands::status;
use crate::db::DbPool;
use crate::AppState;

pub const API_BASE: &str = "https://api.pushover.net/1";
pub const USER_KEY: &str = "pushover_user";
pub const TOKEN_KEY: &str = "pushover_token";
const CONFIG_KEY: &str = "push_config";
const LAST_ERROR_KEY: &str = "push_last_error";
const LAST_ERROR_AT_KEY: &str = "push_last_error_at";
const LAST_OK_KEY: &str = "push_last_ok";
/// Set on a 4xx answer: nothing is sent until the keys are saved or tested.
const BLOCKED_KEY: &str = "push_blocked";
const IMAP_FAIL_SINCE_KEY: &str = "push_imap_fail_since";

/// Only events this recent are notified (no flood on the first run).
pub const LOOKBACK_HOURS: i64 = 24;
/// A queued notice older than this is dropped instead of sent late.
pub const EXPIRE_HOURS: i64 = 36;
/// A reply waits this long for its classification (same pass, normally).
pub const CLASSIFY_WAIT_MIN: i64 = 15;
pub const LISTEN_MIN_SECS: f64 = 60.0;
pub const LISTEN_GROUP_MIN: i64 = 30;
pub const IMAP_DOWN_HOURS: i64 = 2;
pub const SUMMARY_HOUR: u32 = 18;
pub const EXCERPT_CHARS: usize = 200;
/// Notices sent per pass, at most.
const BATCH: usize = 20;

pub fn migrate(conn: &Connection) {
    for stmt in [
        "CREATE TABLE IF NOT EXISTS push_log (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            kind       TEXT NOT NULL,
            source_id  TEXT NOT NULL,
            title      TEXT NOT NULL DEFAULT '',
            message    TEXT NOT NULL DEFAULT '',
            priority   INTEGER NOT NULL DEFAULT 0,
            url        TEXT,
            url_title  TEXT,
            event_at   TEXT,
            status     TEXT NOT NULL DEFAULT 'queued',
            error      TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            sent_at    TEXT,
            UNIQUE (kind, source_id)
        )",
        "CREATE INDEX IF NOT EXISTS idx_push_log_status ON push_log(status, id)",
    ] {
        let _ = conn.execute(stmt, []);
    }
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

fn yes() -> bool {
    true
}
fn quiet_start_default() -> String {
    "22:00".into()
}
fn quiet_end_default() -> String {
    "08:00".into()
}

/// Per-event switches and quiet hours (the two keys live in the Keychain).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PushConfig {
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default = "yes")]
    pub replies: bool,
    /// Out-of-office answers (silent when on). Off by default.
    #[serde(default)]
    pub out_of_office: bool,
    #[serde(default = "yes")]
    pub epk: bool,
    #[serde(default = "yes")]
    pub opens: bool,
    /// Opens counted in the daily summary instead of one push each.
    #[serde(default = "yes")]
    pub opens_in_summary: bool,
    #[serde(default = "yes")]
    pub problems: bool,
    #[serde(default = "yes")]
    pub summary: bool,
    #[serde(default = "yes")]
    pub quiet_enabled: bool,
    #[serde(default = "quiet_start_default")]
    pub quiet_start: String,
    #[serde(default = "quiet_end_default")]
    pub quiet_end: String,
}

impl Default for PushConfig {
    fn default() -> Self {
        serde_json::from_str("{}").expect("defaults")
    }
}

fn get(conn: &Connection, key: &str) -> Option<String> {
    crate::commands::residency::engine::get(conn, key)
}

fn put(conn: &Connection, key: &str, value: &str) {
    crate::commands::residency::engine::put(conn, key, value)
}

fn del(conn: &Connection, key: &str) {
    let _ = conn.execute("DELETE FROM settings WHERE key = ?1", params![key]);
}

pub fn load_config(conn: &Connection) -> PushConfig {
    get(conn, CONFIG_KEY).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn save_config(conn: &Connection, cfg: &PushConfig) {
    put(conn, CONFIG_KEY, &serde_json::to_string(cfg).unwrap_or_default());
}

fn hm(s: &str) -> Option<u32> {
    NaiveTime::parse_from_str(s.trim(), "%H:%M").ok().map(|t| t.hour() * 60 + t.minute())
}

/// Inside quiet hours (Paris time)? The window may wrap midnight.
pub fn in_quiet_hours(cfg: &PushConfig, now: DateTime<Utc>) -> bool {
    if !cfg.quiet_enabled {
        return false;
    }
    let (Some(s), Some(e)) = (hm(&cfg.quiet_start), hm(&cfg.quiet_end)) else { return false };
    let l = Tz::Paris.to_local(now);
    let m = l.hour() * 60 + l.minute();
    if s == e {
        false
    } else if s < e {
        m >= s && m < e
    } else {
        m >= s || m < e
    }
}

// ---------------------------------------------------------------------------
// Network
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Message {
    pub title: String,
    pub message: String,
    pub priority: i64,
    pub url: Option<String>,
    pub url_title: Option<String>,
    pub sound: Option<String>,
    /// Unix time of the event (a notice held overnight shows its real time).
    pub timestamp: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PushError {
    /// 4xx: wrong keys or a rejected message. Not retried.
    Config(String),
    /// Network, 5xx: retried on the next tick.
    Transient(String),
}

impl PushError {
    pub fn text(&self) -> &str {
        match self {
            PushError::Config(s) | PushError::Transient(s) => s,
        }
    }
}

pub trait Pusher {
    fn send(&self, m: &Message) -> Result<(), PushError>;
    /// Check the user key (users/validate.json).
    fn validate(&self) -> Result<(), PushError>;
}

pub struct HttpPusher {
    base: String,
    user: String,
    token: String,
    http: reqwest::blocking::Client,
}

fn form_encode(pairs: &[(&str, String)]) -> String {
    let enc = |s: &str| {
        let mut o = String::with_capacity(s.len());
        for b in s.bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => o.push(b as char),
                b' ' => o.push('+'),
                _ => o.push_str(&format!("%{:02X}", b)),
            }
        }
        o
    };
    pairs.iter().map(|(k, v)| format!("{}={}", enc(k), enc(v))).collect::<Vec<_>>().join("&")
}

/// Pushover's answer: 200 + {"status":1} is a success, a 4xx is a bad
/// configuration (its `errors` explain why), anything else is transient.
pub fn read_answer(code: u16, body: &str) -> Result<(), PushError> {
    let v: serde_json::Value = serde_json::from_str(body).unwrap_or(serde_json::Value::Null);
    let errors = v
        .get("errors")
        .and_then(|e| e.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", "))
        .filter(|s| !s.is_empty());
    if code == 200 && v.get("status").and_then(|s| s.as_i64()) == Some(1) {
        return Ok(());
    }
    let why = errors.unwrap_or_else(|| format!("réponse HTTP {}", code));
    if (400..500).contains(&code) {
        Err(PushError::Config(format!("Pushover refuse : {}", why)))
    } else {
        Err(PushError::Transient(format!("Pushover indisponible : {}", why)))
    }
}

impl HttpPusher {
    pub fn new(user: &str, token: &str) -> Self {
        let http = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .unwrap_or_else(|_| reqwest::blocking::Client::new());
        HttpPusher { base: API_BASE.into(), user: user.trim().into(), token: token.trim().into(), http }
    }

    fn post(&self, path: &str, pairs: &[(&str, String)]) -> Result<(), PushError> {
        let resp = self
            .http
            .post(format!("{}/{}", self.base, path))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(form_encode(pairs))
            .send()
            .map_err(|e| PushError::Transient(format!("Pushover injoignable : {}", e)))?;
        let code = resp.status().as_u16();
        let body = resp.text().unwrap_or_default();
        read_answer(code, &body)
    }
}

impl Pusher for HttpPusher {
    fn send(&self, m: &Message) -> Result<(), PushError> {
        let mut p: Vec<(&str, String)> = vec![
            ("token", self.token.clone()),
            ("user", self.user.clone()),
            ("title", m.title.clone()),
            ("message", m.message.clone()),
            ("priority", m.priority.to_string()),
        ];
        if let Some(u) = &m.url {
            p.push(("url", u.clone()));
        }
        if let Some(u) = &m.url_title {
            p.push(("url_title", u.clone()));
        }
        if let Some(s) = &m.sound {
            p.push(("sound", s.clone()));
        }
        if let Some(t) = m.timestamp {
            p.push(("timestamp", t.to_string()));
        }
        self.post("messages.json", &p)
    }

    fn validate(&self) -> Result<(), PushError> {
        self.post("users/validate.json", &[("token", self.token.clone()), ("user", self.user.clone())])
    }
}

fn credentials(conn: &Connection) -> Option<(String, String)> {
    let user = crate::secrets::read_setting(conn, USER_KEY).filter(|s| !s.trim().is_empty())?;
    let token = crate::secrets::read_setting(conn, TOKEN_KEY).filter(|s| !s.trim().is_empty())?;
    Some((user, token))
}

// ---------------------------------------------------------------------------
// Notices
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Notice {
    pub kind: String,
    pub source_id: String,
    pub title: String,
    pub message: String,
    pub priority: i64,
    pub url: Option<String>,
    pub event_at: Option<DateTime<Utc>>,
}

impl Notice {
    fn new(kind: &str, source: impl ToString, title: String, message: String, priority: i64) -> Self {
        Notice {
            kind: kind.into(),
            source_id: source.to_string(),
            title,
            message,
            priority,
            url: None,
            event_at: None,
        }
    }
    fn at(mut self, t: Option<DateTime<Utc>>) -> Self {
        self.event_at = t;
        self
    }
}

/// Write a notice once. `status` is 'queued' (to send) or a terminal state
/// ('off', 'grouped', 'summary') recorded only for the dedupe. False when the
/// (kind, source) pair was already there.
fn record(conn: &Connection, n: &Notice, status: &str, now: DateTime<Utc>) -> bool {
    conn.execute(
        "INSERT OR IGNORE INTO push_log (kind, source_id, title, message, priority, url, event_at, status, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![n.kind, n.source_id, n.title, n.message, n.priority, n.url, n.event_at.map(fmt), status, fmt(now)],
    )
    .unwrap_or(0)
        > 0
}

fn logged(conn: &Connection, kind: &str, source: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM push_log WHERE kind = ?1 AND source_id = ?2",
        params![kind, source],
        |_| Ok(()),
    )
    .is_ok()
}

fn excerpt(text: &str, n: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= n {
        flat
    } else {
        format!("{}…", flat.chars().take(n).collect::<String>().trim_end())
    }
}

// ---------------- Replies ----------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReplyClass {
    Interested,
    Question,
    Later,
    Refusal,
    WrongPerson,
    Unsubscribe,
    OutOfOffice,
    /// No classification came (no API key, failed call): a human answered.
    Unknown,
}

impl ReplyClass {
    pub fn from_intent(intent: &str) -> Option<Self> {
        Some(match intent {
            "interested" => Self::Interested,
            "question" => Self::Question,
            "later" => Self::Later,
            "not_interested" => Self::Refusal,
            "wrong_person" => Self::WrongPerson,
            "unsubscribe" => Self::Unsubscribe,
            "out_of_office" => Self::OutOfOffice,
            _ => return None,
        })
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Interested => "intéressé",
            Self::Question => "question",
            Self::Later => "plus tard",
            Self::Refusal => "refus",
            Self::WrongPerson => "mauvais contact",
            Self::Unsubscribe => "désinscription",
            Self::OutOfOffice => "absent",
            Self::Unknown => "à lire",
        }
    }

    /// Pushover priority: 1 high (an answer to act on), 0 normal, -1 silent.
    pub fn priority(&self) -> i64 {
        match self {
            Self::Interested | Self::Question | Self::Unknown => 1,
            Self::Later | Self::Refusal | Self::WrongPerson | Self::Unsubscribe => 0,
            Self::OutOfOffice => -1,
        }
    }
}

/// One reply as the scan sees it.
#[derive(Debug, Clone)]
pub struct ReplyRow {
    pub id: i64,
    pub kind: String,
    pub from: String,
    pub subject: String,
    pub text: String,
    pub insight_intent: Option<String>,
    pub desk_intent: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// What a reply is, from the classifications already made. None = wait (the
/// classification normally lands in the same pass; past CLASSIFY_WAIT_MIN the
/// reply is notified as Unknown).
pub fn classify_reply(r: &ReplyRow, now: DateTime<Utc>, may_wait: bool) -> Option<ReplyClass> {
    if let Some(c) = r.insight_intent.as_deref().and_then(ReplyClass::from_intent) {
        return Some(c);
    }
    if let Some(c) = r.desk_intent.as_deref().and_then(ReplyClass::from_intent) {
        return Some(c);
    }
    let p = Pending {
        reply_id: r.id,
        contact_id: 0,
        campaign_id: 0,
        kind: r.kind.clone(),
        from: r.from.clone(),
        subject: r.subject.clone(),
        text: r.text.clone(),
        email_id: None,
    };
    if let Some(i) = replies::rule_classify(&p, now.date_naive()) {
        if let Some(c) = ReplyClass::from_intent(&i.intent) {
            return Some(c);
        }
    }
    if status::looks_like_refusal(&r.text) {
        return Some(ReplyClass::Refusal);
    }
    if may_wait && now - r.created_at < Duration::minutes(CLASSIFY_WAIT_MIN) {
        return None;
    }
    Some(ReplyClass::Unknown)
}

const NAME_SQL: &str = "COALESCE(NULLIF(trim(k.venue),''), NULLIF(trim(k.name),''))";

fn reply_rows(conn: &Connection, since: &str, until: Option<&str>) -> Vec<(ReplyRow, String)> {
    let sql = format!(
        "SELECT r.id, r.kind, r.from_addr, COALESCE(r.subject,''), COALESCE(r.body_excerpt,''),
                (SELECT i.intent FROM reply_insights i WHERE i.email_reply_id = r.id),
                (SELECT d.intent FROM reply_desk d WHERE d.email_reply_id = r.id AND d.intent IS NOT NULL LIMIT 1),
                r.created_at, COALESCE({name}, r.from_addr)
         FROM email_replies r
         LEFT JOIN contacts k ON k.id = COALESCE(r.contact_id,
              (SELECT c2.id FROM contacts c2 WHERE c2.email IS NOT NULL
                 AND lower(trim(c2.email)) = lower(trim(r.from_addr)) LIMIT 1))
         WHERE r.kind IN ('reply','auto_reply') AND r.created_at >= ?1 AND r.created_at < ?2
         ORDER BY r.id",
        name = NAME_SQL
    );
    let Ok(mut st) = conn.prepare(&sql) else { return Vec::new() };
    st.query_map(params![since, until.unwrap_or("9999")], |r| {
        let created: String = r.get(7)?;
        Ok((
            ReplyRow {
                id: r.get(0)?,
                kind: r.get(1)?,
                from: r.get(2)?,
                subject: r.get(3)?,
                text: r.get(4)?,
                insight_intent: r.get(5)?,
                desk_intent: r.get(6)?,
                created_at: parse_utc(&created).unwrap_or_else(Utc::now),
            },
            r.get::<_, String>(8)?,
        ))
    })
    .map(|it| it.flatten().collect())
    .unwrap_or_default()
}

fn scan_replies(conn: &Connection, cfg: &PushConfig, now: DateTime<Utc>) {
    let since = fmt(now - Duration::hours(LOOKBACK_HOURS));
    for (r, name) in reply_rows(conn, &since, None) {
        let source = r.id.to_string();
        if logged(conn, "reply", &source) {
            continue;
        }
        let Some(class) = classify_reply(&r, now, true) else { continue };
        let title = match class {
            ReplyClass::Unknown => format!("Réponse de {}", name),
            c => format!("Réponse de {} : {}", name, c.label()),
        };
        let body = if r.text.trim().is_empty() { r.subject.clone() } else { r.text.clone() };
        let n = Notice::new("reply", source, title, excerpt(&body, EXCERPT_CHARS), class.priority()).at(Some(r.created_at));
        let on = cfg.replies && (class != ReplyClass::OutOfOffice || cfg.out_of_office);
        record(conn, &n, if on { "queued" } else { "off" }, now);
    }
}

// ---------------- insrt EPK activity ----------------

#[derive(Debug, Clone)]
pub struct Listen {
    pub id: i64,
    pub contact_id: Option<i64>,
    pub name: String,
    pub track: String,
    pub seconds: f64,
    pub at: DateTime<Utc>,
}

pub fn minutes(seconds: f64) -> i64 {
    ((seconds / 60.0).round() as i64).max(1)
}

/// Listens of one contact over LISTEN_GROUP_MIN, as one notice.
pub fn listen_notice(group: &[Listen]) -> Notice {
    let first = &group[0];
    let line = |l: &Listen| format!("« {} » ({} min)", if l.track.is_empty() { "un titre" } else { &l.track }, minutes(l.seconds));
    let message = if group.len() == 1 {
        format!("A écouté {}.", line(first))
    } else {
        format!(
            "{} écoutes en {} min : {}.",
            group.len(),
            LISTEN_GROUP_MIN,
            group.iter().map(line).collect::<Vec<_>>().join(", ")
        )
    };
    Notice::new("epk_listens", first.id, format!("insrt : {} écoute", first.name), message, 0).at(Some(first.at))
}

/// Group listens (sorted by time) per contact: a group opens on the first
/// listen not yet notified and closes LISTEN_GROUP_MIN later. A group still
/// open at `now` waits. Returns the closed groups.
pub fn group_listens(listens: &[Listen], now: DateTime<Utc>) -> Vec<Vec<Listen>> {
    let mut per: BTreeMap<String, Vec<Listen>> = BTreeMap::new();
    for l in listens {
        let key = l.contact_id.map_or_else(|| format!("e{}", l.id), |c| c.to_string());
        per.entry(key).or_default().push(l.clone());
    }
    let mut out = Vec::new();
    for (_, mut ls) in per {
        ls.sort_by_key(|l| (l.at, l.id));
        let mut i = 0;
        while i < ls.len() {
            let end = ls[i].at + Duration::minutes(LISTEN_GROUP_MIN);
            if now < end {
                break;
            }
            let group: Vec<Listen> = ls[i..].iter().take_while(|l| l.at <= end).cloned().collect();
            i += group.len();
            out.push(group);
        }
    }
    out
}

fn scan_epk(conn: &Connection, cfg: &PushConfig, now: DateTime<Utc>) {
    let since = fmt(now - Duration::hours(LOOKBACK_HOURS));
    let status = if cfg.epk { "queued" } else { "off" };
    let name_sql = format!("COALESCE({}, 'Un contact')", NAME_SQL);

    // First view of a personal link.
    let views: Vec<(i64, String, String, Option<String>)> = conn
        .prepare(&format!(
            "SELECT p.id, COALESCE(p.link_id, 'c' || p.contact_id, 'e' || p.id), p.at, {name}
             FROM press_activity p LEFT JOIN contacts k ON k.id = p.contact_id
             WHERE p.type = 'view' AND p.created_at >= ?1
               AND NOT EXISTS (SELECT 1 FROM press_activity q WHERE q.type = 'view' AND q.id < p.id
                   AND COALESCE(q.link_id, 'c' || q.contact_id, 'e' || q.id) = COALESCE(p.link_id, 'c' || p.contact_id, 'e' || p.id))
             ORDER BY p.id",
            name = name_sql
        ))
        .and_then(|mut st| {
            st.query_map(params![since], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
                .map(|it| it.flatten().collect())
        })
        .unwrap_or_default();
    for (_, link, at, name) in views {
        let name = name.unwrap_or_else(|| "Un contact".into());
        let n = Notice::new(
            "epk_view",
            link,
            format!("insrt : {} a ouvert le dossier", name),
            "Première consultation de son lien personnel.".into(),
            0,
        )
        .at(parse_utc(&at));
        record(conn, &n, status, now);
    }

    // Listens of 60 s or more, grouped per contact.
    let listens: Vec<Listen> = conn
        .prepare(&format!(
            "SELECT p.id, p.contact_id, {name}, COALESCE(p.track_title, ''), COALESCE(p.seconds, 0), p.at, p.created_at
             FROM press_activity p LEFT JOIN contacts k ON k.id = p.contact_id
             WHERE p.type = 'listen' AND COALESCE(p.seconds, 0) >= ?2 AND p.created_at >= ?1
               AND NOT EXISTS (SELECT 1 FROM push_log l WHERE l.kind = 'epk_listen' AND l.source_id = CAST(p.id AS TEXT))
             ORDER BY p.id",
            name = name_sql
        ))
        .and_then(|mut st| {
            st.query_map(params![since, LISTEN_MIN_SECS], |r| {
                let at: String = r.get(5)?;
                let created: String = r.get(6)?;
                Ok(Listen {
                    id: r.get(0)?,
                    contact_id: r.get(1)?,
                    name: r.get(2)?,
                    track: r.get(3)?,
                    seconds: r.get(4)?,
                    at: parse_utc(&at).or_else(|| parse_utc(&created)).unwrap_or(now),
                })
            })
            .map(|it| it.flatten().collect())
        })
        .unwrap_or_default();
    for group in group_listens(&listens, now) {
        record(conn, &listen_notice(&group), status, now);
        for l in &group {
            record(conn, &Notice::new("epk_listen", l.id, String::new(), String::new(), 0), "grouped", now);
        }
    }

    // Feedback / comment.
    let feedback: Vec<(i64, String, String, Option<f64>, Option<String>, Option<String>)> = conn
        .prepare(&format!(
            "SELECT p.id, {name}, p.at, p.rating, p.comment, p.track_title
             FROM press_activity p LEFT JOIN contacts k ON k.id = p.contact_id
             WHERE (p.type = 'feedback' OR (p.comment IS NOT NULL AND trim(p.comment) <> ''))
               AND p.created_at >= ?1
             ORDER BY p.id",
            name = name_sql
        ))
        .and_then(|mut st| {
            st.query_map(params![since], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))
                .map(|it| it.flatten().collect())
        })
        .unwrap_or_default();
    for (id, name, at, rating, comment, track) in feedback {
        let mut parts = Vec::new();
        if let Some(t) = track.filter(|t| !t.trim().is_empty()) {
            parts.push(format!("« {} »", t));
        }
        if let Some(r) = rating {
            parts.push(format!("note {}", (r * 10.0).round() / 10.0));
        }
        let mut message = parts.join(", ");
        if let Some(c) = comment.filter(|c| !c.trim().is_empty()) {
            if !message.is_empty() {
                message.push_str(" : ");
            }
            message.push_str(&excerpt(&c, EXCERPT_CHARS));
        }
        if message.is_empty() {
            message = "Nouveau retour sur le dossier.".into();
        }
        let n = Notice::new("epk_feedback", id, format!("insrt : retour de {}", name), message, 0).at(parse_utc(&at));
        record(conn, &n, status, now);
    }
}

// ---------------- Email opens ----------------

fn scan_opens(conn: &Connection, cfg: &PushConfig, now: DateTime<Utc>) {
    let since = now - Duration::hours(LOOKBACK_HOURS);
    let rows: Vec<(i64, String, String, String)> = conn
        .prepare(&format!(
            "SELECT e.id, e.opened_at, e.subject, COALESCE({name}, e.to_addr)
             FROM emails e LEFT JOIN contacts k ON k.id = e.contact_id
             WHERE e.opened_at IS NOT NULL AND e.status = 'sent' AND e.sent_at >= ?1
               AND NOT EXISTS (SELECT 1 FROM push_log l WHERE l.kind = 'open' AND l.source_id = CAST(e.id AS TEXT))",
            name = NAME_SQL
        ))
        .and_then(|mut st| {
            st.query_map(params![fmt(now - Duration::days(30))], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
                .map(|it| it.flatten().collect())
        })
        .unwrap_or_default();
    for (id, opened_at, subject, name) in rows {
        let Some(at) = parse_utc(&opened_at) else { continue };
        if at < since {
            continue;
        }
        let n = Notice::new("open", id, format!("Ouvert : {}", name), format!("« {} »", excerpt(&subject, 120)), -1).at(Some(at));
        let status = if !cfg.opens {
            "off"
        } else if cfg.opens_in_summary {
            "summary"
        } else {
            "queued"
        };
        record(conn, &n, status, now);
    }
}

// ---------------- Problems ----------------

fn scan_problems(conn: &Connection, cfg: &PushConfig, now: DateTime<Utc>) {
    let since = fmt(now - Duration::hours(LOOKBACK_HOURS));
    let status = if cfg.problems { "queued" } else { "off" };

    let bounces: Vec<(i64, String, String)> = conn
        .prepare(&format!(
            "SELECT s.id, s.address, s.created_at,
                    (SELECT {name} FROM contacts k WHERE lower(trim(k.email)) = s.address LIMIT 1)
             FROM suppressions s WHERE s.reason = 'bounce' AND s.created_at >= ?1 ORDER BY s.id",
            name = NAME_SQL
        ))
        .and_then(|mut st| {
            st.query_map(params![since], |r| {
                let addr: String = r.get(1)?;
                let name: Option<String> = r.get(3)?;
                Ok((r.get(0)?, match name {
                    Some(n) => format!("{} ({})", addr, n),
                    None => addr,
                }, r.get(2)?))
            })
            .map(|it| it.flatten().collect())
        })
        .unwrap_or_default();
    for (id, who, at) in bounces {
        let n = Notice::new(
            "bounce",
            id,
            "Adresse rejetée".into(),
            format!("{} : rebond définitif, retirée des envois.", who),
            1,
        )
        .at(parse_utc(&at));
        record(conn, &n, status, now);
    }

    let pauses: Vec<(i64, String, String, String, Option<String>)> = conn
        .prepare(
            "SELECT l.id, l.code, l.message, l.at, c.name
             FROM residency_log l LEFT JOIN campaigns c ON c.id = l.campaign_id
             WHERE l.level = 'pause' AND l.at >= ?1 ORDER BY l.id",
        )
        .and_then(|mut st| {
            st.query_map(params![since], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))
                .map(|it| it.flatten().collect())
        })
        .unwrap_or_default();
    for (id, code, message, at, campaign) in pauses {
        let title = match (code.as_str(), campaign) {
            ("deliverability_pause", _) | (_, None) => "Envois automatiques en pause".to_string(),
            (_, Some(c)) => format!("Pilote en pause : {}", c),
        };
        let n = Notice::new("pause", id, title, excerpt(&message, EXCERPT_CHARS), 1).at(parse_utc(&at));
        record(conn, &n, status, now);
    }

    // IMAP: failing since the first error seen, notified once per outage.
    match get(conn, "imap_last_error") {
        None => del(conn, IMAP_FAIL_SINCE_KEY),
        Some(err) => {
            let since_fail = match get(conn, IMAP_FAIL_SINCE_KEY).and_then(|s| parse_utc(&s)) {
                Some(t) => t,
                None => {
                    put(conn, IMAP_FAIL_SINCE_KEY, &fmt(now));
                    now
                }
            };
            if now - since_fail >= Duration::hours(IMAP_DOWN_HOURS) {
                let n = Notice::new(
                    "imap",
                    fmt(since_fail),
                    format!("Synchro IMAP en échec depuis plus de {} h", IMAP_DOWN_HOURS),
                    format!("Les réponses ne sont plus détectées. {}", excerpt(&err, 160)),
                    1,
                )
                .at(Some(since_fail));
                record(conn, &n, status, now);
            }
        }
    }
}

// ---------------- Daily summary ----------------

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Summary {
    pub sent: i64,
    pub opened: i64,
    pub openers: Vec<String>,
    pub epk_views: i64,
    pub epk_listens: i64,
    pub replies: BTreeMap<ReplyClass, i64>,
    pub scheduled: i64,
    /// Next scheduled send, Paris time, "31/10 à 09:15".
    pub next_send: Option<String>,
}

fn plural(n: i64, one: &str, many: &str) -> String {
    format!("{} {}", n, if n > 1 { many } else { one })
}

pub fn summary_notice(day: &str, s: &Summary) -> Notice {
    let mut lines = vec![format!("Envoyés : {}", s.sent)];
    let mut opened = format!("Ouverts : {}", s.opened);
    if !s.openers.is_empty() {
        let shown: Vec<&str> = s.openers.iter().take(4).map(String::as_str).collect();
        let more = s.openers.len().saturating_sub(shown.len());
        opened.push_str(&format!(" ({}{})", shown.join(", "), if more > 0 { format!(" et {} autres", more) } else { String::new() }));
    }
    lines.push(opened);
    lines.push(format!("EPK : {}, {}", plural(s.epk_views, "vue", "vues"), plural(s.epk_listens, "écoute", "écoutes")));
    let total: i64 = s.replies.values().sum();
    if total == 0 {
        lines.push("Réponses : aucune".into());
    } else {
        let detail: Vec<String> = s.replies.iter().map(|(c, n)| format!("{} {}", n, c.label())).collect();
        lines.push(format!("Réponses : {} ({})", total, detail.join(", ")));
    }
    lines.push(match (&s.next_send, s.scheduled) {
        (_, 0) => "À venir : rien de programmé".into(),
        (Some(t), n) => format!("À venir : {}, prochain le {}", plural(n, "envoi programmé", "envois programmés"), t),
        (None, n) => format!("À venir : {}", plural(n, "envoi programmé", "envois programmés")),
    });
    Notice::new(
        "summary",
        day,
        format!("Bilan du jour : {}", plural(s.sent, "envoi", "envois")),
        lines.join("\n"),
        -1,
    )
}

fn in_range(s: &str, start: DateTime<Utc>, end: DateTime<Utc>) -> bool {
    parse_utc(s).map_or(false, |t| t >= start && t < end)
}

pub fn build_summary(conn: &Connection, start: DateTime<Utc>, end: DateTime<Utc>, now: DateTime<Utc>) -> Summary {
    let (s0, s1) = (fmt(start), fmt(end));
    let mut s = Summary {
        sent: conn
            .query_row(
                "SELECT COUNT(*) FROM emails WHERE status = 'sent' AND sent_at >= ?1 AND sent_at < ?2",
                params![s0, s1],
                |r| r.get(0),
            )
            .unwrap_or(0),
        ..Default::default()
    };
    if let Ok(mut st) = conn.prepare(&format!(
        "SELECT e.opened_at, COALESCE({name}, e.to_addr) FROM emails e LEFT JOIN contacts k ON k.id = e.contact_id
         WHERE e.opened_at IS NOT NULL AND e.status = 'sent' AND e.sent_at >= ?1 ORDER BY e.opened_at",
        name = NAME_SQL
    )) {
        let rows: Vec<(String, String)> = st
            .query_map(params![fmt(start - Duration::days(30))], |r| Ok((r.get(0)?, r.get(1)?)))
            .map(|it| it.flatten().collect())
            .unwrap_or_default();
        for (at, name) in rows {
            if in_range(&at, start, end) {
                s.opened += 1;
                if !s.openers.contains(&name) {
                    s.openers.push(name);
                }
            }
        }
    }
    if let Ok(mut st) = conn.prepare(
        "SELECT type, at, COALESCE(seconds, 0) FROM press_activity WHERE type IN ('view','listen') AND created_at >= ?1",
    ) {
        let rows: Vec<(String, String, f64)> = st
            .query_map(params![fmt(start - Duration::days(1))], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .map(|it| it.flatten().collect())
            .unwrap_or_default();
        for (t, at, secs) in rows {
            if !in_range(&at, start, end) {
                continue;
            }
            if t == "view" {
                s.epk_views += 1;
            } else if secs >= LISTEN_MIN_SECS {
                s.epk_listens += 1;
            }
        }
    }
    for (r, _) in reply_rows(conn, &s0, Some(&s1)) {
        if let Some(c) = classify_reply(&r, now, false) {
            *s.replies.entry(c).or_insert(0) += 1;
        }
    }
    let (n, next): (i64, Option<String>) = conn
        .query_row(
            "SELECT COUNT(*), MIN(scheduled_for) FROM campaign_messages m
             JOIN residency_campaigns r ON r.campaign_id = m.campaign_id
             WHERE m.status = 'scheduled' AND r.autopilot = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap_or((0, None));
    s.scheduled = n;
    s.next_send = next.and_then(|t| parse_utc(&t)).map(|t| {
        let t = t.max(now);
        Tz::Paris.to_local(t).format("%d/%m à %H:%M").to_string()
    });
    s
}

fn scan_summary(conn: &Connection, cfg: &PushConfig, now: DateTime<Utc>) {
    if !cfg.summary {
        return;
    }
    let local = Tz::Paris.to_local(now);
    if local.hour() < SUMMARY_HOUR {
        return;
    }
    let day = local.date().format("%Y-%m-%d").to_string();
    if logged(conn, "summary", &day) {
        return;
    }
    let start = Tz::Paris.to_utc(local.date().and_hms_opt(0, 0, 0).expect("midnight"));
    let end = Tz::Paris.to_utc((local.date() + Duration::days(1)).and_hms_opt(0, 0, 0).expect("midnight"));
    let s = build_summary(conn, start, end, now);
    if s.sent == 0 {
        return; // Only on days something was sent; checked again next tick.
    }
    record(conn, &summary_notice(&day, &s).at(Some(now)), "queued", now);
}

// ---------------------------------------------------------------------------
// Delivery
// ---------------------------------------------------------------------------

/// Send what is queued. Quiet hours hold non-urgent notices (priority < 1)
/// until they end. A 4xx stops sending until the configuration changes; a
/// transient error leaves the rest queued for the next tick. Returns how many
/// were sent.
pub fn deliver(conn: &Connection, pusher: &dyn Pusher, cfg: &PushConfig, now: DateTime<Utc>) -> usize {
    let _ = conn.execute(
        "UPDATE push_log SET status = 'expired' WHERE status IN ('queued','held') AND created_at < ?1",
        params![fmt(now - Duration::hours(EXPIRE_HOURS))],
    );
    if get(conn, BLOCKED_KEY).is_some() {
        return 0;
    }
    let quiet = in_quiet_hours(cfg, now);
    let rows: Vec<(i64, String, String, i64, Option<String>, Option<String>, Option<String>)> = conn
        .prepare(&format!(
            "SELECT id, title, message, priority, url, url_title, event_at FROM push_log
             WHERE status IN ('queued','held') ORDER BY id LIMIT {}",
            BATCH
        ))
        .and_then(|mut st| {
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)))
                .map(|it| it.flatten().collect())
        })
        .unwrap_or_default();
    let mut sent = 0;
    for (id, title, message, priority, url, url_title, event_at) in rows {
        if quiet && priority < 1 {
            let _ = conn.execute("UPDATE push_log SET status = 'held' WHERE id = ?1", params![id]);
            continue;
        }
        let m = Message {
            title,
            message,
            priority,
            url,
            url_title,
            sound: None,
            timestamp: event_at.and_then(|t| parse_utc(&t)).map(|t| t.timestamp()),
        };
        match pusher.send(&m) {
            Ok(()) => {
                let _ = conn.execute(
                    "UPDATE push_log SET status = 'sent', sent_at = ?2, error = NULL WHERE id = ?1",
                    params![id, fmt(now)],
                );
                put(conn, LAST_OK_KEY, &fmt(now));
                sent += 1;
            }
            Err(e) => {
                put(conn, LAST_ERROR_KEY, e.text());
                put(conn, LAST_ERROR_AT_KEY, &fmt(now));
                let _ = conn.execute("UPDATE push_log SET error = ?2 WHERE id = ?1", params![id, e.text()]);
                if let PushError::Config(_) = e {
                    let _ = conn.execute("UPDATE push_log SET status = 'failed' WHERE id = ?1", params![id]);
                    put(conn, BLOCKED_KEY, "1");
                }
                break;
            }
        }
    }
    if sent > 0 && get(conn, BLOCKED_KEY).is_none() {
        del(conn, LAST_ERROR_KEY);
        del(conn, LAST_ERROR_AT_KEY);
    }
    sent
}

/// One pass: scan every source, then deliver.
pub fn tick_with(conn: &Connection, pusher: &dyn Pusher, cfg: &PushConfig, now: DateTime<Utc>) -> usize {
    if !cfg.enabled {
        return 0;
    }
    scan_replies(conn, cfg, now);
    scan_epk(conn, cfg, now);
    scan_opens(conn, cfg, now);
    scan_problems(conn, cfg, now);
    scan_summary(conn, cfg, now);
    deliver(conn, pusher, cfg, now)
}

static RUNNING: AtomicBool = AtomicBool::new(false);

/// Entry point of the periodic loop (residency engine). No keys, nothing done.
pub fn tick(pool: &DbPool) {
    if RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    if let Ok(conn) = pool.get() {
        let cfg = load_config(&conn);
        if cfg.enabled {
            if let Some((user, token)) = credentials(&conn) {
                tick_with(&conn, &HttpPusher::new(&user, &token), &cfg, Utc::now());
            }
        }
    }
    RUNNING.store(false, Ordering::SeqCst);
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct PushView {
    pub config: PushConfig,
    /// MASK when set, "" otherwise (secrets never go back to the UI).
    pub user: String,
    pub token: String,
    pub last_error: Option<String>,
    pub last_error_at: Option<String>,
    pub last_sent_at: Option<String>,
    pub blocked: bool,
    pub held: i64,
    pub sent_today: i64,
}

#[tauri::command(async)]
pub fn push_get(state: State<'_, AppState>) -> Result<PushView, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    Ok(view(&conn))
}

fn view(conn: &Connection) -> PushView {
    let count = |sql: &str| conn.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap_or(0);
    PushView {
        config: load_config(conn),
        user: crate::secrets::ui_setting(conn, USER_KEY).unwrap_or_default(),
        token: crate::secrets::ui_setting(conn, TOKEN_KEY).unwrap_or_default(),
        last_error: get(conn, LAST_ERROR_KEY),
        last_error_at: get(conn, LAST_ERROR_AT_KEY),
        last_sent_at: get(conn, LAST_OK_KEY),
        blocked: get(conn, BLOCKED_KEY).is_some(),
        held: count("SELECT COUNT(*) FROM push_log WHERE status = 'held'"),
        sent_today: count("SELECT COUNT(*) FROM push_log WHERE status = 'sent' AND sent_at >= date('now')"),
    }
}

#[tauri::command(async)]
pub fn push_save(
    state: State<'_, AppState>,
    config: PushConfig,
    user: String,
    token: String,
) -> Result<PushView, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    for t in [&config.quiet_start, &config.quiet_end] {
        if hm(t).is_none() {
            return Err(format!("Heure invalide : {} (format HH:MM)", t));
        }
    }
    crate::secrets::write_setting(&conn, USER_KEY, &user)?;
    crate::secrets::write_setting(&conn, TOKEN_KEY, &token)?;
    save_config(&conn, &config);
    // New keys: give sending another chance.
    del(&conn, BLOCKED_KEY);
    Ok(view(&conn))
}

/// "Envoyer un test": checks the user key, then sends a test notification.
#[tauri::command(async)]
pub fn push_test(state: State<'_, AppState>) -> Result<PushView, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let (user, token) =
        credentials(&conn).ok_or_else(|| "Renseignez la clé utilisateur et le jeton d'application Pushover.".to_string())?;
    let pusher = HttpPusher::new(&user, &token);
    run_test(&conn, &pusher, Utc::now())?;
    Ok(view(&conn))
}

pub fn run_test(conn: &Connection, pusher: &dyn Pusher, now: DateTime<Utc>) -> Result<(), String> {
    let r = pusher.validate().and_then(|_| {
        pusher.send(&Message {
            title: "PiersCRM".into(),
            message: "Test réussi : les notifications arrivent sur ce téléphone.".into(),
            priority: 0,
            ..Default::default()
        })
    });
    match r {
        Ok(()) => {
            del(conn, BLOCKED_KEY);
            del(conn, LAST_ERROR_KEY);
            del(conn, LAST_ERROR_AT_KEY);
            put(conn, LAST_OK_KEY, &fmt(now));
            Ok(())
        }
        Err(e) => {
            put(conn, LAST_ERROR_KEY, e.text());
            put(conn, LAST_ERROR_AT_KEY, &fmt(now));
            Err(e.text().to_string())
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct FakePusher {
        sent: RefCell<Vec<Message>>,
        fail: RefCell<Option<PushError>>,
    }
    impl Pusher for FakePusher {
        fn send(&self, m: &Message) -> Result<(), PushError> {
            if let Some(e) = self.fail.borrow().clone() {
                return Err(e);
            }
            self.sent.borrow_mut().push(m.clone());
            Ok(())
        }
        fn validate(&self) -> Result<(), PushError> {
            match self.fail.borrow().clone() {
                Some(e) => Err(e),
                None => Ok(()),
            }
        }
    }

    fn utc(s: &str) -> DateTime<Utc> {
        parse_utc(s).unwrap()
    }

    fn setup() -> Connection {
        let c = crate::commands::residency::testdb::conn();
        c.execute_batch(
            "INSERT INTO contacts (id, category, name, venue, email, status) VALUES
               (1, 'Club', 'Marc', 'Badaboum', 'marc@badaboum.fr', 'contacted'),
               (2, 'Club', 'Léa', 'Rex Club', 'lea@rex.fr', 'contacted');",
        )
        .unwrap();
        c
    }

    /// Daytime in Paris, outside quiet hours (14:00 local in winter time).
    const NOW: &str = "2026-11-10 13:00:00";

    fn reply(c: &Connection, id: i64, contact: i64, kind: &str, text: &str, created: &str) {
        c.execute(
            "INSERT INTO email_replies (id, contact_id, from_addr, subject, message_id, kind, body_excerpt, created_at)
             VALUES (?1, ?2, 'x@y.fr', 'Re: Piers', ?3, ?4, ?5, ?6)",
            params![id, contact, format!("<m{}@y>", id), kind, text, created],
        )
        .unwrap();
    }

    fn insight(c: &Connection, reply_id: i64, intent: &str) {
        c.execute(
            "INSERT INTO reply_insights (email_reply_id, sentiment, intent) VALUES (?1, 'neutral', ?2)",
            params![reply_id, intent],
        )
        .unwrap();
    }

    #[test]
    fn priority_mapping() {
        assert_eq!(ReplyClass::Interested.priority(), 1);
        assert_eq!(ReplyClass::Question.priority(), 1);
        assert_eq!(ReplyClass::Unknown.priority(), 1);
        assert_eq!(ReplyClass::Refusal.priority(), 0);
        assert_eq!(ReplyClass::Later.priority(), 0);
        assert_eq!(ReplyClass::OutOfOffice.priority(), -1);
        assert_eq!(ReplyClass::from_intent("not_interested"), Some(ReplyClass::Refusal));
        assert_eq!(ReplyClass::from_intent("bogus"), None);
    }

    #[test]
    fn reply_notified_once_with_classification() {
        let c = setup();
        let now = utc(NOW);
        reply(&c, 1, 1, "reply", "Bonjour, oui avec plaisir, on peut s'appeler jeudi ?", "2026-11-10 12:50:00");
        insight(&c, 1, "interested");
        let p = FakePusher::default();
        let cfg = PushConfig::default();
        assert_eq!(tick_with(&c, &p, &cfg, now), 1);
        {
            let sent = p.sent.borrow();
            assert_eq!(sent[0].title, "Réponse de Badaboum : intéressé");
            assert_eq!(sent[0].priority, 1);
            assert!(sent[0].message.starts_with("Bonjour, oui avec plaisir"));
        }
        // Next ticks (and a restart: the log is in the DB) send nothing again.
        assert_eq!(tick_with(&c, &p, &cfg, now + Duration::minutes(5)), 0);
        assert_eq!(p.sent.borrow().len(), 1);
    }

    #[test]
    fn reply_waits_for_classification_then_falls_back() {
        let c = setup();
        let now = utc(NOW);
        reply(&c, 1, 2, "reply", "Pouvez-vous m'envoyer votre fiche technique ?", "2026-11-10 12:55:00");
        let p = FakePusher::default();
        let cfg = PushConfig::default();
        assert_eq!(tick_with(&c, &p, &cfg, now), 0, "waits for the classification");
        // The reply desk classified it meanwhile.
        c.execute(
            "INSERT INTO reply_desk (thread_key, kind, email_reply_id, to_addr, intent, since) VALUES ('t1','reply',1,'x@y.fr','question','x')",
            [],
        )
        .unwrap();
        assert_eq!(tick_with(&c, &p, &cfg, now + Duration::minutes(5)), 1);
        assert_eq!(p.sent.borrow()[0].title, "Réponse de Rex Club : question");

        // Nothing classifies it: after the wait it goes out as "Réponse de".
        reply(&c, 2, 1, "reply", "Hello", "2026-11-10 13:00:00");
        assert_eq!(tick_with(&c, &p, &cfg, now + Duration::minutes(CLASSIFY_WAIT_MIN + 1)), 1);
        assert_eq!(p.sent.borrow()[1].title, "Réponse de Badaboum");
        assert_eq!(p.sent.borrow()[1].priority, 1);
    }

    #[test]
    fn refusal_and_out_of_office() {
        let c = setup();
        let now = utc(NOW);
        reply(&c, 1, 1, "reply", "Merci mais ce n'est pas pour nous.", "2026-11-10 12:00:00");
        insight(&c, 1, "not_interested");
        reply(&c, 2, 2, "auto_reply", "Je suis absente jusqu'au 15.", "2026-11-10 12:00:00");
        let p = FakePusher::default();
        let mut cfg = PushConfig::default();
        assert_eq!(tick_with(&c, &p, &cfg, now), 1, "out-of-office is off by default");
        assert_eq!(p.sent.borrow()[0].priority, 0);
        assert_eq!(p.sent.borrow()[0].title, "Réponse de Badaboum : refus");
        // Turning it on later does not resurrect the one already seen.
        cfg.out_of_office = true;
        assert_eq!(tick_with(&c, &p, &cfg, now), 0);
        reply(&c, 3, 2, "auto_reply", "Out of office", "2026-11-10 12:30:00");
        assert_eq!(tick_with(&c, &p, &cfg, now), 1);
        assert_eq!(p.sent.borrow()[1].priority, -1);
    }

    #[test]
    fn old_events_are_not_notified() {
        let c = setup();
        reply(&c, 1, 1, "reply", "vieux", "2026-11-01 12:00:00");
        insight(&c, 1, "interested");
        let p = FakePusher::default();
        assert_eq!(tick_with(&c, &p, &PushConfig::default(), utc(NOW)), 0);
    }

    #[test]
    fn quiet_hours() {
        let cfg = PushConfig::default();
        // Winter: Paris = UTC+1. 21:59 UTC = 22:59 Paris.
        assert!(!in_quiet_hours(&cfg, utc("2026-11-10 20:59:00")));
        assert!(in_quiet_hours(&cfg, utc("2026-11-10 21:00:00")));
        assert!(in_quiet_hours(&cfg, utc("2026-11-11 06:59:00")));
        assert!(!in_quiet_hours(&cfg, utc("2026-11-11 07:00:00")));
        // Summer: Paris = UTC+2.
        assert!(in_quiet_hours(&cfg, utc("2026-07-10 20:30:00")));
        assert!(!in_quiet_hours(&cfg, utc("2026-07-10 06:30:00")));
        let off = PushConfig { quiet_enabled: false, ..Default::default() };
        assert!(!in_quiet_hours(&off, utc("2026-11-10 23:00:00")));
        let day = PushConfig { quiet_start: "12:00".into(), quiet_end: "14:00".into(), ..Default::default() };
        assert!(in_quiet_hours(&day, utc("2026-11-10 12:30:00")));
        assert!(!in_quiet_hours(&day, utc("2026-11-10 14:30:00")));
    }

    #[test]
    fn quiet_hours_hold_non_urgent_and_send_urgent() {
        let c = setup();
        let night = utc("2026-11-10 23:00:00"); // 00:00 Paris
        reply(&c, 1, 1, "reply", "Ok pour nous", "2026-11-10 22:50:00");
        insight(&c, 1, "interested");
        reply(&c, 2, 2, "reply", "Non merci", "2026-11-10 22:50:00");
        insight(&c, 2, "not_interested");
        let p = FakePusher::default();
        let cfg = PushConfig::default();
        assert_eq!(tick_with(&c, &p, &cfg, night), 1, "the urgent one goes out");
        assert_eq!(p.sent.borrow()[0].priority, 1);
        let held: i64 = c.query_row("SELECT COUNT(*) FROM push_log WHERE status = 'held'", [], |r| r.get(0)).unwrap();
        assert_eq!(held, 1);
        // Morning (08:05 Paris): the held one goes, with its real time.
        assert_eq!(tick_with(&c, &p, &cfg, utc("2026-11-11 07:05:00")), 1);
        let m = p.sent.borrow()[1].clone();
        assert_eq!(m.title, "Réponse de Rex Club : refus");
        assert_eq!(m.timestamp, Some(utc("2026-11-10 22:50:00").timestamp()));
    }

    fn listen(c: &Connection, id: i64, contact: i64, track: &str, secs: f64, at: &str) {
        c.execute(
            "INSERT INTO press_activity (id, event_id, contact_id, type, at, track_title, seconds, created_at)
             VALUES (?1, ?2, ?3, 'listen', ?4, ?5, ?6, ?4)",
            params![id, format!("ev{}", id), contact, at, track, secs],
        )
        .unwrap();
    }

    #[test]
    fn listens_grouped_per_contact() {
        let c = setup();
        listen(&c, 1, 1, "Sunset Drive", 150.0, "2026-11-10 12:00:00");
        listen(&c, 2, 1, "Chrome", 240.0, "2026-11-10 12:20:00");
        listen(&c, 3, 1, "Too short", 30.0, "2026-11-10 12:21:00");
        listen(&c, 4, 1, "Later", 90.0, "2026-11-10 12:45:00");
        listen(&c, 5, 2, "Sunset Drive", 60.0, "2026-11-10 12:40:00");
        let p = FakePusher::default();
        let cfg = PushConfig::default();
        // 12:35: Badaboum's first group (12:00 to 12:30) is closed; the rest is open.
        assert_eq!(tick_with(&c, &p, &cfg, utc("2026-11-10 12:35:00")), 1);
        {
            let s = p.sent.borrow();
            assert_eq!(s[0].title, "insrt : Badaboum écoute");
            assert_eq!(s[0].message, "2 écoutes en 30 min : « Sunset Drive » (3 min), « Chrome » (4 min).");
        }
        // 13:20: Badaboum's 12:45 listen and Rex Club's 12:40 one.
        assert_eq!(tick_with(&c, &p, &cfg, utc("2026-11-10 13:20:00")), 2);
        let s = p.sent.borrow();
        assert!(s.iter().any(|m| m.message == "A écouté « Later » (2 min)."));
        assert!(s.iter().any(|m| m.title == "insrt : Rex Club écoute" && m.message == "A écouté « Sunset Drive » (1 min)."));
        drop(s);
        assert_eq!(tick_with(&c, &p, &cfg, utc("2026-11-10 14:00:00")), 0);
    }

    #[test]
    fn epk_first_view_and_feedback() {
        let c = setup();
        c.execute_batch(
            "INSERT INTO press_activity (event_id, contact_id, link_id, type, at, created_at) VALUES
               ('v1', 1, 'L1', 'view', '2026-11-10 11:00:00', '2026-11-10 11:00:00'),
               ('v2', 1, 'L1', 'view', '2026-11-10 11:30:00', '2026-11-10 11:30:00');
             INSERT INTO press_activity (event_id, contact_id, link_id, type, at, rating, comment, created_at) VALUES
               ('f1', 2, 'L2', 'feedback', '2026-11-10 12:00:00', 4, 'Super énergie, on en parle ?', '2026-11-10 12:00:00');",
        )
        .unwrap();
        let p = FakePusher::default();
        assert_eq!(tick_with(&c, &p, &PushConfig::default(), utc(NOW)), 2);
        let s = p.sent.borrow();
        assert_eq!(s[0].title, "insrt : Badaboum a ouvert le dossier");
        assert_eq!(s[1].title, "insrt : retour de Rex Club");
        assert_eq!(s[1].message, "note 4 : Super énergie, on en parle ?");
        assert!(s.iter().all(|m| m.priority == 0));
    }

    #[test]
    fn opens_go_to_summary_by_default() {
        let c = setup();
        c.execute(
            "INSERT INTO emails (id, contact_id, to_addr, subject, body, status, sent_at, opened_at)
             VALUES (1, 1, 'marc@badaboum.fr', 'Piers, résidence', 'x', 'sent', '2026-11-10 09:00:00', '2026-11-10T10:00:00Z')",
            [],
        )
        .unwrap();
        let p = FakePusher::default();
        let mut cfg = PushConfig::default();
        assert_eq!(tick_with(&c, &p, &cfg, utc(NOW)), 0);
        assert!(logged(&c, "open", "1"));
        // Push mode: a new open is a silent push.
        cfg.opens_in_summary = false;
        c.execute(
            "INSERT INTO emails (id, contact_id, to_addr, subject, body, status, sent_at, opened_at)
             VALUES (2, 2, 'lea@rex.fr', 'Piers', 'x', 'sent', '2026-11-10 09:00:00', '2026-11-10 11:00:00')",
            [],
        )
        .unwrap();
        assert_eq!(tick_with(&c, &p, &cfg, utc(NOW)), 1);
        assert_eq!(p.sent.borrow()[0].title, "Ouvert : Rex Club");
        assert_eq!(p.sent.borrow()[0].priority, -1);
    }

    #[test]
    fn problems_are_urgent() {
        let c = setup();
        c.execute_batch(
            "INSERT INTO suppressions (address, reason, created_at) VALUES ('marc@badaboum.fr', 'bounce', '2026-11-10 12:00:00');
             INSERT INTO residency_log (level, code, message, at) VALUES ('pause', 'deliverability_pause', 'Taux de plainte trop élevé', '2026-11-10 12:10:00');
             INSERT INTO residency_log (level, code, message, at) VALUES ('info', 'sent', 'x', '2026-11-10 12:10:00');
             INSERT INTO settings (key, value) VALUES ('imap_last_error', 'login failed');",
        )
        .unwrap();
        let p = FakePusher::default();
        let cfg = PushConfig::default();
        let now = utc(NOW);
        assert_eq!(tick_with(&c, &p, &cfg, now), 2, "IMAP has only just started failing");
        assert_eq!(p.sent.borrow()[0].message, "marc@badaboum.fr (Badaboum) : rebond définitif, retirée des envois.");
        assert_eq!(p.sent.borrow()[1].title, "Envois automatiques en pause");
        assert!(p.sent.borrow().iter().all(|m| m.priority == 1));
        assert_eq!(tick_with(&c, &p, &cfg, now + Duration::minutes(119)), 0);
        assert_eq!(tick_with(&c, &p, &cfg, now + Duration::minutes(121)), 1);
        assert!(p.sent.borrow()[2].title.starts_with("Synchro IMAP en échec"));
        assert_eq!(tick_with(&c, &p, &cfg, now + Duration::minutes(200)), 0, "once per outage");
        // Sync recovers, fails again later: a new outage, a new notice.
        c.execute("DELETE FROM settings WHERE key = 'imap_last_error'", []).unwrap();
        tick_with(&c, &p, &cfg, now + Duration::minutes(210));
        c.execute("INSERT INTO settings (key, value) VALUES ('imap_last_error', 'timeout')", []).unwrap();
        tick_with(&c, &p, &cfg, now + Duration::minutes(215));
        assert_eq!(tick_with(&c, &p, &cfg, now + Duration::minutes(340)), 1);
    }

    #[test]
    fn summary_text() {
        let mut s = Summary {
            sent: 12,
            opened: 5,
            openers: vec!["Badaboum".into(), "Rex Club".into(), "Le Sucre".into(), "Fabric".into(), "Concrete".into()],
            epk_views: 3,
            epk_listens: 1,
            scheduled: 8,
            next_send: Some("11/11 à 09:15".into()),
            ..Default::default()
        };
        s.replies.insert(ReplyClass::Interested, 1);
        s.replies.insert(ReplyClass::Refusal, 2);
        let n = summary_notice("2026-11-10", &s);
        assert_eq!(n.title, "Bilan du jour : 12 envois");
        assert_eq!(n.priority, -1);
        assert_eq!(
            n.message,
            "Envoyés : 12\nOuverts : 5 (Badaboum, Rex Club, Le Sucre, Fabric et 1 autres)\nEPK : 3 vues, 1 écoute\n\
             Réponses : 3 (1 intéressé, 2 refus)\nÀ venir : 8 envois programmés, prochain le 11/11 à 09:15"
        );
        let empty = summary_notice("d", &Summary { sent: 1, ..Default::default() });
        assert_eq!(
            empty.message,
            "Envoyés : 1\nOuverts : 0\nEPK : 0 vue, 0 écoute\nRéponses : aucune\nÀ venir : rien de programmé"
        );
        assert!(!empty.message.contains('—') && !n.message.contains('—'));
    }

    #[test]
    fn summary_at_18_only_on_days_with_sends() {
        let c = setup();
        let p = FakePusher::default();
        let cfg = PushConfig::default();
        // 18:30 Paris, nothing sent: no summary.
        assert_eq!(tick_with(&c, &p, &cfg, utc("2026-11-10 17:30:00")), 0);
        c.execute_batch(
            "INSERT INTO emails (id, contact_id, to_addr, subject, body, status, sent_at, opened_at) VALUES
               (1, 1, 'marc@badaboum.fr', 's', 'b', 'sent', '2026-11-10 09:00:00', '2026-11-10 10:00:00'),
               (2, 2, 'lea@rex.fr', 's', 'b', 'sent', '2026-11-10 09:05:00', NULL),
               (3, 2, 'lea@rex.fr', 's', 'b', 'sent', '2026-11-09 09:05:00', NULL);",
        )
        .unwrap();
        reply(&c, 1, 1, "reply", "Oui !", "2026-11-10 11:00:00");
        insight(&c, 1, "interested");
        // 17:00 Paris: too early (the reply goes out on its own).
        assert_eq!(tick_with(&c, &p, &cfg, utc("2026-11-10 16:00:00")), 1);
        assert_eq!(tick_with(&c, &p, &cfg, utc("2026-11-10 17:30:00")), 1);
        let m = p.sent.borrow()[1].clone();
        assert_eq!(m.title, "Bilan du jour : 2 envois");
        assert!(m.message.contains("Ouverts : 1 (Badaboum)"), "{}", m.message);
        assert!(m.message.contains("Réponses : 1 (1 intéressé)"), "{}", m.message);
        assert_eq!(tick_with(&c, &p, &cfg, utc("2026-11-10 18:30:00")), 0, "once a day");
    }

    #[test]
    fn bad_config_stops_sending_transient_retries() {
        let c = setup();
        let now = utc(NOW);
        reply(&c, 1, 1, "reply", "Oui", "2026-11-10 12:00:00");
        insight(&c, 1, "interested");
        reply(&c, 2, 2, "reply", "Oui aussi", "2026-11-10 12:00:00");
        insight(&c, 2, "interested");
        let p = FakePusher::default();
        let cfg = PushConfig::default();
        *p.fail.borrow_mut() = Some(PushError::Transient("timeout".into()));
        assert_eq!(tick_with(&c, &p, &cfg, now), 0);
        assert_eq!(get(&c, LAST_ERROR_KEY).as_deref(), Some("timeout"));
        *p.fail.borrow_mut() = None;
        assert_eq!(tick_with(&c, &p, &cfg, now), 2, "retried");
        assert_eq!(get(&c, LAST_ERROR_KEY), None);

        reply(&c, 3, 1, "reply", "Encore", "2026-11-10 12:00:00");
        insight(&c, 3, "question");
        *p.fail.borrow_mut() = Some(PushError::Config("Pushover refuse : user key is invalid".into()));
        assert_eq!(tick_with(&c, &p, &cfg, now), 0);
        assert!(get(&c, BLOCKED_KEY).is_some());
        *p.fail.borrow_mut() = None;
        assert_eq!(tick_with(&c, &p, &cfg, now), 0, "blocked until the config changes");
        run_test(&c, &p, now).unwrap();
        assert!(get(&c, BLOCKED_KEY).is_none());
        assert_eq!(p.sent.borrow().last().unwrap().title, "PiersCRM");
    }

    #[test]
    fn answers_and_encoding() {
        assert_eq!(read_answer(200, r#"{"status":1,"request":"x"}"#), Ok(()));
        assert!(matches!(read_answer(400, r#"{"user":"invalid","errors":["user identifier is invalid"],"status":0}"#),
            Err(PushError::Config(e)) if e.contains("user identifier is invalid")));
        assert!(matches!(read_answer(500, ""), Err(PushError::Transient(_))));
        assert!(matches!(read_answer(200, r#"{"status":0}"#), Err(PushError::Transient(_))));
        assert_eq!(form_encode(&[("title", "Réponse : oui & non".into())]), "title=R%C3%A9ponse+%3A+oui+%26+non");
    }

    #[test]
    fn config_defaults() {
        let d = PushConfig::default();
        assert!(d.enabled && d.replies && d.epk && d.opens && d.opens_in_summary && d.problems && d.summary);
        assert!(!d.out_of_office);
        assert_eq!((d.quiet_start.as_str(), d.quiet_end.as_str()), ("22:00", "08:00"));
        let c = setup();
        assert_eq!(load_config(&c), d);
    }
}
