//! Client of the insrt.studio CRM API (press kits, personal private EPK links,
//! link activity). Contract (v1):
//!
//! - `GET  {base}/api/crm/v1/press-kits?artist=<slug>` -> `{"pressKits":[{id,title,artistSlug,updatedAt}]}`
//! - `POST {base}/api/crm/v1/press-links` `{pressKitId, externalRef:"crm:<contact_id>", recipient:{name,email,organisation}, locale}`
//!   -> `{linkId, token, url}` (idempotent per externalRef)
//! - `GET  {base}/api/crm/v1/press-links/activity?since=<ISO>&pressKitId=<id>` -> `{"events":[...], "next": ISO}`
//!
//! `Authorization: Bearer <insrt_api_token>` (a secret, see crate::secrets).
//! The HTTP side sits behind the `Insrt` trait so tests run on fixtures.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

pub const DEFAULT_BASE: &str = "https://insrt.studio";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PressKit {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub artist_slug: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PressLink {
    pub link_id: String,
    #[serde(default)]
    pub token: Option<String>,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ActivityEvent {
    pub id: String,
    #[serde(default)]
    pub link_id: Option<String>,
    #[serde(default)]
    pub external_ref: Option<String>,
    #[serde(rename = "type")]
    pub kind: String,
    pub at: String,
    #[serde(default)]
    pub track_title: Option<String>,
    #[serde(default)]
    pub seconds: Option<f64>,
    #[serde(default)]
    pub completion: Option<f64>,
    #[serde(default)]
    pub rating: Option<f64>,
    #[serde(default)]
    pub comment: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ActivityPage {
    #[serde(default)]
    pub events: Vec<ActivityEvent>,
    #[serde(default)]
    pub next: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Recipient {
    pub name: String,
    pub email: String,
    pub organisation: String,
}

pub trait Insrt: Send + Sync {
    fn press_kits(&self, artist_slug: &str) -> Result<Vec<PressKit>, String>;
    fn press_link(&self, press_kit_id: &str, contact_id: i64, recipient: &Recipient, locale: &str) -> Result<PressLink, String>;
    fn activity(&self, since: &str, press_kit_id: &str) -> Result<ActivityPage, String>;
}

// ---------------- JSON parsing (pure, tested on fixtures) ----------------

pub fn parse_press_kits(raw: &str) -> Result<Vec<PressKit>, String> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct R {
        #[serde(default)]
        press_kits: Vec<PressKit>,
    }
    serde_json::from_str::<R>(raw).map(|r| r.press_kits).map_err(|e| format!("insrt: réponse illisible ({})", e))
}

pub fn parse_press_link(raw: &str) -> Result<PressLink, String> {
    serde_json::from_str(raw).map_err(|e| format!("insrt: réponse illisible ({})", e))
}

pub fn parse_activity(raw: &str) -> Result<ActivityPage, String> {
    serde_json::from_str(raw).map_err(|e| format!("insrt: réponse illisible ({})", e))
}

/// `crm:<contact_id>` -> contact id.
pub fn contact_of_ref(external_ref: &str) -> Option<i64> {
    external_ref.trim().strip_prefix("crm:")?.trim().parse().ok()
}

/// Lowercase ASCII slug of an artist name ("Piers" -> "piers").
pub fn slug(name: &str) -> String {
    let n = crate::db::normalise(name);
    if n.is_empty() {
        "artist".into()
    } else {
        n
    }
}

// ---------------- HTTP ----------------

pub struct HttpInsrt {
    base: String,
    token: String,
    http: reqwest::blocking::Client,
}

impl HttpInsrt {
    pub fn new(base: &str, token: &str) -> Self {
        let http = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| reqwest::blocking::Client::new());
        HttpInsrt { base: base.trim().trim_end_matches('/').to_string(), token: token.trim().to_string(), http }
    }

    fn url(&self, path: &str, q: &[(&str, &str)]) -> Result<reqwest::Url, String> {
        reqwest::Url::parse_with_params(&format!("{}{}", self.base, path), q).map_err(|e| format!("insrt: adresse invalide ({})", e))
    }

    fn check(resp: reqwest::blocking::Response) -> Result<String, String> {
        let status = resp.status().as_u16();
        let body = resp.text().map_err(|e| e.to_string())?;
        match status {
            200..=299 => Ok(body),
            401 | 403 => Err("Jeton insrt invalide (Réglages, Intégrations).".into()),
            404 if body.contains("press_kit_not_found") => Err("insrt: press kit introuvable.".into()),
            // A bare 404 means the CRM API itself is missing: site not deployed yet.
            404 => Err("insrt: l'API CRM n'est pas encore en ligne sur le site (déploiement en attente).".into()),
            400 => Err(format!("insrt: requête refusée ({})", body.chars().take(80).collect::<String>())),
            _ => Err(format!("insrt: HTTP {}", status)),
        }
    }
}

impl Insrt for HttpInsrt {
    fn press_kits(&self, artist_slug: &str) -> Result<Vec<PressKit>, String> {
        let resp = self
            .http
            .get(self.url("/api/crm/v1/press-kits", &[("artist", artist_slug)])?)
            .bearer_auth(&self.token)
            .send()
            .map_err(|e| format!("insrt: {}", e))?;
        parse_press_kits(&Self::check(resp)?)
    }

    fn press_link(&self, press_kit_id: &str, contact_id: i64, recipient: &Recipient, locale: &str) -> Result<PressLink, String> {
        let body = serde_json::json!({
            "pressKitId": press_kit_id,
            "externalRef": format!("crm:{}", contact_id),
            "recipient": recipient,
            "locale": if locale == "en" { "en" } else { "fr" },
        });
        let resp = self
            .http
            .post(format!("{}/api/crm/v1/press-links", self.base))
            .bearer_auth(&self.token)
            .json(&body)
            .send()
            .map_err(|e| format!("insrt: {}", e))?;
        parse_press_link(&Self::check(resp)?)
    }

    fn activity(&self, since: &str, press_kit_id: &str) -> Result<ActivityPage, String> {
        let resp = self
            .http
            .get(self.url("/api/crm/v1/press-links/activity", &[("since", since), ("pressKitId", press_kit_id)])?)
            .bearer_auth(&self.token)
            .send()
            .map_err(|e| format!("insrt: {}", e))?;
        parse_activity(&Self::check(resp)?)
    }
}

/// The configured client, or None when no token is set (emails then carry the
/// campaign's plain asset links).
pub fn client(conn: &Connection) -> Option<Box<dyn Insrt>> {
    let token = crate::secrets::read_setting(conn, "insrt_api_token").filter(|t| !t.trim().is_empty())?;
    let base = crate::secrets::read_setting(conn, "insrt_api_base")
        .filter(|b| b.trim().starts_with("http"))
        .unwrap_or_else(|| DEFAULT_BASE.to_string());
    Some(Box::new(HttpInsrt::new(&base, &token)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_fixtures() {
        let kits = parse_press_kits(
            r#"{"pressKits":[{"id":"pk_1","title":"Piers EPK 2026","artistSlug":"piers","updatedAt":"2026-10-20T10:00:00Z"}]}"#,
        )
        .unwrap();
        assert_eq!(kits[0].id, "pk_1");
        assert_eq!(kits[0].artist_slug.as_deref(), Some("piers"));
        let link = parse_press_link(r#"{"linkId":"ln_9","token":"t0k","url":"https://insrt.studio/p/t0k"}"#).unwrap();
        assert_eq!(link.url, "https://insrt.studio/p/t0k");
        let page = parse_activity(
            r#"{"events":[
                {"id":"ev1","linkId":"ln_9","externalRef":"crm:42","type":"listen","at":"2026-10-28T14:00:00Z","trackTitle":"Mix ADE","seconds":240,"completion":0.4},
                {"id":"ev2","linkId":"ln_9","externalRef":"crm:42","type":"view","at":"2026-10-28T13:59:00Z"},
                {"id":"ev3","externalRef":"crm:42","type":"feedback","at":"2026-10-28T15:00:00Z","rating":5,"comment":"Top"}
            ],"next":"2026-10-28T15:00:01Z"}"#,
        )
        .unwrap();
        assert_eq!(page.events.len(), 3);
        assert_eq!(page.events[0].kind, "listen");
        assert_eq!(page.events[0].seconds, Some(240.0));
        assert_eq!(page.events[2].comment.as_deref(), Some("Top"));
        assert_eq!(page.next.as_deref(), Some("2026-10-28T15:00:01Z"));
        assert!(parse_activity("<html>").is_err());
        assert_eq!(contact_of_ref("crm:42"), Some(42));
        assert_eq!(contact_of_ref("hubspot:1"), None);
        assert_eq!(slug("Piers"), "piers");
    }
}
