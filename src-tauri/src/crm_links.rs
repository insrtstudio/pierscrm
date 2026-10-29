//! Links between CRM modules (audit 2026-10-29, items 5, 7, 9, 10, 15): the
//! contacts -> artist / organisation columns, budget lines -> Agenda events,
//! visa dossiers -> Agenda events, and free-text dates -> ISO `YYYY-MM-DD`.
//!
//! Called once per launch from `db::migrate`. Every step is idempotent:
//! schema changes are `IF NOT EXISTS` / tolerated duplicate-column errors, and
//! the data backfills run once each, recorded under a `settings` key, inside a
//! transaction so a failure leaves the data exactly as it was (and is retried
//! on the next launch). Nothing here may block startup: errors are logged only.

use chrono::{Datelike, NaiveDate, Weekday};
use regex::Regex;
use rusqlite::{params, Connection, OptionalExtension};
use std::sync::OnceLock;

// ---------------------------------------------------------------------------
// Free-text date parser
// ---------------------------------------------------------------------------

/// A date read out of free text. `exact` is false when something in the source
/// could not be represented by one calendar day (a range "Oct 21-25", a weekday
/// that does not match, extra words): the caller then keeps the original text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LooseDate {
    pub date: NaiveDate,
    pub exact: bool,
}

fn month_of(tok: &str) -> Option<u32> {
    Some(match tok {
        "jan" | "janv" | "january" | "janvier" => 1,
        "feb" | "febr" | "february" | "fev" | "fevr" | "fevrier" => 2,
        "mar" | "march" | "mars" => 3,
        "apr" | "april" | "avr" | "avril" => 4,
        "may" | "mai" => 5,
        "jun" | "june" | "juin" => 6,
        "jul" | "july" | "juil" | "juillet" => 7,
        "aug" | "august" | "aou" | "aout" => 8,
        "sep" | "sept" | "september" | "septembre" => 9,
        "oct" | "october" | "octobre" => 10,
        "nov" | "november" | "novembre" => 11,
        "dec" | "december" | "decembre" => 12,
        _ => return None,
    })
}

fn weekday_of(tok: &str) -> Option<Weekday> {
    // French "mar" (mardi) is left out on purpose: it is also March.
    Some(match tok {
        "mon" | "monday" | "lun" | "lundi" => Weekday::Mon,
        "tue" | "tues" | "tuesday" | "mardi" => Weekday::Tue,
        "wed" | "weds" | "wednesday" | "mer" | "mercredi" => Weekday::Wed,
        "thu" | "thur" | "thurs" | "thursday" | "jeu" | "jeudi" => Weekday::Thu,
        "fri" | "friday" | "ven" | "vendredi" => Weekday::Fri,
        "sat" | "saturday" | "sam" | "samedi" => Weekday::Sat,
        "sun" | "sunday" | "dim" | "dimanche" => Weekday::Sun,
        _ => return None,
    })
}

/// Words that carry no information in a date ("le 21 octobre", "21st of Oct").
fn is_filler(tok: &str) -> bool {
    matches!(
        tok,
        "le" | "the" | "of" | "de" | "du" | "st" | "nd" | "rd" | "th" | "er" | "eme" | "e"
    )
}

/// Year for a day given without one. A matching weekday ("Wed Oct 21") picks
/// the year; otherwise the first occurrence from two months before `reference`
/// on (a booking sheet written in July about "Oct 21" means this October, one
/// written in November about "Jan 15" means next January).
fn infer_year(month: u32, day: u32, weekday: Option<Weekday>, reference: NaiveDate) -> Option<(NaiveDate, bool)> {
    let ry = reference.year();
    let candidates: Vec<NaiveDate> = (ry - 1..=ry + 2)
        .filter_map(|y| NaiveDate::from_ymd_opt(y, month, day))
        .collect();
    if candidates.is_empty() {
        return None;
    }
    let floor = reference - chrono::Duration::days(61);
    let default = candidates
        .iter()
        .copied()
        .find(|d| *d >= floor)
        .unwrap_or(*candidates.last().unwrap());
    match weekday {
        None => Some((default, true)),
        Some(w) => {
            // Closest matching year to the default guess.
            let hit = candidates
                .iter()
                .copied()
                .filter(|d| d.weekday() == w)
                .min_by_key(|d| (d.year() - default.year()).abs());
            match hit {
                Some(d) => Some((d, true)),
                // Weekday contradicts every nearby year: keep the guess, flag it.
                None => Some((default, false)),
            }
        }
    }
}

fn fold(s: &str) -> String {
    s.chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'à' | 'á' | 'â' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'î' | 'ï' => 'i',
            'ô' | 'ö' => 'o',
            'û' | 'ù' | 'ü' => 'u',
            'ç' => 'c',
            '–' | '—' => '-',
            c => c,
        })
        .collect()
}

fn re(cell: &'static OnceLock<Regex>, pat: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pat).expect("static regex"))
}

/// Parse a date as people type it in a booking sheet: ISO, `21/10/2026`,
/// `21/10`, `Wed Oct 21`, `Oct 21`, `21 octobre 2026`, `Oct 21-25`, Excel
/// serial numbers. `reference` (usually the row's creation day) resolves a
/// missing year. Returns None when no day can be read ("ADE (date TBC)").
pub fn parse_loose_date(raw: &str, reference: NaiveDate) -> Option<LooseDate> {
    static ISO: OnceLock<Regex> = OnceLock::new();
    static DMY: OnceLock<Regex> = OnceLock::new();
    static YMD: OnceLock<Regex> = OnceLock::new();
    static SERIAL: OnceLock<Regex> = OnceLock::new();
    static TOKENS: OnceLock<Regex> = OnceLock::new();
    static RANGE: OnceLock<Regex> = OnceLock::new();

    let s = fold(raw.trim());
    if s.is_empty() {
        return None;
    }

    // ISO, possibly followed by a time ("2026-10-21 18:00:00").
    if let Some(c) = re(&ISO, r"^(\d{4})-(\d{1,2})-(\d{1,2})(.*)$").captures(&s) {
        let d = NaiveDate::from_ymd_opt(c[1].parse().ok()?, c[2].parse().ok()?, c[3].parse().ok()?)?;
        let rest = c[4].trim();
        let exact = rest.is_empty() || rest.trim_start_matches('t').trim_matches(|ch: char| ch == '0' || ch == ':' || ch == '.' || ch == 'z').is_empty();
        return Some(LooseDate { date: d, exact });
    }
    if let Some(c) = re(&YMD, r"^(\d{4})[/.](\d{1,2})[/.](\d{1,2})$").captures(&s) {
        let d = NaiveDate::from_ymd_opt(c[1].parse().ok()?, c[2].parse().ok()?, c[3].parse().ok()?)?;
        return Some(LooseDate { date: d, exact: true });
    }
    // Numeric day/month[/year]. French order; a first number above 12 with a
    // second at most 12 is unambiguous, the reverse means a US month/day.
    if let Some(c) = re(&DMY, r"^(\d{1,2})[/.\-](\d{1,2})(?:[/.\-](\d{2}|\d{4}))?$").captures(&s) {
        let a: u32 = c[1].parse().ok()?;
        let b: u32 = c[2].parse().ok()?;
        let (day, month) = if a <= 12 && b > 12 { (b, a) } else { (a, b) };
        return match c.get(3) {
            Some(y) => {
                let mut y: i32 = y.as_str().parse().ok()?;
                if y < 100 {
                    y += 2000;
                }
                NaiveDate::from_ymd_opt(y, month, day).map(|d| LooseDate { date: d, exact: true })
            }
            None => infer_year(month, day, None, reference).map(|(d, exact)| LooseDate { date: d, exact }),
        };
    }
    // Excel serial day number (1900 system), as a numeric cell comes out of a
    // spreadsheet export. Limited to 1982..2064 so a stray number is not read.
    if re(&SERIAL, r"^\d{5}(\.\d+)?$").is_match(&s) {
        let n: f64 = s.parse().ok()?;
        if (30_000.0..60_000.0).contains(&n) {
            let base = NaiveDate::from_ymd_opt(1899, 12, 30)?;
            let d = base + chrono::Duration::days(n.trunc() as i64);
            return Some(LooseDate { date: d, exact: n.fract() == 0.0 });
        }
        return None;
    }

    // Textual: collect month, weekday, numbers; anything else makes it inexact.
    let is_range = re(&RANGE, r"\d\s*(?:-|\bto\b|\bau\b|\ba\b)\s*[a-z0-9]").is_match(&s);
    let mut month: Option<u32> = None;
    let mut extra_month = false;
    let mut weekday: Option<Weekday> = None;
    let mut numbers: Vec<u32> = Vec::new();
    let mut leftovers = false;
    for m in re(&TOKENS, r"[a-z]+|\d+").find_iter(&s) {
        let tok = m.as_str();
        if tok.as_bytes()[0].is_ascii_digit() {
            match tok.parse::<u32>() {
                Ok(n) => numbers.push(n),
                Err(_) => leftovers = true,
            }
        } else if let Some(mo) = month_of(tok) {
            if month.is_none() {
                month = Some(mo);
            } else {
                extra_month = true;
            }
        } else if let Some(w) = weekday_of(tok) {
            weekday = weekday.or(Some(w));
        } else if is_filler(tok) || ((tok == "to" || tok == "au" || tok == "a") && is_range) {
        } else {
            leftovers = true;
        }
    }
    let month = month?;
    let year_pos = numbers.iter().position(|n| (1900..=2100).contains(n));
    let year = year_pos.map(|i| numbers.remove(i) as i32);
    let day_pos = numbers.iter().position(|n| (1..=31).contains(n))?;
    let day = numbers.remove(day_pos);
    let inexact_source = is_range || extra_month || leftovers || !numbers.is_empty();
    let (date, ok) = match year {
        Some(y) => {
            let d = NaiveDate::from_ymd_opt(y, month, day)?;
            (d, weekday.map_or(true, |w| d.weekday() == w))
        }
        None => infer_year(month, day, weekday, reference)?,
    };
    Some(LooseDate { date, exact: ok && !inexact_source })
}

/// ISO day and exactness, for the tests.
#[cfg(test)]
pub fn to_iso(raw: &str, reference: NaiveDate) -> Option<(String, bool)> {
    parse_loose_date(raw, reference).map(|d| (d.date.format("%Y-%m-%d").to_string(), d.exact))
}

fn is_iso_day(v: &str) -> bool {
    v.len() == 10 && NaiveDate::parse_from_str(v, "%Y-%m-%d").is_ok()
}

// ---------------------------------------------------------------------------
// Migration entry point
// ---------------------------------------------------------------------------

pub fn migrate(conn: &Connection) {
    schema(conn);
    // Contacts -> artist and contacts -> organisation are no longer backfilled
    // here: both are proposed in the Pipeline and applied only once the user
    // validates them (bulk "assign to an artist", organisation proposals in
    // commands/organisations.rs). Rows a previous version already linked stay.
    once(conn, "dates_iso_v1", migrate_dates);
    // Cheap and safe on every launch: realigns the Agenda-linked revenue lines
    // with the events, whatever path changed an event's status or fee.
    if let Err(e) = crate::commands::data::sync_all_event_budgets(conn) {
        eprintln!("[migrate] event budget sync: {e}");
    }
}

fn schema(conn: &Connection) {
    let _ = conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS organisations (
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            name         TEXT NOT NULL,
            email_domain TEXT,
            notes        TEXT,
            created_at   TEXT NOT NULL DEFAULT (datetime('now'))
        );",
    );
    for stmt in [
        "ALTER TABLE contacts ADD COLUMN organisation_id INTEGER REFERENCES organisations(id) ON DELETE SET NULL",
        // CASCADE: deleting a gig removes its fee line with it.
        "ALTER TABLE budget_items ADD COLUMN event_id INTEGER REFERENCES events(id) ON DELETE CASCADE",
        "ALTER TABLE visa_dossiers ADD COLUMN event_id INTEGER REFERENCES events(id) ON DELETE SET NULL",
    ] {
        let _ = conn.execute(stmt, []);
    }
    for stmt in [
        "CREATE INDEX IF NOT EXISTS idx_contacts_org ON contacts(organisation_id)",
        // One revenue line per gig, at most.
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_budget_event ON budget_items(event_id) WHERE event_id IS NOT NULL",
        "CREATE INDEX IF NOT EXISTS idx_visa_event ON visa_dossiers(event_id)",
        "CREATE INDEX IF NOT EXISTS idx_tasks_due ON tasks(due_date)",
        // Visa check: country of a gig looked up from the Discovery venues of its city.
        "CREATE INDEX IF NOT EXISTS idx_vi_venues_ville_lc ON vi_venues(lower(trim(ville)))",
    ] {
        let _ = conn.execute(stmt, []);
    }
}

/// Runs `f` once in a transaction, recorded under `settings.key`. `f` returns
/// Ok(false) to say "not applicable yet, try again next launch".
fn once(conn: &Connection, key: &str, f: fn(&Connection) -> rusqlite::Result<bool>) {
    let done: bool = conn
        .query_row("SELECT 1 FROM settings WHERE key=?1", [key], |_| Ok(true))
        .optional()
        .ok()
        .flatten()
        .unwrap_or(false);
    if done {
        return;
    }
    let run = || -> rusqlite::Result<()> {
        conn.execute_batch("SAVEPOINT crm_links")?;
        let res = f(conn).and_then(|applied| {
            if applied {
                conn.execute(
                    "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, datetime('now'))",
                    [key],
                )?;
            }
            Ok(())
        });
        match res {
            Ok(()) => conn.execute_batch("RELEASE crm_links"),
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK TO crm_links; RELEASE crm_links");
                Err(e)
            }
        }
    };
    if let Err(e) = run() {
        eprintln!("[migrate] {key}: {e}");
    }
}

// ---- #9: which addresses can stand for an organisation ----

/// Platforms whose addresses land on unrelated venues when a crawl reads a
/// ticketing or listing page: never evidence of one organisation.
const NOT_AN_ORGANISATION: &[&str] = &[
    "skiddle.com", "booking.com", "deezer.com", "spotify.com", "linktr.ee", "beatport.com",
    "playlistlookup.com", "readymag.com", "spacebase.com", "eventbrite.com", "eventbrite.fr",
    "facebook.com", "instagram.com", "google.com", "wixpress.com", "sentry.io", "ra.co",
    "residentadvisor.net", "dice.fm", "shotgun.live", "festscanner.com", "nightlifeticket.nl",
    "onhuddle.co", "example.com", "email.com", "domain.com",
    // Media and listings: they cover many unrelated venues.
    "djmag.com", "mixmag.net", "timeout.com", "wantedinrome.com", "songkick.com",
    "bandsintown.com", "xceed.me", "fatsoma.com", "feverup.com", "partyflock.nl",
    "residentadvisor.com", "allevents.in", "eventim.com", "ticketmaster.com",
];

/// Compliance, support, automated and template mailboxes (same spirit as
/// radar.rs's list, kept local so the migration does not depend on it).
const NOT_A_PERSON_LOCALS: &[&str] = &[
    "privacy", "legal", "support", "help", "dsa", "dpo", "dpd", "gdpr", "abuse", "security",
    "noreply", "no", "donotreply", "webmaster", "postmaster", "copyright", "dmca", "compliance",
    "billing", "invoice", "unsubscribe", "custcare", "customerservice", "kunden", "your",
    "youremail", "name", "email", "example", "test", "nome", "mailer",
];

pub(crate) fn is_org_address(email: &str) -> bool {
    static SHAPE: OnceLock<Regex> = OnceLock::new();
    let e = email.trim().to_lowercase();
    if !re(&SHAPE, r"^[a-z0-9._%+\-]+@[a-z0-9\-]+(\.[a-z0-9\-]+)*\.[a-z]{2,}$").is_match(&e) {
        return false;
    }
    let (local, domain) = e.split_once('@').unwrap_or(("", ""));
    let local_root = local.split(|c| c == '.' || c == '-' || c == '_').next().unwrap_or("");
    if NOT_A_PERSON_LOCALS.contains(&local_root) {
        return false;
    }
    !NOT_AN_ORGANISATION
        .iter()
        .any(|d| domain == *d || domain.ends_with(&format!(".{d}")))
}

// ---- #15: free-text dates -> ISO ----

fn reference_day(created_at: Option<&str>) -> NaiveDate {
    created_at
        .and_then(|s| s.get(..10))
        .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
        .unwrap_or_else(|| chrono::Local::now().date_naive())
}

/// New value for a date column plus the note to keep when the original text
/// carried more than the ISO day (or could not be read at all).
pub(crate) fn convert_date(raw: &str, reference: NaiveDate, label: &str) -> (Option<String>, Option<String>) {
    let v = raw.trim();
    if v.is_empty() || is_iso_day(v) {
        return (Some(v.to_string()).filter(|s| !s.is_empty()), None);
    }
    let note = if label.is_empty() {
        format!("Date d'origine: {v}")
    } else {
        format!("Date d'origine ({label}): {v}")
    };
    match parse_loose_date(v, reference) {
        Some(d) if d.exact => (Some(d.date.format("%Y-%m-%d").to_string()), None),
        Some(d) => (Some(d.date.format("%Y-%m-%d").to_string()), Some(note)),
        None => (None, Some(note)),
    }
}

fn append_note(notes: Option<String>, extra: &[String]) -> Option<String> {
    if extra.is_empty() {
        return notes;
    }
    let add = extra.join("\n");
    match notes.filter(|n| !n.trim().is_empty()) {
        Some(n) => Some(format!("{}\n{}", n.trim_end(), add)),
        None => Some(add),
    }
}

fn migrate_dates(conn: &Connection) -> rusqlite::Result<bool> {
    type Row = (i64, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>);
    let rows: Vec<Row> = {
        let mut stmt = conn.prepare(
            "SELECT id, date, first_contact, follow_up, notes, created_at FROM contacts
             WHERE trim(COALESCE(date,'')) <> '' OR trim(COALESCE(first_contact,'')) <> ''
                OR trim(COALESCE(follow_up,'')) <> ''",
        )?;
        let it = stmt.query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))
        })?;
        it.collect::<rusqlite::Result<_>>()?
    };
    for (id, date, first, follow, notes, created) in rows {
        let reference = reference_day(created.as_deref());
        let mut extra = Vec::new();
        let mut conv = |v: &Option<String>, label: &str| -> Option<String> {
            let (val, note) = convert_date(v.as_deref().unwrap_or(""), reference, label);
            extra.extend(note);
            val
        };
        let nd = conv(&date, "");
        let nf = conv(&first, "premier contact");
        let nu = conv(&follow, "relance");
        let clean = |v: &Option<String>| v.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(String::from);
        if nd == clean(&date) && nf == clean(&first) && nu == clean(&follow) && extra.is_empty() {
            continue;
        }
        let nn = append_note(notes, &extra);
        conn.execute(
            "UPDATE contacts SET date=?2, first_contact=?3, follow_up=?4, notes=?5 WHERE id=?1",
            params![id, nd, nf, nu, nn],
        )?;
    }

    // Tasks have no notes column: an unreadable due date goes into the title.
    let today = chrono::Local::now().date_naive();
    let tasks: Vec<(i64, String, String)> = {
        let mut stmt = conn.prepare(
            "SELECT id, title, due_date FROM tasks WHERE trim(COALESCE(due_date,'')) <> ''",
        )?;
        let it = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        it.collect::<rusqlite::Result<_>>()?
    };
    for (id, title, due) in tasks {
        let (val, note) = convert_date(&due, today, "échéance");
        if val.as_deref() == Some(due.trim()) && note.is_none() {
            continue;
        }
        let title = match note {
            Some(n) => format!("{}\n{}", title.trim_end(), n),
            None => title,
        };
        conn.execute(
            "UPDATE tasks SET due_date=?2, title=?3 WHERE id=?1",
            params![id, val, title],
        )?;
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, dd: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, dd).unwrap()
    }
    fn p(s: &str) -> Option<(String, bool)> {
        to_iso(s, d(2026, 7, 25))
    }
    fn ok(s: &str, iso: &str) {
        assert_eq!(p(s), Some((iso.to_string(), true)), "{s}");
    }
    fn approx(s: &str, iso: &str) {
        assert_eq!(p(s), Some((iso.to_string(), false)), "{s}");
    }

    #[test]
    fn real_formats() {
        // Every distinct value of contacts.date in the real database.
        ok("Wed Oct 21", "2026-10-21");
        ok("Thu Oct 22", "2026-10-22");
        ok("Fri Oct 23", "2026-10-23");
        ok("Sat Oct 24", "2026-10-24");
        ok("Sun Oct 25", "2026-10-25");
        approx("Oct 21-25", "2026-10-21");
        approx("Oct 22-23", "2026-10-22");
        approx("Oct 23-24", "2026-10-23");
        assert_eq!(p("ADE"), None);
        assert_eq!(p("ADE (date TBC)"), None);
    }

    #[test]
    fn numeric_and_iso() {
        ok("2026-10-21", "2026-10-21");
        ok("2026-10-21 00:00:00", "2026-10-21");
        approx("2026-10-21 18:30", "2026-10-21");
        ok("21/10/2026", "2026-10-21");
        ok("21/10/26", "2026-10-21");
        ok("21.10.2026", "2026-10-21");
        ok("21/10", "2026-10-21");
        ok("10/21/2026", "2026-10-21"); // US order, unambiguous
        ok("2026/10/21", "2026-10-21");
        ok("46316", "2026-10-21"); // Excel serial
        assert_eq!(p("31/02/2026"), None);
        assert_eq!(p("123"), None);
    }

    #[test]
    fn words_fr_en() {
        ok("Oct 21", "2026-10-21");
        ok("October 21, 2026", "2026-10-21");
        ok("21 octobre 2026", "2026-10-21");
        ok("mercredi 21 octobre", "2026-10-21");
        ok("le 21 Octobre", "2026-10-21");
        ok("21st of October", "2026-10-21");
        ok("1er décembre", "2026-12-01");
        ok("15 août 2026", "2026-08-15");
        ok("3 févr. 2027", "2027-02-03");
        approx("21-25 Oct", "2026-10-21");
        approx("Oct 21 ADE", "2026-10-21");
        approx("Oct 22 - Nov 2", "2026-10-22");
        // Weekday that contradicts the stated year: kept but flagged.
        approx("Mon Oct 21 2026", "2026-10-21");
    }

    #[test]
    fn year_inference() {
        // Written in November about January: next year.
        assert_eq!(to_iso("Jan 15", d(2026, 11, 10)), Some(("2027-01-15".into(), true)));
        // A little in the past stays this year.
        assert_eq!(to_iso("Oct 1", d(2026, 10, 29)), Some(("2026-10-01".into(), true)));
        // Weekday picks the year: Oct 21 is a Tuesday in 2025.
        assert_eq!(to_iso("Tue Oct 21", d(2026, 7, 25)), Some(("2025-10-21".into(), true)));
    }

    #[test]
    fn convert_keeps_original() {
        let r = d(2026, 7, 25);
        assert_eq!(convert_date("Wed Oct 21", r, ""), (Some("2026-10-21".into()), None));
        assert_eq!(
            convert_date("Oct 21-25", r, ""),
            (Some("2026-10-21".into()), Some("Date d'origine: Oct 21-25".into()))
        );
        assert_eq!(
            convert_date("TBC", r, "relance"),
            (None, Some("Date d'origine (relance): TBC".into()))
        );
        assert_eq!(convert_date("2026-10-21", r, ""), (Some("2026-10-21".into()), None));
        assert_eq!(convert_date("  ", r, ""), (None, None));
    }

    #[test]
    fn org_addresses() {
        assert!(is_org_address("hello.venezia@thisiscombo.com"));
        assert!(is_org_address("press@setspace.uk"));
        assert!(!is_org_address("custcare@skiddle.com"));
        assert!(!is_org_address("privacy@deezer.com"));
        assert!(!is_org_address("support@foo.com"));
        assert!(!is_org_address("kunden.service@booking.com"));
        assert!(!is_org_address("contact@festscanner.com\\"));
        assert!(!is_org_address("a@x.com, b@y.com"));
        assert!(!is_org_address("nome.cognome@gmail.com"));
    }

    #[test]
    fn migration_on_memory_db() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);
             CREATE TABLE artists (id INTEGER PRIMARY KEY, name TEXT);
             CREATE TABLE contacts (id INTEGER PRIMARY KEY, artist_id INTEGER, name TEXT, promoter TEXT,
                email TEXT, date TEXT, first_contact TEXT, follow_up TEXT, notes TEXT,
                created_at TEXT DEFAULT '2026-07-25 18:11:27');
             CREATE TABLE tasks (id INTEGER PRIMARY KEY, title TEXT, due_date TEXT);
             CREATE TABLE events (id INTEGER PRIMARY KEY, title TEXT, date TEXT, venue TEXT, city TEXT,
                status TEXT, fee REAL);
             CREATE TABLE budget_items (id INTEGER PRIMARY KEY, category TEXT, item TEXT NOT NULL,
                min_cost REAL NOT NULL DEFAULT 0, max_cost REAL NOT NULL DEFAULT 0, actual REAL,
                kind TEXT NOT NULL DEFAULT 'expense', notes TEXT, sort INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE visa_dossiers (id INTEGER PRIMARY KEY, title TEXT);
             INSERT INTO artists VALUES (1, 'Piers');
             INSERT INTO contacts (id, name, email, date, notes) VALUES
                (1, 'A', 'hello@combo.com', 'Wed Oct 21', NULL),
                (2, 'B', 'Hello@combo.com ', 'Oct 21-25', 'keep me'),
                (3, 'C', 'custcare@skiddle.com', 'ADE (date TBC)', NULL),
                (4, 'D', 'custcare@skiddle.com', NULL, NULL),
                (5, 'E', 'solo@x.com', '2026-10-22', NULL);
             INSERT INTO tasks VALUES (1, 'T', '21/10'), (2, 'U', 'someday');
             INSERT INTO events VALUES (1, 'Gig', '2026-10-23', 'Club', 'Amsterdam', 'confirmed', 300),
                                       (2, 'Off', '2026-10-24', NULL, NULL, 'cancelled', 200);",
        )
        .unwrap();
        migrate(&conn);
        migrate(&conn); // idempotent

        // Nothing is linked silently: artist and organisation stay for the user to validate.
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM contacts WHERE artist_id IS NOT NULL", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0);
        let orgs: i64 = conn.query_row("SELECT COUNT(*) FROM organisations", [], |r| r.get(0)).unwrap();
        assert_eq!(orgs, 0);
        let get = |id: i64| -> (Option<String>, Option<String>) {
            conn.query_row("SELECT date, notes FROM contacts WHERE id=?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
        };
        assert_eq!(get(1), (Some("2026-10-21".into()), None));
        assert_eq!(get(2), (Some("2026-10-21".into()), Some("keep me\nDate d'origine: Oct 21-25".into())));
        assert_eq!(get(3), (None, Some("Date d'origine: ADE (date TBC)".into())));
        assert_eq!(get(5), (Some("2026-10-22".into()), None));
        let t2: (Option<String>, String) = conn
            .query_row("SELECT due_date, title FROM tasks WHERE id=2", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!(t2, (None, "U\nDate d'origine (échéance): someday".into()));
        // Budget: one revenue line for the confirmed gig, none for the cancelled one.
        let rev: Vec<(i64, f64)> = conn
            .prepare("SELECT event_id, actual FROM budget_items WHERE kind='revenue'")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(rev, vec![(1, 300.0)]);
        conn.execute("UPDATE events SET status='cancelled' WHERE id=1", []).unwrap();
        crate::commands::data::sync_event_budget(&conn, 1).unwrap();
        let left: i64 = conn.query_row("SELECT COUNT(*) FROM budget_items", [], |r| r.get(0)).unwrap();
        assert_eq!(left, 0);
    }
}

/// Full startup migration on a copy of a real database:
/// `PIERSCRM_DB_COPY=/path/copy.db cargo test real_db_copy -- --ignored --nocapture`
#[cfg(test)]
#[test]
#[ignore]
fn real_db_copy() {
    let Ok(path) = std::env::var("PIERSCRM_DB_COPY") else { return };
    let pool = crate::db::init_pool(std::path::Path::new(&path)).expect("init");
    let conn = pool.get().unwrap();
    for (label, sql) in [
        ("contacts with artist", "SELECT COUNT(*) FROM contacts WHERE artist_id IS NOT NULL"),
        ("contacts without artist", "SELECT COUNT(*) FROM contacts WHERE artist_id IS NULL"),
        ("organisations", "SELECT COUNT(*) FROM organisations"),
        ("contacts in an organisation", "SELECT COUNT(*) FROM contacts WHERE organisation_id IS NOT NULL"),
        ("dates ISO", "SELECT COUNT(*) FROM contacts WHERE date GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]'"),
        ("dates non-ISO left", "SELECT COUNT(*) FROM contacts WHERE trim(COALESCE(date,''))<>'' AND date NOT GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]'"),
        ("notes with original date", "SELECT COUNT(*) FROM contacts WHERE notes LIKE '%Date d''origine%'"),
        ("event budget lines", "SELECT COUNT(*) FROM budget_items WHERE event_id IS NOT NULL"),
    ] {
        let n: i64 = conn.query_row(sql, [], |r| r.get(0)).unwrap();
        println!("{label}: {n}");
    }
}
