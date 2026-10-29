//! Radar: source playlist curators (and later labels/A&R) from Spotify + Deezer,
//! then find their public submission emails ourselves by mining playlist
//! descriptions and deep-crawling their site/linktree. Reuses the venue-intel
//! run queue (vi_runs/vi_tasks) and the enrichment helpers. Only publicly shown
//! contact info is stored, always with its source_url.

use crate::commands::enrich::{
    base_candidates, clean_email, crawl_client, extract_contacts, host_in, host_of, internal_links,
    is_non_pitch_email, is_placeholder_email, normalize_site, origin_of, resolve_website_via_search,
    score_email, CRAWL_PATHS,
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
    // Routed through the Keychain for secret keys (see crate::secrets).
    crate::secrets::read_setting(conn, key).filter(|s| !s.trim().is_empty())
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

/// Corporate domains of the streaming platforms themselves. An address sitting
/// here is on the platform's own domain, which is what separates a real
/// editorial contact from the "spotify.promo@gmail.com" pitch scams that plague
/// playlist submission.
///
/// This checks the domain and nothing else: it does not prove the mailbox
/// exists, that it accepts submissions, or that anyone reads it. The UI must say
/// "official domain", never "verified".
///
/// Bare youtube.com is deliberately absent. A YouTube channel is an ordinary
/// curator like any other; only YouTube Music is a platform.
const PLATFORM_DOMAINS: &[&str] = &[
    "spotify.com",
    "deezer.com",
    "apple.com",
    "amazon.com",
    "music.youtube.com",
    "youtubemusic.com",
    "tidal.com",
    "soundcloud.com",
    "beatport.com",
    "audiomack.com",
    "anghami.com",
    "boomplay.com",
];

/// Mailboxes that are never a route to a human who books music: compliance,
/// support and automated addresses.
///
/// This is not cosmetic. Platforms publish these in the footer of every page, so
/// crawling a Deezer-hosted playlist harvested `privacy@deezer.com`,
/// `support@deezer.com` and `dsa-authorities@deezer.com` and filed them as that
/// curator's contact: 117 such rows here, and for 17 curators it was the only
/// address on file. They inflated the "reachable" count and would have been
/// promoted straight into an email campaign.
pub(crate) const NON_PITCH_LOCALS: &[&str] = &[
    "privacy", "legal", "support", "help", "dsa", "dpo", "gdpr", "abuse", "security", "noreply",
    "no-reply", "donotreply", "webmaster", "postmaster", "copyright", "dmca", "compliance",
    "billing", "invoice", "unsubscribe",
];

/// Template addresses and scraping artefacts that are never a real mailbox:
/// `your@email.com` copied from a form placeholder, and `u003ecommunity@ra.co`
/// where a JSON-escaped `>` got glued onto the local part.
pub(crate) const PLACEHOLDER_SQL: &[&str] = &[
    "'your@%'", "'youremail@%'", "'name@%'", "'email@%'", "'example@%'", "'test@%'",
    "'%@example.com'", "'%@email.com'", "'%@domain.com'", "'%@yourdomain.com'",
    "'u003e%'", "'u003c%'", "'%@%sentry%'", "'%bug-reporting%'", "'%wixpress.com'",
];

/// An address found on this many distinct curators or more was scraped off an
/// aggregator page (a directory listing dozens of playlists), not off the
/// curator's own site. In the real data `support@playlistlookup.com` sat on 93
/// curators and the 20 addresses of one directory page on 7 each: every one of
/// those counted as "reachable" and was promoted into contacts, so a campaign
/// would have written to the same directory 93 times.
const SHARED_EMAIL_MIN: i64 = 3;

/// SQL boolean over `col`, true when the address could plausibly reach someone
/// who listens to submissions.
fn pitchable_sql(col: &str) -> String {
    let mut parts: Vec<String> = NON_PITCH_LOCALS
        .iter()
        // Exact local part plus `dsa-authorities@`, `privacy.team@`, `no_reply@`...
        .map(|l| {
            format!(
                "lower({col}) LIKE '{l}@%' OR lower({col}) LIKE '{l}-%@%' \
                 OR lower({col}) LIKE '{l}.%@%' OR lower({col}) LIKE '{l}\\_%@%' ESCAPE '\\'"
            )
        })
        .collect();
    parts.extend(PLACEHOLDER_SQL.iter().map(|p| format!("lower({col}) LIKE {p}")));
    parts.push(format!(
        "(SELECT COUNT(DISTINCT y.curator_id) FROM radar_contacts y WHERE y.valeur={col}) >= {SHARED_EMAIL_MIN}"
    ));
    format!("NOT ({})", parts.join(" OR "))
}

/// Directories, submission marketplaces and socials. Crawling one of these as if
/// it were the curator's site harvests the addresses of everyone it lists (the
/// Serper lookup returned playlistlookup.com for 93 curators and
/// playlistpartner.com for 22). Linktree-style hubs are absent on purpose: they
/// are usually the curator's own page.
const RADAR_SITE_SKIP: &[&str] = &[
    "playlistlookup.", "playlistpartner.", "playlistsupply.", "isitagoodplaylist.",
    "submithub.", "groover.", "playlist-push.", "playlistpush.", "dailyplaylists.",
    "reddit.", "facebook.", "instagram.", "twitter.", "x.com", "tiktok.", "discord.",
    "youtube.", "youtu.be", "spotify.", "deezer.", "apple.com", "soundcloud.",
    "beatport.", "last.fm", "archive.org", "wikipedia.", "google.", "ra.co",
];

/// True when `site` is a directory/social, or is already the site of at least
/// two other curators (which only an aggregator would be).
fn is_foreign_site(conn: &rusqlite::Connection, cid: i64, site: &str) -> bool {
    let origin = origin_of(site);
    // Same pattern syntax as the venue skip list (enrich::host_hit): "name."
    // matches that label anywhere, a full domain itself or a subdomain only, so
    // "x.com" does not catch netflix.com nor "ra.co" sierra.com. Tourism offices
    // and public bodies are skipped too.
    if host_in(&host_of(&origin), RADAR_SITE_SKIP) {
        return true;
    }
    conn.query_row(
        "SELECT COUNT(*) FROM radar_curators WHERE id != ?1 AND site_web IS NOT NULL
           AND (site_web = ?2 OR site_web LIKE ?2 || '/%')",
        params![cid, origin],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n >= 2)
    .unwrap_or(false)
}

/// SQL boolean over `col`, true when the address sits on a platform domain or
/// any subdomain of one, and is not one of the compliance mailboxes above.
/// Matching on '%@domain' or '%.domain' means `a@mail.spotify.com` counts while
/// `a@notspotify.com` does not, and `a@youtube.com` stays out while
/// `a@music.youtube.com` gets in.
fn official_domain_sql(col: &str) -> String {
    let mut parts = Vec::with_capacity(PLATFORM_DOMAINS.len() * 2);
    for d in PLATFORM_DOMAINS {
        parts.push(format!("lower({col}) LIKE '%@{d}'"));
        parts.push(format!("lower({col}) LIKE '%.{d}'"));
    }
    format!("(({}) AND {})", parts.join(" OR "), pitchable_sql(col))
}

fn store_email(conn: &rusqlite::Connection, curator_id: i64, email: &str, score: i64, src: &str, method: &str) {
    // Write-time filter, same rules the read side applies: scraping artefacts,
    // template addresses, compliance mailboxes and RGPD-erased addresses are
    // never stored.
    let Some(email) = clean_email(email) else { return };
    if is_placeholder_email(&email)
        || is_non_pitch_email(&email)
        || crate::commands::vi::is_excluded(conn, &email)
    {
        return;
    }
    let email = email.as_str();
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
                _ => return Err(err("radar.spotify_not_configured", false)),
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
        "labels" => {
            let labels = crate::commands::musicbrainz::search_labels(&genre, limit.min(50))
                .await
                .map_err(|e| err(format!("MusicBrainz: {}", e), true))?;
            for l in labels {
                let name = l.name.clone().unwrap_or_default();
                if name.is_empty() {
                    continue;
                }
                let site = crate::commands::musicbrainz::label_homepage(&l.id).await;
                let country = l.area.as_ref().and_then(|a| a.name.clone());
                upsert_label(pool, &l.id, &name, country.as_deref(), &genre, site.as_deref());
                tokio::time::sleep(std::time::Duration::from_millis(1100)).await; // MB: 1 req/s
            }
        }
        "youtube" => {
            let key = {
                let conn = pool.get().map_err(|e| err(e.to_string(), true))?;
                setting(&conn, "youtube_api_key")
            };
            let key = key.ok_or_else(|| err("radar.youtube_not_configured", false))?;
            let ids = crate::commands::youtube::search_channel_ids(&key, &format!("{} music", genre), 40)
                .await
                .map_err(|e| err(e, true))?;
            for chunk in ids.chunks(50) {
                let chans = crate::commands::youtube::channels(&key, chunk)
                    .await
                    .map_err(|e| err(e, true))?;
                for c in chans {
                    let sn = c.snippet.unwrap_or(crate::commands::youtube::ChannelSnippet {
                        title: None,
                        description: None,
                        custom_url: None,
                        country: None,
                    });
                    let name = sn.title.unwrap_or_default();
                    if name.is_empty() {
                        continue;
                    }
                    let editorial = is_editorial(&name) || name.to_lowercase().ends_with("- topic");
                    let subs = c.statistics.and_then(|s| s.subscriber_count).and_then(|s| s.parse::<i64>().ok());
                    let url = sn
                        .custom_url
                        .map(|u| format!("https://youtube.com/{}", u))
                        .unwrap_or_else(|| format!("https://youtube.com/channel/{}", c.id));
                    upsert_curator(
                        pool, "youtube", &c.id, &name, &name, Some(&url), subs, None, &genre,
                        sn.description.as_deref(), editorial,
                    );
                }
            }
        }
        _ => return Err(err("radar.unknown_source", false)),
    }
    Ok(())
}

fn upsert_label(pool: &DbPool, mbid: &str, name: &str, country: Option<&str>, genre: &str, site: Option<&str>) {
    let Ok(conn) = pool.get() else { return };
    let norm = normalise(name);
    // Stored under the same name the UI uses for this source. It used to be
    // written as 'musicbrainz' while the source filter offered 'labels', so that
    // filter could never match a single row.
    let _ = conn.execute(
        "INSERT INTO radar_curators (source, external_id, kind, nom, nom_normalise, pays, genre, site_web, editorial)
         VALUES ('labels',?1,'label',?2,?3,?4,?5,?6,0)
         ON CONFLICT(source, external_id) DO UPDATE SET
            site_web=COALESCE(radar_curators.site_web, excluded.site_web),
            pays=COALESCE(radar_curators.pays, excluded.pays), updated_at=datetime('now')",
        params![mbid, name, norm, country, genre, site],
    );
    if let Ok(cid) = conn.query_row(
        "SELECT id FROM radar_curators WHERE source='labels' AND external_id=?1",
        params![mbid],
        |r| r.get::<_, i64>(0),
    ) {
        qualify_one(&conn, cid);
    }
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
    // Editorial playlists belong to Spotify/Deezer themselves: there is nobody to
    // email, so they are noise for a booking pipeline. Dropping them at the scan
    // means the counters and the list agree instead of the counters promising
    // results the list then filters away.
    if editorial {
        return;
    }
    let Ok(conn) = pool.get() else { return };
    let norm = normalise(name);
    // A YouTube channel is not a playlist; the kind filter tells them apart.
    let kind = if source == "youtube" { "channel" } else { "playlist" };
    let (mined, site) = description.map(mine_text).unwrap_or_default();
    let _ = conn.execute(
        "INSERT INTO radar_curators (source, external_id, kind, nom, nom_normalise, owner_name, url, followers, nb_tracks, genre, description, site_web, editorial)
         VALUES (?1,?2,?13,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)
         ON CONFLICT(source, external_id) DO UPDATE SET
            followers=excluded.followers, nb_tracks=excluded.nb_tracks,
            description=COALESCE(excluded.description, description),
            site_web=COALESCE(radar_curators.site_web, excluded.site_web),
            updated_at=datetime('now')",
        params![source, ext_id, name, norm, owner, url, followers, nb_tracks, genre, description, site, editorial as i64, kind],
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
    // Only an address the list would surface counts as "has an email".
    let has_email = format!(
        "EXISTS(SELECT 1 FROM radar_contacts c WHERE c.curator_id=radar_curators.id AND c.type='email' AND {})",
        pitchable_sql("c.valeur")
    );
    let _ = conn.execute(
        // SQLite keeps whatever type the expression produced, so without this cast
        // the column holds REAL for every row the MIN() does not clamp to 100,
        // and anything reading it as an integer fails on those rows.
        &format!("UPDATE radar_curators SET
            score = CAST(MIN(100,
                CAST(COALESCE(followers,0) AS REAL) / 500
                + CASE WHEN {has_email} THEN 40 ELSE 0 END
                + CASE WHEN editorial=1 THEN 0 ELSE 10 END) AS INTEGER),
            statut = CASE
                WHEN statut IN ('valide','archive','rejete') THEN statut
                WHEN editorial=1 THEN 'candidat'
                WHEN {has_email} THEN 'qualifie'
                WHEN COALESCE(followers,0) >= 2000 THEN 'qualifie'
                ELSE 'candidat' END,
            updated_at=datetime('now')
         WHERE id=?1"),
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
        // A directory or social page is not the curator's site: crawling it
        // files everyone it lists as this curator's contacts.
        let site = row
            .1
            .and_then(|s| normalize_site(&s))
            .filter(|s| !is_foreign_site(&conn, cid, s));
        (row.0, site, setting(&conn, "serper_api_key"))
    };

    // No site yet? Try to resolve one via web search (Serper), if a key is set.
    if site.is_none() {
        if let Some(key) = api_key {
            let query = format!("{} playlist curator submit demo contact", nom);
            // Accepted only when the result's domain or title carries the
            // curator's name, never a directory from RADAR_SITE_SKIP.
            if let Some(found) =
                resolve_website_via_search(&crawl_client(), &key, &query, &nom, RADAR_SITE_SKIP).await
            {
                let foreign = pool
                    .get()
                    .map(|conn| is_foreign_site(&conn, cid, &found))
                    .unwrap_or(true);
                if !foreign {
                    site = Some(found);
                }
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
            let mut body = body;
            if body.len() > 300_000 {
                // Cut on a char boundary: slicing mid-codepoint panics the worker.
                let mut cut = 300_000;
                while !body.is_char_boundary(cut) {
                    cut -= 1;
                }
                body.truncate(cut);
            }
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
                // The crawled origin replaces a stored directory/social URL that
                // was skipped above.
                "UPDATE radar_curators SET site_web=?2, enriched_at=datetime('now') WHERE id=?1",
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
    let sources: Vec<String> = sources.into_iter().filter(|s| ["spotify","deezer","labels","youtube"].contains(&s.as_str())).collect();
    if genres.is_empty() || sources.is_empty() {
        return Err("radar.pick_genre_source".into());
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

/// Queue an email hunt for every curator the scan just brought in, inside the
/// run that is already open. Finding a curator without an address is useless, so
/// the two phases are one operation from the user's point of view; keeping them
/// in a single run also means one progress bar instead of two buttons whose
/// order the user had to know. Returns how many tasks were queued.
pub fn queue_enrich_tasks(pool: &DbPool, run_id: i64) -> i64 {
    let Ok(conn) = pool.get() else { return 0 };
    let ids: Vec<i64> = {
        let Ok(mut stmt) = conn.prepare(&format!(
            "SELECT id FROM radar_curators
             WHERE editorial=0 AND statut != 'rejete' AND enriched_at IS NULL
               AND NOT EXISTS(SELECT 1 FROM radar_contacts x WHERE x.curator_id=radar_curators.id
                              AND x.type='email' AND {})
             ORDER BY followers DESC LIMIT 400",
            pitchable_sql("x.valeur")
        )) else {
            return 0;
        };
        let Ok(rows) = stmt.query_map([], |r| r.get::<_, i64>(0)) else {
            return 0;
        };
        rows.flatten().collect()
    };
    let Ok(mut stmt) = conn
        .prepare("INSERT INTO vi_tasks (run_id, type, payload, statut) VALUES (?1,'radar_enrich',?2,'en_attente')")
    else {
        return 0;
    };
    let mut n = 0;
    for id in &ids {
        if stmt
            .execute(params![run_id, json!({ "curator_id": id }).to_string()])
            .is_ok()
        {
            n += 1;
        }
    }
    n
}

#[tauri::command]
pub async fn radar_enrich(app: AppHandle, state: State<'_, AppState>, force: Option<bool>) -> Result<i64, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    // A curator whose only addresses are compliance/directory ones still needs a
    // hunt, so "has an email" means a usable one, as everywhere else.
    let mut sql = format!(
        "SELECT id FROM radar_curators WHERE editorial=0 AND statut != 'rejete'
         AND NOT EXISTS(SELECT 1 FROM radar_contacts c WHERE c.curator_id=radar_curators.id AND c.type='email' AND {})",
        pitchable_sql("c.valeur")
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
        return Err("radar.enrich_none".into());
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

/// Only what the list actually shows. `score`, `statut`, `editorial`,
/// `nb_tracks` and `site_web` used to travel here unused, and `score` was typed
/// i64 while qualify_one stores a REAL whenever the value is not clamped to 100.
/// rusqlite refuses that conversion, so a single unclamped row anywhere in the
/// page made the whole command return Err and the list render empty, whatever
/// the filters were set to.
#[derive(Serialize)]
pub struct CuratorRow {
    pub id: i64,
    pub source: String,
    /// playlist | label | channel
    pub kind: String,
    pub nom: String,
    pub owner_name: Option<String>,
    pub url: Option<String>,
    pub followers: Option<i64>,
    pub genre: Option<String>,
    pub best_email: Option<String>,
    pub nb_emails: i64,
    /// `best_email` sits on a streaming platform's own domain. See PLATFORM_DOMAINS.
    pub official: bool,
    pub enriched: bool,
    pub in_pipeline: bool,
    pub rejected: bool,
    /// Pipeline status of the promoted contact (contacts.status), so the list
    /// shows "contacted" / "replied" rather than only "in pipeline".
    pub contact_status: Option<String>,
    /// At least one email was actually sent to that contact.
    pub emailed: bool,
}

/// One list. `filter` is "all" (default), "email" (has an address we can write
/// to), "official", "pipeline" (already promoted) or "rejected" (set aside by
/// hand, hidden everywhere else). `kind` narrows to playlist / label / channel.
#[tauri::command(async)]
pub fn radar_list(
    state: State<'_, AppState>,
    filter: Option<String>,
    kind: Option<String>,
    search: Option<String>,
    limit: Option<i64>,
) -> Result<Vec<CuratorRow>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let official = official_domain_sql("x.valeur");
    // Compliance mailboxes are not contacts, so they are excluded everywhere an
    // address is counted or surfaced rather than only hidden from the badge.
    let usable = format!("x.type='email' AND {}", pitchable_sql("x.valeur"));
    // A platform address outranks anything else, so when a curator exposes both a
    // gmail and an @deezer.com the official one is the one surfaced.
    let rejected_view = filter.as_deref() == Some("rejected");
    let mut sql = format!(
        "SELECT c.id, c.source, c.kind, c.nom, c.owner_name, c.url, CAST(c.followers AS INTEGER), c.genre,
                (SELECT valeur FROM radar_contacts x WHERE x.curator_id=c.id AND {usable}
                 ORDER BY {official} DESC, x.score DESC LIMIT 1),
                (SELECT COUNT(*) FROM radar_contacts x WHERE x.curator_id=c.id AND {usable}),
                EXISTS(SELECT 1 FROM radar_contacts x WHERE x.curator_id=c.id AND {usable} AND {official}),
                (c.enriched_at IS NOT NULL),
                (c.crm_contact_id IS NOT NULL),
                (c.statut = 'rejete'),
                k.status,
                EXISTS(SELECT 1 FROM emails m WHERE m.contact_id=c.crm_contact_id AND m.status='sent')
         FROM radar_curators c
         LEFT JOIN contacts k ON k.id = c.crm_contact_id
         WHERE c.editorial=0 AND {}",
        if rejected_view { "c.statut = 'rejete'" } else { "c.statut != 'rejete'" }
    );
    let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    match filter.as_deref() {
        Some("email") => sql.push_str(&format!(
            " AND EXISTS(SELECT 1 FROM radar_contacts x WHERE x.curator_id=c.id AND {usable})"
        )),
        Some("official") => sql.push_str(&format!(
            " AND EXISTS(SELECT 1 FROM radar_contacts x WHERE x.curator_id=c.id AND {usable} AND {official})"
        )),
        Some("pipeline") => sql.push_str(" AND c.crm_contact_id IS NOT NULL"),
        _ => {}
    }
    if let Some(k) = kind.filter(|k| ["playlist", "label", "channel"].contains(&k.as_str())) {
        sql.push_str(" AND c.kind = ?");
        args.push(Box::new(k));
    }
    if let Some(q) = search.filter(|s| !s.trim().is_empty()) {
        sql.push_str(" AND (c.nom LIKE ? OR c.owner_name LIKE ?)");
        let like = format!("%{}%", q.trim());
        args.push(Box::new(like.clone()));
        args.push(Box::new(like));
    }
    // Platform addresses first, then anyone else reachable: that ordering matches
    // the order in which these leads are worth working.
    sql.push_str(&format!(
        " ORDER BY EXISTS(SELECT 1 FROM radar_contacts x WHERE x.curator_id=c.id AND {usable} AND {official}) DESC,
                   EXISTS(SELECT 1 FROM radar_contacts x WHERE x.curator_id=c.id AND {usable}) DESC,
                   c.followers DESC, c.nom COLLATE NOCASE"
    ));
    sql.push_str(&format!(" LIMIT {}", limit.unwrap_or(400).clamp(1, 1000)));

    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();
    let rows = stmt
        .query_map(refs.as_slice(), |r| {
            Ok(CuratorRow {
                id: r.get(0)?,
                source: r.get(1)?,
                kind: r.get(2)?,
                nom: r.get(3)?,
                owner_name: r.get(4)?,
                url: r.get(5)?,
                followers: r.get(6)?,
                genre: r.get(7)?,
                best_email: r.get(8)?,
                nb_emails: r.get(9)?,
                official: r.get::<_, i64>(10)? != 0,
                enriched: r.get::<_, i64>(11)? != 0,
                in_pipeline: r.get::<_, i64>(12)? != 0,
                rejected: r.get::<_, i64>(13)? != 0,
                contact_status: r.get(14)?,
                emailed: r.get::<_, i64>(15)? != 0,
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
    pub with_email: i64,
    pub official: i64,
    pub in_pipeline: i64,
    /// Not yet promoted and holding a usable address: what the bulk button adds.
    /// The page used to derive it as with_email - in_pipeline, which goes wrong
    /// (negative, or hiding real leads) as soon as a promoted curator's address
    /// stops counting as usable.
    pub addressable: i64,
    /// Promoted curators whose contact still carries an address the list no
    /// longer considers theirs (directory/compliance mailbox), never written to.
    pub stale_contacts: i64,
    /// Set aside by hand ("Rejeter").
    pub rejected: i64,
    /// Visible curators per kind (playlist / label / channel).
    pub playlists: i64,
    pub labels: i64,
    pub channels: i64,
}

/// Every counter is scoped to exactly what the list can show, so a number on
/// screen always corresponds to rows you can actually reach. The old `total`
/// counted editorial and rejected rows the list then hid, which is what made a
/// scan look like it had found hundreds of curators and returned none.
#[tauri::command(async)]
pub fn radar_stats(state: State<'_, AppState>) -> Result<RadarStats, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    const VISIBLE: &str = "editorial=0 AND statut != 'rejete'";
    let usable = format!("x.type='email' AND {}", pitchable_sql("x.valeur"));
    let one = |sql: &str| conn.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap_or(0);
    let kind = |k: &str| one(&format!("SELECT COUNT(*) FROM radar_curators WHERE {VISIBLE} AND kind='{k}'"));
    Ok(RadarStats {
        total: one(&format!("SELECT COUNT(*) FROM radar_curators WHERE {VISIBLE}")),
        with_email: one(&format!(
            "SELECT COUNT(*) FROM radar_curators c WHERE {VISIBLE}
             AND EXISTS(SELECT 1 FROM radar_contacts x WHERE x.curator_id=c.id AND {usable})"
        )),
        official: one(&format!(
            "SELECT COUNT(*) FROM radar_curators c WHERE {VISIBLE}
             AND EXISTS(SELECT 1 FROM radar_contacts x WHERE x.curator_id=c.id AND {usable} AND {})",
            official_domain_sql("x.valeur")
        )),
        in_pipeline: one(&format!(
            "SELECT COUNT(*) FROM radar_curators WHERE {VISIBLE} AND crm_contact_id IS NOT NULL"
        )),
        addressable: one(&format!(
            "SELECT COUNT(*) FROM radar_curators c WHERE {VISIBLE} AND crm_contact_id IS NULL
             AND EXISTS(SELECT 1 FROM radar_contacts x WHERE x.curator_id=c.id AND {usable})"
        )),
        stale_contacts: one(&format!("SELECT COUNT(*) FROM ({})", stale_contacts_sql())),
        rejected: one("SELECT COUNT(*) FROM radar_curators WHERE editorial=0 AND statut='rejete'"),
        playlists: kind("playlist"),
        labels: kind("label"),
        channels: kind("channel"),
    })
}

/// Set curators aside (statut 'rejete'): out of the list, the counters, the bulk
/// promotion and the email hunt. Their pipeline contact, if any, is untouched.
#[tauri::command(async)]
pub fn radar_reject(state: State<'_, AppState>, ids: Vec<i64>) -> Result<i64, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    let mut n = 0;
    for id in ids {
        n += tx
            .execute(
                "UPDATE radar_curators SET statut='rejete', updated_at=datetime('now') WHERE id=?1 AND statut != 'rejete'",
                params![id],
            )
            .map_err(|e| e.to_string())? as i64;
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(n)
}

/// Undo radar_reject: back to the automatic qualification.
#[tauri::command(async)]
pub fn radar_restore(state: State<'_, AppState>, ids: Vec<i64>) -> Result<i64, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let mut n = 0;
    for id in ids {
        n += conn
            .execute(
                "UPDATE radar_curators SET statut = CASE WHEN crm_contact_id IS NOT NULL THEN 'valide' ELSE 'candidat' END
                 WHERE id=?1 AND statut='rejete'",
                params![id],
            )
            .map_err(|e| e.to_string())? as i64;
        qualify_one(&conn, id);
    }
    Ok(n)
}

/// What un-promoting does to the linked contact.
#[derive(Debug, PartialEq)]
pub(crate) enum UnpromoteAction {
    /// Created by Radar for this curator alone, never written to: remove it.
    Delete,
    /// Engaged, shared or pre-existing: keep it, only drop the link.
    Unlink,
}

/// Pure decision for radar_unpromote, so the rule is testable without a DB.
pub(crate) fn unpromote_action(
    status: &str,
    reason: Option<&str>,
    first_contact: bool,
    emails_sent: i64,
    other_links: i64,
) -> UnpromoteAction {
    if status == "to_contact" && !first_contact && emails_sent == 0 && other_links == 0 && reason == Some("Radar") {
        UnpromoteAction::Delete
    } else {
        UnpromoteAction::Unlink
    }
}

/// Take a curator back out of the pipeline. The contact is deleted only when
/// Radar created it, nothing was ever sent to it, it is still "to contact" and
/// no other curator or venue points at it; otherwise it stays and only the
/// link goes, so no history is ever lost. Returns true when it was deleted.
#[tauri::command(async)]
pub fn radar_unpromote(state: State<'_, AppState>, id: i64) -> Result<bool, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let cid: Option<i64> = conn
        .query_row("SELECT crm_contact_id FROM radar_curators WHERE id=?1", params![id], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    let mut deleted = false;
    if let Some(cid) = cid {
        let row: Option<(String, Option<String>, bool)> = tx
            .query_row(
                "SELECT status, reason, first_contact IS NOT NULL FROM contacts WHERE id=?1",
                params![cid],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .ok();
        if let Some((status, reason, first_contact)) = row {
            let sent: i64 = tx
                .query_row("SELECT COUNT(*) FROM emails WHERE contact_id=?1", params![cid], |r| r.get(0))
                .unwrap_or(1);
            let others: i64 = tx
                .query_row(
                    "SELECT (SELECT COUNT(*) FROM radar_curators WHERE crm_contact_id=?1 AND id != ?2)
                          + (SELECT COUNT(*) FROM vi_venues WHERE crm_contact_id=?1)",
                    params![cid, id],
                    |r| r.get(0),
                )
                .unwrap_or(1);
            if unpromote_action(&status, reason.as_deref(), first_contact, sent, others) == UnpromoteAction::Delete {
                tx.execute("DELETE FROM contacts WHERE id=?1", params![cid]).map_err(|e| e.to_string())?;
                deleted = true;
            }
        }
    }
    tx.execute(
        "UPDATE radar_curators SET crm_contact_id=NULL,
            statut = CASE WHEN statut='valide' THEN 'candidat' ELSE statut END,
            updated_at=datetime('now') WHERE id=?1",
        params![id],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    qualify_one(&conn, id);
    Ok(deleted)
}

/// (curator id, contact id) of promoted curators whose contact email is not one
/// of the curator's usable addresses, restricted to contacts nobody has written
/// to or moved along the pipeline, so a repair can never rewrite history.
fn stale_contacts_sql() -> String {
    format!(
        "SELECT r.id, k.id FROM radar_curators r JOIN contacts k ON k.id=r.crm_contact_id
         WHERE k.status='to_contact' AND COALESCE(trim(k.email),'') != ''
           AND NOT EXISTS(SELECT 1 FROM emails e WHERE e.contact_id=k.id)
           AND NOT EXISTS(SELECT 1 FROM radar_contacts x WHERE x.curator_id=r.id AND x.type='email'
                          AND lower(x.valeur)=lower(trim(k.email)) AND {})",
        pitchable_sql("x.valeur")
    )
}

/// Replace the directory/compliance address on promoted curator contacts with
/// the curator's best usable address, or clear it when there is none (the
/// contact stays, it just stops being a campaign target). Only touches contacts
/// still 'to_contact' and never emailed. Returns how many were updated.
#[tauri::command(async)]
pub fn radar_repair_contacts(state: State<'_, AppState>) -> Result<i64, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let pairs: Vec<(i64, i64)> = {
        let mut stmt = conn.prepare(&stale_contacts_sql()).map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))
            .map_err(|e| e.to_string())?;
        rows.flatten().collect()
    };
    let best_sql = format!(
        "SELECT valeur FROM radar_contacts x WHERE x.curator_id=?1 AND x.type='email' AND {}
         ORDER BY {} DESC, x.score DESC LIMIT 1",
        pitchable_sql("x.valeur"),
        official_domain_sql("x.valeur")
    );
    let mut n = 0;
    for (curator_id, contact_id) in pairs {
        let best: Option<String> = conn.query_row(&best_sql, params![curator_id], |r| r.get(0)).ok();
        if conn
            .execute(
                "UPDATE contacts SET email=?2, updated_at=datetime('now') WHERE id=?1",
                params![contact_id, best],
            )
            .is_ok()
        {
            n += 1;
        }
    }
    Ok(n)
}

/// Promote a curator into the CRM contacts pipeline (category 'curator').
/// Idempotent: returns the existing contact if already linked.
#[tauri::command(async)]
pub fn radar_promote(state: State<'_, AppState>, id: i64) -> Result<i64, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    promote_curator_inner(&conn, id)
}

fn promote_curator_inner(conn: &rusqlite::Connection, id: i64) -> Result<i64, String> {
    if let Ok(Some(cid)) = conn.query_row(
        "SELECT crm_contact_id FROM radar_curators WHERE id=?1",
        params![id],
        |r| r.get::<_, Option<i64>>(0),
    ) {
        return Ok(cid);
    }
    let (nom, url, source, kind): (String, Option<String>, String, String) = conn
        .query_row(
            "SELECT nom, url, source, kind FROM radar_curators WHERE id=?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .map_err(|e| e.to_string())?;
    let email: Option<String> = conn
        .query_row(
            &format!(
                "SELECT valeur FROM radar_contacts x WHERE x.curator_id=?1 AND x.type='email' AND {}
                 ORDER BY {} DESC, x.score DESC LIMIT 1",
                pitchable_sql("x.valeur"),
                official_domain_sql("x.valeur")
            ),
            params![id],
            |r| r.get(0),
        )
        .ok();
    // The same mailbox already in contacts (a label behind two playlists, or an
    // address added by hand): link to it rather than creating a duplicate that
    // a campaign would write to twice.
    if let Some(e) = email.as_deref() {
        if let Ok(existing) = conn.query_row(
            "SELECT id FROM contacts WHERE lower(trim(email))=lower(trim(?1)) ORDER BY id LIMIT 1",
            params![e],
            |r| r.get::<_, i64>(0),
        ) {
            conn.execute(
                "UPDATE radar_curators SET crm_contact_id=?2, statut='valide' WHERE id=?1",
                params![id, existing],
            )
            .map_err(|e| e.to_string())?;
            return Ok(existing);
        }
    }
    conn.execute(
        "INSERT INTO contacts (category, name, email, website, notes, contact_channel, status, reason, artist_id)
         VALUES ('curator', ?1, ?2, ?3, ?4, 'email', 'to_contact', 'Radar', ?5)",
        params![
            nom,
            email,
            url,
            format!("Radar {} ({})", kind, source),
            crate::commands::artists::default_artist_id(conn)
        ],
    )
    .map_err(|e| e.to_string())?;
    let contact_id = conn.last_insert_rowid();
    conn.execute("UPDATE radar_curators SET crm_contact_id=?2, statut='valide' WHERE id=?1", params![id, contact_id])
        .map_err(|e| e.to_string())?;
    Ok(contact_id)
}

/// Promote every not-yet-linked curator with an email into contacts.
#[tauri::command(async)]
pub fn radar_promote_bulk(state: State<'_, AppState>) -> Result<i64, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let ids: Vec<i64> = {
        let mut stmt = conn
            .prepare(
                &format!(
                    "SELECT id FROM radar_curators WHERE crm_contact_id IS NULL AND editorial=0
                     AND statut != 'rejete'
                     AND EXISTS(SELECT 1 FROM radar_contacts x WHERE x.curator_id=radar_curators.id
                                AND x.type='email' AND {})",
                    pitchable_sql("x.valeur")
                ),
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt.query_map([], |r| r.get::<_, i64>(0)).map_err(|e| e.to_string())?;
        let mut v = Vec::new();
        for r in rows {
            if let Ok(x) = r {
                v.push(x);
            }
        }
        v
    };
    let mut n = 0;
    for id in ids {
        if promote_curator_inner(&conn, id).is_ok() {
            n += 1;
        }
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unpromote_deletes_only_untouched_radar_contacts() {
        use UnpromoteAction::*;
        assert_eq!(unpromote_action("to_contact", Some("Radar"), false, 0, 0), Delete);
        // emailed, dated, moved along, shared or created by hand: keep it
        assert_eq!(unpromote_action("to_contact", Some("Radar"), false, 1, 0), Unlink);
        assert_eq!(unpromote_action("to_contact", Some("Radar"), true, 0, 0), Unlink);
        assert_eq!(unpromote_action("contacted", Some("Radar"), false, 0, 0), Unlink);
        assert_eq!(unpromote_action("to_contact", Some("Radar"), false, 0, 1), Unlink);
        assert_eq!(unpromote_action("to_contact", None, false, 0, 0), Unlink);
    }

    #[test]
    fn foreign_sites_use_the_shared_host_rules() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE radar_curators (id INTEGER PRIMARY KEY, site_web TEXT);").unwrap();
        assert!(is_foreign_site(&conn, 1, "https://www.playlistlookup.com/x"));
        assert!(is_foreign_site(&conn, 1, "https://old.reddit.com/r/x"));
        assert!(!is_foreign_site(&conn, 1, "https://netflix.com"));
        assert!(!is_foreign_site(&conn, 1, "https://deepvibes.fm"));
        // the same site on two other curators is an aggregator
        conn.execute_batch(
            "INSERT INTO radar_curators VALUES (2,'https://hub.fm'),(3,'https://hub.fm/a');",
        )
        .unwrap();
        assert!(is_foreign_site(&conn, 1, "https://hub.fm"));
    }
}
