//! Resident Advisor GraphQL client. ISOLATED ON PURPOSE.
//!
//! RA exposes a public, unauthenticated GraphQL API at https://ra.co/graphql.
//! There is no official documentation and **the schema changes without notice**.
//! This is the known fragile point of the whole module. If harvesting suddenly
//! returns "réponse RA inattendue", the query below probably drifted:
//!   1. open https://ra.co/events in Chrome, DevTools, Network tab,
//!   2. filter on "graphql", trigger a listing, copy a working request,
//!   3. update EVENT_LISTINGS_QUERY / AREA_QUERY here, nothing else references them.
//!
//! Area ids are resolved via the `area(countryUrlCode, areaUrlName)` query, which
//! works from the API directly (the HTML pages are behind Cloudflare, so the old
//! __NEXT_DATA__ scraping is not usable server-side).

use serde::Deserialize;
use serde_json::json;

const ENDPOINT: &str = "https://ra.co/graphql";

const AREA_QUERY: &str = r#"
query VI_AREA($c: String, $u: String) {
  area(countryUrlCode: $c, areaUrlName: $u) {
    id name urlName country { name urlCode }
  }
}"#;

const VENUE_QUERY: &str = r#"
query VI_VENUE($id: ID!) {
  venue(id: $id) { id name website phone address capacity blurb }
}"#;

const EVENT_LISTINGS_QUERY: &str = r#"
query VI_EVENT_LISTINGS($filters: FilterInputDtoInput, $pageSize: Int, $page: Int) {
  eventListings(filters: $filters, pageSize: $pageSize, page: $page,
                sort: { listingDate: { order: ASCENDING } }) {
    data {
      id
      listingDate
      event {
        id title date contentUrl
        venue { id name contentUrl area { id name urlName country { name urlCode } } }
        artists { id name }
        promoters { id name }
      }
    }
    totalResults
  }
}"#;

/// Build a browser-like HTTP client. RA rejects requests without these headers.
pub fn client() -> reqwest::Client {
    let mut h = reqwest::header::HeaderMap::new();
    h.insert("Referer", "https://ra.co/events".parse().unwrap());
    h.insert("Origin", "https://ra.co".parse().unwrap());
    h.insert("Accept", "*/*".parse().unwrap());
    h.insert("Accept-Language", "en-US,en;q=0.9".parse().unwrap());
    reqwest::Client::builder()
        .user_agent(
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36",
        )
        .default_headers(h)
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("reqwest client")
}

#[derive(Deserialize)]
struct GqlResp<T> {
    data: Option<T>,
    errors: Option<Vec<GqlError>>,
}
#[derive(Deserialize)]
struct GqlError {
    message: String,
}

/// Raw POST. `status` is surfaced so the caller can back off on 429/5xx.
#[derive(Debug)]
pub struct RaError {
    pub message: String,
    pub status: Option<u16>,
    pub retryable: bool,
}
impl RaError {
    fn msg(m: impl Into<String>) -> Self {
        Self { message: m.into(), status: None, retryable: false }
    }
}

async fn post<T: for<'de> Deserialize<'de>>(
    client: &reqwest::Client,
    body: serde_json::Value,
) -> Result<T, RaError> {
    let resp = client
        .post(ENDPOINT)
        .json(&body)
        .send()
        .await
        .map_err(|e| RaError { message: e.to_string(), status: None, retryable: true })?;
    let status = resp.status();
    let code = status.as_u16();
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
        return Err(RaError {
            message: format!("RA HTTP {}", code),
            status: Some(code),
            retryable: true,
        });
    }
    if !status.is_success() {
        return Err(RaError {
            message: format!("RA HTTP {} (Cloudflare ou schéma ?), voir commands/ra.rs", code),
            status: Some(code),
            retryable: code == 403,
        });
    }
    let parsed: GqlResp<T> = resp
        .json()
        .await
        .map_err(|e| RaError::msg(format!("réponse RA illisible: {}, voir commands/ra.rs", e)))?;
    if let Some(errs) = parsed.errors {
        let m = errs.into_iter().map(|e| e.message).collect::<Vec<_>>().join("; ");
        return Err(RaError::msg(format!("GraphQL RA: {}, voir commands/ra.rs", m)));
    }
    parsed
        .data
        .ok_or_else(|| RaError::msg("réponse RA inattendue (schéma changé ?), voir commands/ra.rs"))
}

// ---------------- Area resolution ----------------

#[derive(Deserialize)]
struct AreaData {
    area: Option<AreaNode>,
}
#[derive(Deserialize)]
struct AreaNode {
    id: String,
}

/// Resolve an RA area id from a country url code (e.g. "FR") and an area url name
/// (e.g. "paris"). Returns None when RA does not know that slug.
pub async fn resolve_area(
    client: &reqwest::Client,
    country_code: &str,
    url_name: &str,
) -> Result<Option<i64>, RaError> {
    let body = json!({
        "query": AREA_QUERY,
        "variables": { "c": country_code, "u": url_name }
    });
    let data: AreaData = post(client, body).await?;
    Ok(data.area.and_then(|a| a.id.parse::<i64>().ok()))
}

// ---------------- Event listings ----------------

#[derive(Deserialize)]
pub struct ListingsData {
    #[serde(rename = "eventListings")]
    pub event_listings: Listings,
}
#[derive(Deserialize)]
pub struct Listings {
    pub data: Vec<Listing>,
    #[serde(rename = "totalResults")]
    pub total_results: i64,
}
#[derive(Deserialize)]
pub struct Listing {
    pub event: Option<EventNode>,
}
#[derive(Deserialize)]
pub struct EventNode {
    pub id: String,
    pub title: Option<String>,
    pub date: Option<String>,
    #[serde(rename = "contentUrl")]
    pub content_url: Option<String>,
    pub venue: Option<VenueNode>,
    #[serde(default)]
    pub artists: Vec<NamedNode>,
    #[serde(default)]
    pub promoters: Vec<NamedNode>,
}
#[derive(Deserialize)]
pub struct VenueNode {
    pub id: String,
    pub name: String,
    #[serde(rename = "contentUrl")]
    pub content_url: Option<String>,
    pub area: Option<AreaInfo>,
}
#[derive(Deserialize)]
pub struct AreaInfo {
    pub name: Option<String>,
    pub country: Option<CountryInfo>,
}
#[derive(Deserialize)]
pub struct CountryInfo {
    #[serde(rename = "urlCode")]
    pub url_code: Option<String>,
}
#[derive(Deserialize)]
pub struct NamedNode {
    pub name: String,
}

// ---------------- Venue detail ----------------

#[derive(Deserialize)]
struct VenueData {
    venue: Option<VenueDetailNode>,
}
#[derive(Deserialize, Default)]
pub struct VenueDetailNode {
    pub website: Option<String>,
    pub phone: Option<String>,
    pub address: Option<String>,
    pub capacity: Option<String>,
    pub blurb: Option<String>,
}

/// Fetch a venue's detail (website, phone, address, capacity) straight from RA.
pub async fn fetch_venue_detail(
    client: &reqwest::Client,
    ra_venue_id: i64,
) -> Result<VenueDetailNode, RaError> {
    let body = json!({ "query": VENUE_QUERY, "variables": { "id": ra_venue_id.to_string() } });
    let data: VenueData = post(client, body).await?;
    Ok(data.venue.unwrap_or_default())
}

/// Fetch one page of event listings for an area over a date window (ISO 8601).
pub async fn fetch_listings(
    client: &reqwest::Client,
    area_id: i64,
    gte: &str,
    lte: &str,
    page: i64,
    page_size: i64,
) -> Result<Listings, RaError> {
    let body = json!({
        "query": EVENT_LISTINGS_QUERY,
        "variables": {
            "filters": { "areas": { "eq": area_id }, "listingDate": { "gte": gte, "lte": lte } },
            "pageSize": page_size,
            "page": page
        }
    });
    let data: ListingsData = post(client, body).await?;
    Ok(data.event_listings)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Live smoke test against the real RA API: proves the query + serde structs
    // still match today. Run with: cargo test live_ra_smoke -- --nocapture
    #[test]
    fn live_ra_smoke() {
        let client = client();
        tauri::async_runtime::block_on(async {
            let id = resolve_area(&client, "ES", "ibiza").await.expect("resolve");
            println!("ibiza area id = {:?}", id);
            assert_eq!(id, Some(25));
            let page = fetch_listings(
                &client,
                25,
                "2024-06-01T00:00:00.000Z",
                "2024-06-30T23:59:59.999Z",
                1,
                20,
            )
            .await
            .expect("listings");
            let events: Vec<_> = page.data.iter().filter_map(|l| l.event.as_ref()).collect();
            let with_venue = events.iter().filter(|e| e.venue.is_some()).count();
            let with_artists = events.iter().filter(|e| !e.artists.is_empty()).count();
            println!(
                "ibiza jun2024: {} listings, {} events, {} venue, {} with artists, total={}",
                page.data.len(),
                events.len(),
                with_venue,
                with_artists,
                page.total_results
            );
            if let Some(e) = events.first() {
                println!(
                    "sample: {:?} @ {:?} artists={:?}",
                    e.title,
                    e.venue.as_ref().map(|v| &v.name),
                    e.artists.iter().map(|a| &a.name).collect::<Vec<_>>()
                );
            }
            assert!(events.len() > 0, "expected events");
        });
    }
}
