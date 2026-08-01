//! Deezer public API client (no auth needed for search), isolated on purpose.
//! Used by Radar to map playlists and their creators by genre.

use serde::Deserialize;

const API: &str = "https://api.deezer.com";

#[derive(Deserialize)]
struct SearchResp {
    #[serde(default)]
    data: Vec<PlaylistItem>,
}
#[derive(Deserialize)]
pub struct PlaylistItem {
    pub id: i64,
    pub title: Option<String>,
    pub nb_tracks: Option<i64>,
    pub fans: Option<i64>,
    pub link: Option<String>,
    pub user: Option<DzUser>,
}
#[derive(Deserialize)]
pub struct DzUser {
    pub id: Option<i64>,
    pub name: Option<String>,
}

/// Search playlists by free text (genre keyword). Deezer caps at 100/page.
pub async fn search_playlists(query: &str, limit: i64) -> Result<Vec<PlaylistItem>, String> {
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?;
    let url = format!(
        "{}/search/playlist?q={}&limit={}",
        API,
        urlencoding(query),
        limit.clamp(1, 100)
    );
    let resp = http.get(&url).send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("Deezer HTTP {}", resp.status().as_u16()));
    }
    let r: SearchResp = resp.json().await.map_err(|e| e.to_string())?;
    Ok(r.data)
}

/// Playlist detail (description lives here, not in search results).
pub async fn playlist_description(id: i64) -> Option<String> {
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .ok()?;
    let v: serde_json::Value = http
        .get(format!("{}/playlist/{}", API, id))
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;
    v.get("description")
        .and_then(|d| d.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.trim().is_empty())
}

fn urlencoding(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
            ' ' => "%20".into(),
            _ => c.to_string().bytes().map(|b| format!("%{:02X}", b)).collect(),
        })
        .collect()
}
