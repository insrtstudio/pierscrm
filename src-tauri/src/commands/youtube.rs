//! YouTube Data API v3 client, isolated on purpose. Used by Radar to find music
//! promo channels / curators by genre: their channel description very often
//! carries a public contact email or a link that our crawler resolves.
//! Needs a free API key (Google Cloud) stored in settings `youtube_api_key`.
//! Quota note: search costs 100 units, channels.list 1 unit (10k/day default).

use serde::Deserialize;

const API: &str = "https://www.googleapis.com/youtube/v3";

#[derive(Deserialize)]
struct SearchResp {
    #[serde(default)]
    items: Vec<SearchItem>,
    error: Option<ApiErr>,
}
#[derive(Deserialize)]
struct SearchItem {
    id: Option<SearchId>,
}
#[derive(Deserialize)]
struct SearchId {
    #[serde(rename = "channelId")]
    channel_id: Option<String>,
}
#[derive(Deserialize)]
struct ApiErr {
    message: String,
}

#[derive(Deserialize)]
struct ChannelsResp {
    #[serde(default)]
    items: Vec<Channel>,
    error: Option<ApiErr>,
}
#[derive(Deserialize)]
pub struct Channel {
    pub id: String,
    pub snippet: Option<ChannelSnippet>,
    pub statistics: Option<ChannelStats>,
}
#[derive(Deserialize)]
pub struct ChannelSnippet {
    pub title: Option<String>,
    pub description: Option<String>,
    #[serde(rename = "customUrl")]
    pub custom_url: Option<String>,
    pub country: Option<String>,
}
#[derive(Deserialize)]
pub struct ChannelStats {
    #[serde(rename = "subscriberCount")]
    pub subscriber_count: Option<String>,
}

fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .expect("yt client")
}

/// Channel ids surfaced by a genre query (music curators / promo channels).
pub async fn search_channel_ids(key: &str, query: &str, max: i64) -> Result<Vec<String>, String> {
    let q: String = query
        .bytes()
        .map(|b| if b == b' ' { "%20".to_string() } else { (b as char).to_string() })
        .collect();
    let url = format!(
        "{}/search?part=snippet&type=channel&maxResults={}&q={}&key={}",
        API,
        max.clamp(1, 50),
        q,
        key
    );
    let r: SearchResp = http()
        .get(&url)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    if let Some(e) = r.error {
        return Err(format!("YouTube: {}", e.message));
    }
    Ok(r.items.into_iter().filter_map(|i| i.id.and_then(|x| x.channel_id)).collect())
}

/// Channel details (description carries the contact) for up to 50 ids.
pub async fn channels(key: &str, ids: &[String]) -> Result<Vec<Channel>, String> {
    if ids.is_empty() {
        return Ok(vec![]);
    }
    let url = format!(
        "{}/channels?part=snippet,statistics&id={}&key={}",
        API,
        ids.join(","),
        key
    );
    let r: ChannelsResp = http()
        .get(&url)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    if let Some(e) = r.error {
        return Err(format!("YouTube: {}", e.message));
    }
    Ok(r.items)
}
