//! Beatport: which labels are in the charts, and what a label's best sellers
//! are.
//!
//! Beatport answers 403 to anything that is not a browser, so nothing here
//! calls it from Rust. The user asks for a reading, a window opens on
//! beatport.com (a plain webview, visible, no capability: the page cannot call
//! a single command of the app) and `beatport_scan.js` reads what the page
//! itself holds: the charts of the focus genres, or the top 10 of one label.
//! If Beatport shows a check, the user passes it in that window.
//!
//! The page hands its findings over by trying to navigate to
//! `https://pierscrm.invalid/bp?k=<kind>#<json>`: the navigation is refused
//! and its URL parsed. What comes back is untrusted (it went through a web
//! page): sizes are capped, keys checked against what was asked.
//!
//! A reading is a handful of page views started by a click, never a crawl.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};

use crate::db::normalise;
use crate::AppState;

const WINDOW: &str = "beatport";
const SITE: &str = "https://www.beatport.com";
const SCAN_JS: &str = include_str!("beatport_scan.js");
pub const EVENT: &str = "labels:beatport";

/// (slug, Beatport genre id, name shown). The styles Piers sends demos in.
pub const GENRES: &[(&str, i64, &str)] =
    &[("tech-house", 11, "Tech House"), ("house", 5, "House"), ("jackin-house", 97, "Jackin House")];
/// Top 100: what sells. Hype 100: the same, small labels only.
const CHARTS: &[(&str, &str)] = &[("top", "top-100"), ("hype", "hype-100")];

/// Majors and distributors that show up as "label" on a chart: no demo desk.
const NOT_A_LABEL: &[&str] = &[
    "distrokid", "tunecore", "cd baby", "ditto music", "label services", "warner", "atlantic records", "universal",
    "def jam", "sony music", "columbia", "polydor", "interscope", "capitol records", "virgin music", "ada france",
    "believe",
];

pub fn migrate(conn: &Connection) {
    for stmt in [
        "ALTER TABLE labels ADD COLUMN instagram TEXT",
        "ALTER TABLE labels ADD COLUMN beatport_id INTEGER",
        "ALTER TABLE labels ADD COLUMN beatport_url TEXT",
        "ALTER TABLE labels ADD COLUMN chart_note TEXT",
        "ALTER TABLE labels ADD COLUMN chart_seen_at TEXT",
        "ALTER TABLE labels ADD COLUMN bio TEXT",
        "ALTER TABLE labels ADD COLUMN top_at TEXT",
        "ALTER TABLE labels ADD COLUMN contact_source TEXT",
        "ALTER TABLE labels ADD COLUMN contact_find_at TEXT",
        "CREATE TABLE IF NOT EXISTS label_tracks (
            id        INTEGER PRIMARY KEY AUTOINCREMENT,
            label_id  INTEGER NOT NULL REFERENCES labels(id) ON DELETE CASCADE,
            kind      TEXT NOT NULL,            -- chart (last reading) | top (the label's top 10)
            chart     TEXT NOT NULL DEFAULT '', -- 'tech-house:top', '' for kind top
            position  INTEGER NOT NULL,
            track_id  INTEGER NOT NULL,
            title     TEXT NOT NULL,
            artists   TEXT NOT NULL,
            url       TEXT NOT NULL,
            seen_at   TEXT NOT NULL DEFAULT (datetime('now'))
        )",
        "CREATE INDEX IF NOT EXISTS idx_label_tracks_label ON label_tracks(label_id, kind, position)",
    ] {
        let _ = conn.execute(stmt, []);
    }
}

// ---------------------------------------------------------------------------
// What the page hands over
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Default)]
pub struct LabelRef {
    #[serde(default)]
    pub id: i64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub slug: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Track {
    pub id: i64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub mix: String,
    #[serde(default)]
    pub slug: String,
    #[serde(default)]
    pub pos: i64,
    #[serde(default)]
    pub artists: Vec<String>,
    #[serde(default)]
    pub label: LabelRef,
}

#[derive(Debug, Deserialize)]
struct ChartPayload {
    key: String,
    tracks: Vec<Track>,
}

#[derive(Debug, Deserialize)]
struct LabelPayload {
    id: i64,
    #[serde(default)]
    bio: String,
    #[serde(default)]
    tracks: Vec<Track>,
}

fn cut(s: &str, max: usize) -> String {
    s.trim().chars().take(max).collect()
}

/// Slug as Beatport writes them; anything else never reaches a URL.
fn safe_slug(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(120).collect()
}

fn pct_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// (kind, json) of a hand-over URL, None for any other navigation.
pub fn parse_handover(url: &str) -> Option<(String, String)> {
    let rest = url.strip_prefix("https://pierscrm.invalid/bp?k=")?;
    let (kind, frag) = rest.split_once('#')?;
    if frag.len() > 400_000 {
        return None;
    }
    Some((kind.to_string(), pct_decode(frag)))
}

pub fn track_title(t: &Track) -> String {
    let name = cut(&t.name, 160);
    let mix = cut(&t.mix, 80);
    if mix.is_empty() || mix.eq_ignore_ascii_case("original mix") || mix.eq_ignore_ascii_case("extended mix") {
        name
    } else {
        format!("{name} ({mix})")
    }
}

fn track_url(t: &Track) -> String {
    format!("{SITE}/track/{}/{}", safe_slug(&t.slug), t.id)
}

pub fn chart_keys() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (slug, id, _) in GENRES {
        for (kind, page) in CHARTS {
            out.push((format!("{slug}:{kind}"), format!("/genre/{slug}/{id}/{page}")));
        }
    }
    out
}

fn is_distributor(name: &str) -> bool {
    let n = name.to_lowercase();
    NOT_A_LABEL.iter().any(|p| n.contains(p))
}

// ---------------------------------------------------------------------------
// Storing a reading
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Default)]
pub struct ScanSummary {
    pub charts_read: usize,
    pub labels_seen: usize,
    /// Labels the list did not know.
    pub added: usize,
}

/// "Tech House 3 · House 14": charted tracks per style, focus order.
fn chart_note(per_genre: &BTreeMap<usize, i64>) -> String {
    per_genre.iter().map(|(i, n)| format!("{} {}", GENRES[*i].2, n)).collect::<Vec<_>>().join(" · ")
}

/// Write the charts of one reading: the labels they hold (new ones join the
/// list, source Beatport), where each stands, and its charted tracks. The
/// previous reading is replaced. An archived label stays archived.
pub fn store_scan(conn: &mut Connection, charts: &HashMap<String, Vec<Track>>) -> Result<ScanSummary, String> {
    let wanted = chart_keys();
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM label_tracks WHERE kind = 'chart'", []).map_err(|e| e.to_string())?;
    tx.execute("UPDATE labels SET chart_note = NULL WHERE chart_note IS NOT NULL", []).map_err(|e| e.to_string())?;
    let mut sum = ScanSummary::default();
    // Beatport label id -> (ref, tracks per genre, rows)
    struct Seen<'a> {
        label: &'a LabelRef,
        per_genre: BTreeMap<usize, i64>,
        rows: Vec<(&'a str, &'a Track)>,
        ids: std::collections::HashSet<(usize, i64)>,
    }
    let mut seen: BTreeMap<i64, Seen> = BTreeMap::new();
    for (key, _) in &wanted {
        let Some(tracks) = charts.get(key) else { continue };
        sum.charts_read += 1;
        let genre = GENRES.iter().position(|g| key.starts_with(&format!("{}:", g.0))).unwrap_or(0);
        for t in tracks.iter().take(200) {
            if t.label.id <= 0 || t.id <= 0 || normalise(&t.label.name).is_empty() || is_distributor(&t.label.name) {
                continue;
            }
            let s = seen.entry(t.label.id).or_insert_with(|| Seen {
                label: &t.label,
                per_genre: BTreeMap::new(),
                rows: Vec::new(),
                ids: Default::default(),
            });
            // A track in both the top and the hype chart of a style counts once.
            if s.ids.insert((genre, t.id)) {
                *s.per_genre.entry(genre).or_default() += 1;
            }
            s.rows.push((key.as_str(), t));
        }
    }
    for (bp_id, s) in &seen {
        let name = cut(&s.label.name, 120);
        let norm = normalise(&name);
        let known: Option<i64> = tx
            .query_row(
                "SELECT id FROM labels WHERE beatport_id = ?1 OR name_norm = ?2 ORDER BY beatport_id = ?1 DESC LIMIT 1",
                params![bp_id, norm],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let genres = s.per_genre.keys().map(|i| GENRES[*i].2.to_lowercase()).collect::<Vec<_>>().join(", ");
        let id = match known {
            Some(id) => id,
            None => {
                tx.execute(
                    "INSERT INTO labels (name, name_norm, genres, source) VALUES (?1, ?2, ?3, 'Beatport')",
                    params![name, norm, genres],
                )
                .map_err(|e| e.to_string())?;
                sum.added += 1;
                tx.last_insert_rowid()
            }
        };
        let url = format!("{SITE}/label/{}/{}", safe_slug(&s.label.slug), bp_id);
        tx.execute(
            "UPDATE labels SET beatport_id = ?2, beatport_url = ?3, chart_note = ?4, chart_seen_at = datetime('now'),
                    genres = COALESCE(NULLIF(trim(genres), ''), ?5)
             WHERE id = ?1",
            params![id, bp_id, url, chart_note(&s.per_genre), genres],
        )
        .map_err(|e| e.to_string())?;
        for (key, t) in &s.rows {
            tx.execute(
                "INSERT INTO label_tracks (label_id, kind, chart, position, track_id, title, artists, url)
                 VALUES (?1, 'chart', ?2, ?3, ?4, ?5, ?6, ?7)",
                params![id, key, t.pos.clamp(1, 200), t.id, track_title(t), cut(&t.artists.join(", "), 200), track_url(t)],
            )
            .map_err(|e| e.to_string())?;
        }
    }
    sum.labels_seen = seen.len();
    tx.commit().map_err(|e| e.to_string())?;
    Ok(sum)
}

/// The top 10 of one label, read on its Beatport page.
pub fn store_top(conn: &mut Connection, label_id: i64, bio: &str, tracks: &[Track]) -> Result<usize, String> {
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM label_tracks WHERE label_id = ?1 AND kind = 'top'", params![label_id]).map_err(|e| e.to_string())?;
    let mut n = 0;
    for (i, t) in tracks.iter().take(20).enumerate() {
        if t.id <= 0 {
            continue;
        }
        tx.execute(
            "INSERT INTO label_tracks (label_id, kind, position, track_id, title, artists, url) VALUES (?1, 'top', ?2, ?3, ?4, ?5, ?6)",
            params![label_id, i as i64 + 1, t.id, track_title(t), cut(&t.artists.join(", "), 200), track_url(t)],
        )
        .map_err(|e| e.to_string())?;
        n += 1;
    }
    let bio = cut(bio, 4000);
    tx.execute(
        "UPDATE labels SET top_at = datetime('now'), bio = COALESCE(NULLIF(?2, ''), bio) WHERE id = ?1",
        params![label_id, bio],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(n)
}

// ---------------------------------------------------------------------------
// The window
// ---------------------------------------------------------------------------

/// What the open window was asked for.
enum Job {
    Charts(HashMap<String, Vec<Track>>),
    /// (label id in the list, Beatport id)
    Label(i64, i64),
}

static JOB: Mutex<Option<Job>> = Mutex::new(None);

#[derive(Debug, Clone, Serialize, Default)]
pub struct Progress {
    /// chart | done | label | closed | error
    pub kind: String,
    pub charts_read: usize,
    pub charts_total: usize,
    pub summary: Option<ScanSummary>,
    pub label_id: Option<i64>,
    pub tracks: usize,
    pub error: Option<String>,
}

fn close(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(WINDOW) {
        let _ = w.close();
    }
}

/// One hand-over from the page. Runs on the navigation callback: quick work
/// only, the window is closed from another thread.
fn on_handover(app: &AppHandle, kind: &str, json: &str) {
    let mut p = Progress { kind: kind.to_string(), charts_total: chart_keys().len(), ..Default::default() };
    let mut finished = false;
    {
        let mut job = JOB.lock().unwrap_or_else(|e| e.into_inner());
        match (kind, job.as_mut()) {
            ("chart", Some(Job::Charts(got))) => {
                let Ok(c) = serde_json::from_str::<ChartPayload>(json) else { return };
                if !chart_keys().iter().any(|(k, _)| *k == c.key) {
                    return;
                }
                got.insert(c.key, c.tracks);
                p.charts_read = got.len();
            }
            ("done", Some(Job::Charts(got))) => {
                finished = true;
                p.charts_read = got.len();
                if got.is_empty() {
                    p.kind = "error".into();
                    p.error = Some("Beatport n'a rendu aucun chart. Réessayez dans quelques minutes.".into());
                } else {
                    let res = app
                        .state::<AppState>()
                        .pool
                        .get()
                        .map_err(|e| e.to_string())
                        .and_then(|mut conn| store_scan(&mut conn, got));
                    match res {
                        Ok(s) => p.summary = Some(s),
                        Err(e) => {
                            p.kind = "error".into();
                            p.error = Some(e);
                        }
                    }
                }
            }
            ("label", Some(Job::Label(label_id, bp_id))) => {
                finished = true;
                let Ok(l) = serde_json::from_str::<LabelPayload>(json) else { return };
                if l.id != *bp_id {
                    return;
                }
                p.label_id = Some(*label_id);
                let res = app
                    .state::<AppState>()
                    .pool
                    .get()
                    .map_err(|e| e.to_string())
                    .and_then(|mut conn| store_top(&mut conn, *label_id, &l.bio, &l.tracks));
                match res {
                    Ok(n) => p.tracks = n,
                    Err(e) => {
                        p.kind = "error".into();
                        p.error = Some(e);
                    }
                }
            }
            _ => return,
        }
        if finished {
            *job = None;
        }
    }
    let _ = app.emit(EVENT, p);
    if finished {
        let app = app.clone();
        std::thread::spawn(move || close(&app));
    }
}

fn open(app: &AppHandle, job: Job, task: serde_json::Value, path: &str, title: &str) -> Result<(), String> {
    close(app);
    *JOB.lock().unwrap_or_else(|e| e.into_inner()) = Some(job);
    let js = SCAN_JS.replace("__TASK__", &task.to_string());
    let url = format!("{SITE}{path}").parse().map_err(|_| "Adresse Beatport invalide.".to_string())?;
    let nav_app = app.clone();
    let window = WebviewWindowBuilder::new(app, WINDOW, WebviewUrl::External(url))
        .title(title)
        .inner_size(1040.0, 760.0)
        .initialization_script(&js)
        .on_navigation(move |u| match parse_handover(u.as_str()) {
            Some((kind, json)) => {
                on_handover(&nav_app, &kind, &json);
                false
            }
            None => true,
        })
        .build()
        .map_err(|e| e.to_string())?;
    // Closed by hand before the end: the page waiting for it is told.
    let app = app.clone();
    window.on_window_event(move |e| {
        if matches!(e, tauri::WindowEvent::Destroyed) {
            let pending = JOB.lock().unwrap_or_else(|e| e.into_inner()).take().is_some();
            if pending {
                let _ = app.emit(EVENT, Progress { kind: "closed".into(), ..Default::default() });
            }
        }
    });
    Ok(())
}

/// "Scanner Beatport": opens the reading window on the charts of the focus
/// genres. Returns at once, the outcome comes as `labels:beatport` events.
#[tauri::command]
pub async fn labels_scan_beatport(app: AppHandle) -> Result<usize, String> {
    let keys = chart_keys();
    let charts: Vec<serde_json::Value> =
        keys.iter().map(|(k, p)| serde_json::json!({ "key": k, "path": p })).collect();
    let task = serde_json::json!({ "mode": "charts", "charts": charts });
    open(&app, Job::Charts(HashMap::new()), task, &keys[0].1, "Beatport, lecture des charts")?;
    Ok(keys.len())
}

/// The top 10 of one label, read on its Beatport page (one page view).
#[tauri::command]
pub async fn label_load_top(app: AppHandle, state: State<'_, AppState>, id: i64) -> Result<(), String> {
    let (bp_id, url): (Option<i64>, Option<String>) = {
        let conn = state.pool.get().map_err(|e| e.to_string())?;
        conn.query_row("SELECT beatport_id, beatport_url FROM labels WHERE id = ?1", params![id], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(|_| "Label introuvable.".to_string())?
    };
    let (Some(bp_id), Some(url)) = (bp_id, url) else {
        return Err("Ce label n'a pas de page Beatport connue. Lancez d'abord un scan Beatport.".into());
    };
    let path = url.strip_prefix(SITE).filter(|p| p.starts_with("/label/")).ok_or("Page Beatport invalide.")?.to_string();
    let task = serde_json::json!({ "mode": "label", "id": bp_id });
    open(&app, Job::Label(id, bp_id), task, &path, "Beatport, top du label")
}

// ---------------------------------------------------------------------------
// The card of a label
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct TrackRow {
    pub chart: String,
    pub position: i64,
    pub title: String,
    pub artists: String,
    pub url: String,
}

#[derive(Debug, Serialize)]
pub struct LabelCard {
    pub id: i64,
    pub bio: Option<String>,
    pub chart_seen_at: Option<String>,
    pub top_at: Option<String>,
    pub contact_source: Option<String>,
    pub contact_find_at: Option<String>,
    /// Tracks in the charts at the last reading, best position first.
    pub charted: Vec<TrackRow>,
    /// The label's top 10 on Beatport, once loaded.
    pub top: Vec<TrackRow>,
}

pub fn card(conn: &Connection, id: i64) -> Result<LabelCard, String> {
    let mut c = conn
        .query_row(
            "SELECT bio, chart_seen_at, top_at, contact_source, contact_find_at FROM labels WHERE id = ?1",
            params![id],
            |r| {
                Ok(LabelCard {
                    id,
                    bio: r.get(0)?,
                    chart_seen_at: r.get(1)?,
                    top_at: r.get(2)?,
                    contact_source: r.get(3)?,
                    contact_find_at: r.get(4)?,
                    charted: Vec::new(),
                    top: Vec::new(),
                })
            },
        )
        .map_err(|_| "Label introuvable.".to_string())?;
    let rows = |kind: &str| -> Vec<TrackRow> {
        conn.prepare(
            "SELECT chart, MIN(position), title, artists, url FROM label_tracks WHERE label_id = ?1 AND kind = ?2
             GROUP BY track_id ORDER BY MIN(position), id",
        )
        .and_then(|mut st| {
            st.query_map(params![id, kind], |r| {
                Ok(TrackRow { chart: r.get(0)?, position: r.get(1)?, title: r.get(2)?, artists: r.get(3)?, url: r.get(4)? })
            })
            .map(|it| it.flatten().collect())
        })
        .unwrap_or_default()
    };
    c.charted = rows("chart");
    c.top = rows("top");
    Ok(c)
}

#[tauri::command(async)]
pub fn label_card(state: State<'_, AppState>, id: i64) -> Result<LabelCard, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    card(&conn, id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::residency::testdb;

    fn track(id: i64, pos: i64, name: &str, label: (i64, &str)) -> Track {
        Track {
            id,
            name: name.into(),
            mix: "Original Mix".into(),
            slug: name.to_lowercase().replace(' ', "-"),
            pos,
            artists: vec!["Someone".into()],
            label: LabelRef { id: label.0, name: label.1.into(), slug: label.1.to_lowercase().replace(' ', "-") },
        }
    }

    #[test]
    fn handover_url_is_decoded_and_others_ignored() {
        let (k, j) = parse_handover("https://pierscrm.invalid/bp?k=chart#%7B%22a%22%3A%22Jo%C3%ABlla%22%7D").unwrap();
        assert_eq!(k, "chart");
        assert_eq!(j, "{\"a\":\"Joëlla\"}");
        assert!(parse_handover("https://www.beatport.com/genre/house/5/top-100").is_none());
        assert_eq!(pct_decode("100%"), "100%");
        assert_eq!(pct_decode("%zz%4"), "%zz%4");
    }

    #[test]
    fn a_reading_adds_new_labels_and_updates_known_ones() {
        let mut c = testdb::conn();
        c.execute("INSERT INTO labels (name, name_norm, email, archived) VALUES ('Toolroom', ?1, 'a@toolroom.test', 1)", params![normalise("Toolroom")])
            .unwrap();
        let mut charts = HashMap::new();
        charts.insert(
            "tech-house:top".to_string(),
            vec![track(1, 1, "One", (495, "Toolroom")), track(2, 2, "Two", (7598, "Cecille")), track(9, 3, "X", (5, "DistroKid"))],
        );
        // Same track in the hype chart: counted once for the style.
        charts.insert("tech-house:hype".to_string(), vec![track(2, 4, "Two", (7598, "Cecille"))]);
        charts.insert("house:top".to_string(), vec![track(3, 7, "Three", (7598, "Cecille"))]);
        charts.insert("not-a-chart".to_string(), vec![track(4, 1, "Four", (1, "Ghost"))]);
        let s = store_scan(&mut c, &charts).unwrap();
        assert_eq!((s.charts_read, s.labels_seen, s.added), (3, 2, 1));
        let (note, src, url): (String, String, String) = c
            .query_row("SELECT chart_note, source, beatport_url FROM labels WHERE name = 'Cecille'", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap();
        assert_eq!(note, "Tech House 1 · House 1");
        assert_eq!(src, "Beatport");
        assert_eq!(url, "https://www.beatport.com/label/cecille/7598");
        // The known label kept its address and its archive, and got its page.
        let (email, archived, bp): (String, i64, i64) =
            c.query_row("SELECT email, archived, beatport_id FROM labels WHERE name = 'Toolroom'", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
        assert_eq!((email.as_str(), archived, bp), ("a@toolroom.test", 1, 495));
        // A second reading replaces the first, nothing doubles.
        charts.remove("house:top");
        let s = store_scan(&mut c, &charts).unwrap();
        assert_eq!(s.added, 0);
        let id: i64 = c.query_row("SELECT id FROM labels WHERE name = 'Cecille'", [], |r| r.get(0)).unwrap();
        let card = card(&c, id).unwrap();
        assert_eq!(card.charted.len(), 1);
        assert_eq!(card.charted[0].position, 2);
        assert_eq!(card.charted[0].url, "https://www.beatport.com/track/two/2");
    }

    #[test]
    fn the_top_of_a_label_replaces_the_previous_one() {
        let mut c = testdb::conn();
        c.execute("INSERT INTO labels (name, name_norm) VALUES ('Cecille', 'cecille')", []).unwrap();
        let id = c.last_insert_rowid();
        store_top(&mut c, id, "", &[track(1, 1, "One", (7598, "Cecille")), track(2, 2, "Two", (7598, "Cecille"))]).unwrap();
        store_top(&mut c, id, "A bio", &[track(3, 1, "Three", (7598, "Cecille"))]).unwrap();
        let card = card(&c, id).unwrap();
        assert_eq!(card.top.len(), 1);
        assert_eq!(card.top[0].title, "Three");
        assert_eq!(card.bio.as_deref(), Some("A bio"));
    }
}
