//! MusicBrainz client (open database, no auth), isolated on purpose. Used by
//! Radar to map record labels by genre; their official homepage then feeds the
//! same crawl+Serper enrichment to find demo/submission emails.
//! MusicBrainz asks for a descriptive User-Agent and max 1 request/second.

use serde::Deserialize;

const WS: &str = "https://musicbrainz.org/ws/2";
const UA: &str = "PiersCRM/1.0 (thibault@insrt.fr)";

#[derive(Deserialize)]
struct LabelSearch {
    #[serde(default)]
    labels: Vec<LabelHit>,
}
#[derive(Deserialize)]
pub struct LabelHit {
    pub id: String,
    pub name: Option<String>,
    #[serde(rename = "type")]
    pub type_: Option<String>,
    pub area: Option<Area>,
}
#[derive(Deserialize)]
pub struct Area {
    pub name: Option<String>,
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(UA)
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .expect("mb client")
}

/// Labels tagged with the given genre. MusicBrainz supports `tag:"..."` in the
/// label search index.
pub async fn search_labels(genre: &str, limit: i64) -> Result<Vec<LabelHit>, String> {
    let q = format!("tag:\"{}\"", genre);
    let url = format!(
        "{}/label?query={}&limit={}&fmt=json",
        WS,
        urlencoding(&q),
        limit.clamp(1, 100)
    );
    let resp = client().get(&url).send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("MusicBrainz HTTP {}", resp.status().as_u16()));
    }
    let r: LabelSearch = resp.json().await.map_err(|e| e.to_string())?;
    Ok(r.labels)
}

/// Official homepage of a label from its URL relations, if any.
pub async fn label_homepage(mbid: &str) -> Option<String> {
    let url = format!("{}/label/{}?inc=url-rels&fmt=json", WS, mbid);
    let v: serde_json::Value = client().get(&url).send().await.ok()?.json().await.ok()?;
    let rels = v.get("relations")?.as_array()?;
    // Prefer "official homepage", else the first non-social url.
    let pick = |want_official: bool| -> Option<String> {
        for rel in rels {
            let ty = rel.get("type").and_then(|t| t.as_str()).unwrap_or("");
            let res = rel.get("url").and_then(|u| u.get("resource")).and_then(|r| r.as_str());
            if let Some(res) = res {
                if want_official && ty == "official homepage" {
                    return Some(res.to_string());
                }
                if !want_official {
                    let low = res.to_lowercase();
                    if ["facebook", "instagram", "twitter", "youtube", "discogs", "wikidata", "wikipedia", "soundcloud", "bandcamp", "spotify"]
                        .iter()
                        .all(|s| !low.contains(s))
                    {
                        return Some(res.to_string());
                    }
                }
            }
        }
        None
    };
    pick(true).or_else(|| pick(false))
}

fn urlencoding(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            b' ' => "%20".into(),
            _ => format!("%{:02X}", b),
        })
        .collect()
}
