//! Meta (Facebook) Marketing Insights client, READ ONLY, isolated on purpose.
//!
//! Pulls daily campaign-level spend from the Graph API. Never writes anything
//! to Meta. Settings: `meta_access_token` (long-lived), `meta_ad_account_id`
//! (digits, with or without the act_ prefix), `meta_result_action_type`
//! (default "link_click"). An expired token surfaces as a readable error and
//! never breaks the Spotify half of a snapshot run.

use serde::Deserialize;

const GRAPH: &str = "https://graph.facebook.com/v21.0";

#[derive(Debug)]
pub struct MetaError(pub String);

#[derive(Deserialize)]
struct InsightsResp {
    #[serde(default)]
    data: Vec<InsightRow>,
    paging: Option<Paging>,
    error: Option<GraphError>,
}
#[derive(Deserialize)]
struct Paging {
    next: Option<String>,
}
#[derive(Deserialize)]
struct GraphError {
    message: String,
    code: Option<i64>,
}

#[derive(Deserialize)]
pub struct InsightRow {
    pub campaign_id: Option<String>,
    pub campaign_name: Option<String>,
    pub date_start: Option<String>,
    pub spend: Option<String>,
    pub impressions: Option<String>,
    pub clicks: Option<String>,
    #[serde(default)]
    pub actions: Vec<ActionRow>,
}
#[derive(Deserialize)]
pub struct ActionRow {
    pub action_type: Option<String>,
    pub value: Option<String>,
}

pub struct DailySpend {
    pub campaign_id: String,
    pub campaign_name: String,
    pub spend_date: String,
    pub spend: f64,
    pub impressions: Option<i64>,
    pub clicks: Option<i64>,
    pub results: Option<i64>,
    pub cost_per_result: Option<f64>,
}

/// Fetch the last `days` days of campaign-level daily spend (rolling window,
/// because Meta consolidates numbers 24-48h late).
pub async fn fetch_daily_spend(
    token: &str,
    ad_account_id: &str,
    result_action_type: &str,
    days: i64,
) -> Result<Vec<DailySpend>, MetaError> {
    let account = ad_account_id.trim().trim_start_matches("act_");
    let since = (chrono::Utc::now() - chrono::Duration::days(days)).format("%Y-%m-%d");
    let until = chrono::Utc::now().format("%Y-%m-%d");
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(25))
        .build()
        .map_err(|e| MetaError(e.to_string()))?;

    let mut url = format!(
        "{}/act_{}/insights?level=campaign&time_increment=1&fields=campaign_id,campaign_name,spend,impressions,clicks,actions&time_range={{'since':'{}','until':'{}'}}&access_token={}",
        GRAPH, account, since, until, token
    );

    let mut out = Vec::new();
    // Follow pagination, hard-capped to stay bounded.
    for _ in 0..10 {
        let resp = http
            .get(&url)
            .send()
            .await
            .map_err(|e| MetaError(e.to_string()))?;
        let body: InsightsResp = resp
            .json()
            .await
            .map_err(|e| MetaError(format!("parse: {}", e)))?;
        if let Some(err) = body.error {
            let hint = match err.code {
                Some(190) => " (token expiré ou invalide, régénérez un token long-lived)",
                _ => "",
            };
            return Err(MetaError(format!("Graph API: {}{}", err.message, hint)));
        }
        for r in body.data {
            let spend: f64 = r.spend.as_deref().and_then(|s| s.parse().ok()).unwrap_or(0.0);
            let results: Option<i64> = r
                .actions
                .iter()
                .find(|a| a.action_type.as_deref() == Some(result_action_type))
                .and_then(|a| a.value.as_deref())
                .and_then(|v| v.parse().ok());
            let cpr = match results {
                Some(n) if n > 0 => Some(spend / n as f64),
                _ => None,
            };
            out.push(DailySpend {
                campaign_id: r.campaign_id.unwrap_or_default(),
                campaign_name: r.campaign_name.unwrap_or_default(),
                spend_date: r.date_start.unwrap_or_default(),
                spend,
                impressions: r.impressions.as_deref().and_then(|s| s.parse().ok()),
                clicks: r.clicks.as_deref().and_then(|s| s.parse().ok()),
                results,
                cost_per_result: cpr,
            });
        }
        match body.paging.and_then(|p| p.next) {
            Some(next) => url = next,
            None => break,
        }
    }
    Ok(out)
}
