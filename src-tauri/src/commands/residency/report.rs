//! EPK link activity (insrt.studio) and the daily campaign report.

use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use super::claude::{self, Llm, LlmRequest, SystemBlock};
use super::engine::{self, fmt};
use super::insrt::{contact_of_ref, ActivityEvent, Insrt};

/// A listen at least this long is a "hot" signal.
pub const HOT_LISTEN_SECS: f64 = 60.0;

// ---------------- Press activity ----------------

/// Store one event. Listen events come back with the same id as playback
/// progresses: the row is updated (seconds, completion, time).
pub fn upsert_event(conn: &Connection, ev: &ActivityEvent) -> bool {
    let Some(cid) = ev.external_ref.as_deref().and_then(contact_of_ref) else { return false };
    let exists: bool = conn
        .query_row("SELECT EXISTS(SELECT 1 FROM contacts WHERE id = ?1)", params![cid], |r| r.get(0))
        .unwrap_or(false);
    if !exists {
        return false;
    }
    let campaign: Option<i64> = conn
        .query_row(
            "SELECT m.campaign_id FROM campaign_messages m WHERE m.contact_id = ?1 ORDER BY m.id DESC LIMIT 1",
            params![cid],
            |r| r.get(0),
        )
        .ok();
    let at = engine::parse_utc(&ev.at).map(fmt).unwrap_or_else(|| ev.at.clone());
    conn.execute(
        "INSERT INTO press_activity (event_id, contact_id, campaign_id, link_id, type, at, track_title, seconds, completion, rating, comment)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
         ON CONFLICT(event_id) DO UPDATE SET
            at = excluded.at,
            seconds = MAX(COALESCE(press_activity.seconds, 0), COALESCE(excluded.seconds, 0)),
            completion = MAX(COALESCE(press_activity.completion, 0), COALESCE(excluded.completion, 0)),
            comment = COALESCE(excluded.comment, press_activity.comment)",
        params![
            ev.id, cid, campaign, ev.link_id, ev.kind, at, ev.track_title, ev.seconds, ev.completion, ev.rating, ev.comment
        ],
    )
    .unwrap_or(0)
        > 0
}

/// Press kits whose activity is followed: the kit of every active residency
/// campaign, plus the kit of any other residency campaign that already handed
/// out personal links (paused or finished: a club can open its EPK link weeks
/// after the last email).
pub fn kits_to_poll(conn: &Connection) -> Vec<String> {
    let mut ids = super::active_ids(conn);
    if let Ok(mut st) = conn.prepare(
        "SELECT DISTINCT m.campaign_id FROM campaign_messages m
         WHERE m.press_url IS NOT NULL AND m.press_url <> '' ORDER BY m.campaign_id",
    ) {
        if let Ok(rows) = st.query_map([], |r| r.get::<_, i64>(0)) {
            for id in rows.flatten() {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }
    }
    let mut kits: Vec<String> = Vec::new();
    for id in ids {
        if let Some(k) = super::load(conn, id).and_then(|r| r.config.press_kit_id).filter(|k| !k.trim().is_empty()) {
            if !kits.contains(&k) {
                kits.push(k);
            }
        }
    }
    kits
}

/// Poll the activity of every followed press kit (`kits_to_poll`) since its
/// cursor (`insrt_activity_cursor`, one cursor per kit). Returns how many
/// events were stored or updated.
pub fn poll_press(conn: &Connection, client: &dyn Insrt, now: DateTime<Utc>) -> Result<usize, String> {
    let mut cursors: BTreeMap<String, String> = conn
        .query_row("SELECT value FROM settings WHERE key = 'insrt_activity_cursor'", [], |r| r.get::<_, String>(0))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let kits = kits_to_poll(conn);
    let mut n = 0;
    for kit in kits {
        let mut since = cursors
            .get(&kit)
            .cloned()
            .unwrap_or_else(|| (now - Duration::days(30)).format("%Y-%m-%dT%H:%M:%SZ").to_string());
        for _ in 0..10 {
            let page = client.activity(&since, &kit)?;
            for ev in &page.events {
                if upsert_event(conn, ev) {
                    n += 1;
                }
            }
            match page.next {
                Some(next) if next != since && !page.events.is_empty() => since = next,
                Some(next) => {
                    since = next;
                    break;
                }
                None => break,
            }
        }
        cursors.insert(kit, since);
    }
    engine::put(conn, "insrt_activity_cursor", &serde_json::to_string(&cursors).unwrap_or_default());
    Ok(n)
}

// ---------------- Activity of one contact (contact card) ----------------

/// One track (or video) the contact played, all plays summed.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TrackListen {
    pub title: String,
    pub plays: i64,
    pub seconds: f64,
    /// Best share of the track heard in one play (0..1), when known.
    pub completion: Option<f64>,
    pub last_at: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct FeedbackItem {
    pub at: String,
    pub track_title: Option<String>,
    pub rating: Option<f64>,
    pub comment: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ActivityItem {
    pub at: String,
    /// view | listen | feedback | download
    pub kind: String,
    pub track_title: Option<String>,
    pub seconds: Option<f64>,
    pub completion: Option<f64>,
    pub comment: Option<String>,
    pub campaign_id: Option<i64>,
}

/// What the contact did on their private insrt EPK link.
#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct ContactActivity {
    pub contact_id: i64,
    /// Their personal link (latest one handed out), when known.
    pub link_url: Option<String>,
    pub views: i64,
    pub first_view_at: Option<String>,
    pub last_view_at: Option<String>,
    pub downloads: i64,
    pub listen_seconds: f64,
    /// A listen of at least HOT_LISTEN_SECS.
    pub hot: bool,
    pub tracks: Vec<TrackListen>,
    pub feedback: Vec<FeedbackItem>,
    /// The latest events, newest first (50 at most).
    pub recent: Vec<ActivityItem>,
    pub last_at: Option<String>,
    /// When the activity was last fetched from insrt (any contact).
    pub synced_at: Option<String>,
}

pub fn contact_activity(conn: &Connection, contact_id: i64) -> Result<ContactActivity, String> {
    let mut out = ContactActivity { contact_id, ..Default::default() };
    let (views, first, last, downloads, secs, last_any): (i64, Option<String>, Option<String>, i64, f64, Option<String>) = conn
        .query_row(
            "SELECT
                COALESCE(SUM(type = 'view'), 0),
                MIN(CASE WHEN type = 'view' THEN at END),
                MAX(CASE WHEN type = 'view' THEN at END),
                COALESCE(SUM(type = 'download'), 0),
                COALESCE(SUM(CASE WHEN type = 'listen' THEN seconds END), 0),
                MAX(at)
             FROM press_activity WHERE contact_id = ?1",
            params![contact_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )
        .map_err(|e| e.to_string())?;
    out.views = views;
    out.first_view_at = first;
    out.last_view_at = last;
    out.downloads = downloads;
    out.listen_seconds = secs;
    out.last_at = last_any;

    let mut st = conn
        .prepare(
            "SELECT COALESCE(NULLIF(trim(track_title), ''), '?'), COUNT(*), COALESCE(SUM(seconds), 0), MAX(completion), MAX(at),
                    MAX(COALESCE(seconds, 0))
             FROM press_activity WHERE contact_id = ?1 AND type = 'listen'
             GROUP BY 1 ORDER BY 3 DESC, 1",
        )
        .map_err(|e| e.to_string())?;
    let mut best = 0.0f64;
    out.tracks = st
        .query_map(params![contact_id], |r| {
            Ok((
                TrackListen { title: r.get(0)?, plays: r.get(1)?, seconds: r.get(2)?, completion: r.get(3)?, last_at: r.get(4)? },
                r.get::<_, f64>(5)?,
            ))
        })
        .map_err(|e| e.to_string())?
        .flatten()
        .map(|(t, max)| {
            best = best.max(max);
            t
        })
        .collect();
    out.hot = best >= HOT_LISTEN_SECS;

    let mut st = conn
        .prepare(
            "SELECT at, track_title, rating, comment FROM press_activity
             WHERE contact_id = ?1 AND type = 'feedback' ORDER BY at DESC, id DESC",
        )
        .map_err(|e| e.to_string())?;
    out.feedback = st
        .query_map(params![contact_id], |r| {
            Ok(FeedbackItem { at: r.get(0)?, track_title: r.get(1)?, rating: r.get(2)?, comment: r.get(3)? })
        })
        .map_err(|e| e.to_string())?
        .flatten()
        .collect();

    let mut st = conn
        .prepare(
            "SELECT at, type, track_title, seconds, completion, comment, campaign_id FROM press_activity
             WHERE contact_id = ?1 ORDER BY at DESC, id DESC LIMIT 50",
        )
        .map_err(|e| e.to_string())?;
    out.recent = st
        .query_map(params![contact_id], |r| {
            Ok(ActivityItem {
                at: r.get(0)?,
                kind: r.get(1)?,
                track_title: r.get(2)?,
                seconds: r.get(3)?,
                completion: r.get(4)?,
                comment: r.get(5)?,
                campaign_id: r.get(6)?,
            })
        })
        .map_err(|e| e.to_string())?
        .flatten()
        .collect();

    out.link_url = conn
        .query_row(
            "SELECT press_url FROM campaign_messages WHERE contact_id = ?1 AND press_url IS NOT NULL AND press_url <> ''
             ORDER BY id DESC LIMIT 1",
            params![contact_id],
            |r| r.get(0),
        )
        .ok();
    out.synced_at = engine::get(conn, "insrt_last_poll");
    Ok(out)
}

// ---------------- Email opens ----------------

/// The tracking endpoint's opens.json (an array of {token, opened_at, count}
/// or a map token -> date | {opened_at, count}), as (token, opened_at, count).
pub fn parse_opens(raw: &str) -> Vec<(String, Option<String>, i64)> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) else { return Vec::new() };
    let one = |token: String, o: &serde_json::Value| -> (String, Option<String>, i64) {
        match o {
            serde_json::Value::String(d) => (token, Some(d.clone()), 1),
            _ => (
                token,
                o.get("opened_at").and_then(|x| x.as_str()).map(str::to_string),
                o.get("count").and_then(|x| x.as_i64()).unwrap_or(1).max(1),
            ),
        }
    };
    match v {
        serde_json::Value::Array(items) => items
            .iter()
            .filter_map(|o| o.get("token").and_then(|t| t.as_str()).map(|t| one(t.to_string(), o)))
            .collect(),
        serde_json::Value::Object(map) => map.iter().map(|(k, o)| one(k.clone(), o)).collect(),
        _ => Vec::new(),
    }
}

/// Fetch the opens recorded by the tracking pixel (same source as the
/// "Synchroniser les ouvertures" button of the Outbox), so the daily report
/// counts them without a manual sync. Returns how many emails changed.
pub fn poll_opens(conn: &Connection) -> Result<usize, String> {
    let Some(base) = crate::secrets::read_setting(conn, "tracking_base_url").filter(|b| b.trim().starts_with("http")) else {
        return Ok(0);
    };
    let raw = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?
        .get(format!("{}/opens.json", base.trim().trim_end_matches('/')))
        .send()
        .and_then(|r| r.error_for_status())
        .and_then(|r| r.text())
        .map_err(|e| format!("ouvertures : {}", e))?;
    let mut n = 0;
    for (token, at, count) in parse_opens(&raw) {
        n += conn
            .execute(
                "UPDATE emails SET opened_at = COALESCE(opened_at, ?2), open_count = MAX(open_count, ?3)
                 WHERE track_token = ?1 AND (opened_at IS NULL OR open_count < ?3)",
                params![token, at, count],
            )
            .unwrap_or(0);
    }
    Ok(n)
}

/// "Hot" contacts: listened at least a minute or left feedback on the EPK,
/// and have not answered since.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HotSignal {
    pub contact_id: i64,
    pub campaign_id: Option<i64>,
    pub name: String,
    pub kind: String,
    pub track_title: Option<String>,
    pub seconds: f64,
    pub comment: Option<String>,
    pub at: String,
}

pub fn hot_signals(conn: &Connection, campaign_id: Option<i64>) -> Vec<HotSignal> {
    let Ok(mut st) = conn.prepare(
        "SELECT p.contact_id, p.campaign_id, k.name, p.type, p.track_title, COALESCE(p.seconds, 0), p.comment, p.at
         FROM press_activity p JOIN contacts k ON k.id = p.contact_id
         WHERE ((p.type = 'listen' AND COALESCE(p.seconds, 0) >= ?2) OR p.type = 'feedback')
           AND (?1 IS NULL OR p.campaign_id = ?1)
           AND p.at >= datetime('now', '-14 days')
           AND NOT EXISTS (SELECT 1 FROM email_replies r WHERE r.kind = 'reply'
                           AND (r.contact_id = p.contact_id OR lower(trim(r.from_addr)) = lower(trim(k.email)))
                           AND r.received_at >= datetime('now', '-60 days'))
           AND k.status NOT IN ('declined','confirmed')
         ORDER BY p.type = 'feedback' DESC, p.seconds DESC, p.at DESC",
    ) else {
        return Vec::new();
    };
    let rows: Vec<HotSignal> = st
        .query_map(params![campaign_id, HOT_LISTEN_SECS], |r| {
            Ok(HotSignal {
                contact_id: r.get(0)?,
                campaign_id: r.get(1)?,
                name: r.get(2)?,
                kind: r.get(3)?,
                track_title: r.get(4)?,
                seconds: r.get(5)?,
                comment: r.get(6)?,
                at: r.get(7)?,
            })
        })
        .map(|it| it.flatten().collect())
        .unwrap_or_default();
    // One signal per contact (the strongest).
    let mut seen = std::collections::HashSet::new();
    rows.into_iter().filter(|h| seen.insert(h.contact_id)).collect()
}

// ---------------- Daily report ----------------

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Counts {
    pub sent: i64,
    pub bounced: i64,
    pub delivered: i64,
    pub opened: i64,
    pub views: i64,
    pub listens: i64,
    pub feedback: i64,
    pub downloads: i64,
    pub replies: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReplyLine {
    pub contact_id: Option<i64>,
    pub name: String,
    pub sentiment: String,
    pub intent: String,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportData {
    pub day: String,
    pub yesterday: Counts,
    pub total: Counts,
    pub by_intent: BTreeMap<String, i64>,
    pub by_sentiment: BTreeMap<String, i64>,
    pub replies_yesterday: Vec<ReplyLine>,
    pub followups_today: Vec<String>,
    pub hot: Vec<HotSignal>,
    pub next_actions: Vec<String>,
    pub autopilot: bool,
    pub paused_reason: Option<String>,
    pub audience: i64,
    pub remaining_to_send: i64,
}

fn counts(conn: &Connection, campaign_id: i64, from: &str, to: &str) -> Counts {
    let one = |sql: &str| -> i64 { conn.query_row(sql, params![campaign_id, from, to], |r| r.get(0)).unwrap_or(0) };
    let sent = one("SELECT COUNT(*) FROM emails WHERE campaign_id = ?1 AND status = 'sent' AND sent_at >= ?2 AND sent_at < ?3");
    let bounced = one(
        "SELECT COUNT(*) FROM emails e WHERE e.campaign_id = ?1 AND e.sent_at >= ?2 AND e.sent_at < ?3
           AND EXISTS (SELECT 1 FROM suppressions s WHERE s.reason = 'bounce' AND s.address = lower(trim(e.to_addr)))",
    );
    let opened = one("SELECT COUNT(*) FROM emails WHERE campaign_id = ?1 AND opened_at IS NOT NULL AND opened_at >= ?2 AND opened_at < ?3");
    let press = |t: &str| -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM press_activity WHERE campaign_id = ?1 AND at >= ?2 AND at < ?3 AND type = ?4",
            params![campaign_id, from, to, t],
            |r| r.get(0),
        )
        .unwrap_or(0)
    };
    let replies = one(
        "SELECT COUNT(*) FROM reply_insights i JOIN email_replies r ON r.id = i.email_reply_id
         WHERE i.campaign_id = ?1 AND i.intent <> 'out_of_office' AND r.received_at >= ?2 AND r.received_at < ?3",
    );
    Counts {
        sent,
        bounced,
        delivered: (sent - bounced).max(0),
        opened,
        views: press("view"),
        listens: press("listen"),
        feedback: press("feedback"),
        downloads: press("download"),
        replies,
    }
}

/// UTC bounds of a local calendar day.
fn local_day_bounds(day: NaiveDate) -> (String, String) {
    let to_utc = |d: NaiveDate| {
        chrono::Local
            .from_local_datetime(&d.and_hms_opt(0, 0, 0).expect("valid"))
            .earliest()
            .map(|t| fmt(t.with_timezone(&Utc)))
            .unwrap_or_else(|| format!("{} 00:00:00", d))
    };
    (to_utc(day), to_utc(day + Duration::days(1)))
}

pub fn build(conn: &Connection, campaign_id: i64, today: NaiveDate) -> ReportData {
    let (y_from, y_to) = local_day_bounds(today - Duration::days(1));
    let (t_from, t_to) = local_day_bounds(today);
    let yesterday = counts(conn, campaign_id, &y_from, &y_to);
    let total = counts(conn, campaign_id, "0000", "9999");
    let mut by_intent = BTreeMap::new();
    let mut by_sentiment = BTreeMap::new();
    if let Ok(mut st) = conn.prepare("SELECT intent, sentiment FROM reply_insights WHERE campaign_id = ?1") {
        for (i, s) in st
            .query_map(params![campaign_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map(|it| it.flatten().collect::<Vec<_>>())
            .unwrap_or_default()
        {
            *by_intent.entry(i).or_insert(0) += 1;
            *by_sentiment.entry(s).or_insert(0) += 1;
        }
    }
    let replies_yesterday: Vec<ReplyLine> = conn
        .prepare(
            "SELECT i.contact_id, COALESCE(k.name, r.from_addr), i.sentiment, i.intent, COALESCE(i.summary, '')
             FROM reply_insights i JOIN email_replies r ON r.id = i.email_reply_id LEFT JOIN contacts k ON k.id = i.contact_id
             WHERE i.campaign_id = ?1 AND r.received_at >= ?2 AND r.received_at < ?3 ORDER BY r.received_at",
        )
        .and_then(|mut st| {
            st.query_map(params![campaign_id, y_from, y_to], |r| {
                Ok(ReplyLine { contact_id: r.get(0)?, name: r.get(1)?, sentiment: r.get(2)?, intent: r.get(3)?, summary: r.get(4)? })
            })
            .map(|it| it.flatten().collect())
        })
        .unwrap_or_default();
    let followups_today: Vec<String> = conn
        .prepare(
            "SELECT k.name FROM campaign_messages m JOIN contacts k ON k.id = m.contact_id
             WHERE m.campaign_id = ?1 AND m.status = 'scheduled' AND m.step >= 2
               AND m.scheduled_for < ?2 ORDER BY m.scheduled_for",
        )
        .and_then(|mut st| st.query_map(params![campaign_id, t_to], |r| r.get(0)).map(|it| it.flatten().collect()))
        .unwrap_or_default();
    let _ = t_from;
    let hot = hot_signals(conn, Some(campaign_id));
    let mut next_actions: Vec<String> = Vec::new();
    if let Ok(mut st) = conn.prepare(
        "SELECT COALESCE(k.name, ''), i.intent, i.follow_up_on, i.mentioned_email FROM reply_insights i
         LEFT JOIN contacts k ON k.id = i.contact_id
         WHERE i.campaign_id = ?1 AND i.handled = 0 AND i.intent IN ('interested','question','later','wrong_person')",
    ) {
        let rows: Vec<(String, String, Option<String>, Option<String>)> = st
            .query_map(params![campaign_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .map(|it| it.flatten().collect())
            .unwrap_or_default();
        for (name, intent, fu, mentioned) in rows {
            match intent.as_str() {
                "interested" | "question" => next_actions.push(format!("Répondre à {}", name)),
                "later" => {
                    if fu.as_deref().map_or(false, |d| d <= today.to_string().as_str()) {
                        next_actions.push(format!("Relancer {} (avait dit plus tard)", name))
                    }
                }
                _ => next_actions.push(match mentioned {
                    Some(m) => format!("Trouver le bon contact pour {} (piste : {})", name, m),
                    None => format!("Trouver le bon contact pour {}", name),
                }),
            }
        }
    }
    for h in &hot {
        next_actions.push(format!("{} : signal chaud, pas encore de réponse", h.name));
    }
    let r = super::load(conn, campaign_id);
    let audience: i64 = conn
        .query_row("SELECT COUNT(*) FROM campaign_contacts WHERE campaign_id = ?1", params![campaign_id], |r| r.get(0))
        .unwrap_or(0);
    let remaining_to_send: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM campaign_messages WHERE campaign_id = ?1 AND status IN ('draft','scheduled')",
            params![campaign_id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    ReportData {
        day: today.to_string(),
        yesterday,
        total,
        by_intent,
        by_sentiment,
        replies_yesterday,
        followups_today,
        hot,
        next_actions,
        autopilot: r.as_ref().map_or(false, |r| r.autopilot),
        paused_reason: r.and_then(|r| r.paused_reason),
        audience,
        remaining_to_send,
    }
}

/// A plain summary when Claude is not configured (or fails).
pub fn rules_synthesis(d: &ReportData) -> String {
    let mut lines = vec![format!(
        "Hier : {} envoi(s), {} ouverture(s), {} écoute(s) de l'EPK, {} réponse(s).",
        d.yesterday.sent, d.yesterday.opened, d.yesterday.listens, d.yesterday.replies
    )];
    lines.push(format!(
        "Depuis le début : {} envois, {} réponses dont {} intéressée(s), {} refus.",
        d.total.sent,
        d.total.replies,
        d.by_intent.get("interested").copied().unwrap_or(0) + d.by_intent.get("question").copied().unwrap_or(0),
        d.by_intent.get("not_interested").copied().unwrap_or(0)
    ));
    if !d.next_actions.is_empty() {
        lines.push(format!("À faire : {}.", d.next_actions.iter().take(3).cloned().collect::<Vec<_>>().join(" ; ")));
    }
    lines.push(match (&d.paused_reason, d.autopilot) {
        (Some(r), _) => format!("Pilote auto en pause : {}", r),
        (None, true) => format!("Pilote auto actif, {} message(s) encore à envoyer.", d.remaining_to_send),
        (None, false) => "Pilote auto coupé.".into(),
    });
    lines.join("\n")
}

const SYNTHESIS_SYSTEM: &str = "You write the daily note of a residency booking campaign for a DJ and his manager. \
French, 4 to 6 short lines, plain text, no markdown, no emojis, never an em dash or en dash. Factual: only \
what the data says. Lead with what changed yesterday, then the overall picture, then the one or two things to \
do today. Warm but concise.";

pub fn synthesis(conn: &Connection, campaign_id: i64, d: &ReportData, llm: Option<&dyn Llm>) -> String {
    let Some(llm) = llm else { return rules_synthesis(d) };
    let req = LlmRequest {
        model: claude::drafting_model(conn),
        max_tokens: 300,
        system: vec![SystemBlock { text: SYNTHESIS_SYSTEM.into(), cache: false }],
        user: serde_json::to_string(d).unwrap_or_default(),
        schema: None,
    };
    match llm.complete(&req) {
        Ok(r) => {
            claude::log_usage(conn, "synthesis", Some(campaign_id), &r.model, &r.usage);
            let t = claude::no_dashes(r.text.trim());
            if t.is_empty() {
                rules_synthesis(d)
            } else {
                t
            }
        }
        Err(e) => {
            super::log(conn, Some(campaign_id), "warn", "synthesis_failed", &e);
            rules_synthesis(d)
        }
    }
}

/// Build and store today's report. False when the campaign has no audience
/// yet (nothing to report) or the report already exists.
pub fn build_and_store(conn: &Connection, campaign_id: i64, today: NaiveDate, llm: Option<&dyn Llm>) -> Result<bool, String> {
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM campaign_reports WHERE campaign_id = ?1 AND day = ?2)",
            params![campaign_id, today.to_string()],
            |r| r.get(0),
        )
        .unwrap_or(false);
    let data = build(conn, campaign_id, today);
    if exists || data.audience == 0 {
        return Ok(false);
    }
    let text = synthesis(conn, campaign_id, &data, llm);
    conn.execute(
        "INSERT OR REPLACE INTO campaign_reports (campaign_id, day, data, synthesis) VALUES (?1, ?2, ?3, ?4)",
        params![campaign_id, today.to_string(), serde_json::to_string(&data).map_err(|e| e.to_string())?, text],
    )
    .map_err(|e| e.to_string())?;
    super::log(conn, Some(campaign_id), "info", "report", &format!("Rapport du {} prêt", today));
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::residency::claude::fake::FakeLlm;
    use crate::commands::residency::engine::tests::setup;
    use crate::commands::residency::insrt::{ActivityPage, PressKit, PressLink, Recipient};
    use std::sync::Mutex;

    struct FakeInsrt {
        pages: Mutex<Vec<ActivityPage>>,
    }
    impl Insrt for FakeInsrt {
        fn press_kits(&self, _: &str) -> Result<Vec<PressKit>, String> {
            Ok(vec![])
        }
        fn press_link(&self, _: &str, _: i64, _: &Recipient, _: &str) -> Result<PressLink, String> {
            Err("unused".into())
        }
        fn activity(&self, _: &str, _: &str) -> Result<ActivityPage, String> {
            Ok(self.pages.lock().unwrap().pop().unwrap_or(ActivityPage { events: vec![], next: None }))
        }
    }

    fn ev(id: &str, cid: i64, kind: &str, secs: Option<f64>) -> ActivityEvent {
        ActivityEvent {
            id: id.into(),
            link_id: Some("ln".into()),
            external_ref: Some(format!("crm:{}", cid)),
            kind: kind.into(),
            at: Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            track_title: Some("Mix ADE".into()),
            seconds: secs,
            completion: None,
            rating: None,
            comment: None,
        }
    }

    #[test]
    fn press_polling_upserts_and_hot_signals() {
        let c = setup();
        let mut cfg = crate::commands::residency::load(&c, 1).unwrap().config;
        cfg.press_kit_id = Some("pk1".into());
        crate::commands::residency::save_config(&c, 1, &cfg).unwrap();
        let client = FakeInsrt {
            pages: Mutex::new(vec![
                // Popped last: the same listen, further along.
                ActivityPage { events: vec![ev("listen:1", 10, "listen", Some(240.0))], next: Some("2026-10-29T12:00:00Z".into()) },
                ActivityPage {
                    events: vec![ev("event:1", 10, "view", None), ev("listen:1", 10, "listen", Some(20.0)), ev("x", 999, "view", None)],
                    next: Some("2026-10-29T11:00:00Z".into()),
                },
            ]),
        };
        let n = poll_press(&c, &client, Utc::now()).unwrap();
        assert_eq!(n, 3);
        let (count, secs): (i64, f64) = c
            .query_row("SELECT COUNT(*), MAX(seconds) FROM press_activity WHERE type = 'listen'", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!((count, secs), (1, 240.0));
        let cursor: String = c.query_row("SELECT value FROM settings WHERE key = 'insrt_activity_cursor'", [], |r| r.get(0)).unwrap();
        assert!(cursor.contains("pk1") && cursor.contains("2026-10-29T12:00:00Z"));
        let hot = hot_signals(&c, Some(1));
        assert_eq!(hot.len(), 1);
        assert_eq!(hot[0].seconds, 240.0);
    }

    #[test]
    fn kits_of_finished_campaigns_stay_followed() {
        let c = setup();
        let mut cfg = crate::commands::residency::load(&c, 1).unwrap().config;
        cfg.press_kit_id = Some("pk1".into());
        crate::commands::residency::save_config(&c, 1, &cfg).unwrap();
        c.execute_batch("UPDATE campaigns SET status = 'done' WHERE id = 1").unwrap();
        // Finished, no link handed out: nothing to follow.
        assert!(kits_to_poll(&c).is_empty());
        c.execute_batch(
            "INSERT INTO campaign_messages (campaign_id, contact_id, step, status, press_url) VALUES (1, 10, 7, 'sent', 'https://insrt.studio/fr/p/abc')",
        )
        .unwrap();
        assert_eq!(kits_to_poll(&c), vec!["pk1".to_string()]);
    }

    #[test]
    fn contact_card_activity() {
        let c = setup();
        c.execute_batch(
            "INSERT INTO campaign_messages (campaign_id, contact_id, step, status, press_url) VALUES (1, 10, 7, 'sent', 'https://insrt.studio/fr/p/abc');
             INSERT INTO press_activity (event_id, contact_id, campaign_id, type, at, track_title, seconds, completion, comment) VALUES
               ('event:1', 10, 1, 'view', '2026-10-28 13:59:00', NULL, NULL, NULL, NULL),
               ('event:2', 10, 1, 'view', '2026-10-29 09:00:00', NULL, NULL, NULL, NULL),
               ('listen:1', 10, 1, 'listen', '2026-10-28 14:04:00', 'Mix ADE', 240, 0.4, NULL),
               ('listen:2', 10, 1, 'listen', '2026-10-29 09:05:00', 'Mix ADE', 30, 0.05, NULL),
               ('listen:3', 10, 1, 'listen', '2026-10-29 09:10:00', 'PIERS FM Vol. 4', 20, 0.02, NULL),
               ('feedback:1', 10, 1, 'feedback', '2026-10-29 09:12:00', 'Mix ADE', NULL, NULL, 'Top'),
               ('event:3', 10, 1, 'download', '2026-10-29 09:13:00', NULL, NULL, NULL, NULL),
               ('event:9', 11, 1, 'view', '2026-10-29 10:00:00', NULL, NULL, NULL, NULL);",
        )
        .unwrap();
        let a = contact_activity(&c, 10).unwrap();
        assert_eq!(a.views, 2);
        assert_eq!(a.first_view_at.as_deref(), Some("2026-10-28 13:59:00"));
        assert_eq!(a.last_view_at.as_deref(), Some("2026-10-29 09:00:00"));
        assert_eq!(a.downloads, 1);
        assert_eq!(a.listen_seconds, 290.0);
        assert!(a.hot);
        assert_eq!(a.tracks.len(), 2);
        assert_eq!(a.tracks[0].title, "Mix ADE");
        assert_eq!((a.tracks[0].plays, a.tracks[0].seconds, a.tracks[0].completion), (2, 270.0, Some(0.4)));
        assert_eq!(a.feedback[0].comment.as_deref(), Some("Top"));
        assert_eq!(a.recent.len(), 7);
        assert_eq!(a.recent[0].kind, "download");
        assert_eq!(a.last_at.as_deref(), Some("2026-10-29 09:13:00"));
        assert_eq!(a.link_url.as_deref(), Some("https://insrt.studio/fr/p/abc"));
        // A contact without activity: an empty card, not an error.
        let none = contact_activity(&c, 99).unwrap();
        assert_eq!((none.views, none.listen_seconds, none.hot), (0, 0.0, false));
        assert!(none.tracks.is_empty() && none.recent.is_empty());
    }

    #[test]
    fn opens_formats() {
        let a = parse_opens(r#"[{"token":"t1","opened_at":"2026-10-28 10:00:00","count":3}]"#);
        assert_eq!(a, vec![("t1".to_string(), Some("2026-10-28 10:00:00".to_string()), 3)]);
        let mut b = parse_opens(r#"{"t2":"2026-10-28 11:00:00","t3":{"opened_at":"2026-10-28 12:00:00"}}"#);
        b.sort();
        assert_eq!(b[0].0, "t2");
        assert_eq!(b[1].2, 1);
        assert!(parse_opens("<html>").is_empty());
    }

    #[test]
    fn report_aggregation_and_synthesis() {
        let c = setup();
        let today = chrono::Local::now().date_naive();
        let (y_from, _) = local_day_bounds(today - Duration::days(1));
        // Yesterday: two sends (one bounced), one open, one reply classified interested.
        c.execute_batch(&format!(
            "INSERT INTO emails (id, contact_id, campaign_id, to_addr, subject, body, status, sent_at, opened_at) VALUES
               (1, 10, 1, 'booking@cluba.fr', 's', 'b', 'sent', datetime('{y}', '+10 hours'), datetime('{y}', '+11 hours')),
               (2, 11, 1, 'booking@clubb.co.uk', 's', 'b', 'sent', datetime('{y}', '+10 hours'), NULL);
             INSERT INTO suppressions (address, reason) VALUES ('booking@clubb.co.uk', 'bounce');
             INSERT INTO email_replies (id, email_id, contact_id, from_addr, message_id, kind, received_at) VALUES
               (1, 1, 10, 'booking@cluba.fr', '<r>', 'reply', datetime('{y}', '+12 hours'));
             INSERT INTO reply_insights (email_reply_id, contact_id, campaign_id, sentiment, intent, summary)
               VALUES (1, 10, 1, 'positive', 'interested', 'Partant pour un call');
             INSERT INTO press_activity (event_id, contact_id, campaign_id, type, at, seconds)
               VALUES ('listen:9', 10, 1, 'listen', datetime('{y}', '+13 hours'), 300);",
            y = y_from
        ))
        .unwrap();
        let d = build(&c, 1, today);
        assert_eq!(d.yesterday.sent, 2);
        assert_eq!(d.yesterday.bounced, 1);
        assert_eq!(d.yesterday.delivered, 1);
        assert_eq!(d.yesterday.opened, 1);
        assert_eq!(d.yesterday.listens, 1);
        assert_eq!(d.yesterday.replies, 1);
        assert_eq!(d.total.sent, 2);
        assert_eq!(d.by_intent.get("interested"), Some(&1));
        assert_eq!(d.replies_yesterday[0].summary, "Partant pour un call");
        assert!(d.next_actions.iter().any(|a| a.contains("Répondre à Club A")));
        assert_eq!(d.audience, 2);
        // They already answered: not a hot signal.
        assert!(d.hot.is_empty());
        let rules = rules_synthesis(&d);
        assert!(rules.contains("2 envoi(s)") && !rules.contains('—'));

        let llm = FakeLlm::new(&["Hier, deux envois — une réponse positive.\nÀ faire : répondre au Club A."]);
        assert!(build_and_store(&c, 1, today, Some(&llm)).unwrap());
        let s: String = c.query_row("SELECT synthesis FROM campaign_reports WHERE campaign_id = 1", [], |r| r.get(0)).unwrap();
        assert!(s.starts_with("Hier, deux envois, une réponse positive."));
        assert_eq!(llm.seen.lock().unwrap()[0].max_tokens, 300);
        // Once per day.
        assert!(!build_and_store(&c, 1, today, Some(&llm)).unwrap());
    }
}
