//! Confirmed gigs published on insrt.studio (artist page, "Dates" section).
//!
//! Each Agenda event has a "publier sur insrt" switch, off by default: nothing
//! leaves the CRM until the user turns it on. Once a gig was pushed, every later
//! change (edit, status change, deletion, switch turned off) is pushed again so
//! the site never keeps a stale or cancelled date public. The gig goes through
//! the `Insrt` trait (`push_gig`, upsert on `crm-event:<event_id>`).
//!
//! Table `insrt_gigs` (one row per event that was ever given the switch):
//! - publish      the switch
//! - country      ISO code (or name) shown on the site; the Agenda prefills it
//!                from the gig's inferred country
//! - ticket_url   ticket link shown on the site
//! - pushed       JSON of the last payload insrt accepted (what the site shows)
//! - pushed_at, remote_id, last_error
//!
//! No foreign key on events: the row outlives a deleted event so the date can
//! be withdrawn from the site, then it is removed.

use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::commands::residency::insrt::{self, gig_ref, GigPush, Insrt};
use crate::db::DbPool;
use crate::AppState;

/// One push at a time: the Agenda save and the switch save can both ask for a
/// sync of the same event; the second one then finds it already in sync.
static SYNC: Mutex<()> = Mutex::new(());

pub fn migrate(conn: &Connection) {
    let _ = conn.execute(
        "CREATE TABLE IF NOT EXISTS insrt_gigs (
            event_id    INTEGER PRIMARY KEY,
            publish     INTEGER NOT NULL DEFAULT 0,
            country     TEXT,
            ticket_url  TEXT,
            pushed      TEXT,
            pushed_at   TEXT,
            remote_id   TEXT,
            last_error  TEXT,
            updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
        )",
        [],
    );
}

#[derive(Debug, Clone, Default)]
struct Row {
    publish: bool,
    country: Option<String>,
    ticket_url: Option<String>,
    pushed: Option<String>,
}

fn row(conn: &Connection, event_id: i64) -> Option<Row> {
    conn.query_row(
        "SELECT publish, country, ticket_url, pushed FROM insrt_gigs WHERE event_id = ?1",
        params![event_id],
        |r| {
            Ok(Row {
                publish: r.get::<_, i64>(0)? != 0,
                country: r.get(1)?,
                ticket_url: r.get(2)?,
                pushed: r.get(3)?,
            })
        },
    )
    .optional()
    .ok()
    .flatten()
}

fn clean(s: Option<String>) -> Option<String> {
    s.map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

/// What insrt should show for this event right now. None: nothing to send
/// (never published, or no row). Errors are user-facing (French).
pub fn payload(conn: &Connection, event_id: i64) -> Result<Option<GigPush>, String> {
    let Some(r) = row(conn, event_id) else { return Ok(None) };
    let previous: Option<GigPush> = r.pushed.as_deref().and_then(|p| serde_json::from_str(p).ok());
    let event: Option<(String, Option<String>, String, Option<String>, Option<String>, String, Option<String>)> = conn
        .query_row(
            "SELECT e.title, e.venue, e.date, e.start_time, e.city, e.status, a.name
             FROM events e LEFT JOIN artists a ON a.id = e.artist_id WHERE e.id = ?1",
            params![event_id],
            |x| Ok((x.get(0)?, x.get(1)?, x.get(2)?, x.get(3)?, x.get(4)?, x.get(5)?, x.get(6)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let Some((title, venue, date, start, city, status, artist)) = event else {
        // Deleted in the Agenda: withdraw what the site shows, if anything.
        return Ok(previous.map(|p| GigPush { published: false, status: "cancelled".into(), ..p }));
    };
    if previous.is_none() && !r.publish {
        return Ok(None);
    }
    let artist = clean(artist)
        .or_else(|| crate::commands::email::sole_artist(conn))
        .ok_or_else(|| "Choisir l'artiste de la date avant de la publier sur insrt.".to_string())?;
    let day = date.trim().get(..10).unwrap_or(date.trim()).to_string();
    if chrono::NaiveDate::parse_from_str(&day, "%Y-%m-%d").is_err() {
        return Err(format!("Date invalide pour insrt ({}).", date));
    }
    let start_time = clean(start).and_then(|s| {
        let hm = s.get(..5).unwrap_or(&s).to_string();
        chrono::NaiveTime::parse_from_str(&hm, "%H:%M").ok().map(|_| hm)
    });
    let ticket_url = clean(r.ticket_url).filter(|u| u.starts_with("http://") || u.starts_with("https://"));
    Ok(Some(GigPush {
        external_ref: gig_ref(event_id),
        artist_slug: insrt::slug(&artist),
        date: day,
        start_time,
        venue: clean(venue).unwrap_or_else(|| title.trim().to_string()),
        city: clean(city),
        country: clean(r.country),
        ticket_url,
        status: if status == "cancelled" { "cancelled".into() } else { "confirmed".into() },
        // Only a confirmed gig with the switch on is public; a hold stays private.
        published: r.publish && status == "confirmed",
    }))
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// Nothing to send (never published, or insrt already shows this).
    Unchanged,
    Pushed { published: bool },
}

/// Push the event to insrt when what the site shows differs from the CRM.
/// Records the result (pushed payload or last error) on the row.
pub fn sync_one(conn: &Connection, client: &dyn Insrt, event_id: i64) -> Result<Outcome, String> {
    let _guard = SYNC.lock().unwrap_or_else(|p| p.into_inner());
    let gig = match payload(conn, event_id) {
        Ok(Some(g)) => g,
        Ok(None) => {
            // Deleted before it was ever published: forget it.
            let _ = conn.execute(
                "DELETE FROM insrt_gigs WHERE event_id = ?1 AND NOT EXISTS (SELECT 1 FROM events WHERE id = ?1)",
                params![event_id],
            );
            return Ok(Outcome::Unchanged);
        }
        Err(e) => {
            set_error(conn, event_id, &e);
            return Err(e);
        }
    };
    let json = serde_json::to_string(&gig).map_err(|e| e.to_string())?;
    let r = row(conn, event_id).unwrap_or_default();
    if r.pushed.as_deref() == Some(json.as_str()) {
        let _ = conn.execute("UPDATE insrt_gigs SET last_error = NULL WHERE event_id = ?1", params![event_id]);
        return Ok(Outcome::Unchanged);
    }
    match client.push_gig(&gig) {
        Ok(ack) => {
            let deleted: bool = !conn
                .query_row("SELECT EXISTS(SELECT 1 FROM events WHERE id = ?1)", params![event_id], |x| x.get(0))
                .unwrap_or(true);
            if deleted {
                let _ = conn.execute("DELETE FROM insrt_gigs WHERE event_id = ?1", params![event_id]);
            } else {
                let _ = conn.execute(
                    "UPDATE insrt_gigs SET pushed = ?2, pushed_at = datetime('now'), remote_id = ?3, last_error = NULL
                     WHERE event_id = ?1",
                    params![event_id, json, ack.id],
                );
            }
            Ok(Outcome::Pushed { published: gig.published })
        }
        Err(e) => {
            set_error(conn, event_id, &e);
            Err(e)
        }
    }
}

fn set_error(conn: &Connection, event_id: i64, e: &str) {
    let _ = conn.execute("UPDATE insrt_gigs SET last_error = ?2 WHERE event_id = ?1", params![event_id, e]);
}

/// Retry every gig not in sync (the engine calls this every 30 minutes).
/// Returns how many were pushed.
pub fn sync_all(conn: &Connection, client: &dyn Insrt) -> usize {
    let ids: Vec<i64> = conn
        .prepare("SELECT event_id FROM insrt_gigs WHERE publish = 1 OR pushed IS NOT NULL ORDER BY event_id")
        .and_then(|mut st| st.query_map([], |r| r.get(0)).map(|it| it.flatten().collect()))
        .unwrap_or_default();
    ids.into_iter()
        .filter(|id| matches!(sync_one(conn, client, *id), Ok(Outcome::Pushed { .. })))
        .count()
}

/// After an Agenda save or delete: push in the background (the HTTP call
/// must not hold the save). Without a token, the gig waits with an error.
pub fn after_event_change(pool: &DbPool, event_id: i64) {
    let Ok(conn) = pool.get() else { return };
    let tracked: bool = conn
        .query_row("SELECT EXISTS(SELECT 1 FROM insrt_gigs WHERE event_id = ?1)", params![event_id], |r| r.get(0))
        .unwrap_or(false);
    drop(conn);
    if !tracked {
        return;
    }
    let pool = pool.clone();
    std::thread::spawn(move || {
        let Ok(conn) = pool.get() else { return };
        run_with_client(&conn, event_id);
    });
}

fn run_with_client(conn: &Connection, event_id: i64) {
    match insrt::client(conn) {
        Some(client) => {
            let _ = sync_one(conn, client.as_ref(), event_id);
        }
        None => {
            if matches!(payload(conn, event_id), Ok(Some(_))) {
                set_error(conn, event_id, "Jeton insrt non configuré (Réglages, Intégrations).");
            }
        }
    }
}

// ---------------- Commands ----------------

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct GigState {
    pub event_id: i64,
    pub publish: bool,
    pub country: Option<String>,
    pub ticket_url: Option<String>,
    pub pushed_at: Option<String>,
    pub last_error: Option<String>,
    /// The site shows this date right now (last accepted push was public).
    pub live: bool,
    /// What the site shows matches the CRM.
    pub in_sync: bool,
}

pub fn state_of(conn: &Connection, event_id: i64) -> GigState {
    let found: Option<(bool, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT publish, country, ticket_url, pushed, pushed_at, last_error FROM insrt_gigs WHERE event_id = ?1",
            params![event_id],
            |r| Ok((r.get::<_, i64>(0)? != 0, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )
        .optional()
        .ok()
        .flatten();
    let Some((publish, country, ticket_url, pushed, pushed_at, last_error)) = found else {
        return GigState { event_id, in_sync: true, ..Default::default() };
    };
    let live = pushed
        .as_deref()
        .and_then(|p| serde_json::from_str::<GigPush>(p).ok())
        .map_or(false, |g| g.published);
    let in_sync = match payload(conn, event_id) {
        Ok(None) => true,
        Ok(Some(g)) => serde_json::to_string(&g).ok() == pushed,
        Err(_) => false,
    };
    GigState { event_id, publish, country, ticket_url, pushed_at, last_error, live, in_sync }
}

#[tauri::command(async)]
pub fn insrt_gig_get(state: State<'_, AppState>, event_id: i64) -> Result<GigState, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    Ok(state_of(&conn, event_id))
}

/// Save the switch, country and ticket link of an event, then push it now.
/// A failed push is not an error here: it is recorded and shown in the state.
#[tauri::command(async)]
pub fn insrt_gig_set(
    state: State<'_, AppState>,
    event_id: i64,
    publish: bool,
    country: Option<String>,
    ticket_url: Option<String>,
) -> Result<GigState, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    save_settings(&conn, event_id, publish, country, ticket_url)?;
    run_with_client(&conn, event_id);
    Ok(state_of(&conn, event_id))
}

pub fn save_settings(
    conn: &Connection,
    event_id: i64,
    publish: bool,
    country: Option<String>,
    ticket_url: Option<String>,
) -> Result<(), String> {
    let ticket_url = clean(ticket_url);
    if let Some(u) = &ticket_url {
        if !(u.starts_with("http://") || u.starts_with("https://")) {
            return Err("Le lien billetterie doit commencer par https://".into());
        }
    }
    conn.execute(
        "INSERT INTO insrt_gigs (event_id, publish, country, ticket_url) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(event_id) DO UPDATE SET publish = excluded.publish, country = excluded.country,
            ticket_url = excluded.ticket_url, updated_at = datetime('now')",
        params![event_id, publish as i64, clean(country).map(country_code), ticket_url],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// "fr" -> "FR"; a country name is kept as typed.
fn country_code(c: String) -> String {
    if c.len() == 2 && c.chars().all(|x| x.is_ascii_alphabetic()) {
        c.to_ascii_uppercase()
    } else {
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::residency::insrt::{ActivityPage, GigAck, PressKit, PressLink, Recipient};
    use crate::commands::residency::testdb;

    #[derive(Default)]
    struct Fake {
        sent: Mutex<Vec<GigPush>>,
        fail: Mutex<Option<String>>,
    }

    impl Insrt for Fake {
        fn press_kits(&self, _: &str) -> Result<Vec<PressKit>, String> {
            Ok(vec![])
        }
        fn press_link(&self, _: &str, _: i64, _: &Recipient, _: &str) -> Result<PressLink, String> {
            Err("unused".into())
        }
        fn activity(&self, _: &str, _: &str) -> Result<ActivityPage, String> {
            Ok(ActivityPage { events: vec![], next: None })
        }
        fn push_gig(&self, gig: &GigPush) -> Result<GigAck, String> {
            if let Some(e) = self.fail.lock().unwrap().clone() {
                return Err(e);
            }
            self.sent.lock().unwrap().push(gig.clone());
            Ok(GigAck { id: "g1".into(), external_ref: gig.external_ref.clone(), published: gig.published, created: true, updated_at: None })
        }
    }

    fn setup() -> Connection {
        let c = testdb::conn();
        c.execute_batch(
            "INSERT INTO artists (id, name) VALUES (1, 'Piers');
             INSERT INTO events (id, artist_id, title, venue, city, date, start_time, status)
               VALUES (5, 1, 'Piers @ Le Club', 'Le Club', 'Paris', '2027-03-14', '23:30:00', 'confirmed');",
        )
        .unwrap();
        c
    }

    #[test]
    fn nothing_leaves_without_the_switch() {
        let c = setup();
        let f = Fake::default();
        // No row: never sent.
        assert_eq!(sync_one(&c, &f, 5).unwrap(), Outcome::Unchanged);
        // Row with the switch off (country typed, not published): still never sent.
        save_settings(&c, 5, false, Some("fr".into()), None).unwrap();
        assert_eq!(sync_one(&c, &f, 5).unwrap(), Outcome::Unchanged);
        assert!(f.sent.lock().unwrap().is_empty());
        assert!(state_of(&c, 5).in_sync);
        assert!(!state_of(&c, 5).live);
    }

    #[test]
    fn publish_edit_cancel_delete() {
        let c = setup();
        let f = Fake::default();
        save_settings(&c, 5, true, Some("fr".into()), Some("https://tickets.example/x".into())).unwrap();
        assert!(!state_of(&c, 5).in_sync);
        assert_eq!(sync_one(&c, &f, 5).unwrap(), Outcome::Pushed { published: true });
        {
            let sent = f.sent.lock().unwrap();
            let g = &sent[0];
            assert_eq!(g.external_ref, "crm-event:5");
            assert_eq!(g.artist_slug, "piers");
            assert_eq!((g.date.as_str(), g.start_time.as_deref()), ("2027-03-14", Some("23:30")));
            assert_eq!((g.venue.as_str(), g.city.as_deref(), g.country.as_deref()), ("Le Club", Some("Paris"), Some("FR")));
            assert_eq!(g.ticket_url.as_deref(), Some("https://tickets.example/x"));
            assert_eq!((g.status.as_str(), g.published), ("confirmed", true));
        }
        let st = state_of(&c, 5);
        assert!(st.live && st.in_sync && st.pushed_at.is_some());
        // Same data: no second call.
        assert_eq!(sync_one(&c, &f, 5).unwrap(), Outcome::Unchanged);
        assert_eq!(f.sent.lock().unwrap().len(), 1);

        // Edited in the Agenda: pushed again.
        c.execute_batch("UPDATE events SET venue = 'Le Club 2' WHERE id = 5").unwrap();
        assert!(!state_of(&c, 5).in_sync);
        sync_one(&c, &f, 5).unwrap();
        assert_eq!(f.sent.lock().unwrap()[1].venue, "Le Club 2");

        // Back on hold: stays on insrt but private.
        c.execute_batch("UPDATE events SET status = 'hold' WHERE id = 5").unwrap();
        assert_eq!(sync_one(&c, &f, 5).unwrap(), Outcome::Pushed { published: false });
        // Cancelled.
        c.execute_batch("UPDATE events SET status = 'cancelled' WHERE id = 5").unwrap();
        sync_one(&c, &f, 5).unwrap();
        let last = f.sent.lock().unwrap().last().cloned().unwrap();
        assert_eq!((last.status.as_str(), last.published), ("cancelled", false));
        assert!(!state_of(&c, 5).live);

        // Deleted: withdrawn, then the row goes.
        c.execute_batch("UPDATE events SET status = 'confirmed' WHERE id = 5").unwrap();
        sync_one(&c, &f, 5).unwrap();
        c.execute_batch("DELETE FROM events WHERE id = 5").unwrap();
        assert_eq!(sync_one(&c, &f, 5).unwrap(), Outcome::Pushed { published: false });
        let last = f.sent.lock().unwrap().last().cloned().unwrap();
        assert_eq!((last.external_ref.as_str(), last.status.as_str(), last.published), ("crm-event:5", "cancelled", false));
        assert!(row(&c, 5).is_none());
    }

    #[test]
    fn switch_off_unpublishes_and_failures_are_kept() {
        let c = setup();
        let f = Fake::default();
        save_settings(&c, 5, true, None, None).unwrap();
        sync_one(&c, &f, 5).unwrap();
        save_settings(&c, 5, false, None, None).unwrap();
        *f.fail.lock().unwrap() = Some("insrt: HTTP 500".into());
        assert!(sync_one(&c, &f, 5).is_err());
        let st = state_of(&c, 5);
        assert_eq!(st.last_error.as_deref(), Some("insrt: HTTP 500"));
        assert!(st.live && !st.in_sync);
        // The engine retry clears it.
        *f.fail.lock().unwrap() = None;
        assert_eq!(sync_all(&c, &f), 1);
        let st = state_of(&c, 5);
        assert!(!st.live && st.in_sync && st.last_error.is_none());
        assert!(!f.sent.lock().unwrap().last().unwrap().published);
    }

    #[test]
    fn validation() {
        let c = setup();
        assert!(save_settings(&c, 5, true, None, Some("tickets.example".into())).is_err());
        // Without an artist on the event (and several artists), it cannot be published.
        c.execute_batch(
            "INSERT INTO artists (id, name) VALUES (2, 'Other');
             INSERT INTO events (id, title, date, status) VALUES (6, 'Sans artiste', '2027-04-01', 'confirmed');",
        )
        .unwrap();
        save_settings(&c, 6, true, None, None).unwrap();
        let f = Fake::default();
        assert!(sync_one(&c, &f, 6).is_err());
        assert!(state_of(&c, 6).last_error.unwrap().contains("artiste"));
        // The venue falls back on the title, the time is dropped when invalid.
        c.execute_batch("UPDATE events SET artist_id = 1, start_time = 'soir' WHERE id = 6").unwrap();
        sync_one(&c, &f, 6).unwrap();
        let g = f.sent.lock().unwrap()[0].clone();
        assert_eq!((g.venue.as_str(), g.start_time), ("Sans artiste", None));
    }
}
