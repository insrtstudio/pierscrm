//! Strike while it's warm: a target who really looked at the EPK gets the
//! next follow-up the next day instead of on its J+5 / J+12 slot.
//!
//! "Really looked" excludes the reflex hits of mail security scanners: they
//! open every link of an incoming email within a minute or two of delivery
//! (the first Dutch sends had their "views" 30 to 90 seconds after going
//! out). Counted as interest:
//! - a listen of at least HOT_LISTEN_SECS, a comment, a download;
//! - a page view at least SCANNER_GRACE_MIN after the first email went out.
//!
//! Each (campaign, target) is acted on once (`signal_actions`). Nothing is
//! rewritten: the follow-up keeps its validated text and thread subject, only
//! its date moves, and the sending window, budget and daily cap still apply.

use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection};

use super::engine::fmt;
use super::report::HOT_LISTEN_SECS;

/// Views closer than this to the send are a scanner's, not a person's.
pub const SCANNER_GRACE_MIN: i64 = 15;
/// Signals older than this are not acted on (the follow-up may have gone).
pub const LOOKBACK_DAYS: i64 = 4;
/// The advanced follow-up goes out no sooner than this after the signal.
pub const ADVANCE_HOURS: i64 = 18;

pub fn migrate(conn: &Connection) {
    let _ = conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS signal_actions (
            campaign_id INTEGER NOT NULL,
            contact_id  INTEGER NOT NULL,
            kind        TEXT NOT NULL,
            message_id  INTEGER,
            acted_at    TEXT NOT NULL DEFAULT (datetime('now')),
            PRIMARY KEY (campaign_id, contact_id)
        );",
    );
}

#[derive(Debug, Clone, PartialEq)]
pub struct Warm {
    pub campaign_id: i64,
    pub contact_id: i64,
    pub name: String,
    /// "listen", "feedback", "download" or "view".
    pub kind: String,
    pub at: String,
}

/// Targets with a human signal in the lookback window, not acted on yet, who
/// have not replied and are still in sequence. One row per target, the
/// strongest signal first.
pub fn warm_targets(conn: &Connection, now: DateTime<Utc>) -> Vec<Warm> {
    let since = fmt(now - Duration::days(LOOKBACK_DAYS));
    let Ok(mut st) = conn.prepare(
        "SELECT p.campaign_id, p.contact_id, k.name, p.type, p.at
         FROM press_activity p
         JOIN contacts k ON k.id = p.contact_id
         JOIN campaign_messages m1 ON m1.campaign_id = p.campaign_id AND m1.contact_id = p.contact_id
                                  AND m1.step = 1 AND m1.status = 'sent'
         LEFT JOIN emails e ON e.id = m1.sent_email_id
         WHERE p.at >= ?1 AND p.campaign_id IS NOT NULL
           AND (   (p.type = 'listen' AND COALESCE(p.seconds, 0) >= ?2)
                OR p.type IN ('feedback', 'download')
                OR (p.type = 'view' AND p.at >= datetime(COALESCE(e.sent_at, m1.updated_at), ?3)))
           AND NOT EXISTS (SELECT 1 FROM signal_actions s WHERE s.campaign_id = p.campaign_id AND s.contact_id = p.contact_id)
           AND NOT EXISTS (SELECT 1 FROM email_replies r WHERE r.kind = 'reply' AND r.contact_id = p.contact_id)
           AND k.status NOT IN ('declined', 'confirmed', 'in_discussion')
         ORDER BY CASE p.type WHEN 'feedback' THEN 0 WHEN 'download' THEN 1 WHEN 'listen' THEN 2 ELSE 3 END, p.at",
    ) else {
        return Vec::new();
    };
    let grace = format!("+{} minutes", SCANNER_GRACE_MIN);
    let rows: Vec<Warm> = st
        .query_map(params![since, HOT_LISTEN_SECS, grace], |r| {
            Ok(Warm { campaign_id: r.get(0)?, contact_id: r.get(1)?, name: r.get(2)?, kind: r.get(3)?, at: r.get(4)? })
        })
        .map(|it| it.flatten().collect())
        .unwrap_or_default();
    let mut seen = std::collections::HashSet::new();
    rows.into_iter().filter(|w| seen.insert((w.campaign_id, w.contact_id))).collect()
}

fn signal_words(kind: &str) -> &'static str {
    match kind {
        "feedback" => "a laissé un commentaire sur l'EPK",
        "download" => "a téléchargé un morceau",
        "listen" => "a écouté le mix",
        _ => "a ouvert la page presse",
    }
}

/// Bring each warm target's next scheduled follow-up forward. Returns the
/// lines logged ("Paard a écouté le mix, relance avancée au …").
pub fn act(conn: &Connection, now: DateTime<Utc>) -> Vec<String> {
    let mut out = Vec::new();
    for w in warm_targets(conn, now) {
        let target = now + Duration::hours(ADVANCE_HOURS);
        let next: Option<(i64, Option<String>)> = conn
            .query_row(
                "SELECT id, scheduled_for FROM campaign_messages
                 WHERE campaign_id = ?1 AND contact_id = ?2 AND step >= 2 AND status = 'scheduled'
                 ORDER BY step LIMIT 1",
                params![w.campaign_id, w.contact_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .ok();
        let mut moved: Option<i64> = None;
        if let Some((id, when)) = next {
            let later = when.as_deref().and_then(super::engine::parse_utc).is_none_or(|t| t > target);
            if later {
                let _ = conn.execute(
                    "UPDATE campaign_messages SET scheduled_for = ?2, note = ?3, updated_at = datetime('now') WHERE id = ?1",
                    params![id, fmt(target), format!("Avancée : {}", signal_words(&w.kind))],
                );
                moved = Some(id);
            }
        }
        let _ = conn.execute(
            "INSERT OR IGNORE INTO signal_actions (campaign_id, contact_id, kind, message_id) VALUES (?1, ?2, ?3, ?4)",
            params![w.campaign_id, w.contact_id, w.kind, moved],
        );
        let line = match moved {
            Some(_) => format!(
                "{} {}, relance avancée à demain ({})",
                w.name,
                signal_words(&w.kind),
                chrono::DateTime::<chrono::Local>::from(target).format("%d/%m")
            ),
            None => format!("{} {}, aucune relance à avancer", w.name, signal_words(&w.kind)),
        };
        super::log(conn, Some(w.campaign_id), "info", "warm_signal", &line);
        out.push(line);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::residency::testdb;

    fn setup(c: &Connection, now: DateTime<Utc>) {
        migrate(c);
        let sent = fmt(now - Duration::hours(30));
        c.execute_batch(&format!(
            "INSERT INTO artists (id, name) VALUES (1, 'Piers');
             INSERT INTO campaigns (id, name, kind, artist_id) VALUES (1, 'R', 'residency', 1);
             INSERT INTO contacts (id, category, name, email, status) VALUES
                (10, 'venue', 'Paard', 'info@paard.nl', 'contacted'),
                (11, 'venue', 'Pllek', 'info@pllek.nl', 'contacted'),
                (12, 'venue', 'BIRD', 'info@bird.nl', 'contacted');
             INSERT INTO emails (id, contact_id, to_addr, subject, body, status, sent_at) VALUES
                (1, 10, 'info@paard.nl', 's', 'b', 'sent', '{sent}'),
                (2, 11, 'info@pllek.nl', 's', 'b', 'sent', '{sent}'),
                (3, 12, 'info@bird.nl', 's', 'b', 'sent', '{sent}');
             INSERT INTO campaign_messages (campaign_id, contact_id, step, status, sent_email_id, updated_at) VALUES
                (1, 10, 1, 'sent', 1, '{sent}'), (1, 11, 1, 'sent', 2, '{sent}'), (1, 12, 1, 'sent', 3, '{sent}');
             INSERT INTO campaign_messages (campaign_id, contact_id, step, status, scheduled_for) VALUES
                (1, 10, 2, 'scheduled', '{far}'), (1, 11, 2, 'scheduled', '{far}'), (1, 12, 2, 'scheduled', '{far}');",
            far = fmt(now + Duration::days(4)),
        ))
        .unwrap();
    }

    fn activity(c: &Connection, id: &str, contact: i64, kind: &str, at: DateTime<Utc>, secs: f64) {
        c.execute(
            "INSERT INTO press_activity (event_id, contact_id, campaign_id, type, at, seconds) VALUES (?1, ?2, 1, ?3, ?4, ?5)",
            params![id, contact, kind, fmt(at), secs],
        )
        .unwrap();
    }

    #[test]
    fn scanner_views_ignored_human_signals_advance() {
        let c = testdb::conn();
        let now = Utc::now();
        setup(&c, now);
        let sent = now - Duration::hours(30);
        // Scanner: 40 s after delivery. Short listen: 20 s.
        activity(&c, "a", 10, "view", sent + Duration::seconds(40), 0.0);
        activity(&c, "b", 11, "listen", sent + Duration::hours(3), 20.0);
        assert!(warm_targets(&c, now).is_empty());
        // A real view hours later, a real listen.
        activity(&c, "c", 10, "view", sent + Duration::hours(5), 0.0);
        activity(&c, "d", 11, "listen", sent + Duration::hours(6), 240.0);
        let w = warm_targets(&c, now);
        assert_eq!(w.len(), 2);
        assert_eq!(w[0].kind, "listen");
        let lines = act(&c, now);
        assert_eq!(lines.len(), 2);
        let when: String = c
            .query_row("SELECT scheduled_for FROM campaign_messages WHERE contact_id = 11 AND step = 2", [], |r| r.get(0))
            .unwrap();
        assert_eq!(when, fmt(now + Duration::hours(ADVANCE_HOURS)));
        // BIRD showed nothing: untouched. Acted targets are not acted on twice.
        let bird: String = c
            .query_row("SELECT scheduled_for FROM campaign_messages WHERE contact_id = 12 AND step = 2", [], |r| r.get(0))
            .unwrap();
        assert_eq!(bird, fmt(now + Duration::days(4)));
        assert!(act(&c, now).is_empty());
    }

    #[test]
    fn replied_targets_are_left_alone() {
        let c = testdb::conn();
        let now = Utc::now();
        setup(&c, now);
        activity(&c, "a", 12, "feedback", now - Duration::hours(2), 0.0);
        c.execute("INSERT INTO email_replies (contact_id, from_addr, kind) VALUES (12, 'info@bird.nl', 'reply')", []).unwrap();
        assert!(warm_targets(&c, now).is_empty());
    }
}
