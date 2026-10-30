//! "Bon moment pour écrire": when a venue or a promoter is worth pitching.
//!
//! Programmers book roughly two to four months ahead. Each venue / promoter
//! gets a booking lead time, inferred from RA when we saw its events announced
//! well before they happened (vi_venue_events.first_seen_at), else the 10-week
//! default. Its month profile (events per calendar month over the last 12
//! months) tells a year-round club from a seasonal one (Ibiza, open airs, ski
//! resorts): the right moment to write is when the date we would ask for
//! (today + lead time) falls in a month the room is actually programming.
//!
//! Everything here is pure (dates in, verdict out) and unit-tested; the DB side
//! stores `lead_weeks`, `lead_source` and `month_profile` (see fit::recompute).

use chrono::{Datelike, Duration, NaiveDate};
use serde::Serialize;

/// Booking lead time when nothing better is known.
pub const DEFAULT_LEAD_WEEKS: i64 = 10;
/// Bounds of an inferred lead time (a programmer rarely books under six weeks
/// out, or more than ~five months out for club nights).
pub const MIN_LEAD_WEEKS: i64 = 6;
pub const MAX_LEAD_WEEKS: i64 = 20;
/// Weeks between a booking and its RA announcement, added to an observed
/// announcement lead to get the booking lead.
const BOOK_TO_ANNOUNCE_WEEKS: i64 = 4;
/// "Soon" = the window opens within this many days.
const SOON_DAYS: i64 = 28;

/// Booking lead time in weeks and where it comes from ("observed" | "default").
/// `obs` are (event date, date we first saw it on RA). Only events seen at
/// least a week before they happened say anything about announcements; with
/// fewer than three of those the default stands.
pub fn estimate_lead(obs: &[(NaiveDate, NaiveDate)]) -> (i64, &'static str) {
    let mut leads: Vec<i64> = obs
        .iter()
        .map(|(ev, seen)| (*ev - *seen).num_days())
        .filter(|d| *d >= 7)
        .collect();
    if leads.len() < 3 {
        return (DEFAULT_LEAD_WEEKS, "default");
    }
    leads.sort_unstable();
    let mid = leads.len() / 2;
    let median = if leads.len() % 2 == 0 { (leads[mid - 1] + leads[mid]) / 2 } else { leads[mid] };
    let weeks = ((median as f64) / 7.0).round() as i64 + BOOK_TO_ANNOUNCE_WEEKS;
    (weeks.clamp(MIN_LEAD_WEEKS, MAX_LEAD_WEEKS), "observed")
}

/// Events per calendar month (index 0 = January) for the dates in
/// [today - 365 days, today].
pub fn month_profile(dates: &[NaiveDate], today: NaiveDate) -> [i64; 12] {
    let from = today - Duration::days(365);
    let mut p = [0i64; 12];
    for d in dates {
        if *d >= from && *d <= today {
            p[d.month0() as usize] += 1;
        }
    }
    p
}

/// "1,0,3,..." for storage.
pub fn profile_to_string(p: &[i64; 12]) -> String {
    p.iter().map(|n| n.to_string()).collect::<Vec<_>>().join(",")
}

pub fn profile_from_str(s: &str) -> Option<[i64; 12]> {
    let v: Vec<i64> = s.split(',').map(|x| x.trim().parse().ok()).collect::<Option<_>>()?;
    v.try_into().ok()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    /// Write now: the date we would ask for falls in a month they program.
    Now,
    /// The window opens within four weeks.
    Soon,
    /// Off season: the window opens later (write_from).
    Later,
    /// No event history in the last 12 months.
    Unknown,
}

impl State {
    #[allow(dead_code)]
    pub fn as_str(self) -> &'static str {
        match self {
            State::Now => "now",
            State::Soon => "soon",
            State::Later => "later",
            State::Unknown => "unknown",
        }
    }
    /// Send-queue priority, lower first. Unknown sits between "now" and
    /// "soon": no history is not a reason to hold a pitch back.
    pub fn rank(self) -> u8 {
        match self {
            State::Now => 0,
            State::Unknown => 1,
            State::Soon => 2,
            State::Later => 3,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Timing {
    pub state: State,
    pub lead_weeks: i64,
    /// First day it makes sense to write (today when state is now).
    pub write_from: Option<NaiveDate>,
    /// Month (1-12) of the date the pitch would be for.
    pub target_month: u32,
    /// Programs at least 9 months out of 12.
    pub year_round: bool,
}

/// Months that count as "programming": at least a third of an average month,
/// and at least one event.
fn active_months(p: &[i64; 12]) -> [bool; 12] {
    let total: i64 = p.iter().sum();
    let floor = ((total as f64) / 36.0).ceil().max(1.0) as i64;
    let mut a = [false; 12];
    for (i, n) in p.iter().enumerate() {
        a[i] = *n >= floor;
    }
    a
}

fn first_of_month(y: i32, m0: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m0 + 1, 1).expect("valid month")
}

/// When to write to a room with this month profile and lead time.
pub fn window(today: NaiveDate, lead_weeks: i64, profile: &[i64; 12]) -> Timing {
    let lead_weeks = lead_weeks.clamp(1, 52);
    let target = today + Duration::weeks(lead_weeks);
    let total: i64 = profile.iter().sum();
    let mut t = Timing {
        state: State::Unknown,
        lead_weeks,
        write_from: None,
        target_month: target.month(),
        year_round: false,
    };
    if total == 0 {
        return t;
    }
    let active = active_months(profile);
    t.year_round = active.iter().filter(|a| **a).count() >= 9;
    let m0 = target.month0();
    if active[m0 as usize] {
        t.state = State::Now;
        t.write_from = Some(today);
        return t;
    }
    // Next programming month after the target month.
    let (mut y, mut m) = (target.year(), m0);
    for _ in 0..12 {
        if m == 11 {
            m = 0;
            y += 1;
        } else {
            m += 1;
        }
        if active[m as usize] {
            let from = first_of_month(y, m) - Duration::weeks(lead_weeks);
            t.write_from = Some(from);
            t.state = if (from - today).num_days() <= SOON_DAYS { State::Soon } else { State::Later };
            return t;
        }
    }
    t
}

/// Window for a stored row (lead_weeks / month_profile columns), with the
/// defaults when the row was never computed.
pub fn window_for_row(today: NaiveDate, lead_weeks: Option<i64>, profile: Option<&str>) -> Timing {
    let p = profile.and_then(profile_from_str).unwrap_or([0; 12]);
    window(today, lead_weeks.unwrap_or(DEFAULT_LEAD_WEEKS), &p)
}

/// Soft priority for a send queue: reorders only the items `is_first` selects
/// (first emails of a sequence) among the slots they already occupy, by `key`
/// (lower first), keeping their original order on ties. Follow-ups and every
/// other item keep their exact position, so a soft priority never delays a
/// follow-up or changes how many emails go out.
pub fn reorder_firsts<T, K: Ord>(items: &mut Vec<T>, is_first: impl Fn(&T) -> bool, key: impl Fn(&T) -> K) {
    let slots: Vec<usize> = items.iter().enumerate().filter(|(_, x)| is_first(x)).map(|(i, _)| i).collect();
    if slots.len() < 2 {
        return;
    }
    let mut keyed: Vec<(K, usize)> = slots.iter().map(|&i| (key(&items[i]), i)).collect();
    // Stable on the original index.
    keyed.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    let order: Vec<usize> = keyed.into_iter().map(|(_, i)| i).collect();
    let mut taken: Vec<Option<T>> = std::mem::take(items).into_iter().map(Some).collect();
    let picked: Vec<T> = order.iter().map(|&i| taken[i].take().expect("unique index")).collect();
    let mut picked = picked.into_iter();
    let mut out = Vec::with_capacity(taken.len());
    for (i, slot) in taken.into_iter().enumerate() {
        if slots.binary_search(&i).is_ok() {
            out.push(picked.next().expect("same count"));
        } else {
            out.push(slot.expect("untouched"));
        }
    }
    *items = out;
}

/// (timing rank, -fit) for a pipeline contact: its Discovery venue or
/// promoter, if any. Contacts Discovery does not know rank as "unknown".
pub fn contact_priority(conn: &rusqlite::Connection, contact_id: i64, today: NaiveDate) -> (u8, i64) {
    let row: Option<(Option<i64>, Option<String>, Option<i64>)> = conn
        .query_row(
            "SELECT lead_weeks, month_profile, fit_score FROM vi_venues WHERE crm_contact_id = ?1
             UNION ALL
             SELECT lead_weeks, month_profile, fit_score FROM vi_promoter_orgs WHERE crm_contact_id = ?1
             LIMIT 1",
            [contact_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .ok();
    match row {
        Some((lead, profile, fit)) => {
            let t = window_for_row(today, lead, profile.as_deref());
            (t.state.rank(), -fit.unwrap_or(0))
        }
        None => (State::Unknown.rank(), 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn lead_defaults_without_announcements() {
        assert_eq!(estimate_lead(&[]), (10, "default"));
        // Seen after the fact (a harvest of past events): no signal.
        let obs = vec![(d("2026-05-01"), d("2026-06-01")), (d("2026-05-02"), d("2026-05-01"))];
        assert_eq!(estimate_lead(&obs), (10, "default"));
    }

    #[test]
    fn lead_from_observed_announcements() {
        // Announced 6, 8 and 10 weeks out: median 8 weeks + 4 to book = 12.
        let obs = vec![
            (d("2026-06-01"), d("2026-06-01") - Duration::weeks(6)),
            (d("2026-06-08"), d("2026-06-08") - Duration::weeks(8)),
            (d("2026-06-15"), d("2026-06-15") - Duration::weeks(10)),
        ];
        assert_eq!(estimate_lead(&obs), (12, "observed"));
        // Clamped to the bounds.
        let far: Vec<_> = (0..4).map(|i| (d("2027-06-01") + Duration::days(i), d("2026-01-01"))).collect();
        assert_eq!(estimate_lead(&far).0, MAX_LEAD_WEEKS);
        let near: Vec<_> = (0..4).map(|i| (d("2026-06-08") + Duration::days(i), d("2026-06-01"))).collect();
        assert_eq!(estimate_lead(&near).0, MIN_LEAD_WEEKS);
    }

    #[test]
    fn profile_round_trip_and_window() {
        let today = d("2026-10-30");
        let dates = vec![d("2026-01-10"), d("2026-01-20"), d("2025-10-31"), d("2025-10-29"), d("2026-11-01")];
        let p = month_profile(&dates, today);
        assert_eq!(p[0], 2);
        assert_eq!(p[9], 1); // 2025-10-29 is more than a year back
        assert_eq!(profile_from_str(&profile_to_string(&p)), Some(p));
        assert_eq!(profile_from_str("1,2"), None);
    }

    #[test]
    fn year_round_club_is_always_now() {
        let p = [4; 12];
        let t = window(d("2026-10-30"), 10, &p);
        assert_eq!(t.state, State::Now);
        assert!(t.year_round);
        assert_eq!(t.write_from, Some(d("2026-10-30")));
        assert_eq!(t.target_month, 1); // 10 weeks after Oct 30 = early January
    }

    #[test]
    fn seasonal_venue_waits_for_its_window() {
        // Ibiza-like: June to September only.
        let mut p = [0i64; 12];
        for m in 5..9 {
            p[m] = 20;
        }
        // Late October: next season starts June 1st, write 10 weeks before.
        let t = window(d("2026-10-30"), 10, &p);
        assert_eq!(t.state, State::Later);
        assert_eq!(t.write_from, Some(d("2027-06-01") - Duration::weeks(10)));
        assert!(!t.year_round);
        // Mid March: the window (March 23rd) opens within four weeks.
        let t = window(d("2027-03-01"), 10, &p);
        assert_eq!(t.state, State::Soon);
        // Early April: asking for mid June, in season.
        let t = window(d("2027-04-05"), 10, &p);
        assert_eq!(t.state, State::Now);
        // A closed month inside a year-round profile (August off).
        let mut q = [6i64; 12];
        q[7] = 0;
        let t = window(d("2026-06-01"), 10, &q);
        assert_eq!(t.target_month, 8);
        assert_eq!(t.state, State::Soon); // September window opens June 23rd
    }

    #[test]
    fn unknown_without_history() {
        let t = window_for_row(d("2026-10-30"), None, None);
        assert_eq!(t.state, State::Unknown);
        assert_eq!(t.lead_weeks, DEFAULT_LEAD_WEEKS);
        assert_eq!(t.write_from, None);
        assert!(State::Now.rank() < State::Unknown.rank() && State::Unknown.rank() < State::Later.rank());
    }

    #[test]
    fn reorder_only_touches_first_emails() {
        // (step, rank): follow-ups keep their slot.
        let mut v = vec![(1, 3), (2, 0), (1, 0), (3, 9), (1, 1), (1, 0)];
        reorder_firsts(&mut v, |x| x.0 == 1, |x| x.1);
        assert_eq!(v, vec![(1, 0), (2, 0), (1, 0), (3, 9), (1, 1), (1, 3)]);
        // Ties keep the original order.
        let mut w = vec![("a", 1), ("b", 1), ("c", 0)];
        reorder_firsts(&mut w, |_| true, |x| x.1);
        assert_eq!(w, vec![("c", 0), ("a", 1), ("b", 1)]);
        let mut e: Vec<(i32, i32)> = vec![];
        reorder_firsts(&mut e, |_| true, |x| x.1);
        assert!(e.is_empty());
    }
}
