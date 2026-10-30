//! From the offer to the confirmed date.
//!
//! - `offers`: one booking offer for a contact (date, venue, set, fee and deal,
//!   deposit, travel, hotel, meals, guest list, tech, answer deadline) and its
//!   life cycle draft -> sent -> accepted / declined / expired.
//! - The offer drives the Agenda: once sent with a date it holds the date
//!   (event `hold`), once accepted the event is `confirmed` with the fee. The
//!   event stays the only owner of the budget revenue line
//!   (`data::sync_event_budget`), so an offer never adds money on its own and
//!   nothing is counted twice. Pipeline moves go through `status.rs`.
//! - `money`: deposit / balance due and paid dates, invoice numbers.
//! - `advancing`: the per-gig checklist (J-21 .. J+3) and the J-14 email.
//! - `docs` + `pdf`: contract, rider and invoice PDFs, stored in the app data
//!   dir and attached to an email draft the user reviews (never sent alone).
//!
//! Schema is additive and idempotent (`migrate`, called from db.rs).

pub mod advancing;
pub mod docs;
pub mod money;
pub mod pdf;

use chrono::{Local, NaiveDate, NaiveTime};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::commands::{events, status};
use crate::models::Event;
use crate::AppState;

pub const OFFER_STATUSES: &[&str] = &["draft", "sent", "accepted", "declined", "expired"];
pub const FEE_TYPES: &[&str] = &["flat", "versus", "door"];

pub fn migrate(conn: &Connection) {
    let _ = conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS offers (
            id                INTEGER PRIMARY KEY AUTOINCREMENT,
            contact_id        INTEGER REFERENCES contacts(id) ON DELETE SET NULL,
            artist_id         INTEGER REFERENCES artists(id) ON DELETE SET NULL,
            event_id          INTEGER REFERENCES events(id) ON DELETE SET NULL,
            status            TEXT NOT NULL DEFAULT 'draft',
            date              TEXT,
            venue             TEXT,
            city              TEXT,
            set_time          TEXT,
            set_length        INTEGER,
            fee_amount        REAL,
            currency          TEXT NOT NULL DEFAULT 'EUR',
            fee_type          TEXT NOT NULL DEFAULT 'flat',
            deal_terms        TEXT,
            deposit_pct       REAL NOT NULL DEFAULT 50,
            travel_by         TEXT,
            travel_from       TEXT,
            travel_to         TEXT,
            hotel             TEXT,
            meals             TEXT,
            guest_list        INTEGER,
            tech_requirements TEXT,
            answer_by         TEXT,
            language          TEXT NOT NULL DEFAULT 'fr',
            notes             TEXT,
            sent_at           TEXT,
            accepted_at       TEXT,
            deposit_due       TEXT,
            deposit_paid_at   TEXT,
            balance_due       TEXT,
            balance_paid_at   TEXT,
            created_at        TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at        TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_offers_contact ON offers(contact_id);
        CREATE INDEX IF NOT EXISTS idx_offers_event ON offers(event_id);
        CREATE INDEX IF NOT EXISTS idx_offers_status ON offers(status);",
    );
    money::migrate(conn);
    advancing::migrate(conn);
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Offer {
    #[serde(default)]
    pub id: Option<i64>,
    #[serde(default)]
    pub contact_id: Option<i64>,
    #[serde(default)]
    pub artist_id: Option<i64>,
    #[serde(default)]
    pub event_id: Option<i64>,
    #[serde(default = "draft")]
    pub status: String,
    #[serde(default)]
    pub date: Option<String>,
    #[serde(default)]
    pub venue: Option<String>,
    #[serde(default)]
    pub city: Option<String>,
    /// "23:30"
    #[serde(default)]
    pub set_time: Option<String>,
    /// Minutes.
    #[serde(default)]
    pub set_length: Option<i64>,
    /// Flat fee, or the guarantee of a versus / door deal.
    #[serde(default)]
    pub fee_amount: Option<f64>,
    #[serde(default = "eur")]
    pub currency: String,
    /// flat | versus | door
    #[serde(default = "flat")]
    pub fee_type: String,
    /// "versus 70% of net door", "100% door after 300 € costs"...
    #[serde(default)]
    pub deal_terms: Option<String>,
    #[serde(default = "fifty")]
    pub deposit_pct: f64,
    /// promoter | artist | split
    #[serde(default)]
    pub travel_by: Option<String>,
    #[serde(default)]
    pub travel_from: Option<String>,
    #[serde(default)]
    pub travel_to: Option<String>,
    #[serde(default)]
    pub hotel: Option<String>,
    #[serde(default)]
    pub meals: Option<String>,
    #[serde(default)]
    pub guest_list: Option<i64>,
    #[serde(default)]
    pub tech_requirements: Option<String>,
    #[serde(default)]
    pub answer_by: Option<String>,
    /// Language of the documents: fr | en.
    #[serde(default = "fr")]
    pub language: String,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub sent_at: Option<String>,
    #[serde(default)]
    pub accepted_at: Option<String>,
    #[serde(default)]
    pub deposit_due: Option<String>,
    #[serde(default)]
    pub deposit_paid_at: Option<String>,
    #[serde(default)]
    pub balance_due: Option<String>,
    #[serde(default)]
    pub balance_paid_at: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    // Read-only, joined for the lists.
    #[serde(default)]
    pub contact_name: Option<String>,
    #[serde(default)]
    pub contact_email: Option<String>,
    #[serde(default)]
    pub artist_name: Option<String>,
}

fn draft() -> String {
    "draft".into()
}
fn eur() -> String {
    "EUR".into()
}
fn flat() -> String {
    "flat".into()
}
fn fr() -> String {
    "fr".into()
}
fn fifty() -> f64 {
    50.0
}

impl Offer {
    pub fn deposit_amount(&self) -> f64 {
        money::round2(self.fee_amount.unwrap_or(0.0) * self.deposit_pct.clamp(0.0, 100.0) / 100.0)
    }
    pub fn balance_amount(&self) -> f64 {
        money::round2(self.fee_amount.unwrap_or(0.0) - self.deposit_amount())
    }
}

const SELECT: &str = "SELECT o.*, c.name AS contact_name, c.email AS contact_email, a.name AS artist_name
     FROM offers o
     LEFT JOIN contacts c ON c.id = o.contact_id
     LEFT JOIN artists a ON a.id = o.artist_id";

fn from_row(r: &rusqlite::Row) -> rusqlite::Result<Offer> {
    Ok(Offer {
        id: r.get("id")?,
        contact_id: r.get("contact_id")?,
        artist_id: r.get("artist_id")?,
        event_id: r.get("event_id")?,
        status: r.get("status")?,
        date: r.get("date")?,
        venue: r.get("venue")?,
        city: r.get("city")?,
        set_time: r.get("set_time")?,
        set_length: r.get("set_length")?,
        fee_amount: r.get("fee_amount")?,
        currency: r.get("currency")?,
        fee_type: r.get("fee_type")?,
        deal_terms: r.get("deal_terms")?,
        deposit_pct: r.get("deposit_pct")?,
        travel_by: r.get("travel_by")?,
        travel_from: r.get("travel_from")?,
        travel_to: r.get("travel_to")?,
        hotel: r.get("hotel")?,
        meals: r.get("meals")?,
        guest_list: r.get("guest_list")?,
        tech_requirements: r.get("tech_requirements")?,
        answer_by: r.get("answer_by")?,
        language: r.get("language")?,
        notes: r.get("notes")?,
        sent_at: r.get("sent_at")?,
        accepted_at: r.get("accepted_at")?,
        deposit_due: r.get("deposit_due")?,
        deposit_paid_at: r.get("deposit_paid_at")?,
        balance_due: r.get("balance_due")?,
        balance_paid_at: r.get("balance_paid_at")?,
        created_at: r.get("created_at")?,
        updated_at: r.get("updated_at")?,
        contact_name: r.get("contact_name")?,
        contact_email: r.get("contact_email")?,
        artist_name: r.get("artist_name")?,
    })
}

pub(crate) fn get(conn: &Connection, id: i64) -> Result<Offer, String> {
    conn.query_row(&format!("{SELECT} WHERE o.id = ?1"), params![id], from_row)
        .optional()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("offer {id} not found"))
}

pub(crate) fn today() -> NaiveDate {
    Local::now().date_naive()
}

pub(crate) fn parse_day(s: &Option<String>) -> Option<NaiveDate> {
    s.as_deref()
        .and_then(|v| NaiveDate::parse_from_str(v.get(..10).unwrap_or(v), "%Y-%m-%d").ok())
}

fn clean(s: &Option<String>) -> Option<String> {
    s.as_ref().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

/// End of the set: start + length, wrapping past midnight ("23:30" + 120 -> "01:30").
pub fn set_end(start: &Option<String>, length: Option<i64>) -> Option<String> {
    let t = NaiveTime::parse_from_str(start.as_deref()?.trim(), "%H:%M").ok()?;
    let len = length.filter(|l| *l > 0)?;
    let (end, _) = t.overflowing_add_signed(chrono::Duration::minutes(len));
    Some(end.format("%H:%M").to_string())
}

// ---------------------------------------------------------------------------
// Save + life cycle
// ---------------------------------------------------------------------------

/// Insert or update the offer's own fields (status transitions are applied by
/// `save` around this).
fn write_row(conn: &Connection, o: &Offer) -> Result<i64, String> {
    let fee_type = if FEE_TYPES.contains(&o.fee_type.as_str()) { o.fee_type.as_str() } else { "flat" };
    let lang = if o.language == "en" { "en" } else { "fr" };
    let currency = {
        let c = o.currency.trim().to_uppercase();
        if c.is_empty() { "EUR".to_string() } else { c }
    };
    let p = params![
        o.contact_id,
        o.artist_id,
        o.event_id,
        o.status,
        clean(&o.date),
        clean(&o.venue),
        clean(&o.city),
        clean(&o.set_time),
        o.set_length,
        o.fee_amount,
        currency,
        fee_type,
        clean(&o.deal_terms),
        o.deposit_pct.clamp(0.0, 100.0),
        clean(&o.travel_by),
        clean(&o.travel_from),
        clean(&o.travel_to),
        clean(&o.hotel),
        clean(&o.meals),
        o.guest_list,
        clean(&o.tech_requirements),
        clean(&o.answer_by),
        lang,
        clean(&o.notes),
        clean(&o.deposit_due),
        clean(&o.deposit_paid_at),
        clean(&o.balance_due),
        clean(&o.balance_paid_at),
    ];
    match o.id {
        Some(id) => {
            let n = conn
                .execute(
                    "UPDATE offers SET contact_id=?1, artist_id=?2, event_id=?3, status=?4, date=?5, venue=?6,
                        city=?7, set_time=?8, set_length=?9, fee_amount=?10, currency=?11, fee_type=?12,
                        deal_terms=?13, deposit_pct=?14, travel_by=?15, travel_from=?16, travel_to=?17,
                        hotel=?18, meals=?19, guest_list=?20, tech_requirements=?21, answer_by=?22,
                        language=?23, notes=?24, deposit_due=?25, deposit_paid_at=?26, balance_due=?27,
                        balance_paid_at=?28, updated_at=datetime('now')
                     WHERE id=?29",
                    rusqlite::params_from_iter(p.iter().copied().chain(std::iter::once(&id as &dyn rusqlite::ToSql))),
                )
                .map_err(|e| e.to_string())?;
            if n == 0 {
                return Err(format!("offer {id} not found"));
            }
            Ok(id)
        }
        None => {
            conn.execute(
                "INSERT INTO offers (contact_id, artist_id, event_id, status, date, venue, city, set_time,
                    set_length, fee_amount, currency, fee_type, deal_terms, deposit_pct, travel_by,
                    travel_from, travel_to, hotel, meals, guest_list, tech_requirements, answer_by,
                    language, notes, deposit_due, deposit_paid_at, balance_due, balance_paid_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26,?27,?28)",
                p,
            )
            .map_err(|e| e.to_string())?;
            Ok(conn.last_insert_rowid())
        }
    }
}

/// Save an offer and apply what its status means for the Agenda, the budget
/// (through the event) and the pipeline. Returns the offer id and the id of
/// the event it touched, if any.
pub(crate) fn save(conn: &Connection, offer: &Offer, today: NaiveDate) -> Result<(i64, Option<i64>), String> {
    if !OFFER_STATUSES.contains(&offer.status.as_str()) {
        return Err(format!("unknown offer status: {}", offer.status));
    }
    let before: Option<Offer> = offer.id.map(|id| get(conn, id)).transpose()?;
    let prev_status = before.as_ref().map(|o| o.status.clone()).unwrap_or_else(|| "draft".into());
    let mut o = offer.clone();
    // Keep the link to the event the offer created even if the form did not send it.
    if o.event_id.is_none() {
        o.event_id = before.as_ref().and_then(|b| b.event_id);
    }
    let today_s = today.format("%Y-%m-%d").to_string();
    o.sent_at = before.as_ref().and_then(|b| b.sent_at.clone());
    o.accepted_at = before.as_ref().and_then(|b| b.accepted_at.clone());
    if matches!(o.status.as_str(), "sent" | "accepted") && o.sent_at.is_none() {
        o.sent_at = Some(today_s.clone());
    }
    if o.status == "accepted" {
        if o.accepted_at.is_none() {
            o.accepted_at = Some(today_s.clone());
        }
        let accepted = parse_day(&o.accepted_at).unwrap_or(today);
        let (dep, bal) = money::default_due_dates(accepted, parse_day(&o.date), o.deposit_pct);
        if o.deposit_due.is_none() {
            o.deposit_due = dep;
        }
        if o.balance_due.is_none() {
            o.balance_due = Some(bal);
        }
    }

    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    let id = write_row(&tx, &o)?;
    o.id = Some(id);
    tx.execute(
        "UPDATE offers SET sent_at=?2, accepted_at=?3 WHERE id=?1",
        params![id, o.sent_at, o.accepted_at],
    )
    .map_err(|e| e.to_string())?;
    let event_id = apply_status(&tx, &o, &prev_status)?;
    if event_id != o.event_id {
        tx.execute("UPDATE offers SET event_id=?2 WHERE id=?1", params![id, event_id])
            .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok((id, event_id))
}

/// What a status means outside the offer. Returns the linked event id.
fn apply_status(conn: &Connection, o: &Offer, prev: &str) -> Result<Option<i64>, String> {
    let cid = o.contact_id;
    match o.status.as_str() {
        "sent" => {
            if let Some(c) = cid {
                status::on_offer_sent(conn, c);
            }
            // The date is held in the Agenda while the promoter decides.
            if parse_day(&o.date).is_some() {
                return upsert_event(conn, o, "hold").map(Some);
            }
            Ok(o.event_id)
        }
        "accepted" => {
            if parse_day(&o.date).is_none() {
                return Err("An accepted offer needs a date (YYYY-MM-DD).".into());
            }
            let id = upsert_event(conn, o, "confirmed")?;
            if let Some(c) = cid {
                status::on_event_confirmed(conn, c);
            }
            Ok(Some(id))
        }
        "declined" | "expired" => {
            release_event(conn, o)?;
            if o.status == "declined" && prev != "declined" {
                if let Some(c) = cid {
                    status::on_offer_declined(conn, c);
                }
            }
            Ok(o.event_id)
        }
        // Back to draft: a held date is released, a confirmed one too.
        _ => {
            if prev != "draft" {
                release_event(conn, o)?;
            }
            Ok(o.event_id)
        }
    }
}

/// Create or update the Agenda event of an offer. The event's own budget sync
/// (in `save_event_row`) keeps exactly one revenue line for a confirmed gig.
fn upsert_event(conn: &Connection, o: &Offer, ev_status: &str) -> Result<i64, String> {
    let existing: Option<Event> = match o.event_id {
        Some(id) => conn
            .query_row("SELECT * FROM events WHERE id=?1", params![id], Event::from_row)
            .optional()
            .map_err(|e| e.to_string())?,
        None => None,
    };
    // A confirmed gig is never pushed back to "hold" by re-sending the offer.
    let ev_status = match &existing {
        Some(e) if e.status == "confirmed" && ev_status == "hold" => "confirmed",
        _ => ev_status,
    };
    let title = match &existing {
        Some(e) if !e.title.trim().is_empty() => e.title.clone(),
        _ => event_title(conn, o),
    };
    let ev = Event {
        id: existing.as_ref().and_then(|e| e.id),
        artist_id: o.artist_id.or(existing.as_ref().and_then(|e| e.artist_id)),
        contact_id: o.contact_id,
        title,
        venue: clean(&o.venue).or(existing.as_ref().and_then(|e| e.venue.clone())),
        city: clean(&o.city).or(existing.as_ref().and_then(|e| e.city.clone())),
        date: o.date.clone().unwrap_or_default().chars().take(10).collect(),
        start_time: clean(&o.set_time),
        end_time: set_end(&o.set_time, o.set_length),
        status: ev_status.to_string(),
        fee: o.fee_amount,
        notes: existing.as_ref().and_then(|e| e.notes.clone()),
        created_at: None,
        updated_at: None,
    };
    let prev_contact = existing.as_ref().and_then(|e| e.contact_id);
    let was_confirmed = existing.as_ref().map(|e| e.status == "confirmed").unwrap_or(false);
    let id = events::save_event_row(conn, &ev)?;
    if was_confirmed && (ev_status != "confirmed" || prev_contact != o.contact_id) {
        if let Some(pc) = prev_contact {
            status::on_event_cancelled(conn, pc);
        }
    }
    Ok(id)
}

/// The offer fell through: its event is cancelled (kept, so the history shows
/// the date that was held) and a confirmation is withdrawn in the pipeline.
fn release_event(conn: &Connection, o: &Offer) -> Result<(), String> {
    let Some(eid) = o.event_id else { return Ok(()) };
    let row: Option<(Option<i64>, String)> = conn
        .query_row("SELECT contact_id, status FROM events WHERE id=?1", params![eid], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()
        .map_err(|e| e.to_string())?;
    let Some((ev_contact, ev_status)) = row else { return Ok(()) };
    if ev_status == "cancelled" {
        return Ok(());
    }
    conn.execute(
        "UPDATE events SET status='cancelled', updated_at=datetime('now') WHERE id=?1",
        params![eid],
    )
    .map_err(|e| e.to_string())?;
    let _ = crate::commands::data::sync_event_budget(conn, eid);
    if ev_status == "confirmed" {
        if let Some(c) = ev_contact {
            status::on_event_cancelled(conn, c);
        }
    }
    Ok(())
}

fn event_title(conn: &Connection, o: &Offer) -> String {
    let artist: Option<String> = o.artist_id.and_then(|a| {
        conn.query_row("SELECT name FROM artists WHERE id=?1", params![a], |r| r.get(0))
            .optional()
            .ok()
            .flatten()
    });
    let place = clean(&o.venue)
        .or_else(|| {
            o.contact_id.and_then(|c| {
                conn.query_row("SELECT COALESCE(NULLIF(trim(venue),''), name) FROM contacts WHERE id=?1", params![c], |r| r.get(0))
                    .optional()
                    .ok()
                    .flatten()
            })
        })
        .unwrap_or_else(|| "Gig".into());
    match artist {
        Some(a) if !a.trim().is_empty() => format!("{a} @ {place}"),
        _ => place,
    }
}

/// Sent offers whose answer deadline passed become "expired" (their held date
/// is released). Returns how many moved.
pub(crate) fn expire_overdue(conn: &Connection, today: NaiveDate) -> usize {
    let ids: Vec<i64> = conn
        .prepare("SELECT id FROM offers WHERE status='sent' AND answer_by IS NOT NULL AND answer_by <> '' AND substr(answer_by,1,10) < ?1")
        .and_then(|mut s| {
            s.query_map(params![today.format("%Y-%m-%d").to_string()], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<i64>>>()
        })
        .unwrap_or_default();
    let mut n = 0;
    for id in ids {
        if let Ok(mut o) = get(conn, id) {
            o.status = "expired".into();
            if save(conn, &o, today).is_ok() {
                n += 1;
            }
        }
    }
    n
}

pub(crate) fn list(conn: &Connection, contact_id: Option<i64>, event_id: Option<i64>) -> Result<Vec<Offer>, String> {
    let mut sql = format!("{SELECT} WHERE 1=1");
    let mut args: Vec<i64> = Vec::new();
    if let Some(c) = contact_id {
        sql.push_str(" AND o.contact_id = ?");
        args.push(c);
    }
    if let Some(e) = event_id {
        sql.push_str(" AND o.event_id = ?");
        args.push(e);
    }
    sql.push_str(" ORDER BY COALESCE(o.date, '9999') DESC, o.id DESC");
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(args), from_row)
        .map_err(|e| e.to_string())?;
    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[tauri::command(async)]
pub fn list_offers(
    state: State<'_, AppState>,
    contact_id: Option<i64>,
    event_id: Option<i64>,
) -> Result<Vec<Offer>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    expire_overdue(&conn, today());
    list(&conn, contact_id, event_id)
}

#[tauri::command(async)]
pub fn get_offer(state: State<'_, AppState>, id: i64) -> Result<Offer, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    get(&conn, id)
}

/// A new offer, prefilled from the contact (venue, city, date) and the artist
/// (tech rider). Not saved.
#[tauri::command(async)]
pub fn new_offer_for_contact(state: State<'_, AppState>, contact_id: i64) -> Result<Offer, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    prefill(&conn, contact_id)
}

pub(crate) fn prefill(conn: &Connection, contact_id: i64) -> Result<Offer, String> {
    let (name, venue, area, date, artist_id): (String, Option<String>, Option<String>, Option<String>, Option<i64>) = conn
        .query_row(
            "SELECT name, venue, area, date, artist_id FROM contacts WHERE id=?1",
            params![contact_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .map_err(|e| e.to_string())?;
    // The contact's artist, else the only artist of the roster.
    let artist_id = artist_id.or_else(|| {
        conn.query_row("SELECT id FROM artists WHERE (SELECT COUNT(*) FROM artists)=1", [], |r| r.get(0))
            .optional()
            .ok()
            .flatten()
    });
    let tech: Option<String> = artist_id.and_then(|a| {
        conn.query_row("SELECT tech_rider FROM artists WHERE id=?1", params![a], |r| r.get(0))
            .optional()
            .ok()
            .flatten()
    });
    let date = date.filter(|d| NaiveDate::parse_from_str(d.get(..10).unwrap_or(d), "%Y-%m-%d").is_ok());
    let deadline = (today() + chrono::Duration::days(7)).format("%Y-%m-%d").to_string();
    let lang = conn
        .query_row("SELECT value FROM settings WHERE key='offer_default_language'", [], |r| r.get::<_, String>(0))
        .optional()
        .ok()
        .flatten()
        .unwrap_or_else(|| "fr".into());
    Ok(Offer {
        contact_id: Some(contact_id),
        artist_id,
        status: "draft".into(),
        venue: clean(&venue).or(Some(name)),
        city: clean(&area),
        date,
        set_length: Some(90),
        currency: "EUR".into(),
        fee_type: "flat".into(),
        deposit_pct: 50.0,
        travel_by: Some("promoter".into()),
        tech_requirements: clean(&tech),
        answer_by: Some(deadline),
        language: lang,
        ..Default::default()
    })
}

#[derive(Serialize)]
pub struct SavedOffer {
    pub id: i64,
    pub event_id: Option<i64>,
}

#[tauri::command(async)]
pub fn save_offer(state: State<'_, AppState>, offer: Offer) -> Result<SavedOffer, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let before_event = offer.id.and_then(|id| get(&conn, id).ok()).and_then(|o| o.event_id);
    let (id, event_id) = save(&conn, &offer, today())?;
    for e in [before_event, event_id].into_iter().flatten() {
        crate::commands::insrt_gigs::after_event_change(&state.pool, e);
    }
    Ok(SavedOffer { id, event_id })
}

#[tauri::command(async)]
pub fn set_offer_status(state: State<'_, AppState>, id: i64, status: String) -> Result<SavedOffer, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let mut o = get(&conn, id)?;
    o.status = status;
    let (id, event_id) = save(&conn, &o, today())?;
    if let Some(e) = event_id {
        crate::commands::insrt_gigs::after_event_change(&state.pool, e);
    }
    Ok(SavedOffer { id, event_id })
}

/// Deletes the offer only: its Agenda event (if any) stays, the user decides
/// about the date in the Agenda.
#[tauri::command(async)]
pub fn delete_offer(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM offers WHERE id=?1", params![id]).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn db() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        crate::db::migrate_for_tests(&c).unwrap();
        c.execute("INSERT INTO artists (id, name, tech_rider) VALUES (1, 'Piers', 'CDJ-3000 x2, DJM-900')", []).unwrap();
        c.execute(
            "INSERT INTO contacts (id, name, venue, area, email, status) VALUES (1, 'Paradiso', 'Paradiso', 'Amsterdam', 'book@paradiso.nl', 'contacted')",
            [],
        )
        .unwrap();
        c
    }

    pub fn day(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn contact_status(c: &Connection) -> String {
        c.query_row("SELECT status FROM contacts WHERE id=1", [], |r| r.get(0)).unwrap()
    }

    fn revenue(c: &Connection) -> (i64, f64) {
        c.query_row(
            "SELECT COUNT(*), COALESCE(SUM(actual),0) FROM budget_items WHERE kind='revenue'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap()
    }

    pub fn offer(c: &Connection) -> Offer {
        let mut o = prefill(c, 1).unwrap();
        o.date = Some("2026-12-12".into());
        o.set_time = Some("23:30".into());
        o.set_length = Some(120);
        o.fee_amount = Some(1200.0);
        o
    }

    #[test]
    fn prefill_takes_contact_and_artist() {
        let c = db();
        let o = prefill(&c, 1).unwrap();
        assert_eq!(o.artist_id, Some(1));
        assert_eq!(o.venue.as_deref(), Some("Paradiso"));
        assert_eq!(o.city.as_deref(), Some("Amsterdam"));
        assert_eq!(o.tech_requirements.as_deref(), Some("CDJ-3000 x2, DJM-900"));
    }

    #[test]
    fn set_end_wraps_midnight() {
        assert_eq!(set_end(&Some("23:30".into()), Some(120)).as_deref(), Some("01:30"));
        assert_eq!(set_end(&Some("21:00".into()), Some(90)).as_deref(), Some("22:30"));
        assert_eq!(set_end(&Some("late".into()), Some(90)), None);
        assert_eq!(set_end(&Some("21:00".into()), None), None);
    }

    #[test]
    fn status_flow_moves_contact_event_and_budget_once() {
        let c = db();
        let t = day("2026-11-01");
        // Draft: nothing outside the offer.
        let (id, ev) = save(&c, &offer(&c), t).unwrap();
        assert_eq!(ev, None);
        assert_eq!(contact_status(&c), "contacted");

        // Sent: contact in discussion, date held, no money yet.
        let mut o = get(&c, id).unwrap();
        o.status = "sent".into();
        let (_, ev) = save(&c, &o, t).unwrap();
        let ev = ev.expect("hold event");
        assert_eq!(contact_status(&c), "in_discussion");
        let (st, start, end): (String, String, String) = c
            .query_row("SELECT status, start_time, end_time FROM events WHERE id=?1", [ev], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap();
        assert_eq!((st.as_str(), start.as_str(), end.as_str()), ("hold", "23:30", "01:30"));
        assert_eq!(revenue(&c), (0, 0.0));

        // Accepted: same event confirmed, one revenue line, contact confirmed,
        // due dates set.
        let mut o = get(&c, id).unwrap();
        o.status = "accepted".into();
        let (_, ev2) = save(&c, &o, t).unwrap();
        assert_eq!(ev2, Some(ev));
        assert_eq!(contact_status(&c), "confirmed");
        assert_eq!(revenue(&c), (1, 1200.0));
        let o = get(&c, id).unwrap();
        assert_eq!(o.accepted_at.as_deref(), Some("2026-11-01"));
        assert_eq!(o.deposit_due.as_deref(), Some("2026-11-15"));
        assert_eq!(o.balance_due.as_deref(), Some("2026-12-12"));

        // Saved again with a new fee (and accepted twice): still one line.
        let mut o = get(&c, id).unwrap();
        o.fee_amount = Some(1500.0);
        save(&c, &o, t).unwrap();
        save(&c, &o, t).unwrap();
        assert_eq!(revenue(&c), (1, 1500.0));
        let n: i64 = c.query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
        crate::commands::data::sync_all_event_budgets(&c).unwrap();
        assert_eq!(revenue(&c), (1, 1500.0));

        // Declined after all: event cancelled, money gone, contact declined.
        let mut o = get(&c, id).unwrap();
        o.status = "declined".into();
        save(&c, &o, t).unwrap();
        assert_eq!(revenue(&c), (0, 0.0));
        let st: String = c.query_row("SELECT status FROM events WHERE id=?1", [ev], |r| r.get(0)).unwrap();
        assert_eq!(st, "cancelled");
        assert_eq!(contact_status(&c), "declined");
    }

    #[test]
    fn declined_keeps_contact_open_when_another_offer_runs() {
        let c = db();
        let t = day("2026-11-01");
        let mut a = offer(&c);
        a.status = "sent".into();
        save(&c, &a, t).unwrap();
        let mut b = offer(&c);
        b.date = Some("2027-01-20".into());
        b.status = "sent".into();
        let (bid, _) = save(&c, &b, t).unwrap();
        let mut b = get(&c, bid).unwrap();
        b.status = "declined".into();
        save(&c, &b, t).unwrap();
        assert_eq!(contact_status(&c), "in_discussion");
    }

    #[test]
    fn accepted_needs_a_date_and_bad_status_is_refused() {
        let c = db();
        let mut o = offer(&c);
        o.date = None;
        o.status = "accepted".into();
        assert!(save(&c, &o, day("2026-11-01")).is_err());
        let n: i64 = c.query_row("SELECT COUNT(*) FROM offers", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0, "rolled back");
        o.status = "maybe".into();
        assert!(save(&c, &o, day("2026-11-01")).is_err());
    }

    #[test]
    fn overdue_sent_offers_expire_and_release_the_hold() {
        let c = db();
        let mut o = offer(&c);
        o.status = "sent".into();
        o.answer_by = Some("2026-11-05".into());
        let (id, ev) = save(&c, &o, day("2026-11-01")).unwrap();
        assert_eq!(expire_overdue(&c, day("2026-11-05")), 0);
        assert_eq!(expire_overdue(&c, day("2026-11-06")), 1);
        assert_eq!(get(&c, id).unwrap().status, "expired");
        let st: String = c.query_row("SELECT status FROM events WHERE id=?1", [ev.unwrap()], |r| r.get(0)).unwrap();
        assert_eq!(st, "cancelled");
        assert_eq!(expire_overdue(&c, day("2026-11-07")), 0);
    }
}
