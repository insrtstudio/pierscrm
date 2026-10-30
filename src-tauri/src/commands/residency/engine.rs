//! The residency autopilot: a background thread started at launch.
//!
//! Every 5 minutes (and once shortly after launch, to catch up) it re-checks
//! the safety gates, classifies new replies, polls the EPK link activity every
//! 30 minutes and writes the daily report after 08:00. Between ticks it sends
//! at most one due message at a time, 3 to 12 minutes apart, and only:
//! - for campaigns whose autopilot is on and not paused,
//! - inside the send window, evaluated in the target's own time zone,
//! - within both the global daily cap (outbox quota) and the campaign's daily_max,
//! - after a fresh reply sync (stop-on-reply must not be blind),
//! - when nothing stopped the sequence for that contact (reply of any kind,
//!   bounce, unsubscribe, refusal, suppression).
//! Every decision that changes something is written to residency_log.

use chrono::{DateTime, Duration, NaiveDateTime, TimeZone, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex};

use super::zone::{spacing_secs, Tz};
use super::{clear_soft_pause, content, load, log, set_pause, Config};
use crate::commands::email::SendResult;
use crate::db::DbPool;

pub const TICK_SECS: i64 = 300;
/// A reply sync older than this is redone before a send.
pub const SYNC_FRESH_MIN: i64 = 15;
/// No successful reply sync for this long pauses the autopilot.
pub const SYNC_STALE_HOURS: i64 = 6;
pub const PRESS_POLL_MIN: i64 = 30;
/// Bounce gate: more than 5 % over the last 20 sends.
pub const BOUNCE_WINDOW: i64 = 20;
pub const BOUNCE_MAX_PCT: i64 = 5;
/// The daily report is built on the first tick after this local hour.
pub const REPORT_HOUR: u32 = 8;

pub fn fmt(dt: DateTime<Utc>) -> String {
    dt.format("%Y-%m-%d %H:%M:%S").to_string()
}

pub fn parse_utc(s: &str) -> Option<DateTime<Utc>> {
    let t = s.trim();
    NaiveDateTime::parse_from_str(t.get(..19).unwrap_or(t), "%Y-%m-%d %H:%M:%S")
        .or_else(|_| NaiveDateTime::parse_from_str(t.get(..19).unwrap_or(t), "%Y-%m-%dT%H:%M:%S"))
        .ok()
        .map(|n| Utc.from_utc_datetime(&n))
}

pub(crate) fn get(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM settings WHERE key = ?1", params![key], |r| r.get::<_, Option<String>>(0))
        .ok()
        .flatten()
        .filter(|s| !s.trim().is_empty())
}

pub fn put(conn: &Connection, key: &str, value: &str) {
    let _ = conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    );
}

// ---------------- Safety gates ----------------

#[derive(Debug, Clone, PartialEq)]
pub enum Gate {
    Open,
    /// Clears by itself once the cause is gone.
    Soft(String),
    /// Stays until the user resumes.
    Hard(String),
}

/// When the last reply sync succeeded. Older builds did not record it: no
/// error recorded after the last sync counts as a success.
pub fn last_sync_ok(conn: &Connection) -> Option<DateTime<Utc>> {
    get(conn, "imap_last_ok").and_then(|s| parse_utc(&s)).or_else(|| {
        if get(conn, "imap_last_error").is_none() {
            get(conn, "imap_last_sync").and_then(|s| parse_utc(&s))
        } else {
            None
        }
    })
}

/// Bounces among the last BOUNCE_WINDOW sends (all campaigns: the sending
/// domain's reputation is shared), counting only sends after the last time
/// the user resumed from a bounce pause.
pub fn recent_bounces(conn: &Connection) -> i64 {
    let since = get(conn, "residency_bounce_reset_at").unwrap_or_else(|| "1970-01-01 00:00:00".into());
    conn.query_row(
        &format!(
            "SELECT COUNT(*) FROM (
                SELECT e.to_addr FROM emails e
                WHERE e.status IN ('sent','failed') AND e.sent_at > ?1
                ORDER BY e.sent_at DESC, e.id DESC LIMIT {}) last
             WHERE EXISTS (SELECT 1 FROM suppressions s WHERE s.reason = 'bounce' AND s.address = lower(trim(last.to_addr)))",
            BOUNCE_WINDOW
        ),
        params![since],
        |r| r.get(0),
    )
    .unwrap_or(0)
}

pub fn safety_gate(conn: &Connection, smtp_ok: bool, now: DateTime<Utc>) -> Gate {
    let bounces = recent_bounces(conn);
    // Fixed denominator: with fewer than 20 sends, 2 bounces already exceed 5 %.
    if bounces * 100 > BOUNCE_MAX_PCT * BOUNCE_WINDOW {
        return Gate::Hard(format!(
            "Taux de rebond trop élevé : {} adresses rejetées sur les {} derniers envois. Vérifiez les adresses avant de reprendre.",
            bounces, BOUNCE_WINDOW
        ));
    }
    if !smtp_ok {
        return Gate::Soft("SMTP non configuré (Réglages, Email).".into());
    }
    match last_sync_ok(conn) {
        Some(t) if now - t <= Duration::hours(SYNC_STALE_HOURS) => Gate::Open,
        _ => Gate::Soft(
            "La détection des réponses (IMAP) n'a pas réussi depuis plus de 6 h : sans elle le pilote ne verrait pas une réponse."
                .into(),
        ),
    }
}

pub fn is_auth_error(text: &str) -> bool {
    let t = text.to_lowercase();
    t.contains("535") || t.contains("5.7.8") || t.contains("authentication") || t.contains("auth failed")
        || t.contains("invalid credentials") || t.contains("login failed")
}

/// Apply the gate to every residency campaign with the autopilot on.
pub fn apply_gates(conn: &Connection, smtp_ok: bool, now: DateTime<Utc>) -> Gate {
    let gate = safety_gate(conn, smtp_ok, now);
    for id in autopilot_ids(conn) {
        match &gate {
            Gate::Open => clear_soft_pause(conn, id),
            Gate::Soft(r) => set_pause(conn, id, "soft", r),
            Gate::Hard(r) => set_pause(conn, id, "hard", r),
        }
    }
    gate
}

fn autopilot_ids(conn: &Connection) -> Vec<i64> {
    super::active_ids(conn)
        .into_iter()
        .filter(|id| load(conn, *id).map_or(false, |r| r.autopilot))
        .collect()
}

pub fn pause_all_hard(conn: &Connection, reason: &str) {
    for id in autopilot_ids(conn) {
        set_pause(conn, id, "hard", reason);
    }
}

// ---------------- Stop rules ----------------

/// When step 1 went out to this contact in this campaign (J0).
pub fn j0(conn: &Connection, campaign_id: i64, contact_id: i64) -> Option<DateTime<Utc>> {
    conn.query_row(
        "SELECT e.sent_at FROM campaign_messages m JOIN emails e ON e.id = m.sent_email_id
         WHERE m.campaign_id = ?1 AND m.contact_id = ?2 AND m.step = 1 AND m.status = 'sent'",
        params![campaign_id, contact_id],
        |r| r.get::<_, String>(0),
    )
    .ok()
    .and_then(|s| parse_utc(&s))
}

/// Why the sequence must stop for this contact, if it must. A reply of any
/// kind stops it (an out-of-office only reschedules); so do a bounce, an
/// unsubscribe, a refusal, a suppression, or a conversation already open.
pub fn stop_reason(conn: &Connection, campaign_id: i64, contact_id: i64) -> Option<String> {
    let (email, status): (String, String) = conn
        .query_row(
            "SELECT COALESCE(email,''), status FROM contacts WHERE id = ?1",
            params![contact_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok()?;
    let addr = email.trim().to_lowercase();
    if addr.is_empty() {
        return Some("pas d'adresse email".into());
    }
    if let Some(r) = crate::commands::outbox::suppression_reason(conn, &addr) {
        return Some(format!("adresse en liste de suppression ({})", r));
    }
    match status.as_str() {
        "declined" => return Some("contact passé en refus".into()),
        "confirmed" => return Some("date confirmée".into()),
        "in_discussion" => return Some("conversation déjà en cours".into()),
        _ => {}
    }
    let bounced: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM campaign_contacts WHERE campaign_id = ?1 AND contact_id = ?2 AND state = 'bounced')",
            params![campaign_id, contact_id],
            |r| r.get(0),
        )
        .unwrap_or(false);
    if bounced {
        return Some("rebond".into());
    }
    let since = j0(conn, campaign_id, contact_id).map(fmt)?;
    let replied: Option<String> = conn
        .query_row(
            "SELECT r.kind FROM email_replies r
             WHERE (r.contact_id = ?1 OR lower(trim(r.from_addr)) = ?2)
               AND r.kind IN ('reply','unsubscribe','bounce') AND r.received_at >= ?3
               AND NOT EXISTS (SELECT 1 FROM reply_insights i WHERE i.email_reply_id = r.id AND i.intent = 'out_of_office')
             ORDER BY r.received_at LIMIT 1",
            params![contact_id, addr, since],
            |r| r.get(0),
        )
        .optional()
        .ok()
        .flatten();
    replied.map(|k| match k.as_str() {
        "bounce" => "rebond".to_string(),
        "unsubscribe" => "désinscription".to_string(),
        _ => "réponse reçue".to_string(),
    })
}

/// Skip whatever is still pending for this contact in this campaign.
pub fn skip_remaining(conn: &Connection, campaign_id: i64, contact_id: i64, reason: &str) -> usize {
    let n = conn
        .execute(
            "UPDATE campaign_messages SET status = 'skipped', note = ?3, updated_at = datetime('now')
             WHERE campaign_id = ?1 AND contact_id = ?2 AND status IN ('draft','scheduled')",
            params![campaign_id, contact_id, reason],
        )
        .unwrap_or(0);
    if n > 0 {
        let name: String = conn
            .query_row("SELECT name FROM contacts WHERE id = ?1", params![contact_id], |r| r.get(0))
            .unwrap_or_default();
        log(conn, Some(campaign_id), "info", "sequence_stopped", &format!("{} : séquence arrêtée ({})", name, reason));
    }
    n
}

// ---------------- Picking and sending ----------------

#[derive(Debug, Clone)]
pub struct DueMsg {
    pub id: i64,
    pub campaign_id: i64,
    pub contact_id: i64,
    pub step: i64,
    pub subject: String,
    pub body: String,
    pub to: String,
}

/// Messages of the campaign sent since local midnight.
pub fn campaign_sent_today(conn: &Connection, campaign_id: i64) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM emails WHERE campaign_id = ?1 AND status = 'sent'
           AND sent_at >= datetime('now', 'localtime', 'start of day', 'utc')",
        params![campaign_id],
        |r| r.get(0),
    )
    .unwrap_or(0)
}

/// Time zone of a contact: the country of its Discovery venue, else the zone's.
pub fn tz_of(conn: &Connection, contact_id: i64, cfg: &Config) -> Tz {
    let pays: Option<String> = conn
        .query_row(
            "SELECT pays FROM vi_venues WHERE crm_contact_id = ?1 AND pays IS NOT NULL LIMIT 1",
            params![contact_id],
            |r| r.get(0),
        )
        .ok();
    Tz::for_country(&pays.unwrap_or_else(|| cfg.zone.default_country()))
}

fn due_messages(conn: &Connection, now: DateTime<Utc>) -> Vec<DueMsg> {
    let Ok(mut st) = conn.prepare(
        "SELECT m.id, m.campaign_id, m.contact_id, m.step, m.subject, m.body, COALESCE(k.email,'')
         FROM campaign_messages m
         JOIN residency_campaigns r ON r.campaign_id = m.campaign_id
         JOIN campaigns c ON c.id = m.campaign_id
         JOIN contacts k ON k.id = m.contact_id
         WHERE m.status = 'scheduled' AND (m.scheduled_for IS NULL OR m.scheduled_for <= ?1)
           AND r.autopilot = 1 AND r.paused_reason IS NULL AND c.status = 'active' AND c.kind = 'residency'
         ORDER BY COALESCE(m.scheduled_for, m.updated_at), m.step DESC, m.id",
    ) else {
        return Vec::new();
    };
    st.query_map(params![fmt(now)], |r| {
        Ok(DueMsg {
            id: r.get(0)?,
            campaign_id: r.get(1)?,
            contact_id: r.get(2)?,
            step: r.get(3)?,
            subject: r.get(4)?,
            body: r.get(5)?,
            to: r.get(6)?,
        })
    })
    .map(|it| it.flatten().collect())
    .unwrap_or_default()
}

/// The next message that may go out right now, or None. Messages whose
/// sequence must stop are skipped on the way (and logged).
pub fn pick_next(conn: &Connection, now: DateTime<Utc>) -> Option<DueMsg> {
    let quota = crate::commands::outbox::quota(conn);
    if quota.remaining <= 0 {
        return None;
    }
    // Shared sending budget and sender health (deliverability).
    if crate::commands::deliverability::automated_block(conn, now).is_some() {
        return None;
    }
    let mut cfgs: HashMap<i64, Config> = HashMap::new();
    for m in due_messages(conn, now) {
        if let Some(reason) = stop_reason(conn, m.campaign_id, m.contact_id) {
            skip_remaining(conn, m.campaign_id, m.contact_id, &reason);
            continue;
        }
        let cfg = cfgs
            .entry(m.campaign_id)
            .or_insert_with(|| load(conn, m.campaign_id).map(|r| r.config).unwrap_or_default())
            .clone();
        if campaign_sent_today(conn, m.campaign_id) >= cfg.daily_max.max(0) {
            continue;
        }
        if !cfg.window.contains(tz_of(conn, m.contact_id, &cfg), now) {
            continue;
        }
        // Its domain had its share today: another venue may go instead.
        if crate::commands::deliverability::domain_capped(conn, &m.to, now) {
            continue;
        }
        // Template variables must all be resolved.
        if !content::unresolved(&format!("{}\n{}", m.subject, m.body)).is_empty() {
            let _ = conn.execute(
                "UPDATE campaign_messages SET status = 'draft', note = 'Variables à compléter', updated_at = datetime('now') WHERE id = ?1",
                params![m.id],
            );
            continue;
        }
        return Some(m);
    }
    None
}

/// Sends one email. The real one goes through email::send_one (guardrails,
/// Message-ID, List-Unsubscribe, pipeline, audience); tests use a fake.
pub trait Mailer {
    fn send(&self, contact_id: i64, campaign_id: i64, to: &str, subject: &str, body: &str)
        -> Result<(SendResult, Option<i64>), String>;
}

pub struct SmtpMailer {
    pub pool: DbPool,
}

impl Mailer for SmtpMailer {
    fn send(&self, contact_id: i64, campaign_id: i64, to: &str, subject: &str, body: &str)
        -> Result<(SendResult, Option<i64>), String> {
        crate::commands::email::send_one(&self.pool, Some(contact_id), Some(campaign_id), to, subject, body)
    }
}

#[derive(Debug, PartialEq)]
pub enum Sent {
    Ok,
    Skipped,
    Failed,
    /// Nothing changed (quota reached in the meantime): try again later.
    Wait,
    Paused,
}

/// Send a picked message and apply the result: next step scheduled, sequence
/// stopped on a bounce, autopilot paused on an SMTP authentication error.
pub fn send_picked(conn: &Connection, mailer: &dyn Mailer, m: &DueMsg, now: DateTime<Utc>) -> Sent {
    let name: String = conn
        .query_row("SELECT name FROM contacts WHERE id = ?1", params![m.contact_id], |r| r.get(0))
        .unwrap_or_default();
    match mailer.send(m.contact_id, m.campaign_id, &m.to, &m.subject, &m.body) {
        Err(e) => {
            if e.contains("Sender email") || e.contains("SMTP host") {
                set_pause(conn, m.campaign_id, "soft", "SMTP non configuré (Réglages, Email).");
                return Sent::Paused;
            }
            mark(conn, m.id, "failed", Some(&e), None);
            log(conn, Some(m.campaign_id), "warn", "send_failed", &format!("{} (étape {}) : {}", name, m.step, e));
            Sent::Failed
        }
        Ok((r, email_id)) if r.ok => {
            mark(conn, m.id, "sent", None, email_id);
            let start = if m.step == 1 { Some(now) } else { j0(conn, m.campaign_id, m.contact_id) };
            if let (Some(start), Some(cfg)) = (start, load(conn, m.campaign_id).map(|r| r.config)) {
                content::create_next_step(conn, m.campaign_id, &cfg, m.contact_id, m.step, start);
            }
            log(conn, Some(m.campaign_id), "send", "sent", &format!("{} : étape {} envoyée à {}", name, m.step, m.to));
            Sent::Ok
        }
        Ok((r, _)) => match r.code.as_deref() {
            Some("quota") => Sent::Wait,
            Some(c) if c.starts_with("budget_") => Sent::Wait,
            Some("suppressed") => {
                skip_remaining(conn, m.campaign_id, m.contact_id, "adresse en liste de suppression");
                Sent::Skipped
            }
            Some("bounced") => {
                mark(conn, m.id, "failed", r.error.as_deref(), None);
                skip_remaining(conn, m.campaign_id, m.contact_id, "rebond");
                log(conn, Some(m.campaign_id), "warn", "bounce", &format!("{} : adresse rejetée ({})", name, m.to));
                Sent::Failed
            }
            _ => {
                let err = r.error.unwrap_or_default();
                if is_auth_error(&err) {
                    pause_all_hard(conn, &format!("Erreur d'authentification SMTP : {}. Vérifiez le mot de passe puis reprenez.", err));
                    return Sent::Paused;
                }
                mark(conn, m.id, "failed", Some(&err), None);
                log(conn, Some(m.campaign_id), "warn", "send_failed", &format!("{} (étape {}) : {}", name, m.step, err));
                Sent::Failed
            }
        },
    }
}

fn mark(conn: &Connection, id: i64, status: &str, note: Option<&str>, email_id: Option<i64>) {
    let _ = conn.execute(
        "UPDATE campaign_messages SET status = ?2, note = COALESCE(?3, note), sent_email_id = COALESCE(?4, sent_email_id),
                updated_at = datetime('now') WHERE id = ?1",
        params![id, status, note, email_id],
    );
}

/// Earliest instant the next send may happen (spacing between sends).
pub fn next_send_at(conn: &Connection) -> Option<DateTime<Utc>> {
    get(conn, "residency_next_send_at").and_then(|s| parse_utc(&s))
}

/// When the campaign's next email should go out, for the status line: the
/// first scheduled message, pushed to the spacing, the daily cap and the
/// window of its target.
pub fn next_send_estimate(conn: &Connection, campaign_id: i64, cfg: &Config, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let rows: Vec<(i64, Option<String>)> = conn
        .prepare(
            "SELECT contact_id, scheduled_for FROM campaign_messages
             WHERE campaign_id = ?1 AND status = 'scheduled' ORDER BY COALESCE(scheduled_for, '0') LIMIT 50",
        )
        .ok()?
        .query_map(params![campaign_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .ok()?
        .flatten()
        .collect();
    let floor = next_send_at(conn).filter(|t| *t > now).unwrap_or(now);
    let capped = campaign_sent_today(conn, campaign_id) >= cfg.daily_max
        || crate::commands::outbox::quota(conn).remaining <= 0;
    rows.into_iter()
        .map(|(cid, when)| {
            let mut t = when.as_deref().and_then(parse_utc).map_or(floor, |w| w.max(floor));
            if capped {
                // Tomorrow, local midnight onwards.
                let local = chrono::Local::now().date_naive() + Duration::days(1);
                let midnight = chrono::Local
                    .from_local_datetime(&local.and_hms_opt(0, 0, 0).expect("valid"))
                    .earliest()
                    .map(|d| d.with_timezone(&Utc))
                    .unwrap_or(now + Duration::days(1));
                t = t.max(midnight);
            }
            cfg.window.next_open(tz_of(conn, cid, cfg), t)
        })
        .min()
}

// ---------------- Background thread ----------------

static WAKE: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());
static CLASSIFY_RUNNING: AtomicBool = AtomicBool::new(false);

/// Ask the engine to run a tick now (autopilot switched on, drafts validated).
pub fn wake() {
    let (m, cv) = &WAKE;
    if let Ok(mut w) = m.lock() {
        *w = true;
        cv.notify_all();
    }
}

fn sleep_or_wake(secs: u64) -> bool {
    let (m, cv) = &WAKE;
    let Ok(guard) = m.lock() else {
        std::thread::sleep(std::time::Duration::from_secs(secs));
        return false;
    };
    let (mut g, _) = cv
        .wait_timeout_while(guard, std::time::Duration::from_secs(secs), |w| !*w)
        .unwrap_or_else(|e| e.into_inner());
    let woken = *g;
    *g = false;
    woken
}

fn smtp_ok(pool: &DbPool) -> bool {
    crate::commands::email::load_smtp_from(pool)
        .map(|c| !c.host.trim().is_empty() && !c.from_email.trim().is_empty() && !c.password.trim().is_empty())
        .unwrap_or(false)
}

/// Classify the new replies off the caller's thread (after a manual sync).
pub fn classify_in_background(pool: DbPool) {
    std::thread::spawn(move || classify_now(&pool));
}

fn classify_now(pool: &DbPool) {
    if CLASSIFY_RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    if let Ok(conn) = pool.get() {
        let llm = super::claude::client(&conn);
        super::replies::classify_pending(&conn, llm.as_deref(), chrono::Local::now().date_naive());
    }
    CLASSIFY_RUNNING.store(false, Ordering::SeqCst);
}

fn ensure_fresh_sync(pool: &DbPool, now: DateTime<Utc>) {
    let fresh = pool
        .get()
        .ok()
        .and_then(|c| last_sync_ok(&c))
        .map_or(false, |t| now - t < Duration::minutes(SYNC_FRESH_MIN));
    if fresh {
        return;
    }
    let res = crate::commands::outbox::sync_replies_blocking(pool);
    if let Ok(conn) = pool.get() {
        match res {
            Ok(r) if !r.skipped && (r.replies > 0 || r.bounces > 0) => log(
                &conn,
                None,
                "info",
                "reply_sync",
                &format!("Synchro des réponses avant envoi : {} réponse(s), {} rebond(s)", r.replies, r.bounces),
            ),
            Ok(_) => {}
            // Logged once per distinct error, not every 5 minutes.
            Err(e) => {
                if get(&conn, "residency_last_sync_err").as_deref() != Some(e.as_str()) {
                    put(&conn, "residency_last_sync_err", &e);
                    log(&conn, None, "warn", "reply_sync_failed", &format!("Synchro des réponses impossible : {}", e));
                }
            }
        }
    }
    classify_now(pool);
}

fn tick(pool: &DbPool, app: &tauri::AppHandle, now: DateTime<Utc>) {
    let Ok(conn) = pool.get() else { return };
    // With an autopilot on, the engine keeps reply detection fresh itself
    // (otherwise a stale sync would pause it until the UI synced).
    let any_autopilot = !autopilot_ids(&conn).is_empty();
    drop(conn);
    if any_autopilot {
        ensure_fresh_sync(pool, now);
    } else {
        classify_now(pool);
    }
    let Ok(conn) = pool.get() else { return };
    apply_gates(&conn, smtp_ok(pool), now);
    // Sender health (auto-pause) and one-click unsubscribes.
    crate::commands::deliverability::on_tick(&conn, now);
    drop(conn);
    let Ok(conn) = pool.get() else { return };

    // EPK link activity every 30 minutes.
    let last_poll = get(&conn, "insrt_last_poll").and_then(|s| parse_utc(&s));
    if last_poll.map_or(true, |t| now - t >= Duration::minutes(PRESS_POLL_MIN)) {
        if let Some(client) = super::insrt::client(&conn) {
            match super::report::poll_press(&conn, client.as_ref(), now) {
                Ok(n) if n > 0 => log(&conn, None, "info", "press_activity", &format!("{} événement(s) EPK reçus", n)),
                Ok(_) => {}
                Err(e) => log(&conn, None, "warn", "press_poll_failed", &e),
            }
            // Gigs whose push to insrt failed (or is pending) are retried here.
            let pushed = crate::commands::insrt_gigs::sync_all(&conn, client.as_ref());
            if pushed > 0 {
                log(&conn, None, "info", "insrt_gigs", &format!("{} date(s) mise(s) à jour sur insrt", pushed));
            }
        }
        // Pixel opens, on the same 30-minute rhythm.
        if !super::active_ids(&conn).is_empty() {
            if let Err(e) = super::report::poll_opens(&conn) {
                eprintln!("[residency] {}", e);
            }
        }
        put(&conn, "insrt_last_poll", &fmt(now));
    }

    // Daily report on the first tick after 08:00 local.
    let local = chrono::Local::now();
    let today = local.date_naive().format("%Y-%m-%d").to_string();
    use chrono::Timelike;
    if local.hour() >= REPORT_HOUR && get(&conn, "residency_report_day").as_deref() != Some(today.as_str()) {
        put(&conn, "residency_report_day", &today);
        let llm = super::claude::client(&conn);
        let mut built = 0;
        for id in super::active_ids(&conn) {
            match super::report::build_and_store(&conn, id, local.date_naive(), llm.as_deref()) {
                Ok(true) => built += 1,
                Ok(false) => {}
                Err(e) => log(&conn, Some(id), "warn", "report_failed", &e),
            }
        }
        if built > 0 {
            notify(app, built);
        }
    }
}

fn notify(app: &tauri::AppHandle, n: usize) {
    use tauri_plugin_notification::NotificationExt;
    let body = if n == 1 {
        "Le point du jour sur la campagne résidence est disponible.".to_string()
    } else {
        format!("Le point du jour sur {} campagnes résidence est disponible.", n)
    };
    use tauri_plugin_notification::PermissionState;
    if !matches!(app.notification().permission_state(), Ok(PermissionState::Granted)) {
        let _ = app.notification().request_permission();
    }
    let _ = app.notification().builder().title("Rapport résidence prêt").body(body).show();
}

/// Start the autopilot thread. First pass 20 s after launch (catch-up).
pub fn start(pool: DbPool, app: tauri::AppHandle) {
    std::thread::Builder::new()
        .name("residency-autopilot".into())
        .spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(20));
            let mut last_tick: Option<DateTime<Utc>> = None;
            loop {
                // A panic in one pass must not kill the autopilot thread.
                let pool2 = pool.clone();
                let app2 = app.clone();
                let lt = last_tick;
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || run_once(&pool2, &app2, lt)));
                match r {
                    Ok(t) => last_tick = t,
                    Err(_) => eprintln!("[residency] autopilot pass panicked"),
                }
                if sleep_or_wake(30) {
                    last_tick = None;
                }
            }
        })
        .ok();
}

fn run_once(pool: &DbPool, app: &tauri::AppHandle, last_tick: Option<DateTime<Utc>>) -> Option<DateTime<Utc>> {
    let now = Utc::now();
    let ticked = if last_tick.map_or(true, |t| now - t >= Duration::seconds(TICK_SECS)) {
        tick(pool, app, now);
        Some(now)
    } else {
        last_tick
    };
    let Ok(conn) = pool.get() else { return ticked };
    if next_send_at(&conn).map_or(false, |t| now < t) || pick_next(&conn, now).is_none() {
        return ticked;
    }
    drop(conn);
    // Something may go out: make sure no reply arrived meanwhile.
    ensure_fresh_sync(pool, now);
    let Ok(conn) = pool.get() else { return ticked };
    if !matches!(apply_gates(&conn, smtp_ok(pool), now), Gate::Open) {
        return ticked;
    }
    if let Some(m) = pick_next(&conn, now) {
        let mailer = SmtpMailer { pool: pool.clone() };
        let outcome = send_picked(&conn, &mailer, &m, now);
        if outcome != Sent::Wait {
            let gap = spacing_secs(m.id as u64);
            put(&conn, "residency_next_send_at", &fmt(now + Duration::seconds(gap)));
        }
    }
    ticked
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::commands::residency::{save_config, testdb};
    use std::cell::RefCell;

    /// Records sends into `emails` like the real path, without SMTP.
    pub struct FakeMailer<'a> {
        pub conn: &'a Connection,
        pub fail_with: RefCell<Option<SendResult>>,
    }
    impl Mailer for FakeMailer<'_> {
        fn send(&self, cid: i64, camp: i64, to: &str, subject: &str, body: &str) -> Result<(SendResult, Option<i64>), String> {
            if let Some(r) = self.fail_with.borrow_mut().take() {
                return Ok((r, None));
            }
            self.conn
                .execute(
                    "INSERT INTO emails (contact_id, campaign_id, to_addr, subject, body, status, message_id)
                     VALUES (?1, ?2, ?3, ?4, ?5, 'sent', '<x@y>')",
                    params![cid, camp, to, subject, body],
                )
                .unwrap();
            let id = self.conn.last_insert_rowid();
            crate::commands::status::on_email_sent(self.conn, Some(cid), to);
            crate::commands::campaigns::mark_audience(self.conn, camp, cid, "sent");
            Ok((SendResult { ok: true, error: None, tracked: false, code: None }, Some(id)))
        }
    }

    fn utc(s: &str) -> DateTime<Utc> {
        parse_utc(s).unwrap()
    }

    pub fn setup() -> Connection {
        let c = testdb::conn();
        c.execute_batch(
            "INSERT INTO artists (id, name) VALUES (1, 'Piers');
             INSERT INTO campaigns (id, name, kind, artist_id) VALUES (1, 'R', 'residency', 1);
             INSERT INTO contacts (id, category, name, venue, area, email, status) VALUES
                (10, 'venue', 'Club A', 'Club A', 'Paris', 'booking@cluba.fr', 'to_contact'),
                (11, 'venue', 'Club B', 'Club B', 'London', 'booking@clubb.co.uk', 'to_contact');
             INSERT INTO vi_venues (nom, nom_normalise, ville, pays, statut, crm_contact_id) VALUES
                ('Club B', 'clubb', 'London', 'UK', 'valide', 11);
             INSERT INTO campaign_contacts (campaign_id, contact_id, state) VALUES (1, 10, 'planned'), (1, 11, 'planned');
             INSERT INTO settings (key, value) VALUES ('imap_last_ok', datetime('now'));",
        )
        .unwrap();
        let mut cfg = Config::default();
        cfg.assets.push(crate::commands::residency::Asset {
            kind: "mix".into(),
            title: "Mix".into(),
            url: "https://sc.com/m".into(),
            note: String::new(),
        });
        save_config(&c, 1, &cfg).unwrap();
        c.execute("UPDATE residency_campaigns SET autopilot = 1", []).unwrap();
        for cid in [10, 11] {
            c.execute(
                "INSERT INTO campaign_messages (campaign_id, contact_id, step, subject, body, status)
                 VALUES (1, ?1, 1, 'Résidence', 'Bonjour, https://sc.com/m', 'scheduled')",
                params![cid],
            )
            .unwrap();
        }
        c
    }

    #[test]
    fn window_caps_and_step_progression() {
        let c = setup();
        let mailer = FakeMailer { conn: &c, fail_with: RefCell::new(None) };
        // Monday 2 Nov 2026 09:30 UTC: open in Paris (10:30), closed in London (09:30).
        let t = utc("2026-11-02 09:30:00");
        let m = pick_next(&c, t).unwrap();
        assert_eq!(m.contact_id, 10);
        assert_eq!(send_picked(&c, &mailer, &m, t), Sent::Ok);
        // London is not in its window yet; nothing else may go.
        assert!(pick_next(&c, t).is_none());
        // Step 2 exists, scheduled J0 + 5 days, rendered and resolved.
        let (status, when, body): (String, String, String) = c
            .query_row("SELECT status, scheduled_for, body FROM campaign_messages WHERE contact_id = 10 AND step = 2", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap();
        assert_eq!(status, "scheduled");
        assert_eq!(when, "2026-11-07 09:30:00");
        assert!(body.contains("Club A") && content::unresolved(&body).is_empty());
        // Saturday 7 Nov: due but outside the window. Monday 9 Nov it goes.
        assert!(pick_next(&c, utc("2026-11-07 10:00:00")).is_none());
        // London at 10:30 London time.
        let t2 = utc("2026-11-02 10:30:00");
        assert_eq!(pick_next(&c, t2).unwrap().contact_id, 11);
        // Campaign daily max.
        let mut cfg = load(&c, 1).unwrap().config;
        cfg.daily_max = 1;
        save_config(&c, 1, &cfg).unwrap();
        // (1 sent "today" in SQL time only if now is today; force it.)
        c.execute("UPDATE emails SET sent_at = datetime('now')", []).unwrap();
        assert!(pick_next(&c, t2).is_none());
        cfg.daily_max = 8;
        save_config(&c, 1, &cfg).unwrap();
        // Autopilot off or paused: nothing.
        c.execute("UPDATE residency_campaigns SET paused_reason = 'x', paused_kind = 'hard'", []).unwrap();
        assert!(pick_next(&c, t2).is_none());
        c.execute("UPDATE residency_campaigns SET paused_reason = NULL, paused_kind = NULL, autopilot = 0", []).unwrap();
        assert!(pick_next(&c, t2).is_none());
    }

    #[test]
    fn reply_stops_sequence_and_ooo_does_not() {
        let c = setup();
        let mailer = FakeMailer { conn: &c, fail_with: RefCell::new(None) };
        let t = utc("2026-11-02 09:30:00");
        let m = pick_next(&c, t).unwrap();
        send_picked(&c, &mailer, &m, t);
        let sent_at: String = c.query_row("SELECT sent_at FROM emails", [], |r| r.get(0)).unwrap();
        // An out-of-office classified as such does not stop.
        c.execute(
            "INSERT INTO email_replies (id, email_id, contact_id, from_addr, message_id, kind, received_at)
             VALUES (1, 1, 10, 'booking@cluba.fr', '<r1>', 'reply', datetime(?1, '+1 hour'))",
            params![sent_at],
        )
        .unwrap();
        c.execute(
            "INSERT INTO reply_insights (email_reply_id, contact_id, campaign_id, sentiment, intent) VALUES (1, 10, 1, 'neutral', 'out_of_office')",
            [],
        )
        .unwrap();
        assert_eq!(stop_reason(&c, 1, 10), None);
        // A human reply stops it, and pick_next skips the pending step.
        c.execute(
            "INSERT INTO email_replies (id, email_id, contact_id, from_addr, message_id, kind, received_at)
             VALUES (2, 1, 10, 'booking@cluba.fr', '<r2>', 'reply', datetime(?1, '+2 hours'))",
            params![sent_at],
        )
        .unwrap();
        assert_eq!(stop_reason(&c, 1, 10).as_deref(), Some("réponse reçue"));
        c.execute("UPDATE campaign_messages SET scheduled_for = NULL WHERE step = 2", []).unwrap();
        let _ = pick_next(&c, utc("2026-11-09 09:30:00"));
        let st: String = c.query_row("SELECT status FROM campaign_messages WHERE contact_id = 10 AND step = 2", [], |r| r.get(0)).unwrap();
        assert_eq!(st, "skipped");
        // Suppression and declined stop too.
        c.execute("INSERT INTO suppressions (address, reason) VALUES ('booking@clubb.co.uk', 'unsubscribe')", []).unwrap();
        assert!(stop_reason(&c, 1, 11).unwrap().contains("suppression"));
    }

    #[test]
    fn safety_gates() {
        let c = setup();
        let now = Utc::now();
        assert_eq!(safety_gate(&c, true, now), Gate::Open);
        assert!(matches!(safety_gate(&c, false, now), Gate::Soft(_)));
        // Stale reply sync.
        c.execute("UPDATE settings SET value = datetime('now','-7 hours') WHERE key = 'imap_last_ok'", []).unwrap();
        assert!(matches!(safety_gate(&c, true, now), Gate::Soft(r) if r.contains("IMAP")));
        apply_gates(&c, true, now);
        assert!(load(&c, 1).unwrap().paused_reason.unwrap().contains("IMAP"));
        c.execute("UPDATE settings SET value = datetime('now') WHERE key = 'imap_last_ok'", []).unwrap();
        apply_gates(&c, true, now);
        assert!(load(&c, 1).unwrap().paused_reason.is_none());
        // Bounce rate: 1 bounce in 20 is 5 % (allowed), 2 is over.
        for i in 0..20 {
            c.execute(
                "INSERT INTO emails (to_addr, subject, body, status) VALUES (?1, 's', 'b', 'sent')",
                params![format!("a{}@x.fr", i)],
            )
            .unwrap();
        }
        c.execute("INSERT INTO suppressions (address, reason) VALUES ('a1@x.fr', 'bounce')", []).unwrap();
        assert_eq!(safety_gate(&c, true, now), Gate::Open);
        c.execute("INSERT INTO suppressions (address, reason) VALUES ('a2@x.fr', 'bounce')", []).unwrap();
        assert!(matches!(safety_gate(&c, true, now), Gate::Hard(_)));
        apply_gates(&c, true, now);
        assert_eq!(load(&c, 1).unwrap().paused_kind.as_deref(), Some("hard"));
        // A resume resets the bounce window.
        put(&c, "residency_bounce_reset_at", &fmt(Utc::now() + Duration::seconds(5)));
        assert_eq!(safety_gate(&c, true, now), Gate::Open);
        // SMTP auth error pauses hard and keeps the message scheduled.
        c.execute("UPDATE residency_campaigns SET paused_reason = NULL, paused_kind = NULL", []).unwrap();
        let mailer = FakeMailer {
            conn: &c,
            fail_with: RefCell::new(Some(SendResult {
                ok: false,
                error: Some("535 5.7.8 Authentication credentials invalid".into()),
                tracked: false,
                code: None,
            })),
        };
        let t = utc("2026-11-02 09:30:00");
        let m = pick_next(&c, t).unwrap();
        assert_eq!(send_picked(&c, &mailer, &m, t), Sent::Paused);
        assert_eq!(load(&c, 1).unwrap().paused_kind.as_deref(), Some("hard"));
        let st: String = c.query_row("SELECT status FROM campaign_messages WHERE id = ?1", [m.id], |r| r.get(0)).unwrap();
        assert_eq!(st, "scheduled");
        assert!(is_auth_error("535 Authentication failed") && !is_auth_error("550 5.1.1 user unknown"));
    }

    #[test]
    fn unresolved_message_is_not_sent() {
        let c = setup();
        c.execute("UPDATE campaign_messages SET body = 'Bonjour {{first_name}}' WHERE contact_id = 10", []).unwrap();
        let t = utc("2026-11-02 09:30:00");
        assert!(pick_next(&c, t).is_none());
        let st: String = c.query_row("SELECT status FROM campaign_messages WHERE contact_id = 10", [], |r| r.get(0)).unwrap();
        assert_eq!(st, "draft");
    }
}
