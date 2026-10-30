//! "Mesurer ce qui marche": the booking funnel, subject-line A/B tests and the
//! Monday digest.
//!
//! Funnel. The unit is a *prospect*: one (contact, campaign) pair that received
//! at least one email. A pair belongs to the period of its first send (cohort),
//! and every later outcome counts for it whenever it happens:
//!
//! sent -> opened -> EPK listened -> replied -> positive reply -> offer ->
//! confirmed -> fee collected
//!
//! - opened     pixel open on any email of the pair (a reply or a listen implies it)
//! - listened   insrt EPK `listen` event for the contact after the first send
//! - replied    a real reply (IMAP sync, out-of-office excluded)
//! - positive   reply read as positive / interested by the residency agent
//! - offer      an Agenda event (hold or confirmed) created after the first send
//! - confirmed  a confirmed Agenda event, or the contact marked confirmed
//! - collected  a confirmed gig with a fee, played (date passed)
//!
//! From `replied` on, stages are cumulative (a confirmed booking implies a
//! reply, even one taken by phone). `listened` is a side branch: not everyone
//! gets an EPK link, so it is never forced. Contact-level outcomes (events,
//! listens, replies not tied to an email) go to the contact's pair whose first
//! send precedes them most closely (its own campaign first when known).
//!
//! Breakdowns by city, country, campaign and contact category are full
//! funnels over prospects. Breakdowns by step, template, weekday and hour are
//! per email (sent, opened, replied, positive): those are properties of one
//! send, not of a prospect. Weekday and hour are the recipient's local time.
//!
//! A/B. A step (bulk send or residency sequence) may carry 2 or 3 subject
//! lines. Each recipient gets one deterministically (hash of the contact id,
//! salted per campaign/step), stored on `emails.subject_variant`. Results per
//! arm come with a two-proportion test on the reply rate; under MIN_ARM sends
//! per arm the verdict is "not enough data".
//!
//! Digest. On Monday (first engine tick after 08:00) last week is summarised
//! into `weekly_digests`: the week's cohort and activity, best/worst campaigns
//! and variants over eight weeks, and up to three suggestions computed from
//! the last 90 days. Emailing it (to the sender's own address only) is off by
//! default, setting `funnel_digest_email` = "1".

use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveDateTime, Timelike, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use tauri::State;

use crate::commands::residency::zone::Tz;
use crate::AppState;

// ---------------------------------------------------------------------------
// Schema (additive, idempotent; run from db::migrate)
// ---------------------------------------------------------------------------

/// - emails.subject_variant   'A' | 'B' | 'C' when the send was part of an A/B test
/// - emails.template_id       template the bulk send started from (breakdown + "keep the best")
/// - emails.step              sequence step of a residency email
/// - campaign_messages.subject_variant  variant drafted for that message
/// - ab_subjects              the raw subject template of each bulk arm ("keep the best")
/// - weekly_digests           one Monday digest per week
pub fn migrate(conn: &Connection) {
    for stmt in [
        "ALTER TABLE emails ADD COLUMN subject_variant TEXT",
        "ALTER TABLE emails ADD COLUMN template_id INTEGER",
        "ALTER TABLE emails ADD COLUMN step INTEGER",
        "ALTER TABLE campaign_messages ADD COLUMN subject_variant TEXT",
        "CREATE TABLE IF NOT EXISTS ab_subjects (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            campaign_id INTEGER NOT NULL DEFAULT 0,
            template_id INTEGER NOT NULL DEFAULT 0,
            step        INTEGER NOT NULL DEFAULT 0,
            label       TEXT NOT NULL,
            subject     TEXT NOT NULL,
            created_at  TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at  TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE (campaign_id, template_id, step, label)
        )",
        "CREATE TABLE IF NOT EXISTS weekly_digests (
            week_start TEXT PRIMARY KEY,
            data       TEXT NOT NULL,
            emailed_at TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        )",
        "CREATE INDEX IF NOT EXISTS idx_campaign_messages_sent_email ON campaign_messages(sent_email_id)",
        "CREATE INDEX IF NOT EXISTS idx_press_activity_type ON press_activity(type, contact_id)",
        "CREATE INDEX IF NOT EXISTS idx_events_contact ON events(contact_id)",
    ] {
        let _ = conn.execute(stmt, []);
    }
}

// ---------------------------------------------------------------------------
// A/B subject variants
// ---------------------------------------------------------------------------

pub const MAX_VARIANTS: usize = 3;
/// Sends per arm below which no verdict is given.
pub const MIN_ARM: i64 = 30;

pub fn variant_label(i: usize) -> &'static str {
    ["A", "B", "C"].get(i).copied().unwrap_or("C")
}

fn label_index(label: &str) -> Option<usize> {
    match label.trim() {
        "A" => Some(0),
        "B" => Some(1),
        "C" => Some(2),
        _ => None,
    }
}

/// Deterministic arm for a recipient: SplitMix64 of (contact id, salt). The
/// salt (campaign, step) keeps one contact from landing on "A" in every test.
pub fn variant_index(contact_id: i64, salt: i64, n: usize) -> usize {
    if n <= 1 {
        return 0;
    }
    let mut z = (contact_id as u64) ^ (salt as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z % n as u64) as usize
}

/// The arms of a test: the main subject first (A), then the extra ones,
/// trimmed, deduplicated, at most MAX_VARIANTS. One entry = no test.
pub fn clean_variants(main: &str, extra: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for s in std::iter::once(main).chain(extra.iter().map(String::as_str)) {
        let s = s.trim();
        if !s.is_empty() && !out.iter().any(|o| o == s) && out.len() < MAX_VARIANTS {
            out.push(s.to_string());
        }
    }
    out
}

/// The subject template for a recipient and its label, or None without a test.
pub fn pick<'a>(contact_id: i64, salt: i64, variants: &'a [String]) -> Option<(&'static str, &'a str)> {
    if variants.len() < 2 {
        return None;
    }
    let i = variant_index(contact_id, salt, variants.len());
    Some((variant_label(i), variants[i].as_str()))
}

/// Salt of a test: campaign and step (or template for bulk sends).
pub fn salt(campaign_id: Option<i64>, scope: i64) -> i64 {
    campaign_id.unwrap_or(0).wrapping_mul(1000).wrapping_add(scope)
}

/// Remember the raw subject of each bulk arm, for "keep the best".
pub fn record_variants(conn: &Connection, campaign_id: Option<i64>, template_id: Option<i64>, variants: &[String]) {
    if variants.len() < 2 {
        return;
    }
    for (i, s) in variants.iter().enumerate() {
        let _ = conn.execute(
            "INSERT INTO ab_subjects (campaign_id, template_id, step, label, subject) VALUES (?1, ?2, 0, ?3, ?4)
             ON CONFLICT(campaign_id, template_id, step, label) DO UPDATE SET subject = excluded.subject, updated_at = datetime('now')",
            params![campaign_id.unwrap_or(0), template_id.unwrap_or(0), variant_label(i), s],
        );
    }
}

/// Tag a logged email with its template / step / variant (bulk send path).
pub fn tag_email(conn: &Connection, email_id: i64, template_id: Option<i64>, step: Option<i64>, variant: Option<&str>) {
    if template_id.is_none() && step.is_none() && variant.is_none() {
        return;
    }
    let _ = conn.execute(
        "UPDATE emails SET template_id = COALESCE(?2, template_id), step = COALESCE(?3, step),
                subject_variant = COALESCE(?4, subject_variant) WHERE id = ?1",
        params![email_id, template_id, step, variant],
    );
}

/// Residency path: copy step + variant of a sent campaign message onto its email row.
pub fn tag_from_message(conn: &Connection, message_id: i64, email_id: i64) {
    let _ = conn.execute(
        "UPDATE emails SET step = COALESCE(step, (SELECT step FROM campaign_messages WHERE id = ?1)),
                subject_variant = COALESCE(subject_variant, (SELECT subject_variant FROM campaign_messages WHERE id = ?1))
         WHERE id = ?2",
        params![message_id, email_id],
    );
}

/// Normal CDF (Abramowitz-Stegun 7.1.26 erf, |error| < 1.5e-7).
fn norm_cdf(x: f64) -> f64 {
    let t = 1.0 / (1.0 + 0.327_591_1 * x.abs() / std::f64::consts::SQRT_2);
    let y = 1.0
        - (((((1.061_405_429 * t - 1.453_152_027) * t) + 1.421_413_741) * t - 0.284_496_736) * t + 0.254_829_592)
            * t
            * (-(x * x) / 2.0).exp();
    if x >= 0.0 {
        0.5 * (1.0 + y)
    } else {
        0.5 * (1.0 - y)
    }
}

/// Two-sided p-value of a two-proportion z-test (k1/n1 vs k2/n2).
pub fn two_proportion_p(k1: i64, n1: i64, k2: i64, n2: i64) -> Option<f64> {
    if n1 <= 0 || n2 <= 0 {
        return None;
    }
    let (p1, p2) = (k1 as f64 / n1 as f64, k2 as f64 / n2 as f64);
    let p = (k1 + k2) as f64 / (n1 + n2) as f64;
    let se = (p * (1.0 - p) * (1.0 / n1 as f64 + 1.0 / n2 as f64)).sqrt();
    if se == 0.0 {
        return Some(1.0);
    }
    let z = (p1 - p2) / se;
    Some((2.0 * (1.0 - norm_cdf(z.abs()))).clamp(0.0, 1.0))
}

// ---------------------------------------------------------------------------
// Output types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Stages {
    pub sent: i64,
    pub opened: i64,
    pub listened: i64,
    pub replied: i64,
    pub positive: i64,
    pub offer: i64,
    pub confirmed: i64,
    pub collected: i64,
    /// Fees of the collected gigs (EUR).
    pub fee_collected: f64,
}

impl Stages {
    fn reply_rate(&self) -> Option<f64> {
        rate(self.replied, self.sent)
    }
}

fn rate(k: i64, n: i64) -> Option<f64> {
    (n > 0).then(|| k as f64 / n as f64)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversion {
    pub from: String,
    pub to: String,
    pub rate: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Row {
    pub key: String,
    pub label: String,
    #[serde(default)]
    pub kind: Option<String>,
    pub stages: Stages,
    pub reply_rate: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Arm {
    pub label: String,
    /// One subject as sent (rendered for its first recipient).
    pub example: String,
    pub sent: i64,
    pub opened: i64,
    pub replied: i64,
    pub positive: i64,
    pub reply_rate: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VariantGroup {
    pub campaign_id: i64,
    pub campaign_name: String,
    /// 'residency' | 'standard'
    pub kind: String,
    pub template_id: Option<i64>,
    pub template_name: Option<String>,
    pub step: Option<i64>,
    pub arms: Vec<Arm>,
    /// 'insufficient' | 'significant' | 'trend' | 'none'
    pub verdict: String,
    pub p_value: Option<f64>,
    pub best: Option<String>,
    /// "Keep the best" is possible (residency step or a bulk send from a template).
    pub can_keep: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FilterOptions {
    pub campaigns: Vec<(i64, String)>,
    pub categories: Vec<String>,
    pub countries: Vec<String>,
    pub cities: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FunnelReport {
    pub from: Option<String>,
    pub to: Option<String>,
    pub total: Stages,
    pub conversions: Vec<Conversion>,
    pub median_reply_hours: Option<f64>,
    pub median_reply_to_confirmed_days: Option<f64>,
    /// Prospects who listened to the EPK and never replied.
    pub listened_no_reply: i64,
    pub by_city: Vec<Row>,
    pub by_country: Vec<Row>,
    pub by_campaign: Vec<Row>,
    pub by_category: Vec<Row>,
    pub by_step: Vec<Row>,
    pub by_template: Vec<Row>,
    pub by_weekday: Vec<Row>,
    pub by_hour: Vec<Row>,
    /// Weekday x part of day ("2-matin"), used by the suggestions.
    pub by_slot: Vec<Row>,
    pub variants: Vec<VariantGroup>,
    pub options: FilterOptions,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct FunnelQuery {
    /// YYYY-MM-DD, inclusive.
    pub from: Option<String>,
    pub to: Option<String>,
    pub campaign_id: Option<i64>,
    /// 'residency' | 'standard'
    pub kind: Option<String>,
    pub category: Option<String>,
    pub country: Option<String>,
    pub city: Option<String>,
}

// ---------------------------------------------------------------------------
// Raw facts
// ---------------------------------------------------------------------------

/// Timestamps in the database are SQLite `datetime('now')` (UTC), RFC 3339
/// (insrt) or a bare date.
pub fn parse_ts(raw: &str) -> Option<NaiveDateTime> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(d) = DateTime::parse_from_rfc3339(s) {
        return Some(d.naive_utc());
    }
    for f in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M"] {
        if let Ok(d) = NaiveDateTime::parse_from_str(s, f) {
            return Some(d);
        }
    }
    NaiveDate::parse_from_str(s.get(..10)?, "%Y-%m-%d").ok().and_then(|d| d.and_hms_opt(0, 0, 0))
}

#[derive(Debug, Clone)]
struct Mail {
    contact_id: i64,
    campaign_id: i64,
    campaign_name: String,
    campaign_kind: String,
    sent_at: NaiveDateTime,
    opened: bool,
    reply_at: Option<NaiveDateTime>,
    positive: bool,
    variant: Option<String>,
    step: Option<i64>,
    template_id: Option<i64>,
    template_name: Option<String>,
    subject: String,
}

#[derive(Debug, Clone, Default)]
struct ContactInfo {
    category: String,
    city: String,
    country: String,
}

#[derive(Debug, Clone, PartialEq)]
enum Kind {
    Listen,
    Reply,
    Positive,
    Offer,
    Confirmed,
    Collected(f64),
}

#[derive(Debug, Clone)]
struct Outcome {
    contact_id: i64,
    campaign_id: Option<i64>,
    at: NaiveDateTime,
    kind: Kind,
}

fn load_mails(conn: &Connection) -> Result<Vec<Mail>, String> {
    let mut st = conn
        .prepare(
            "SELECT e.contact_id, COALESCE(e.campaign_id, 0), COALESCE(cp.name, ''), COALESCE(cp.kind, 'standard'),
                    e.sent_at,
                    (e.opened_at IS NOT NULL OR COALESCE(e.open_count, 0) > 0),
                    EXISTS (SELECT 1 FROM email_replies r WHERE r.email_id = e.id AND r.kind = 'reply'),
                    (SELECT MIN(COALESCE(r.received_at, r.created_at)) FROM email_replies r
                      WHERE r.email_id = e.id AND r.kind = 'reply'
                        AND NOT EXISTS (SELECT 1 FROM reply_insights i
                                        WHERE i.email_reply_id = r.id AND i.intent = 'out_of_office')),
                    e.replied_at,
                    EXISTS (SELECT 1 FROM email_replies r JOIN reply_insights i ON i.email_reply_id = r.id
                            WHERE r.email_id = e.id AND (i.sentiment = 'positive' OR i.intent = 'interested')),
                    COALESCE(e.subject_variant, m.subject_variant),
                    COALESCE(e.step, m.step),
                    e.template_id, t.name, e.subject
             FROM emails e
             LEFT JOIN campaigns cp ON cp.id = e.campaign_id
             LEFT JOIN campaign_messages m ON m.sent_email_id = e.id
             LEFT JOIN templates t ON t.id = e.template_id
             WHERE e.status = 'sent' AND e.contact_id IS NOT NULL
             ORDER BY e.sent_at, e.id",
        )
        .map_err(|e| e.to_string())?;
    let rows = st
        .query_map([], |r| {
            let sent_at: String = r.get(4)?;
            let has_rows: bool = r.get(6)?;
            let first: Option<String> = r.get(7)?;
            let legacy: Option<String> = r.get(8)?;
            // Replies recorded: the first one that is not an out-of-office.
            // None recorded (older data): the replied_at stamp.
            let reply_at = if has_rows { first } else { legacy };
            Ok((
                sent_at,
                reply_at,
                Mail {
                    contact_id: r.get(0)?,
                    campaign_id: r.get(1)?,
                    campaign_name: r.get(2)?,
                    campaign_kind: r.get(3)?,
                    sent_at: NaiveDateTime::default(),
                    opened: r.get(5)?,
                    reply_at: None,
                    positive: r.get(9)?,
                    variant: r.get(10)?,
                    step: r.get(11)?,
                    template_id: r.get(12)?,
                    template_name: r.get(13)?,
                    subject: r.get(14)?,
                },
            ))
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for row in rows.flatten() {
        let (sent_at, reply_at, mut m) = row;
        let Some(sent) = parse_ts(&sent_at) else { continue };
        m.sent_at = sent;
        m.reply_at = reply_at.as_deref().and_then(parse_ts).filter(|r| *r >= sent);
        out.push(m);
    }
    Ok(out)
}

fn load_contacts(conn: &Connection) -> HashMap<i64, ContactInfo> {
    let mut out = HashMap::new();
    let Ok(mut st) = conn.prepare(
        "SELECT c.id, COALESCE(c.category, ''),
                COALESCE(NULLIF(trim(c.area), ''),
                         (SELECT v.ville FROM vi_venues v WHERE v.crm_contact_id = c.id LIMIT 1), ''),
                COALESCE((SELECT v.pays FROM vi_venues v WHERE v.crm_contact_id = c.id LIMIT 1), '')
         FROM contacts c
         WHERE EXISTS (SELECT 1 FROM emails e WHERE e.contact_id = c.id AND e.status = 'sent')",
    ) else {
        return out;
    };
    if let Ok(rows) = st.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            ContactInfo { category: r.get(1)?, city: r.get::<_, String>(2)?.trim().to_string(), country: r.get::<_, String>(3)?.trim().to_string() },
        ))
    }) {
        for (id, info) in rows.flatten() {
            out.insert(id, info);
        }
    }
    out
}

fn query_outcomes(conn: &Connection, sql: &str, out: &mut Vec<Outcome>, kind: impl Fn(Option<f64>) -> Kind) {
    let Ok(mut st) = conn.prepare(sql) else { return };
    let Ok(rows) = st.query_map([], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, Option<f64>>(3)?))
    }) else {
        return;
    };
    for (cid, camp, at, val) in rows.flatten() {
        if let Some(at) = at.as_deref().and_then(parse_ts) {
            out.push(Outcome { contact_id: cid, campaign_id: camp.filter(|c| *c > 0), at, kind: kind(val) });
        }
    }
}

fn load_outcomes(conn: &Connection, today: NaiveDate) -> Vec<Outcome> {
    let mut out = Vec::new();
    query_outcomes(
        conn,
        "SELECT contact_id, campaign_id, at, NULL FROM press_activity WHERE type = 'listen' AND contact_id IS NOT NULL",
        &mut out,
        |_| Kind::Listen,
    );
    // Replies not tied to an email (matched to the contact only).
    query_outcomes(
        conn,
        "SELECT r.contact_id, NULL, COALESCE(r.received_at, r.created_at), NULL FROM email_replies r
         WHERE r.kind = 'reply' AND r.email_id IS NULL AND r.contact_id IS NOT NULL
           AND NOT EXISTS (SELECT 1 FROM reply_insights i WHERE i.email_reply_id = r.id AND i.intent = 'out_of_office')",
        &mut out,
        |_| Kind::Reply,
    );
    query_outcomes(
        conn,
        "SELECT COALESCE(i.contact_id, r.contact_id), i.campaign_id, COALESCE(r.received_at, i.created_at), NULL
         FROM reply_insights i JOIN email_replies r ON r.id = i.email_reply_id
         WHERE (i.sentiment = 'positive' OR i.intent = 'interested') AND COALESCE(i.contact_id, r.contact_id) IS NOT NULL",
        &mut out,
        |_| Kind::Positive,
    );
    query_outcomes(
        conn,
        "SELECT contact_id, NULL, created_at, NULL FROM events
         WHERE contact_id IS NOT NULL AND status IN ('hold', 'confirmed')",
        &mut out,
        |_| Kind::Offer,
    );
    // An offers table (offers module), when present: any row is an offer.
    query_outcomes(
        conn,
        "SELECT contact_id, NULL, created_at, NULL FROM offers WHERE contact_id IS NOT NULL",
        &mut out,
        |_| Kind::Offer,
    );
    // Confirmation time: the event's last update while confirmed (closest stamp we have).
    query_outcomes(
        conn,
        "SELECT contact_id, NULL, COALESCE(updated_at, created_at), NULL FROM events
         WHERE contact_id IS NOT NULL AND status = 'confirmed'",
        &mut out,
        |_| Kind::Confirmed,
    );
    query_outcomes(
        conn,
        "SELECT c.id, NULL, c.updated_at, NULL FROM contacts c
         WHERE c.status = 'confirmed'
           AND NOT EXISTS (SELECT 1 FROM events e WHERE e.contact_id = c.id AND e.status = 'confirmed')",
        &mut out,
        |_| Kind::Confirmed,
    );
    // Played gigs with a fee: the Budget revenue line when there is one.
    let today_s = today.format("%Y-%m-%d").to_string();
    let sql = format!(
        "SELECT e.contact_id, NULL, e.date, COALESCE(b.actual, e.fee) FROM events e
         LEFT JOIN budget_items b ON b.event_id = e.id AND b.kind = 'revenue'
         WHERE e.contact_id IS NOT NULL AND e.status = 'confirmed'
           AND COALESCE(b.actual, e.fee, 0) > 0 AND substr(e.date, 1, 10) <= '{}'",
        today_s
    );
    query_outcomes(conn, &sql, &mut out, |v| Kind::Collected(v.unwrap_or(0.0)));
    out
}

// ---------------------------------------------------------------------------
// Aggregation (pure)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
struct Pair {
    contact_id: i64,
    campaign_id: i64,
    campaign_name: String,
    campaign_kind: String,
    first_sent: NaiveDateTime,
    opened: bool,
    listened: bool,
    reply_at: Option<NaiveDateTime>,
    positive: bool,
    offer: bool,
    confirmed_at: Option<NaiveDateTime>,
    collected: bool,
    fee: f64,
}

fn min_opt(a: Option<NaiveDateTime>, b: NaiveDateTime) -> Option<NaiveDateTime> {
    Some(a.map_or(b, |a| a.min(b)))
}

fn build_pairs(mails: &[Mail], outcomes: &[Outcome]) -> Vec<Pair> {
    let mut index: HashMap<(i64, i64), usize> = HashMap::new();
    let mut pairs: Vec<Pair> = Vec::new();
    for m in mails {
        let i = *index.entry((m.contact_id, m.campaign_id)).or_insert_with(|| {
            pairs.push(Pair {
                contact_id: m.contact_id,
                campaign_id: m.campaign_id,
                campaign_name: m.campaign_name.clone(),
                campaign_kind: m.campaign_kind.clone(),
                first_sent: m.sent_at,
                ..Default::default()
            });
            pairs.len() - 1
        });
        let p = &mut pairs[i];
        p.first_sent = p.first_sent.min(m.sent_at);
        p.opened |= m.opened;
        if let Some(r) = m.reply_at {
            p.reply_at = min_opt(p.reply_at, r);
        }
        p.positive |= m.positive;
    }
    // Contact -> its pairs, for attribution.
    let mut by_contact: HashMap<i64, Vec<usize>> = HashMap::new();
    for (i, p) in pairs.iter().enumerate() {
        by_contact.entry(p.contact_id).or_default().push(i);
    }
    for o in outcomes {
        let Some(cands) = by_contact.get(&o.contact_id) else { continue };
        let own = o.campaign_id.and_then(|c| cands.iter().copied().find(|&i| pairs[i].campaign_id == c && pairs[i].first_sent <= o.at));
        let target = own.or_else(|| {
            cands.iter().copied().filter(|&i| pairs[i].first_sent <= o.at).max_by_key(|&i| pairs[i].first_sent)
        });
        let Some(i) = target else { continue };
        let p = &mut pairs[i];
        match o.kind {
            Kind::Listen => p.listened = true,
            Kind::Reply => p.reply_at = min_opt(p.reply_at, o.at),
            Kind::Positive => {
                p.positive = true;
                p.reply_at = min_opt(p.reply_at, o.at);
            }
            Kind::Offer => p.offer = true,
            Kind::Confirmed => p.confirmed_at = min_opt(p.confirmed_at, o.at),
            Kind::Collected(fee) => {
                p.collected = true;
                p.fee += fee;
            }
        }
    }
    pairs
}

fn add_pair(s: &mut Stages, p: &Pair) {
    let collected = p.collected;
    let confirmed = p.confirmed_at.is_some() || collected;
    let offer = p.offer || confirmed;
    let positive = p.positive || offer;
    let replied = p.reply_at.is_some() || positive;
    s.sent += 1;
    s.opened += (p.opened || p.listened || replied) as i64;
    s.listened += p.listened as i64;
    s.replied += replied as i64;
    s.positive += positive as i64;
    s.offer += offer as i64;
    s.confirmed += confirmed as i64;
    s.collected += collected as i64;
    s.fee_collected += p.fee;
}

fn add_mail(s: &mut Stages, m: &Mail) {
    let replied = m.reply_at.is_some() || m.positive;
    s.sent += 1;
    s.opened += (m.opened || replied) as i64;
    s.replied += replied as i64;
    s.positive += m.positive as i64;
}

fn median(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = v.len();
    Some(if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2.0 })
}

/// Grouping helper: key -> (label, kind, stages), sorted by sends.
#[derive(Default)]
struct Groups(BTreeMap<String, (String, Option<String>, Stages)>);

impl Groups {
    fn slot(&mut self, key: String, label: &str, kind: Option<&str>) -> &mut Stages {
        &mut self
            .0
            .entry(key)
            .or_insert_with(|| (label.to_string(), kind.map(str::to_string), Stages::default()))
            .2
    }
    fn rows(self, limit: usize, natural: bool) -> Vec<Row> {
        let mut rows: Vec<Row> = self
            .0
            .into_iter()
            .map(|(key, (label, kind, stages))| Row { key, label, kind, reply_rate: stages.reply_rate(), stages })
            .collect();
        if !natural {
            rows.sort_by(|a, b| b.stages.sent.cmp(&a.stages.sent).then(a.label.cmp(&b.label)));
        }
        rows.truncate(limit);
        rows
    }
}

fn norm_key(s: &str) -> String {
    crate::db::normalise(s)
}

pub fn daypart(hour: u32) -> &'static str {
    match hour {
        0..=11 => "matin",
        12..=16 => "apres_midi",
        _ => "soir",
    }
}

fn local_time(country: &str, at: NaiveDateTime) -> NaiveDateTime {
    Tz::for_country(country).to_local(DateTime::<Utc>::from_naive_utc_and_offset(at, Utc))
}

fn in_period(d: NaiveDateTime, from: Option<NaiveDate>, to: Option<NaiveDate>) -> bool {
    from.map_or(true, |f| d.date() >= f) && to.map_or(true, |t| d.date() <= t)
}

fn day(s: &Option<String>) -> Option<NaiveDate> {
    s.as_deref().and_then(|s| NaiveDate::parse_from_str(s.trim().get(..10)?, "%Y-%m-%d").ok())
}

/// The whole report from raw facts. Pure: everything comes in as arguments.
fn aggregate(
    mails: &[Mail],
    contacts: &HashMap<i64, ContactInfo>,
    outcomes: &[Outcome],
    q: &FunnelQuery,
) -> FunnelReport {
    let pairs = build_pairs(mails, outcomes);
    let (from, to) = (day(&q.from), day(&q.to));
    let blank = ContactInfo::default();
    let info = |cid: i64| contacts.get(&cid).unwrap_or(&blank);

    // Filter options over everything (so a filter never hides its own choices).
    let mut options = FilterOptions::default();
    {
        let mut camps: BTreeMap<i64, String> = BTreeMap::new();
        let (mut cats, mut countries, mut cities) = (BTreeMap::new(), BTreeMap::new(), BTreeMap::new());
        for p in &pairs {
            if p.campaign_id > 0 {
                camps.insert(p.campaign_id, p.campaign_name.clone());
            }
            let i = info(p.contact_id);
            for (map, v) in [(&mut cats, &i.category), (&mut countries, &i.country), (&mut cities, &i.city)] {
                if !v.is_empty() {
                    map.entry(norm_key(v)).or_insert_with(|| v.clone());
                }
            }
        }
        options.campaigns = camps.into_iter().collect();
        options.categories = cats.into_values().collect();
        options.countries = countries.into_values().collect();
        options.cities = cities.into_values().collect();
    }

    let matches = |cid: i64, camp: i64, kind: &str| -> bool {
        let i = info(cid);
        q.campaign_id.map_or(true, |c| c == camp)
            && q.kind.as_deref().filter(|k| !k.is_empty()).map_or(true, |k| k == kind)
            && q.category.as_deref().filter(|k| !k.is_empty()).map_or(true, |k| norm_key(k) == norm_key(&i.category))
            && q.country.as_deref().filter(|k| !k.is_empty()).map_or(true, |k| norm_key(k) == norm_key(&i.country))
            && q.city.as_deref().filter(|k| !k.is_empty()).map_or(true, |k| norm_key(k) == norm_key(&i.city))
    };

    let mut rep = FunnelReport { from: q.from.clone(), to: q.to.clone(), options, ..Default::default() };
    let (mut city, mut country, mut camp, mut cat) = (Groups::default(), Groups::default(), Groups::default(), Groups::default());
    let mut reply_hours = Vec::new();
    let mut confirm_days = Vec::new();
    for p in pairs.iter().filter(|p| in_period(p.first_sent, from, to) && matches(p.contact_id, p.campaign_id, &p.campaign_kind)) {
        let i = info(p.contact_id);
        add_pair(&mut rep.total, p);
        add_pair(city.slot(norm_key(&i.city), &i.city, None), p);
        add_pair(country.slot(norm_key(&i.country), &i.country, None), p);
        add_pair(camp.slot(p.campaign_id.to_string(), &p.campaign_name, Some(&p.campaign_kind)), p);
        add_pair(cat.slot(i.category.clone(), &i.category, None), p);
        if let Some(r) = p.reply_at {
            reply_hours.push((r - p.first_sent).num_minutes() as f64 / 60.0);
            if let Some(c) = p.confirmed_at.filter(|c| *c >= r) {
                confirm_days.push((c - r).num_minutes() as f64 / 1440.0);
            }
        }
        if p.listened && p.reply_at.is_none() && !p.positive && !p.offer && p.confirmed_at.is_none() {
            rep.listened_no_reply += 1;
        }
    }
    rep.median_reply_hours = median(reply_hours);
    rep.median_reply_to_confirmed_days = median(confirm_days);
    let t = &rep.total;
    // Chain predecessor of each stage; `listened` is a side branch off `opened`.
    rep.conversions = [
        ("sent", "opened", t.opened, t.sent),
        ("opened", "listened", t.listened, t.opened),
        ("opened", "replied", t.replied, t.opened),
        ("replied", "positive", t.positive, t.replied),
        ("positive", "offer", t.offer, t.positive),
        ("offer", "confirmed", t.confirmed, t.offer),
        ("confirmed", "collected", t.collected, t.confirmed),
    ]
    .iter()
    .map(|(f, to, k, n)| Conversion { from: f.to_string(), to: to.to_string(), rate: rate(*k, *n) })
    .collect();
    rep.by_city = city.rows(30, false);
    rep.by_country = country.rows(30, false);
    rep.by_campaign = camp.rows(50, false);
    rep.by_category = cat.rows(20, false);

    // Per email. Position in its pair = step when the email has none (bulk).
    let mut position: HashMap<(i64, i64), i64> = HashMap::new();
    let (mut step, mut tpl, mut wd, mut hr, mut slot) =
        (Groups::default(), Groups::default(), Groups::default(), Groups::default(), Groups::default());
    let mut vgroups: BTreeMap<(i64, i64, i64), (Mail, BTreeMap<String, (String, Stages)>)> = BTreeMap::new();
    for m in mails {
        let pos = position.entry((m.contact_id, m.campaign_id)).or_insert(0);
        *pos += 1;
        if !in_period(m.sent_at, from, to) || !matches(m.contact_id, m.campaign_id, &m.campaign_kind) {
            continue;
        }
        let i = info(m.contact_id);
        let n = m.step.unwrap_or(*pos);
        add_mail(step.slot(format!("{:03}", n), &n.to_string(), None), m);
        let (tk, tl, tkind) = match (m.template_id, &m.template_name) {
            (Some(id), name) => (format!("t{}", id), name.clone().unwrap_or_default(), "template"),
            (None, _) if m.campaign_kind == "residency" => ("residency".to_string(), String::new(), "residency"),
            _ => (String::new(), String::new(), "none"),
        };
        add_mail(tpl.slot(tk, &tl, Some(tkind)), m);
        let local = local_time(&i.country, m.sent_at);
        let (w, h) = (local.weekday().number_from_monday(), local.hour());
        add_mail(wd.slot(w.to_string(), &w.to_string(), None), m);
        add_mail(hr.slot(format!("{:02}", h), &h.to_string(), None), m);
        let part = daypart(h);
        add_mail(slot.slot(format!("{}-{}", w, part), &format!("{}-{}", w, part), None), m);
        if let Some(v) = m.variant.as_deref().filter(|v| !v.is_empty()) {
            let scope = if m.campaign_kind == "residency" { m.step.unwrap_or(n) } else { 0 };
            let key = (m.campaign_id, m.template_id.unwrap_or(0), scope);
            let g = vgroups.entry(key).or_insert_with(|| (m.clone(), BTreeMap::new()));
            let arm = g.1.entry(v.to_string()).or_insert_with(|| (m.subject.clone(), Stages::default()));
            add_mail(&mut arm.1, m);
        }
    }
    rep.by_step = step.rows(20, true);
    rep.by_template = tpl.rows(30, false);
    rep.by_weekday = wd.rows(7, true);
    rep.by_hour = hr.rows(24, true);
    rep.by_slot = slot.rows(21, true);
    rep.variants = vgroups
        .into_iter()
        .map(|((camp_id, tpl_id, scope), (m, arms))| {
            let arms: Vec<Arm> = arms
                .into_iter()
                .map(|(label, (example, s))| Arm {
                    label,
                    example,
                    reply_rate: s.reply_rate(),
                    sent: s.sent,
                    opened: s.opened,
                    replied: s.replied,
                    positive: s.positive,
                })
                .collect();
            let (verdict, p_value, best) = verdict(&arms);
            let residency = m.campaign_kind == "residency";
            VariantGroup {
                campaign_id: camp_id,
                campaign_name: m.campaign_name.clone(),
                kind: m.campaign_kind.clone(),
                template_id: (tpl_id > 0).then_some(tpl_id),
                template_name: m.template_name.clone(),
                step: (scope > 0).then_some(scope),
                can_keep: (residency && scope > 0) || (!residency && tpl_id > 0),
                arms,
                verdict,
                p_value,
                best,
            }
        })
        .collect();
    rep
}

/// Verdict of a test on reply rates: best arm vs the worst one.
pub fn verdict(arms: &[Arm]) -> (String, Option<f64>, Option<String>) {
    if arms.len() < 2 || arms.iter().any(|a| a.sent < MIN_ARM) {
        return ("insufficient".into(), None, None);
    }
    let r = |a: &Arm| a.replied as f64 / a.sent.max(1) as f64;
    let best = arms.iter().max_by(|a, b| r(a).partial_cmp(&r(b)).unwrap_or(std::cmp::Ordering::Equal)).unwrap();
    let worst = arms.iter().min_by(|a, b| r(a).partial_cmp(&r(b)).unwrap_or(std::cmp::Ordering::Equal)).unwrap();
    let p = two_proportion_p(best.replied, best.sent, worst.replied, worst.sent);
    let v = match p {
        Some(p) if p < 0.05 => "significant",
        Some(p) if p < 0.2 => "trend",
        _ => "none",
    };
    let best_label = (v != "none").then(|| best.label.clone());
    (v.into(), p, best_label)
}

/// The report straight from the database.
pub fn report(conn: &Connection, q: &FunnelQuery, today: NaiveDate) -> Result<FunnelReport, String> {
    let mails = load_mails(conn)?;
    let contacts = load_contacts(conn);
    let outcomes = load_outcomes(conn, today);
    Ok(aggregate(&mails, &contacts, &outcomes, q))
}

// ---------------------------------------------------------------------------
// Suggestions (pure, from a report)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Suggestion {
    /// best_slot | dead_city | weak_city | variant_winner | listened_no_reply |
    /// category_gap | followups_work | dead_step | opens_no_replies | need_data
    pub code: String,
    /// Values for the translated sentence (numbers already formatted).
    pub values: BTreeMap<String, String>,
    /// French sentence (digest email, fallback).
    pub text: String,
    #[serde(skip)]
    pub score: f64,
}

const WEEKDAYS_FR: [&str; 7] = ["lundi", "mardi", "mercredi", "jeudi", "vendredi", "samedi", "dimanche"];

fn daypart_fr(code: &str) -> &'static str {
    match code {
        "matin" => "matin",
        "apres_midi" => "après-midi",
        _ => "soir",
    }
}

fn category_fr(cat: &str) -> String {
    match cat {
        "venue" => "clubs".into(),
        "promoter" => "promoteurs".into(),
        "curator" => "curateurs".into(),
        "lineup" => "line-ups".into(),
        "major" => "labels".into(),
        "" => "contacts sans catégorie".into(),
        other => other.to_string(),
    }
}

fn pct(r: f64) -> String {
    format!("{:.0}", r * 100.0)
}

fn times(x: f64) -> String {
    format!("{:.1}", x).replace('.', ",")
}

fn sugg(code: &str, score: f64, text: String, values: &[(&str, String)]) -> Suggestion {
    Suggestion {
        code: code.into(),
        values: values.iter().map(|(k, v)| (k.to_string(), v.clone())).collect(),
        text,
        score,
    }
}

/// Up to `max` concrete suggestions, strongest first.
pub fn suggestions(rep: &FunnelReport, max: usize) -> Vec<Suggestion> {
    let mut out: Vec<Suggestion> = Vec::new();
    // Per-email reply rate overall (the slots are per email).
    let (mut sent, mut replied) = (0i64, 0i64);
    for r in &rep.by_weekday {
        sent += r.stages.sent;
        replied += r.stages.replied;
    }
    let overall = rate(replied, sent).unwrap_or(0.0);

    // 1. A send slot that clearly answers better.
    if overall > 0.0 {
        if let Some(best) = rep
            .by_slot
            .iter()
            .filter(|r| r.stages.sent >= 15 && r.stages.replied >= 3)
            .max_by(|a, b| a.reply_rate.partial_cmp(&b.reply_rate).unwrap_or(std::cmp::Ordering::Equal))
        {
            let ratio = best.reply_rate.unwrap_or(0.0) / overall;
            if ratio >= 1.5 {
                let (w, part) = best.key.split_once('-').unwrap_or(("1", "matin"));
                let wname = w.parse::<usize>().ok().and_then(|w| WEEKDAYS_FR.get(w.wrapping_sub(1))).copied().unwrap_or("lundi");
                out.push(sugg(
                    "best_slot",
                    ratio,
                    format!(
                        "Les envois du {} {} répondent {}x plus que la moyenne ({} réponses sur {} envois) : programmer les prochains à ce moment.",
                        wname, daypart_fr(part), times(ratio), best.stages.replied, best.stages.sent
                    ),
                    &[("weekday", w.into()), ("part", part.into()), ("ratio", times(ratio)),
                      ("replied", best.stages.replied.to_string()), ("sent", best.stages.sent.to_string())],
                ));
            }
        }
    }

    // 2. Cities that do not answer.
    let total_rate = rep.total.reply_rate().unwrap_or(0.0);
    for c in rep.by_city.iter().filter(|c| !c.label.is_empty()) {
        if c.stages.sent >= 20 && c.stages.replied == 0 {
            out.push(sugg(
                "dead_city",
                1.0 + c.stages.sent as f64 / 10.0,
                format!("{} : 0 réponse sur {}, revoir le pitch ou la cible.", c.label, c.stages.sent),
                &[("city", c.label.clone()), ("sent", c.stages.sent.to_string())],
            ));
        } else if c.stages.sent >= 25 && total_rate > 0.0 && c.reply_rate.unwrap_or(0.0) <= total_rate * 0.3 {
            out.push(sugg(
                "weak_city",
                1.0 + c.stages.sent as f64 / 20.0,
                format!(
                    "{} : {} % de réponses contre {} % en moyenne, revoir le pitch.",
                    c.label, pct(c.reply_rate.unwrap_or(0.0)), pct(total_rate)
                ),
                &[("city", c.label.clone()), ("rate", pct(c.reply_rate.unwrap_or(0.0))), ("avg", pct(total_rate))],
            ));
        }
    }

    // 3. A subject line that wins.
    for g in rep.variants.iter().filter(|g| g.verdict == "significant") {
        let Some(best) = g.best.as_deref().and_then(|b| g.arms.iter().find(|a| a.label == b)) else { continue };
        let Some(worst) = g.arms.iter().filter(|a| a.label != best.label).min_by(|a, b| {
            a.reply_rate.partial_cmp(&b.reply_rate).unwrap_or(std::cmp::Ordering::Equal)
        }) else {
            continue;
        };
        let name = if g.campaign_name.is_empty() { "envoi groupé".to_string() } else { g.campaign_name.clone() };
        out.push(sugg(
            "variant_winner",
            4.0,
            format!(
                "Objet {} gagnant sur « {} » ({} % de réponses contre {} %) : garder la meilleure.",
                best.label, name, pct(best.reply_rate.unwrap_or(0.0)), pct(worst.reply_rate.unwrap_or(0.0))
            ),
            &[("label", best.label.clone()), ("campaign", name), ("best", pct(best.reply_rate.unwrap_or(0.0))),
              ("other", pct(worst.reply_rate.unwrap_or(0.0)))],
        ));
    }

    // 4. Warm leads: they listened, they did not answer.
    if rep.listened_no_reply > 0 {
        let n = rep.listened_no_reply;
        out.push(sugg(
            "listened_no_reply",
            2.5 + n as f64 / 5.0,
            format!("{} contact(s) ont écouté l'EPK sans répondre : une relance perso maintenant.", n),
            &[("count", n.to_string())],
        ));
    }

    // 5. Categories far apart.
    let cats: Vec<&Row> = rep.by_category.iter().filter(|r| r.stages.sent >= 15).collect();
    if cats.len() >= 2 {
        let by = |a: &&Row, b: &&Row| a.reply_rate.partial_cmp(&b.reply_rate).unwrap_or(std::cmp::Ordering::Equal);
        let hi = cats.iter().copied().max_by(by).unwrap();
        let lo = cats.iter().copied().min_by(by).unwrap();
        let (rh, rl) = (hi.reply_rate.unwrap_or(0.0), lo.reply_rate.unwrap_or(0.0));
        if rh > 0.0 && (rl == 0.0 || rh / rl >= 2.0) && hi.key != lo.key {
            let ratio = if rl > 0.0 { times(rh / rl) } else { String::new() };
            let text = if rl > 0.0 {
                format!("Les {} répondent {}x plus que les {} : leur consacrer plus d'envois.", category_fr(&hi.key), ratio, category_fr(&lo.key))
            } else {
                format!("Les {} répondent ({} %), les {} jamais : leur consacrer plus d'envois.", category_fr(&hi.key), pct(rh), category_fr(&lo.key))
            };
            out.push(sugg(
                "category_gap",
                1.5 + if rl > 0.0 { rh / rl / 2.0 } else { 1.0 },
                text,
                &[("best", hi.key.clone()), ("worst", lo.key.clone()), ("ratio", ratio), ("rate", pct(rh))],
            ));
        }
    }

    // 6. Sequence steps.
    let step_replies: i64 = rep.by_step.iter().map(|r| r.stages.replied).sum();
    if step_replies >= 5 {
        let later: i64 = rep.by_step.iter().filter(|r| r.label != "1").map(|r| r.stages.replied).sum();
        let share = later as f64 / step_replies as f64;
        if share >= 0.3 {
            out.push(sugg(
                "followups_work",
                1.2 + share,
                format!("Les relances apportent {} % des réponses : garder la séquence complète.", pct(share)),
                &[("share", pct(share))],
            ));
        }
    }
    for r in rep.by_step.iter().filter(|r| r.label != "1" && r.stages.sent >= 30 && r.stages.replied == 0) {
        out.push(sugg(
            "dead_step",
            1.0 + r.stages.sent as f64 / 30.0,
            format!("L'étape {} ne rapporte aucune réponse sur {} envois : la raccourcir ou la retirer.", r.label, r.stages.sent),
            &[("step", r.label.clone()), ("sent", r.stages.sent.to_string())],
        ));
    }

    // 7. Opened a lot, answered little.
    if sent >= 30 {
        let opened: i64 = rep.by_weekday.iter().map(|r| r.stages.opened).sum();
        let (o, r) = (opened as f64 / sent as f64, overall);
        if o >= 0.4 && r < 0.05 {
            out.push(sugg(
                "opens_no_replies",
                1.0 + o,
                format!(
                    "Beaucoup d'ouvertures ({} %) mais peu de réponses ({} %) : l'objet accroche, le message doit demander moins.",
                    pct(o), pct(r)
                ),
                &[("open", pct(o)), ("reply", pct(r))],
            ));
        }
    }

    out.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    // One suggestion per kind, at most.
    let mut seen = std::collections::HashSet::new();
    out.retain(|s| seen.insert(s.code.clone()));
    out.truncate(max);
    if out.is_empty() {
        out.push(sugg(
            "need_data",
            0.0,
            format!("Pas encore assez d'envois pour conclure ({} sur la période) : viser 30 envois par piste testée.", sent),
            &[("sent", sent.to_string())],
        ));
    }
    out
}

// ---------------------------------------------------------------------------
// Weekly digest
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Activity {
    pub sent: i64,
    pub replies: i64,
    pub positive: i64,
    pub offers: i64,
    pub confirmed: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Digest {
    pub week_start: String,
    pub week_end: String,
    /// Prospects first contacted that week.
    pub cohort: Stages,
    /// What happened that week, whatever the send date.
    pub activity: Activity,
    /// Over the last eight weeks, campaigns with at least 10 prospects.
    pub best_campaign: Option<Row>,
    pub worst_campaign: Option<Row>,
    /// Tests over the last eight weeks, decided ones first.
    pub variants: Vec<VariantGroup>,
    pub suggestions: Vec<Suggestion>,
    pub generated_at: String,
    #[serde(default)]
    pub emailed_at: Option<String>,
}

fn count(conn: &Connection, sql: &str, a: &str, b: &str) -> i64 {
    conn.query_row(sql, params![a, b], |r| r.get(0)).unwrap_or(0)
}

fn activity(conn: &Connection, start: NaiveDate, end: NaiveDate) -> Activity {
    let a = start.format("%Y-%m-%d 00:00:00").to_string();
    let b = (end + Duration::days(1)).format("%Y-%m-%d 00:00:00").to_string();
    Activity {
        sent: count(conn, "SELECT COUNT(*) FROM emails WHERE status = 'sent' AND contact_id IS NOT NULL AND sent_at >= ?1 AND sent_at < ?2", &a, &b),
        replies: count(
            conn,
            "SELECT COUNT(*) FROM email_replies r WHERE r.kind = 'reply'
               AND replace(substr(COALESCE(r.received_at, r.created_at), 1, 19), 'T', ' ') >= ?1
               AND replace(substr(COALESCE(r.received_at, r.created_at), 1, 19), 'T', ' ') < ?2
               AND NOT EXISTS (SELECT 1 FROM reply_insights i WHERE i.email_reply_id = r.id AND i.intent = 'out_of_office')",
            &a,
            &b,
        ),
        positive: count(
            conn,
            "SELECT COUNT(*) FROM reply_insights i JOIN email_replies r ON r.id = i.email_reply_id
             WHERE (i.sentiment = 'positive' OR i.intent = 'interested')
               AND replace(substr(COALESCE(r.received_at, i.created_at), 1, 19), 'T', ' ') >= ?1
               AND replace(substr(COALESCE(r.received_at, i.created_at), 1, 19), 'T', ' ') < ?2",
            &a,
            &b,
        ),
        offers: count(conn, "SELECT COUNT(*) FROM events WHERE contact_id IS NOT NULL AND status IN ('hold','confirmed') AND created_at >= ?1 AND created_at < ?2", &a, &b),
        confirmed: count(conn, "SELECT COUNT(*) FROM events WHERE status = 'confirmed' AND COALESCE(updated_at, created_at) >= ?1 AND COALESCE(updated_at, created_at) < ?2", &a, &b),
    }
}

/// Monday of the week before `today`'s.
pub fn last_week_start(today: NaiveDate) -> NaiveDate {
    today - Duration::days(today.weekday().num_days_from_monday() as i64 + 7)
}

fn range_query(start: NaiveDate, end: NaiveDate) -> FunnelQuery {
    FunnelQuery {
        from: Some(start.format("%Y-%m-%d").to_string()),
        to: Some(end.format("%Y-%m-%d").to_string()),
        ..Default::default()
    }
}

pub fn build_digest(conn: &Connection, week_start: NaiveDate, now: NaiveDateTime) -> Result<Digest, String> {
    let week_end = week_start + Duration::days(6);
    let today = now.date();
    let mails = load_mails(conn)?;
    let contacts = load_contacts(conn);
    let outcomes = load_outcomes(conn, today);
    let week = aggregate(&mails, &contacts, &outcomes, &range_query(week_start, week_end));
    let eight = aggregate(&mails, &contacts, &outcomes, &range_query(week_end - Duration::days(55), week_end));
    let ninety = aggregate(&mails, &contacts, &outcomes, &range_query(week_end - Duration::days(89), week_end));

    let ranked: Vec<&Row> = eight.by_campaign.iter().filter(|r| r.key != "0" && r.stages.sent >= 10).collect();
    let by = |a: &&&Row, b: &&&Row| {
        a.reply_rate
            .partial_cmp(&b.reply_rate)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.stages.positive.cmp(&b.stages.positive))
    };
    let best = ranked.iter().max_by(by).map(|r| (*r).clone());
    let worst = if ranked.len() >= 2 { ranked.iter().min_by(by).map(|r| (*r).clone()) } else { None };
    let mut variants = eight.variants.clone();
    let order = |v: &str| match v {
        "significant" => 0,
        "trend" => 1,
        "none" => 2,
        _ => 3,
    };
    variants.sort_by_key(|g| (order(&g.verdict), -g.arms.iter().map(|a| a.sent).sum::<i64>()));
    variants.truncate(4);
    Ok(Digest {
        week_start: week_start.format("%Y-%m-%d").to_string(),
        week_end: week_end.format("%Y-%m-%d").to_string(),
        cohort: week.total,
        activity: activity(conn, week_start, week_end),
        best_campaign: best,
        worst_campaign: worst,
        variants,
        suggestions: suggestions(&ninety, 3),
        generated_at: now.format("%Y-%m-%d %H:%M:%S").to_string(),
        emailed_at: None,
    })
}

pub fn store_digest(conn: &Connection, d: &Digest) -> Result<(), String> {
    let json = serde_json::to_string(d).map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO weekly_digests (week_start, data) VALUES (?1, ?2)
         ON CONFLICT(week_start) DO UPDATE SET data = excluded.data",
        params![d.week_start, json],
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

pub fn load_digest(conn: &Connection, week_start: &str) -> Option<Digest> {
    conn.query_row(
        "SELECT data, emailed_at FROM weekly_digests WHERE week_start = ?1",
        params![week_start],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)),
    )
    .optional()
    .ok()
    .flatten()
    .and_then(|(data, emailed)| {
        let mut d: Digest = serde_json::from_str(&data).ok()?;
        d.emailed_at = emailed;
        Some(d)
    })
}

/// Monday, first tick from 08:00 (local): build and store last week's digest
/// once. Returns it when it was just built.
pub fn weekly_tick(conn: &Connection, local_now: NaiveDateTime) -> Option<Digest> {
    if local_now.weekday() != chrono::Weekday::Mon || local_now.hour() < 8 {
        return None;
    }
    let start = last_week_start(local_now.date());
    let key = start.format("%Y-%m-%d").to_string();
    if load_digest(conn, &key).is_some() {
        return None;
    }
    let d = build_digest(conn, start, local_now).ok()?;
    store_digest(conn, &d).ok()?;
    Some(d)
}

fn fr_day(s: &str) -> String {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").map(|d| d.format("%d/%m").to_string()).unwrap_or_else(|_| s.to_string())
}

/// Plain-text version of a digest (email). French, no em dash.
pub fn digest_text(d: &Digest) -> (String, String) {
    let subject = format!("Point du lundi, semaine du {} au {}", fr_day(&d.week_start), fr_day(&d.week_end));
    let c = &d.cohort;
    let a = &d.activity;
    let mut b = String::new();
    b.push_str(&format!("Semaine du {} au {}\n\n", fr_day(&d.week_start), fr_day(&d.week_end)));
    b.push_str("Cette semaine\n");
    b.push_str(&format!(
        "- {} email(s) envoyé(s), {} réponse(s) reçue(s) dont {} positive(s)\n- {} offre(s), {} date(s) confirmée(s)\n\n",
        a.sent, a.replies, a.positive, a.offers, a.confirmed
    ));
    b.push_str("Prospects contactés pour la première fois\n");
    b.push_str(&format!(
        "- {} contacté(s), {} ouvert(s), {} écoute(s) EPK, {} réponse(s), {} positive(s), {} offre(s), {} confirmé(s)\n\n",
        c.sent, c.opened, c.listened, c.replied, c.positive, c.offer, c.confirmed
    ));
    let line = |r: &Row| {
        format!(
            "{} : {} % de réponses ({} sur {})",
            if r.label.is_empty() { "sans campagne" } else { &r.label },
            pct(r.reply_rate.unwrap_or(0.0)),
            r.stages.replied,
            r.stages.sent
        )
    };
    if let Some(r) = &d.best_campaign {
        b.push_str(&format!("Meilleure campagne (8 semaines) : {}\n", line(r)));
    }
    if let Some(r) = &d.worst_campaign {
        b.push_str(&format!("Campagne à revoir : {}\n", line(r)));
    }
    if !d.variants.is_empty() {
        b.push_str("\nTests d'objet\n");
        for g in &d.variants {
            let arms: Vec<String> = g
                .arms
                .iter()
                .map(|a| format!("{} {} % ({}/{})", a.label, pct(a.reply_rate.unwrap_or(0.0)), a.replied, a.sent))
                .collect();
            let v = match g.verdict.as_str() {
                "significant" => "écart net",
                "trend" => "tendance",
                "none" => "pas de différence nette",
                _ => "pas assez de données",
            };
            let name = if g.campaign_name.is_empty() { "envoi groupé" } else { &g.campaign_name };
            b.push_str(&format!("- {} : {} ({})\n", name, arms.join(", "), v));
        }
    }
    b.push_str("\nÀ faire\n");
    for (i, s) in d.suggestions.iter().enumerate() {
        b.push_str(&format!("{}. {}\n", i + 1, s.text));
    }
    (subject.replace('—', ","), b.replace('—', ","))
}

/// Email the digest to the sender's own address (never anyone else). Only
/// called when the `funnel_digest_email` setting is "1".
fn email_digest(pool: &crate::db::DbPool, d: &Digest) -> Result<(), String> {
    use lettre::message::Mailbox;
    use lettre::Transport;
    let cfg = crate::commands::email::load_smtp_from(pool)?;
    let own: Mailbox = cfg.from_email.trim().parse().map_err(|_| "Adresse d'expédition invalide".to_string())?;
    let (subject, body) = digest_text(d);
    let msg = lettre::Message::builder()
        .from(own.clone())
        .to(own)
        .subject(subject)
        .body(body)
        .map_err(|e| e.to_string())?;
    let transport = crate::commands::email::build_transport(&cfg)?;
    transport.send(&msg).map(|_| ()).map_err(|e| e.to_string())
}

/// Called by the residency engine tick: build the Monday digest and, when
/// enabled, email it to the sender's own address.
pub fn on_tick(pool: &crate::db::DbPool, local_now: NaiveDateTime) {
    let Ok(conn) = pool.get() else { return };
    let Some(d) = weekly_tick(&conn, local_now) else { return };
    let enabled = conn
        .query_row("SELECT value FROM settings WHERE key = 'funnel_digest_email'", [], |r| r.get::<_, Option<String>>(0))
        .ok()
        .flatten()
        .is_some_and(|v| v.trim() == "1");
    if !enabled {
        return;
    }
    drop(conn);
    match email_digest(pool, &d) {
        Ok(()) => {
            if let Ok(conn) = pool.get() {
                let _ = conn.execute(
                    "UPDATE weekly_digests SET emailed_at = datetime('now') WHERE week_start = ?1",
                    params![d.week_start],
                );
            }
        }
        Err(e) => eprintln!("[funnel] digest email: {}", e),
    }
}

// ---------------------------------------------------------------------------
// Keep the best
// ---------------------------------------------------------------------------

/// Make `label` the only subject of the test. Residency: the step's subject
/// (FR and EN) becomes that variant and the variants are cleared. Bulk: the
/// template's subject becomes that arm's raw subject.
pub fn keep_variant(conn: &Connection, campaign_id: i64, template_id: Option<i64>, step: Option<i64>, label: &str) -> Result<String, String> {
    let idx = label_index(label).ok_or("Variante inconnue")?;
    if let (Some(step), Some(mut r)) = (step, crate::commands::residency::load(conn, campaign_id)) {
        let s = r
            .config
            .steps
            .get_mut((step - 1).max(0) as usize)
            .ok_or("Étape introuvable")?;
        let fr = clean_variants(&s.subject, &s.subject_variants);
        let en = clean_variants(&s.subject_en, &s.subject_variants_en);
        let chosen = fr.get(idx).cloned().ok_or("Variante introuvable")?;
        s.subject = chosen.clone();
        if let Some(e) = en.get(idx) {
            s.subject_en = e.clone();
        }
        s.subject_variants.clear();
        s.subject_variants_en.clear();
        crate::commands::residency::save_config(conn, campaign_id, &r.config)?;
        return Ok(chosen);
    }
    let tpl = template_id.ok_or("Aucun modèle à mettre à jour pour cet envoi")?;
    let subject: String = conn
        .query_row(
            "SELECT subject FROM ab_subjects WHERE campaign_id = ?1 AND template_id = ?2 AND step = 0 AND label = ?3",
            params![campaign_id, tpl, label],
            |r| r.get(0),
        )
        .map_err(|_| "Objet de la variante introuvable".to_string())?;
    let n = conn
        .execute("UPDATE templates SET subject = ?2 WHERE id = ?1", params![tpl, subject])
        .map_err(|e| e.to_string())?;
    if n == 0 {
        return Err("Modèle introuvable".into());
    }
    Ok(subject)
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

fn local_today() -> NaiveDate {
    chrono::Local::now().date_naive()
}

#[tauri::command(async)]
pub fn funnel_report(state: State<'_, AppState>, query: Option<FunnelQuery>) -> Result<FunnelReport, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    report(&conn, &query.unwrap_or_default(), local_today())
}

#[tauri::command(async)]
pub fn funnel_keep_variant(
    state: State<'_, AppState>,
    campaign_id: i64,
    template_id: Option<i64>,
    step: Option<i64>,
    label: String,
) -> Result<String, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    keep_variant(&conn, campaign_id, template_id, step, &label)
}

/// Last week's digest: the stored one, built on demand when missing (or
/// rebuilt with `refresh`).
#[tauri::command(async)]
pub fn funnel_digest(state: State<'_, AppState>, refresh: Option<bool>) -> Result<Digest, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let now = chrono::Local::now().naive_local();
    let start = last_week_start(now.date());
    let key = start.format("%Y-%m-%d").to_string();
    if !refresh.unwrap_or(false) {
        if let Some(d) = load_digest(&conn, &key) {
            return Ok(d);
        }
    }
    let mut d = build_digest(&conn, start, now)?;
    store_digest(&conn, &d)?;
    d.emailed_at = load_digest(&conn, &key).and_then(|x| x.emailed_at);
    Ok(d)
}

// ---------------------------------------------------------------------------
// Tests (in-memory database with the real schema)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::residency::testdb;

    fn ts(s: &str) -> NaiveDateTime {
        parse_ts(s).unwrap()
    }

    /// Two campaigns (one residency), five contacts in three cities.
    fn fixture() -> Connection {
        let c = testdb::conn();
        c.execute_batch(
            "INSERT INTO campaigns (id, name, kind) VALUES (1, 'Clubs Paris', 'standard'), (2, 'Résidence Londres', 'residency');
             INSERT INTO templates (id, name, subject, body) VALUES (7, 'Pitch court', 'Hello {{venue}}', 'b');
             INSERT INTO contacts (id, name, category, area, status) VALUES
               (10, 'Club A', 'venue', 'Paris', 'confirmed'),
               (11, 'Club B', 'venue', 'Paris', 'contacted'),
               (12, 'Promo C', 'promoter', 'Lyon', 'in_discussion'),
               (13, 'Club D', 'venue', 'London', 'contacted'),
               (14, 'Club E', 'venue', 'London', 'contacted');
             INSERT INTO vi_venues (nom, nom_normalise, ville, pays, crm_contact_id) VALUES
               ('Club D', 'clubd', 'London', 'GB', 13), ('Club A', 'cluba', 'Paris', 'FR', 10);
             -- Campaign 1 (bulk, template 7, A/B): A to 10 and 12, B to 11.
             INSERT INTO emails (id, contact_id, campaign_id, to_addr, subject, body, status, sent_at, opened_at, template_id, subject_variant, replied_at) VALUES
               (1, 10, 1, 'a@x.fr', 'Hello Club A', 'b', 'sent', '2026-10-06 08:00:00', '2026-10-06 09:00:00', 7, 'A', '2026-10-07 08:00:00'),
               (2, 11, 1, 'b@x.fr', 'Salut Club B', 'b', 'sent', '2026-10-06 08:05:00', NULL, 7, 'B', NULL),
               (3, 12, 1, 'c@x.fr', 'Hello Promo C', 'b', 'sent', '2026-10-07 14:00:00', NULL, 7, 'A', NULL),
               (4, 11, 1, 'b@x.fr', 'Re: Salut Club B', 'b', 'sent', '2026-10-13 08:00:00', NULL, NULL, NULL, NULL),
               -- failed send: never counted
               (5, 13, 1, 'd@x.co.uk', 'x', 'b', 'failed', '2026-10-06 08:00:00', NULL, NULL, NULL, NULL);
             -- Campaign 2 (residency): 13 and 14, step 1.
             INSERT INTO emails (id, contact_id, campaign_id, to_addr, subject, body, status, sent_at) VALUES
               (6, 13, 2, 'd@x.co.uk', 'Residency D', 'b', 'sent', '2026-10-08 10:00:00'),
               (7, 14, 2, 'e@x.co.uk', 'Residency E', 'b', 'sent', '2026-10-08 10:30:00');
             INSERT INTO campaign_messages (campaign_id, contact_id, step, subject, body, status, sent_email_id, subject_variant) VALUES
               (2, 13, 1, 'Residency D', 'b', 'sent', 6, 'B'),
               (2, 14, 1, 'Residency E', 'b', 'sent', 7, 'A');
             -- Replies: 10 real reply (positive), 12 replied by matching the contact only,
             -- 13 out-of-office only.
             INSERT INTO email_replies (id, email_id, contact_id, from_addr, message_id, kind, received_at) VALUES
               (1, 1, 10, 'a@x.fr', '<r1>', 'reply', '2026-10-07 08:00:00'),
               (2, NULL, 12, 'c@x.fr', '<r2>', 'reply', '2026-10-09 14:00:00'),
               (3, 6, 13, 'd@x.co.uk', '<r3>', 'reply', '2026-10-08 11:00:00');
             INSERT INTO reply_insights (email_reply_id, contact_id, campaign_id, sentiment, intent) VALUES
               (1, 10, 1, 'positive', 'interested'),
               (3, 13, 2, 'neutral', 'out_of_office');
             -- EPK: 14 listened (no reply), 13 listened before any send (ignored? no, after).
             INSERT INTO press_activity (event_id, contact_id, campaign_id, type, at, seconds) VALUES
               ('l1', 14, 2, 'listen', '2026-10-09T10:00:00Z', 200),
               ('l0', 13, NULL, 'listen', '2026-09-01T10:00:00Z', 30);
             -- Club A: offer then confirmed gig, played, fee via the Budget line.
             INSERT INTO events (id, contact_id, title, date, status, fee, created_at, updated_at) VALUES
               (1, 10, 'Club A night', '2026-10-20', 'confirmed', 300, '2026-10-08 10:00:00', '2026-10-10 08:00:00'),
               -- an old event before any email: not part of the funnel
               (2, 11, 'Old gig', '2026-01-10', 'confirmed', 100, '2026-01-01 10:00:00', '2026-01-01 10:00:00');
             UPDATE budget_items SET actual = 350 WHERE event_id = 1;
             INSERT OR IGNORE INTO budget_items (item, kind, actual, event_id) VALUES ('Club A', 'revenue', 350, 1);",
        )
        .unwrap();
        c
    }

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 30).unwrap()
    }

    #[test]
    fn migrate_is_idempotent() {
        let c = testdb::conn();
        migrate(&c);
        migrate(&c);
        c.execute("INSERT INTO emails (to_addr, subject, body, subject_variant, template_id, step) VALUES ('a','b','c','A',1,2)", [])
            .unwrap();
    }

    #[test]
    fn variants_are_deterministic_and_balanced() {
        assert_eq!(clean_variants(" Hello ", &["".into(), "Hello".into(), "Hi".into(), "Yo".into(), "Z".into()]), vec!["Hello", "Hi", "Yo"]);
        assert!(pick(1, 0, &["x".to_string()]).is_none());
        let v = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        for cid in 0..50 {
            assert_eq!(pick(cid, 7, &v), pick(cid, 7, &v));
        }
        let mut counts = [0; 3];
        for cid in 1..=3000 {
            counts[variant_index(cid, salt(Some(4), 1), 3)] += 1;
        }
        for n in counts {
            assert!((850..=1150).contains(&n), "{:?}", counts);
        }
        // Another salt reshuffles recipients.
        let same = (1..=200).filter(|&c| variant_index(c, 1, 2) == variant_index(c, 2, 2)).count();
        assert!(same < 150);
    }

    #[test]
    fn significance() {
        let arm = |l: &str, s, r| Arm { label: l.into(), example: String::new(), sent: s, opened: 0, replied: r, positive: 0, reply_rate: rate(r, s) };
        assert_eq!(verdict(&[arm("A", 29, 10), arm("B", 100, 1)]).0, "insufficient");
        let (v, p, best) = verdict(&[arm("A", 100, 20), arm("B", 100, 5)]);
        assert_eq!(v, "significant");
        assert!(p.unwrap() < 0.01);
        assert_eq!(best.as_deref(), Some("A"));
        assert_eq!(verdict(&[arm("A", 40, 4), arm("B", 40, 5)]).0, "none");
        let p = two_proportion_p(20, 100, 5, 100).unwrap();
        assert!((p - 0.0016).abs() < 0.001, "{}", p);
    }

    #[test]
    fn funnel_counts_attribution_and_delays() {
        let c = fixture();
        let r = report(&c, &FunnelQuery::default(), today()).unwrap();
        // Pairs: (10,1) (11,1) (12,1) (13,2) (14,2); the failed send is ignored.
        let t = &r.total;
        assert_eq!(t.sent, 5);
        // 10 opened; 12 replied (implies opened); 14 listened (implies opened).
        assert_eq!(t.opened, 3);
        assert_eq!(t.listened, 1, "the listen before any email does not count");
        // 10 (email reply) + 12 (contact-matched reply); 13 is only an out-of-office.
        assert_eq!(t.replied, 2);
        assert_eq!(t.positive, 1);
        assert_eq!(t.offer, 1);
        assert_eq!(t.confirmed, 1, "the gig booked before any email is not ours");
        assert_eq!(t.collected, 1);
        assert_eq!(t.fee_collected, 350.0);
        assert_eq!(r.listened_no_reply, 1);
        // Delays: 10 -> 24 h, 12 -> 48 h; median 36 h. Reply -> confirmed: 3 days.
        assert_eq!(r.median_reply_hours, Some(36.0));
        assert_eq!(r.median_reply_to_confirmed_days, Some(3.0));
        let conv = |to: &str| r.conversions.iter().find(|c| c.to == to).unwrap().rate.unwrap();
        assert!((conv("opened") - 0.6).abs() < 1e-9);
        assert!((conv("positive") - 0.5).abs() < 1e-9);

        // Breakdowns.
        let paris = r.by_city.iter().find(|x| x.label == "Paris").unwrap();
        assert_eq!((paris.stages.sent, paris.stages.replied, paris.stages.confirmed), (2, 1, 1));
        let gb = r.by_country.iter().find(|x| x.label == "GB").unwrap();
        assert_eq!(gb.stages.sent, 1);
        let res = r.by_campaign.iter().find(|x| x.key == "2").unwrap();
        assert_eq!((res.kind.as_deref(), res.stages.sent, res.stages.listened), (Some("residency"), 2, 1));
        let promo = r.by_category.iter().find(|x| x.key == "promoter").unwrap();
        assert_eq!(promo.stages.replied, 1);
        // Per email: 6 sends; step 2 = the follow-up to Club B.
        assert_eq!(r.by_step.iter().map(|x| x.stages.sent).sum::<i64>(), 6);
        assert_eq!(r.by_step.iter().find(|x| x.label == "2").unwrap().stages.sent, 1);
        let tpl = r.by_template.iter().find(|x| x.key == "t7").unwrap();
        assert_eq!((tpl.label.as_str(), tpl.stages.sent, tpl.stages.replied), ("Pitch court", 3, 1));
        assert!(r.by_template.iter().any(|x| x.key == "residency" && x.stages.sent == 2));
        // Recipient local time: Paris 08:00 UTC in October (summer time) = 10h.
        assert!(r.by_hour.iter().any(|x| x.label == "10"));
        // 2026-10-06 is a Tuesday.
        assert!(r.by_weekday.iter().any(|x| x.key == "2" && x.stages.sent >= 2));

        // A/B groups: bulk (campaign 1, template 7) and residency step 1.
        assert_eq!(r.variants.len(), 2);
        let bulk = r.variants.iter().find(|g| g.campaign_id == 1).unwrap();
        assert_eq!(bulk.template_id, Some(7));
        assert!(bulk.can_keep);
        let a = bulk.arms.iter().find(|a| a.label == "A").unwrap();
        assert_eq!((a.sent, a.replied, a.example.as_str()), (2, 1, "Hello Club A"));
        assert_eq!(bulk.verdict, "insufficient");
        let resi = r.variants.iter().find(|g| g.campaign_id == 2).unwrap();
        assert_eq!((resi.step, resi.can_keep), (Some(1), true));

        // Options list every value.
        assert_eq!(r.options.campaigns.len(), 2);
        assert!(r.options.cities.contains(&"London".to_string()));
    }

    #[test]
    fn funnel_filters_and_period() {
        let c = fixture();
        let q = FunnelQuery { city: Some("paris".into()), ..Default::default() };
        assert_eq!(report(&c, &q, today()).unwrap().total.sent, 2);
        let q = FunnelQuery { kind: Some("residency".into()), ..Default::default() };
        assert_eq!(report(&c, &q, today()).unwrap().total.sent, 2);
        let q = FunnelQuery { category: Some("promoter".into()), ..Default::default() };
        assert_eq!(report(&c, &q, today()).unwrap().total.replied, 1);
        // Cohort by first send: Club B's follow-up on the 13th does not bring it into that week.
        let q = FunnelQuery { from: Some("2026-10-12".into()), to: Some("2026-10-18".into()), ..Default::default() };
        let r = report(&c, &q, today()).unwrap();
        assert_eq!(r.total.sent, 0);
        assert_eq!(r.by_step.iter().map(|x| x.stages.sent).sum::<i64>(), 1, "per-email views keep the send");
        // Before the gig date, the fee is not collected yet.
        let r = report(&c, &FunnelQuery::default(), NaiveDate::from_ymd_opt(2026, 10, 15).unwrap()).unwrap();
        assert_eq!((r.total.confirmed, r.total.collected), (1, 0));
    }

    fn synthetic_report() -> FunnelReport {
        // 200 emails: Tuesday mornings answer at 30 %, the rest at 5 %; London never.
        let mut mails = Vec::new();
        let mut contacts = HashMap::new();
        for i in 0..200i64 {
            let tuesday = i % 4 == 0;
            let london = i % 5 == 1 && !tuesday;
            let sent = if tuesday { ts("2026-10-06 07:00:00") } else { ts("2026-10-08 15:00:00") };
            let replies = if london { false } else if tuesday { i % 10 < 3 || i % 20 == 4 } else { i % 20 == 3 };
            contacts.insert(i, ContactInfo {
                category: if i % 2 == 0 { "venue".into() } else { "promoter".into() },
                city: if london { "London".into() } else { "Paris".into() },
                country: if london { "GB".into() } else { "FR".into() },
            });
            mails.push(Mail {
                contact_id: i,
                campaign_id: 1,
                campaign_name: "Clubs".into(),
                campaign_kind: "standard".into(),
                sent_at: sent,
                opened: true,
                reply_at: replies.then(|| sent + Duration::hours(5)),
                positive: false,
                variant: Some(if i % 2 == 0 { "A".into() } else { "B".into() }),
                step: None,
                template_id: Some(3),
                template_name: Some("T".into()),
                subject: "s".into(),
            });
        }
        let outcomes = vec![Outcome { contact_id: 2, campaign_id: None, at: ts("2026-10-09 10:00:00"), kind: Kind::Listen }];
        aggregate(&mails, &contacts, &outcomes, &FunnelQuery::default())
    }

    #[test]
    fn suggestions_from_data() {
        let r = synthetic_report();
        assert_eq!(suggestions(&r, 3).len(), 3);
        let s = suggestions(&r, 10);
        let codes: Vec<&str> = s.iter().map(|x| x.code.as_str()).collect();
        assert!(codes.contains(&"best_slot"), "{:?}", codes);
        assert!(codes.contains(&"dead_city"), "{:?}", codes);
        let slot = s.iter().find(|x| x.code == "best_slot").unwrap();
        assert!(slot.text.starts_with("Les envois du mardi matin répondent"), "{}", slot.text);
        let city = s.iter().find(|x| x.code == "dead_city").unwrap();
        assert!(city.text.starts_with("London : 0 réponse sur 30"), "{}", city.text);
        for x in &s {
            assert!(!x.text.contains('—'));
        }
        // No data: one honest suggestion.
        let empty = suggestions(&FunnelReport::default(), 3);
        assert_eq!(empty[0].code, "need_data");
    }

    #[test]
    fn digest_weekly_once_and_text() {
        let c = fixture();
        // Thursday: nothing. Monday 07:00: nothing. Monday 09:00: built once.
        assert!(weekly_tick(&c, ts("2026-10-15 09:00:00")).is_none());
        assert!(weekly_tick(&c, ts("2026-10-12 07:00:00")).is_none());
        let d = weekly_tick(&c, ts("2026-10-12 09:00:00")).expect("built");
        assert!(weekly_tick(&c, ts("2026-10-12 10:00:00")).is_none());
        assert_eq!((d.week_start.as_str(), d.week_end.as_str()), ("2026-10-05", "2026-10-11"));
        assert_eq!(d.cohort.sent, 5);
        assert_eq!(d.activity.sent, 5);
        assert_eq!(d.activity.replies, 2, "out-of-office excluded");
        assert_eq!(d.activity.positive, 1);
        assert_eq!(d.activity.confirmed, 1);
        assert!(!d.suggestions.is_empty() && d.suggestions.len() <= 3);
        let stored = load_digest(&c, "2026-10-05").unwrap();
        assert_eq!(stored.cohort, d.cohort);
        let (subject, body) = digest_text(&d);
        assert!(subject.contains("05/10"));
        assert!(body.contains("À faire"));
        assert!(!subject.contains('—') && !body.contains('—'));
        assert_eq!(last_week_start(NaiveDate::from_ymd_opt(2026, 10, 30).unwrap()).to_string(), "2026-10-19");
    }

    #[test]
    fn keep_best_bulk_and_residency() {
        let c = fixture();
        record_variants(&c, Some(1), Some(7), &["Hello {{venue}}".into(), "Salut {{venue}}".into()]);
        assert_eq!(keep_variant(&c, 1, Some(7), None, "B").unwrap(), "Salut {{venue}}");
        let s: String = c.query_row("SELECT subject FROM templates WHERE id = 7", [], |r| r.get(0)).unwrap();
        assert_eq!(s, "Salut {{venue}}");
        assert!(keep_variant(&c, 1, None, None, "A").is_err());

        let mut cfg = crate::commands::residency::Config::default();
        cfg.steps[0].subject_variants = vec!["Variante B".into()];
        cfg.steps[0].subject_variants_en = vec!["Variant B".into()];
        crate::commands::residency::save_config(&c, 2, &cfg).unwrap();
        assert_eq!(keep_variant(&c, 2, None, Some(1), "B").unwrap(), "Variante B");
        let r = crate::commands::residency::load(&c, 2).unwrap();
        assert_eq!(r.config.steps[0].subject, "Variante B");
        assert_eq!(r.config.steps[0].subject_en, "Variant B");
        assert!(r.config.steps[0].subject_variants.is_empty());
    }

    #[test]
    fn tagging_emails() {
        let c = fixture();
        tag_email(&c, 4, Some(7), None, Some("B"));
        let (t, v): (Option<i64>, Option<String>) =
            c.query_row("SELECT template_id, subject_variant FROM emails WHERE id = 4", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!((t, v.as_deref()), (Some(7), Some("B")));
        let mid: i64 = c.query_row("SELECT id FROM campaign_messages WHERE sent_email_id = 6", [], |r| r.get(0)).unwrap();
        tag_from_message(&c, mid, 6);
        let (s, v): (Option<i64>, Option<String>) =
            c.query_row("SELECT step, subject_variant FROM emails WHERE id = 6", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!((s, v.as_deref()), (Some(1), Some("B")));
    }
}
