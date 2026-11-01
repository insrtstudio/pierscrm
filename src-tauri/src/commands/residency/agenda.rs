//! Agenda › Actions as a week, Monday to Sunday: for each day, what is left
//! to do (the residency actions, the autopilot sends planned that day, the
//! replies put aside until then) and what was done (emails sent, grouped per
//! campaign, replies sent, items dismissed, campaigns created, gig follow-ups,
//! offers and payments). Everything is read from the existing history tables;
//! the other modules' to-dos (tasks, follow-ups, gigs...) are placed by the
//! page itself with the same rule as `place`.

use chrono::{Duration, NaiveDate};
use rusqlite::{params, Connection};
use serde::Serialize;
use tauri::State;

use super::commands::{actions, ActionItem};
use crate::AppState;

/// One line of what was done, already grouped ("12 premiers emails" is one).
#[derive(Serialize, Debug, Clone)]
pub struct DoneItem {
    pub key: String,
    /// first_emails | followups | emails | reply | auto_reply | dismissed |
    /// handled | expand_later | campaign | thanks | rebook | routing |
    /// offer_sent | offer_accepted | paid
    pub kind: String,
    pub day: String,
    pub count: i64,
    pub campaign_id: Option<i64>,
    pub campaign: Option<String>,
    pub contact_id: Option<i64>,
    /// Contact, event or venue the line is about.
    pub name: Option<String>,
    /// A few recipients of a grouped line, for the detail.
    pub names: Vec<String>,
    /// Latest timestamp of the line (UTC), when it has one.
    pub at: Option<String>,
    pub event_id: Option<i64>,
}

impl DoneItem {
    fn new(key: String, kind: &str, day: String) -> Self {
        DoneItem {
            key,
            kind: kind.into(),
            day,
            count: 1,
            campaign_id: None,
            campaign: None,
            contact_id: None,
            name: None,
            names: Vec::new(),
            at: None,
            event_id: None,
        }
    }
}

#[derive(Serialize, Debug)]
pub struct AgendaDay {
    pub day: String,
    pub todo: Vec<ActionItem>,
    pub done: Vec<DoneItem>,
}

#[derive(Serialize, Debug)]
pub struct AgendaWeek {
    /// Monday of the week, YYYY-MM-DD.
    pub start: String,
    pub today: String,
    pub days: Vec<AgendaDay>,
    /// Contacts with a residency reply to handle, any day: the page skips its
    /// generic "reply" line for them.
    pub reply_contacts: Vec<i64>,
}

/// Monday of the week holding `d`.
pub fn monday(d: NaiveDate) -> NaiveDate {
    use chrono::Datelike;
    d - Duration::days(d.weekday().num_days_from_monday() as i64)
}

/// Where a to-do due on `day` shows in the week `start..=end`: its own day,
/// or today when it is late (and today is in the week). None: another week.
/// Returns the day and whether it is overdue.
pub fn place(day: &str, today: &str, start: &str, end: &str) -> Option<(String, bool)> {
    if day < today {
        return (today >= start && today <= end).then(|| (today.to_string(), true));
    }
    (day >= start && day <= end).then(|| (day.to_string(), false))
}

/// Local day of a stored value: UTC timestamps are converted, plain dates kept.
fn local_day(col: &str) -> String {
    format!("(CASE WHEN length({c}) > 10 THEN date({c}, 'localtime') ELSE substr({c}, 1, 10) END)", c = col)
}

/// The week starting on `start` (a Monday), seen on `today`.
pub fn week(conn: &Connection, start: NaiveDate, today: NaiveDate) -> Result<AgendaWeek, String> {
    let start = monday(start);
    let end = start + Duration::days(6);
    let (s, e, t) = (start.to_string(), end.to_string(), today.to_string());
    let mut days: Vec<AgendaDay> = (0..7)
        .map(|i| AgendaDay { day: (start + Duration::days(i)).to_string(), todo: Vec::new(), done: Vec::new() })
        .collect();
    let idx = |d: &str| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok().map(|x| (x - start).num_days()).filter(|n| (0..7).contains(n)).map(|n| n as usize);

    // ---- To do ----
    let all = actions(conn)?;
    let reply_contacts: Vec<i64> = {
        let mut v: Vec<i64> = all
            .iter()
            .filter(|a| matches!(a.kind.as_str(), "reply" | "unread" | "later" | "wrong"))
            .filter_map(|a| a.contact_id)
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    };
    let mut todo = all;
    todo.extend(scheduled(conn, &t)?);
    todo.extend(snoozed(conn)?);
    for mut a in todo {
        let day = a.day.get(..10).unwrap_or(&t).to_string();
        let Some((d, overdue)) = place(&day, &t, &s, &e) else { continue };
        a.overdue = overdue;
        if let Some(i) = idx(&d) {
            days[i].todo.push(a);
        }
    }

    // ---- Done ----
    for d in done(conn, &s, &e)? {
        if let Some(i) = idx(&d.day) {
            days[i].done.push(d);
        }
    }
    Ok(AgendaWeek { start: s, today: t, days, reply_contacts })
}

/// Autopilot sends planned after today, per campaign and day ("8 envois
/// prévus"). Today's are the "plan" action already.
fn scheduled(conn: &Connection, today: &str) -> Result<Vec<ActionItem>, String> {
    let ids = super::active_ids(conn);
    let day = local_day("m.scheduled_for");
    let mut st = conn
        .prepare(&format!(
            "SELECT m.campaign_id, c.name, {day} AS d, COUNT(*), SUM(m.step > 1)
             FROM campaign_messages m JOIN campaigns c ON c.id = m.campaign_id
             JOIN residency_campaigns r ON r.campaign_id = m.campaign_id
             WHERE m.status = 'scheduled' AND m.scheduled_for IS NOT NULL AND r.autopilot = 1
               AND r.paused_reason IS NULL AND {day} > ?1
             GROUP BY m.campaign_id, d ORDER BY d, m.campaign_id"
        ))
        .map_err(|e| e.to_string())?;
    let rows: Vec<(i64, String, String, i64, i64)> = st
        .query_map(params![today], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))
        .map_err(|e| e.to_string())?
        .flatten()
        .collect();
    Ok(rows
        .into_iter()
        .filter(|(id, ..)| ids.contains(id))
        .map(|(id, name, d, n, fu)| {
            let mut a = ActionItem::new(format!("sc{}-{}", id, d), "scheduled", d, name.clone());
            a.campaign_id = Some(id);
            a.campaign = Some(name);
            a.count = Some(n);
            a.followups = Some(fu);
            a
        })
        .collect())
}

/// Replies put aside ("Plus tard" in Outbox › Réponses): back on their day.
fn snoozed(conn: &Connection) -> Result<Vec<ActionItem>, String> {
    let day = local_day("d.snooze_until");
    let mut st = conn
        .prepare(&format!(
            "SELECT d.id, d.contact_id, d.campaign_id, COALESCE(k.name, d.to_addr), {day}, d.subject
             FROM reply_desk d LEFT JOIN contacts k ON k.id = d.contact_id
             WHERE d.state = 'snoozed' AND d.snooze_until IS NOT NULL"
        ))
        .map_err(|e| e.to_string())?;
    let rows: Vec<(i64, Option<i64>, Option<i64>, String, String, String)> = st
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))
        .map_err(|e| e.to_string())?
        .flatten()
        .collect();
    Ok(rows
        .into_iter()
        .map(|(id, cid, camp, name, d, subject)| {
            let mut a = ActionItem::new(format!("sn{}", id), "snoozed", d, name);
            a.contact_id = cid;
            a.campaign_id = camp;
            a.desk_id = Some(id);
            a.suggested_subject = (!subject.is_empty()).then_some(subject);
            a
        })
        .collect())
}

/// Everything done between `start` and `end` (local days, inclusive).
pub fn done(conn: &Connection, start: &str, end: &str) -> Result<Vec<DoneItem>, String> {
    let mut out = Vec::new();
    let err = |e: rusqlite::Error| e.to_string();

    // Emails sent. A residency email is a first email or a follow-up of its
    // campaign; an answer (Outbox › Réponses, autonomy, or a "Re:" written
    // from the app) is one line per contact; the rest is grouped per campaign.
    let day = local_day("e.sent_at");
    let mut st = conn
        .prepare(&format!(
            "SELECT e.id, {day}, e.sent_at, e.contact_id, COALESCE(k.name, e.to_addr), e.campaign_id, c.name,
                    (SELECT MIN(m.step) FROM campaign_messages m WHERE m.sent_email_id = e.id),
                    EXISTS(SELECT 1 FROM autosends a WHERE a.sent_email_id = e.id),
                    EXISTS(SELECT 1 FROM reply_desk d WHERE d.sent_email_id = e.id),
                    lower(e.subject) LIKE 're:%'
             FROM emails e LEFT JOIN contacts k ON k.id = e.contact_id LEFT JOIN campaigns c ON c.id = e.campaign_id
             WHERE e.status = 'sent' AND e.sent_at >= date(?1, '-1 day') AND e.sent_at < date(?2, '+2 day')
               AND {day} BETWEEN ?1 AND ?2
             ORDER BY e.sent_at"
        ))
        .map_err(err)?;
    type E = (i64, String, String, Option<i64>, String, Option<i64>, Option<String>, Option<i64>, bool, bool, bool);
    let rows: Vec<E> = st
        .query_map(params![start, end], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?, r.get(9)?, r.get(10)?))
        })
        .map_err(err)?
        .flatten()
        .collect();
    let mut groups: Vec<DoneItem> = Vec::new();
    for (id, d, at, cid, name, camp, camp_name, step, auto, desk, re) in rows {
        let kind = match step {
            Some(1) => "first_emails",
            Some(_) => "followups",
            None if auto => "auto_reply",
            None if desk || re => "reply",
            None => "emails",
        };
        if matches!(kind, "reply" | "auto_reply") {
            let mut x = DoneItem::new(format!("em{}", id), kind, d);
            x.contact_id = cid;
            x.name = Some(name);
            x.campaign_id = camp;
            x.campaign = camp_name;
            x.at = Some(at);
            out.push(x);
            continue;
        }
        let key = format!("{}{}-{}", kind, camp.unwrap_or(0), d);
        match groups.iter_mut().find(|g| g.key == key) {
            Some(g) => {
                g.count += 1;
                if g.names.len() < 3 && !g.names.contains(&name) {
                    g.names.push(name);
                }
                g.at = Some(at);
            }
            None => {
                let mut g = DoneItem::new(key, kind, d);
                g.campaign_id = camp;
                g.campaign = camp_name;
                g.names.push(name);
                g.at = Some(at);
                groups.push(g);
            }
        }
    }
    out.extend(groups);

    // Replies dismissed in Outbox › Réponses (ignored), and residency actions
    // marked done without an email that day (the email already says it).
    let day = local_day("d.updated_at");
    let mut st = conn
        .prepare(&format!(
            "SELECT d.id, {day}, d.updated_at, d.contact_id, COALESCE(k.name, d.to_addr), d.campaign_id
             FROM reply_desk d LEFT JOIN contacts k ON k.id = d.contact_id
             WHERE d.state = 'ignored' AND {day} BETWEEN ?1 AND ?2"
        ))
        .map_err(err)?;
    let rows: Vec<(i64, String, String, Option<i64>, String, Option<i64>)> = st
        .query_map(params![start, end], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))
        .map_err(err)?
        .flatten()
        .collect();
    for (id, d, at, cid, name, camp) in rows {
        let mut x = DoneItem::new(format!("dk{}", id), "dismissed", d);
        x.contact_id = cid;
        x.name = Some(name);
        x.campaign_id = camp;
        x.at = Some(at);
        out.push(x);
    }
    let day = local_day("i.handled_at");
    let mut st = conn
        .prepare(&format!(
            "SELECT i.id, {day} AS d, i.handled_at, i.contact_id, COALESCE(k.name, r.from_addr), i.campaign_id, c.name
             FROM reply_insights i JOIN email_replies r ON r.id = i.email_reply_id
             LEFT JOIN contacts k ON k.id = i.contact_id LEFT JOIN campaigns c ON c.id = i.campaign_id
             WHERE i.handled = 1 AND i.handled_at IS NOT NULL AND {day} BETWEEN ?1 AND ?2
               AND NOT EXISTS (SELECT 1 FROM reply_desk x WHERE x.email_reply_id = i.email_reply_id AND x.state IN ('sent','ignored'))
               AND NOT EXISTS (SELECT 1 FROM emails e WHERE e.contact_id = i.contact_id AND e.status = 'sent'
                               AND date(e.sent_at, 'localtime') = {day})"
        ))
        .map_err(err)?;
    let rows: Vec<(i64, String, String, Option<i64>, String, Option<i64>, Option<String>)> = st
        .query_map(params![start, end], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)))
        .map_err(err)?
        .flatten()
        .collect();
    for (id, d, at, cid, name, camp, camp_name) in rows {
        let mut x = DoneItem::new(format!("ih{}", id), "handled", d);
        x.contact_id = cid;
        x.name = Some(name);
        x.campaign_id = camp;
        x.campaign = camp_name;
        x.at = Some(at);
        out.push(x);
    }

    // Campaigns created, and next-zone proposals put off.
    let day = local_day("c.created_at");
    let mut st = conn
        .prepare(&format!("SELECT c.id, {day}, c.created_at, c.name FROM campaigns c WHERE {day} BETWEEN ?1 AND ?2"))
        .map_err(err)?;
    let rows: Vec<(i64, String, String, String)> = st
        .query_map(params![start, end], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .map_err(err)?
        .flatten()
        .collect();
    for (id, d, at, name) in rows {
        let mut x = DoneItem::new(format!("cc{}", id), "campaign", d);
        x.campaign_id = Some(id);
        x.campaign = Some(name);
        x.at = Some(at);
        out.push(x);
    }
    let day = local_day("x.created_at");
    let mut st = conn
        .prepare(&format!(
            "SELECT x.id, {day}, x.created_at, x.source_campaign_id, c.name FROM residency_expansions x
             JOIN campaigns c ON c.id = x.source_campaign_id WHERE x.kind = 'dismissed' AND {day} BETWEEN ?1 AND ?2"
        ))
        .map_err(err)?;
    let rows: Vec<(i64, String, String, i64, String)> = st
        .query_map(params![start, end], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))
        .map_err(err)?
        .flatten()
        .collect();
    for (id, d, at, camp, name) in rows {
        let mut x = DoneItem::new(format!("xl{}", id), "expand_later", d);
        x.campaign_id = Some(camp);
        x.campaign = Some(name);
        x.at = Some(at);
        out.push(x);
    }

    // After a gig: thank-you, rebook reminder, routing (done or dismissed).
    let day = local_day("f.done_at");
    let mut st = conn
        .prepare(&format!(
            "SELECT f.event_id, f.kind, {day}, f.done_at, COALESCE(NULLIF(ev.venue, ''), ev.title), ev.contact_id
             FROM gig_followups f JOIN events ev ON ev.id = f.event_id
             WHERE f.kind IN ('thanks','rebook','routing') AND {day} BETWEEN ?1 AND ?2"
        ))
        .map_err(err)?;
    let rows: Vec<(i64, String, String, String, String, Option<i64>)> = st
        .query_map(params![start, end], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))
        .map_err(err)?
        .flatten()
        .collect();
    for (ev, kind, d, at, name, cid) in rows {
        let mut x = DoneItem::new(format!("gf{}{}", kind, ev), &kind, d);
        x.event_id = Some(ev);
        x.contact_id = cid;
        x.name = Some(name);
        x.at = Some(at);
        out.push(x);
    }

    // Offers: sent, accepted, deposit / balance received.
    for (col, kind) in [("sent_at", "offer_sent"), ("accepted_at", "offer_accepted"), ("deposit_paid_at", "paid"), ("balance_paid_at", "paid")] {
        let day = local_day(&format!("o.{}", col));
        let sql = format!(
            "SELECT o.id, {day}, o.{col}, COALESCE(NULLIF(o.venue, ''), NULLIF(o.city, ''), k.name, ''), o.contact_id, o.event_id
             FROM offers o LEFT JOIN contacts k ON k.id = o.contact_id
             WHERE o.{col} IS NOT NULL AND o.{col} <> '' AND {day} BETWEEN ?1 AND ?2"
        );
        let mut st = conn.prepare(&sql).map_err(err)?;
        let rows: Vec<(i64, String, String, String, Option<i64>, Option<i64>)> = st
            .query_map(params![start, end], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))
            .map_err(err)?
            .flatten()
            .collect();
        for (id, d, at, name, cid, ev) in rows {
            let mut x = DoneItem::new(format!("of{}{}", col, id), kind, d);
            x.name = Some(name);
            x.contact_id = cid;
            x.event_id = ev;
            x.at = (at.len() > 10).then_some(at);
            // Which part was paid, in the campaign slot (no campaign here).
            if kind == "paid" {
                x.campaign = Some(col.trim_end_matches("_paid_at").to_string());
            }
            out.push(x);
        }
    }

    out.sort_by(|a, b| a.day.cmp(&b.day).then(b.at.cmp(&a.at)));
    Ok(out)
}

#[tauri::command(async)]
pub fn agenda_week(state: State<'_, AppState>, start: Option<String>) -> Result<AgendaWeek, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let today = chrono::Local::now().date_naive();
    let start = start
        .and_then(|s| NaiveDate::parse_from_str(s.get(..10).unwrap_or(&s), "%Y-%m-%d").ok())
        .unwrap_or(today);
    week(&conn, start, today)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::residency::engine::tests::setup;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn place_puts_late_items_on_today() {
        let (s, e) = ("2026-10-26", "2026-11-01");
        assert_eq!(place("2026-10-28", "2026-10-28", s, e), Some(("2026-10-28".into(), false)));
        assert_eq!(place("2026-10-31", "2026-10-28", s, e), Some(("2026-10-31".into(), false)));
        // Late: on today, flagged, even when due in an earlier week.
        assert_eq!(place("2026-10-20", "2026-10-28", s, e), Some(("2026-10-28".into(), true)));
        // Next week: late items stay on today's week, future ones show.
        assert_eq!(place("2026-10-20", "2026-10-28", "2026-11-02", "2026-11-08"), None);
        assert_eq!(place("2026-11-03", "2026-10-28", "2026-11-02", "2026-11-08"), Some(("2026-11-03".into(), false)));
        // Last week: nothing to do any more there.
        assert_eq!(place("2026-10-21", "2026-10-28", "2026-10-19", "2026-10-25"), None);
        assert_eq!(monday(d("2026-11-01")), d("2026-10-26"));
        assert_eq!(monday(d("2026-10-26")), d("2026-10-26"));
    }

    /// Real today (the residency actions are "today" by construction), fixed
    /// offsets from its Monday for everything else.
    #[test]
    fn week_places_todo_and_done_on_their_days() {
        let c = setup();
        let today = chrono::Local::now().date_naive();
        let mon = monday(today);
        let next = mon + Duration::days(7);
        let prev = mon - Duration::days(7);
        let at = |day: NaiveDate| format!("{} 12:00:00", day);
        // A reply received 9 days ago, unanswered: late, on today.
        c.execute_batch(&format!(
            "INSERT INTO email_replies (id, contact_id, from_addr, subject, message_id, kind, received_at)
               VALUES (1, 10, 'booking@cluba.fr', 'Résidence', '<a>', 'reply', '{old}');
             INSERT INTO reply_insights (email_reply_id, contact_id, campaign_id, sentiment, intent, summary)
               VALUES (1, 10, 1, 'positive', 'question', 'Demande le cachet');
             INSERT INTO email_replies (id, contact_id, from_addr, subject, message_id, kind, received_at)
               VALUES (2, 11, 'booking@clubb.co.uk', 'Résidence', '<b>', 'reply', '{old}');
             INSERT INTO reply_insights (email_reply_id, contact_id, campaign_id, sentiment, intent, follow_up_on)
               VALUES (2, 11, 1, 'neutral', 'later', '{later}');",
            old = at(today - Duration::days(9)),
            later = next + Duration::days(1),
        ))
        .unwrap();
        // A follow-up the autopilot sends next Wednesday.
        c.execute(
            "INSERT INTO campaign_messages (campaign_id, contact_id, step, subject, body, status, scheduled_for)
             VALUES (1, 10, 2, 'Re: Résidence', 'Relance', 'scheduled', ?1)",
            params![format!("{} 09:00:00", next + Duration::days(2))],
        )
        .unwrap();
        // Last week, Tuesday: two first emails and a hand-written answer.
        let tue = prev + Duration::days(1);
        c.execute_batch(&format!(
            "INSERT INTO emails (id, contact_id, campaign_id, to_addr, subject, body, status, sent_at) VALUES
                (100, 10, 1, 'booking@cluba.fr', 'Résidence', 'x', 'sent', '{t}'),
                (101, 11, 1, 'booking@clubb.co.uk', 'Résidence', 'x', 'sent', '{t}'),
                (102, 11, NULL, 'booking@clubb.co.uk', 'Re: dates', 'x', 'sent', '{t}'),
                (103, 11, NULL, 'booking@clubb.co.uk', 'Raté', 'x', 'failed', '{t}');
             UPDATE campaign_messages SET status = 'sent', sent_email_id = 100 WHERE contact_id = 10 AND step = 1;
             UPDATE campaign_messages SET status = 'sent', sent_email_id = 101 WHERE contact_id = 11 AND step = 1;
             UPDATE campaigns SET created_at = '{t}' WHERE id = 1;",
            t = at(tue)
        ))
        .unwrap();

        // This week: the late reply sits on today, flagged.
        let w = week(&c, today, today).unwrap();
        assert_eq!(w.start, mon.to_string());
        assert_eq!(w.days.len(), 7);
        let day_of = |w: &AgendaWeek, kind: &str| {
            w.days.iter().find(|x| x.todo.iter().any(|a| a.kind == kind)).map(|x| x.day.clone())
        };
        assert_eq!(day_of(&w, "reply"), Some(today.to_string()));
        let reply = w.days.iter().flat_map(|x| &x.todo).find(|a| a.kind == "reply").unwrap();
        assert!(reply.overdue);
        assert_eq!(w.reply_contacts, vec![10, 11]);
        assert!(w.days.iter().all(|x| x.done.is_empty()));

        // Next week: no late items, the "later" follow-up on Tuesday, the
        // planned send on Wednesday.
        let w = week(&c, next + Duration::days(3), today).unwrap();
        assert_eq!(w.start, next.to_string());
        assert_eq!(day_of(&w, "later"), Some((next + Duration::days(1)).to_string()));
        assert_eq!(day_of(&w, "reply"), None);
        let sc = w.days.iter().flat_map(|x| &x.todo).find(|a| a.kind == "scheduled").unwrap();
        assert_eq!(sc.day, (next + Duration::days(2)).to_string());
        assert_eq!((sc.count, sc.followups, sc.overdue), (Some(1), Some(1), false));

        // Last week: what was done, grouped, on Tuesday; the failed email is not there.
        let w = week(&c, prev, today).unwrap();
        assert!(w.days.iter().all(|x| x.todo.is_empty()));
        let tue_done = &w.days[1].done;
        assert_eq!(w.days[1].day, tue.to_string());
        let firsts = tue_done.iter().find(|x| x.kind == "first_emails").unwrap();
        assert_eq!((firsts.count, firsts.campaign.as_deref()), (2, Some("R")));
        assert_eq!(firsts.names, vec!["Club A", "Club B"]);
        assert_eq!(tue_done.iter().filter(|x| x.kind == "reply").count(), 1);
        assert!(tue_done.iter().any(|x| x.kind == "campaign"));
        assert!(!tue_done.iter().any(|x| x.kind == "emails"), "{:?}", tue_done);
        assert_eq!(w.days.iter().map(|x| x.done.len()).sum::<usize>(), 3);

        // Marked done today, no email: one "handled" line today.
        c.execute("UPDATE reply_insights SET handled = 1, handled_at = ?1 WHERE email_reply_id = 2", params![at(today)]).unwrap();
        let w = week(&c, today, today).unwrap();
        let t = w.days.iter().find(|x| x.day == today.to_string()).unwrap();
        assert_eq!(t.done.iter().filter(|x| x.kind == "handled").count(), 1);
        assert_eq!(w.reply_contacts, vec![10]);
    }
}
