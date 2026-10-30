//! Where a residency campaign aims and when it may send (pure logic, no DB).
//!
//! - Target zone: presets (Paris / Île-de-France, London, Belgique, Suisse) or
//!   free cities / country codes, matched against vi_venues.pays / ville_norm.
//! - Email language per target: French for France and the French-speaking
//!   cities of Belgium and Switzerland, English elsewhere (campaign override).
//! - Time zone per target country (Europe/London for GB, Europe/Paris else),
//!   with the EU daylight-saving rule computed here (no tz database crate).
//! - Send window (days + hours, in the target's local time).

use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Timelike, Utc, Weekday};
use serde::{Deserialize, Serialize};

use crate::db::normalise;

/// A campaign's target zone. Cities are accent-folded like vi_venues.ville_norm.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Zone {
    /// Preset keys picked in the UI ("paris", "london", "belgium", "switzerland").
    #[serde(default)]
    pub presets: Vec<String>,
    /// Extra cities, free text ("Lyon", "Genève").
    #[serde(default)]
    pub cities: Vec<String>,
    /// Extra ISO country codes ("BE", "GB").
    #[serde(default)]
    pub countries: Vec<String>,
}

/// Paris plus the Île-de-France communes with a club scene RA lists separately.
pub const PARIS_IDF: &[&str] = &[
    "paris", "iledefrance", "saintdenis", "saintouen", "saintouensurseine", "pantin", "montreuil",
    "aubervilliers", "bagnolet", "ivrysurseine", "vitrysurseine", "boulognebillancourt",
    "issylesmoulineaux", "clichy", "levalloisperret", "nanterre", "vincennes", "romainville",
    "noisylesec", "bobigny", "lepresaintgervais", "malakoff", "gentilly", "arcueil",
];

/// Presets offered by "Nouvelle campagne résidence", Paris first.
pub const PRESETS: &[(&str, &str)] = &[
    ("paris", "Paris / Île-de-France"),
    ("london", "London"),
    ("belgium", "Belgique (Bruxelles, Anvers, Gand...)"),
    ("switzerland", "Suisse (Zurich, Genève, Bâle...)"),
];

/// French-speaking cities of Belgium and Switzerland (folded).
const FRENCH_CITIES: &[&str] = &[
    "bruxelles", "brussels", "brussel", "liege", "namur", "charleroi", "mons", "louvainlaneuve",
    "tournai", "wavre", "arlon", "geneve", "geneva", "genf", "lausanne", "neuchatel", "fribourg",
    "sion", "montreux", "vevey", "delemont", "yverdonlesbains", "nyon", "lachauxdefonds",
];

impl Zone {
    /// Folded city list and ISO country codes the zone covers.
    pub fn resolve(&self) -> (Vec<String>, Vec<String>) {
        let mut cities: Vec<String> = Vec::new();
        let mut countries: Vec<String> = Vec::new();
        for p in &self.presets {
            match p.as_str() {
                "paris" => cities.extend(PARIS_IDF.iter().map(|c| c.to_string())),
                "london" => cities.push("london".into()),
                "belgium" => countries.push("BE".into()),
                "switzerland" => countries.push("CH".into()),
                _ => {}
            }
        }
        cities.extend(self.cities.iter().map(|c| normalise(c)).filter(|c| c.len() >= 2));
        countries.extend(
            self.countries
                .iter()
                .map(|c| iso_country(c))
                .filter(|c| c.len() == 2),
        );
        cities.sort();
        cities.dedup();
        countries.sort();
        countries.dedup();
        (cities, countries)
    }

    pub fn is_empty(&self) -> bool {
        let (c, k) = self.resolve();
        c.is_empty() && k.is_empty()
    }

    /// The country of the zone when it holds only one (used for targets whose
    /// country is unknown, e.g. a pipeline contact typed by hand).
    pub fn default_country(&self) -> String {
        let (_, countries) = self.resolve();
        let mut set: Vec<String> = countries;
        for p in &self.presets {
            match p.as_str() {
                "paris" => set.push("FR".into()),
                "london" => set.push("GB".into()),
                _ => {}
            }
        }
        set.sort();
        set.dedup();
        if set.len() == 1 {
            set.remove(0)
        } else {
            "FR".into()
        }
    }
}

/// ISO code from what Discovery stores ("UK" for Great Britain).
pub fn iso_country(raw: &str) -> String {
    let up = raw.trim().to_uppercase();
    if up == "UK" {
        "GB".into()
    } else {
        up
    }
}

/// The values vi_venues.pays may hold for an ISO code.
pub fn pays_values(iso: &str) -> Vec<String> {
    if iso == "GB" {
        vec!["GB".into(), "UK".into()]
    } else {
        vec![iso.to_string()]
    }
}

/// Email language for a target: "fr" or "en". `override_lang` is the
/// campaign setting ("auto", "fr", "en").
/// City name as written in the email's language: Discovery stores RA's English
/// names ("Brussels", "Ghent"), a French email says "Bruxelles", "Gand".
pub fn city_label(city: &str, lang: &str) -> String {
    const PAIRS: &[(&str, &str)] = &[
        ("brussels", "Bruxelles"), ("antwerp", "Anvers"), ("ghent", "Gand"), ("bruges", "Bruges"),
        ("leuven", "Louvain"), ("mechelen", "Malines"), ("liege", "Liège"), ("geneva", "Genève"),
        ("basel", "Bâle"), ("bern", "Berne"), ("lucerne", "Lucerne"), ("zurich", "Zurich"),
        ("london", "Londres"), ("cologne", "Cologne"), ("munich", "Munich"), ("vienna", "Vienne"),
        ("lisbon", "Lisbonne"), ("barcelona", "Barcelone"), ("amsterdam", "Amsterdam"), ("berlin", "Berlin"),
    ];
    let key = normalise(city);
    match lang {
        "fr" => PAIRS.iter().find(|(en, _)| *en == key).map(|(_, fr)| fr.to_string()),
        _ => PAIRS
            .iter()
            .find(|(_, fr)| normalise(fr) == key)
            .map(|(en, _)| {
                let mut c = en.chars();
                c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
            }),
    }
    .unwrap_or_else(|| city.trim().to_string())
}

pub fn language_for(country: &str, city: Option<&str>, override_lang: &str) -> &'static str {
    match override_lang {
        "fr" => return "fr",
        "en" => return "en",
        _ => {}
    }
    let c = iso_country(country);
    if c == "FR" || c == "MC" || c == "LU" {
        return "fr";
    }
    if c == "BE" || c == "CH" {
        let folded = city.map(normalise).unwrap_or_default();
        if FRENCH_CITIES.iter().any(|f| *f == folded) {
            return "fr";
        }
        return "en";
    }
    "en"
}

// ---------------- Time zones ----------------

/// Local time zone of a target, as a fixed standard offset + EU DST.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tz {
    London,
    Paris,
}

impl Tz {
    pub fn for_country(country: &str) -> Tz {
        match iso_country(country).as_str() {
            "GB" | "IE" | "PT" => Tz::London,
            _ => Tz::Paris,
        }
    }

    fn standard_offset_h(&self) -> i64 {
        match self {
            Tz::London => 0,
            Tz::Paris => 1,
        }
    }

    /// UTC offset (hours) at a UTC instant. EU rule: summer time from the last
    /// Sunday of March 01:00 UTC to the last Sunday of October 01:00 UTC, the
    /// same instant in every EU/UK zone.
    pub fn offset_h(&self, at: DateTime<Utc>) -> i64 {
        self.standard_offset_h() + if is_eu_dst(at) { 1 } else { 0 }
    }

    pub fn to_local(&self, at: DateTime<Utc>) -> NaiveDateTime {
        at.naive_utc() + Duration::hours(self.offset_h(at))
    }

    /// UTC instant of a local wall-clock time (the DST gap/overlap hour is
    /// resolved with the offset in force just before it; good enough for a
    /// 10:00-17:00 window).
    pub fn to_utc(&self, local: NaiveDateTime) -> DateTime<Utc> {
        let guess = Utc.from_utc_datetime(&(local - Duration::hours(self.standard_offset_h())));
        let off = self.offset_h(guess - Duration::hours(1));
        Utc.from_utc_datetime(&(local - Duration::hours(off)))
    }
}

fn last_sunday(year: i32, month: u32) -> NaiveDate {
    let next = if month == 12 {
        NaiveDate::from_ymd_opt(year + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(year, month + 1, 1)
    }
    .expect("valid date");
    let mut d = next - Duration::days(1);
    while d.weekday() != Weekday::Sun {
        d -= Duration::days(1);
    }
    d
}

pub fn is_eu_dst(at: DateTime<Utc>) -> bool {
    let y = at.year();
    let one = NaiveTime::from_hms_opt(1, 0, 0).expect("valid time");
    let start = Utc.from_utc_datetime(&last_sunday(y, 3).and_time(one));
    let end = Utc.from_utc_datetime(&last_sunday(y, 10).and_time(one));
    at >= start && at < end
}

// ---------------- Send window ----------------

/// Days (1 = Monday ... 7 = Sunday) and hours during which mail may go out,
/// in the target's local time. Default Mon-Fri 10:00-17:00.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Window {
    pub days: Vec<u32>,
    /// "HH:MM"
    pub start: String,
    pub end: String,
}

impl Default for Window {
    fn default() -> Self {
        Window { days: vec![1, 2, 3, 4, 5], start: "10:00".into(), end: "17:00".into() }
    }
}

fn hm(s: &str) -> Option<u32> {
    let (h, m) = s.trim().split_once(':')?;
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

impl Window {
    fn bounds(&self) -> (u32, u32) {
        let s = hm(&self.start).unwrap_or(600);
        let e = hm(&self.end).unwrap_or(1020);
        if e > s {
            (s, e)
        } else {
            (600, 1020)
        }
    }

    fn day_ok(&self, d: Weekday) -> bool {
        let n = d.number_from_monday();
        if self.days.is_empty() {
            n <= 5
        } else {
            self.days.contains(&n)
        }
    }

    /// Is `at` inside the window for a target in `tz`?
    pub fn contains(&self, tz: Tz, at: DateTime<Utc>) -> bool {
        let local = tz.to_local(at);
        let (s, e) = self.bounds();
        let minute = local.hour() * 60 + local.minute();
        self.day_ok(local.weekday()) && minute >= s && minute < e
    }

    /// First instant >= `at` inside the window (`at` itself when already in).
    pub fn next_open(&self, tz: Tz, at: DateTime<Utc>) -> DateTime<Utc> {
        if self.contains(tz, at) {
            return at;
        }
        let (s, e) = self.bounds();
        let local = tz.to_local(at);
        let minute = local.hour() * 60 + local.minute();
        let mut day = local.date();
        // Today is still possible when we are before the opening hour.
        if !(self.day_ok(day.weekday()) && minute < s) {
            day += Duration::days(1);
        }
        for _ in 0..14 {
            if self.day_ok(day.weekday()) {
                let open = day.and_time(NaiveTime::from_hms_opt(s / 60, s % 60, 0).expect("valid"));
                let utc = tz.to_utc(open);
                if utc >= at {
                    return utc;
                }
            }
            day += Duration::days(1);
        }
        let _ = e;
        at + Duration::days(1)
    }
}

/// A uniformly spread delay between sends, in seconds (3 to 12 minutes),
/// from a cheap random source (no rand crate needed).
pub fn spacing_secs(seed: u64) -> i64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(seed ^ Utc::now().timestamp_micros() as u64);
    180 + (h.finish() % 541) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(s: &str) -> DateTime<Utc> {
        Utc.from_utc_datetime(&NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap())
    }

    #[test]
    fn dst_and_offsets() {
        // 2026: DST from Sun 29 March to Sun 25 October, 01:00 UTC.
        assert!(!is_eu_dst(utc("2026-03-29 00:59")));
        assert!(is_eu_dst(utc("2026-03-29 01:00")));
        assert!(is_eu_dst(utc("2026-10-25 00:59")));
        assert!(!is_eu_dst(utc("2026-10-25 01:00")));
        assert_eq!(Tz::Paris.offset_h(utc("2026-07-01 12:00")), 2);
        assert_eq!(Tz::Paris.offset_h(utc("2026-11-02 12:00")), 1);
        assert_eq!(Tz::London.offset_h(utc("2026-07-01 12:00")), 1);
        assert_eq!(Tz::London.offset_h(utc("2026-11-02 12:00")), 0);
        let local = NaiveDateTime::parse_from_str("2026-11-02 10:00", "%Y-%m-%d %H:%M").unwrap();
        assert_eq!(Tz::Paris.to_utc(local), utc("2026-11-02 09:00"));
        assert_eq!(Tz::London.to_utc(local), utc("2026-11-02 10:00"));
        assert_eq!(Tz::for_country("UK"), Tz::London);
        assert_eq!(Tz::for_country("BE"), Tz::Paris);
    }

    #[test]
    fn window_contains_and_next_open() {
        let w = Window::default();
        // Monday 2 Nov 2026, 09:30 UTC = 10:30 Paris, 09:30 London.
        let t = utc("2026-11-02 09:30");
        assert!(w.contains(Tz::Paris, t));
        assert!(!w.contains(Tz::London, t));
        assert_eq!(w.next_open(Tz::London, t), utc("2026-11-02 10:00"));
        // 16:59 Paris is in, 17:00 is out.
        assert!(w.contains(Tz::Paris, utc("2026-11-02 15:59")));
        assert!(!w.contains(Tz::Paris, utc("2026-11-02 16:00")));
        // Friday evening -> Monday 10:00 Paris (09:00 UTC).
        assert_eq!(w.next_open(Tz::Paris, utc("2026-11-06 17:30")), utc("2026-11-09 09:00"));
        // Saturday is never in.
        assert!(!w.contains(Tz::Paris, utc("2026-11-07 10:00")));
        // Custom window, weekend only.
        let we = Window { days: vec![6, 7], start: "12:00".into(), end: "14:00".into() };
        assert_eq!(we.next_open(Tz::Paris, utc("2026-11-02 09:30")), utc("2026-11-07 11:00"));
        let s = spacing_secs(1);
        assert!((180..=720).contains(&s));
    }

    #[test]
    fn zones_and_languages() {
        let z = Zone { presets: vec!["paris".into()], ..Default::default() };
        let (cities, countries) = z.resolve();
        assert!(cities.contains(&"paris".to_string()) && cities.contains(&"pantin".to_string()));
        assert!(countries.is_empty());
        assert_eq!(z.default_country(), "FR");
        let z = Zone { presets: vec!["belgium".into()], cities: vec!["Genève".into()], countries: vec!["uk".into()] };
        let (cities, countries) = z.resolve();
        assert_eq!(cities, vec!["geneve".to_string()]);
        assert_eq!(countries, vec!["BE".to_string(), "GB".to_string()]);
        assert_eq!(Zone { presets: vec!["london".into()], ..Default::default() }.default_country(), "GB");

        assert_eq!(language_for("FR", Some("Paris"), "auto"), "fr");
        assert_eq!(language_for("BE", Some("Bruxelles"), "auto"), "fr");
        assert_eq!(language_for("BE", Some("Antwerp"), "auto"), "en");
        assert_eq!(language_for("CH", Some("Genève"), "auto"), "fr");
        assert_eq!(language_for("CH", Some("Zurich"), "auto"), "en");
        assert_eq!(language_for("UK", Some("London"), "auto"), "en");
        assert_eq!(city_label("Brussels", "fr"), "Bruxelles");
        assert_eq!(city_label("Ghent", "fr"), "Gand");
        assert_eq!(city_label("Zürich", "fr"), "Zurich");
        assert_eq!(city_label("Bruxelles", "en"), "Brussels");
        assert_eq!(city_label("Paris", "fr"), "Paris");
        assert_eq!(language_for("UK", Some("London"), "fr"), "fr");
        assert_eq!(pays_values("GB"), vec!["GB".to_string(), "UK".to_string()]);
    }
}
