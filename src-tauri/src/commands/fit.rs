//! "Booke des artistes comme Piers": a 0-100 fit score per venue and per
//! promoter, from what they programmed over the last 12 months.
//!
//! Inputs, all from the RA harvest:
//! - lineups (vi_event_artists, plus vi_evidence for venues harvested before
//!   lineups were stored) against the artist's comparable artists
//!   (artist_comparables, editable in Settings),
//! - genre of each night: its title ("House Sessions", "tech house all
//!   night") or an artist of the matching genre on the bill,
//! - how many nights and how recent the last one,
//! - capacity (venue capacity, median of the rooms a promoter uses).
//!
//! `score_fit` is pure and unit-tested; `recompute` loads the data, scores
//! every venue and promoter and stores fit_score + fit_detail (JSON the UI
//! turns into "a programmé X, Y ; 14 soirées house en 12 mois") together with
//! the timing inputs (lead_weeks, month_profile, see timing.rs).

use std::collections::{HashMap, HashSet};

use chrono::{Duration, NaiveDate, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tauri::State;

use super::timing;
use crate::db::normalise;
use crate::AppState;

/// Weights of the four parts (they sum to 100).
pub const W_OVERLAP: f64 = 40.0;
pub const W_GENRE: f64 = 25.0;
pub const W_ACTIVITY: f64 = 15.0;
pub const W_RECENCY: f64 = 5.0;
pub const W_CAPACITY: f64 = 15.0;
/// Genre nights in 12 months for the full genre part.
const GENRE_FULL: f64 = 12.0;
/// Nights in 12 months for the full activity part.
const NIGHTS_FULL: f64 = 24.0;
/// Capacity that suits an artist in development (same as the residency default).
pub const CAP_RANGE: (i64, i64) = (150, 800);

#[derive(Debug, Clone, Default)]
pub struct EventFacts {
    pub date: NaiveDate,
    pub title: Option<String>,
    /// Normalised artist names (db::normalise).
    pub artists: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct FitParams<'a> {
    /// Normalised comparable name -> display name.
    pub comparables: &'a HashMap<String, String>,
    /// Normalised names of artists of the artist's genre (reference artists
    /// whose genres match, plus the comparables).
    pub genre_artists: &'a HashSet<String>,
    /// Genre words to look for in event titles ("house", "tech house").
    pub genre_words: &'a [String],
    pub cap_range: (i64, i64),
    pub today: NaiveDate,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct FitParts {
    pub overlap: i64,
    pub genre: i64,
    pub activity: i64,
    pub capacity: i64,
}

/// Stored as fit_detail (JSON) and sent to the UI as is.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct FitResult {
    pub score: i64,
    /// Comparable artists they booked, most frequent first (display names).
    pub matched: Vec<String>,
    pub genre_nights: i64,
    pub nights: i64,
    pub last_date: Option<String>,
    pub capacity: Option<i64>,
    /// First genre word, for "14 soirées house".
    pub genre_label: String,
    pub parts: FitParts,
}

fn tokens(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric()).filter(|t| !t.is_empty()).map(normalise).collect()
}

/// Whether a title names the genre: every word of a genre phrase, in order,
/// as whole words ("Warehouse" is not "house", "Tech-House Night" is).
pub fn title_has_genre(title: &str, genre_words: &[String]) -> bool {
    let t = tokens(title);
    genre_words.iter().any(|g| {
        let g = tokens(g);
        !g.is_empty() && t.windows(g.len()).any(|w| w == g.as_slice())
    })
}

fn capacity_points(cap: Option<i64>, (lo, hi): (i64, i64)) -> f64 {
    match cap {
        None => W_CAPACITY * 0.5,
        Some(c) if c >= lo && c <= hi => W_CAPACITY,
        Some(c) if c >= lo / 2 && c <= hi * 2 => W_CAPACITY * 0.3,
        Some(_) => 0.0,
    }
}

/// Fit of one venue / promoter. None when they had no event in the last 12
/// months (no basis for a score: callers keep their previous behaviour).
pub fn score_fit(events: &[EventFacts], capacity: Option<i64>, p: &FitParams) -> Option<FitResult> {
    let from = p.today - Duration::days(365);
    let recent: Vec<&EventFacts> = events.iter().filter(|e| e.date >= from && e.date <= p.today).collect();
    if recent.is_empty() {
        return None;
    }
    // Comparables booked, counted once per night.
    let mut freq: HashMap<&str, (i64, NaiveDate)> = HashMap::new();
    let mut genre_nights = 0i64;
    for e in &recent {
        let mut seen: HashSet<&str> = HashSet::new();
        let mut genre = e.title.as_deref().is_some_and(|t| title_has_genre(t, p.genre_words));
        for a in &e.artists {
            if p.comparables.contains_key(a) && seen.insert(a.as_str()) {
                let f = freq.entry(a.as_str()).or_insert((0, e.date));
                f.0 += 1;
                f.1 = f.1.max(e.date);
            }
            if p.genre_artists.contains(a) || p.comparables.contains_key(a) {
                genre = true;
            }
        }
        if genre {
            genre_nights += 1;
        }
    }
    let mut matched: Vec<(&str, (i64, NaiveDate))> = freq.into_iter().collect();
    matched.sort_by(|a, b| b.1 .0.cmp(&a.1 .0).then(b.1 .1.cmp(&a.1 .1)).then(a.0.cmp(b.0)));
    let distinct = matched.len() as i32;
    let nights = recent.len() as i64;
    let last = recent.iter().map(|e| e.date).max().expect("non empty");
    let since = (p.today - last).num_days();

    let overlap = W_OVERLAP * (1.0 - 0.6f64.powi(distinct));
    let genre = W_GENRE * (genre_nights as f64 / GENRE_FULL).min(1.0);
    let activity = W_ACTIVITY * (nights as f64 / NIGHTS_FULL).min(1.0)
        + match since {
            s if s <= 45 => W_RECENCY,
            s if s <= 120 => W_RECENCY * 0.6,
            s if s <= 240 => W_RECENCY * 0.2,
            _ => 0.0,
        };
    let cap = capacity_points(capacity, p.cap_range);
    let parts = FitParts {
        overlap: overlap.round() as i64,
        genre: genre.round() as i64,
        activity: activity.round() as i64,
        capacity: cap.round() as i64,
    };
    let score = (overlap + genre + activity + cap).round().clamp(0.0, 100.0) as i64;
    Some(FitResult {
        score,
        matched: matched.iter().map(|(n, _)| p.comparables.get(*n).cloned().unwrap_or_else(|| n.to_string())).collect(),
        genre_nights,
        nights,
        last_date: Some(last.format("%Y-%m-%d").to_string()),
        capacity,
        genre_label: p.genre_words.first().cloned().unwrap_or_else(|| "house".into()),
        parts,
    })
}

/// "a programmé X, Y ; 14 soirées house en 12 mois" (no em dash, ever).
pub fn explain_fr(r: &FitResult) -> String {
    let mut parts = Vec::new();
    if !r.matched.is_empty() {
        parts.push(format!("a programmé {}", r.matched.iter().take(3).cloned().collect::<Vec<_>>().join(", ")));
    }
    if r.genre_nights > 0 {
        let s = if r.genre_nights > 1 { "s" } else { "" };
        parts.push(format!("{} soirée{s} {} en 12 mois", r.genre_nights, r.genre_label));
    } else {
        let s = if r.nights > 1 { "s" } else { "" };
        parts.push(format!("{} soirée{s} en 12 mois", r.nights));
    }
    parts.join(" ; ")
}

/// Genre words from the artist's free-text genres ("House, Tech House").
pub fn genre_words_of(genres: Option<&str>) -> Vec<String> {
    let mut out: Vec<String> = genres
        .unwrap_or("")
        .split([',', '/', ';', '|', '\n'])
        .map(|g| g.trim().to_lowercase())
        .filter(|g| g.len() >= 3)
        .collect();
    out.dedup();
    if out.is_empty() {
        out.push("house".into());
    }
    out
}

// ---------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------

/// Columns and tables of the fit score and timing. Additive and idempotent.
pub(crate) fn migrate(conn: &Connection) {
    for stmt in [
        "ALTER TABLE vi_venues ADD COLUMN fit_score INTEGER",
        "ALTER TABLE vi_venues ADD COLUMN fit_detail TEXT",
        "ALTER TABLE vi_venues ADD COLUMN fit_at TEXT",
        "ALTER TABLE vi_venues ADD COLUMN lead_weeks INTEGER",
        "ALTER TABLE vi_venues ADD COLUMN lead_source TEXT",
        "ALTER TABLE vi_venues ADD COLUMN month_profile TEXT",
        "CREATE INDEX IF NOT EXISTS idx_vi_venues_fit ON vi_venues(fit_score DESC)",
        // Comparable artists, per artist of the roster.
        "CREATE TABLE IF NOT EXISTS artist_comparables (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            artist_id     INTEGER NOT NULL REFERENCES artists(id) ON DELETE CASCADE,
            nom           TEXT NOT NULL,
            nom_normalise TEXT NOT NULL,
            source        TEXT NOT NULL DEFAULT 'manual',
            created_at    TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE (artist_id, nom_normalise)
        )",
    ] {
        let _ = conn.execute(stmt, []);
    }
}

// ---------------------------------------------------------------------------
// Comparable artists
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct Comparable {
    pub id: i64,
    pub nom: String,
    /// manual | colineup | genre
    pub source: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Suggestion {
    pub nom: String,
    pub source: String,
    /// Shared lineups (colineup) or 0.
    pub count: i64,
}

/// The artist the stored scores are for: the one asked, else the roster's
/// only (or first) artist.
pub fn resolve_artist(conn: &Connection, artist_id: Option<i64>) -> Option<i64> {
    artist_id
        .or_else(|| crate::commands::artists::default_artist_id(conn))
        .or_else(|| conn.query_row("SELECT id FROM artists ORDER BY id LIMIT 1", [], |r| r.get(0)).ok())
}

pub fn list_comparables(conn: &Connection, artist_id: i64) -> Result<Vec<Comparable>, String> {
    let mut st = conn
        .prepare("SELECT id, nom, source FROM artist_comparables WHERE artist_id = ?1 ORDER BY nom COLLATE NOCASE")
        .map_err(|e| e.to_string())?;
    let rows = st
        .query_map([artist_id], |r| Ok(Comparable { id: r.get(0)?, nom: r.get(1)?, source: r.get(2)? }))
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
}

pub fn add_comparable(conn: &Connection, artist_id: i64, nom: &str, source: &str) -> Result<bool, String> {
    let nom = nom.trim();
    let norm = normalise(nom);
    if norm.is_empty() {
        return Err("fit.name_empty".into());
    }
    conn.execute(
        "INSERT OR IGNORE INTO artist_comparables (artist_id, nom, nom_normalise, source) VALUES (?1, ?2, ?3, ?4)",
        params![artist_id, nom, norm, source],
    )
    .map(|n| n > 0)
    .map_err(|e| e.to_string())
}

/// Suggested comparables: first the artists who shared a harvested lineup with
/// this artist (RA lineups, or a gig of ours matched to an RA event on the
/// same venue and date), then the reference artists of the same genre. Names
/// already in the list are left out.
pub fn suggest_comparables(conn: &Connection, artist_id: i64) -> Result<Vec<Suggestion>, String> {
    let (name, genres): (String, Option<String>) = conn
        .query_row("SELECT name, genres FROM artists WHERE id = ?1", [artist_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(|e| e.to_string())?;
    let me = normalise(&name);
    let have: HashSet<String> = list_comparables(conn, artist_id)?.into_iter().map(|c| normalise(&c.nom)).collect();
    let mut out: Vec<Suggestion> = Vec::new();
    let mut seen: HashSet<String> = have.clone();
    seen.insert(me.clone());

    // Co-lineups: RA events with the artist on the bill, plus RA events at a
    // venue of one of our gigs on the gig's date.
    let sql = "
        WITH mine AS (
            SELECT ra_event_id FROM vi_event_artists WHERE nom_normalise = ?1
            UNION
            SELECT ve.ra_event_id FROM events g
              JOIN vi_venues v ON lower(trim(v.nom)) = lower(trim(g.venue))
              JOIN vi_venue_events ve ON ve.venue_id = v.id AND ve.date_event = substr(g.date, 1, 10)
             WHERE g.artist_id = ?2 AND g.venue IS NOT NULL AND trim(g.venue) <> ''
        )
        SELECT MIN(a.nom), a.nom_normalise, COUNT(DISTINCT a.ra_event_id) n
        FROM vi_event_artists a JOIN mine m ON m.ra_event_id = a.ra_event_id
        WHERE a.nom_normalise <> ?1
        GROUP BY a.nom_normalise ORDER BY n DESC, MIN(a.nom) LIMIT 30";
    {
        let mut st = conn.prepare(sql).map_err(|e| e.to_string())?;
        let rows = st
            .query_map(params![me, artist_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?)))
            .map_err(|e| e.to_string())?;
        for (nom, norm, n) in rows.flatten() {
            if seen.insert(norm) {
                out.push(Suggestion { nom, source: "colineup".into(), count: n });
            }
        }
    }

    // Same genre: reference artists whose genres match the artist's. The
    // seeded reference list is house / tech house and carries no genres, so
    // when none has genres, its tier 2 and 3 names (the scene's working DJs,
    // not the headliners) stand in.
    let words = genre_words_of(genres.as_deref());
    let refs: Vec<(String, String, i64, Option<String>)> = {
        let mut st = conn
            .prepare("SELECT nom, nom_normalise, tier, genres FROM vi_reference_artists WHERE actif = 1 ORDER BY tier, nom COLLATE NOCASE")
            .map_err(|e| e.to_string())?;
        let rows = st
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .map_err(|e| e.to_string())?;
        rows.flatten().collect()
    };
    let any_genres = refs.iter().any(|r| r.3.as_deref().is_some_and(|g| !g.trim().is_empty()));
    for (nom, norm, tier, g) in refs {
        let ok = if any_genres {
            g.as_deref().is_some_and(|g| genre_words_of(Some(g)).iter().any(|x| words.contains(x)))
        } else {
            tier >= 2
        };
        if ok && seen.insert(norm) {
            out.push(Suggestion { nom, source: "genre".into(), count: 0 });
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Recompute
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Default)]
pub struct FitStats {
    pub artist_id: Option<i64>,
    pub comparables: i64,
    pub venues_scored: i64,
    pub promoters_scored: i64,
}

fn parse_day(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s.get(0..10)?, "%Y-%m-%d").ok()
}

/// Score every venue and promoter for `artist_id` (see resolve_artist) as of
/// `today`, and store the timing inputs. Rows without any event in the last 12
/// months get a NULL score (their callers fall back to the old ranking).
pub fn recompute(conn: &Connection, artist_id: Option<i64>, today: NaiveDate) -> Result<FitStats, String> {
    let artist_id = resolve_artist(conn, artist_id);
    let genres: Option<String> = artist_id
        .and_then(|id| conn.query_row("SELECT genres FROM artists WHERE id = ?1", [id], |r| r.get(0)).ok())
        .flatten();
    let genre_words = genre_words_of(genres.as_deref());
    let mut comparables: HashMap<String, String> = HashMap::new();
    if let Some(id) = artist_id {
        for c in list_comparables(conn, id)? {
            comparables.insert(normalise(&c.nom), c.nom);
        }
    }
    let mut genre_artists: HashSet<String> = HashSet::new();
    {
        let mut st = conn
            .prepare("SELECT nom_normalise, genres FROM vi_reference_artists WHERE actif = 1")
            .map_err(|e| e.to_string())?;
        let rows: Vec<(String, Option<String>)> =
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map_err(|e| e.to_string())?.flatten().collect();
        let any = rows.iter().any(|r| r.1.as_deref().is_some_and(|g| !g.trim().is_empty()));
        for (n, g) in rows {
            // Without genres on the list, the whole (house / tech house) list counts.
            let ok = !any || g.as_deref().is_some_and(|g| genre_words_of(Some(g)).iter().any(|x| genre_words.contains(x)));
            if ok {
                genre_artists.insert(n);
            }
        }
    }
    let p = FitParams {
        comparables: &comparables,
        genre_artists: &genre_artists,
        genre_words: &genre_words,
        cap_range: CAP_RANGE,
        today,
    };
    let cutoff = (today - Duration::days(365)).format("%Y-%m-%d").to_string();
    let lead_cutoff = (today - Duration::days(730)).format("%Y-%m-%d").to_string();

    // Events of the last 12 months, with their lineups.
    let mut events: HashMap<i64, (i64, EventFacts)> = HashMap::new();
    {
        let mut st = conn
            .prepare("SELECT ra_event_id, venue_id, date_event, titre FROM vi_venue_events WHERE date_event >= ?1")
            .map_err(|e| e.to_string())?;
        let rows = st
            .query_map([&cutoff], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, Option<String>>(3)?)))
            .map_err(|e| e.to_string())?;
        for (eid, vid, date, titre) in rows.flatten() {
            if let Some(d) = date.as_deref().and_then(parse_day) {
                events.insert(eid, (vid, EventFacts { date: d, title: titre, artists: Vec::new() }));
            }
        }
        let mut st = conn
            .prepare(
                "SELECT a.ra_event_id, a.nom_normalise FROM vi_event_artists a
                 JOIN vi_venue_events e ON e.ra_event_id = a.ra_event_id WHERE e.date_event >= ?1",
            )
            .map_err(|e| e.to_string())?;
        let rows = st.query_map([&cutoff], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))).map_err(|e| e.to_string())?;
        for (eid, a) in rows.flatten() {
            if let Some((_, e)) = events.get_mut(&eid) {
                e.artists.push(a);
            }
        }
    }
    // Per venue, plus the reference-artist evidence of venues harvested before
    // lineups were stored (merged into the night of the same date).
    let mut by_venue: HashMap<i64, Vec<EventFacts>> = HashMap::new();
    for (vid, e) in events.values() {
        by_venue.entry(*vid).or_default().push(e.clone());
    }
    {
        let mut st = conn
            .prepare("SELECT venue_id, artiste, date_event, titre_event FROM vi_evidence WHERE date_event >= ?1")
            .map_err(|e| e.to_string())?;
        let rows = st
            .query_map([&cutoff], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, Option<String>>(3)?)))
            .map_err(|e| e.to_string())?;
        for (vid, artiste, date, titre) in rows.flatten() {
            let Some(d) = parse_day(&date) else { continue };
            let norm = normalise(&artiste);
            let list = by_venue.entry(vid).or_default();
            match list.iter_mut().find(|e| e.date == d) {
                Some(e) => {
                    if !e.artists.contains(&norm) {
                        e.artists.push(norm);
                    }
                }
                None => list.push(EventFacts { date: d, title: titre, artists: vec![norm] }),
            }
        }
    }
    // Lead observations (event seen on RA well before it happened).
    let mut obs_venue: HashMap<i64, Vec<(NaiveDate, NaiveDate)>> = HashMap::new();
    let mut obs_event: HashMap<i64, (NaiveDate, NaiveDate)> = HashMap::new();
    {
        let mut st = conn
            .prepare(
                "SELECT ra_event_id, venue_id, date_event, first_seen_at FROM vi_venue_events
                 WHERE first_seen_at IS NOT NULL AND date_event >= ?1",
            )
            .map_err(|e| e.to_string())?;
        let rows = st
            .query_map([&lead_cutoff], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?)))
            .map_err(|e| e.to_string())?;
        for (eid, vid, ev, seen) in rows.flatten() {
            if let (Some(ev), Some(seen)) = (parse_day(&ev), parse_day(&seen)) {
                obs_venue.entry(vid).or_default().push((ev, seen));
                obs_event.insert(eid, (ev, seen));
            }
        }
    }

    let mut stats = FitStats { artist_id, comparables: comparables.len() as i64, ..Default::default() };
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    let caps: HashMap<i64, i64> = {
        let mut st = tx
            .prepare("SELECT id, capacite_est FROM vi_venues WHERE capacite_est IS NOT NULL AND capacite_est > 0")
            .map_err(|e| e.to_string())?;
        let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))).map_err(|e| e.to_string())?;
        rows.flatten().collect()
    };
    // Venues: clear, then write the ones with data.
    tx.execute("UPDATE vi_venues SET fit_score = NULL, fit_detail = NULL WHERE fit_score IS NOT NULL", [])
        .map_err(|e| e.to_string())?;
    {
        let mut up = tx
            .prepare(
                "UPDATE vi_venues SET fit_score = ?2, fit_detail = ?3, fit_at = datetime('now'),
                    lead_weeks = ?4, lead_source = ?5, month_profile = ?6 WHERE id = ?1",
            )
            .map_err(|e| e.to_string())?;
        for (vid, evs) in &by_venue {
            let Some(r) = score_fit(evs, caps.get(vid).copied(), &p) else { continue };
            let dates: Vec<NaiveDate> = evs.iter().map(|e| e.date).collect();
            let prof = timing::month_profile(&dates, today);
            let (lead, src) = timing::estimate_lead(obs_venue.get(vid).map(Vec::as_slice).unwrap_or(&[]));
            let detail = serde_json::to_string(&r).unwrap_or_default();
            up.execute(params![vid, r.score, detail, lead, src, timing::profile_to_string(&prof)])
                .map_err(|e| e.to_string())?;
            stats.venues_scored += 1;
        }
    }

    // Promoters: their own nights (vi_event_promoters), any venue.
    let mut by_org: HashMap<i64, Vec<i64>> = HashMap::new();
    {
        let mut st = tx
            .prepare(
                "SELECT ep.promoter_id, ep.ra_event_id FROM vi_event_promoters ep
                 JOIN vi_venue_events e ON e.ra_event_id = ep.ra_event_id WHERE e.date_event >= ?1",
            )
            .map_err(|e| e.to_string())?;
        let rows = st.query_map([&cutoff], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))).map_err(|e| e.to_string())?;
        for (pid, eid) in rows.flatten() {
            by_org.entry(pid).or_default().push(eid);
        }
    }
    let org_caps: HashMap<i64, i64> = {
        let mut st = tx
            .prepare(
                "SELECT p.org_id, v.capacite_est FROM vi_promoters p JOIN vi_venues v ON v.id = p.venue_id
                 WHERE p.org_id IS NOT NULL AND v.capacite_est IS NOT NULL AND v.capacite_est > 0",
            )
            .map_err(|e| e.to_string())?;
        let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))).map_err(|e| e.to_string())?;
        let mut all: HashMap<i64, Vec<i64>> = HashMap::new();
        for (o, c) in rows.flatten() {
            all.entry(o).or_default().push(c);
        }
        all.into_iter()
            .map(|(o, mut v)| {
                v.sort_unstable();
                (o, v[v.len() / 2])
            })
            .collect()
    };
    tx.execute(
        "UPDATE vi_promoter_orgs SET fit_score = NULL, fit_detail = NULL, nb_events_12m = 0 WHERE fit_score IS NOT NULL OR nb_events_12m > 0",
        [],
    )
    .map_err(|e| e.to_string())?;
    {
        let mut up = tx
            .prepare(
                "UPDATE vi_promoter_orgs SET fit_score = ?2, fit_detail = ?3, fit_at = datetime('now'),
                    lead_weeks = ?4, lead_source = ?5, month_profile = ?6, nb_events_12m = ?7 WHERE id = ?1",
            )
            .map_err(|e| e.to_string())?;
        for (pid, eids) in &by_org {
            let evs: Vec<EventFacts> = eids.iter().filter_map(|e| events.get(e).map(|(_, f)| f.clone())).collect();
            let Some(r) = score_fit(&evs, org_caps.get(pid).copied(), &p) else { continue };
            let dates: Vec<NaiveDate> = evs.iter().map(|e| e.date).collect();
            let prof = timing::month_profile(&dates, today);
            let obs: Vec<(NaiveDate, NaiveDate)> = eids.iter().filter_map(|e| obs_event.get(e).copied()).collect();
            let (lead, src) = timing::estimate_lead(&obs);
            let detail = serde_json::to_string(&r).unwrap_or_default();
            up.execute(params![pid, r.score, detail, lead, src, timing::profile_to_string(&prof), r.nights])
                .map_err(|e| e.to_string())?;
            stats.promoters_scored += 1;
        }
    }
    crate::commands::promoters::refresh_aggregates(&tx);
    tx.commit().map_err(|e| e.to_string())?;
    if let Some(id) = artist_id {
        let _ = conn.execute(
            "INSERT OR REPLACE INTO settings (key, value) VALUES ('fit_artist_id', ?1)",
            [id.to_string()],
        );
    }
    Ok(stats)
}

/// The artist the stored scores were last computed for.
pub fn scored_artist(conn: &Connection) -> Option<i64> {
    conn.query_row("SELECT value FROM settings WHERE key = 'fit_artist_id'", [], |r| r.get::<_, String>(0))
        .ok()
        .and_then(|s| s.parse().ok())
}

/// Recompute for the artist last scored (after a harvest run). Best-effort.
pub fn recompute_default(conn: &Connection) {
    let id = scored_artist(conn);
    let _ = recompute(conn, id, Utc::now().date_naive());
}

/// The FR explanation of a stored fit_detail, for the residency target list.
pub fn explain_stored(detail: Option<&str>) -> Option<String> {
    let r: FitResult = serde_json::from_str(detail?).ok()?;
    Some(explain_fr(&r))
}

/// Comparable artists named in a stored fit_detail.
pub fn matched_artists(detail: Option<&str>) -> Vec<String> {
    detail
        .and_then(|d| serde_json::from_str::<FitResult>(d).ok())
        .map(|r| r.matched)
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[tauri::command(async)]
pub fn fit_recompute(state: State<'_, AppState>, artist_id: Option<i64>) -> Result<FitStats, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    recompute(&conn, artist_id, Utc::now().date_naive())
}

#[tauri::command(async)]
pub fn fit_list_comparables(state: State<'_, AppState>, artist_id: i64) -> Result<Vec<Comparable>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    list_comparables(&conn, artist_id)
}

#[tauri::command(async)]
pub fn fit_add_comparable(state: State<'_, AppState>, artist_id: i64, nom: String) -> Result<bool, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    add_comparable(&conn, artist_id, &nom, "manual")
}

#[tauri::command(async)]
pub fn fit_delete_comparable(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM artist_comparables WHERE id = ?1", [id]).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command(async)]
pub fn fit_suggest_comparables(state: State<'_, AppState>, artist_id: i64) -> Result<Vec<Suggestion>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    suggest_comparables(&conn, artist_id)
}

/// Add every suggestion (co-lineups first, then up to `max_genre` genre
/// names). Returns how many were added; existing names are kept.
#[tauri::command(async)]
pub fn fit_seed_comparables(state: State<'_, AppState>, artist_id: i64, max_genre: Option<i64>) -> Result<i64, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    seed_comparables(&conn, artist_id, max_genre.unwrap_or(40).max(0) as usize)
}

pub fn seed_comparables(conn: &Connection, artist_id: i64, max_genre: usize) -> Result<i64, String> {
    let mut n = 0;
    let mut genre = 0;
    for s in suggest_comparables(conn, artist_id)? {
        if s.source == "genre" {
            if genre >= max_genre {
                continue;
            }
            genre += 1;
        }
        if add_comparable(conn, artist_id, &s.nom, &s.source)? {
            n += 1;
        }
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn ev(date: &str, title: &str, artists: &[&str]) -> EventFacts {
        EventFacts { date: d(date), title: Some(title.into()), artists: artists.iter().map(|a| normalise(a)).collect() }
    }

    struct Fx {
        comparables: HashMap<String, String>,
        genre_artists: HashSet<String>,
        words: Vec<String>,
    }
    fn fx() -> Fx {
        let comparables: HashMap<String, String> =
            ["Chris Stussy", "Prunk", "Sidney Charles"].iter().map(|n| (normalise(n), n.to_string())).collect();
        let genre_artists: HashSet<String> = ["Rossi.", "Toman"].iter().map(|n| normalise(n)).collect();
        Fx { comparables, genre_artists, words: vec!["house".into(), "tech house".into()] }
    }
    fn params(f: &Fx) -> FitParams<'_> {
        FitParams {
            comparables: &f.comparables,
            genre_artists: &f.genre_artists,
            genre_words: &f.words,
            cap_range: CAP_RANGE,
            today: d("2026-10-30"),
        }
    }

    #[test]
    fn genre_in_titles_is_whole_words() {
        let w = vec!["house".to_string(), "tech house".to_string()];
        assert!(title_has_genre("Tech-House Night w/ Friends", &w));
        assert!(title_has_genre("HOUSE SESSIONS", &w));
        assert!(!title_has_genre("Warehouse Project", &w));
        assert!(!title_has_genre("Techno all night", &w));
        assert!(!title_has_genre("", &w));
        assert_eq!(genre_words_of(Some("House, Tech House / ")), vec!["house", "tech house"]);
        assert_eq!(genre_words_of(None), vec!["house"]);
    }

    #[test]
    fn no_recent_event_no_score() {
        let f = fx();
        assert!(score_fit(&[], Some(300), &params(&f)).is_none());
        let old = vec![ev("2025-06-01", "House night", &["Chris Stussy"])];
        assert!(score_fit(&old, Some(300), &params(&f)).is_none());
    }

    #[test]
    fn fitting_club_scores_high_and_explains_itself() {
        let f = fx();
        let mut evs = Vec::new();
        // 14 house nights over the year, two comparables, one of them twice.
        for i in 0..14 {
            let date = d("2026-10-20") - Duration::days(i * 21);
            let artists: Vec<&str> = match i {
                0 => vec!["Chris Stussy", "Local DJ"],
                3 => vec!["Prunk"],
                5 => vec!["Chris Stussy"],
                _ => vec!["Someone"],
            };
            evs.push(EventFacts {
                date,
                title: Some(if i % 2 == 0 { "House Sessions".into() } else { "Club night".into() }),
                artists: artists.iter().map(|a| normalise(a)).collect(),
            });
        }
        // An odd night with a genre artist and no genre in the title.
        evs[1].artists.push(normalise("Toman"));
        let r = score_fit(&evs, Some(400), &params(&f)).unwrap();
        assert_eq!(r.matched, vec!["Chris Stussy", "Prunk"]);
        assert_eq!(r.nights, 14);
        // 7 titled + nights 3 and 5 (comparables) + night 1 (genre artist).
        assert_eq!(r.genre_nights, 10);
        assert_eq!(r.parts.capacity, 15);
        assert_eq!(r.parts.overlap, 26); // 40 * (1 - 0.6^2)
        assert!(r.score >= 70, "{r:?}");
        assert_eq!(r.last_date.as_deref(), Some("2026-10-20"));
        assert_eq!(explain_fr(&r), "a programmé Chris Stussy, Prunk ; 10 soirées house en 12 mois");
        assert!(!explain_fr(&r).contains('\u{2014}'));
    }

    #[test]
    fn arena_without_overlap_scores_low() {
        let f = fx();
        let evs = vec![ev("2026-03-01", "Big Techno", &["Headliner"]), ev("2026-02-01", "Rock", &["Band"])];
        let r = score_fit(&evs, Some(8000), &params(&f)).unwrap();
        assert!(r.matched.is_empty());
        assert_eq!(r.genre_nights, 0);
        assert_eq!(r.parts.capacity, 0);
        assert!(r.score < 10, "{r:?}");
        assert_eq!(explain_fr(&r), "2 soirées en 12 mois");
        // Unknown capacity is neutral, not a penalty or a bonus.
        let u = score_fit(&evs, None, &params(&f)).unwrap();
        assert_eq!(u.parts.capacity, 8);
    }

    #[test]
    fn score_is_monotonic_in_overlap() {
        let f = fx();
        let base = vec![ev("2026-10-01", "Night", &["A"])];
        let one = vec![ev("2026-10-01", "Night", &["Prunk"])];
        let two = vec![ev("2026-10-01", "Night", &["Prunk", "Sidney Charles"])];
        let s = |e: &[EventFacts]| score_fit(e, Some(300), &params(&f)).unwrap().score;
        assert!(s(&base) < s(&one) && s(&one) < s(&two));
    }

    #[test]
    fn recompute_on_memory_db() {
        let c = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::migrate_for_tests(&c).unwrap();
        c.execute_batch(
            "INSERT INTO artists (id, name, genres) VALUES (1, 'Piers', 'House, Tech House');
             INSERT INTO vi_reference_artists (nom, nom_normalise, tier) VALUES
               ('Black Coffee', 'blackcoffee', 1), ('Toman', 'toman', 2), ('Prunk', 'prunk', 2);
             INSERT INTO vi_venues (id, nom, nom_normalise, ville, pays, statut, capacite_est)
               VALUES (1, 'Club A', 'cluba', 'Paris', 'FR', 'qualifie', 400),
                      (2, 'Club B', 'clubb', 'Paris', 'FR', 'qualifie', NULL),
                      (3, 'Old Club', 'oldclub', 'Paris', 'FR', 'qualifie', 300);
             INSERT INTO vi_venue_events (ra_event_id, venue_id, date_event, titre) VALUES
               (10, 1, '2026-10-01', 'House Sessions'), (11, 1, '2026-09-01', 'Club night'),
               (12, 2, '2026-08-01', 'Techno'), (13, 3, '2024-01-01', 'House');
             INSERT INTO vi_event_artists (ra_event_id, nom, nom_normalise) VALUES
               (10, 'Piers', 'piers'), (10, 'Prunk', 'prunk'), (11, 'Chris Stussy', 'chrisstussy'),
               (12, 'Headliner', 'headliner');
             INSERT INTO vi_evidence (venue_id, artiste, date_event, source_url) VALUES (2, 'Sidney Charles', '2026-07-01', 'u');
             INSERT INTO vi_promoter_orgs (id, cle, nom, nom_normalise) VALUES (5, 'ra:5', 'Collectif X', 'collectifx');
             INSERT INTO vi_event_promoters (ra_event_id, promoter_id) VALUES (10, 5), (11, 5);
             INSERT INTO vi_promoters (venue_id, nom, nb_events, org_id) VALUES (1, 'Collectif X', 2, 5);",
        )
        .unwrap();
        // Suggestions: Prunk and Chris Stussy? Only Prunk shared a lineup with Piers.
        let s = suggest_comparables(&c, 1).unwrap();
        assert_eq!(s[0], Suggestion { nom: "Prunk".into(), source: "colineup".into(), count: 1 });
        // Genre names: tier 2+ only (no genres on the list), Prunk not twice.
        let genre: Vec<&str> = s.iter().filter(|x| x.source == "genre").map(|x| x.nom.as_str()).collect();
        assert_eq!(genre, vec!["Toman"]);
        assert_eq!(seed_comparables(&c, 1, 0).unwrap(), 1);
        add_comparable(&c, 1, "Chris Stussy", "manual").unwrap();
        add_comparable(&c, 1, "Sidney Charles", "manual").unwrap();
        assert!(!add_comparable(&c, 1, "chris  STUSSY", "manual").unwrap());
        assert!(add_comparable(&c, 1, " ", "manual").is_err());

        let st = recompute(&c, None, d("2026-10-30")).unwrap();
        assert_eq!(st.artist_id, Some(1));
        assert_eq!(st.comparables, 3);
        assert_eq!(st.venues_scored, 2);
        assert_eq!(st.promoters_scored, 1);
        let (fa, da, lead, prof): (i64, String, i64, String) = c
            .query_row("SELECT fit_score, fit_detail, lead_weeks, month_profile FROM vi_venues WHERE id = 1", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })
            .unwrap();
        let fb: i64 = c.query_row("SELECT fit_score FROM vi_venues WHERE id = 2", [], |r| r.get(0)).unwrap();
        let fo: Option<i64> = c.query_row("SELECT fit_score FROM vi_venues WHERE id = 3", [], |r| r.get(0)).unwrap();
        assert!(fa > fb, "{fa} vs {fb}");
        assert_eq!(fo, None);
        assert_eq!(lead, 10);
        assert_eq!(prof, "0,0,0,0,0,0,0,0,1,1,0,0");
        assert!(explain_stored(Some(&da)).unwrap().starts_with("a programmé Prunk, Chris Stussy"));
        // Venue B: the comparable came from vi_evidence only.
        let db: String = c.query_row("SELECT fit_detail FROM vi_venues WHERE id = 2", [], |r| r.get(0)).unwrap();
        assert!(db.contains("Sidney Charles"));
        let (fp, n12): (i64, i64) = c
            .query_row("SELECT fit_score, nb_events_12m FROM vi_promoter_orgs WHERE id = 5", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert!(fp > 0);
        assert_eq!(n12, 2);
        assert_eq!(scored_artist(&c), Some(1));
        // Idempotent.
        let again = recompute(&c, None, d("2026-10-30")).unwrap();
        assert_eq!(again.venues_scored, 2);
    }
}
