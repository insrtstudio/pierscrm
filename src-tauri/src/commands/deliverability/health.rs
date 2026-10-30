//! Sender health over the last 7 days, per sending address: bounce,
//! complaint, unsubscribe and reply rates. Above the thresholds every
//! automated send (bulk, residency autopilot) is paused until the user
//! resumes from the banner; a resume restarts the window, so the sends that
//! caused the pause do not trip it again at once.

use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use super::{fmt, get, parse_utc, put};

pub const WINDOW_DAYS: i64 = 7;
pub const PAUSE_KEY: &str = "deliverability_pause";
pub const RESET_KEY: &str = "deliverability_health_reset_at";
pub const THRESHOLDS_KEY: &str = "deliverability_thresholds";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Thresholds {
    /// Percent of sends that bounced.
    pub bounce_pct: f64,
    /// Percent of sends reported as spam (feedback loop reports).
    pub complaint_pct: f64,
    /// Percent of recipients who unsubscribed.
    pub unsubscribe_pct: f64,
    /// Rates are computed over at least this many sends, so two bounces in the
    /// first five emails do not read as 40 %, and one in twenty still counts.
    pub min_sample: u32,
}

impl Default for Thresholds {
    fn default() -> Self {
        Thresholds { bounce_pct: 5.0, complaint_pct: 0.3, unsubscribe_pct: 5.0, min_sample: 20 }
    }
}

impl Thresholds {
    pub fn sanitized(mut self) -> Self {
        self.bounce_pct = self.bounce_pct.clamp(0.5, 20.0);
        self.complaint_pct = self.complaint_pct.clamp(0.05, 5.0);
        self.unsubscribe_pct = self.unsubscribe_pct.clamp(0.5, 50.0);
        self.min_sample = self.min_sample.clamp(5, 500);
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Stats {
    pub sender: String,
    pub sent: u32,
    pub bounces: u32,
    pub complaints: u32,
    pub unsubscribes: u32,
    pub replies: u32,
    pub bounce_rate: f64,
    pub complaint_rate: f64,
    pub unsubscribe_rate: f64,
    pub reply_rate: f64,
}

/// One row of the window: (sender, address, bounced, complained, unsubscribed, replied).
pub type Row = (String, String, bool, bool, bool, bool);

fn pct(n: u32, d: u32) -> f64 {
    if d == 0 {
        0.0
    } else {
        (n as f64 * 1000.0 / d as f64).round() / 10.0
    }
}

/// Per-sender stats from the window's rows. A recipient counts once per
/// event kind (two sends to a bounced address are one bounce).
pub fn aggregate(rows: &[Row]) -> Vec<Stats> {
    use std::collections::{BTreeMap, HashSet};
    #[derive(Default)]
    struct Acc {
        sent: u32,
        b: HashSet<String>,
        c: HashSet<String>,
        u: HashSet<String>,
        r: u32,
    }
    let mut by: BTreeMap<String, Acc> = BTreeMap::new();
    for (sender, addr, b, c, u, r) in rows {
        let a = by.entry(sender.clone()).or_default();
        a.sent += 1;
        if *b {
            a.b.insert(addr.clone());
        }
        if *c {
            a.c.insert(addr.clone());
        }
        if *u {
            a.u.insert(addr.clone());
        }
        if *r {
            a.r += 1;
        }
    }
    by.into_iter()
        .map(|(sender, a)| {
            let (b, c, u) = (a.b.len() as u32, a.c.len() as u32, a.u.len() as u32);
            Stats {
                sender,
                sent: a.sent,
                bounces: b,
                complaints: c,
                unsubscribes: u,
                replies: a.r,
                bounce_rate: pct(b, a.sent),
                complaint_rate: pct(c, a.sent),
                unsubscribe_rate: pct(u, a.sent),
                reply_rate: pct(a.r, a.sent),
            }
        })
        .collect()
}

/// Why sending must pause, if a threshold is crossed (strictly above).
pub fn verdict(s: &Stats, th: &Thresholds) -> Option<String> {
    let d = s.sent.max(th.min_sample) as f64;
    let over = |n: u32, limit: f64| n as f64 * 100.0 / d > limit;
    if over(s.bounces, th.bounce_pct) {
        return Some(format!(
            "Taux de rebond trop élevé pour {} : {} adresse(s) rejetée(s) sur {} envois en 7 jours (seuil {} %). Les messageries pénalisent vite un expéditeur qui écrit à des adresses inexistantes. Nettoyez les adresses douteuses (Prospection) avant de reprendre.",
            s.sender, s.bounces, s.sent, th.bounce_pct
        ));
    }
    if over(s.complaints, th.complaint_pct) {
        return Some(format!(
            "Signalements spam pour {} : {} sur {} envois en 7 jours (seuil {} %). Au-delà de 0,3 %, Gmail bloque l'expéditeur. Revoyez le ciblage et le message avant de reprendre.",
            s.sender, s.complaints, s.sent, th.complaint_pct
        ));
    }
    if over(s.unsubscribes, th.unsubscribe_pct) {
        return Some(format!(
            "Beaucoup de désinscriptions pour {} : {} sur {} envois en 7 jours (seuil {} %). Le ciblage ou le message ne convient pas, revoyez-les avant de reprendre.",
            s.sender, s.unsubscribes, s.sent, th.unsubscribe_pct
        ));
    }
    None
}

// ---------------- DB side ----------------

pub fn load_thresholds(conn: &Connection) -> Thresholds {
    get(conn, THRESHOLDS_KEY)
        .and_then(|j| serde_json::from_str::<Thresholds>(&j).ok())
        .unwrap_or_default()
        .sanitized()
}

pub fn save_thresholds(conn: &Connection, th: Thresholds) -> Result<Thresholds, String> {
    let th = th.sanitized();
    put(conn, THRESHOLDS_KEY, &serde_json::to_string(&th).map_err(|e| e.to_string())?);
    Ok(th)
}

/// The configured sender address (lowercased), if any.
pub fn current_sender(conn: &Connection) -> String {
    get(conn, "smtp")
        .and_then(|j| serde_json::from_str::<serde_json::Value>(&j).ok())
        .and_then(|v| v.get("from_email").and_then(|f| f.as_str()).map(crate::commands::outbox::norm_addr))
        .unwrap_or_default()
}

/// Start of the window: 7 days back, or the last resume if more recent.
pub fn window_start(conn: &Connection, now: DateTime<Utc>) -> DateTime<Utc> {
    let w = now - Duration::days(WINDOW_DAYS);
    match get(conn, RESET_KEY).and_then(|s| parse_utc(&s)) {
        Some(r) if r > w => r,
        _ => w,
    }
}

/// Sends of the window (sent, or rejected as a hard bounce) with their
/// outcome flags. Emails sent before `from_addr` existed belong to the
/// current sender.
pub fn window_rows(conn: &Connection, since: DateTime<Utc>) -> Vec<Row> {
    let me = current_sender(conn);
    let Ok(mut st) = conn.prepare(
        "SELECT COALESCE(NULLIF(e.from_addr, ''), ?2), lower(trim(e.to_addr)),
                EXISTS(SELECT 1 FROM suppressions s WHERE s.reason = 'bounce' AND s.address = lower(trim(e.to_addr))) AS b,
                EXISTS(SELECT 1 FROM email_replies r WHERE r.kind = 'complaint' AND r.email_id = e.id),
                EXISTS(SELECT 1 FROM suppressions s WHERE s.reason = 'unsubscribe' AND s.address = lower(trim(e.to_addr))
                        AND s.created_at >= e.sent_at),
                e.replied_at IS NOT NULL
         FROM emails e
         WHERE e.sent_at >= ?1 AND (e.status = 'sent' OR (e.status = 'failed' AND
               EXISTS(SELECT 1 FROM suppressions s WHERE s.reason = 'bounce' AND s.address = lower(trim(e.to_addr)))))",
    ) else {
        return Vec::new();
    };
    st.query_map(params![fmt(since), me], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))
    })
    .map(|it| it.flatten().collect())
    .unwrap_or_default()
}

pub fn stats(conn: &Connection, now: DateTime<Utc>) -> Vec<Stats> {
    aggregate(&window_rows(conn, window_start(conn, now)))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pause {
    pub reason: String,
    pub since: String,
    pub sender: String,
}

pub fn current_pause(conn: &Connection) -> Option<Pause> {
    get(conn, PAUSE_KEY).and_then(|j| serde_json::from_str(&j).ok())
}

/// Re-evaluate the current sender; pause automated sending when a threshold
/// is crossed. Returns the pause in force, if any.
pub fn refresh(conn: &Connection, now: DateTime<Utc>) -> Option<Pause> {
    if let Some(p) = current_pause(conn) {
        return Some(p);
    }
    let me = current_sender(conn);
    let th = load_thresholds(conn);
    let reason = stats(conn, now)
        .into_iter()
        .filter(|s| me.is_empty() || s.sender == me)
        .find_map(|s| verdict(&s, &th).map(|r| (s.sender, r)));
    let (sender, reason) = reason?;
    let p = Pause { reason: reason.clone(), since: fmt(now), sender };
    put(conn, PAUSE_KEY, &serde_json::to_string(&p).unwrap_or_default());
    crate::commands::residency::log(conn, None, "pause", "deliverability_pause", &reason);
    Some(p)
}

/// The user dealt with the cause: lift the pause and restart the window.
pub fn resume(conn: &Connection, now: DateTime<Utc>) {
    let _ = conn.execute("DELETE FROM settings WHERE key = ?1", params![PAUSE_KEY]);
    put(conn, RESET_KEY, &fmt(now));
    crate::commands::residency::log(conn, None, "info", "deliverability_resume", "Envois automatiques repris par l'utilisateur.");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(b: bool, c: bool, u: bool, r: bool, addr: &str) -> Row {
        ("me@insrt.fr".into(), addr.into(), b, c, u, r)
    }

    #[test]
    fn aggregation_and_thresholds() {
        let mut rows: Vec<Row> = (0..40).map(|i| row(false, false, false, i % 10 == 0, &format!("a{}@x.fr", i))).collect();
        rows.push(row(true, false, false, false, "gone@x.fr"));
        rows.push(row(true, false, false, false, "gone@x.fr")); // same address twice: one bounce
        let s = &aggregate(&rows)[0];
        assert_eq!((s.sent, s.bounces, s.replies), (42, 1, 4));
        assert_eq!(s.bounce_rate, 2.4);
        assert_eq!(s.reply_rate, 9.5);
        let th = Thresholds::default();
        assert_eq!(verdict(s, &th), None);
        // 3 bounces / 42 = 7.1 % > 5 %.
        rows.push(row(true, false, false, false, "b2@x.fr"));
        rows.push(row(true, false, false, false, "b3@x.fr"));
        assert!(verdict(&aggregate(&rows)[0], &th).unwrap().contains("rebond"));
    }

    #[test]
    fn small_samples_and_complaints() {
        let th = Thresholds::default();
        // 1 bounce in 3 sends: 1/20 = 5 %, not above.
        let s = Stats { sender: "me".into(), sent: 3, bounces: 1, ..Default::default() };
        assert_eq!(verdict(&s, &th), None);
        let s = Stats { sender: "me".into(), sent: 3, bounces: 2, ..Default::default() };
        assert!(verdict(&s, &th).is_some());
        // A single spam report over 200 sends is 0.5 % > 0.3 %.
        let s = Stats { sender: "me".into(), sent: 200, complaints: 1, ..Default::default() };
        assert!(verdict(&s, &th).unwrap().contains("spam"));
        let s = Stats { sender: "me".into(), sent: 400, complaints: 1, ..Default::default() };
        assert_eq!(verdict(&s, &th), None);
        // Unsubscribes: 1 in 20 is fine, 2 is not.
        let s = Stats { sender: "me".into(), sent: 20, unsubscribes: 1, ..Default::default() };
        assert_eq!(verdict(&s, &th), None);
        let s = Stats { sender: "me".into(), sent: 20, unsubscribes: 2, ..Default::default() };
        assert!(verdict(&s, &th).unwrap().contains("désinscriptions"));
        let t = Thresholds { bounce_pct: 0.0, complaint_pct: 99.0, unsubscribe_pct: 0.0, min_sample: 0 }.sanitized();
        assert_eq!((t.bounce_pct, t.complaint_pct, t.min_sample), (0.5, 5.0, 5));
    }

    #[test]
    fn pause_and_resume_on_db() {
        let c = crate::commands::residency::testdb::conn();
        put(&c, "smtp", r#"{"from_email":"Booking@insrt.fr"}"#);
        let now = Utc::now();
        for i in 0..20 {
            c.execute(
                "INSERT INTO emails (to_addr, subject, body, status, sent_at) VALUES (?1, 's', 'b', 'sent', ?2)",
                params![format!("a{}@x.fr", i), fmt(now - Duration::hours(1))],
            )
            .unwrap();
        }
        // Old sends (8 days) are out of the window.
        c.execute(
            "INSERT INTO emails (to_addr, subject, body, status, sent_at, from_addr) VALUES ('old@x.fr', 's', 'b', 'sent', ?1, 'other@x.fr')",
            params![fmt(now - Duration::days(8))],
        )
        .unwrap();
        // A hard SMTP rejection counts as an attempt and a bounce.
        c.execute(
            "INSERT INTO emails (to_addr, subject, body, status, sent_at) VALUES ('dead@x.fr', 's', 'b', 'failed', ?1)",
            params![fmt(now - Duration::minutes(30))],
        )
        .unwrap();
        c.execute("INSERT INTO suppressions (address, reason) VALUES ('dead@x.fr', 'bounce')", []).unwrap();
        let st = stats(&c, now);
        assert_eq!(st.len(), 1);
        assert_eq!((st[0].sender.as_str(), st[0].sent, st[0].bounces), ("booking@insrt.fr", 21, 1));
        assert_eq!(refresh(&c, now), None);
        // A second bounce: 2/21 = 9.5 %, paused.
        c.execute("INSERT INTO suppressions (address, reason) VALUES ('a3@x.fr', 'bounce')", []).unwrap();
        let p = refresh(&c, now).unwrap();
        assert!(p.reason.contains("rebond"));
        assert_eq!(current_pause(&c), Some(p));
        // Resume: the window restarts, nothing left to judge.
        resume(&c, now);
        assert_eq!(current_pause(&c), None);
        assert_eq!(refresh(&c, now + Duration::seconds(1)), None);
        assert!(stats(&c, now + Duration::seconds(1)).is_empty());
    }
}
