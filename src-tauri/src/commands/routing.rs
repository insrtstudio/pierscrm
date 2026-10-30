//! Chaining dates and bringing clubs back.
//!
//! - Tour routing: around a gig (D-2..D+2), the days with no other date and
//!   the clubs nearby worth a pitch: CRM contacts first (warm ones on top: in
//!   discussion, replied, previously booked), then qualified Discovery venues
//!   not in the pipeline yet. Distances come from a small static table of the
//!   main European club cities (vi_venues.latitude/longitude when the harvest
//!   ever filled them, the average of a city's venues as a last resort),
//!   haversine. One click drafts "je joue à X le samedi 12, je serais dispo le
//!   vendredi 11 à Y" in Thibault's voice (Claude when a key is set, a
//!   template otherwise). Nothing is ever sent from here: the draft opens in
//!   the compose window.
//! - Rebook loop: J+2 after a confirmed gig, a thank-you that proposes another
//!   date; J+60, a reminder when nothing new was booked with that contact.
//!   "Previously booked" is derived (a confirmed gig in the past with the
//!   contact, see PREVIOUSLY_BOOKED_SQL), so it can never drift.
//! - Social proof: the night's assets on the gig (photo and video links,
//!   attendance note, a quote from the programmer), the "a joué à" list of
//!   the artist (used by the residency drafts, content.rs) and, on demand
//!   only, a press kit on insrt.studio listing those dates.
//!
//! Tables (additive, idempotent):
//! - gig_recaps    one row per gig with its assets and whether it shows in "a joué à"
//! - gig_followups (event_id, kind) handled: 'thanks', 'rebook', 'routing'

use std::collections::{HashMap, HashSet};

use chrono::{Datelike, Duration, Months, NaiveDate, Weekday};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::State;

use crate::commands::residency::claude::{self, Llm, LlmRequest, SystemBlock};
use crate::commands::residency::content::parse_draft;
use crate::commands::residency::insrt::{self, Insrt, PressKitAck, PressKitUpsert};
use crate::commands::residency::zone;
use crate::db::normalise;
use crate::AppState;

/// Days looked at on each side of a gig.
pub const WINDOW_DAYS: i64 = 2;
/// Default search radius around the gig city.
pub const DEFAULT_RADIUS_KM: f64 = 300.0;
/// Most suggestions returned for one gig.
pub const MAX_SUGGESTIONS: usize = 40;
/// Thank-you: due this many days after the gig, offered for THANKS_WINDOW days.
pub const THANKS_AFTER_DAYS: i64 = 2;
pub const THANKS_WINDOW_DAYS: i64 = 30;
/// Rebook reminder: due this many days after the gig, offered until REBOOK_WINDOW_DAYS.
pub const REBOOK_AFTER_DAYS: i64 = 60;
pub const REBOOK_WINDOW_DAYS: i64 = 180;
/// Upcoming confirmed gigs the Actions view checks for routing.
pub const ROUTING_HORIZON_DAYS: i64 = 60;
/// Setting holding the id of the "dates jouées" press kit once created.
pub const PLAYED_KIT_SETTING: &str = "played_at_press_kit_id";

/// SQL predicate: contact `c` was booked before (a confirmed gig with them,
/// already played). Reusable by targeting: such a contact is warm.
pub const PREVIOUSLY_BOOKED_SQL: &str = "EXISTS (SELECT 1 FROM events pb
     WHERE pb.contact_id = c.id AND pb.status = 'confirmed' AND pb.date < date('now'))";

pub fn migrate(conn: &Connection) {
    for stmt in [
        "CREATE TABLE IF NOT EXISTS gig_recaps (
            event_id     INTEGER PRIMARY KEY,
            photos       TEXT,
            videos       TEXT,
            attendance   TEXT,
            quote        TEXT,
            quote_author TEXT,
            played_at    INTEGER NOT NULL DEFAULT 1,
            updated_at   TEXT NOT NULL DEFAULT (datetime('now'))
        )",
        "CREATE TABLE IF NOT EXISTS gig_followups (
            event_id INTEGER NOT NULL,
            kind     TEXT NOT NULL,
            done_at  TEXT NOT NULL DEFAULT (datetime('now')),
            PRIMARY KEY (event_id, kind)
        )",
        "CREATE INDEX IF NOT EXISTS idx_events_contact ON events(contact_id)",
    ] {
        let _ = conn.execute(stmt, []);
    }
}

// ---------------- Geography (pure) ----------------

/// Main European club cities: folded name (crate::db::normalise, like
/// vi_venues.ville_norm), latitude, longitude, ISO country. Aliases share
/// coordinates. Île-de-France communes resolve to Paris (zone::PARIS_IDF).
const CITIES: &[(&str, f64, f64, &str)] = &[
    // France
    ("paris", 48.8566, 2.3522, "FR"),
    ("lyon", 45.7640, 4.8357, "FR"),
    ("marseille", 43.2965, 5.3698, "FR"),
    ("lille", 50.6292, 3.0573, "FR"),
    ("bordeaux", 44.8378, -0.5792, "FR"),
    ("toulouse", 43.6047, 1.4442, "FR"),
    ("nantes", 47.2184, -1.5536, "FR"),
    ("nice", 43.7102, 7.2620, "FR"),
    ("montpellier", 43.6108, 3.8767, "FR"),
    ("strasbourg", 48.5734, 7.7521, "FR"),
    ("rennes", 48.1173, -1.6778, "FR"),
    ("grenoble", 45.1885, 5.7245, "FR"),
    ("rouen", 49.4432, 1.0999, "FR"),
    ("caen", 49.1829, -0.3707, "FR"),
    ("dijon", 47.3220, 5.0415, "FR"),
    ("tours", 47.3941, 0.6848, "FR"),
    ("annecy", 45.8992, 6.1294, "FR"),
    ("biarritz", 43.4832, -1.5586, "FR"),
    ("metz", 49.1193, 6.1757, "FR"),
    ("nancy", 48.6921, 6.1844, "FR"),
    ("monaco", 43.7384, 7.4246, "MC"),
    // Belgium, Luxembourg
    ("bruxelles", 50.8503, 4.3517, "BE"),
    ("brussels", 50.8503, 4.3517, "BE"),
    ("brussel", 50.8503, 4.3517, "BE"),
    ("anvers", 51.2194, 4.4025, "BE"),
    ("antwerpen", 51.2194, 4.4025, "BE"),
    ("antwerp", 51.2194, 4.4025, "BE"),
    ("gand", 51.0543, 3.7174, "BE"),
    ("gent", 51.0543, 3.7174, "BE"),
    ("ghent", 51.0543, 3.7174, "BE"),
    ("liege", 50.6326, 5.5797, "BE"),
    ("namur", 50.4674, 4.8718, "BE"),
    ("charleroi", 50.4108, 4.4446, "BE"),
    ("luxembourg", 49.6116, 6.1319, "LU"),
    // Netherlands
    ("amsterdam", 52.3676, 4.9041, "NL"),
    ("rotterdam", 51.9244, 4.4777, "NL"),
    ("utrecht", 52.0907, 5.1214, "NL"),
    ("eindhoven", 51.4416, 5.4697, "NL"),
    ("denhaag", 52.0705, 4.3007, "NL"),
    ("thehague", 52.0705, 4.3007, "NL"),
    ("groningen", 53.2194, 6.5665, "NL"),
    ("nijmegen", 51.8126, 5.8372, "NL"),
    // United Kingdom, Ireland
    ("london", 51.5074, -0.1278, "GB"),
    ("londres", 51.5074, -0.1278, "GB"),
    ("manchester", 53.4808, -2.2426, "GB"),
    ("bristol", 51.4545, -2.5879, "GB"),
    ("leeds", 53.8008, -1.5491, "GB"),
    ("glasgow", 55.8642, -4.2518, "GB"),
    ("edinburgh", 55.9533, -3.1883, "GB"),
    ("liverpool", 53.4084, -2.9916, "GB"),
    ("brighton", 50.8225, -0.1372, "GB"),
    ("birmingham", 52.4862, -1.8904, "GB"),
    ("sheffield", 53.3811, -1.4701, "GB"),
    ("newcastle", 54.9783, -1.6178, "GB"),
    ("belfast", 54.5973, -5.9301, "GB"),
    ("dublin", 53.3498, -6.2603, "IE"),
    // Germany, Austria, Switzerland
    ("berlin", 52.5200, 13.4050, "DE"),
    ("hamburg", 53.5511, 9.9937, "DE"),
    ("munich", 48.1351, 11.5820, "DE"),
    ("munchen", 48.1351, 11.5820, "DE"),
    ("cologne", 50.9375, 6.9603, "DE"),
    ("koln", 50.9375, 6.9603, "DE"),
    ("frankfurt", 50.1109, 8.6821, "DE"),
    ("frankfurtammain", 50.1109, 8.6821, "DE"),
    ("leipzig", 51.3397, 12.3731, "DE"),
    ("dusseldorf", 51.2277, 6.7735, "DE"),
    ("stuttgart", 48.7758, 9.1829, "DE"),
    ("mannheim", 49.4875, 8.4660, "DE"),
    ("dresden", 51.0504, 13.7373, "DE"),
    ("offenbach", 50.0956, 8.7761, "DE"),
    ("vienna", 48.2082, 16.3738, "AT"),
    ("wien", 48.2082, 16.3738, "AT"),
    ("vienne", 48.2082, 16.3738, "AT"),
    ("zurich", 47.3769, 8.5417, "CH"),
    ("geneve", 46.2044, 6.1432, "CH"),
    ("geneva", 46.2044, 6.1432, "CH"),
    ("genf", 46.2044, 6.1432, "CH"),
    ("basel", 47.5596, 7.5886, "CH"),
    ("bale", 47.5596, 7.5886, "CH"),
    ("lausanne", 46.5197, 6.6323, "CH"),
    ("bern", 46.9480, 7.4474, "CH"),
    ("berne", 46.9480, 7.4474, "CH"),
    ("lucerne", 47.0502, 8.3093, "CH"),
    // Spain, Portugal
    ("barcelona", 41.3851, 2.1734, "ES"),
    ("barcelone", 41.3851, 2.1734, "ES"),
    ("madrid", 40.4168, -3.7038, "ES"),
    ("valencia", 39.4699, -0.3763, "ES"),
    ("ibiza", 38.9067, 1.4206, "ES"),
    ("eivissa", 38.9067, 1.4206, "ES"),
    ("bilbao", 43.2630, -2.9350, "ES"),
    ("malaga", 36.7213, -4.4214, "ES"),
    ("lisbon", 38.7223, -9.1393, "PT"),
    ("lisboa", 38.7223, -9.1393, "PT"),
    ("lisbonne", 38.7223, -9.1393, "PT"),
    ("porto", 41.1579, -8.6291, "PT"),
    // Italy
    ("milan", 45.4642, 9.1900, "IT"),
    ("milano", 45.4642, 9.1900, "IT"),
    ("rome", 41.9028, 12.4964, "IT"),
    ("roma", 41.9028, 12.4964, "IT"),
    ("turin", 45.0703, 7.6869, "IT"),
    ("torino", 45.0703, 7.6869, "IT"),
    ("bologna", 44.4949, 11.3426, "IT"),
    ("naples", 40.8518, 14.2681, "IT"),
    ("napoli", 40.8518, 14.2681, "IT"),
    ("florence", 43.7696, 11.2558, "IT"),
    ("firenze", 43.7696, 11.2558, "IT"),
    // Nordics, Baltics
    ("copenhagen", 55.6761, 12.5683, "DK"),
    ("kobenhavn", 55.6761, 12.5683, "DK"),
    ("copenhague", 55.6761, 12.5683, "DK"),
    ("stockholm", 59.3293, 18.0686, "SE"),
    ("gothenburg", 57.7089, 11.9746, "SE"),
    ("oslo", 59.9139, 10.7522, "NO"),
    ("helsinki", 60.1699, 24.9384, "FI"),
    ("tallinn", 59.4370, 24.7536, "EE"),
    ("riga", 56.9496, 24.1052, "LV"),
    ("vilnius", 54.6872, 25.2797, "LT"),
    // Central and Eastern Europe
    ("prague", 50.0755, 14.4378, "CZ"),
    ("praha", 50.0755, 14.4378, "CZ"),
    ("budapest", 47.4979, 19.0402, "HU"),
    ("warsaw", 52.2297, 21.0122, "PL"),
    ("warszawa", 52.2297, 21.0122, "PL"),
    ("varsovie", 52.2297, 21.0122, "PL"),
    ("krakow", 50.0647, 19.9450, "PL"),
    ("cracovie", 50.0647, 19.9450, "PL"),
    ("bucharest", 44.4268, 26.1025, "RO"),
    ("bucuresti", 44.4268, 26.1025, "RO"),
    ("clujnapoca", 46.7712, 23.6236, "RO"),
    ("sofia", 42.6977, 23.3219, "BG"),
    ("belgrade", 44.7866, 20.4489, "RS"),
    ("beograd", 44.7866, 20.4489, "RS"),
    ("zagreb", 45.8150, 15.9819, "HR"),
    ("ljubljana", 46.0569, 14.5058, "SI"),
    ("bratislava", 48.1486, 17.1077, "SK"),
    ("athens", 37.9838, 23.7275, "GR"),
    ("athenes", 37.9838, 23.7275, "GR"),
    ("tbilisi", 41.7151, 44.8271, "GE"),
    ("kyiv", 50.4501, 30.5234, "UA"),
];

/// Great-circle distance in km.
pub fn haversine_km(a: (f64, f64), b: (f64, f64)) -> f64 {
    let r = 6371.0_f64;
    let (la1, lo1) = (a.0.to_radians(), a.1.to_radians());
    let (la2, lo2) = (b.0.to_radians(), b.1.to_radians());
    let dla = la2 - la1;
    let dlo = lo2 - lo1;
    let h = (dla / 2.0).sin().powi(2) + la1.cos() * la2.cos() * (dlo / 2.0).sin().powi(2);
    2.0 * r * h.sqrt().asin()
}

/// Folded city key of a free-text city ("Paris 11e", "Lyon, France",
/// "Saint-Ouen (93)"): the part before a comma or parenthesis, then the first
/// word alone when the whole does not match a known city.
pub fn city_key(raw: &str) -> String {
    let head = raw.split([',', '(', '/']).next().unwrap_or("").trim();
    let whole = normalise(head);
    if city_entry(&whole).is_some() {
        return whole;
    }
    let first = head.split_whitespace().next().map(normalise).unwrap_or_default();
    if city_entry(&first).is_some() {
        return first;
    }
    whole
}

fn city_entry(key: &str) -> Option<(f64, f64, &'static str)> {
    if key.is_empty() {
        return None;
    }
    let key = if zone::PARIS_IDF.contains(&key) { "paris" } else { key };
    CITIES.iter().find(|c| c.0 == key).map(|c| (c.1, c.2, c.3))
}

/// Coordinates of a city key from the static table.
pub fn city_coords(key: &str) -> Option<(f64, f64)> {
    city_entry(key).map(|(la, lo, _)| (la, lo))
}

/// ISO country of a city key from the static table.
pub fn city_country(key: &str) -> Option<&'static str> {
    city_entry(key).map(|(_, _, c)| c)
}

/// Known city keys within `radius_km` of a point (the search area).
pub fn cities_within(center: (f64, f64), radius_km: f64) -> Vec<String> {
    let mut out: Vec<String> = CITIES
        .iter()
        .filter(|c| haversine_km(center, (c.1, c.2)) <= radius_km)
        .map(|c| c.0.to_string())
        .collect();
    if haversine_km(center, (48.8566, 2.3522)) <= radius_km {
        out.extend(zone::PARIS_IDF.iter().map(|s| s.to_string()));
    }
    out.sort();
    out.dedup();
    out
}

/// Coordinates of a city: static table, else the average of the Discovery
/// venues of that city that carry coordinates.
fn coords_with_db(conn: &Connection, key: &str, cache: &mut HashMap<String, Option<(f64, f64)>>) -> Option<(f64, f64)> {
    if key.is_empty() {
        return None;
    }
    if let Some(v) = cache.get(key) {
        return *v;
    }
    let v = city_coords(key).or_else(|| {
        conn.query_row(
            "SELECT AVG(latitude), AVG(longitude) FROM vi_venues
             WHERE ville_norm = ?1 AND latitude IS NOT NULL AND longitude IS NOT NULL",
            params![key],
            |r| Ok((r.get::<_, Option<f64>>(0)?, r.get::<_, Option<f64>>(1)?)),
        )
        .ok()
        .and_then(|(a, b)| Some((a?, b?)))
    });
    cache.insert(key.to_string(), v);
    v
}

// ---------------- Dates (pure) ----------------

const FR_DAYS: [&str; 7] = ["lundi", "mardi", "mercredi", "jeudi", "vendredi", "samedi", "dimanche"];
const EN_DAYS: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
const FR_MONTHS: [&str; 12] = [
    "janvier", "février", "mars", "avril", "mai", "juin", "juillet", "août", "septembre", "octobre", "novembre", "décembre",
];
const EN_MONTHS: [&str; 12] = [
    "January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December",
];

pub fn parse_day(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s.get(..10)?, "%Y-%m-%d").ok()
}

fn iso(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

pub fn weekday_word(d: NaiveDate, lang: &str) -> &'static str {
    let i = d.weekday().num_days_from_monday() as usize;
    if lang == "en" { EN_DAYS[i] } else { FR_DAYS[i] }
}

pub fn month_word(d: NaiveDate, lang: &str) -> &'static str {
    let i = d.month0() as usize;
    if lang == "en" { EN_MONTHS[i] } else { FR_MONTHS[i] }
}

/// "samedi 12 décembre" / "Saturday 12 December".
pub fn day_words(d: NaiveDate, lang: &str) -> String {
    format!("{} {} {}", weekday_word(d, lang), d.day(), month_word(d, lang))
}

/// "mars 2026" / "March 2026".
pub fn month_year(d: NaiveDate, lang: &str) -> String {
    format!("{} {}", month_word(d, lang), d.year())
}

/// The days around a gig (D-2..D+2, the gig day excluded) not already taken.
pub fn free_days(gig: NaiveDate, busy: &HashSet<NaiveDate>) -> Vec<NaiveDate> {
    (-WINDOW_DAYS..=WINDOW_DAYS)
        .filter(|o| *o != 0)
        .map(|o| gig + Duration::days(o))
        .filter(|d| !busy.contains(d))
        .collect()
}

/// The free day to offer first: club nights (Friday, Saturday), then
/// Thursday, then Sunday, then the rest; closest to the gig on a tie.
pub fn best_day(gig: NaiveDate, free: &[NaiveDate]) -> Option<NaiveDate> {
    let pref = |d: &NaiveDate| match d.weekday() {
        Weekday::Fri | Weekday::Sat => 0,
        Weekday::Thu => 1,
        Weekday::Sun => 2,
        _ => 3,
    };
    free.iter().copied().min_by_key(|d| (pref(d), (*d - gig).num_days().abs(), *d))
}

// ---------------- Ranking (pure) ----------------

/// Warmth tier, lower is warmer: in discussion, replied, previously booked,
/// contacted, never contacted, gone silent, then Discovery venues not in the
/// pipeline yet.
pub fn warmth(status: Option<&str>, replied: bool, booked: bool) -> u8 {
    let Some(st) = status else { return 6 };
    if st == "in_discussion" {
        0
    } else if replied {
        1
    } else if booked || st == "confirmed" {
        2
    } else if st == "contacted" || st == "followed_up" {
        3
    } else if st == "no_answer" {
        5
    } else {
        4
    }
}

pub fn warmth_label(w: u8) -> &'static str {
    match w {
        0 => "discussion",
        1 => "replied",
        2 => "booked",
        3 => "contacted",
        4 => "cold",
        5 => "silent",
        _ => "new",
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Suggestion {
    /// "contact" (in the pipeline) | "venue" (Discovery, not promoted yet)
    pub kind: String,
    pub contact_id: Option<i64>,
    pub venue_id: Option<i64>,
    pub name: String,
    pub city: String,
    pub country: Option<String>,
    pub distance_km: f64,
    pub warmth: u8,
    pub warmth_label: String,
    pub status: Option<String>,
    /// Discovery qualification score (0 when unknown).
    pub score: i64,
    pub has_email: bool,
    pub previously_booked: bool,
}

/// Warm first, then fit (qualification score, by tens), then closest.
pub fn rank(mut s: Vec<Suggestion>) -> Vec<Suggestion> {
    s.sort_by(|a, b| {
        a.warmth
            .cmp(&b.warmth)
            .then((b.score / 10).cmp(&(a.score / 10)))
            .then(a.distance_km.partial_cmp(&b.distance_km).unwrap_or(std::cmp::Ordering::Equal))
            .then(a.name.cmp(&b.name))
    });
    s
}

// ---------------- Tour plan (DB) ----------------

#[derive(Debug, Clone, Serialize)]
pub struct TourPlan {
    pub event_id: i64,
    pub title: String,
    pub venue: Option<String>,
    pub city: Option<String>,
    pub date: String,
    /// The gig city was found (static table or Discovery coordinates).
    pub located: bool,
    pub radius_km: f64,
    pub free_days: Vec<String>,
    pub best_day: Option<String>,
    pub suggestions: Vec<Suggestion>,
}

struct Gig {
    id: i64,
    artist_id: Option<i64>,
    contact_id: Option<i64>,
    title: String,
    venue: Option<String>,
    city: Option<String>,
    date: String,
}

fn load_gig(conn: &Connection, event_id: i64) -> Result<Gig, String> {
    conn.query_row(
        "SELECT id, artist_id, contact_id, title, venue, city, date FROM events WHERE id = ?1",
        params![event_id],
        |r| {
            Ok(Gig {
                id: r.get(0)?,
                artist_id: r.get(1)?,
                contact_id: r.get(2)?,
                title: r.get(3)?,
                venue: r.get(4)?,
                city: r.get(5)?,
                date: r.get(6)?,
            })
        },
    )
    .optional()
    .map_err(|e| e.to_string())?
    .ok_or_else(|| "Date introuvable.".to_string())
}

fn nonempty(s: Option<String>) -> Option<String> {
    s.map(|x| x.trim().to_string()).filter(|x| !x.is_empty())
}

pub fn tour_plan(conn: &Connection, event_id: i64, radius_km: f64) -> Result<TourPlan, String> {
    let gig = load_gig(conn, event_id)?;
    let day = parse_day(&gig.date).ok_or_else(|| "Date invalide.".to_string())?;
    let (from, to) = (iso(day - Duration::days(WINDOW_DAYS)), iso(day + Duration::days(WINDOW_DAYS)));

    // Days already taken by another date of the same artist (any artist when
    // the gig has none).
    let mut busy: HashSet<NaiveDate> = HashSet::new();
    {
        let mut st = conn
            .prepare(
                "SELECT date FROM events WHERE id <> ?1 AND status <> 'cancelled' AND date BETWEEN ?2 AND ?3
                   AND (?4 IS NULL OR artist_id IS NULL OR artist_id = ?4)",
            )
            .map_err(|e| e.to_string())?;
        let it = st
            .query_map(params![gig.id, from, format!("{}T99", to), gig.artist_id], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        for d in it.flatten() {
            if let Some(d) = parse_day(&d) {
                busy.insert(d);
            }
        }
    }
    let free = free_days(day, &busy);
    let best = best_day(day, &free);

    let mut cache: HashMap<String, Option<(f64, f64)>> = HashMap::new();
    let gig_key = gig.city.as_deref().map(city_key).unwrap_or_default();
    let center = coords_with_db(conn, &gig_key, &mut cache);
    let gig_venue_key = gig.venue.as_deref().map(normalise).unwrap_or_default();
    let mut plan = TourPlan {
        event_id: gig.id,
        title: gig.title.clone(),
        venue: gig.venue.clone(),
        city: gig.city.clone(),
        date: gig.date.clone(),
        located: center.is_some(),
        radius_km,
        free_days: free.iter().map(|d| iso(*d)).collect(),
        best_day: best.map(iso),
        suggestions: vec![],
    };
    if free.is_empty() || (center.is_none() && gig_key.is_empty()) {
        return Ok(plan);
    }

    // Distance of a candidate city, None when out of reach or unknown.
    let distance = |key: &str, own: Option<(f64, f64)>, cache: &mut HashMap<String, Option<(f64, f64)>>| -> Option<f64> {
        if !gig_key.is_empty() && key == gig_key {
            return Some(0.0);
        }
        let c = center?;
        let p = own.or_else(|| coords_with_db(conn, key, cache))?;
        let d = haversine_km(c, p);
        (d <= radius_km).then_some(d)
    };

    let mut out: Vec<Suggestion> = Vec::new();

    // Pipeline contacts, a Discovery venue linked to them lending its city,
    // coordinates and score.
    {
        let sql = format!(
            "SELECT c.id, c.name, c.venue, c.area, c.status, COALESCE(c.email, ''),
                    v.ville, v.pays, v.latitude, v.longitude, COALESCE(v.score_qualif, 0),
                    {booked},
                    EXISTS (SELECT 1 FROM email_replies r WHERE r.contact_id = c.id)
                      OR EXISTS (SELECT 1 FROM emails m WHERE m.contact_id = c.id AND m.replied_at IS NOT NULL),
                    EXISTS (SELECT 1 FROM events e WHERE e.contact_id = c.id AND e.status <> 'cancelled'
                              AND e.date BETWEEN ?1 AND ?2)
             FROM contacts c
             LEFT JOIN vi_venues v ON v.id = (SELECT id FROM vi_venues WHERE crm_contact_id = c.id LIMIT 1)
             WHERE c.status <> 'declined' AND c.id IS NOT ?3",
            booked = PREVIOUSLY_BOOKED_SQL
        );
        let mut st = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let rows = st
            .query_map(params![from, format!("{}T99", to), gig.contact_id], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, Option<String>>(6)?,
                    r.get::<_, Option<String>>(7)?,
                    r.get::<_, Option<f64>>(8)?,
                    r.get::<_, Option<f64>>(9)?,
                    r.get::<_, i64>(10)?,
                    r.get::<_, bool>(11)?,
                    r.get::<_, bool>(12)?,
                    r.get::<_, bool>(13)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        for row in rows.flatten() {
            let (id, name, venue, area, status, email, vville, vpays, lat, lng, score, booked, replied, taken) = row;
            if taken {
                continue;
            }
            let label = nonempty(venue).unwrap_or(name);
            if !gig_venue_key.is_empty() && normalise(&label) == gig_venue_key {
                continue;
            }
            let Some(city) = nonempty(area).or(nonempty(vville)) else { continue };
            let key = city_key(&city);
            let own = lat.zip(lng);
            let Some(d) = distance(&key, own, &mut cache) else { continue };
            let w = warmth(Some(&status), replied, booked);
            out.push(Suggestion {
                kind: "contact".into(),
                contact_id: Some(id),
                venue_id: None,
                name: label,
                city,
                country: nonempty(vpays).or_else(|| city_country(&key).map(String::from)),
                distance_km: (d * 10.0).round() / 10.0,
                warmth: w,
                warmth_label: warmth_label(w).into(),
                status: Some(status),
                score,
                has_email: !email.trim().is_empty(),
                previously_booked: booked,
            });
        }
    }

    // Qualified Discovery venues not in the pipeline yet, in the cities of
    // the search area (or with coordinates inside it).
    {
        let mut keys = match center {
            Some(c) => cities_within(c, radius_km),
            None => vec![],
        };
        if !gig_key.is_empty() && !keys.contains(&gig_key) {
            keys.push(gig_key.clone());
        }
        let (c_lat, c_lng) = center.unwrap_or((0.0, 0.0));
        let dlat = radius_km / 111.0;
        let dlng = radius_km / (111.0 * c_lat.to_radians().cos().max(0.2));
        let ph: Vec<String> = (0..keys.len()).map(|i| format!("?{}", i + 6)).collect();
        let sql = format!(
            "SELECT id, nom, ville, pays, latitude, longitude, score_qualif, COALESCE(ville_norm, '')
             FROM vi_venues
             WHERE statut IN ('qualifie', 'valide') AND crm_contact_id IS NULL AND ville IS NOT NULL
               AND (ville_norm IN ({in_list})
                    OR (?1 AND latitude BETWEEN ?2 AND ?3 AND longitude BETWEEN ?4 AND ?5))
             ORDER BY score_qualif DESC, nb_events_periode DESC
             LIMIT 400",
            in_list = if ph.is_empty() { "NULL".to_string() } else { ph.join(",") }
        );
        let mut args: Vec<Box<dyn rusqlite::types::ToSql>> = vec![
            Box::new(center.is_some()),
            Box::new(c_lat - dlat),
            Box::new(c_lat + dlat),
            Box::new(c_lng - dlng),
            Box::new(c_lng + dlng),
        ];
        for k in &keys {
            args.push(Box::new(k.clone()));
        }
        let refs: Vec<&dyn rusqlite::types::ToSql> = args.iter().map(|b| b.as_ref()).collect();
        let mut st = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let rows = st
            .query_map(refs.as_slice(), |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, Option<f64>>(4)?,
                    r.get::<_, Option<f64>>(5)?,
                    r.get::<_, i64>(6)?,
                    r.get::<_, String>(7)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        let known: HashSet<String> = out.iter().map(|s| normalise(&s.name)).collect();
        for (id, nom, ville, pays, lat, lng, score, vnorm) in rows.flatten() {
            let nk = normalise(&nom);
            if known.contains(&nk) || (!gig_venue_key.is_empty() && nk == gig_venue_key) {
                continue;
            }
            let key = if vnorm.is_empty() { city_key(&ville) } else { vnorm };
            let Some(d) = distance(&key, lat.zip(lng), &mut cache) else { continue };
            let w = warmth(None, false, false);
            out.push(Suggestion {
                kind: "venue".into(),
                contact_id: None,
                venue_id: Some(id),
                name: nom,
                city: ville,
                country: pays,
                distance_km: (d * 10.0).round() / 10.0,
                warmth: w,
                warmth_label: warmth_label(w).into(),
                status: None,
                score,
                has_email: false,
                previously_booked: false,
            });
        }
    }

    let mut ranked = rank(out);
    ranked.truncate(MAX_SUGGESTIONS);
    plan.suggestions = ranked;
    Ok(plan)
}

/// Upcoming confirmed gigs with room around them and somebody to pitch: one
/// line each in Agenda › Actions (until dismissed).
#[derive(Debug, Clone, Serialize)]
pub struct TourAction {
    pub event_id: i64,
    pub title: String,
    pub date: String,
    pub city: Option<String>,
    pub free_days: Vec<String>,
    pub count: usize,
    pub warm: usize,
    pub top: Vec<String>,
}

pub fn tour_actions(conn: &Connection, today: NaiveDate) -> Result<Vec<TourAction>, String> {
    let mut st = conn
        .prepare(
            "SELECT id FROM events e WHERE status = 'confirmed' AND date >= ?1 AND date <= ?2
               AND NOT EXISTS (SELECT 1 FROM gig_followups f WHERE f.event_id = e.id AND f.kind = 'routing')
             ORDER BY date LIMIT 20",
        )
        .map_err(|e| e.to_string())?;
    let ids: Vec<i64> = st
        .query_map(params![iso(today), format!("{}T99", iso(today + Duration::days(ROUTING_HORIZON_DAYS)))], |r| r.get(0))
        .map_err(|e| e.to_string())?
        .flatten()
        .collect();
    let mut out = Vec::new();
    for id in ids {
        let p = tour_plan(conn, id, DEFAULT_RADIUS_KM)?;
        if p.free_days.is_empty() || p.suggestions.is_empty() {
            continue;
        }
        out.push(TourAction {
            event_id: id,
            title: p.title,
            date: p.date,
            city: p.city,
            free_days: p.free_days,
            count: p.suggestions.len(),
            warm: p.suggestions.iter().filter(|s| s.warmth <= 2).count(),
            top: p.suggestions.iter().take(3).map(|s| s.name.clone()).collect(),
        });
    }
    Ok(out)
}

// ---------------- Drafts (Claude or template) ----------------

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PitchDraft {
    pub subject: String,
    pub body: String,
    pub lang: String,
    /// The model that wrote it, or "template".
    pub generated_by: String,
    /// Why the template was used although a key is set (Claude failed).
    pub note: Option<String>,
}

const ROUTING_SYSTEM: &str = "You write a short booking email from a DJ/producer to a club or promoter. \
It goes out under the artist's own name, written by Thibault who plays as the artist name given, and must read \
like he typed it himself between two studio sessions: warm, direct, a bit casual, confident without selling.

The kind of email is given:
- routing: he already plays a confirmed date nearby (venue, city, day given) and offers the free day given to this club.
- thanks: two days after he played at their place. Thank them for the night, one true detail if given, and propose doing it again (the month given).
- rebook: about two months after he played there, nothing new booked. A light check-in proposing a new date (the month given).

Rules:
- 50 to 110 words in the body. Plain text, short paragraphs, no markdown, no emojis, no lists.
- Open with \"Hello,\" (or \"Hello <first name>,\" when a first name is given). Close with \"À bientôt,\" / \"Cheers,\" then \"Thibault\" \
on its own line (the full signature is appended automatically, never write it).
- Use only the facts provided: dates, places, past gigs, quotes. Never invent a fact, a name, a number or a fee. \
For routing, say the confirmed date plainly (\"Je joue à <city> le samedi 12, je serais dispo le vendredi 11\").
- When the contact booked him before, say it simply, it is the reason for the email.
- End with one simple, relaxed question.
- Sound human: vary sentence length, spoken turns, contractions in English. At most one exclamation mark.
- Never use: \"Je me permets\", \"N'hésitez pas\", \"correspond parfaitement\", \"passionné\", \"univers\", \"synergie\", \
\"je serais ravi de\", \"Cordialement\", \"I hope this email finds you well\", \"I'm reaching out\", \"excited\", \"thrilled\", \
\"passionate\", \"vibrant\", \"elevate\", \"journey\", \"unique opportunity\". No em dashes or en dashes.
- Subject: under 60 characters, lowercase feel. No clickbait.
- Write in the language requested.

Answer with JSON only: {\"subject\": \"...\", \"body\": \"...\"}.";

fn draft_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {"subject": {"type": "string"}, "body": {"type": "string"}},
        "required": ["subject", "body"],
        "additionalProperties": false
    })
}

/// Everything a routing / thanks / rebook email may say.
#[derive(Debug, Clone, Default)]
pub struct DraftFacts {
    /// "routing" | "thanks" | "rebook"
    pub kind: String,
    pub lang: String,
    pub artist: String,
    pub first_name: Option<String>,
    /// The club written to.
    pub target: String,
    pub target_city: String,
    /// The confirmed gig the email builds on.
    pub gig_venue: String,
    pub gig_city: String,
    pub gig_day: Option<NaiveDate>,
    /// routing: the free day offered.
    pub offer_day: Option<NaiveDate>,
    /// thanks / rebook: the month proposed for the next date.
    pub next_month: Option<NaiveDate>,
    pub previously_booked: bool,
    /// "Badaboum, Paris (March 2026)" lines.
    pub played_at: Vec<String>,
    pub attendance: Option<String>,
    pub quote: Option<String>,
}

impl DraftFacts {
    fn prompt(&self) -> String {
        let mut f = vec![
            format!("Kind: {}", self.kind),
            format!("Language: {}", if self.lang == "en" { "English" } else { "French" }),
            format!("Artist name: {}", self.artist),
            format!("Contact first name: {}", self.first_name.clone().unwrap_or_else(|| "unknown".into())),
            format!("Club written to: {}{}", self.target, if self.target_city.is_empty() { String::new() } else { format!(" ({})", self.target_city) }),
        ];
        if let Some(d) = self.gig_day {
            let place = [self.gig_venue.as_str(), self.gig_city.as_str()].iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join(", ");
            f.push(format!("Confirmed gig: {} on {} ({})", place, day_words(d, &self.lang), iso(d)));
        }
        if let Some(d) = self.offer_day {
            f.push(format!("Free day offered: {} ({})", day_words(d, &self.lang), iso(d)));
        }
        if let Some(d) = self.next_month {
            f.push(format!("Month proposed for the next date: {}", month_word(d, &self.lang)));
        }
        f.push(format!("Booked him before: {}", if self.previously_booked { "yes" } else { "no" }));
        if !self.played_at.is_empty() {
            f.push(format!("Clubs he played recently: {}", self.played_at.join("; ")));
        }
        if let Some(a) = &self.attendance {
            f.push(format!("Note about the night (from Thibault): {}", a));
        }
        if let Some(q) = &self.quote {
            f.push(format!("What the programmer said after the night: {}", q));
        }
        f.join("\n")
    }

    /// The template version (no API key, or Claude failed).
    pub fn template(&self) -> (String, String) {
        let en = self.lang == "en";
        let hello = match &self.first_name {
            Some(n) => format!("Hello {},", n),
            None => "Hello,".to_string(),
        };
        let close = if en { "Cheers,\nThibault" } else { "À bientôt,\nThibault" };
        match self.kind.as_str() {
            "thanks" => {
                let when = self.gig_day.map(|d| weekday_word(d, &self.lang).to_string()).unwrap_or_default();
                let month = self.next_month.map(|d| month_word(d, &self.lang).to_string()).unwrap_or_default();
                if en {
                    (
                        format!("thanks for {}", if when.is_empty() { "the other night".to_string() } else { when.clone() }),
                        format!(
                            "{hello}\n\nThanks again for {} at {}, I had a really good night.\n\nShall we do it again{}?\n\n{close}",
                            if when.is_empty() { "the other night".to_string() } else { when },
                            self.target,
                            if month.is_empty() { String::new() } else { format!(" in {}", month) },
                        ),
                    )
                } else {
                    (
                        format!("merci pour {}", if when.is_empty() { "l'autre soir".to_string() } else { when.clone() }),
                        format!(
                            "{hello}\n\nMerci encore pour {} à {}, j'ai passé une super soirée.\n\nOn se refait ça{} ?\n\n{close}",
                            if when.is_empty() { "l'autre soir".to_string() } else { when },
                            self.target,
                            if month.is_empty() { String::new() } else { format!(" en {}", month) },
                        ),
                    )
                }
            }
            "rebook" => {
                let month = self.next_month.map(|d| month_word(d, &self.lang).to_string()).unwrap_or_default();
                let when = self.gig_day.map(|d| month_word(d, &self.lang).to_string()).unwrap_or_default();
                if en {
                    (
                        format!("{}, round two?", self.target),
                        format!(
                            "{hello}\n\nI still think about {} at {}. I'm planning my next dates{} and I'd love to come back.\n\nWould you have a slot?\n\n{close}",
                            if when.is_empty() { "the night".to_string() } else { format!("that night in {}", when) },
                            self.target,
                            if month.is_empty() { String::new() } else { format!(" for {}", month) },
                        ),
                    )
                } else {
                    (
                        format!("{}, on remet ça ?", self.target),
                        format!(
                            "{hello}\n\nJe repense souvent à {} chez vous à {}. Je regarde mes prochaines dates{} et j'aimerais bien repasser.\n\nVous auriez un créneau ?\n\n{close}",
                            if when.is_empty() { "la soirée".to_string() } else { format!("la soirée de {}", when) },
                            self.target,
                            if month.is_empty() { String::new() } else { format!(" pour {}", month) },
                        ),
                    )
                }
            }
            _ => {
                let gig_place = if self.gig_city.is_empty() { self.gig_venue.clone() } else { self.gig_city.clone() };
                let gig_day = self.gig_day.map(|d| day_words(d, &self.lang)).unwrap_or_default();
                let offer = self.offer_day.map(|d| day_words(d, &self.lang)).unwrap_or_default();
                let same_city = !self.target_city.is_empty() && city_key(&self.target_city) == city_key(&self.gig_city);
                let there = if same_city || self.target_city.is_empty() { self.target.clone() } else { self.target_city.clone() };
                let short_offer = self.offer_day.map(|d| format!("{} {}", weekday_word(d, &self.lang), d.day())).unwrap_or_default();
                if en {
                    let warm = if self.previously_booked {
                        "I loved playing at yours last time, it'd be great to do it again.\n\n".to_string()
                    } else if let Some(p) = self.played_at.first() {
                        format!("Lately I've played {}.\n\n", p)
                    } else {
                        String::new()
                    };
                    (
                        format!("{} x {}, free on {}", self.target, self.artist, short_offer),
                        format!(
                            "{hello}\n\nThibault here, I play as {} (house, tech house). I'm playing {} on {}, and I'm free on {} in {} if you're looking for someone that night.\n\n{warm}Happy to send a mix and the tech rider if it fits your programming. Would that work for you?\n\n{close}",
                            self.artist, gig_place, gig_day, offer, there,
                        ),
                    )
                } else {
                    let warm = if self.previously_booked {
                        "J'avais vraiment kiffé jouer chez vous, ce serait cool de remettre ça.\n\n".to_string()
                    } else if let Some(p) = self.played_at.first() {
                        format!("Dernièrement j'ai joué à {}.\n\n", p)
                    } else {
                        String::new()
                    };
                    (
                        format!("{} x {}, dispo le {}", self.target, self.artist, short_offer),
                        format!(
                            "{hello}\n\nMoi c'est Thibault, je mixe sous le nom de {} (house, tech house). Je joue à {} le {}, et je serais dispo le {} à {} si jamais vous cherchez quelqu'un ce soir-là.\n\n{warm}Je vous envoie un mix et la fiche technique si ça colle avec votre prog. Ça vous dirait ?\n\n{close}",
                            self.artist, gig_place, gig_day, offer, there,
                        ),
                    )
                }
            }
        }
    }
}

/// Claude when a client is given, the template otherwise or on failure.
/// The draft is only returned: it opens in the compose window, never sent.
pub fn write_draft(conn: &Connection, llm: Option<&dyn Llm>, model: &str, facts: &DraftFacts) -> PitchDraft {
    let (t_subject, t_body) = facts.template();
    let fallback = |note: Option<String>| PitchDraft {
        subject: claude::no_dashes(&t_subject),
        body: claude::no_dashes(&t_body),
        lang: facts.lang.clone(),
        generated_by: "template".into(),
        note,
    };
    let Some(llm) = llm else { return fallback(None) };
    let req = LlmRequest {
        model: model.to_string(),
        max_tokens: 350,
        system: vec![SystemBlock { text: ROUTING_SYSTEM.into(), cache: true }],
        user: facts.prompt(),
        schema: Some(draft_schema()),
    };
    let mut last = String::new();
    for _ in 0..2 {
        match llm.complete(&req) {
            Ok(resp) => {
                claude::log_usage(conn, "routing", None, &resp.model, &resp.usage);
                if let Some((subject, body)) = parse_draft(&resp.text) {
                    return PitchDraft {
                        subject,
                        body,
                        lang: facts.lang.clone(),
                        generated_by: if resp.model.is_empty() { model.to_string() } else { resp.model },
                        note: None,
                    };
                }
                last = "réponse inattendue".into();
            }
            Err(e) => {
                last = e;
                break;
            }
        }
    }
    fallback(Some(format!("IA indisponible, modèle utilisé : {}", last)))
}

fn artist_of(conn: &Connection, artist_id: Option<i64>) -> String {
    artist_id
        .and_then(|id| conn.query_row("SELECT name FROM artists WHERE id = ?1", params![id], |r| r.get::<_, String>(0)).ok())
        .or_else(|| crate::commands::email::sole_artist(conn))
        .unwrap_or_else(|| "Piers".into())
}

fn lang_of(country: Option<&str>, city: &str) -> String {
    let key = city_key(city);
    let c = country.map(String::from).or_else(|| city_country(&key).map(String::from));
    match c {
        Some(c) => zone::language_for(&c, Some(city), "auto").to_string(),
        None => "fr".into(),
    }
}

/// The routing pitch to one suggestion (a pipeline contact or a Discovery venue).
pub fn routing_facts(
    conn: &Connection,
    event_id: i64,
    contact_id: Option<i64>,
    venue_id: Option<i64>,
    day: &str,
) -> Result<DraftFacts, String> {
    let gig = load_gig(conn, event_id)?;
    let offer = parse_day(day).ok_or_else(|| "Jour invalide.".to_string())?;
    let (target, city, country, first, booked) = if let Some(cid) = contact_id {
        let sql = format!(
            "SELECT COALESCE(NULLIF(trim(c.venue), ''), c.name), COALESCE(NULLIF(trim(c.area), ''), v.ville, ''), v.pays,
                    c.promoter, {booked}
             FROM contacts c
             LEFT JOIN vi_venues v ON v.id = (SELECT id FROM vi_venues WHERE crm_contact_id = c.id LIMIT 1)
             WHERE c.id = ?1",
            booked = PREVIOUSLY_BOOKED_SQL
        );
        conn.query_row(&sql, params![cid], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, bool>(4)?,
            ))
        })
        .optional()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Contact introuvable.".to_string())?
    } else if let Some(vid) = venue_id {
        conn.query_row(
            "SELECT nom, COALESCE(ville, ''), pays FROM vi_venues WHERE id = ?1",
            params![vid],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?, None, false)),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Lieu introuvable.".to_string())?
    } else {
        return Err("Contact ou lieu manquant.".into());
    };
    let lang = lang_of(country.as_deref(), &city);
    let played = played_at(conn, gig.artist_id, chrono::Local::now().date_naive())
        .unwrap_or_default()
        .into_iter()
        .filter(|p| normalise(&p.venue) != normalise(&target))
        .take(3)
        .map(|p| p.fact(&lang))
        .collect();
    Ok(DraftFacts {
        kind: "routing".into(),
        lang,
        artist: artist_of(conn, gig.artist_id),
        first_name: crate::commands::residency::targeting::first_name_of(first.as_deref()),
        target,
        target_city: city,
        gig_venue: gig.venue.unwrap_or_default(),
        gig_city: gig.city.unwrap_or_default(),
        gig_day: parse_day(&gig.date),
        offer_day: Some(offer),
        next_month: None,
        previously_booked: booked,
        played_at: played,
        attendance: None,
        quote: None,
    })
}

// ---------------- Rebook loop ----------------

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct GigFollowup {
    /// "thanks" (J+2) | "rebook" (J+60)
    pub kind: String,
    pub event_id: i64,
    pub contact_id: i64,
    pub contact_name: String,
    pub title: String,
    pub venue: Option<String>,
    pub city: Option<String>,
    pub gig_date: String,
    /// When it became due (YYYY-MM-DD).
    pub due: String,
}

/// Thank-you and rebook reminders due on `today`.
/// - thanks: a confirmed gig with a contact, played 2 to 30 days ago, no email
///   sent to that contact since the gig, not marked handled.
/// - rebook: played 60 to 180 days ago, no later date with that contact (nor at
///   that venue), contact not declined, not marked handled.
pub fn due_followups(conn: &Connection, today: NaiveDate) -> Result<Vec<GigFollowup>, String> {
    let mut out = Vec::new();
    let base = "SELECT e.id, e.contact_id, c.name, e.title, e.venue, e.city, e.date
         FROM events e JOIN contacts c ON c.id = e.contact_id
         WHERE e.status = 'confirmed' AND e.date <= ?1 AND e.date >= ?2
           AND NOT EXISTS (SELECT 1 FROM gig_followups f WHERE f.event_id = e.id AND f.kind = ?3)";
    let thanks = format!(
        "{base} AND NOT EXISTS (SELECT 1 FROM emails m WHERE m.status = 'sent' AND date(m.sent_at) > substr(e.date, 1, 10)
               AND (m.contact_id = c.id OR (c.email IS NOT NULL AND trim(c.email) <> ''
                    AND lower(trim(m.to_addr)) = lower(trim(c.email)))))
         ORDER BY e.date"
    );
    let rebook = format!(
        "{base} AND c.status <> 'declined'
           AND NOT EXISTS (SELECT 1 FROM events x WHERE x.id <> e.id AND x.status <> 'cancelled' AND x.date > e.date
               AND (x.contact_id = e.contact_id
                    OR (trim(COALESCE(e.venue, '')) <> '' AND lower(trim(x.venue)) = lower(trim(e.venue)))))
         ORDER BY e.date"
    );
    for (kind, sql, after, window) in [
        ("thanks", thanks, THANKS_AFTER_DAYS, THANKS_WINDOW_DAYS),
        ("rebook", rebook, REBOOK_AFTER_DAYS, REBOOK_WINDOW_DAYS),
    ] {
        let mut st = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let latest = format!("{}T99", iso(today - Duration::days(after)));
        let earliest = iso(today - Duration::days(window));
        let rows = st
            .query_map(params![latest, earliest, kind], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, Option<String>>(5)?,
                    r.get::<_, String>(6)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        for (event_id, contact_id, contact_name, title, venue, city, date) in rows.flatten() {
            let due = parse_day(&date).map(|d| iso(d + Duration::days(after))).unwrap_or_else(|| iso(today));
            out.push(GigFollowup { kind: kind.into(), event_id, contact_id, contact_name, title, venue, city, gig_date: date, due });
        }
    }
    Ok(out)
}

pub fn mark_followup(conn: &Connection, event_id: i64, kind: &str) -> Result<(), String> {
    if !["thanks", "rebook", "routing"].contains(&kind) {
        return Err(format!("unknown kind: {kind}"));
    }
    conn.execute(
        "INSERT OR IGNORE INTO gig_followups (event_id, kind) VALUES (?1, ?2)",
        params![event_id, kind],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn add_months(d: NaiveDate, n: u32) -> NaiveDate {
    d.checked_add_months(Months::new(n)).unwrap_or(d)
}

pub fn followup_facts(conn: &Connection, event_id: i64, kind: &str, today: NaiveDate) -> Result<DraftFacts, String> {
    let gig = load_gig(conn, event_id)?;
    let cid = gig.contact_id.ok_or_else(|| "Aucun contact lié à cette date.".to_string())?;
    let (target, area, promoter, country): (String, String, Option<String>, Option<String>) = conn
        .query_row(
            "SELECT COALESCE(NULLIF(trim(c.venue), ''), c.name), COALESCE(c.area, ''), c.promoter,
                    (SELECT pays FROM vi_venues WHERE crm_contact_id = c.id LIMIT 1)
             FROM contacts c WHERE c.id = ?1",
            params![cid],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .map_err(|e| e.to_string())?;
    let recap = recap_get(conn, event_id)?;
    let gig_day = parse_day(&gig.date);
    let city = gig.city.clone().filter(|c| !c.trim().is_empty()).unwrap_or(area);
    let lang = lang_of(country.as_deref(), &city);
    let next_month = match kind {
        "thanks" => gig_day.map(|d| add_months(d, 4)),
        _ => Some(add_months(today, 2)),
    };
    let target = gig.venue.clone().filter(|v| !v.trim().is_empty()).unwrap_or(target);
    Ok(DraftFacts {
        kind: if kind == "rebook" { "rebook".into() } else { "thanks".into() },
        lang,
        artist: artist_of(conn, gig.artist_id),
        first_name: crate::commands::residency::targeting::first_name_of(promoter.as_deref()),
        target: target.clone(),
        target_city: city.clone(),
        gig_venue: target,
        gig_city: city,
        gig_day,
        offer_day: None,
        next_month,
        previously_booked: true,
        played_at: vec![],
        attendance: Some(recap.attendance.clone()).filter(|s| !s.trim().is_empty()),
        quote: Some(recap.quote.clone()).filter(|s| !s.trim().is_empty()),
    })
}

// ---------------- Social proof ----------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct GigRecap {
    pub event_id: i64,
    /// Links, one per line.
    #[serde(default)]
    pub photos: String,
    #[serde(default)]
    pub videos: String,
    #[serde(default)]
    pub attendance: String,
    #[serde(default)]
    pub quote: String,
    #[serde(default)]
    pub quote_author: String,
    /// Listed in "a joué à" (on by default).
    #[serde(default = "yes")]
    pub played_at: bool,
    #[serde(default)]
    pub updated_at: Option<String>,
}

fn yes() -> bool {
    true
}

pub fn recap_get(conn: &Connection, event_id: i64) -> Result<GigRecap, String> {
    Ok(conn
        .query_row(
            "SELECT photos, videos, attendance, quote, quote_author, played_at, updated_at FROM gig_recaps WHERE event_id = ?1",
            params![event_id],
            |r| {
                Ok(GigRecap {
                    event_id,
                    photos: r.get::<_, Option<String>>(0)?.unwrap_or_default(),
                    videos: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    attendance: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    quote: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                    quote_author: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
                    played_at: r.get::<_, i64>(5)? != 0,
                    updated_at: r.get(6)?,
                })
            },
        )
        .optional()
        .map_err(|e| e.to_string())?
        .unwrap_or(GigRecap { event_id, played_at: true, ..Default::default() }))
}

/// Links typed one per line (or separated by spaces): only http(s) kept.
pub fn clean_links(raw: &str) -> String {
    raw.split_whitespace()
        .map(|s| s.trim_matches(|c| c == ',' || c == ';'))
        .filter(|s| s.starts_with("http://") || s.starts_with("https://"))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn recap_save(conn: &Connection, r: &GigRecap) -> Result<GigRecap, String> {
    conn.execute(
        "INSERT INTO gig_recaps (event_id, photos, videos, attendance, quote, quote_author, played_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, datetime('now'))
         ON CONFLICT(event_id) DO UPDATE SET photos = excluded.photos, videos = excluded.videos,
            attendance = excluded.attendance, quote = excluded.quote, quote_author = excluded.quote_author,
            played_at = excluded.played_at, updated_at = excluded.updated_at",
        params![
            r.event_id,
            clean_links(&r.photos),
            clean_links(&r.videos),
            r.attendance.trim(),
            claude::no_dashes(r.quote.trim()),
            r.quote_author.trim(),
            r.played_at as i64
        ],
    )
    .map_err(|e| e.to_string())?;
    recap_get(conn, r.event_id)
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PlayedAt {
    pub event_id: i64,
    pub venue: String,
    pub city: Option<String>,
    pub date: String,
    /// "a joué à Badaboum, Paris (mars 2026)"
    pub line_fr: String,
    /// "played at Badaboum, Paris (March 2026)"
    pub line_en: String,
    pub quote: Option<String>,
    pub quote_author: Option<String>,
    pub videos: Vec<String>,
    pub photos: Vec<String>,
}

impl PlayedAt {
    /// "Badaboum, Paris (mars 2026)", the bare fact for prompts.
    pub fn fact(&self, lang: &str) -> String {
        place_when(&self.venue, self.city.as_deref(), &self.date, lang)
    }
}

fn place_when(venue: &str, city: Option<&str>, date: &str, lang: &str) -> String {
    let place = match city.map(str::trim).filter(|c| !c.is_empty()) {
        Some(c) => format!("{}, {}", venue.trim(), c),
        None => venue.trim().to_string(),
    };
    match parse_day(date) {
        Some(d) => format!("{} ({})", place, month_year(d, lang)),
        None => place,
    }
}

/// "a joué à <venue>, <city> (<mois année>)" / "played at ...".
pub fn played_at_line(venue: &str, city: Option<&str>, date: &str, lang: &str) -> String {
    let pw = place_when(venue, city, date, lang);
    if lang == "en" { format!("played at {}", pw) } else { format!("a joué à {}", pw) }
}

/// The clubs the artist played: confirmed gigs already past, with a venue,
/// not hidden from the list, latest first, one line per venue (its latest
/// date). Gigs without an artist count for the artist given.
pub fn played_at(conn: &Connection, artist_id: Option<i64>, today: NaiveDate) -> Result<Vec<PlayedAt>, String> {
    let mut st = conn
        .prepare(
            "SELECT e.id, e.venue, e.city, e.date, r.quote, r.quote_author, r.videos, r.photos
             FROM events e LEFT JOIN gig_recaps r ON r.event_id = e.id
             WHERE e.status = 'confirmed' AND e.date < ?1 AND trim(COALESCE(e.venue, '')) <> ''
               AND COALESCE(r.played_at, 1) = 1
               AND (?2 IS NULL OR e.artist_id IS NULL OR e.artist_id = ?2)
             ORDER BY e.date DESC",
        )
        .map_err(|e| e.to_string())?;
    let rows = st
        .query_map(params![iso(today), artist_id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, Option<String>>(6)?,
                r.get::<_, Option<String>>(7)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let lines = |s: Option<String>| s.unwrap_or_default().lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect::<Vec<_>>();
    for (event_id, venue, city, date, quote, author, videos, photos) in rows.flatten() {
        if !seen.insert(normalise(&venue)) {
            continue;
        }
        out.push(PlayedAt {
            event_id,
            line_fr: played_at_line(&venue, city.as_deref(), &date, "fr"),
            line_en: played_at_line(&venue, city.as_deref(), &date, "en"),
            venue: venue.trim().to_string(),
            city,
            date,
            quote: quote.filter(|q| !q.trim().is_empty()),
            quote_author: author.filter(|a| !a.trim().is_empty()),
            videos: lines(videos),
            photos: lines(photos),
        });
    }
    Ok(out)
}

/// The facts residency drafts may cite (content.rs): the campaign artist's
/// latest clubs, in English like the rest of the prompt.
pub fn played_at_facts_for_campaign(conn: &Connection, campaign_id: i64) -> Vec<String> {
    let artist_id: Option<i64> = conn
        .query_row("SELECT artist_id FROM campaigns WHERE id = ?1", params![campaign_id], |r| r.get(0))
        .ok()
        .flatten();
    played_at(conn, artist_id, chrono::Local::now().date_naive())
        .unwrap_or_default()
        .iter()
        .take(6)
        .map(|p| p.fact("en"))
        .collect()
}

/// The content of the "dates jouées" press kit (insrt back-office shape:
/// headline, pitch in RichText, concept, stats, photos; every text {fr, en}).
/// Photos stay empty: insrt then shows the artist's EPK photos (photo links
/// from the CRM are not insrt media keys, they go in the text instead).
pub fn played_at_kit_content(artist: &str, items: &[PlayedAt]) -> serde_json::Value {
    let para = |lang: &str| -> String {
        let mut p: Vec<String> = items
            .iter()
            .map(|i| {
                let fact = i.fact(lang);
                // **Venue**, City (month year)
                match fact.split_once(", ") {
                    Some((v, rest)) if v == i.venue => format!("**{}**, {}", v, rest),
                    _ => fact,
                }
            })
            .collect();
        for i in items {
            if let Some(q) = &i.quote {
                let who = i.quote_author.clone().map(|a| format!(", {}", a)).unwrap_or_default();
                p.push(if lang == "en" {
                    format!("*\u{201c}{}\u{201d}*{} ({})", q.trim(), who, i.venue)
                } else {
                    format!("*« {} »*{} ({})", q.trim(), who, i.venue)
                });
            }
        }
        for i in items {
            for (n, v) in i.videos.iter().enumerate() {
                let label = if lang == "en" { "Video" } else { "Vidéo" };
                let suffix = if i.videos.len() > 1 { format!(" {}", n + 1) } else { String::new() };
                p.push(format!("[{} {}{}]({})", label, i.venue, suffix, v));
            }
            for (n, ph) in i.photos.iter().enumerate() {
                let suffix = if i.photos.len() > 1 { format!(" {}", n + 1) } else { String::new() };
                p.push(format!("[Photos {}{}]({})", i.venue, suffix, ph));
            }
        }
        p.join("\n\n")
    };
    let cities: HashSet<String> = items.iter().filter_map(|i| i.city.as_deref().map(city_key)).filter(|c| !c.is_empty()).collect();
    let mut stats = vec![json!({"value": items.len().to_string(), "label": {"fr": "dates jouées", "en": "gigs played"}})];
    if !cities.is_empty() {
        stats.push(json!({"value": cities.len().to_string(), "label": {"fr": "villes", "en": "cities"}}));
    }
    let empty = json!({"fr": "", "en": ""});
    json!({
        "headline": {"fr": format!("{} a joué à", artist), "en": format!("{} played at", artist)},
        "pitch": {"fr": para("fr"), "en": para("en")},
        "concept": {"format": empty, "frequency": empty, "contentPlan": empty},
        "stats": stats,
        "photos": [],
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct PushResult {
    pub id: String,
    pub title: String,
    pub created: bool,
    pub count: usize,
}

/// Write the "dates jouées" kit on insrt (only on the user's click). Its own
/// kit, so the residency kits written in the back office are never replaced;
/// its id is kept to update the same one next time.
pub fn push_played_at(conn: &Connection, client: &dyn Insrt, artist_id: Option<i64>, today: NaiveDate) -> Result<PushResult, String> {
    let items = played_at(conn, artist_id, today)?;
    if items.is_empty() {
        return Err("Aucune date jouée à publier (dates confirmées passées, avec un lieu).".into());
    }
    let artist = artist_of(conn, artist_id);
    let key = match artist_id {
        Some(a) => format!("{}:{}", PLAYED_KIT_SETTING, a),
        None => PLAYED_KIT_SETTING.to_string(),
    };
    let id: Option<String> = conn
        .query_row("SELECT value FROM settings WHERE key = ?1", params![key], |r| r.get(0))
        .optional()
        .map_err(|e| e.to_string())?
        .flatten()
        .filter(|v: &String| !v.trim().is_empty());
    let kit = PressKitUpsert {
        id,
        artist_slug: insrt::slug(&artist),
        title: format!("{} · dates jouées", artist),
        content: played_at_kit_content(&artist, &items),
    };
    let ack: PressKitAck = client.upsert_press_kit(&kit)?;
    conn.execute(
        "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
        params![key, ack.id],
    )
    .map_err(|e| e.to_string())?;
    Ok(PushResult { id: ack.id, title: if ack.title.is_empty() { kit.title } else { ack.title }, created: ack.created, count: items.len() })
}

// ---------------- Tauri commands ----------------

fn today() -> NaiveDate {
    chrono::Local::now().date_naive()
}

#[tauri::command(async)]
pub fn routing_tour_plan(state: State<'_, AppState>, event_id: i64, radius_km: Option<f64>) -> Result<TourPlan, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let r = radius_km.filter(|r| *r > 0.0 && *r <= 2000.0).unwrap_or(DEFAULT_RADIUS_KM);
    tour_plan(&conn, event_id, r)
}

#[tauri::command(async)]
pub fn routing_tour_actions(state: State<'_, AppState>) -> Result<Vec<TourAction>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    tour_actions(&conn, today())
}

/// Draft the routing pitch (never sent: it opens in the compose window).
#[tauri::command]
pub async fn routing_pitch_draft(
    state: State<'_, AppState>,
    event_id: i64,
    contact_id: Option<i64>,
    venue_id: Option<i64>,
    day: String,
) -> Result<PitchDraft, String> {
    let pool = state.pool.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let conn = pool.get().map_err(|e| e.to_string())?;
        let facts = routing_facts(&conn, event_id, contact_id, venue_id, &day)?;
        let llm = claude::client(&conn);
        let model = claude::drafting_model(&conn);
        Ok(write_draft(&conn, llm.as_deref(), &model, &facts))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(async)]
pub fn routing_followups(state: State<'_, AppState>) -> Result<Vec<GigFollowup>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    due_followups(&conn, today())
}

/// Draft the thank-you (kind "thanks") or the rebook reminder ("rebook").
#[tauri::command]
pub async fn routing_followup_draft(state: State<'_, AppState>, event_id: i64, kind: String) -> Result<PitchDraft, String> {
    let pool = state.pool.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let conn = pool.get().map_err(|e| e.to_string())?;
        let facts = followup_facts(&conn, event_id, &kind, today())?;
        let llm = claude::client(&conn);
        let model = claude::drafting_model(&conn);
        Ok(write_draft(&conn, llm.as_deref(), &model, &facts))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Mark a thank-you / rebook / routing line handled (sent, or dismissed).
#[tauri::command(async)]
pub fn routing_followup_done(state: State<'_, AppState>, event_id: i64, kind: String) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    mark_followup(&conn, event_id, &kind)
}

#[tauri::command(async)]
pub fn gig_recap_get(state: State<'_, AppState>, event_id: i64) -> Result<GigRecap, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    recap_get(&conn, event_id)
}

#[tauri::command(async)]
pub fn gig_recap_save(state: State<'_, AppState>, recap: GigRecap) -> Result<GigRecap, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    recap_save(&conn, &recap)
}

#[tauri::command(async)]
pub fn played_at_list(state: State<'_, AppState>, artist_id: Option<i64>) -> Result<Vec<PlayedAt>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    played_at(&conn, artist_id, today())
}

/// "Mettre à jour le dossier presse": the only path that sends the list to insrt.
#[tauri::command]
pub async fn played_at_push(state: State<'_, AppState>, artist_id: Option<i64>) -> Result<PushResult, String> {
    let pool = state.pool.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let conn = pool.get().map_err(|e| e.to_string())?;
        let client = insrt::client(&conn).ok_or_else(|| "Jeton insrt non configuré (Réglages, Intégrations).".to_string())?;
        push_played_at(&conn, client.as_ref(), artist_id, today())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::residency::claude::fake::FakeLlm;
    use crate::commands::residency::insrt::{ActivityPage, PressKit, PressLink, Recipient};
    use crate::commands::residency::testdb;
    use std::sync::Mutex;

    fn d(s: &str) -> NaiveDate {
        parse_day(s).unwrap()
    }

    #[test]
    fn haversine_and_cities() {
        let paris = city_coords("paris").unwrap();
        let brussels = city_coords("bruxelles").unwrap();
        let km = haversine_km(paris, brussels);
        assert!((km - 264.0).abs() < 5.0, "{}", km);
        assert!(haversine_km(paris, paris) < 1e-9);
        let london = city_coords("london").unwrap();
        assert!((haversine_km(paris, london) - 344.0).abs() < 6.0);
        assert_eq!(city_key("Paris 11e"), "paris");
        assert_eq!(city_key("Lyon, France"), "lyon");
        assert_eq!(city_key("Saint-Ouen (93)"), "saintouen");
        assert_eq!(city_coords("saintouen"), Some(paris));
        assert_eq!(city_key("Genève"), "geneve");
        assert_eq!(city_country("antwerpen"), Some("BE"));
        assert!(city_coords("atlantis").is_none());
        let near = cities_within(paris, 300.0);
        assert!(near.contains(&"lille".to_string()) && near.contains(&"bruxelles".to_string()));
        assert!(!near.contains(&"berlin".to_string()) && !near.contains(&"lyon".to_string()));
    }

    #[test]
    fn days_and_words() {
        // 2026-12-12 is a Saturday.
        let gig = d("2026-12-12");
        assert_eq!(weekday_word(gig, "fr"), "samedi");
        assert_eq!(day_words(d("2026-12-11"), "fr"), "vendredi 11 décembre");
        assert_eq!(day_words(gig, "en"), "Saturday 12 December");
        let busy: HashSet<NaiveDate> = [d("2026-12-13")].into_iter().collect();
        let free = free_days(gig, &busy);
        assert_eq!(free, vec![d("2026-12-10"), d("2026-12-11"), d("2026-12-14")]);
        // Friday beats Thursday and Monday.
        assert_eq!(best_day(gig, &free), Some(d("2026-12-11")));
        assert_eq!(best_day(gig, &[d("2026-12-10"), d("2026-12-14")]), Some(d("2026-12-10")));
        assert_eq!(best_day(gig, &[]), None);
        assert_eq!(played_at_line("Badaboum", Some("Paris"), "2026-03-14", "fr"), "a joué à Badaboum, Paris (mars 2026)");
        assert_eq!(played_at_line("Badaboum", None, "2026-03-14", "en"), "played at Badaboum (March 2026)");
    }

    #[test]
    fn warmth_order() {
        assert_eq!(warmth(Some("in_discussion"), true, true), 0);
        assert_eq!(warmth(Some("contacted"), true, false), 1);
        assert_eq!(warmth(Some("no_answer"), false, true), 2);
        assert_eq!(warmth(Some("followed_up"), false, false), 3);
        assert_eq!(warmth(Some("to_contact"), false, false), 4);
        assert_eq!(warmth(None, false, false), 6);
        let s = |name: &str, w: u8, score: i64, dist: f64| Suggestion {
            kind: "contact".into(),
            contact_id: None,
            venue_id: None,
            name: name.into(),
            city: String::new(),
            country: None,
            distance_km: dist,
            warmth: w,
            warmth_label: warmth_label(w).into(),
            status: None,
            score,
            has_email: true,
            previously_booked: false,
        };
        let r = rank(vec![s("far cold", 4, 90, 200.0), s("venue", 6, 99, 0.0), s("warm", 0, 0, 250.0), s("near cold", 4, 95, 10.0), s("weak", 4, 20, 1.0)]);
        let names: Vec<&str> = r.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names, vec!["warm", "near cold", "far cold", "weak", "venue"]);
    }

    fn setup() -> Connection {
        let c = testdb::conn();
        migrate(&c);
        c.execute_batch(
            "INSERT INTO artists (id, name) VALUES (1, 'Piers');
             INSERT INTO contacts (id, category, name, venue, area, email, status) VALUES
                (10, 'venue', 'Le Sucre', 'Le Sucre', 'Lyon', 'prog@sucre.fr', 'confirmed'),
                (11, 'venue', 'Nuits', 'La Rotonde', 'Paris', 'booking@rotonde.fr', 'in_discussion'),
                (12, 'venue', 'Bar Lille', 'Le Bar', 'Lille', 'hi@bar.fr', 'to_contact'),
                (13, 'venue', 'Fuse', 'Fuse', 'Bruxelles', 'fuse@fuse.be', 'contacted'),
                (14, 'venue', 'Watergate', 'Watergate', 'Berlin', 'wg@wg.de', 'in_discussion'),
                (15, 'venue', 'Refus', 'Refus', 'Paris', 'no@no.fr', 'declined'),
                (16, 'venue', 'Concrete', 'Concrete', 'Paris', 'c@c.fr', 'to_contact'),
                (17, 'venue', 'Badaboum', 'Badaboum', 'Paris', 'b@b.fr', 'contacted');
             INSERT INTO events (id, artist_id, contact_id, title, venue, city, date, status) VALUES
                (1, 1, 11, 'Piers @ Rotonde', 'La Rotonde', 'Paris', '2026-12-12', 'confirmed'),
                (2, 1, 16, 'Piers @ Concrete', 'Concrete', 'Paris', '2026-12-13', 'hold'),
                (3, 1, 17, 'Piers @ Badaboum', 'Badaboum', 'Paris', '2026-03-14', 'confirmed');
             INSERT INTO vi_venues (id, nom, nom_normalise, ville, ville_norm, pays, statut, score_qualif) VALUES
                (100, 'Rex Club', 'rexclub', 'Paris', 'paris', 'FR', 'qualifie', 80),
                (101, 'Kiosk', 'kiosk', 'Gent', 'gent', 'BE', 'qualifie', 60),
                (102, 'Archived', 'archived', 'Paris', 'paris', 'FR', 'archive', 99),
                (103, 'Le Bar', 'lebar', 'Lille', 'lille', 'FR', 'qualifie', 70);
             INSERT INTO email_replies (contact_id, from_addr, message_id) VALUES (13, 'fuse@fuse.be', 'm1');",
        )
        .unwrap();
        c
    }

    #[test]
    fn tour_plan_ranks_warm_first_and_skips_taken() {
        let c = setup();
        let p = tour_plan(&c, 1, 300.0).unwrap();
        assert!(p.located);
        // The 13th is taken by the Concrete hold.
        assert_eq!(p.free_days, vec!["2026-12-10", "2026-12-11", "2026-12-14"]);
        assert_eq!(p.best_day.as_deref(), Some("2026-12-11"));
        let names: Vec<&str> = p.suggestions.iter().map(|s| s.name.as_str()).collect();
        // Own gig contact, declined, Berlin (too far), Concrete (date in the
        // window), archived venues and the Discovery twin of a contact are out.
        assert!(!names.contains(&"La Rotonde") && !names.contains(&"Refus") && !names.contains(&"Watergate"));
        assert!(!names.contains(&"Concrete") && !names.contains(&"Archived"));
        assert_eq!(names.iter().filter(|n| **n == "Le Bar").count(), 1);
        // Replied (Fuse), previously booked (Badaboum), cold (Le Bar), then Discovery.
        assert_eq!(names, vec!["Fuse", "Badaboum", "Le Bar", "Rex Club", "Kiosk"]);
        let bada = &p.suggestions[1];
        assert!(bada.previously_booked && bada.warmth_label == "booked" && bada.distance_km == 0.0);
        assert_eq!(p.suggestions[0].warmth_label, "replied");
        assert!(p.suggestions[0].distance_km > 250.0);
        assert_eq!(p.suggestions[3].kind, "venue");
        // A smaller radius keeps Paris only.
        let p = tour_plan(&c, 1, 50.0).unwrap();
        let names: Vec<&str> = p.suggestions.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["Badaboum", "Rex Club"]);
        // Routing action listed until dismissed.
        let acts = tour_actions(&c, d("2026-12-01")).unwrap();
        assert_eq!(acts.len(), 1);
        assert_eq!(acts[0].warm, 2);
        mark_followup(&c, 1, "routing").unwrap();
        assert!(tour_actions(&c, d("2026-12-01")).unwrap().is_empty());
    }

    #[test]
    fn routing_draft_template_and_claude() {
        let c = setup();
        let f = routing_facts(&c, 1, Some(13), None, "2026-12-11").unwrap();
        assert_eq!(f.lang, "fr");
        assert_eq!(f.target, "Fuse");
        let draft = write_draft(&c, None, "claude-haiku-5-5", &f);
        assert_eq!(draft.generated_by, "template");
        assert!(draft.body.contains("Je joue à Paris le samedi 12 décembre, et je serais dispo le vendredi 11 décembre à Bruxelles"), "{}", draft.body);
        assert!(draft.body.contains("Badaboum, Paris (mars 2026)"), "{}", draft.body);
        assert!(draft.body.starts_with("Hello,") && draft.body.ends_with("Thibault"));
        assert!(!draft.body.contains('—') && !draft.subject.contains('—'));
        assert!(draft.subject.contains("dispo le vendredi 11"));
        // Previously booked target: says so.
        let f = routing_facts(&c, 1, Some(17), None, "2026-12-11").unwrap();
        assert!(f.previously_booked);
        assert!(f.played_at.is_empty(), "the target itself is not cited");
        let b = write_draft(&c, None, "m", &f).body;
        assert!(b.contains("remettre ça") && b.contains("à Badaboum si jamais"), "{}", b);
        // English for a Discovery venue in Gent.
        let f = routing_facts(&c, 1, None, Some(101), "2026-12-14").unwrap();
        assert_eq!(f.lang, "en");
        assert!(write_draft(&c, None, "m", &f).body.contains("free on Monday 14 December in Gent"));
        // Claude: one retry on junk, dashes removed, facts in the prompt.
        let llm = FakeLlm::new(&["junk", r#"{"subject":"Fuse x Piers","body":"Hello,\n\nJe joue à Paris — samedi.\n\nThibault"}"#]);
        let f = routing_facts(&c, 1, Some(13), None, "2026-12-11").unwrap();
        let draft = write_draft(&c, Some(&llm), "claude-haiku-5-5", &f);
        assert_eq!(draft.generated_by, "claude-haiku-5-5");
        assert!(!draft.body.contains('—'));
        let seen = llm.seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        assert!(seen[0].user.contains("Free day offered: vendredi 11 décembre (2026-12-11)"));
        assert!(seen[0].user.contains("Kind: routing"));
        drop(seen);
        let calls: i64 = c.query_row("SELECT COUNT(*) FROM ai_usage WHERE purpose = 'routing'", [], |r| r.get(0)).unwrap();
        assert_eq!(calls, 2);
        // Claude failing twice falls back to the template with a note.
        let llm = FakeLlm::new(&["x", "y"]);
        let draft = write_draft(&c, Some(&llm), "claude-haiku-5-5", &f);
        assert_eq!(draft.generated_by, "template");
        assert!(draft.note.is_some());
    }

    #[test]
    fn rebook_loop() {
        let c = setup();
        // Badaboum played 2026-03-14. Two days later: the thank-you is due.
        let due = due_followups(&c, d("2026-03-16")).unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!((due[0].kind.as_str(), due[0].event_id, due[0].due.as_str()), ("thanks", 3, "2026-03-16"));
        assert!(due_followups(&c, d("2026-03-15")).unwrap().is_empty());
        // An email sent to the contact after the gig counts as the thank-you.
        c.execute(
            "INSERT INTO emails (contact_id, to_addr, subject, body, status, sent_at) VALUES (NULL, 'B@b.fr ', 's', 'b', 'sent', '2026-03-15 10:00:00')",
            [],
        )
        .unwrap();
        assert!(due_followups(&c, d("2026-03-17")).unwrap().is_empty());
        c.execute("DELETE FROM emails", []).unwrap();
        // Draft: weekday, venue, month four months on.
        let f = followup_facts(&c, 3, "thanks", d("2026-03-16")).unwrap();
        let draft = write_draft(&c, None, "m", &f);
        assert_eq!(draft.subject, "merci pour samedi");
        assert!(draft.body.contains("Merci encore pour samedi à Badaboum") && draft.body.contains("On se refait ça en juillet ?"), "{}", draft.body);
        mark_followup(&c, 3, "thanks").unwrap();
        assert!(due_followups(&c, d("2026-03-17")).unwrap().is_empty());
        // J+60, nothing new booked: the rebook reminder.
        let due = due_followups(&c, d("2026-05-14")).unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!((due[0].kind.as_str(), due[0].due.as_str()), ("rebook", "2026-05-13"));
        let f = followup_facts(&c, 3, "rebook", d("2026-05-14")).unwrap();
        assert!(write_draft(&c, None, "m", &f).body.contains("pour juillet"));
        // A later date with them (even on hold) stops it.
        c.execute(
            "INSERT INTO events (artist_id, contact_id, title, venue, city, date, status) VALUES (1, 17, 'again', 'Badaboum', 'Paris', '2026-09-01', 'hold')",
            [],
        )
        .unwrap();
        assert!(due_followups(&c, d("2026-05-14")).unwrap().is_empty());
        assert!(mark_followup(&c, 3, "bogus").is_err());
    }

    #[derive(Default)]
    struct FakeInsrt {
        sent: Mutex<Vec<PressKitUpsert>>,
    }
    impl Insrt for FakeInsrt {
        fn press_kits(&self, _: &str) -> Result<Vec<PressKit>, String> {
            Ok(vec![])
        }
        fn press_link(&self, _: &str, _: i64, _: &Recipient, _: &str) -> Result<PressLink, String> {
            Err("unused".into())
        }
        fn activity(&self, _: &str, _: &str) -> Result<ActivityPage, String> {
            Ok(ActivityPage { events: vec![], next: None })
        }
        fn upsert_press_kit(&self, kit: &PressKitUpsert) -> Result<PressKitAck, String> {
            self.sent.lock().unwrap().push(kit.clone());
            insrt::parse_press_kit_ack(r#"{"id":"pk_42","title":"Piers · dates jouées","created":true}"#)
        }
    }

    #[test]
    fn played_at_recaps_and_press_kit() {
        let c = setup();
        c.execute_batch(
            "INSERT INTO events (id, artist_id, contact_id, title, venue, city, date, status) VALUES
                (4, 1, 10, 'Sucre', 'Le Sucre', 'Lyon', '2026-06-20', 'confirmed'),
                (5, 1, 10, 'Sucre again', 'Le Sucre', 'Lyon', '2026-02-01', 'confirmed'),
                (6, 1, NULL, 'Cancelled', 'Fabric', 'London', '2026-05-01', 'cancelled');",
        )
        .unwrap();
        let r = recap_save(
            &c,
            &GigRecap {
                event_id: 4,
                photos: "https://photos.app/abc, not-a-link".into(),
                videos: "https://youtu.be/x https://youtu.be/y".into(),
                attendance: " 600, complet ".into(),
                quote: "Une des meilleures closing — de l'année".into(),
                quote_author: "Marc, programmateur".into(),
                played_at: true,
                updated_at: None,
            },
        )
        .unwrap();
        assert_eq!(r.photos, "https://photos.app/abc");
        assert_eq!(r.videos, "https://youtu.be/x\nhttps://youtu.be/y");
        assert_eq!(r.attendance, "600, complet");
        assert!(!r.quote.contains('—'));
        assert!(recap_get(&c, 999).unwrap().played_at, "on by default");

        let list = played_at(&c, Some(1), d("2026-10-30")).unwrap();
        let lines: Vec<&str> = list.iter().map(|p| p.line_fr.as_str()).collect();
        // Latest per venue, cancelled and future gigs out.
        assert_eq!(lines, vec!["a joué à Le Sucre, Lyon (juin 2026)", "a joué à Badaboum, Paris (mars 2026)"]);
        assert_eq!(list[0].videos.len(), 2);
        // Hidden from the list.
        c.execute("INSERT INTO gig_recaps (event_id, played_at) VALUES (3, 0)", []).unwrap();
        assert_eq!(played_at(&c, Some(1), d("2026-10-30")).unwrap().len(), 1);
        c.execute("DELETE FROM gig_recaps WHERE event_id = 3", []).unwrap();

        // Residency drafts get the facts (campaign of artist 1).
        c.execute("INSERT INTO campaigns (id, name, artist_id) VALUES (7, 'R', 1)", []).unwrap();
        let facts = played_at_facts_for_campaign(&c, 7);
        assert_eq!(facts[0], "Le Sucre, Lyon (June 2026)");

        // Press kit: content shape, then the id is reused on the next push.
        let fake = FakeInsrt::default();
        let res = push_played_at(&c, &fake, Some(1), d("2026-10-30")).unwrap();
        assert_eq!((res.id.as_str(), res.count, res.created), ("pk_42", 2, true));
        let sent = fake.sent.lock().unwrap().clone();
        let v = serde_json::to_value(&sent[0]).unwrap();
        assert!(v.get("id").is_none());
        assert_eq!(v["artistSlug"], "piers");
        assert_eq!(v["title"], "Piers · dates jouées");
        let fr = v["content"]["pitch"]["fr"].as_str().unwrap();
        assert!(fr.starts_with("**Le Sucre**, Lyon (juin 2026)\n\n**Badaboum**, Paris (mars 2026)"), "{}", fr);
        assert!(fr.contains("*« Une des meilleures closing, de l'année »*, Marc, programmateur (Le Sucre)"), "{}", fr);
        assert!(fr.contains("[Vidéo Le Sucre 1](https://youtu.be/x)") && fr.contains("[Photos Le Sucre](https://photos.app/abc)"));
        assert!(v["content"]["pitch"]["en"].as_str().unwrap().contains("Lyon (June 2026)"));
        assert_eq!(v["content"]["stats"][0]["value"], "2");
        assert_eq!(v["content"]["stats"][1]["label"]["fr"], "villes");
        assert_eq!(v["content"]["photos"], json!([]));
        assert_eq!(v["content"]["headline"]["fr"], "Piers a joué à");
        drop(sent);
        push_played_at(&c, &fake, Some(1), d("2026-10-30")).unwrap();
        assert_eq!(fake.sent.lock().unwrap()[1].id.as_deref(), Some("pk_42"));
        // Nothing to push: an error, no call.
        let empty = FakeInsrt::default();
        assert!(push_played_at(&c, &empty, Some(1), d("2026-01-01")).is_err());
        assert!(empty.sent.lock().unwrap().is_empty());
        // The ack parser on the documented contract.
        let ack = insrt::parse_press_kit_ack(r#"{"id":"k1","title":"T","created":false}"#).unwrap();
        assert!(!ack.created);
        assert!(insrt::parse_press_kit_ack("<html>").is_err());
    }
}
