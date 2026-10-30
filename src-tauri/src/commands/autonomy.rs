//! Graduated autonomy for the reply desk. Emails stay in Thibault's voice and
//! signed Thibault: the agent (internal name in the app only, see
//! commands/playbook.rs) may at most send a few low-risk answers on its own.
//!
//! Levels, per category (settings `autonomy_<category>`, all 0 by default):
//! - 0 "je valide tout": nothing leaves without Thibault's click (as before);
//! - 1 "envoi seul du sans-risque": a draft of that category is scheduled to go
//!   out after a delay (setting `autosend_delay_min`, 15 by default), can be
//!   cancelled from the inbox, and the notification says when it leaves.
//!
//! Categories: sending the EPK / mix when they ask for music, proposing call
//! slots, a polite answer to an out-of-office or a "on revient vers vous", the
//! light follow-ups of stalled threads (reply desk nudges).
//!
//! The hard safety layer is code, not prompt: `may_autosend` refuses any draft
//! (or incoming message) mentioning money, a figure, a calendar date or a
//! hold, a contract, exclusivity, a deposit, a fee, any refusal or negative
//! turn; a classifier confidence under MIN_CONFIDENCE; a draft no approved
//! playbook entry covers. It runs when the send is scheduled and again right
//! before it leaves. Sends go through the threaded send path
//! (email::send_one_in_thread: suppression list, daily cap, sending budget)
//! after the automated-send health gate, every one is logged with its reason
//! (`autosends`), and setting `autosend_paused` = 1 stops everything at once.

use chrono::{Duration, NaiveDateTime, TimeZone, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::commands::email::{SendResult, ThreadRef};
use crate::commands::{playbook, reply_desk, status};
use crate::AppState;

pub const DEFAULT_DELAY_MIN: i64 = 15;
/// Below this classifier confidence a draft always waits for Thibault.
pub const MIN_CONFIDENCE: f64 = 0.8;
/// A send held back by the sending budget is retried after this delay...
pub const DEFER_MIN: i64 = 30;
/// ...and given up (left to Thibault) after this long.
pub const GIVE_UP_HOURS: i64 = 24;

const TS: &str = "%Y-%m-%d %H:%M:%S";

fn fmt_ts(d: NaiveDateTime) -> String {
    d.format(TS).to_string()
}

fn parse_ts(s: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(s.get(..19).unwrap_or(s), TS).ok()
}

pub fn migrate(conn: &Connection) {
    for stmt in [
        "CREATE TABLE IF NOT EXISTS autosends (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            desk_id       INTEGER NOT NULL REFERENCES reply_desk(id) ON DELETE CASCADE,
            category      TEXT NOT NULL,
            reason        TEXT NOT NULL,
            confidence    REAL,
            playbook_ids  TEXT,
            subject       TEXT NOT NULL,
            body          TEXT NOT NULL,
            state         TEXT NOT NULL DEFAULT 'scheduled',
            due_at        TEXT NOT NULL,
            done_at       TEXT,
            note          TEXT,
            sent_email_id INTEGER REFERENCES emails(id) ON DELETE SET NULL,
            created_at    TEXT NOT NULL DEFAULT (datetime('now'))
        )",
        "CREATE INDEX IF NOT EXISTS idx_autosends_state ON autosends(state, due_at)",
        "CREATE INDEX IF NOT EXISTS idx_autosends_desk ON autosends(desk_id)",
        // What the draft does (category the model gave), how sure it is, and
        // why it waits for Thibault when its category is on autopilot.
        "ALTER TABLE reply_desk ADD COLUMN auto_category TEXT",
        "ALTER TABLE reply_desk ADD COLUMN auto_confidence REAL",
        "ALTER TABLE reply_desk ADD COLUMN auto_note TEXT",
    ] {
        let _ = conn.execute(stmt, []);
    }
}

// ---------------- Categories ----------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    /// They asked for music: send the EPK / mix link.
    MusicRequest,
    /// Propose a call (moments of the week, no calendar date).
    CallSlots,
    /// Out of office / "on revient vers vous": a polite holding answer.
    Holding,
    /// Light follow-up of a stalled thread.
    Nudge,
}

pub const CATEGORIES: [Category; 4] = [Category::MusicRequest, Category::CallSlots, Category::Holding, Category::Nudge];

impl Category {
    pub fn key(self) -> &'static str {
        match self {
            Category::MusicRequest => "music_request",
            Category::CallSlots => "call_slots",
            Category::Holding => "holding",
            Category::Nudge => "nudge",
        }
    }
    pub fn parse(s: &str) -> Option<Category> {
        CATEGORIES.iter().copied().find(|c| c.key() == s.trim())
    }
    pub fn label_fr(self) -> &'static str {
        match self {
            Category::MusicRequest => "envoi du mix ou du lien presse",
            Category::CallSlots => "proposition d'appel",
            Category::Holding => "réponse d'attente",
            Category::Nudge => "relance légère",
        }
    }
    /// Playbook topics that can back a draft of this category.
    pub fn topics(self) -> &'static [&'static str] {
        match self {
            Category::MusicRequest => &["mixes_links", "promo_assets", "obj_send_mix"],
            Category::CallSlots => &["call"],
            Category::Holding => &["holding"],
            Category::Nudge => &["follow_up"],
        }
    }
}

/// The reply a draft answers, as the safety layer sees it.
#[derive(Debug, Clone, Default)]
pub struct ReplyClass<'a> {
    pub category: Option<Category>,
    /// Desk / classifier intent (interested, question, not_interested...).
    pub intent: Option<&'a str>,
    /// Their message (subject + text), empty for a nudge.
    pub their_text: &'a str,
}

// ---------------- The hard safety layer ----------------

/// Words that end autonomy wherever they appear (folded, no accents).
/// Substrings: stems that never occur inside an innocent word.
const RISKY_STEMS: &[(&str, &str)] = &[
    ("exclusiv", "exclusivité"),
    ("contrat", "contrat"),
    ("contract", "contrat"),
    ("acompte", "acompte"),
    ("cachet", "cachet"),
    ("remuner", "rémunération"),
    ("defrai", "défraiement"),
    ("factur", "facture"),
    ("invoice", "facture"),
    ("paiement", "paiement"),
    ("payment", "paiement"),
    ("tarif", "tarif"),
    ("budget", "budget"),
    ("deposit", "acompte"),
    ("gratuit", "gratuit"),
    ("honorair", "honoraires"),
    ("week-end", "week-end"),
    ("weekend", "week-end"),
    ("week end", "week-end"),
    ("calendrier", "calendrier"),
    ("malheureusement", "refus"),
    ("unfortunately", "refus"),
    ("annul", "annulation"),
    ("cancel", "annulation"),
    ("refus", "refus"),
    ("declin", "refus"),
];

/// Whole words only.
const RISKY_WORDS: &[(&str, &str)] = &[
    ("fee", "fee"),
    ("fees", "fee"),
    ("pay", "paiement"),
    ("paid", "paiement"),
    ("payer", "paiement"),
    ("prix", "prix"),
    ("price", "prix"),
    ("pricing", "prix"),
    ("rate", "prix"),
    ("rates", "prix"),
    ("eur", "montant"),
    ("euro", "montant"),
    ("euros", "montant"),
    ("usd", "montant"),
    ("gbp", "montant"),
    ("chf", "montant"),
    ("ht", "montant"),
    ("ttc", "montant"),
    ("date", "date"),
    ("dates", "date"),
    ("hold", "option"),
    ("holds", "option"),
    ("option", "option"),
    ("options", "option"),
    ("janvier", "date"),
    ("fevrier", "date"),
    ("mars", "date"),
    ("avril", "date"),
    ("mai", "date"),
    ("juin", "date"),
    ("juillet", "date"),
    ("aout", "date"),
    ("septembre", "date"),
    ("octobre", "date"),
    ("novembre", "date"),
    ("decembre", "date"),
    ("janv", "date"),
    ("fevr", "date"),
    ("avr", "date"),
    ("juil", "date"),
    ("sept", "date"),
    ("oct", "date"),
    ("nov", "date"),
    ("dec", "date"),
    ("january", "date"),
    ("february", "date"),
    ("march", "date"),
    ("april", "date"),
    ("june", "date"),
    ("july", "date"),
    ("august", "date"),
    ("september", "date"),
    ("october", "date"),
    ("november", "date"),
    ("december", "date"),
    ("jan", "date"),
    ("feb", "date"),
    ("apr", "date"),
    ("jun", "date"),
    ("jul", "date"),
    ("aug", "date"),
    ("sep", "date"),
    ("desole", "refus"),
    ("sorry", "refus"),
];

fn fold(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| match c {
            'à' | 'â' | 'ä' | 'á' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'î' | 'ï' | 'í' => 'i',
            'ô' | 'ö' | 'ó' => 'o',
            'ù' | 'û' | 'ü' | 'ú' => 'u',
            'ç' => 'c',
            '\u{2019}' | '\u{2018}' => '\'',
            c => c,
        })
        .collect()
}

/// The first risky thing in a text (money, figure, date, hold, contract,
/// exclusivity, refusal), named in French. URLs are ignored (a mix link may
/// hold digits), and so are times of day ("15h", "10:30") and mix volume
/// numbers ("Vol. 4"), which a call proposal or a mix answer may need.
pub fn risky_term(text: &str) -> Option<&'static str> {
    if ['€', '$', '£'].iter().any(|c| text.contains(*c)) {
        return Some("montant");
    }
    let t = fold(text);
    let url = regex::Regex::new(r"(https?://|www\.)\S+").expect("valid regex");
    let t = url.replace_all(&t, " ").to_string();
    let harmless = regex::Regex::new(r"\b\d{1,2}\s?h(\d{2})?\b|\b\d{1,2}:\d{2}\b|\b\d{1,2}\s?(am|pm)\b|\bvol\.?\s?\d+\b")
        .expect("valid regex");
    let t = harmless.replace_all(&t, " ").to_string();
    if t.chars().any(|c| c.is_ascii_digit()) {
        return Some("chiffre ou date");
    }
    if let Some((_, label)) = RISKY_STEMS.iter().find(|(s, _)| t.contains(s)) {
        return Some(label);
    }
    let words: Vec<&str> = t.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    RISKY_WORDS.iter().find(|(w, _)| words.contains(w)).map(|(_, label)| *label)
}

/// May this draft go out without Thibault? Ok(reason logged with the send) or
/// Err(why it waits). Pure: everything it needs is passed in.
pub fn may_autosend(
    class: &ReplyClass,
    draft_text: &str,
    playbook_coverage: bool,
    confidence: f64,
    level: u8,
) -> Result<String, String> {
    let Some(cat) = class.category else {
        return Err("Pas une réponse sans risque.".into());
    };
    if level == 0 {
        return Err(format!("« {} » est réglé sur « je valide tout ».", cat.label_fr()));
    }
    if draft_text.trim().is_empty() {
        return Err("Brouillon vide.".into());
    }
    if let Some(i) = class.intent {
        if matches!(i, "not_interested" | "unsubscribe" | "wrong_person" | "later") {
            return Err("Réponse négative ou à recadrer.".into());
        }
    }
    if status::looks_like_refusal(class.their_text) || status::looks_like_refusal(draft_text) {
        return Err("Ton négatif ou refus.".into());
    }
    if !confidence.is_finite() || confidence < MIN_CONFIDENCE {
        return Err(format!("Confiance trop faible ({:.0} %).", (confidence.max(0.0) * 100.0).min(100.0)));
    }
    if !playbook_coverage {
        return Err("Aucune entrée validée de la bibliothèque ne couvre cette réponse.".into());
    }
    if let Some(term) = risky_term(draft_text) {
        return Err(format!("Le brouillon parle de {}.", term));
    }
    if let Some(term) = risky_term(class.their_text) {
        return Err(format!("Leur message parle de {}.", term));
    }
    Ok(format!("{}, couvert par la bibliothèque, confiance {:.0} %", cat.label_fr(), confidence * 100.0))
}

// ---------------- Settings ----------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Level {
    pub category: String,
    pub level: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Settings {
    /// false = every autonomous send is off (one-click switch).
    pub enabled: bool,
    pub delay_min: i64,
    pub levels: Vec<Level>,
    pub agent_name: String,
    pub min_confidence: f64,
}

fn setting(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM settings WHERE key = ?1", params![key], |r| r.get::<_, Option<String>>(0))
        .ok()
        .flatten()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn put(conn: &Connection, key: &str, value: &str) {
    let _ = conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    );
}

pub fn level_of(conn: &Connection, cat: Category) -> u8 {
    setting(conn, &format!("autonomy_{}", cat.key())).and_then(|v| v.parse().ok()).unwrap_or(0).min(1)
}

pub fn enabled(conn: &Connection) -> bool {
    setting(conn, "autosend_paused").as_deref() != Some("1")
}

pub fn delay_min(conn: &Connection) -> i64 {
    setting(conn, "autosend_delay_min").and_then(|v| v.parse().ok()).unwrap_or(DEFAULT_DELAY_MIN).clamp(5, 240)
}

pub fn load(conn: &Connection) -> Settings {
    Settings {
        enabled: enabled(conn),
        delay_min: delay_min(conn),
        levels: CATEGORIES.iter().map(|c| Level { category: c.key().into(), level: level_of(conn, *c) }).collect(),
        agent_name: playbook::agent_name(conn),
        min_confidence: MIN_CONFIDENCE,
    }
}

pub fn save(conn: &Connection, s: &Settings) {
    put(conn, "autosend_paused", if s.enabled { "0" } else { "1" });
    put(conn, "autosend_delay_min", &s.delay_min.clamp(5, 240).to_string());
    for l in &s.levels {
        if let Some(c) = Category::parse(&l.category) {
            put(conn, &format!("autonomy_{}", c.key()), &l.level.min(1).to_string());
        }
    }
    let name = s.agent_name.trim();
    put(conn, "agent_name", if name.is_empty() { playbook::DEFAULT_AGENT_NAME } else { name });
    if !s.enabled {
        cancel_all(conn, "Envois automatiques coupés.");
    }
}

/// Is anything on autopilot? (The draft prompt then asks for risk-free wording.)
pub fn any_on(conn: &Connection) -> bool {
    enabled(conn) && CATEGORIES.iter().any(|c| level_of(conn, *c) > 0)
}

// ---------------- Scheduling ----------------

struct DeskRow {
    kind: String,
    state: String,
    edited: bool,
    subject: String,
    body: String,
    intent: Option<String>,
    category: Option<String>,
    confidence: Option<f64>,
    playbook_ids: Option<String>,
    their: String,
}

fn desk_row(conn: &Connection, desk_id: i64) -> Option<DeskRow> {
    conn.query_row(
        "SELECT d.kind, d.state, d.edited, d.subject, d.body, d.intent, d.auto_category, d.auto_confidence, d.playbook_ids,
                COALESCE(r.subject, '') || char(10) || COALESCE(r.body_excerpt, '')
         FROM reply_desk d LEFT JOIN email_replies r ON r.id = d.email_reply_id WHERE d.id = ?1",
        params![desk_id],
        |r| {
            Ok(DeskRow {
                kind: r.get(0)?,
                state: r.get(1)?,
                edited: r.get::<_, i64>(2)? != 0,
                subject: r.get(3)?,
                body: r.get(4)?,
                intent: r.get(5)?,
                category: r.get(6)?,
                confidence: r.get(7)?,
                playbook_ids: r.get(8)?,
                their: r.get(9)?,
            })
        },
    )
    .ok()
}

/// The safety decision for one desk item as it stands now. None when its
/// category (if any) is not on autopilot: nothing to say then.
fn decide(conn: &Connection, row: &DeskRow) -> Option<(Category, f64, Result<String, String>)> {
    let nudge = row.kind == "nudge";
    let cat = if nudge { Some(Category::Nudge) } else { row.category.as_deref().and_then(Category::parse) }?;
    let level = level_of(conn, cat);
    if level == 0 {
        return None;
    }
    // A nudge comes from the thread state (rules), not a guess.
    let confidence = if nudge { 1.0 } else { row.confidence.unwrap_or(0.0) };
    let cited = playbook::parse_ids(row.playbook_ids.as_deref());
    let coverage = if nudge && cited.is_empty() {
        playbook::covers(conn, cat.topics(), None)
    } else {
        playbook::covers(conn, cat.topics(), Some(&cited))
    };
    let class = ReplyClass { category: Some(cat), intent: row.intent.as_deref(), their_text: if nudge { "" } else { &row.their } };
    let mut verdict = may_autosend(&class, &row.body, coverage, confidence, level);
    if !enabled(conn) {
        verdict = Err("Envois automatiques coupés.".into());
    } else if row.edited {
        verdict = Err("Brouillon modifié par Thibault.".into());
    }
    Some((cat, confidence, verdict))
}

/// After a draft is written: schedule it when it may go out alone, else note
/// why it waits. Returns the autosend id.
pub fn consider(conn: &Connection, desk_id: i64, now: NaiveDateTime) -> Option<i64> {
    cancel_for_desk(conn, desk_id, "Nouveau brouillon.");
    let row = desk_row(conn, desk_id)?;
    if row.state != "open" {
        return None;
    }
    let Some((cat, confidence, verdict)) = decide(conn, &row) else {
        let _ = conn.execute("UPDATE reply_desk SET auto_note = NULL WHERE id = ?1", params![desk_id]);
        return None;
    };
    match verdict {
        Ok(reason) => {
            let _ = conn.execute("UPDATE reply_desk SET auto_note = NULL WHERE id = ?1", params![desk_id]);
            let due = now + Duration::minutes(delay_min(conn));
            conn.execute(
                "INSERT INTO autosends (desk_id, category, reason, confidence, playbook_ids, subject, body, due_at, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![desk_id, cat.key(), reason, confidence, row.playbook_ids, row.subject, row.body, fmt_ts(due), fmt_ts(now)],
            )
            .ok()?;
            Some(conn.last_insert_rowid())
        }
        Err(why) => {
            let _ = conn.execute("UPDATE reply_desk SET auto_note = ?2 WHERE id = ?1", params![desk_id, why]);
            None
        }
    }
}

pub fn cancel(conn: &Connection, autosend_id: i64, why: &str) -> bool {
    conn.execute(
        "UPDATE autosends SET state = 'cancelled', note = ?2, done_at = datetime('now') WHERE id = ?1 AND state = 'scheduled'",
        params![autosend_id, why],
    )
    .unwrap_or(0)
        > 0
}

pub fn cancel_for_desk(conn: &Connection, desk_id: i64, why: &str) -> usize {
    conn.execute(
        "UPDATE autosends SET state = 'cancelled', note = ?2, done_at = datetime('now') WHERE desk_id = ?1 AND state = 'scheduled'",
        params![desk_id, why],
    )
    .unwrap_or(0)
}

pub fn cancel_all(conn: &Connection, why: &str) -> usize {
    conn.execute(
        "UPDATE autosends SET state = 'cancelled', note = ?1, done_at = datetime('now') WHERE state = 'scheduled'",
        params![why],
    )
    .unwrap_or(0)
}

/// Thibault sends (or ignores) an item himself: cancel what was scheduled.
/// Err when the agent is sending it this very moment.
pub fn take_over(conn: &Connection, desk_id: i64, why: &str) -> Result<(), String> {
    let sending: i64 = conn
        .query_row("SELECT COUNT(*) FROM autosends WHERE desk_id = ?1 AND state = 'sending'", params![desk_id], |r| r.get(0))
        .unwrap_or(0);
    if sending > 0 {
        return Err("Cette réponse part en ce moment en envoi automatique.".into());
    }
    cancel_for_desk(conn, desk_id, why);
    Ok(())
}

/// Local "HH:MM" of the scheduled send of a desk item, for the notification.
pub fn scheduled_hhmm(conn: &Connection, desk_id: i64) -> Option<String> {
    let due: String = conn
        .query_row("SELECT due_at FROM autosends WHERE desk_id = ?1 AND state = 'scheduled' ORDER BY id DESC LIMIT 1", params![desk_id], |r| r.get(0))
        .optional()
        .ok()
        .flatten()?;
    let d = parse_ts(&due)?;
    Some(chrono::Local.from_utc_datetime(&d).format("%H:%M").to_string())
}

// ---------------- Sending what is due ----------------

/// What the sender gets for one autonomous send.
pub struct Outgoing {
    pub contact_id: Option<i64>,
    pub campaign_id: Option<i64>,
    pub to: String,
    pub subject: String,
    pub body: String,
    pub thread: ThreadRef,
}

pub type Sender<'a> = dyn FnMut(&Outgoing) -> Result<(SendResult, Option<i64>), String> + 'a;

#[derive(Debug, Default, Serialize, PartialEq)]
pub struct DueResult {
    pub sent: usize,
    pub cancelled: usize,
    pub blocked: usize,
    pub deferred: usize,
    pub failed: usize,
}

fn finish(conn: &Connection, id: i64, state: &str, note: Option<&str>, email_id: Option<i64>) {
    let _ = conn.execute(
        "UPDATE autosends SET state = ?2, note = COALESCE(?3, note), sent_email_id = COALESCE(?4, sent_email_id), done_at = datetime('now') WHERE id = ?1",
        params![id, state, note, email_id],
    );
}

fn defer(conn: &Connection, id: i64, created: &str, now: NaiveDateTime, why: &str) -> bool {
    let too_old = parse_ts(created).map_or(false, |c| now - c > Duration::hours(GIVE_UP_HOURS));
    if too_old {
        finish(conn, id, "failed", Some(&format!("{} Laissé à Thibault.", why)), None);
        return false;
    }
    let _ = conn.execute(
        "UPDATE autosends SET state = 'scheduled', due_at = ?2, note = ?3 WHERE id = ?1",
        params![id, fmt_ts(now + Duration::minutes(DEFER_MIN)), why],
    );
    true
}

/// Send the autosends whose time has come, each re-checked first. `send` is
/// the real threaded send path in the app, a fake in tests.
pub fn process_due(conn: &Connection, now: NaiveDateTime, send: &mut Sender<'_>) -> DueResult {
    let mut res = DueResult::default();
    // A send interrupted mid-way (app quit) is left to Thibault, never retried.
    let _ = conn.execute(
        "UPDATE autosends SET state = 'failed', note = 'Envoi interrompu, à vérifier dans l''historique des emails.'
         WHERE state = 'sending' AND done_at < ?1",
        params![fmt_ts(now - Duration::minutes(10))],
    );
    // Answered from Compose, superseded by a newer reply...: closed first.
    reply_desk::auto_close(conn, now);
    let due: Vec<(i64, i64, String, String, String)> = conn
        .prepare("SELECT id, desk_id, subject, body, created_at FROM autosends WHERE state = 'scheduled' AND due_at <= ?1 ORDER BY due_at, id")
        .and_then(|mut st| {
            st.query_map(params![fmt_ts(now)], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))).map(|it| it.flatten().collect())
        })
        .unwrap_or_default();
    for (id, desk_id, subject, body, created) in due {
        let Some(row) = desk_row(conn, desk_id) else {
            cancel(conn, id, "Élément supprimé.");
            res.cancelled += 1;
            continue;
        };
        if !enabled(conn) {
            cancel(conn, id, "Envois automatiques coupés.");
            res.cancelled += 1;
            continue;
        }
        if row.state != "open" {
            cancel(conn, id, "Déjà traité.");
            res.cancelled += 1;
            continue;
        }
        if row.edited || row.body != body || row.subject != subject {
            cancel(conn, id, "Brouillon modifié par Thibault.");
            res.cancelled += 1;
            continue;
        }
        // Same checks as when it was scheduled, with today's settings.
        match decide(conn, &row) {
            Some((_, _, Ok(_))) => {}
            Some((_, _, Err(why))) => {
                finish(conn, id, "blocked", Some(&why), None);
                res.blocked += 1;
                continue;
            }
            None => {
                finish(conn, id, "blocked", Some("Catégorie repassée en « je valide tout »."), None);
                res.blocked += 1;
                continue;
            }
        }
        // Automated sends respect the sender health pause and the budget.
        let utc = Utc.from_utc_datetime(&now);
        if let Some(why) = crate::commands::deliverability::automated_block(conn, utc) {
            if defer(conn, id, &created, now, &why) {
                res.deferred += 1;
            } else {
                res.failed += 1;
            }
            continue;
        }
        // Claim it: Thibault clicking "Envoyer" now gets a clear refusal.
        let claimed = conn
            .execute("UPDATE autosends SET state = 'sending', done_at = ?2 WHERE id = ?1 AND state = 'scheduled'", params![id, fmt_ts(now)])
            .unwrap_or(0);
        if claimed == 0 {
            continue;
        }
        let (contact_id, campaign_id, to): (Option<i64>, Option<i64>, String) = conn
            .query_row("SELECT contact_id, campaign_id, to_addr FROM reply_desk WHERE id = ?1", params![desk_id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap_or((None, None, String::new()));
        let out = Outgoing { contact_id, campaign_id, to, subject: subject.clone(), body: body.clone(), thread: reply_desk::thread_ref(conn, desk_id) };
        match send(&out) {
            Ok((r, email_id)) if r.ok => {
                reply_desk::mark_sent(conn, desk_id, email_id, &subject, &body);
                finish(conn, id, "sent", None, email_id);
                res.sent += 1;
            }
            Ok((r, _)) => {
                let why = r.error.unwrap_or_else(|| "Envoi refusé.".into());
                let code = r.code.unwrap_or_default();
                if code == "quota" || code.starts_with("budget") {
                    if defer(conn, id, &created, now, &why) {
                        res.deferred += 1;
                    } else {
                        res.failed += 1;
                    }
                } else {
                    finish(conn, id, "failed", Some(&why), None);
                    res.failed += 1;
                }
            }
            Err(e) => {
                finish(conn, id, "failed", Some(&e), None);
                res.failed += 1;
            }
        }
    }
    res
}

/// Background loop: every 30 seconds, the due autosends through the real
/// threaded send path.
pub fn start(pool: crate::db::DbPool) {
    std::thread::Builder::new()
        .name("reply-autosend".into())
        .spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_secs(30));
            let p = pool.clone();
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                let Ok(conn) = p.get() else { return DueResult::default() };
                let mut send = |o: &Outgoing| {
                    crate::commands::email::send_one_in_thread(&p, o.contact_id, o.campaign_id, &o.to, &o.subject, &o.body, Some(&o.thread))
                };
                process_due(&conn, Utc::now().naive_utc(), &mut send)
            }));
            match r {
                Ok(res) if res != DueResult::default() => reply_desk::emit_changed(),
                Ok(_) => {}
                Err(_) => eprintln!("[autonomy] pass panicked"),
            }
        })
        .ok();
}

// ---------------- Overview (inbox + settings) ----------------

#[derive(Debug, Serialize, Clone)]
pub struct AutoRow {
    pub id: i64,
    pub desk_id: i64,
    pub contact: String,
    pub category: String,
    pub reason: String,
    pub state: String,
    pub due_at: String,
    pub done_at: Option<String>,
    pub note: Option<String>,
    pub subject: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct PlaybookRef {
    pub id: i64,
    pub topic: String,
    pub title: String,
}

/// Per open desk item: what the agent knows about its autonomy.
#[derive(Debug, Serialize, Clone)]
pub struct DeskAuto {
    pub desk_id: i64,
    pub category: Option<String>,
    pub confidence: Option<f64>,
    /// Why it waits for Thibault although its category is on autopilot.
    pub note: Option<String>,
    pub playbook: Vec<PlaybookRef>,
    /// Scheduled autonomous send, if any.
    pub autosend_id: Option<i64>,
    pub due_at: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Overview {
    pub settings: Settings,
    pub items: Vec<DeskAuto>,
    pub history: Vec<AutoRow>,
}

pub fn history(conn: &Connection, limit: i64) -> Vec<AutoRow> {
    conn.prepare(
        "SELECT a.id, a.desk_id, COALESCE(NULLIF(k.venue,''), k.name, d.to_addr, ''), a.category, a.reason, a.state, a.due_at,
                a.done_at, a.note, a.subject
         FROM autosends a LEFT JOIN reply_desk d ON d.id = a.desk_id LEFT JOIN contacts k ON k.id = d.contact_id
         ORDER BY a.id DESC LIMIT ?1",
    )
    .and_then(|mut st| {
        st.query_map(params![limit], |r| {
            Ok(AutoRow {
                id: r.get(0)?,
                desk_id: r.get(1)?,
                contact: r.get(2)?,
                category: r.get(3)?,
                reason: r.get(4)?,
                state: r.get(5)?,
                due_at: r.get(6)?,
                done_at: r.get(7)?,
                note: r.get(8)?,
                subject: r.get(9)?,
            })
        })
        .map(|it| it.flatten().collect())
    })
    .unwrap_or_default()
}

pub fn overview(conn: &Connection) -> Overview {
    let entries = playbook::list(conn);
    let items: Vec<DeskAuto> = conn
        .prepare(
            "SELECT d.id, d.auto_category, d.auto_confidence, d.auto_note, d.playbook_ids, a.id, a.due_at
             FROM reply_desk d LEFT JOIN autosends a ON a.desk_id = d.id AND a.state IN ('scheduled', 'sending')
             WHERE d.state = 'open'",
        )
        .and_then(|mut st| {
            st.query_map([], |r| {
                let ids = playbook::parse_ids(r.get::<_, Option<String>>(4)?.as_deref());
                Ok(DeskAuto {
                    desk_id: r.get(0)?,
                    category: r.get(1)?,
                    confidence: r.get(2)?,
                    note: r.get(3)?,
                    playbook: ids
                        .iter()
                        .filter_map(|i| entries.iter().find(|e| e.id == *i))
                        .map(|e| PlaybookRef { id: e.id, topic: e.topic.clone(), title: e.title.clone() })
                        .collect(),
                    autosend_id: r.get(5)?,
                    due_at: r.get(6)?,
                })
            })
            .map(|it| it.flatten().collect())
        })
        .unwrap_or_default();
    Overview { settings: load(conn), items, history: history(conn, 30) }
}

// ---------------- Commands ----------------

fn conn(state: &State<'_, AppState>) -> Result<r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>, String> {
    state.pool.get().map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub fn autonomy_overview(state: State<'_, AppState>) -> Result<Overview, String> {
    Ok(overview(&*conn(&state)?))
}

#[tauri::command(async)]
pub fn autonomy_save(state: State<'_, AppState>, settings: Settings) -> Result<Settings, String> {
    let c = conn(&state)?;
    save(&c, &settings);
    Ok(load(&c))
}

/// The one-click switch: every scheduled send is cancelled, nothing leaves alone.
#[tauri::command(async)]
pub fn autonomy_stop_all(state: State<'_, AppState>) -> Result<usize, String> {
    let c = conn(&state)?;
    put(&c, "autosend_paused", "1");
    Ok(cancel_all(&c, "Envois automatiques coupés."))
}

#[tauri::command(async)]
pub fn autosend_cancel(state: State<'_, AppState>, id: i64) -> Result<bool, String> {
    Ok(cancel(&*conn(&state)?, id, "Annulé par Thibault."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::residency::testdb;

    fn class(cat: Category) -> ReplyClass<'static> {
        ReplyClass { category: Some(cat), intent: Some("question"), their_text: "Vous avez un mix à nous envoyer ?" }
    }

    const MIX_DRAFT: &str = "Hello,\n\nAvec plaisir, voilà le dernier mix, le Vol. 4 :\nhttps://soundcloud.com/piers/vol-4-2026\n\nÀ bientôt,\nThibault";

    #[test]
    fn may_autosend_accepts_the_low_risk_case() {
        let ok = may_autosend(&class(Category::MusicRequest), MIX_DRAFT, true, 0.92, 1).unwrap();
        assert!(ok.contains("mix") && ok.contains("92"));
        // Call slots with moments of the week and a time of day are fine.
        let call = "Hello,\n\nOn s'appelle jeudi vers 15h ou vendredi en fin de matinée ?\n\nÀ bientôt,\nThibault";
        assert!(may_autosend(&class(Category::CallSlots), call, true, 0.9, 1).is_ok());
    }

    #[test]
    fn may_autosend_rejects_everything_else() {
        let c = class(Category::MusicRequest);
        // Level 0, no category, low confidence, no playbook coverage.
        assert!(may_autosend(&c, MIX_DRAFT, true, 0.95, 0).unwrap_err().contains("je valide tout"));
        assert!(may_autosend(&ReplyClass { category: None, ..c.clone() }, MIX_DRAFT, true, 0.95, 1).is_err());
        assert!(may_autosend(&c, MIX_DRAFT, true, 0.5, 1).unwrap_err().contains("Confiance"));
        assert!(may_autosend(&c, MIX_DRAFT, true, f64::NAN, 1).is_err());
        assert!(may_autosend(&c, MIX_DRAFT, false, 0.95, 1).unwrap_err().contains("bibliothèque"));
        assert!(may_autosend(&c, "  ", true, 0.95, 1).is_err());
        // Money, figures, dates, holds, contracts, exclusivity, deposit, fee.
        for bad in [
            "Mon cachet est de 800 €.",
            "Je demande 1200 euros.",
            "Libre le 14 novembre.",
            "Libre le 14/11 si besoin.",
            "Dispo en mars pour une date.",
            "Je peux poser une option sur ce soir-là.",
            "Je vous envoie le contrat.",
            "Pas d'exclusivité de mon côté.",
            "Un acompte à la signature.",
            "My fee is flexible.",
            "We can put a hold on it.",
            "Free on the weekend of the 21st.",
            "Le tarif dépend du format.",
            "Malheureusement je ne suis pas dispo.",
            "Exclusive rights for the city.",
        ] {
            let r = may_autosend(&c, &format!("Hello,\n\n{}\n\nThibault", bad), true, 0.99, 1);
            assert!(r.is_err(), "should block: {}", bad);
        }
        // Their message mentioning money or a date keeps it for Thibault too.
        let money = ReplyClass { their_text: "Envoyez un mix et votre cachet svp", ..c.clone() };
        assert!(may_autosend(&money, MIX_DRAFT, true, 0.99, 1).unwrap_err().contains("Leur message"));
        let refusal = ReplyClass { their_text: "Merci mais nous allons passer notre tour.", ..c.clone() };
        assert!(may_autosend(&refusal, MIX_DRAFT, true, 0.99, 1).is_err());
        let neg = ReplyClass { intent: Some("not_interested"), ..c.clone() };
        assert!(may_autosend(&neg, MIX_DRAFT, true, 0.99, 1).is_err());
        // Digits inside the link are not a date; a bare one is.
        assert_eq!(risky_term("https://sc.com/2026/11/14/mix"), None);
        assert_eq!(risky_term("rdv 10:30 ou 9h"), None);
        assert_eq!(risky_term("le 7"), Some("chiffre ou date"));
        assert_eq!(risky_term("Update on our side"), None, "whole words only");
    }

    fn base() -> Connection {
        let c = testdb::conn();
        c.execute_batch(
            "INSERT INTO artists (id, name, mix_url) VALUES (1, 'Piers', 'https://sc.com/m');
             INSERT INTO contacts (id, name, venue, email, status) VALUES (10, 'Club A', 'Club A', 'booking@cluba.fr', 'in_discussion');
             INSERT INTO emails (id, contact_id, to_addr, subject, body, status, sent_at, message_id) VALUES
               (1, 10, 'booking@cluba.fr', 'Piers x Club A', 'Hello, ...', 'sent', '2026-10-20 10:00:00', '<s1@insrt.fr>');
             INSERT INTO email_replies (id, email_id, contact_id, from_addr, subject, message_id, kind, received_at, body_excerpt) VALUES
               (1, 1, 10, 'booking@cluba.fr', 'Re: Piers x Club A', '<r1@cluba.fr>', 'reply', '2026-10-30 10:30:00',
                'Bonjour, vous auriez un mix à nous faire écouter ?');",
        )
        .unwrap();
        c
    }

    fn t(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, TS).unwrap()
    }

    fn mix_entry(c: &Connection) -> i64 {
        playbook::save(
            c,
            &playbook::Entry {
                kind: "objection".into(),
                topic: "obj_send_mix".into(),
                title: "Envoyez-nous un mix".into(),
                text_fr: "Avec plaisir, voilà le dernier.".into(),
                status: "approved".into(),
                ..Default::default()
            },
        )
        .unwrap()
    }

    fn answer(id: i64) -> String {
        format!(
            r#"{{"intent":"question","body":"Hello,\n\nAvec plaisir, voilà le dernier mix :\nhttps://sc.com/m\n\nÀ bientôt,\nThibault","category":"music_request","confidence":0.93,"playbook_used":[{}]}}"#,
            id
        )
    }

    fn ok_send(sent: &mut Vec<String>) -> impl FnMut(&Outgoing) -> Result<(SendResult, Option<i64>), String> + '_ {
        move |o: &Outgoing| {
            sent.push(format!("{}|{}|{:?}", o.to, o.subject, o.thread.in_reply_to));
            Ok((SendResult { ok: true, error: None, tracked: false, code: None }, None))
        }
    }

    #[test]
    fn level_one_schedules_notifies_and_sends_after_the_delay() {
        let c = base();
        let pid = mix_entry(&c);
        c.execute("INSERT INTO settings (key, value) VALUES ('autonomy_music_request', '1')", []).unwrap();
        let now = t("2026-10-30 10:40:00");
        let llm = crate::commands::residency::claude::fake::FakeLlm::new(&[&answer(pid)]);
        let r = reply_desk::run_pass(&c, Some(&llm), now, now.date());
        // The approved entry went into the cached system block; the user
        // message asks for risk-free wording.
        let seen = llm.seen.lock().unwrap();
        assert_eq!(seen[0].system.len(), 2);
        assert!(!seen[0].system[0].cache && seen[0].system[1].cache);
        assert!(seen[0].system[1].text.contains(&format!("[P{}]", pid)));
        assert!(seen[0].user.contains("Autonomy"));
        drop(seen);
        // The notification says when it leaves.
        assert_eq!(r.notify.len(), 1);
        assert!(r.notify[0].2.contains("Partira seule à"), "{}", r.notify[0].2);
        let (desk_id, used): (i64, String) =
            c.query_row("SELECT id, playbook_ids FROM reply_desk", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!(used, format!("[{}]", pid));
        let (state, due): (String, String) = c.query_row("SELECT state, due_at FROM autosends", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!((state.as_str(), due.as_str()), ("scheduled", "2026-10-30 10:55:00"));
        let ov = overview(&c);
        assert_eq!(ov.items[0].due_at.as_deref(), Some("2026-10-30 10:55:00"));
        assert_eq!(ov.items[0].playbook[0].id, pid);

        // Not yet due: nothing happens.
        let mut sent = vec![];
        assert_eq!(process_due(&c, t("2026-10-30 10:50:00"), &mut ok_send(&mut sent)), DueResult::default());
        assert!(sent.is_empty());
        // Due: sent in the thread, the item closes, the log keeps the reason.
        let res = process_due(&c, t("2026-10-30 10:56:00"), &mut ok_send(&mut sent));
        assert_eq!(res.sent, 1);
        assert_eq!(sent, vec!["booking@cluba.fr|Re: Piers x Club A|Some(\"<r1@cluba.fr>\")".to_string()]);
        let st: String = c.query_row("SELECT state FROM reply_desk WHERE id = ?1", params![desk_id], |r| r.get(0)).unwrap();
        assert_eq!(st, "sent");
        let h = history(&c, 10);
        assert_eq!(h[0].state, "sent");
        assert!(h[0].reason.contains("mix"));
        // Never twice.
        assert_eq!(process_due(&c, t("2026-10-30 11:30:00"), &mut ok_send(&mut sent)).sent, 0);
    }

    fn scheduled(c: &Connection) -> (i64, i64) {
        let pid = mix_entry(c);
        c.execute("INSERT INTO settings (key, value) VALUES ('autonomy_music_request', '1')", []).unwrap();
        let llm = crate::commands::residency::claude::fake::FakeLlm::new(&[&answer(pid)]);
        reply_desk::run_pass(c, Some(&llm), t("2026-10-30 10:40:00"), t("2026-10-30 10:40:00").date());
        c.query_row("SELECT id, desk_id FROM autosends WHERE state = 'scheduled'", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap()
    }

    #[test]
    fn cancel_edit_stop_and_take_over() {
        // Cancelled from the inbox.
        let c = base();
        let (aid, desk) = scheduled(&c);
        assert!(cancel(&c, aid, "Annulé par Thibault."));
        let mut sent = vec![];
        assert_eq!(process_due(&c, t("2026-10-30 12:00:00"), &mut ok_send(&mut sent)).sent, 0);
        assert!(sent.is_empty());
        let st: String = c.query_row("SELECT state FROM reply_desk WHERE id = ?1", params![desk], |r| r.get(0)).unwrap();
        assert_eq!(st, "open", "back to Thibault");

        // Edited by Thibault: cancelled when due.
        let c = base();
        let (_, desk) = scheduled(&c);
        c.execute("UPDATE reply_desk SET body = body || ' Bises', edited = 1 WHERE id = ?1", params![desk]).unwrap();
        let r = process_due(&c, t("2026-10-30 12:00:00"), &mut ok_send(&mut sent));
        assert_eq!((r.sent, r.cancelled), (0, 1));

        // Global switch off: everything scheduled is cancelled at once.
        let c = base();
        scheduled(&c);
        let mut s = load(&c);
        s.enabled = false;
        save(&c, &s);
        assert_eq!(c.query_row("SELECT COUNT(*) FROM autosends WHERE state = 'scheduled'", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
        assert_eq!(process_due(&c, t("2026-10-30 12:00:00"), &mut ok_send(&mut sent)).sent, 0);

        // Category lowered back to level 0 before the send: blocked.
        let c = base();
        scheduled(&c);
        c.execute("UPDATE settings SET value = '0' WHERE key = 'autonomy_music_request'", []).unwrap();
        assert_eq!(process_due(&c, t("2026-10-30 12:00:00"), &mut ok_send(&mut sent)).blocked, 1);

        // Thibault sends himself: the autosend is cancelled; while the agent
        // is sending, his click is refused.
        let c = base();
        let (aid, desk) = scheduled(&c);
        take_over(&c, desk, "Envoyé par Thibault.").unwrap();
        let st: String = c.query_row("SELECT state FROM autosends WHERE id = ?1", params![aid], |r| r.get(0)).unwrap();
        assert_eq!(st, "cancelled");
        c.execute("UPDATE autosends SET state = 'sending' WHERE id = ?1", params![aid]).unwrap();
        assert!(take_over(&c, desk, "x").is_err());
        assert!(sent.is_empty());
    }

    #[test]
    fn budget_defers_and_risky_drafts_wait() {
        let c = base();
        let (aid, _) = scheduled(&c);
        let mut quota = |_: &Outgoing| Ok::<_, String>((SendResult { ok: false, error: Some("Quota journalier atteint".into()), tracked: false, code: Some("quota".into()) }, None));
        let r = process_due(&c, t("2026-10-30 11:00:00"), &mut quota);
        assert_eq!(r.deferred, 1);
        let (st, due): (String, String) = c.query_row("SELECT state, due_at FROM autosends WHERE id = ?1", params![aid], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!((st.as_str(), due.as_str()), ("scheduled", "2026-10-30 11:30:00"));
        // Still blocked a day later: given up, left to Thibault.
        assert_eq!(process_due(&c, t("2026-10-31 12:00:00"), &mut quota).failed, 1);
        let st: String = c.query_row("SELECT state FROM autosends WHERE id = ?1", params![aid], |r| r.get(0)).unwrap();
        assert_eq!(st, "failed");

        // A draft quoting a fee never gets scheduled; the item says why.
        let c = base();
        let pid = mix_entry(&c);
        c.execute("INSERT INTO settings (key, value) VALUES ('autonomy_music_request', '1')", []).unwrap();
        let risky = answer(pid).replace("voilà le dernier mix", "mon cachet est de 800 €, voilà le mix");
        let llm = crate::commands::residency::claude::fake::FakeLlm::new(&[&risky]);
        let r = reply_desk::run_pass(&c, Some(&llm), t("2026-10-30 10:40:00"), t("2026-10-30 10:40:00").date());
        assert!(!r.notify[0].2.contains("Partira"));
        assert_eq!(c.query_row("SELECT COUNT(*) FROM autosends", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
        let note: String = c.query_row("SELECT auto_note FROM reply_desk", [], |r| r.get(0)).unwrap();
        assert!(note.contains("montant"), "{}", note);

        // Level 0 (default): nothing scheduled, no note either.
        let c = base();
        let pid = mix_entry(&c);
        let llm = crate::commands::residency::claude::fake::FakeLlm::new(&[&answer(pid)]);
        reply_desk::run_pass(&c, Some(&llm), t("2026-10-30 10:40:00"), t("2026-10-30 10:40:00").date());
        assert_eq!(c.query_row("SELECT COUNT(*) FROM autosends", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
        assert!(!llm.seen.lock().unwrap()[0].user.contains("Autonomy"));
    }
}
