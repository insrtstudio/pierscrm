//! Radar: source playlist curators (and later labels/A&R) from Spotify + Deezer,
//! then find their public submission emails ourselves by mining playlist
//! descriptions and deep-crawling their site/linktree. Reuses the venue-intel
//! run queue (vi_runs/vi_tasks) and the enrichment helpers. Only publicly shown
//! contact info is stored, always with its source_url.

use crate::commands::enrich::{
    base_candidates, crawl_client, extract_contacts, internal_links, normalize_site, origin_of,
    resolve_website_via_search, score_email, CRAWL_PATHS,
};
use crate::commands::ra::RaError;
use crate::commands::{deezer, spotify::Spotify};
use crate::db::{normalise, DbPool};
use crate::AppState;
use rusqlite::params;
use serde::Serialize;
use serde_json::json;
use tauri::{AppHandle, State};

fn setting(conn: &rusqlite::Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM settings WHERE key=?1", params![key], |r| {
        r.get::<_, String>(0)
    })
    .ok()
    .filter(|s| !s.trim().is_empty())
}

fn err(m: impl Into<String>, retryable: bool) -> RaError {
    RaError { message: m.into(), status: None, retryable }
}

/// Playlist networks that never hand out a submission email.
fn is_editorial(owner: &str) -> bool {
    let o = owner.to_lowercase();
    ["spotify", "deezer", "filtr", "topsify", "digster", "editor", "amazon", "apple"]
        .iter()
        .any(|k| o.contains(k))
}

/// Pull emails and a candidate site (linktree/url) out of free text.
fn mine_text(text: &str) -> (Vec<(String, i64)>, Option<String>) {
    let (emails, _) = extract_contacts(text);
    let scored: Vec<(String, i64)> = emails
        .into_iter()
        .map(|(e, _)| {
            let (s, _) = score_email(&e, false);
            (e, s)
        })
        .collect();
    // First URL that looks like a real contact hub.
    let mut site = None;
    for tok in text.split_whitespace() {
        let low = tok.to_lowercase();
        if low.starts_with("http") || low.contains("linktr.ee") || low.contains("linktree") {
            site = normalize_site(tok.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '/' && c != ':' && c != '.' && c != '-'));
            if site.is_some() {
                break;
            }
        }
    }
    (scored, site)
}

fn store_email(conn: &rusqlite::Connection, curator_id: i64, email: &str, score: i64, src: &str, method: &str) {
    let (_, role) = score_email(email, false);
    let _ = conn.execute(
        "INSERT OR IGNORE INTO radar_contacts (curator_id, type, valeur, role_devine, score, source_url, source_method)
         VALUES (?1,'email',?2,?3,?4,?5,?6)",
        params![curator_id, email, role, score, src, method],
    );
}

// ---------------- Harvest ----------------

pub async fn process_harvest_task(pool: &DbPool, payload: &str) -> Result<(), RaError> {
    let v: serde_json::Value = serde_json::from_str(payload).map_err(|e| err(e.to_string(), false))?;
    let source = v["source"].as_str().unwrap_or("");
    let genre = v["genre"].as_str().unwrap_or("").to_string();
    let limit = v["limit"].as_i64().unwrap_or(40);

    match source {
        "deezer" => {
            let pls = deezer::search_playlists(&genre, limit)
                .await
                .map_err(|e| err(format!("Deezer: {}", e), true))?;
            for p in pls {
                let owner = p.user.as_ref().and_then(|u| u.name.clone()).unwrap_or_default();
                let editorial = is_editorial(&owner);
                let desc = deezer::playlist_description(p.id).await;
                upsert_curator(
                    pool,
                    "deezer",
                    &p.id.to_string(),
                    p.title.as_deref().unwrap_or("?"),
                    &owner,
                    p.link.as_deref(),
                    p.fans,
                    p.nb_tracks,
                    &genre,
                    desc.as_deref(),
                    editorial,
                );
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
        }
        "spotify" => {
            let (cid, cs) = {
                let conn = pool.get().map_err(|e| err(e.to_string(), true))?;
                (setting(&conn, "spotify_client_id"), setting(&conn, "spotify_client_secret"))
            };
            let (cid, cs) = match (cid, cs) {
                (Some(a), Some(b)) => (a, b),
                _ => return Err(err("Spotify non configuré (Réglages > Pulse).", false)),
            };
            let sp = Spotify::new(cid, cs);
            let pls = sp
                .search_playlists(&genre, 50, 0)
                .await
                .map_err(|e| err(format!("Spotify: {}", e.0), true))?;
            for p in pls {
                let owner = p.owner.as_ref().and_then(|o| o.display_name.clone()).unwrap_or_default();
                let editorial = is_editorial(&owner);
                upsert_curator(
                    pool,
                    "spotify",
                    &p.id,
                    p.name.as_deref().unwrap_or("?"),
                    &owner,
                    p.external_urls.as_ref().and_then(|e| e.spotify.as_deref()),
                    p.followers.and_then(|f| f.total),
                    p.tracks.and_then(|t| t.total),
                    &genre,
                    p.description.as_deref(),
                    editorial,
                );
            }
        }
        _ => return Err(err("source inconnue", false)),
    }
    Ok(())
}

fn upsert_curator(
    pool: &DbPool,
    source: &str,
    ext_id: &str,
    name: &str,
    owner: &str,
    url: Option<&str>,
    followers: Option<i64>,
    nb_tracks: Option<i64>,
    genre: &str,
    description: Option<&str>,
    editorial: bool,
) {
    let Ok(conn) = pool.get() else { return };
    let norm = normalise(name);
    let (mined, site) = description.map(mine_text).unwrap_or_default();
    let _ = conn.execute(
        "INSERT INTO radar_curators (source, external_id, kind, nom, nom_normalise, owner_name, url, followers, nb_tracks, genre, description, site_web, editorial)
         VALUES (?1,?2,'playlist',?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)
         ON CONFLICT(source, external_id) DO UPDATE SET
            followers=excluded.followers, nb_tracks=excluded.nb_tracks,
            description=COALESCE(excluded.description, description),
            site_web=COALESCE(radar_curators.site_web, excluded.site_web),
            updated_at=datetime('now')",
        params![source, ext_id, name, norm, owner, url, followers, nb_tracks, genre, description, site, editorial as i64],
    );
    let cid: i64 = match conn.query_row(
        "SELECT id FROM radar_curators WHERE source=?1 AND external_id=?2",
        params![source, ext_id],
        |r| r.get(0),
    ) {
        Ok(x) => x,
        Err(_) => return,
    };
    // Emails found straight in the description (submission addresses).
    for (email, score) in mined {
        store_email(&conn, cid, &email, score, url.unwrap_or(""), "description");
    }
    qualify_one(&conn, cid);
}

fn qualify_one(conn: &rusqlite::Connection, cid: i64) {
    let _ = conn.execute(
        "UPDATE radar_curators SET
            score = MIN(100,
                CAST(COALESCE(followers,0) AS REAL) / 500
                + CASE WHEN EXISTS(SELECT 1 FROM radar_contacts c WHERE c.curator_id=radar_curators.id AND c.type='email') THEN 40 ELSE 0 END
                + CASE WHEN editorial=1 THEN 0 ELSE 10 END),
            statut = CASE
                WHEN statut IN ('valide','archive','rejete') THEN statut
                WHEN editorial=1 THEN 'candidat'
                WHEN EXISTS(SELECT 1 FROM radar_contacts c WHERE c.curator_id=radar_curators.id AND c.type='email') THEN 'qualifie'
                WHEN COALESCE(followers,0) >= 2000 THEN 'qualifie'
                ELSE 'candidat' END,
            updated_at=datetime('now')
         WHERE id=?1",
        params![cid],
    );
}

// ---------------- Enrich (find emails ourselves) ----------------

pub async fn process_enrich_task(pool: &DbPool, payload: &str) -> Result<(), RaError> {
    let v: serde_json::Value = serde_json::from_str(payload).map_err(|e| err(e.to_string(), false))?;
    let cid = v["curator_id"].as_i64().unwrap_or(0);

    let (nom, mut site, api_key): (String, Option<String>, Option<String>) = {
        let conn = pool.get().map_err(|e| err(e.to_string(), true))?;
        let row = conn
            .query_row(
                "SELECT nom, site_web FROM radar_curators WHERE id=?1",
                params![cid],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)),
            )
            .map_err(|e| err(e.to_string(), false))?;
        (row.0, row.1.and_then(|s| normalize_site(&s)), setting(&conn, "serper_api_key"))
    };

    // No site yet? Try to resolve one via web search (Serper), if a key is set.
    if site.is_none() {
        if let Some(key) = api_key {
            let query = format!("{} playlist curator submit demo contact", nom);
            if let Some(found) = resolve_website_via_search(&crawl_client(), &key, &query).await {
                site = Some(found);
            }
            tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        }
    }

    if let Some(raw) = site {
        let client = crawl_client();
        let mut base = origin_of(&raw);
        let mut home = String::new();
        for cand in base_candidates(&raw) {
            if let Ok(r) = client.get(cand.as_str()).send().await {
                if r.status().is_success() {
                    if let Ok(b) = r.text().await {
                        base = cand;
                        home = b;
                        break;
                    }
                }
            }
        }
        let mut urls: Vec<String> = CRAWL_PATHS.iter().map(|p| format!("{}{}", base, p)).collect();
        for l in internal_links(&home, &base) {
            if !urls.contains(&l) {
                urls.push(l);
            }
        }
        let mut fetches = 0;
        for url in &urls {
            if fetches >= 7 {
                break;
            }
            let body = if url == &base && !home.is_empty() {
                home.clone()
            } else {
                let Ok(r) = client.get(url.as_str()).send().await else { continue };
                if !r.status().is_success() {
                    continue;
                }
                fetches += 1;
                match r.text().await {
                    Ok(b) => b,
                    Err(_) => continue,
                }
            };
            let body = if body.len() > 300_000 { body[..300_000].to_string() } else { body };
            let (emails, _) = extract_contacts(&body);
            if let Ok(conn) = pool.get() {
                for (email, from_mailto) in emails {
                    let (score, _) = score_email(&email, from_mailto);
                    store_email(&conn, cid, &email, score, url, if from_mailto { "mailto" } else { "regex" });
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        if let Ok(conn) = pool.get() {
            let _ = conn.execute(
                "UPDATE radar_curators SET site_web=COALESCE(site_web, ?2), enriched_at=datetime('now') WHERE id=?1",
                params![cid, base],
            );
        }
    } else if let Ok(conn) = pool.get() {
        let _ = conn.execute("UPDATE radar_curators SET enriched_at=datetime('now') WHERE id=?1", params![cid]);
    }

    if let Ok(conn) = pool.get() {
        qualify_one(&conn, cid);
    }
    Ok(())
}

// ---------------- Commands: start runs ----------------

#[tauri::command]
pub async fn radar_harvest(
    app: AppHandle,
    state: State<'_, AppState>,
    genres: Vec<String>,
    sources: Vec<String>,
    limit: Option<i64>,
) -> Result<i64, String> {
    let genres: Vec<String> = genres.into_iter().map(|g| g.trim().to_string()).filter(|g| !g.is_empty()).collect();
    let sources: Vec<String> = sources.into_iter().filter(|s| s == "spotify" || s == "deezer").collect();
    if genres.is_empty() || sources.is_empty() {
        return Err("Choisissez au moins un genre et une source.".into());
    }
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO vi_runs (type, params, statut, started_at) VALUES ('radar_harvest', ?1, 'en_cours', datetime('now'))",
        params![json!({ "genres": genres, "sources": sources }).to_string()],
    )
    .map_err(|e| e.to_string())?;
    let run_id = conn.last_insert_rowid();
    let mut stmt = conn
        .prepare("INSERT INTO vi_tasks (run_id, type, payload, statut) VALUES (?1,'radar_harvest',?2,'en_attente')")
        .map_err(|e| e.to_string())?;
    for s in &sources {
        for g in &genres {
            stmt.execute(params![run_id, json!({ "source": s, "genre": g, "limit": limit.unwrap_or(40) }).to_string()])
                .map_err(|e| e.to_string())?;
        }
    }
    drop(stmt);
    crate::commands::harvest::spawn_worker(state.pool.clone(), state.cancels.clone(), app, run_id);
    Ok(run_id)
}

#[tauri::command]
pub async fn radar_enrich(app: AppHandle, state: State<'_, AppState>, force: Option<bool>) -> Result<i64, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let mut sql = String::from(
        "SELECT id FROM radar_curators WHERE editorial=0 AND statut != 'rejete'
         AND NOT EXISTS(SELECT 1 FROM radar_contacts c WHERE c.curator_id=radar_curators.id AND c.type='email')",
    );
    if !force.unwrap_or(false) {
        sql.push_str(" AND enriched_at IS NULL");
    }
    sql.push_str(" ORDER BY score DESC LIMIT 400");
    let ids: Vec<i64> = {
        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let rows = stmt.query_map([], |r| r.get::<_, i64>(0)).map_err(|e| e.to_string())?;
        let mut v = Vec::new();
        for r in rows {
            if let Ok(x) = r {
                v.push(x);
            }
        }
        v
    };
    if ids.is_empty() {
        return Err("Rien à enrichir (moissonne d'abord des curateurs).".into());
    }
    conn.execute(
        "INSERT INTO vi_runs (type, params, statut, started_at) VALUES ('radar_enrich', ?1, 'en_cours', datetime('now'))",
        params![json!({ "curators": ids.len() }).to_string()],
    )
    .map_err(|e| e.to_string())?;
    let run_id = conn.last_insert_rowid();
    let mut stmt = conn
        .prepare("INSERT INTO vi_tasks (run_id, type, payload, statut) VALUES (?1,'radar_enrich',?2,'en_attente')")
        .map_err(|e| e.to_string())?;
    for id in &ids {
        stmt.execute(params![run_id, json!({ "curator_id": id }).to_string()])
            .map_err(|e| e.to_string())?;
    }
    drop(stmt);
    crate::commands::harvest::spawn_worker(state.pool.clone(), state.cancels.clone(), app, run_id);
    Ok(run_id)
}

// ---------------- Reads ----------------

#[derive(Serialize)]
pub struct CuratorRow {
    pub id: i64,
    pub source: String,
    pub nom: String,
    pub owner_name: Option<String>,
    pub url: Option<String>,
    pub followers: Option<i64>,
    pub nb_tracks: Option<i64>,
    pub genre: Option<String>,
    pub statut: String,
    pub score: i64,
    pub editorial: bool,
    pub site_web: Option<String>,
    pub best_email: Option<String>,
    pub nb_emails: i64,
    pub enriched: bool,
    pub in_pipeline: bool,
}

#[tauri::command]
pub fn radar_list(
    state: State<AppState>,
    statut: Option<String>,
    source: Option<String>,
    genre: Option<String>,
    search: Option<String>,
    has_email: Option<bool>,
    hide_editorial: Option<bool>,
    limit: Option<i64>,
) -> Result<Vec<CuratorRow>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let mut sql = String::from(
        "SELECT c.id, c.source, c.nom, c.owner_name, c.url, c.followers, c.nb_tracks, c.genre, c.statut, c.score, c.editorial, c.site_web,
                (SELECT valeur FROM radar_contacts x WHERE x.curator_id=c.id AND x.type='email' ORDER BY x.score DESC LIMIT 1),
                (SELECT COUNT(*) FROM radar_contacts x WHERE x.curator_id=c.id AND x.type='email'),
                (c.enriched_at IS NOT NULL),
                (c.crm_contact_id IS NOT NULL)
         FROM radar_curators c WHERE 1=1",
    );
    let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    if let Some(s) = statut.filter(|s| !s.is_empty() && s != "all") {
        sql.push_str(" AND c.statut=?");
        args.push(Box::new(s));
    }
    if let Some(s) = source.filter(|s| !s.is_empty() && s != "all") {
        sql.push_str(" AND c.source=?");
        args.push(Box::new(s));
    }
    if let Some(g) = genre.filter(|s| !s.is_empty() && s != "all") {
        sql.push_str(" AND c.genre=?");
        args.push(Box::new(g));
    }
    if let Some(q) = search.filter(|s| !s.trim().is_empty()) {
        sql.push_str(" AND (c.nom LIKE ? OR c.owner_name LIKE ?)");
        let like = format!("%{}%", q.trim());
        args.push(Box::new(like.clone()));
        args.push(Box::new(like));
    }
    if has_email == Some(true) {
        sql.push_str(" AND EXISTS(SELECT 1 FROM radar_contacts x WHERE x.curator_id=c.id AND x.type='email')");
    }
    if hide_editorial == Some(true) {
        sql.push_str(" AND c.editorial=0");
    }
    sql.push_str(" ORDER BY c.score DESC, c.followers DESC");
    sql.push_str(&format!(" LIMIT {}", limit.unwrap_or(400).clamp(1, 1000)));

    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();
    let rows = stmt
        .query_map(refs.as_slice(), |r| {
            Ok(CuratorRow {
                id: r.get(0)?,
                source: r.get(1)?,
                nom: r.get(2)?,
                owner_name: r.get(3)?,
                url: r.get(4)?,
                followers: r.get(5)?,
                nb_tracks: r.get(6)?,
                genre: r.get(7)?,
                statut: r.get(8)?,
                score: r.get(9)?,
                editorial: r.get::<_, i64>(10)? != 0,
                site_web: r.get(11)?,
                best_email: r.get(12)?,
                nb_emails: r.get(13)?,
                enriched: r.get::<_, i64>(14)? != 0,
                in_pipeline: r.get::<_, i64>(15)? != 0,
            })
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

#[derive(Serialize)]
pub struct RadarStats {
    pub total: i64,
    pub qualifie: i64,
    pub with_email: i64,
    pub editorial: i64,
    pub in_pipeline: i64,
    pub genres: Vec<(String, i64)>,
}

#[tauri::command]
pub fn radar_stats(state: State<AppState>) -> Result<RadarStats, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let one = |sql: &str| conn.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap_or(0);
    let mut genres = Vec::new();
    if let Ok(mut s) = conn.prepare(
        "SELECT COALESCE(genre,'?'), COUNT(*) n FROM radar_curators GROUP BY genre ORDER BY n DESC LIMIT 10",
    ) {
        if let Ok(rows) = s.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))) {
            genres = rows.filter_map(|r| r.ok()).collect();
        }
    }
    Ok(RadarStats {
        total: one("SELECT COUNT(*) FROM radar_curators"),
        qualifie: one("SELECT COUNT(*) FROM radar_curators WHERE statut='qualifie'"),
        with_email: one("SELECT COUNT(DISTINCT curator_id) FROM radar_contacts WHERE type='email'"),
        editorial: one("SELECT COUNT(*) FROM radar_curators WHERE editorial=1"),
        in_pipeline: one("SELECT COUNT(*) FROM radar_curators WHERE crm_contact_id IS NOT NULL"),
        genres,
    })
}

/// Promote a curator into the CRM contacts pipeline (category 'curator').
#[tauri::command]
pub fn radar_promote(state: State<AppState>, id: i64) -> Result<i64, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let (nom, url): (String, Option<String>) = conn
        .query_row("SELECT nom, url FROM radar_curators WHERE id=?1", params![id], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(|e| e.to_string())?;
    let email: Option<String> = conn
        .query_row(
            "SELECT valeur FROM radar_contacts WHERE curator_id=?1 AND type='email' ORDER BY score DESC LIMIT 1",
            params![id],
            |r| r.get(0),
        )
        .ok();
    conn.execute(
        "INSERT INTO contacts (category, name, email, website, status, reason)
         VALUES ('curator', ?1, ?2, ?3, 'to_contact', 'Radar')",
        params![nom, email, url],
    )
    .map_err(|e| e.to_string())?;
    let contact_id = conn.last_insert_rowid();
    conn.execute("UPDATE radar_curators SET crm_contact_id=?2, statut='valide' WHERE id=?1", params![id, contact_id])
        .map_err(|e| e.to_string())?;
    Ok(contact_id)
}
