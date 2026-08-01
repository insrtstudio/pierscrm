//! Pulse module: daily Spotify snapshots (artist/track popularity, playlist
//! followers) + read-only Meta Ads daily spend, so campaign impact is visible
//! as an overlay. Desktop adaptation of the "intellijend" concept: instead of
//! server crons, a snapshot runs at app launch (if none exists for today, UTC)
//! plus a manual button. All upserts are idempotent (ON CONFLICT DO UPDATE).

use crate::commands::meta;
use crate::commands::spotify::{parse_spotify_id, Spotify};
use crate::db::DbPool;
use crate::AppState;
use rusqlite::params;
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

fn setting(conn: &rusqlite::Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM settings WHERE key=?1", params![key], |r| {
        r.get::<_, String>(0)
    })
    .ok()
    .filter(|s| !s.trim().is_empty())
}

// ---------------- Snapshot run ----------------

#[derive(Serialize, Clone, Default)]
pub struct PulseReport {
    pub artists: i64,
    pub tracks: i64,
    pub playlists: i64,
    pub spend_rows: i64,
    pub errors: Vec<String>,
    pub ran: bool,
}

async fn run_snapshot(pool: &DbPool) -> PulseReport {
    let mut report = PulseReport { ran: true, ..Default::default() };
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();

    // ---- Spotify half ----
    let creds = {
        let conn = pool.get().ok();
        conn.map(|c| (setting(&c, "spotify_client_id"), setting(&c, "spotify_client_secret")))
    };
    if let Some((Some(id), Some(secret))) = creds {
        let sp = Spotify::new(id, secret);

        // Artists: every profile with a spotify_artist_id.
        let artist_ids: Vec<String> = pool
            .get()
            .ok()
            .map(|c| {
                let mut stmt = c
                    .prepare("SELECT DISTINCT spotify_artist_id FROM artists WHERE spotify_artist_id IS NOT NULL AND trim(spotify_artist_id) != ''")
                    .ok()?;
                let rows = stmt.query_map([], |r| r.get::<_, String>(0)).ok()?;
                Some(rows.filter_map(|r| r.ok()).collect::<Vec<_>>())
            })
            .flatten()
            .unwrap_or_default();
        for raw in &artist_ids {
            // Accept a pasted artist URL or URI as well as the bare id.
            let aid: String = parse_spotify_id(raw).map(|(_, id)| id).unwrap_or_else(|| raw.trim().into());
            let aid = &aid;
            match sp.artist(aid).await {
                Ok(a) => {
                    if let Ok(conn) = pool.get() {
                        let _ = conn.execute(
                            "INSERT INTO pulse_artist_snapshots (artist_spotify_id, snapshot_date, popularity, followers)
                             VALUES (?1,?2,?3,?4)
                             ON CONFLICT(artist_spotify_id, snapshot_date) DO UPDATE SET
                                popularity=excluded.popularity, followers=excluded.followers",
                            params![aid.trim(), today, a.popularity, a.followers.and_then(|f| f.total)],
                        );
                        report.artists += 1;
                    }
                }
                Err(e) => report.errors.push(format!("artiste {}: {}", aid, e.0)),
            }
        }

        // Tracked tracks, batched by 50.
        let track_ids: Vec<String> = pool
            .get()
            .ok()
            .map(|c| {
                let mut stmt = c
                    .prepare("SELECT track_spotify_id FROM pulse_tracked_tracks WHERE is_active=1")
                    .ok()?;
                let rows = stmt.query_map([], |r| r.get::<_, String>(0)).ok()?;
                Some(rows.filter_map(|r| r.ok()).collect::<Vec<_>>())
            })
            .flatten()
            .unwrap_or_default();
        if !track_ids.is_empty() {
            match sp.tracks(&track_ids).await {
                Ok(tracks) => {
                    if let Ok(conn) = pool.get() {
                        for t in tracks {
                            let _ = conn.execute(
                                "INSERT INTO pulse_track_snapshots (track_spotify_id, track_name, snapshot_date, popularity)
                                 VALUES (?1,?2,?3,?4)
                                 ON CONFLICT(track_spotify_id, snapshot_date) DO UPDATE SET
                                    popularity=excluded.popularity, track_name=excluded.track_name",
                                params![t.id, t.name, today, t.popularity],
                            );
                            report.tracks += 1;
                        }
                    }
                }
                Err(e) => report.errors.push(format!("tracks: {}", e.0)),
            }
        }

        // Watchlist playlists.
        let playlists: Vec<(i64, String)> = pool
            .get()
            .ok()
            .map(|c| {
                let mut stmt = c
                    .prepare("SELECT id, playlist_spotify_id FROM pulse_playlist_watchlist WHERE is_active=1")
                    .ok()?;
                let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).ok()?;
                Some(rows.filter_map(|r| r.ok()).collect::<Vec<_>>())
            })
            .flatten()
            .unwrap_or_default();
        for (pid, sid) in &playlists {
            match sp.playlist(sid.trim()).await {
                Ok(p) => {
                    if let Ok(conn) = pool.get() {
                        let contains: bool = p
                            .tracks
                            .as_ref()
                            .map(|tr| {
                                tr.items.iter().any(|it| {
                                    it.track
                                        .as_ref()
                                        .and_then(|t| t.id.as_ref())
                                        .map(|id| track_ids.iter().any(|x| x == id))
                                        .unwrap_or(false)
                                })
                            })
                            .unwrap_or(false);
                        let _ = conn.execute(
                            "INSERT INTO pulse_playlist_snapshots (playlist_id, snapshot_date, followers, track_count, contains_our_track)
                             VALUES (?1,?2,?3,?4,?5)
                             ON CONFLICT(playlist_id, snapshot_date) DO UPDATE SET
                                followers=excluded.followers, track_count=excluded.track_count,
                                contains_our_track=excluded.contains_our_track",
                            params![
                                pid, today,
                                p.followers.and_then(|f| f.total),
                                p.tracks.as_ref().and_then(|t| t.total),
                                contains
                            ],
                        );
                        // Keep name/owner fresh.
                        let _ = conn.execute(
                            "UPDATE pulse_playlist_watchlist SET name=COALESCE(?2,name), owner_name=COALESCE(?3,owner_name) WHERE id=?1",
                            params![pid, p.name, p.owner.and_then(|o| o.display_name)],
                        );
                        report.playlists += 1;
                    }
                }
                Err(e) => report.errors.push(format!("playlist {}: {}", sid, e.0)),
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
    } else {
        report
            .errors
            .push("Spotify non configuré (client id/secret dans Réglages).".into());
    }

    // ---- Meta half (never blocks the Spotify half) ----
    let meta_cfg = {
        let conn = pool.get().ok();
        conn.map(|c| {
            (
                setting(&c, "meta_access_token"),
                setting(&c, "meta_ad_account_id"),
                setting(&c, "meta_result_action_type").unwrap_or_else(|| "link_click".into()),
            )
        })
    };
    if let Some((Some(token), Some(account), action_type)) = meta_cfg {
        match meta::fetch_daily_spend(&token, &account, &action_type, 3).await {
            Ok(rows) => {
                if let Ok(conn) = pool.get() {
                    for r in rows {
                        if r.campaign_id.is_empty() || r.spend_date.is_empty() {
                            continue;
                        }
                        let _ = conn.execute(
                            "INSERT INTO pulse_meta_spend (campaign_id, campaign_name, spend_date, spend, impressions, clicks, results, cost_per_result)
                             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)
                             ON CONFLICT(campaign_id, spend_date) DO UPDATE SET
                                spend=excluded.spend, impressions=excluded.impressions, clicks=excluded.clicks,
                                results=excluded.results, cost_per_result=excluded.cost_per_result,
                                campaign_name=excluded.campaign_name",
                            params![r.campaign_id, r.campaign_name, r.spend_date, r.spend, r.impressions, r.clicks, r.results, r.cost_per_result],
                        );
                        report.spend_rows += 1;
                    }
                }
            }
            Err(e) => report.errors.push(format!("Meta: {}", e.0)),
        }
    }

    if let Ok(conn) = pool.get() {
        let _ = conn.execute(
            "INSERT INTO settings (key, value) VALUES ('pulse_last_snapshot', ?1)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![today],
        );
    }
    report
}

/// Manual snapshot (button in the Pulse page).
#[tauri::command]
pub async fn pulse_snapshot(state: State<'_, AppState>) -> Result<PulseReport, String> {
    Ok(run_snapshot(&state.pool).await)
}

/// Auto snapshot at app launch: runs only if none was taken today (UTC).
/// Spawned from setup; emits `pulse:snapshot-done` when it actually ran.
pub fn auto_snapshot_on_launch(pool: DbPool, app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let (configured, last) = match pool.get() {
            Ok(c) => (
                setting(&c, "spotify_client_id").is_some(),
                setting(&c, "pulse_last_snapshot"),
            ),
            Err(_) => (false, None),
        };
        if !configured || last.as_deref() == Some(today.as_str()) {
            return;
        }
        // Small delay so app startup stays snappy.
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        let report = run_snapshot(&pool).await;
        let _ = app.emit("pulse:snapshot-done", report);
    });
}

// ---------------- Reads: KPIs + series ----------------

#[derive(Serialize)]
pub struct PulseKpis {
    pub artist_popularity: Option<i64>,
    pub artist_delta7: Option<i64>,
    pub artist_followers: Option<i64>,
    pub best_track_name: Option<String>,
    pub best_track_popularity: Option<i64>,
    pub best_track_delta7: Option<i64>,
    pub spend_30d: f64,
    pub cpr_7d: Option<f64>,
    pub cpr_30d: Option<f64>,
    pub last_snapshot: Option<String>,
}

#[tauri::command]
pub fn pulse_kpis(state: State<AppState>, artist_spotify_id: Option<String>) -> Result<PulseKpis, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let aid = artist_spotify_id
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            conn.query_row(
                "SELECT spotify_artist_id FROM artists WHERE spotify_artist_id IS NOT NULL AND trim(spotify_artist_id) != '' ORDER BY id LIMIT 1",
                [],
                |r| r.get::<_, String>(0),
            )
            .ok()
        })
        .unwrap_or_default();

    let latest = |sql: &str, p: &[&dyn rusqlite::ToSql]| -> Option<i64> {
        conn.query_row(sql, p, |r| r.get::<_, Option<i64>>(0)).ok().flatten()
    };
    let artist_popularity = latest(
        "SELECT popularity FROM pulse_artist_snapshots WHERE artist_spotify_id=?1 ORDER BY snapshot_date DESC LIMIT 1",
        &[&aid],
    );
    let artist_pop_7 = latest(
        "SELECT popularity FROM pulse_artist_snapshots WHERE artist_spotify_id=?1 AND snapshot_date <= date('now','-7 day') ORDER BY snapshot_date DESC LIMIT 1",
        &[&aid],
    );
    let artist_followers = latest(
        "SELECT followers FROM pulse_artist_snapshots WHERE artist_spotify_id=?1 ORDER BY snapshot_date DESC LIMIT 1",
        &[&aid],
    );

    // Best track by latest popularity.
    let best: Option<(String, String, i64)> = conn
        .query_row(
            "SELECT s.track_spotify_id, COALESCE(s.track_name, s.track_spotify_id), s.popularity
             FROM pulse_track_snapshots s
             WHERE s.snapshot_date = (SELECT MAX(snapshot_date) FROM pulse_track_snapshots)
             ORDER BY s.popularity DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .ok();
    let (best_track_name, best_track_popularity, best_track_delta7) = match best {
        Some((tid, name, pop)) => {
            let old = latest(
                "SELECT popularity FROM pulse_track_snapshots WHERE track_spotify_id=?1 AND snapshot_date <= date('now','-7 day') ORDER BY snapshot_date DESC LIMIT 1",
                &[&tid],
            );
            (Some(name), Some(pop), old.map(|o| pop - o))
        }
        None => (None, None, None),
    };

    let spend_30d: f64 = conn
        .query_row(
            "SELECT COALESCE(SUM(spend),0) FROM pulse_meta_spend WHERE spend_date >= date('now','-30 day')",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0.0);
    let cpr = |days: i64| -> Option<f64> {
        conn.query_row(
            &format!(
                "SELECT SUM(spend), SUM(results) FROM pulse_meta_spend WHERE spend_date >= date('now','-{} day') AND results IS NOT NULL",
                days
            ),
            [],
            |r| Ok((r.get::<_, Option<f64>>(0)?, r.get::<_, Option<i64>>(1)?)),
        )
        .ok()
        .and_then(|(s, n)| match (s, n) {
            (Some(s), Some(n)) if n > 0 => Some(s / n as f64),
            _ => None,
        })
    };

    Ok(PulseKpis {
        artist_popularity,
        artist_delta7: match (artist_popularity, artist_pop_7) {
            (Some(a), Some(b)) => Some(a - b),
            _ => None,
        },
        artist_followers,
        best_track_name,
        best_track_popularity,
        best_track_delta7,
        spend_30d,
        cpr_7d: cpr(7),
        cpr_30d: cpr(30),
        last_snapshot: setting(&conn, "pulse_last_snapshot"),
    })
}

#[derive(Serialize)]
pub struct PulsePoint {
    pub date: String,
    pub artist_pop: Option<i64>,
    pub track_pop: Option<i64>,
    pub spend: f64,
}

#[derive(Serialize)]
pub struct PulseSeries {
    pub points: Vec<PulsePoint>,
    pub releases: Vec<(String, String)>, // (date, name)
}

#[tauri::command]
pub fn pulse_series(
    state: State<AppState>,
    artist_spotify_id: Option<String>,
    track_spotify_id: Option<String>,
    days: Option<i64>,
) -> Result<PulseSeries, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let days = days.unwrap_or(90).clamp(7, 365);
    let aid = artist_spotify_id.unwrap_or_default();
    let tid = track_spotify_id.unwrap_or_default();

    let mut points: Vec<PulsePoint> = Vec::new();
    let mut stmt = conn
        .prepare(&format!(
            "WITH RECURSIVE dates(d) AS (
                SELECT date('now','-{} day') UNION ALL SELECT date(d,'+1 day') FROM dates WHERE d < date('now')
             )
             SELECT d,
                (SELECT popularity FROM pulse_artist_snapshots a WHERE a.artist_spotify_id=?1 AND a.snapshot_date=d),
                (SELECT popularity FROM pulse_track_snapshots t WHERE t.track_spotify_id=?2 AND t.snapshot_date=d),
                COALESCE((SELECT SUM(spend) FROM pulse_meta_spend m WHERE m.spend_date=d), 0)
             FROM dates",
            days
        ))
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![aid, tid], |r| {
            Ok(PulsePoint {
                date: r.get(0)?,
                artist_pop: r.get(1)?,
                track_pop: r.get(2)?,
                spend: r.get(3)?,
            })
        })
        .map_err(|e| e.to_string())?;
    for r in rows {
        points.push(r.map_err(|e| e.to_string())?);
    }

    let mut releases = Vec::new();
    let mut stmt = conn
        .prepare(
            "SELECT release_date, COALESCE(name, track_spotify_id) FROM pulse_tracked_tracks
             WHERE release_date IS NOT NULL AND release_date >= date('now', ?1)",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![format!("-{} day", days)], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(|e| e.to_string())?;
    for r in rows {
        releases.push(r.map_err(|e| e.to_string())?);
    }

    Ok(PulseSeries { points, releases })
}

// ---------------- Tracked tracks ----------------

#[derive(Serialize)]
pub struct TrackedTrack {
    pub id: i64,
    pub track_spotify_id: String,
    pub name: Option<String>,
    pub release_date: Option<String>,
    pub is_active: bool,
    pub latest_popularity: Option<i64>,
}

#[tauri::command]
pub fn pulse_tracked_list(state: State<AppState>) -> Result<Vec<TrackedTrack>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT t.id, t.track_spotify_id, t.name, t.release_date, t.is_active,
                    (SELECT popularity FROM pulse_track_snapshots s WHERE s.track_spotify_id=t.track_spotify_id ORDER BY snapshot_date DESC LIMIT 1)
             FROM pulse_tracked_tracks t ORDER BY t.is_active DESC, t.release_date DESC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(TrackedTrack {
                id: r.get(0)?,
                track_spotify_id: r.get(1)?,
                name: r.get(2)?,
                release_date: r.get(3)?,
                is_active: r.get::<_, i64>(4)? != 0,
                latest_popularity: r.get(5)?,
            })
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

#[tauri::command]
pub async fn pulse_tracked_add(
    state: State<'_, AppState>,
    input: String,
    artist_id: Option<i64>,
) -> Result<(), String> {
    let (kind, id) = parse_spotify_id(&input).ok_or("Lien ou id Spotify invalide")?;
    if kind != "track" && kind != "unknown" {
        return Err(format!("Ce lien est un {} Spotify, pas un track.", kind));
    }
    // Fetch name + release date when credentials are set (best effort).
    let creds = {
        let conn = state.pool.get().map_err(|e| e.to_string())?;
        (setting(&conn, "spotify_client_id"), setting(&conn, "spotify_client_secret"))
    };
    let (name, release) = if let (Some(cid), Some(cs)) = creds {
        match Spotify::new(cid, cs).track(&id).await {
            Ok(t) => (t.name, t.album.and_then(|a| a.release_date)),
            Err(_) => (None, None),
        }
    } else {
        (None, None)
    };
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO pulse_tracked_tracks (artist_id, track_spotify_id, name, release_date)
         VALUES (?1,?2,?3,?4)
         ON CONFLICT(track_spotify_id) DO UPDATE SET is_active=1, name=COALESCE(excluded.name, name)",
        params![artist_id, id, name, release],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn pulse_tracked_toggle(state: State<AppState>, id: i64, active: bool) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE pulse_tracked_tracks SET is_active=?2 WHERE id=?1",
        params![id, active],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

// ---------------- Playlist watchlist ----------------

#[derive(Serialize)]
pub struct WatchRow {
    pub id: i64,
    pub playlist_spotify_id: String,
    pub name: Option<String>,
    pub owner_name: Option<String>,
    pub notes: Option<String>,
    pub is_active: bool,
    pub followers: Option<i64>,
    pub delta7: Option<i64>,
    pub delta30: Option<i64>,
    pub contains_our_track: bool,
    pub spark: Vec<i64>,
}

#[tauri::command]
pub fn pulse_watchlist(state: State<AppState>) -> Result<Vec<WatchRow>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT w.id, w.playlist_spotify_id, w.name, w.owner_name, w.notes, w.is_active,
                (SELECT followers FROM pulse_playlist_snapshots s WHERE s.playlist_id=w.id ORDER BY snapshot_date DESC LIMIT 1),
                (SELECT followers FROM pulse_playlist_snapshots s WHERE s.playlist_id=w.id AND snapshot_date <= date('now','-7 day') ORDER BY snapshot_date DESC LIMIT 1),
                (SELECT followers FROM pulse_playlist_snapshots s WHERE s.playlist_id=w.id AND snapshot_date <= date('now','-30 day') ORDER BY snapshot_date DESC LIMIT 1),
                COALESCE((SELECT contains_our_track FROM pulse_playlist_snapshots s WHERE s.playlist_id=w.id ORDER BY snapshot_date DESC LIMIT 1), 0)
             FROM pulse_playlist_watchlist w
             ORDER BY w.is_active DESC, w.created_at DESC",
        )
        .map_err(|e| e.to_string())?;
    let base: Vec<(i64, String, Option<String>, Option<String>, Option<String>, bool, Option<i64>, Option<i64>, Option<i64>, bool)> = stmt
        .query_map([], |r| {
            Ok((
                r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?,
                r.get::<_, i64>(5)? != 0,
                r.get(6)?, r.get(7)?, r.get(8)?,
                r.get::<_, i64>(9)? != 0,
            ))
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();

    let mut out = Vec::new();
    for (id, sid, name, owner, notes, active, cur, f7, f30, contains) in base {
        let mut spark = Vec::new();
        if let Ok(mut s) = conn.prepare(
            "SELECT followers FROM pulse_playlist_snapshots WHERE playlist_id=?1 AND followers IS NOT NULL ORDER BY snapshot_date DESC LIMIT 30",
        ) {
            if let Ok(rows) = s.query_map(params![id], |r| r.get::<_, i64>(0)) {
                spark = rows.filter_map(|r| r.ok()).collect();
                spark.reverse();
            }
        }
        out.push(WatchRow {
            id,
            playlist_spotify_id: sid,
            name,
            owner_name: owner,
            notes,
            is_active: active,
            followers: cur,
            delta7: match (cur, f7) {
                (Some(a), Some(b)) => Some(a - b),
                _ => None,
            },
            delta30: match (cur, f30) {
                (Some(a), Some(b)) => Some(a - b),
                _ => None,
            },
            contains_our_track: contains,
            spark,
        });
    }
    Ok(out)
}

#[tauri::command]
pub async fn pulse_watchlist_add(state: State<'_, AppState>, input: String) -> Result<(), String> {
    let (kind, id) = parse_spotify_id(&input).ok_or("Lien ou id Spotify invalide")?;
    if kind != "playlist" && kind != "unknown" {
        return Err(format!("Ce lien est un {} Spotify, pas une playlist.", kind));
    }
    let creds = {
        let conn = state.pool.get().map_err(|e| e.to_string())?;
        (setting(&conn, "spotify_client_id"), setting(&conn, "spotify_client_secret"))
    };
    let (name, owner) = if let (Some(cid), Some(cs)) = creds {
        match Spotify::new(cid, cs).playlist(&id).await {
            Ok(p) => (p.name, p.owner.and_then(|o| o.display_name)),
            Err(e) => return Err(format!("Playlist introuvable: {}", e.0)),
        }
    } else {
        (None, None)
    };
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO pulse_playlist_watchlist (playlist_spotify_id, name, owner_name)
         VALUES (?1,?2,?3)
         ON CONFLICT(playlist_spotify_id) DO UPDATE SET is_active=1",
        params![id, name, owner],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn pulse_watchlist_toggle(state: State<AppState>, id: i64, active: bool) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE pulse_playlist_watchlist SET is_active=?2 WHERE id=?1",
        params![id, active],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
