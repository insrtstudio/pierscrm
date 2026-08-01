//! Spotify Web API client, ISOLATED ON PURPOSE (like ra.rs).
//!
//! Client credentials flow only (public data, no user OAuth). The token is
//! cached in memory with its expiry; 429s are retried with the Retry-After
//! header honoured. Credentials live in the settings table
//! (`spotify_client_id` / `spotify_client_secret`), editable in Réglages.

use serde::Deserialize;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const TOKEN_URL: &str = "https://accounts.spotify.com/api/token";
const API: &str = "https://api.spotify.com/v1";

static TOKEN_CACHE: Mutex<Option<(String, Instant)>> = Mutex::new(None);

pub struct Spotify {
    http: reqwest::Client,
    client_id: String,
    client_secret: String,
}

#[derive(Debug)]
pub struct SpotifyError(pub String);

impl std::fmt::Display for SpotifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Deserialize)]
struct TokenResp {
    access_token: String,
    expires_in: u64,
}

#[derive(Deserialize)]
pub struct ArtistResp {
    pub popularity: Option<i64>,
    pub followers: Option<Followers>,
    pub name: Option<String>,
}
#[derive(Deserialize)]
pub struct Followers {
    pub total: Option<i64>,
}

#[derive(Deserialize)]
struct TracksResp {
    tracks: Vec<Option<TrackResp>>,
}
#[derive(Deserialize)]
pub struct TrackResp {
    pub id: String,
    pub name: Option<String>,
    pub popularity: Option<i64>,
    pub album: Option<AlbumResp>,
}
#[derive(Deserialize)]
pub struct AlbumResp {
    pub release_date: Option<String>,
}

#[derive(Deserialize)]
pub struct PlaylistResp {
    pub name: Option<String>,
    pub followers: Option<Followers>,
    pub tracks: Option<PlaylistTracks>,
    pub owner: Option<PlaylistOwner>,
}
#[derive(Deserialize)]
pub struct PlaylistTracks {
    pub total: Option<i64>,
    #[serde(default)]
    pub items: Vec<PlaylistItem>,
}
#[derive(Deserialize)]
pub struct PlaylistItem {
    pub track: Option<PlaylistItemTrack>,
}
#[derive(Deserialize)]
pub struct PlaylistItemTrack {
    pub id: Option<String>,
}
#[derive(Deserialize)]
pub struct PlaylistOwner {
    pub display_name: Option<String>,
}

impl Spotify {
    pub fn new(client_id: String, client_secret: String) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(20))
                .build()
                .expect("spotify client"),
            client_id,
            client_secret,
        }
    }

    async fn token(&self) -> Result<String, SpotifyError> {
        if let Some((tok, until)) = TOKEN_CACHE.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            if Instant::now() < until {
                return Ok(tok);
            }
        }
        let resp = self
            .http
            .post(TOKEN_URL)
            .basic_auth(&self.client_id, Some(&self.client_secret))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body("grant_type=client_credentials")
            .send()
            .await
            .map_err(|e| SpotifyError(format!("token: {}", e)))?;
        if !resp.status().is_success() {
            return Err(SpotifyError(format!(
                "token HTTP {} (client id/secret invalides ?)",
                resp.status().as_u16()
            )));
        }
        let t: TokenResp = resp
            .json()
            .await
            .map_err(|e| SpotifyError(format!("token parse: {}", e)))?;
        // Refresh one minute early.
        let until = Instant::now() + Duration::from_secs(t.expires_in.saturating_sub(60));
        *TOKEN_CACHE.lock().unwrap_or_else(|e| e.into_inner()) =
            Some((t.access_token.clone(), until));
        Ok(t.access_token)
    }

    /// GET with up to 3 retries on 429, honouring Retry-After.
    async fn get_json<T: for<'de> Deserialize<'de>>(&self, url: &str) -> Result<T, SpotifyError> {
        for attempt in 0..4u32 {
            let tok = self.token().await?;
            let resp = self
                .http
                .get(url)
                .bearer_auth(&tok)
                .send()
                .await
                .map_err(|e| SpotifyError(e.to_string()))?;
            let status = resp.status();
            if status.as_u16() == 429 && attempt < 3 {
                let wait = resp
                    .headers()
                    .get("Retry-After")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(1 << attempt);
                tokio::time::sleep(Duration::from_secs(wait.min(30))).await;
                continue;
            }
            if status.as_u16() == 401 {
                // Token revoked mid-flight: drop the cache and retry once.
                *TOKEN_CACHE.lock().unwrap_or_else(|e| e.into_inner()) = None;
                if attempt < 3 {
                    continue;
                }
            }
            if !status.is_success() {
                return Err(SpotifyError(format!("HTTP {} sur {}", status.as_u16(), url)));
            }
            return resp
                .json::<T>()
                .await
                .map_err(|e| SpotifyError(format!("parse: {}", e)));
        }
        Err(SpotifyError("rate limit persistant (429)".into()))
    }

    pub async fn artist(&self, id: &str) -> Result<ArtistResp, SpotifyError> {
        self.get_json(&format!("{}/artists/{}", API, id)).await
    }

    /// Batch of up to 50 track ids.
    pub async fn tracks(&self, ids: &[String]) -> Result<Vec<TrackResp>, SpotifyError> {
        let mut out = Vec::new();
        for chunk in ids.chunks(50) {
            let url = format!("{}/tracks?ids={}", API, chunk.join(","));
            let r: TracksResp = self.get_json(&url).await?;
            out.extend(r.tracks.into_iter().flatten());
        }
        Ok(out)
    }

    pub async fn track(&self, id: &str) -> Result<TrackResp, SpotifyError> {
        self.get_json(&format!("{}/tracks/{}", API, id)).await
    }

    /// Playlist meta + the first 100 track ids (enough to flag "contains ours").
    pub async fn playlist(&self, id: &str) -> Result<PlaylistResp, SpotifyError> {
        let url = format!(
            "{}/playlists/{}?fields=name,followers.total,owner.display_name,tracks(total,items(track(id)))",
            API, id
        );
        self.get_json(&url).await
    }
}

/// Accepts open.spotify.com URLs, spotify: URIs, or bare 22-char ids, and
/// returns (kind, id) when recognised. kind is "artist" | "track" | "playlist".
pub fn parse_spotify_id(input: &str) -> Option<(String, String)> {
    let s = input.trim();
    let id_ok = |id: &str| id.len() == 22 && id.chars().all(|c| c.is_ascii_alphanumeric());
    for kind in ["artist", "track", "playlist"] {
        // spotify:kind:id
        if let Some(rest) = s.strip_prefix(&format!("spotify:{}:", kind)) {
            let id = rest.split(&['?', '&'][..]).next().unwrap_or(rest);
            if id_ok(id) {
                return Some((kind.into(), id.into()));
            }
        }
        // https://open.spotify.com/(intl-xx/)?kind/id
        if let Some(pos) = s.find(&format!("/{}/", kind)) {
            let rest = &s[pos + kind.len() + 2..];
            let id = rest.split(&['?', '/', '&'][..]).next().unwrap_or(rest);
            if id_ok(id) {
                return Some((kind.into(), id.into()));
            }
        }
    }
    if id_ok(s) {
        // Bare id: kind unknown, caller decides.
        return Some(("unknown".into(), s.into()));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_spotify_inputs() {
        let id = "37i9dQZF1DXcBWIGoYBM5M";
        assert_eq!(
            parse_spotify_id(&format!("https://open.spotify.com/playlist/{}?si=abc", id)),
            Some(("playlist".into(), id.into()))
        );
        assert_eq!(
            parse_spotify_id(&format!("https://open.spotify.com/intl-fr/track/{}", id)),
            Some(("track".into(), id.into()))
        );
        assert_eq!(
            parse_spotify_id(&format!("spotify:playlist:{}", id)),
            Some(("playlist".into(), id.into()))
        );
        assert_eq!(parse_spotify_id(id), Some(("unknown".into(), id.into())));
        assert_eq!(parse_spotify_id("not an id"), None);
    }
}
