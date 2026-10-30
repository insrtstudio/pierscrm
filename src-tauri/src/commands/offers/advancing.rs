//! Advancing: what has to be settled before (and just after) a confirmed gig.
//!
//! A fixed checklist with due dates relative to the gig day (J-21 .. J+3).
//! Only the state of an item is stored (`advancing`: done or not applicable);
//! the list itself and the due dates are computed, so every confirmed gig has
//! its checklist without any row being created. "payment" is also done when
//! the balance of the gig's offer is marked paid. The pseudo item "email"
//! records that the J-14 advancing email went out.

use chrono::NaiveDate;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use tauri::State;

use super::{parse_day, today};
use crate::AppState;

/// (key, days relative to the gig).
pub const ITEMS: &[(&str, i64)] = &[
    ("travel", -21),
    ("hotel", -21),
    ("set_time", -14),
    ("tech_rider", -14),
    ("promo", -14),
    ("guest_list", -7),
    ("night_contact", -2),
    ("payment", 3),
];

/// When the advancing email to the venue is due.
pub const EMAIL_OFFSET: i64 = -14;

pub fn migrate(conn: &Connection) {
    let _ = conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS advancing (
            event_id INTEGER NOT NULL REFERENCES events(id) ON DELETE CASCADE,
            item     TEXT NOT NULL,
            state    TEXT NOT NULL DEFAULT 'done',
            done_at  TEXT NOT NULL DEFAULT (datetime('now')),
            PRIMARY KEY (event_id, item)
        );",
    );
}

pub fn due_date(gig: NaiveDate, offset: i64) -> NaiveDate {
    gig + chrono::Duration::days(offset)
}

#[derive(Debug, Clone, Serialize)]
pub struct AdvItem {
    pub key: String,
    pub offset: i64,
    pub due: String,
    /// None (open), "done" or "na".
    pub state: Option<String>,
    pub done_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GigAdvancing {
    pub event_id: i64,
    pub title: String,
    pub date: String,
    pub venue: Option<String>,
    pub city: Option<String>,
    pub contact_id: Option<i64>,
    pub offer_id: Option<i64>,
    pub items: Vec<AdvItem>,
    pub email_due: String,
    pub email_sent_at: Option<String>,
}

fn states(conn: &Connection, event_id: i64) -> Vec<(String, String, String)> {
    conn.prepare("SELECT item, state, done_at FROM advancing WHERE event_id=?1")
        .and_then(|mut s| {
            s.query_map(params![event_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .unwrap_or_default()
}

/// The checklist of one gig (None when the event is unknown or has no ISO date).
pub(crate) fn for_event(conn: &Connection, event_id: i64) -> Option<GigAdvancing> {
    let (title, date, venue, city, contact_id): (String, String, Option<String>, Option<String>, Option<i64>) = conn
        .query_row(
            "SELECT title, date, venue, city, contact_id FROM events WHERE id=?1",
            params![event_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()
        .ok()
        .flatten()?;
    let gig = parse_day(&Some(date.clone()))?;
    let (offer_id, balance_paid): (Option<i64>, Option<String>) = conn
        .query_row(
            "SELECT id, balance_paid_at FROM offers WHERE event_id=?1 ORDER BY (status='accepted') DESC, id DESC LIMIT 1",
            params![event_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .ok()
        .flatten()
        .map(|(a, b)| (Some(a), b))
        .unwrap_or((None, None));
    let st = states(conn, event_id);
    let lookup = |k: &str| st.iter().find(|(i, _, _)| i == k).map(|(_, s, d)| (s.clone(), d.clone()));
    let fmt = |d: NaiveDate| d.format("%Y-%m-%d").to_string();
    let items = ITEMS
        .iter()
        .map(|(k, off)| {
            let mut s = lookup(k);
            if *k == "payment" && s.is_none() {
                s = balance_paid.clone().map(|d| ("done".to_string(), d));
            }
            AdvItem {
                key: k.to_string(),
                offset: *off,
                due: fmt(due_date(gig, *off)),
                state: s.as_ref().map(|x| x.0.clone()),
                done_at: s.map(|x| x.1),
            }
        })
        .collect();
    Some(GigAdvancing {
        event_id,
        title,
        date: fmt(gig),
        venue,
        city,
        contact_id,
        offer_id,
        items,
        email_due: fmt(due_date(gig, EMAIL_OFFSET)),
        email_sent_at: lookup("email").map(|x| x.1),
    })
}

/// Confirmed gigs between J-30 (still being advanced, or payment to collect)
/// and `until`, with their checklist.
pub(crate) fn list(conn: &Connection, today: NaiveDate, until: NaiveDate) -> Vec<GigAdvancing> {
    let from = (today - chrono::Duration::days(30)).format("%Y-%m-%d").to_string();
    // The checklist starts 21 days before the gig.
    let to = (until + chrono::Duration::days(21)).format("%Y-%m-%d").to_string();
    let ids: Vec<i64> = conn
        .prepare("SELECT id FROM events WHERE status='confirmed' AND date >= ?1 AND date <= ?2 ORDER BY date")
        .and_then(|mut s| {
            s.query_map(params![from, to], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<i64>>>()
        })
        .unwrap_or_default();
    ids.into_iter().filter_map(|id| for_event(conn, id)).collect()
}

pub(crate) fn set_state(conn: &Connection, event_id: i64, item: &str, state: Option<&str>) -> Result<(), String> {
    if item != "email" && !ITEMS.iter().any(|(k, _)| *k == item) {
        return Err(format!("unknown advancing item: {item}"));
    }
    match state {
        None => {
            conn.execute("DELETE FROM advancing WHERE event_id=?1 AND item=?2", params![event_id, item])
                .map_err(|e| e.to_string())?;
        }
        Some(s @ ("done" | "na")) => {
            conn.execute(
                "INSERT INTO advancing (event_id, item, state) VALUES (?1,?2,?3)
                 ON CONFLICT(event_id, item) DO UPDATE SET state=excluded.state, done_at=datetime('now')",
                params![event_id, item, s],
            )
            .map_err(|e| e.to_string())?;
        }
        Some(s) => return Err(format!("unknown advancing state: {s}")),
    }
    Ok(())
}

#[tauri::command(async)]
pub fn list_advancing(state: State<'_, AppState>, until: Option<String>) -> Result<Vec<GigAdvancing>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let t = today();
    let until = parse_day(&until).unwrap_or(t + chrono::Duration::days(14));
    Ok(list(&conn, t, until))
}

#[tauri::command(async)]
pub fn get_event_advancing(state: State<'_, AppState>, event_id: i64) -> Result<Option<GigAdvancing>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    Ok(for_event(&conn, event_id))
}

#[tauri::command(async)]
pub fn set_advancing_item(
    state: State<'_, AppState>,
    event_id: i64,
    item: String,
    value: Option<String>,
) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    set_state(&conn, event_id, &item, value.as_deref())
}

#[cfg(test)]
mod tests {
    use super::super::tests::{day, db, offer};
    use super::super::{money, save};
    use super::*;

    #[test]
    fn due_dates_relative_to_gig() {
        let g = day("2026-12-12");
        assert_eq!(due_date(g, -21), day("2026-11-21"));
        assert_eq!(due_date(g, -14), day("2026-11-28"));
        assert_eq!(due_date(g, -7), day("2026-12-05"));
        assert_eq!(due_date(g, -2), day("2026-12-10"));
        assert_eq!(due_date(g, 3), day("2026-12-15"));
    }

    #[test]
    fn checklist_of_a_confirmed_gig() {
        let c = db();
        let mut o = offer(&c);
        o.status = "accepted".into();
        let (oid, ev) = save(&c, &o, day("2026-11-01")).unwrap();
        let ev = ev.unwrap();
        let g = for_event(&c, ev).unwrap();
        assert_eq!(g.offer_id, Some(oid));
        assert_eq!(g.email_due, "2026-11-28");
        let due: Vec<(&str, &str)> = g.items.iter().map(|i| (i.key.as_str(), i.due.as_str())).collect();
        assert_eq!(due[0], ("travel", "2026-11-21"));
        assert_eq!(due[7], ("payment", "2026-12-15"));
        assert!(g.items.iter().all(|i| i.state.is_none()));

        set_state(&c, ev, "hotel", Some("na")).unwrap();
        set_state(&c, ev, "travel", Some("done")).unwrap();
        set_state(&c, ev, "travel", Some("done")).unwrap();
        set_state(&c, ev, "email", Some("done")).unwrap();
        assert!(set_state(&c, ev, "visa", Some("done")).is_err());
        assert!(set_state(&c, ev, "travel", Some("maybe")).is_err());
        money::set_paid(&c, oid, "balance", Some("2026-12-13".into())).unwrap();
        let g = for_event(&c, ev).unwrap();
        let st = |k: &str| g.items.iter().find(|i| i.key == k).unwrap().state.clone();
        assert_eq!(st("travel").as_deref(), Some("done"));
        assert_eq!(st("hotel").as_deref(), Some("na"));
        assert_eq!(st("payment").as_deref(), Some("done"), "balance paid");
        assert!(g.email_sent_at.is_some());
        set_state(&c, ev, "travel", None).unwrap();
        assert!(for_event(&c, ev).unwrap().items[0].state.is_none());

        // Listed while it is coming up, not once long past.
        assert_eq!(list(&c, day("2026-11-25"), day("2026-12-09")).len(), 1);
        assert_eq!(list(&c, day("2027-02-01"), day("2027-02-15")).len(), 0);
    }
}
