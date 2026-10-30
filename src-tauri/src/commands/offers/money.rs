//! Deposit and balance of an accepted offer: due dates, paid dates, invoice
//! numbers, and the expected / received / overdue view used by Budget, Home
//! and Agenda › Actions.
//!
//! The money expected for a gig is the fee of its confirmed Agenda event (the
//! budget revenue line, see `data::sync_event_budget`); payments here only say
//! how much of it came in. Nothing in this file writes `budget_items`.
//!
//! Invoice numbers: `{prefix}{year}-{seq:03}` (prefix from the
//! `invoice_prefix` setting, "F" by default), one sequence per calendar year,
//! never reused, no gaps (a number is only taken when a PDF is issued, and an
//! invoice issued twice for the same offer and part keeps its number).

use chrono::{Datelike, NaiveDate};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use tauri::State;

use super::{get, parse_day, today};
use crate::AppState;

pub fn migrate(conn: &Connection) {
    let _ = conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS invoices (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            offer_id   INTEGER REFERENCES offers(id) ON DELETE SET NULL,
            part       TEXT NOT NULL,
            number     TEXT NOT NULL UNIQUE,
            year       INTEGER NOT NULL,
            seq        INTEGER NOT NULL,
            amount     REAL NOT NULL,
            currency   TEXT NOT NULL DEFAULT 'EUR',
            issued_on  TEXT NOT NULL,
            due_on     TEXT,
            file_path  TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE (year, seq)
        );
        CREATE UNIQUE INDEX IF NOT EXISTS idx_invoices_offer_part ON invoices(offer_id, part) WHERE offer_id IS NOT NULL;",
    );
}

pub fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// Default due dates once an offer is accepted on `accepted`:
/// - deposit: 14 days after acceptance, but no later than 21 days before the
///   gig (and never before the acceptance itself); none when the deposit is 0%.
/// - balance: the day of the gig (paid on the night or by transfer that day);
///   30 days after acceptance when the date is unknown.
pub fn default_due_dates(accepted: NaiveDate, gig: Option<NaiveDate>, deposit_pct: f64) -> (Option<String>, String) {
    let fmt = |d: NaiveDate| d.format("%Y-%m-%d").to_string();
    let deposit = (deposit_pct > 0.0).then(|| {
        let mut d = accepted + chrono::Duration::days(14);
        if let Some(g) = gig {
            d = d.min(g - chrono::Duration::days(21));
        }
        fmt(d.max(accepted))
    });
    let balance = gig.unwrap_or(accepted + chrono::Duration::days(30));
    (deposit, fmt(balance))
}

fn prefix(conn: &Connection) -> String {
    conn.query_row("SELECT value FROM settings WHERE key='invoice_prefix'", [], |r| r.get::<_, String>(0))
        .optional()
        .ok()
        .flatten()
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| "F".into())
}

pub fn format_number(prefix: &str, year: i32, seq: i64) -> String {
    format!("{prefix}{year}-{seq:03}")
}

#[derive(Debug, Clone, Serialize)]
pub struct Invoice {
    pub id: i64,
    pub offer_id: Option<i64>,
    /// deposit | balance
    pub part: String,
    pub number: String,
    pub amount: f64,
    pub currency: String,
    pub issued_on: String,
    pub due_on: Option<String>,
    pub file_path: Option<String>,
}

fn invoice_row(r: &rusqlite::Row) -> rusqlite::Result<Invoice> {
    Ok(Invoice {
        id: r.get("id")?,
        offer_id: r.get("offer_id")?,
        part: r.get("part")?,
        number: r.get("number")?,
        amount: r.get("amount")?,
        currency: r.get("currency")?,
        issued_on: r.get("issued_on")?,
        due_on: r.get("due_on")?,
        file_path: r.get("file_path")?,
    })
}

pub(crate) fn find_invoice(conn: &Connection, offer_id: i64, part: &str) -> Option<Invoice> {
    conn.query_row(
        "SELECT * FROM invoices WHERE offer_id=?1 AND part=?2",
        params![offer_id, part],
        invoice_row,
    )
    .optional()
    .ok()
    .flatten()
}

/// The invoice of one part of an offer, numbered on first issue. Issued again,
/// the same number comes back with the current amount (an amount that changed
/// before payment is corrected on the same invoice).
pub(crate) fn issue_invoice(conn: &Connection, offer_id: i64, part: &str, today: NaiveDate) -> Result<Invoice, String> {
    let o = get(conn, offer_id)?;
    if o.status != "accepted" {
        return Err("Invoices are issued for accepted offers only.".into());
    }
    let (amount, due) = match part {
        "deposit" if o.deposit_amount() > 0.0 => (o.deposit_amount(), o.deposit_due.clone()),
        "deposit" => return Err("This offer has no deposit.".into()),
        "balance" => (o.balance_amount(), o.balance_due.clone()),
        _ => return Err(format!("unknown invoice part: {part}")),
    };
    if let Some(inv) = find_invoice(conn, offer_id, part) {
        conn.execute(
            "UPDATE invoices SET amount=?2, currency=?3, due_on=?4 WHERE id=?1",
            params![inv.id, amount, o.currency, due],
        )
        .map_err(|e| e.to_string())?;
        return find_invoice(conn, offer_id, part).ok_or_else(|| "invoice vanished".into());
    }
    let year = today.year();
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    let seq: i64 = tx
        .query_row("SELECT COALESCE(MAX(seq), 0) + 1 FROM invoices WHERE year=?1", params![year], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    let number = format_number(&prefix(&tx), year, seq);
    tx.execute(
        "INSERT INTO invoices (offer_id, part, number, year, seq, amount, currency, issued_on, due_on)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![offer_id, part, number, year, seq, amount, o.currency, today.format("%Y-%m-%d").to_string(), due],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    find_invoice(conn, offer_id, part).ok_or_else(|| "invoice not saved".into())
}

/// One part (deposit or balance) of an accepted offer, for the money views.
#[derive(Debug, Clone, Serialize)]
pub struct Payment {
    pub offer_id: i64,
    pub event_id: Option<i64>,
    pub contact_id: Option<i64>,
    pub title: String,
    pub gig_date: Option<String>,
    pub part: String,
    pub amount: f64,
    pub currency: String,
    pub due: Option<String>,
    pub paid_at: Option<String>,
    pub overdue: bool,
    pub invoice_number: Option<String>,
}

pub(crate) fn payments(conn: &Connection, today: NaiveDate) -> Result<Vec<Payment>, String> {
    let offers = super::list(conn, None, None)?;
    let mut out = Vec::new();
    for o in offers.into_iter().filter(|o| o.status == "accepted") {
        let id = o.id.unwrap_or_default();
        let title = [o.artist_name.clone(), o.venue.clone().or(o.contact_name.clone())]
            .into_iter()
            .flatten()
            .filter(|s| !s.trim().is_empty())
            .collect::<Vec<_>>()
            .join(" @ ");
        let parts = [
            ("deposit", o.deposit_amount(), o.deposit_due.clone(), o.deposit_paid_at.clone()),
            ("balance", o.balance_amount(), o.balance_due.clone(), o.balance_paid_at.clone()),
        ];
        for (part, amount, due, paid) in parts {
            if amount <= 0.0 {
                continue;
            }
            let overdue = paid.is_none() && parse_day(&due).map(|d| d < today).unwrap_or(false);
            out.push(Payment {
                offer_id: id,
                event_id: o.event_id,
                contact_id: o.contact_id,
                title: title.clone(),
                gig_date: o.date.clone(),
                part: part.into(),
                amount,
                currency: o.currency.clone(),
                due,
                paid_at: paid,
                overdue,
                invoice_number: find_invoice(conn, id, part).map(|i| i.number),
            });
        }
    }
    out.sort_by(|a, b| a.due.cmp(&b.due));
    Ok(out)
}

/// Mark one part paid on `paid_at` (YYYY-MM-DD), or unpaid with None.
pub(crate) fn set_paid(conn: &Connection, offer_id: i64, part: &str, paid_at: Option<String>) -> Result<(), String> {
    let col = match part {
        "deposit" => "deposit_paid_at",
        "balance" => "balance_paid_at",
        _ => return Err(format!("unknown payment part: {part}")),
    };
    let paid_at = paid_at.map(|s| s.trim().chars().take(10).collect::<String>()).filter(|s| !s.is_empty());
    if let Some(p) = &paid_at {
        NaiveDate::parse_from_str(p, "%Y-%m-%d").map_err(|_| format!("invalid date: {p}"))?;
    }
    let n = conn
        .execute(
            &format!("UPDATE offers SET {col}=?2, updated_at=datetime('now') WHERE id=?1"),
            params![offer_id, paid_at],
        )
        .map_err(|e| e.to_string())?;
    if n == 0 {
        return Err(format!("offer {offer_id} not found"));
    }
    Ok(())
}

#[tauri::command(async)]
pub fn list_offer_payments(state: State<'_, AppState>) -> Result<Vec<Payment>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    super::expire_overdue(&conn, today());
    payments(&conn, today())
}

#[tauri::command(async)]
pub fn set_offer_payment(
    state: State<'_, AppState>,
    offer_id: i64,
    part: String,
    paid_at: Option<String>,
) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    set_paid(&conn, offer_id, &part, paid_at)
}

#[tauri::command(async)]
pub fn list_offer_invoices(state: State<'_, AppState>, offer_id: i64) -> Result<Vec<Invoice>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT * FROM invoices WHERE offer_id=?1 ORDER BY year, seq")
        .map_err(|e| e.to_string())?;
    let rows = stmt.query_map(params![offer_id], invoice_row).map_err(|e| e.to_string())?;
    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::super::tests::{day, db, offer};
    use super::super::save;
    use super::*;

    #[test]
    fn due_dates() {
        let a = day("2026-11-01");
        assert_eq!(
            default_due_dates(a, Some(day("2026-12-12")), 50.0),
            (Some("2026-11-15".into()), "2026-12-12".into())
        );
        // Gig soon: deposit no later than J-21, never before acceptance.
        assert_eq!(
            default_due_dates(a, Some(day("2026-11-30")), 50.0),
            (Some("2026-11-09".into()), "2026-11-30".into())
        );
        assert_eq!(
            default_due_dates(a, Some(day("2026-11-10")), 30.0),
            (Some("2026-11-01".into()), "2026-11-10".into())
        );
        assert_eq!(default_due_dates(a, None, 0.0), (None, "2026-12-01".into()));
    }

    #[test]
    fn invoice_numbers_are_sequential_per_year_and_stable() {
        let c = db();
        let t = day("2026-11-01");
        let mut o = offer(&c);
        o.status = "accepted".into();
        let (a, _) = save(&c, &o, t).unwrap();
        let mut o2 = offer(&c);
        o2.date = Some("2027-02-01".into());
        o2.status = "accepted".into();
        let (b, _) = save(&c, &o2, t).unwrap();

        let i1 = issue_invoice(&c, a, "deposit", t).unwrap();
        let i2 = issue_invoice(&c, b, "deposit", t).unwrap();
        let again = issue_invoice(&c, a, "deposit", t).unwrap();
        assert_eq!(i1.number, "F2026-001");
        assert_eq!(i2.number, "F2026-002");
        assert_eq!(again.number, "F2026-001", "same part keeps its number");
        assert_eq!(i1.amount, 600.0);

        c.execute("INSERT INTO settings (key, value) VALUES ('invoice_prefix', 'PIERS-')", []).unwrap();
        let i3 = issue_invoice(&c, a, "balance", t).unwrap();
        assert_eq!(i3.number, "PIERS-2026-003");
        let next_year = issue_invoice(&c, b, "balance", day("2027-01-03")).unwrap();
        assert_eq!(next_year.number, "PIERS-2027-001");

        // Not for an offer still in draft.
        let (d, _) = save(&c, &offer(&c), t).unwrap();
        assert!(issue_invoice(&c, d, "deposit", t).is_err());
    }

    #[test]
    fn payments_overdue_and_paid() {
        let c = db();
        let t = day("2026-11-01");
        let mut o = offer(&c);
        o.status = "accepted".into();
        let (id, _) = save(&c, &o, t).unwrap();
        let p = payments(&c, day("2026-11-16")).unwrap();
        assert_eq!(p.len(), 2);
        assert!(p[0].part == "deposit" && p[0].overdue && p[0].amount == 600.0);
        assert!(!p[1].overdue);
        set_paid(&c, id, "deposit", Some("2026-11-16".into())).unwrap();
        let p = payments(&c, day("2026-11-16")).unwrap();
        assert!(!p[0].overdue);
        assert_eq!(p[0].paid_at.as_deref(), Some("2026-11-16"));
        assert!(set_paid(&c, id, "deposit", Some("16/11".into())).is_err());
        // Payments never touch the budget: still one line at the fee.
        let (n, sum): (i64, f64) = c
            .query_row("SELECT COUNT(*), SUM(actual) FROM budget_items WHERE kind='revenue'", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!((n, sum), (1, 1200.0));
    }
}
