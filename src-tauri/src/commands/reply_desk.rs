//! "Réponses à traiter": answer an interested programmer within the hour.
//!
//! Each pass (residency engine tick, every 5 minutes, and after each reply
//! sync) looks at what came in and what went quiet:
//! - a reply classified interested / question (residency replies, by
//!   residency/replies.rs) or, for any other reply, judged so by Claude while
//!   drafting (rules first: refusals, out-of-office and unsubscribes never
//!   cost a call) becomes a `reply` item with a draft answer;
//! - a conversation "in discussion" whose last message is Thibault's and has
//!   gone unanswered for N days (setting `desk_nudge_days`, default 4) becomes
//!   a `nudge` item with a light follow-up draft;
//! - a programmer's message left unanswered for more than 24 hours shows red.
//!
//! Drafts are written in Thibault's voice (house rules shared with the first
//! email, residency/content.rs) with the facts the app knows: the artist's fee
//! range, tech rider and mix link, free weekends of the next three months from
//! the Agenda, the contact's personal insrt EPK link. Haiku by default, system
//! prompt cached, cost recorded in `ai_usage`. Without an API key, a short
//! template. Nothing is ever sent automatically: Thibault edits and clicks
//! "Envoyer", which goes through email::send_one_in_thread (guardrails,
//! In-Reply-To / References, logged in `emails`).
//!
//! A new reply item triggers a native notification (setting `desk_notify`,
//! "0" turns it off). Clicking it brings the app forward; the frontend then
//! opens the item (ReplyDeskWatcher).

use chrono::{Datelike, Duration, Months, NaiveDate, NaiveDateTime, Utc, Weekday};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::{json, Value};
use std::sync::OnceLock;
use tauri::State;

use crate::commands::email::{self, SendResult, ThreadRef};
use crate::commands::residency::claude::{self, Llm, LlmRequest, SystemBlock};
use crate::commands::residency::content::HOUSE_VOICE;
use crate::commands::residency::replies::{self, Pending};
use crate::commands::status;
use crate::AppState;

pub const DEFAULT_NUDGE_DAYS: i64 = 4;
/// A programmer's message unanswered for longer than this shows red.
pub const OVERDUE_HOURS: i64 = 24;
/// Replies older than this are history, not something to answer now.
pub const LOOKBACK_DAYS: i64 = 14;
/// Drafts written per pass (a first run on a full inbox stays cheap).
pub const MAX_DRAFTS_PER_PASS: usize = 10;
/// Only replies this recent trigger a notification (no flood on first run).
pub const NOTIFY_WITHIN_HOURS: i64 = 24;
/// A residency reply waits this long for its own classification first.
pub const RESIDENCY_WAIT_MIN: i64 = 60;
/// Weeks of availability offered in drafts.
pub const AVAILABILITY_MONTHS: u32 = 3;

const TS: &str = "%Y-%m-%d %H:%M:%S";

pub fn migrate(conn: &Connection) {
    for stmt in [
        "CREATE TABLE IF NOT EXISTS reply_desk (
            id             INTEGER PRIMARY KEY AUTOINCREMENT,
            thread_key     TEXT NOT NULL UNIQUE,
            kind           TEXT NOT NULL,
            contact_id     INTEGER REFERENCES contacts(id) ON DELETE CASCADE,
            campaign_id    INTEGER REFERENCES campaigns(id) ON DELETE SET NULL,
            email_reply_id INTEGER REFERENCES email_replies(id) ON DELETE CASCADE,
            email_id       INTEGER REFERENCES emails(id) ON DELETE SET NULL,
            to_addr        TEXT NOT NULL,
            intent         TEXT,
            lang           TEXT NOT NULL DEFAULT 'fr',
            subject        TEXT NOT NULL DEFAULT '',
            body           TEXT NOT NULL DEFAULT '',
            draft_by       TEXT,
            edited         INTEGER NOT NULL DEFAULT 0,
            state          TEXT NOT NULL DEFAULT 'pending',
            snooze_until   TEXT,
            notified_at    TEXT,
            sent_email_id  INTEGER,
            since          TEXT NOT NULL,
            created_at     TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at     TEXT NOT NULL DEFAULT (datetime('now'))
        )",
        "CREATE INDEX IF NOT EXISTS idx_reply_desk_state ON reply_desk(state)",
        "CREATE INDEX IF NOT EXISTS idx_reply_desk_contact ON reply_desk(contact_id)",
    ] {
        let _ = conn.execute(stmt, []);
    }
}

// ---------------- Pure logic ----------------

/// Who holds the ball in a conversation.
#[derive(Debug, Clone, PartialEq)]
pub enum ThreadState {
    /// The programmer wrote last: Thibault owes an answer. `overdue` past 24h.
    AwaitingUs { since: NaiveDateTime, overdue: bool },
    /// Thibault wrote last. `stalled` once the nudge delay has passed.
    AwaitingThem { since: NaiveDateTime, stalled: bool },
    Idle,
}

pub fn thread_state(
    last_in: Option<NaiveDateTime>,
    last_out: Option<NaiveDateTime>,
    now: NaiveDateTime,
    nudge_days: i64,
) -> ThreadState {
    match (last_in, last_out) {
        (Some(i), o) if o.map_or(true, |o| i > o) => {
            ThreadState::AwaitingUs { since: i, overdue: now - i > Duration::hours(OVERDUE_HOURS) }
        }
        (_, Some(o)) => ThreadState::AwaitingThem { since: o, stalled: now - o >= Duration::days(nudge_days.max(1)) },
        _ => ThreadState::Idle,
    }
}

/// French or English, from the words of a message. None when it cannot tell.
pub fn detect_lang(text: &str) -> Option<&'static str> {
    const FR: &[&str] = &[
        "le", "la", "les", "et", "est", "pour", "vous", "nous", "je", "merci", "bonjour", "une", "des", "avec",
        "pas", "dans", "sur", "soiree", "cachet", "quel", "quelle", "votre", "notre", "seriez", "dispo",
    ];
    const EN: &[&str] = &[
        "the", "and", "is", "for", "you", "we", "thanks", "thank", "hi", "with", "not", "are", "this", "would",
        "could", "gig", "fee", "night", "what", "your", "our",
    ];
    let words: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(crate::db::normalise)
        .collect();
    let count = |ws: &[&str]| words.iter().filter(|w| ws.contains(&w.as_str())).count();
    let (fr, en) = (count(FR), count(EN));
    if fr == 0 && en == 0 {
        None
    } else if fr >= en {
        Some("fr")
    } else {
        Some("en")
    }
}

/// Saturdays of the weekends (Friday + Saturday) with nothing booked, from
/// `today` up to `until` included.
pub fn free_weekends(busy: &[NaiveDate], today: NaiveDate, until: NaiveDate) -> Vec<NaiveDate> {
    let mut sat = today;
    while sat.weekday() != Weekday::Sat {
        sat += Duration::days(1);
    }
    let mut out = Vec::new();
    while sat <= until {
        let fri = sat - Duration::days(1);
        if !busy.contains(&sat) && !busy.contains(&fri) {
            out.push(sat);
        }
        sat += Duration::days(7);
    }
    out
}

const FR_MONTHS: [&str; 12] =
    ["janv.", "févr.", "mars", "avr.", "mai", "juin", "juil.", "août", "sept.", "oct.", "nov.", "déc."];
const EN_MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

pub fn human_day(d: NaiveDate, lang: &str) -> String {
    let m = d.month0() as usize;
    if lang == "en" {
        format!("{} {}", EN_MONTHS[m], d.day())
    } else {
        format!("{} {}", d.day(), FR_MONTHS[m])
    }
}

/// "14 nov., 21 nov. et 5 déc." / "Nov 14, Nov 21 and Dec 5".
pub fn human_list(days: &[NaiveDate], lang: &str) -> String {
    let parts: Vec<String> = days.iter().map(|d| human_day(*d, lang)).collect();
    match parts.len() {
        0 => String::new(),
        1 => parts[0].clone(),
        n => format!("{} {} {}", parts[..n - 1].join(", "), if lang == "en" { "and" } else { "et" }, parts[n - 1]),
    }
}

/// What a draft may rely on. Every field is optional: what is unknown is
/// simply not offered (never invented).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Facts {
    pub artist: String,
    pub fee_range: Option<String>,
    pub tech_rider: Option<String>,
    pub mix_url: Option<String>,
    pub epk_url: Option<String>,
    pub free_saturdays: Vec<NaiveDate>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    Reply,
    Nudge,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Reply => "reply",
            Kind::Nudge => "nudge",
        }
    }
    fn parse(s: &str) -> Kind {
        if s == "nudge" {
            Kind::Nudge
        } else {
            Kind::Reply
        }
    }
}

/// Everything a draft is written from.
#[derive(Debug, Clone, Default)]
pub struct DraftInput {
    pub kind: Option<Kind>,
    pub lang: String,
    pub today: Option<NaiveDate>,
    pub contact: String,
    pub venue: String,
    pub from_addr: String,
    pub ours: Option<Message>,
    pub theirs: Option<Message>,
    pub facts: Facts,
}

#[derive(Debug, Clone, Default)]
pub struct Message {
    pub at: String,
    pub subject: String,
    pub body: String,
}

pub const DESK_INTENTS: &[&str] = &["interested", "question", "not_interested", "other"];

/// The cached system prompt (static text: identical across calls, so the
/// prefix cache holds).
pub fn reply_system() -> String {
    format!(
        "You draft the email that Thibault, a DJ/producer who plays under his artist name, sends to a club programmer \
or promoter in an ongoing booking conversation. Thibault reads and edits it before sending, so it must be ready to \
send as is.

Two cases:
- reply: the programmer answered. Answer what they asked or offered using only the facts given (fee range, technical \
rider, mix link, free weekends, press link) and move one concrete step toward a date or a call.
- nudge: Thibault's last message got no answer for a few days while the conversation was going. Write a short, light \
follow-up (40 to 80 words) that makes answering easy, optionally offering one or two free weekends.

Voice: warm, direct, a bit casual, confident without selling, as if typed between two studio sessions. Keep the \
formality of the thread (tu or vous as they wrote; vous by default in French).

Rules:
- 50 to 130 words for a reply. Plain text, short paragraphs, no markdown, no emojis, no lists, no subject line.
- Open with \"Hello,\" (\"Hello <first name>,\" when the programmer signed with a first name); \"Bonjour\" also works in French.
- Close with \"À bientôt,\" or \"Cheers,\" then \"Thibault\" on its own line. The full signature is appended automatically, never write it.
- Never invent a fact, a price, a date, a name or a number. Quote the fee range only as given and only if they asked \
about money or conditions. If they ask something the facts do not answer, say Thibault comes back to them on that point today.
- Dates: offer at most three of the free weekends, the closest to anything they mentioned.
- Links: one or two at most, copied verbatim, each on its own line. Prefer the personal press link when there is one.
- Write in the language requested (the language of the thread).
{}
Also classify the programmer's last message (for a nudge, answer interested): interested (open to a date or a talk), \
question (asks for information before deciding: fee, dates, rider, references, mix), not_interested (declines), \
other (out of office, unrelated, newsletter, anything else).

Answer with JSON only: {{\"intent\": \"...\", \"body\": \"...\"}}.",
        HOUSE_VOICE
    )
}

fn answer_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "intent": {"type": "string", "enum": DESK_INTENTS},
            "body": {"type": "string"}
        },
        "required": ["intent", "body"],
        "additionalProperties": false
    })
}

fn clip(s: &str, n: usize) -> String {
    let t = s.trim();
    if t.chars().count() <= n {
        t.to_string()
    } else {
        format!("{}…", t.chars().take(n).collect::<String>())
    }
}

/// The per-item user message: case, language, facts, the two last messages.
pub fn build_user(input: &DraftInput) -> String {
    let kind = input.kind.unwrap_or(Kind::Reply);
    let lang = if input.lang == "en" { "English" } else { "French" };
    let f = &input.facts;
    let mut s = format!(
        "Case: {}\nLanguage: {}\nToday: {}\nArtist name: {}\nProgrammer: {}{} <{}>\n\nFacts:\n",
        kind.as_str(),
        lang,
        input.today.map(|d| d.to_string()).unwrap_or_default(),
        if f.artist.is_empty() { "unknown" } else { &f.artist },
        input.contact,
        if input.venue.trim().is_empty() || input.venue == input.contact { String::new() } else { format!(" ({})", input.venue) },
        input.from_addr
    );
    let mut fact = |k: &str, v: &Option<String>| {
        if let Some(v) = v.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
            s.push_str(&format!("- {}: {}\n", k, clip(v, 400)));
        }
    };
    fact("Fee range", &f.fee_range);
    fact("Technical rider", &f.tech_rider);
    fact("Latest mix", &f.mix_url);
    fact("Personal press link (insrt EPK)", &f.epk_url);
    if f.free_saturdays.is_empty() {
        s.push_str("- Free weekends: unknown, do not offer dates\n");
    } else {
        let days: Vec<String> = f
            .free_saturdays
            .iter()
            .take(8)
            .map(|d| format!("{} ({})", d, human_day(*d, &input.lang)))
            .collect();
        s.push_str(&format!("- Free weekends (Saturday dates, next {} months): {}\n", AVAILABILITY_MONTHS, days.join(", ")));
    }
    if let Some(m) = &input.ours {
        s.push_str(&format!("\nThibault's last email ({}):\nSubject: {}\n{}\n", m.at, m.subject, clip(&m.body, 1200)));
    }
    if let Some(m) = &input.theirs {
        let body = if m.body.trim().is_empty() { "(no text, subject only)".to_string() } else { clip(&m.body, 2500) };
        s.push_str(&format!("\nTheir message ({}):\nSubject: {}\n{}\n", m.at, m.subject, body));
    }
    s
}

pub fn build_request(input: &DraftInput, model: &str) -> LlmRequest {
    LlmRequest {
        model: model.to_string(),
        max_tokens: 500,
        system: vec![SystemBlock { text: reply_system(), cache: true }],
        user: build_user(input),
        schema: Some(answer_schema()),
    }
}

/// (intent, body) from the model's answer.
pub fn parse_answer(text: &str) -> Option<(String, String)> {
    let v = claude::extract_json(text)?;
    let intent = v.get("intent")?.as_str()?.trim().to_lowercase();
    let body = claude::no_dashes(v.get("body")?.as_str()?.trim());
    (DESK_INTENTS.contains(&intent.as_str()) && !body.is_empty()).then_some((intent, body))
}

/// The short answer used without an API key (or when Claude fails): honest,
/// only the facts the app has, signed Thibault.
pub fn template_draft(input: &DraftInput) -> String {
    let en = input.lang == "en";
    let f = &input.facts;
    let days: Vec<NaiveDate> = f.free_saturdays.iter().take(3).copied().collect();
    let link = f.epk_url.clone().or_else(|| f.mix_url.clone()).filter(|l| !l.trim().is_empty());
    let mut parts: Vec<String> = vec!["Hello,".into()];
    match (input.kind.unwrap_or(Kind::Reply), en) {
        (Kind::Reply, false) => parts.push("Merci pour votre retour, ça me fait vraiment plaisir.".into()),
        (Kind::Reply, true) => parts.push("Thanks a lot for getting back to me, really glad to hear from you.".into()),
        (Kind::Nudge, false) => parts.push("Je reviens vers vous suite à mon dernier message, est-ce que vous avez pu en parler de votre côté ?".into()),
        (Kind::Nudge, true) => parts.push("Just following up on my last message, have you had a chance to talk it over?".into()),
    }
    if !days.is_empty() {
        let l = human_list(&days, &input.lang);
        parts.push(if en {
            format!("On my side I'm free on the weekends of {}.", l)
        } else if days.len() == 1 {
            format!("De mon côté je suis libre le week-end du {}.", l)
        } else {
            format!("De mon côté je suis libre les week-ends du {}.", l)
        });
    }
    if let Some(l) = link {
        parts.push(if en { format!("Everything is here if useful:\n{}", l.trim()) } else { format!("Tout est là si besoin :\n{}", l.trim()) });
    }
    parts.push(if en { "Shall we have a quick call this week?".into() } else { "On s'appelle cette semaine pour en parler ?".into() });
    parts.push(if en { "Cheers,\nThibault".into() } else { "À bientôt,\nThibault".into() });
    parts.join("\n\n")
}

pub fn re_subject(s: &str) -> String {
    let t = s.trim();
    let l = t.to_lowercase();
    if l.starts_with("re:") || l.starts_with("re :") || l.starts_with("aw:") {
        t.to_string()
    } else if t.is_empty() {
        "Re:".into()
    } else {
        format!("Re: {}", t)
    }
}

// ---------------- Reading the DB ----------------

fn ts(s: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(s.get(..19).unwrap_or(s), TS).ok()
}

fn fmt_ts(d: NaiveDateTime) -> String {
    d.format(TS).to_string()
}

fn setting(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM settings WHERE key = ?1", params![key], |r| r.get::<_, Option<String>>(0))
        .ok()
        .flatten()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub fn nudge_days(conn: &Connection) -> i64 {
    setting(conn, "desk_nudge_days").and_then(|v| v.parse().ok()).filter(|n: &i64| *n >= 1).unwrap_or(DEFAULT_NUDGE_DAYS)
}

pub fn notify_enabled(conn: &Connection) -> bool {
    setting(conn, "desk_notify").as_deref() != Some("0")
}

/// The artist this conversation is about: the contact's, the campaign's, else
/// the only (or first) artist of the roster.
fn artist_for(conn: &Connection, contact_id: Option<i64>, campaign_id: Option<i64>) -> Option<(i64, Facts)> {
    let artist_of = |sql: &str, id: i64| -> Option<i64> {
        conn.query_row(sql, params![id], |r| r.get::<_, Option<i64>>(0)).ok().flatten()
    };
    let id: Option<i64> = contact_id
        .and_then(|c| artist_of("SELECT artist_id FROM contacts WHERE id = ?1", c))
        .or_else(|| campaign_id.and_then(|c| artist_of("SELECT artist_id FROM campaigns WHERE id = ?1", c)))
        .or_else(|| conn.query_row("SELECT id FROM artists ORDER BY id LIMIT 1", [], |r| r.get(0)).ok());
    let id = id?;
    conn.query_row(
        "SELECT name, fee_range, tech_rider, mix_url FROM artists WHERE id = ?1",
        params![id],
        |r| {
            Ok(Facts {
                artist: r.get(0)?,
                fee_range: r.get(1)?,
                tech_rider: r.get(2)?,
                mix_url: r.get(3)?,
                ..Default::default()
            })
        },
    )
    .ok()
    .map(|f| (id, f))
}

/// Days the artist is booked or on hold (Agenda events), between two dates.
fn busy_dates(conn: &Connection, artist_id: i64, from: NaiveDate, until: NaiveDate) -> Vec<NaiveDate> {
    let Ok(mut st) = conn.prepare(
        "SELECT substr(date, 1, 10) FROM events
         WHERE (artist_id = ?1 OR artist_id IS NULL) AND status <> 'cancelled'
           AND substr(date, 1, 10) BETWEEN ?2 AND ?3",
    ) else {
        return Vec::new();
    };
    st.query_map(params![artist_id, from.to_string(), until.to_string()], |r| r.get::<_, String>(0))
        .map(|it| it.flatten().filter_map(|d| NaiveDate::parse_from_str(&d, "%Y-%m-%d").ok()).collect())
        .unwrap_or_default()
}

/// The contact's personal insrt EPK link (the latest one created for them).
fn epk_for(conn: &Connection, contact_id: i64) -> Option<String> {
    conn.query_row(
        "SELECT press_url FROM campaign_messages WHERE contact_id = ?1 AND press_url IS NOT NULL AND trim(press_url) <> ''
         ORDER BY updated_at DESC, id DESC LIMIT 1",
        params![contact_id],
        |r| r.get(0),
    )
    .ok()
}

pub fn facts_for(conn: &Connection, contact_id: Option<i64>, campaign_id: Option<i64>, today: NaiveDate) -> Facts {
    let mut facts = Facts::default();
    if let Some((aid, f)) = artist_for(conn, contact_id, campaign_id) {
        let until = today.checked_add_months(Months::new(AVAILABILITY_MONTHS)).unwrap_or(today + Duration::days(91));
        let busy = busy_dates(conn, aid, today - Duration::days(1), until);
        facts = Facts { free_saturdays: free_weekends(&busy, today, until), ..f };
    }
    facts.epk_url = contact_id.and_then(|c| epk_for(conn, c));
    facts
}

/// Latest message each way for a contact: (last inbound reply, last sent email).
pub fn last_messages(conn: &Connection, contact_id: i64) -> (Option<NaiveDateTime>, Option<(i64, NaiveDateTime)>) {
    let last_in: Option<String> = conn
        .query_row(
            "SELECT MAX(COALESCE(r.received_at, r.created_at)) FROM email_replies r, contacts c
             WHERE c.id = ?1 AND r.kind = 'reply'
               AND (r.contact_id = c.id OR (c.email IS NOT NULL AND lower(trim(c.email)) = lower(trim(r.from_addr))))",
            params![contact_id],
            |r| r.get(0),
        )
        .ok()
        .flatten();
    let last_out: Option<(i64, String)> = conn
        .query_row(
            "SELECT e.id, e.sent_at FROM emails e WHERE e.contact_id = ?1 AND e.status = 'sent'
             ORDER BY e.sent_at DESC, e.id DESC LIMIT 1",
            params![contact_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .ok()
        .flatten();
    (
        last_in.as_deref().and_then(ts),
        last_out.and_then(|(id, s)| ts(&s).map(|d| (id, d))),
    )
}

// ---------------- The pass ----------------

#[derive(Debug, Default, Serialize)]
pub struct PassResult {
    pub created: usize,
    pub drafted: usize,
    pub closed: usize,
    /// New reply items to notify: (item id, title, body).
    #[serde(skip)]
    pub notify: Vec<(i64, String, String)>,
}

/// Items already handled elsewhere close by themselves: a reply answered from
/// Compose, a nudge whose contact wrote back or left "in discussion". Snoozed
/// items whose time has come reopen.
pub fn auto_close(conn: &Connection, now: NaiveDateTime) -> usize {
    let n = fmt_ts(now);
    let _ = conn.execute(
        "UPDATE reply_desk SET state = 'open', snooze_until = NULL, updated_at = datetime('now')
         WHERE state = 'snoozed' AND snooze_until <= ?1",
        params![n],
    );
    let answered = conn
        .execute(
            "UPDATE reply_desk SET state = 'answered', updated_at = datetime('now')
             WHERE state IN ('pending','open','snoozed') AND EXISTS (
                SELECT 1 FROM emails e WHERE e.status = 'sent' AND e.sent_at > reply_desk.since
                  AND (e.contact_id = reply_desk.contact_id OR lower(trim(e.to_addr)) = lower(trim(reply_desk.to_addr)))
                  AND (reply_desk.kind = 'reply' OR e.id <> reply_desk.email_id))",
            [],
        )
        .unwrap_or(0);
    let nudges = conn
        .execute(
            "UPDATE reply_desk SET state = 'answered', updated_at = datetime('now')
             WHERE kind = 'nudge' AND state IN ('pending','open','snoozed') AND (
                EXISTS (SELECT 1 FROM email_replies r WHERE r.kind = 'reply' AND COALESCE(r.received_at, r.created_at) > reply_desk.since
                          AND (r.contact_id = reply_desk.contact_id OR lower(trim(r.from_addr)) = lower(trim(reply_desk.to_addr))))
                OR NOT EXISTS (SELECT 1 FROM contacts c WHERE c.id = reply_desk.contact_id AND c.status = 'in_discussion'))",
            [],
        )
        .unwrap_or(0);
    // A newer reply from the same contact replaces the older item.
    let superseded = conn
        .execute(
            "UPDATE reply_desk SET state = 'answered', updated_at = datetime('now')
             WHERE kind = 'reply' AND state IN ('pending','open','snoozed') AND EXISTS (
                SELECT 1 FROM reply_desk d WHERE d.kind = 'reply' AND d.contact_id = reply_desk.contact_id
                  AND d.id > reply_desk.id AND d.state IN ('pending','open','snoozed'))",
            [],
        )
        .unwrap_or(0);
    answered + nudges + superseded
}

struct ReplyCand {
    reply_id: i64,
    contact_id: i64,
    campaign_id: Option<i64>,
    email_id: Option<i64>,
    from: String,
    subject: String,
    text: String,
    received: String,
    residency: bool,
    insight: Option<String>,
}

fn reply_candidates(conn: &Connection, now: NaiveDateTime) -> Vec<ReplyCand> {
    let since = fmt_ts(now - Duration::days(LOOKBACK_DAYS));
    let Ok(mut st) = conn.prepare(
        "SELECT r.id, k.id, e.campaign_id, r.email_id, r.from_addr, COALESCE(r.subject,''), COALESCE(r.body_excerpt,''),
                COALESCE(r.received_at, r.created_at), COALESCE(c.kind,'') = 'residency', i.intent
         FROM email_replies r
         JOIN contacts k ON k.id = r.contact_id
              OR (r.contact_id IS NULL AND k.email IS NOT NULL AND lower(trim(k.email)) = lower(trim(r.from_addr)))
         LEFT JOIN emails e ON e.id = r.email_id
         LEFT JOIN campaigns c ON c.id = e.campaign_id
         LEFT JOIN reply_insights i ON i.email_reply_id = r.id
         WHERE r.kind = 'reply' AND COALESCE(r.received_at, r.created_at) >= ?1
           AND k.status <> 'declined'
           AND NOT EXISTS (SELECT 1 FROM reply_desk d WHERE d.thread_key = 'r:' || r.id)
           AND NOT EXISTS (SELECT 1 FROM emails s WHERE s.status = 'sent' AND s.sent_at > COALESCE(r.received_at, r.created_at)
                             AND (s.contact_id = k.id OR lower(trim(s.to_addr)) = lower(trim(r.from_addr))))
         GROUP BY r.id
         ORDER BY r.id",
    ) else {
        return Vec::new();
    };
    st.query_map(params![since], |r| {
        Ok(ReplyCand {
            reply_id: r.get(0)?,
            contact_id: r.get(1)?,
            campaign_id: r.get(2)?,
            email_id: r.get(3)?,
            from: r.get(4)?,
            subject: r.get(5)?,
            text: r.get(6)?,
            received: r.get(7)?,
            residency: r.get(8)?,
            insight: r.get(9)?,
        })
    })
    .map(|it| it.flatten().collect())
    .unwrap_or_default()
}

/// What to do with one reply: Some(state, intent) to record, None to wait.
fn triage(c: &ReplyCand, llm: bool, now: NaiveDateTime) -> Option<(&'static str, Option<String>)> {
    if let Some(i) = &c.insight {
        let keep = matches!(i.as_str(), "interested" | "question");
        return Some((if keep { "pending" } else { "skipped" }, Some(i.clone())));
    }
    // Residency replies get their own classification first (more context).
    let young = ts(&c.received).map_or(false, |r| now - r < Duration::minutes(RESIDENCY_WAIT_MIN));
    if c.residency && llm && young {
        return None;
    }
    let p = Pending {
        reply_id: c.reply_id,
        contact_id: c.contact_id,
        campaign_id: c.campaign_id.unwrap_or(0),
        kind: "reply".into(),
        from: c.from.clone(),
        subject: c.subject.clone(),
        text: c.text.clone(),
        email_id: c.email_id,
    };
    if replies::rule_classify(&p, now.date()).is_some() || status::looks_like_refusal(&c.text) {
        return Some(("skipped", None));
    }
    Some(("pending", None))
}

fn insert_item(
    conn: &Connection,
    key: &str,
    kind: Kind,
    contact_id: i64,
    campaign_id: Option<i64>,
    reply_id: Option<i64>,
    email_id: Option<i64>,
    to: &str,
    intent: Option<&str>,
    subject: &str,
    state: &str,
    since: &str,
) -> bool {
    conn.execute(
        "INSERT OR IGNORE INTO reply_desk (thread_key, kind, contact_id, campaign_id, email_reply_id, email_id, to_addr, intent, subject, state, since)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![key, kind.as_str(), contact_id, campaign_id, reply_id, email_id, to.trim().to_lowercase(), intent, subject, state, since],
    )
    .unwrap_or(0)
        > 0
}

/// Contacts in discussion whose last message is Thibault's, unanswered for
/// the nudge delay: one nudge per email of his.
fn create_nudges(conn: &Connection, now: NaiveDateTime) -> usize {
    let days = nudge_days(conn);
    let ids: Vec<i64> = conn
        .prepare("SELECT id FROM contacts WHERE status = 'in_discussion'")
        .and_then(|mut st| st.query_map([], |r| r.get(0)).map(|it| it.flatten().collect()))
        .unwrap_or_default();
    let mut n = 0;
    for cid in ids {
        let (last_in, last_out) = last_messages(conn, cid);
        let Some((eid, out_at)) = last_out else { continue };
        if !matches!(thread_state(last_in, Some(out_at), now, days), ThreadState::AwaitingThem { stalled: true, .. }) {
            continue;
        }
        // Thibault's silence is on purpose past a month: the follow-up queue owns it.
        if now - out_at > Duration::days(30) {
            continue;
        }
        let Some((to, subject, campaign)) = conn
            .query_row("SELECT to_addr, subject, campaign_id FROM emails WHERE id = ?1", params![eid], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<i64>>(2)?))
            })
            .ok()
        else {
            continue;
        };
        if crate::commands::outbox::suppression_reason(conn, &to).is_some() {
            continue;
        }
        if insert_item(conn, &format!("n:{}", eid), Kind::Nudge, cid, campaign, None, Some(eid), &to, None, &re_subject(&subject), "pending", &fmt_ts(out_at)) {
            n += 1;
        }
    }
    n
}

fn contact_names(conn: &Connection, cid: i64) -> (String, String) {
    conn.query_row("SELECT name, COALESCE(venue,'') FROM contacts WHERE id = ?1", params![cid], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap_or_default()
}

fn our_message(conn: &Connection, email_id: Option<i64>, contact_id: i64, before: Option<&str>) -> Option<Message> {
    let row = match email_id {
        Some(id) => conn
            .query_row("SELECT sent_at, subject, body FROM emails WHERE id = ?1", params![id], |r| {
                Ok(Message { at: r.get(0)?, subject: r.get(1)?, body: r.get(2)? })
            })
            .ok(),
        None => None,
    };
    row.or_else(|| {
        conn.query_row(
            "SELECT sent_at, subject, body FROM emails WHERE contact_id = ?1 AND status = 'sent' AND (?2 IS NULL OR sent_at <= ?2)
             ORDER BY sent_at DESC LIMIT 1",
            params![contact_id, before],
            |r| Ok(Message { at: r.get(0)?, subject: r.get(1)?, body: r.get(2)? }),
        )
        .ok()
    })
}

/// Everything the draft of one item is written from.
pub fn input_for(conn: &Connection, id: i64, today: NaiveDate) -> Option<DraftInput> {
    let (kind, cid, camp, rid, eid, to, since): (String, i64, Option<i64>, Option<i64>, Option<i64>, String, String) = conn
        .query_row(
            "SELECT kind, contact_id, campaign_id, email_reply_id, email_id, to_addr, since FROM reply_desk WHERE id = ?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)),
        )
        .ok()?;
    let kind = Kind::parse(&kind);
    let theirs = rid.and_then(|rid| {
        conn.query_row(
            "SELECT COALESCE(received_at, created_at), COALESCE(subject,''), COALESCE(body_excerpt,'') FROM email_replies WHERE id = ?1",
            params![rid],
            |r| Ok(Message { at: r.get(0)?, subject: r.get(1)?, body: r.get(2)? }),
        )
        .ok()
    });
    let ours = our_message(conn, eid, cid, (kind == Kind::Reply).then_some(since.as_str()));
    let lang = theirs
        .as_ref()
        .and_then(|m| detect_lang(&format!("{}\n{}", m.subject, m.body)))
        .or_else(|| ours.as_ref().and_then(|m| detect_lang(&m.body)))
        .unwrap_or("fr")
        .to_string();
    let (contact, venue) = contact_names(conn, cid);
    Some(DraftInput {
        kind: Some(kind),
        lang,
        today: Some(today),
        contact,
        venue,
        from_addr: to,
        ours,
        theirs,
        facts: facts_for(conn, Some(cid), camp, today),
    })
}

/// Write the draft of one item. Claude when available (one retry on an
/// unreadable answer), else the template. Returns the new state.
pub fn draft_item(conn: &Connection, id: i64, llm: Option<&dyn Llm>, today: NaiveDate) -> Option<String> {
    let input = input_for(conn, id, today)?;
    let known: Option<String> = conn.query_row("SELECT intent FROM reply_desk WHERE id = ?1", params![id], |r| r.get(0)).ok().flatten();
    let campaign: Option<i64> = conn.query_row("SELECT campaign_id FROM reply_desk WHERE id = ?1", params![id], |r| r.get(0)).ok().flatten();
    let mut out: Option<(String, String, String)> = None;
    if let Some(llm) = llm {
        let model = claude::drafting_model(conn);
        let req = build_request(&input, &model);
        for _ in 0..2 {
            match llm.complete(&req) {
                Ok(resp) => {
                    let m = if resp.model.is_empty() { model.clone() } else { resp.model.clone() };
                    claude::log_usage(conn, "reply_draft", campaign, &m, &resp.usage);
                    if let Some((intent, body)) = parse_answer(&resp.text) {
                        out = Some((intent, body, m));
                        break;
                    }
                }
                Err(e) => {
                    eprintln!("[reply_desk] draft {}: {}", id, e);
                    break;
                }
            }
        }
    }
    let (intent, body, by) = match out {
        Some((intent, body, m)) => (known.clone().unwrap_or(intent), body, m),
        None => (known.clone().unwrap_or_else(|| "unknown".into()), template_draft(&input), "template".into()),
    };
    // Claude judged an unclassified reply not worth an answer: off the desk.
    let state = if known.is_none() && input.kind == Some(Kind::Reply) && matches!(intent.as_str(), "not_interested" | "other") {
        "skipped"
    } else {
        "open"
    };
    let _ = conn.execute(
        "UPDATE reply_desk SET body = ?2, intent = ?3, lang = ?4, draft_by = ?5, edited = 0, state = ?6, updated_at = datetime('now')
         WHERE id = ?1 AND state IN ('pending','open','snoozed')",
        params![id, body, intent, input.lang, by, state],
    );
    Some(state.to_string())
}

/// One pass: close what was handled elsewhere, pick up new replies and stalled
/// threads, write the drafts, collect what to notify.
pub fn run_pass(conn: &Connection, llm: Option<&dyn Llm>, now: NaiveDateTime, today: NaiveDate) -> PassResult {
    let mut res = PassResult { closed: auto_close(conn, now), ..Default::default() };
    for c in reply_candidates(conn, now) {
        let Some((state, intent)) = triage(&c, llm.is_some(), now) else { continue };
        if insert_item(
            conn,
            &format!("r:{}", c.reply_id),
            Kind::Reply,
            c.contact_id,
            c.campaign_id,
            Some(c.reply_id),
            c.email_id,
            &c.from,
            intent.as_deref(),
            &re_subject(&c.subject),
            state,
            &c.received,
        ) && state == "pending"
        {
            res.created += 1;
        }
    }
    res.created += create_nudges(conn, now);
    res.closed += auto_close(conn, now);

    let pending: Vec<i64> = conn
        .prepare("SELECT id FROM reply_desk WHERE state = 'pending' ORDER BY kind DESC, since LIMIT ?1")
        .and_then(|mut st| st.query_map(params![MAX_DRAFTS_PER_PASS as i64], |r| r.get(0)).map(|it| it.flatten().collect()))
        .unwrap_or_default();
    for id in pending {
        if draft_item(conn, id, llm, today).is_some() {
            res.drafted += 1;
        }
    }

    // Notifications: new open reply items, recent ones only. Every item is
    // marked, so turning notifications back on never floods.
    let recent = fmt_ts(now - Duration::hours(NOTIFY_WITHIN_HOURS));
    let rows: Vec<(i64, String, String, String, String)> = conn
        .prepare(
            "SELECT d.id, COALESCE(NULLIF(k.venue,''), k.name, d.to_addr), COALESCE(d.intent,''), d.since, COALESCE(i.summary,'')
             FROM reply_desk d LEFT JOIN contacts k ON k.id = d.contact_id
             LEFT JOIN reply_insights i ON i.email_reply_id = d.email_reply_id
             WHERE d.kind = 'reply' AND d.state = 'open' AND d.notified_at IS NULL",
        )
        .and_then(|mut st| {
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))).map(|it| it.flatten().collect())
        })
        .unwrap_or_default();
    let on = notify_enabled(conn);
    for (id, who, intent, since, summary) in rows {
        let _ = conn.execute("UPDATE reply_desk SET notified_at = datetime('now') WHERE id = ?1", params![id]);
        if on && since >= recent {
            let what = match intent.as_str() {
                "question" => "pose une question",
                _ => "est intéressé",
            };
            let body = if summary.trim().is_empty() {
                format!("{} {}. Brouillon de réponse prêt.", who, what)
            } else {
                format!("{}. Brouillon de réponse prêt.", summary.trim().trim_end_matches('.'))
            };
            res.notify.push((id, format!("Réponse de {}", who), body));
        }
    }
    res
}

// ---------------- Notifications ----------------

static APP: OnceLock<tauri::AppHandle> = OnceLock::new();

/// Called once at startup (residency engine start), so passes that run off a
/// command (engine tick, background classification) can notify.
pub fn set_app(app: tauri::AppHandle) {
    let _ = APP.set(app);
}

pub fn deliver(res: &PassResult) {
    let Some(app) = APP.get() else { return };
    use tauri::Emitter;
    if res.created > 0 || res.closed > 0 || res.drafted > 0 {
        let _ = app.emit("reply-desk", ());
    }
    if res.notify.is_empty() {
        return;
    }
    use tauri_plugin_notification::{NotificationExt, PermissionState};
    if !matches!(app.notification().permission_state(), Ok(PermissionState::Granted)) {
        let _ = app.notification().request_permission();
    }
    let list: Vec<&(i64, String, String)> = res.notify.iter().collect();
    if list.len() > 3 {
        let _ = app
            .notification()
            .builder()
            .title(format!("{} réponses à traiter", list.len()))
            .body("Des programmateurs ont répondu. Brouillons prêts dans Envois, Réponses à traiter.")
            .show();
    } else {
        for (_, title, body) in &list {
            let _ = app.notification().builder().title(title).body(body).show();
        }
    }
    // The frontend opens this item when the app comes forward (notification click).
    let _ = app.emit("reply-desk-notified", list[0].0);
}

/// The pass as the engine runs it: Claude if configured, then notifications.
pub fn run_and_deliver(conn: &Connection, llm: Option<&dyn Llm>) {
    let now = Utc::now().naive_utc();
    let res = run_pass(conn, llm, now, chrono::Local::now().date_naive());
    deliver(&res);
}

// ---------------- Listing ----------------

#[derive(Debug, Serialize, Clone)]
pub struct DeskItem {
    pub id: i64,
    pub kind: String,
    pub state: String,
    pub contact_id: Option<i64>,
    pub contact: String,
    pub venue: String,
    pub campaign_id: Option<i64>,
    pub campaign: Option<String>,
    pub to_addr: String,
    pub intent: Option<String>,
    pub summary: Option<String>,
    pub lang: String,
    pub subject: String,
    pub body: String,
    pub draft_by: Option<String>,
    pub edited: bool,
    pub since: String,
    pub hours_waiting: i64,
    /// Programmer waiting for more than 24h (reply items).
    pub overdue: bool,
    pub their_subject: Option<String>,
    pub their_text: Option<String>,
    pub their_at: Option<String>,
    pub our_subject: Option<String>,
    pub our_text: Option<String>,
    pub our_at: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DeskList {
    pub items: Vec<DeskItem>,
    pub snoozed: i64,
    pub nudge_days: i64,
    pub notify: bool,
}

pub fn list(conn: &Connection, now: NaiveDateTime) -> Result<DeskList, String> {
    auto_close(conn, now);
    let mut st = conn
        .prepare(
            "SELECT d.id, d.kind, d.state, d.contact_id, COALESCE(k.name, d.to_addr), COALESCE(k.venue,''), d.campaign_id, c.name,
                    d.to_addr, d.intent, i.summary, d.lang, d.subject, d.body, d.draft_by, d.edited, d.since,
                    r.subject, r.body_excerpt, COALESCE(r.received_at, r.created_at), d.email_id
             FROM reply_desk d
             LEFT JOIN contacts k ON k.id = d.contact_id
             LEFT JOIN campaigns c ON c.id = d.campaign_id
             LEFT JOIN email_replies r ON r.id = d.email_reply_id
             LEFT JOIN reply_insights i ON i.email_reply_id = d.email_reply_id
             WHERE d.state = 'open'
             ORDER BY d.since",
        )
        .map_err(|e| e.to_string())?;
    let rows = st
        .query_map([], |r| {
            Ok((
                DeskItem {
                    id: r.get(0)?,
                    kind: r.get(1)?,
                    state: r.get(2)?,
                    contact_id: r.get(3)?,
                    contact: r.get(4)?,
                    venue: r.get(5)?,
                    campaign_id: r.get(6)?,
                    campaign: r.get(7)?,
                    to_addr: r.get(8)?,
                    intent: r.get(9)?,
                    summary: r.get(10)?,
                    lang: r.get(11)?,
                    subject: r.get(12)?,
                    body: r.get(13)?,
                    draft_by: r.get(14)?,
                    edited: r.get::<_, i64>(15)? != 0,
                    since: r.get(16)?,
                    hours_waiting: 0,
                    overdue: false,
                    their_subject: r.get(17)?,
                    their_text: r.get(18)?,
                    their_at: r.get(19)?,
                    our_subject: None,
                    our_text: None,
                    our_at: None,
                },
                r.get::<_, Option<i64>>(20)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    let days = nudge_days(conn);
    let mut items = Vec::new();
    for row in rows.flatten() {
        let (mut it, eid) = row;
        let since = ts(&it.since).unwrap_or(now);
        it.hours_waiting = (now - since).num_hours().max(0);
        if it.kind == "reply" {
            it.overdue = matches!(thread_state(Some(since), None, now, days), ThreadState::AwaitingUs { overdue: true, .. });
        }
        if let Some(cid) = it.contact_id {
            let before = (it.kind == "reply").then(|| it.since.clone());
            if let Some(m) = our_message(conn, eid, cid, before.as_deref()) {
                it.our_subject = Some(m.subject);
                it.our_text = Some(m.body);
                it.our_at = Some(m.at);
            }
        }
        items.push(it);
    }
    // Overdue first, then answers before nudges, the longest wait first.
    items.sort_by(|a, b| {
        b.overdue
            .cmp(&a.overdue)
            .then((a.kind == "nudge").cmp(&(b.kind == "nudge")))
            .then(a.since.cmp(&b.since))
    });
    let snoozed: i64 = conn.query_row("SELECT COUNT(*) FROM reply_desk WHERE state = 'snoozed'", [], |r| r.get(0)).unwrap_or(0);
    Ok(DeskList { items, snoozed, nudge_days: days, notify: notify_enabled(conn) })
}

#[derive(Debug, Serialize, Default)]
pub struct DeskCount {
    pub open: i64,
    pub overdue: i64,
}

pub fn count(conn: &Connection, now: NaiveDateTime) -> DeskCount {
    let limit = fmt_ts(now - Duration::hours(OVERDUE_HOURS));
    conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(kind = 'reply' AND since < ?1), 0) FROM reply_desk WHERE state = 'open'",
        params![limit],
        |r| Ok(DeskCount { open: r.get(0)?, overdue: r.get(1)? }),
    )
    .unwrap_or_default()
}

/// In-Reply-To / References for the answer to an item.
pub fn thread_ref(conn: &Connection, id: i64) -> ThreadRef {
    let (rid, eid): (Option<i64>, Option<i64>) = conn
        .query_row("SELECT email_reply_id, email_id FROM reply_desk WHERE id = ?1", params![id], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap_or((None, None));
    let ours: Option<String> = eid
        .and_then(|e| conn.query_row("SELECT message_id FROM emails WHERE id = ?1", params![e], |r| r.get(0)).ok().flatten());
    // Ids we made up for messages without one are not worth threading on.
    let theirs: Option<String> = rid
        .and_then(|r| conn.query_row("SELECT message_id FROM email_replies WHERE id = ?1", params![r], |x| x.get(0)).ok().flatten())
        .filter(|m: &String| !m.starts_with("<imap-"));
    let mut references: Vec<String> = Vec::new();
    for m in [&ours, &theirs].into_iter().flatten() {
        if !m.trim().is_empty() && !references.contains(m) {
            references.push(m.trim().to_string());
        }
    }
    ThreadRef { in_reply_to: theirs.or(ours), references }
}

/// After a successful send: the item is done, and so is the residency action.
pub fn mark_sent(conn: &Connection, id: i64, email_id: Option<i64>, subject: &str, body: &str) {
    let _ = conn.execute(
        "UPDATE reply_desk SET state = 'sent', sent_email_id = ?2, subject = ?3, body = ?4, updated_at = datetime('now') WHERE id = ?1",
        params![id, email_id, subject, body],
    );
    let _ = conn.execute(
        "UPDATE reply_insights SET handled = 1 WHERE email_reply_id = (SELECT email_reply_id FROM reply_desk WHERE id = ?1)",
        params![id],
    );
}

// ---------------- Commands ----------------

fn conn(state: &State<'_, AppState>) -> Result<r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>, String> {
    state.pool.get().map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub fn desk_list(state: State<'_, AppState>) -> Result<DeskList, String> {
    list(&*conn(&state)?, Utc::now().naive_utc())
}

#[tauri::command(async)]
pub fn desk_count(state: State<'_, AppState>) -> Result<DeskCount, String> {
    Ok(count(&*conn(&state)?, Utc::now().naive_utc()))
}

/// Run a pass now (button "Actualiser"): new items and their drafts.
#[tauri::command]
pub async fn desk_refresh(state: State<'_, AppState>) -> Result<PassResult, String> {
    let pool = state.pool.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let conn = pool.get().map_err(|e| e.to_string())?;
        let llm = claude::client(&conn);
        let mut res = run_pass(&conn, llm.as_deref(), Utc::now().naive_utc(), chrono::Local::now().date_naive());
        // Thibault is looking at the list: no notification needed.
        res.notify.clear();
        Ok(res)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(async)]
pub fn desk_save(state: State<'_, AppState>, id: i64, subject: String, body: String) -> Result<(), String> {
    conn(&state)?
        .execute(
            "UPDATE reply_desk SET subject = ?2, body = ?3, edited = 1, updated_at = datetime('now') WHERE id = ?1",
            params![id, subject, body],
        )
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command(async)]
pub fn desk_ignore(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    let c = conn(&state)?;
    c.execute("UPDATE reply_desk SET state = 'ignored', updated_at = datetime('now') WHERE id = ?1", params![id])
        .map_err(|e| e.to_string())?;
    let _ = c.execute(
        "UPDATE reply_insights SET handled = 1 WHERE email_reply_id = (SELECT email_reply_id FROM reply_desk WHERE id = ?1)",
        params![id],
    );
    Ok(())
}

/// "Plus tard": back in the list after `hours` (default: tomorrow, same time).
#[tauri::command(async)]
pub fn desk_snooze(state: State<'_, AppState>, id: i64, hours: Option<i64>) -> Result<(), String> {
    let until = fmt_ts(Utc::now().naive_utc() + Duration::hours(hours.unwrap_or(24).clamp(1, 24 * 30)));
    conn(&state)?
        .execute(
            "UPDATE reply_desk SET state = 'snoozed', snooze_until = ?2, updated_at = datetime('now') WHERE id = ?1",
            params![id, until],
        )
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Write the draft again (after the facts changed, or to get another take).
#[tauri::command]
pub async fn desk_redraft(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    let pool = state.pool.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let conn = pool.get().map_err(|e| e.to_string())?;
        let llm = claude::client(&conn);
        // A redraft never takes the item off the desk.
        let known: Option<String> = conn.query_row("SELECT intent FROM reply_desk WHERE id = ?1", params![id], |r| r.get(0)).ok().flatten();
        if known.is_none() {
            let _ = conn.execute("UPDATE reply_desk SET intent = 'unknown' WHERE id = ?1", params![id]);
        }
        draft_item(&conn, id, llm.as_deref(), chrono::Local::now().date_naive()).ok_or_else(|| "Élément introuvable.".to_string())?;
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Send the (edited) answer through the normal path: guardrails, threading
/// headers, logged in `emails`, pipeline updated.
#[tauri::command(async)]
pub fn desk_send(state: State<'_, AppState>, id: i64, subject: String, body: String) -> Result<SendResult, String> {
    let (contact_id, campaign_id, to, thread) = {
        let c = conn(&state)?;
        let (cid, camp, to, st): (Option<i64>, Option<i64>, String, String) = c
            .query_row("SELECT contact_id, campaign_id, to_addr, state FROM reply_desk WHERE id = ?1", params![id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })
            .map_err(|_| "Élément introuvable.".to_string())?;
        if st == "sent" {
            return Err("Cette réponse est déjà partie.".into());
        }
        if body.trim().is_empty() {
            return Err("Le message est vide.".into());
        }
        (cid, camp, to, thread_ref(&c, id))
    };
    let (res, email_id) = email::send_one_in_thread(&state.pool, contact_id, campaign_id, &to, &subject, &body, Some(&thread))?;
    if res.ok {
        mark_sent(&*conn(&state)?, id, email_id, &subject, &body);
    }
    Ok(res)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::residency::claude::fake::FakeLlm;
    use crate::commands::residency::testdb;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }
    fn t(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, TS).unwrap()
    }

    #[test]
    fn thread_state_rules() {
        let now = t("2026-10-30 12:00:00");
        // They wrote last, 30h ago: overdue.
        assert_eq!(
            thread_state(Some(t("2026-10-29 06:00:00")), Some(t("2026-10-20 10:00:00")), now, 4),
            ThreadState::AwaitingUs { since: t("2026-10-29 06:00:00"), overdue: true }
        );
        // They wrote 2h ago: not yet.
        assert!(matches!(
            thread_state(Some(t("2026-10-30 10:00:00")), None, now, 4),
            ThreadState::AwaitingUs { overdue: false, .. }
        ));
        // We wrote last, 5 days ago: stalled at 4 days, not at 7.
        let s = thread_state(Some(t("2026-10-20 10:00:00")), Some(t("2026-10-25 11:00:00")), now, 4);
        assert_eq!(s, ThreadState::AwaitingThem { since: t("2026-10-25 11:00:00"), stalled: true });
        assert!(matches!(
            thread_state(Some(t("2026-10-20 10:00:00")), Some(t("2026-10-25 11:00:00")), now, 7),
            ThreadState::AwaitingThem { stalled: false, .. }
        ));
        assert_eq!(thread_state(None, None, now, 4), ThreadState::Idle);
    }

    #[test]
    fn availability_extraction() {
        // Today Friday 30 Oct 2026. Saturdays: 31 Oct, 7, 14, 21, 28 Nov.
        let busy = [d("2026-10-31"), d("2026-11-13"), d("2026-11-26")];
        let w = free_weekends(&busy, d("2026-10-30"), d("2026-11-28"));
        // 31 Oct (Sat booked) and 14 Nov (Fri 13 booked) are out; a Thursday gig does not count.
        assert_eq!(w, vec![d("2026-11-07"), d("2026-11-21"), d("2026-11-28")]);
        assert_eq!(human_list(&w, "fr"), "7 nov., 21 nov. et 28 nov.");
        assert_eq!(human_list(&w[..2], "en"), "Nov 7 and Nov 21");
        assert!(free_weekends(&[], d("2026-11-02"), d("2026-11-06")).is_empty());
    }

    #[test]
    fn availability_from_agenda() {
        let c = testdb::conn();
        c.execute_batch(
            "INSERT INTO artists (id, name, fee_range, tech_rider, mix_url) VALUES (1, 'Piers', '800 à 1200 €', 'CDJ-3000 x2, DJM-900', 'https://sc.com/piers/vol4');
             INSERT INTO contacts (id, name, venue, email, status, artist_id) VALUES (5, 'Léa', 'Club A', 'lea@cluba.fr', 'in_discussion', 1);
             INSERT INTO events (artist_id, title, date, status) VALUES
               (1, 'Gig', '2026-11-07', 'confirmed'), (1, 'Hold', '2026-11-20', 'hold'), (1, 'Off', '2026-11-28', 'cancelled');
             INSERT INTO campaigns (id, name, kind) VALUES (1, 'R', 'residency');
             INSERT INTO campaign_messages (campaign_id, contact_id, step, press_url) VALUES (1, 5, 1, 'https://insrt.studio/fr/p/lea');",
        )
        .unwrap();
        let f = facts_for(&c, Some(5), None, d("2026-10-30"));
        assert_eq!(f.artist, "Piers");
        assert_eq!(f.fee_range.as_deref(), Some("800 à 1200 €"));
        assert_eq!(f.epk_url.as_deref(), Some("https://insrt.studio/fr/p/lea"));
        assert_eq!(&f.free_saturdays[..3], &[d("2026-10-31"), d("2026-11-14"), d("2026-11-28")]);
        // Three months ahead, no further.
        assert!(f.free_saturdays.last().unwrap() <= &d("2027-01-30"));
    }

    #[test]
    fn language_detection() {
        assert_eq!(detect_lang("Bonjour, merci pour le mix, quelle est votre date ?"), Some("fr"));
        assert_eq!(detect_lang("Hi Thibault, thanks for the mix. What is your fee for a Friday night?"), Some("en"));
        assert_eq!(detect_lang("👍"), None);
    }

    fn input() -> DraftInput {
        DraftInput {
            kind: Some(Kind::Reply),
            lang: "en".into(),
            today: Some(d("2026-10-30")),
            contact: "Jose".into(),
            venue: "De School".into(),
            from_addr: "jose@club.nl".into(),
            ours: Some(Message { at: "2026-10-20".into(), subject: "residency".into(), body: "Hello, ...".into() }),
            theirs: Some(Message { at: "2026-10-29".into(), subject: "Re: residency".into(), body: "What's your fee?".into() }),
            facts: Facts {
                artist: "Piers".into(),
                fee_range: Some("800 to 1200 EUR".into()),
                tech_rider: None,
                mix_url: Some("https://sc.com/m".into()),
                epk_url: Some("https://insrt.studio/en/p/x".into()),
                free_saturdays: vec![d("2026-11-07"), d("2026-11-21")],
            },
        }
    }

    #[test]
    fn prompt_building() {
        let req = build_request(&input(), "claude-haiku-5-5");
        assert_eq!(req.model, "claude-haiku-5-5");
        assert_eq!(req.system.len(), 1);
        assert!(req.system[0].cache);
        // House rules shared with the first email.
        assert!(req.system[0].text.contains("N'hésitez pas") && req.system[0].text.contains("Je me permets"));
        assert_eq!(reply_system(), req.system[0].text, "the cached prefix is stable");
        let u = &req.user;
        assert!(u.contains("Case: reply") && u.contains("Language: English"));
        assert!(u.contains("Fee range: 800 to 1200 EUR"));
        assert!(u.contains("Personal press link (insrt EPK): https://insrt.studio/en/p/x"));
        assert!(u.contains("2026-11-07 (Nov 7)"));
        assert!(!u.contains("Technical rider"));
        assert!(u.contains("What's your fee?"));
        let mut no_dates = input();
        no_dates.facts.free_saturdays.clear();
        assert!(build_user(&no_dates).contains("do not offer dates"));
    }

    #[test]
    fn answer_parsing_and_template() {
        let (i, b) = parse_answer(r#"{"intent":"question","body":"Hello Jose — happy to.\n\nCheers,\nThibault"}"#).unwrap();
        assert_eq!(i, "question");
        assert!(!b.contains('—'));
        assert!(parse_answer(r#"{"intent":"bogus","body":"x"}"#).is_none());
        assert!(parse_answer("nope").is_none());
        let en = template_draft(&input());
        assert!(en.contains("Nov 7 and Nov 21") && en.contains("https://insrt.studio/en/p/x") && en.ends_with("Thibault"));
        let mut fr = input();
        fr.lang = "fr".into();
        fr.kind = Some(Kind::Nudge);
        let fr = template_draft(&fr);
        assert!(fr.contains("7 nov. et 21 nov.") && fr.contains("À bientôt,\nThibault"));
        assert!(!fr.contains('—'));
        assert_eq!(re_subject("Re: residency"), "Re: residency");
        assert_eq!(re_subject("residency"), "Re: residency");
    }

    fn base() -> Connection {
        let c = testdb::conn();
        c.execute_batch(
            "INSERT INTO artists (id, name, fee_range, mix_url) VALUES (1, 'Piers', '800 à 1200 €', 'https://sc.com/m');
             INSERT INTO campaigns (id, name) VALUES (2, 'Booking hiver');
             INSERT INTO contacts (id, name, venue, email, status) VALUES
               (10, 'Club A', 'Club A', 'booking@cluba.fr', 'in_discussion'),
               (11, 'Club B', 'Club B', 'prog@clubb.fr', 'in_discussion'),
               (12, 'Club C', 'Club C', 'hello@clubc.fr', 'in_discussion');
             INSERT INTO emails (id, contact_id, campaign_id, to_addr, subject, body, status, sent_at, message_id) VALUES
               (1, 10, 2, 'booking@cluba.fr', 'Piers x Club A', 'Hello, ...', 'sent', '2026-10-20 10:00:00', '<s1@insrt.fr>'),
               (2, 11, 2, 'prog@clubb.fr', 'Piers x Club B', 'Hello, ...', 'sent', '2026-10-20 10:00:00', '<s2@insrt.fr>'),
               (3, 12, 2, 'hello@clubc.fr', 'Piers x Club C', 'Hello, ...', 'sent', '2026-10-25 09:00:00', '<s3@insrt.fr>');
             INSERT INTO email_replies (id, email_id, contact_id, from_addr, subject, message_id, kind, received_at, body_excerpt) VALUES
               (1, 1, 10, 'booking@cluba.fr', 'Re: Piers x Club A', '<r1@cluba.fr>', 'reply', '2026-10-29 08:00:00',
                'Bonjour, merci pour le mix ! Vous seriez dispo un samedi en novembre ? Quel est votre cachet ?'),
               (2, 2, 11, 'prog@clubb.fr', 'Re: Piers x Club B', '<r2@clubb.fr>', 'reply', '2026-10-30 09:00:00',
                'Merci mais nous allons passer notre tour, bonne continuation.'),
               (3, 3, 12, 'hello@clubc.fr', 'Re: Piers x Club C', '<r3@clubc.fr>', 'reply', '2026-10-21 09:00:00', 'Avec plaisir, on en reparle.');",
        )
        .unwrap();
        c
    }

    #[test]
    fn pass_drafts_notifies_and_threads() {
        let c = base();
        let now = t("2026-10-30 11:00:00");
        let llm = FakeLlm::new(&[
            r#"{"intent":"question","body":"Bonjour,\n\nAvec plaisir. Je suis libre le 7 nov.\n\nÀ bientôt,\nThibault"}"#,
        ]);
        let r = run_pass(&c, Some(&llm), now, d("2026-10-30"));
        // Club A: question -> drafted by Claude. Club B: refusal by rules (no call).
        // Club C: its reply (21 Oct) is older than our last email (25 Oct), 5 days
        // ago, no answer: a nudge (written by Claude too: FakeLlm has one answer,
        // so the nudge falls back to the template).
        assert_eq!(r.created, 2);
        let seen = llm.seen.lock().unwrap();
        assert!(seen[0].user.contains("Quel est votre cachet") && seen[0].user.contains("Language: French"));
        assert!(seen[0].user.contains("Fee range: 800 à 1200 €"));
        drop(seen);
        let l = list(&c, now).unwrap();
        assert_eq!(l.items.len(), 2);
        let a = &l.items[0];
        assert_eq!((a.kind.as_str(), a.contact_id, a.intent.as_deref()), ("reply", Some(10), Some("question")));
        assert!(a.overdue, "27h without an answer");
        assert!(a.body.contains("libre le 7 nov."));
        assert_eq!(a.subject, "Re: Piers x Club A");
        assert_eq!(a.our_subject.as_deref(), Some("Piers x Club A"));
        let n = &l.items[1];
        assert_eq!((n.kind.as_str(), n.contact_id, n.draft_by.as_deref()), ("nudge", Some(12), Some("template")));
        assert!(n.body.contains("Thibault") && n.subject == "Re: Piers x Club C");
        // The reply is notified once (recent enough); the refusal never shows.
        assert_eq!(r.notify.len(), 0, "27h old: past the notification window");
        let calls: i64 = c.query_row("SELECT COUNT(*) FROM ai_usage WHERE purpose = 'reply_draft'", [], |r| r.get(0)).unwrap();
        assert!(calls >= 1);
        // Threading: answers the programmer's message, references the chain.
        let tr = thread_ref(&c, a.id);
        assert_eq!(tr.in_reply_to.as_deref(), Some("<r1@cluba.fr>"));
        assert_eq!(tr.references, vec!["<s1@insrt.fr>".to_string(), "<r1@cluba.fr>".to_string()]);
        let tn = thread_ref(&c, n.id);
        assert_eq!(tn.in_reply_to.as_deref(), Some("<s3@insrt.fr>"));
        // A second pass is a no-op.
        let r2 = run_pass(&c, Some(&FakeLlm::new(&[])), now, d("2026-10-30"));
        assert_eq!((r2.created, r2.drafted), (0, 0));
        // Sending (recorded the way send_one does) closes the item; answering
        // from Compose closes the nudge too.
        c.execute(
            "INSERT INTO emails (id, contact_id, to_addr, subject, body, status, sent_at) VALUES (9, 10, 'booking@cluba.fr', 'Re', 'b', 'sent', '2026-10-30 11:30:00')",
            [],
        )
        .unwrap();
        mark_sent(&c, a.id, Some(9), "Re: Piers x Club A", "b");
        c.execute(
            "INSERT INTO emails (contact_id, to_addr, subject, body, status, sent_at) VALUES (12, 'hello@clubc.fr', 'Re', 'b', 'sent', '2026-10-30 11:40:00')",
            [],
        )
        .unwrap();
        let l = list(&c, t("2026-10-30 12:00:00")).unwrap();
        assert!(l.items.is_empty());
        let st: String = c.query_row("SELECT state FROM reply_desk WHERE kind = 'nudge'", [], |r| r.get(0)).unwrap();
        assert_eq!(st, "answered");
    }

    #[test]
    fn fresh_reply_notifies_and_unrelated_is_dropped() {
        let c = base();
        c.execute("UPDATE email_replies SET received_at = '2026-10-30 10:30:00' WHERE id = 1", []).unwrap();
        c.execute("UPDATE contacts SET status = 'confirmed' WHERE id = 12", []).unwrap();
        // Club A unclassified: Claude says interested. Then a newsletter-like
        // reply from a new contact: Claude says other, off the desk.
        c.execute_batch(
            "INSERT INTO contacts (id, name, email, status) VALUES (13, 'Club D', 'news@clubd.fr', 'contacted');
             INSERT INTO emails (id, contact_id, to_addr, subject, body, status, sent_at) VALUES (4, 13, 'news@clubd.fr', 's', 'b', 'sent', '2026-10-22 10:00:00');
             INSERT INTO email_replies (id, email_id, contact_id, from_addr, subject, message_id, kind, received_at, body_excerpt)
               VALUES (4, 4, 13, 'news@clubd.fr', 'Our November line-up', '<r4@clubd.fr>', 'reply', '2026-10-30 10:00:00', 'Discover our line-up for November');",
        )
        .unwrap();
        // Drafted oldest first: Club D (10:00), then Club A (10:30).
        let llm = FakeLlm::new(&[
            r#"{"intent":"other","body":"-"}"#,
            r#"{"intent":"interested","body":"Hello,\n\nTop.\n\nÀ bientôt,\nThibault"}"#,
        ]);
        let r = run_pass(&c, Some(&llm), t("2026-10-30 11:00:00"), d("2026-10-30"));
        assert_eq!(r.notify.len(), 1);
        assert!(r.notify[0].1.contains("Club A"));
        let l = list(&c, t("2026-10-30 11:00:00")).unwrap();
        assert_eq!(l.items.len(), 1);
        assert!(!l.items[0].overdue);
        // Notified once only.
        let again = run_pass(&c, None, t("2026-10-30 11:05:00"), d("2026-10-30"));
        assert!(again.notify.is_empty());
        // Notifications off: nothing to show even for a new reply.
        c.execute("INSERT INTO settings (key, value) VALUES ('desk_notify', '0')", []).unwrap();
        assert!(!notify_enabled(&c));
    }

    #[test]
    fn residency_reply_waits_for_its_classification() {
        let c = base();
        c.execute_batch(
            "INSERT INTO campaigns (id, name, kind) VALUES (3, 'Résidence', 'residency');
             UPDATE emails SET campaign_id = 3 WHERE id = 1;
             UPDATE email_replies SET received_at = '2026-10-30 10:50:00' WHERE id = 1;",
        )
        .unwrap();
        let now = t("2026-10-30 11:00:00");
        // Unclassified and young: wait (the residency agent reads it first).
        run_pass(&c, Some(&FakeLlm::new(&["x", "x", "x", "x"])), now, d("2026-10-30"));
        let n: i64 = c.query_row("SELECT COUNT(*) FROM reply_desk WHERE thread_key = 'r:1'", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0);
        // Once classified interested it comes in. Without an API key: template.
        c.execute(
            "INSERT INTO reply_insights (email_reply_id, contact_id, campaign_id, sentiment, intent, summary) VALUES (1, 10, 3, 'neutral', 'interested', 'Partant')",
            [],
        )
        .unwrap();
        let r = run_pass(&c, None, now, d("2026-10-30"));
        assert_eq!(r.notify.len(), 1);
        assert!(r.notify[0].2.starts_with("Partant"));
        let (by, intent): (String, String) = c
            .query_row("SELECT draft_by, intent FROM reply_desk WHERE thread_key = 'r:1'", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!((by.as_str(), intent.as_str()), ("template", "interested"));
    }
}
