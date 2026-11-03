//! What is left on every paid service the autopilot runs on, in one place
//! (Admin, Crédits), so a run never stops on an empty account unnoticed.
//!
//! - Serper (web search): live balance, GET google.serper.dev/account.
//! - MillionVerifier (mailbox check): live balance, GET /api/v3/credits.
//! - Pushover (iPhone notifications): messages left this month.
//! - Claude (Anthropic): no balance API for a plain key. The user types the
//!   balance after a top-up; what the app spent since (ai_usage) is taken off.
//!   A "credit balance is too low" answer marks it empty right away.
//!
//! A daily snapshot per service (credit_snapshots) gives the burn rate and an
//! estimate of the days left. Once a day the engine notifies when one runs low.

use std::sync::Mutex;

use rusqlite::{params, Connection};
use serde::Serialize;
use tauri::State;

use crate::AppState;

pub const ANTHROPIC_BALANCE_KEY: &str = "anthropic_credit_usd";
pub const ANTHROPIC_BALANCE_AT_KEY: &str = "anthropic_credit_at";
const DAY_KEY: &str = "credits_check_day";
const CHECK_HOUR: u32 = 9;

static ANTHROPIC_EMPTY: Mutex<Option<String>> = Mutex::new(None);

/// Claude answered that the account has no credit left.
pub fn mark_anthropic_empty() {
    *ANTHROPIC_EMPTY.lock().unwrap_or_else(|e| e.into_inner()) = Some(chrono::Utc::now().to_rfc3339());
}

pub fn migrate(conn: &Connection) {
    let _ = conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS credit_snapshots (
            service TEXT NOT NULL,
            day     TEXT NOT NULL,
            value   REAL NOT NULL,
            PRIMARY KEY (service, day)
        );",
    );
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Credit {
    /// "claude", "serper", "millionverifier", "pushover".
    pub service: String,
    pub key_set: bool,
    pub remaining: Option<f64>,
    /// "usd", "searches", "checks", "messages".
    pub unit: String,
    /// "ok", "low", "empty", "error", "unset", "unknown".
    pub status: String,
    pub detail: Option<String>,
    /// Average use per day over the last week, when known.
    pub per_day: Option<f64>,
    pub days_left: Option<f64>,
    pub spent_month: Option<f64>,
    pub manage_url: String,
}

fn threshold(service: &str) -> f64 {
    match service {
        "claude" => 5.0,
        "serper" => 2000.0,
        "millionverifier" => 300.0,
        "pushover" => 500.0,
        _ => 0.0,
    }
}

fn status_for(service: &str, remaining: f64, days_left: Option<f64>) -> &'static str {
    if remaining <= 0.0 {
        "empty"
    } else if remaining < threshold(service) || days_left.is_some_and(|d| d < 7.0) {
        "low"
    } else {
        "ok"
    }
}

fn key(conn: &Connection, k: &str) -> Option<String> {
    crate::secrets::read_setting(conn, k).filter(|v| !v.trim().is_empty())
}

fn http() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .unwrap_or_else(|_| reqwest::blocking::Client::new())
}

fn get_json(rb: reqwest::blocking::RequestBuilder) -> Result<serde_json::Value, String> {
    let r = rb.send().map_err(|_| "service injoignable".to_string())?;
    let status = r.status().as_u16();
    if status == 401 || status == 403 {
        return Err("clé refusée".into());
    }
    if status >= 400 {
        return Err(format!("erreur HTTP {status}"));
    }
    r.json::<serde_json::Value>().map_err(|_| "réponse illisible".to_string())
}

fn with_param(base: &str, name: &str, value: &str) -> Result<reqwest::Url, String> {
    reqwest::Url::parse_with_params(base, &[(name, value.trim())]).map_err(|_| "adresse invalide".to_string())
}

pub fn parse_serper(v: &serde_json::Value) -> Option<f64> {
    v.get("balance").and_then(|b| b.as_f64())
}

pub fn parse_millionverifier(v: &serde_json::Value) -> Option<f64> {
    v.get("credits").and_then(|b| b.as_f64().or_else(|| b.as_str().and_then(|s| s.parse().ok())))
}

pub fn parse_pushover(v: &serde_json::Value) -> Option<f64> {
    v.get("remaining").and_then(|b| b.as_f64())
}

/// Store today's value and return the average daily use over the last week
/// (drops between snapshots; a top-up is not counted as use).
fn snapshot_and_rate(conn: &Connection, service: &str, value: f64) -> Option<f64> {
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let _ = conn.execute(
        "INSERT INTO credit_snapshots (service, day, value) VALUES (?1, ?2, ?3)
         ON CONFLICT(service, day) DO UPDATE SET value = excluded.value",
        params![service, today, value],
    );
    let rows: Vec<(String, f64)> = conn
        .prepare("SELECT day, value FROM credit_snapshots WHERE service = ?1 AND day >= date('now','localtime','-7 days') ORDER BY day")
        .and_then(|mut st| st.query_map([service], |r| Ok((r.get(0)?, r.get(1)?))).map(|it| it.flatten().collect()))
        .unwrap_or_default();
    rate(&rows)
}

/// Average daily use from (day, value) snapshots, oldest first.
pub fn rate(rows: &[(String, f64)]) -> Option<f64> {
    if rows.len() < 2 {
        return None;
    }
    let parse = |d: &str| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok();
    let (first, last) = (parse(&rows[0].0)?, parse(&rows[rows.len() - 1].0)?);
    let days = (last - first).num_days();
    if days < 1 {
        return None;
    }
    let used: f64 = rows.windows(2).map(|w| (w[0].1 - w[1].1).max(0.0)).sum();
    Some(used / days as f64)
}

fn days_left(remaining: f64, per_day: Option<f64>) -> Option<f64> {
    per_day.filter(|r| *r > 0.0).map(|r| remaining / r)
}

fn live(conn: &Connection, service: &str, unit: &str, manage_url: &str, key_name: &str, fetch: impl Fn(&str) -> Result<Option<f64>, String>) -> Credit {
    let mut c = Credit {
        service: service.into(),
        key_set: false,
        remaining: None,
        unit: unit.into(),
        status: "unset".into(),
        detail: None,
        per_day: None,
        days_left: None,
        spent_month: None,
        manage_url: manage_url.into(),
    };
    let Some(k) = key(conn, key_name) else { return c };
    c.key_set = true;
    match fetch(&k) {
        Ok(Some(v)) => {
            c.per_day = snapshot_and_rate(conn, service, v);
            c.days_left = days_left(v, c.per_day);
            c.status = status_for(service, v, c.days_left).into();
            c.remaining = Some(v);
        }
        Ok(None) => {
            c.status = "error".into();
            c.detail = Some("réponse illisible".into());
        }
        Err(e) => {
            c.status = "error".into();
            c.detail = Some(e);
        }
    }
    c
}

fn claude(conn: &Connection) -> Credit {
    let month = super::residency::claude::month_cost(conn, None).cost_usd;
    let per_day: Option<f64> = conn
        .query_row("SELECT COALESCE(SUM(cost_usd), 0) FROM ai_usage WHERE at >= datetime('now', '-7 days')", [], |r| r.get::<_, f64>(0))
        .ok()
        .map(|w| w / 7.0)
        .filter(|r| *r > 0.0);
    let mut c = Credit {
        service: "claude".into(),
        key_set: key(conn, "anthropic_api_key").is_some(),
        remaining: None,
        unit: "usd".into(),
        status: "unknown".into(),
        detail: None,
        per_day,
        days_left: None,
        spent_month: Some(month),
        manage_url: "https://console.anthropic.com/settings/billing".into(),
    };
    if !c.key_set {
        c.status = "unset".into();
        return c;
    }
    let declared: Option<f64> = super::residency::engine::get(conn, ANTHROPIC_BALANCE_KEY).and_then(|v| v.parse().ok());
    let at = super::residency::engine::get(conn, ANTHROPIC_BALANCE_AT_KEY);
    if let (Some(d), Some(at)) = (declared, at) {
        let spent: f64 = conn
            .query_row("SELECT COALESCE(SUM(cost_usd), 0) FROM ai_usage WHERE at >= ?1", [&at], |r| r.get(0))
            .unwrap_or(0.0);
        let left = d - spent;
        c.remaining = Some(left);
        c.days_left = days_left(left, per_day);
        c.status = status_for("claude", left, c.days_left).into();
        c.detail = Some(format!("solde saisi le {}", at.get(..10).unwrap_or(&at)));
    }
    if let Some(when) = ANTHROPIC_EMPTY.lock().unwrap_or_else(|e| e.into_inner()).clone() {
        c.status = "empty".into();
        c.detail = Some(format!("Claude a répondu « crédit épuisé » ({})", when.get(..10).unwrap_or(&when)));
    }
    c
}

/// Every service, live where an API exists.
pub fn overview(conn: &Connection) -> Vec<Credit> {
    let client = http();
    vec![
        claude(conn),
        live(conn, "serper", "searches", "https://serper.dev/billing", "serper_api_key", |k| {
            get_json(client.get("https://google.serper.dev/account").header("X-API-KEY", k.trim())).map(|v| parse_serper(&v))
        }),
        live(conn, "millionverifier", "checks", "https://app.millionverifier.com/", "email_verifier_key", |k| {
            get_json(client.get(with_param("https://api.millionverifier.com/api/v3/credits", "api", k)?))
                .map(|v| parse_millionverifier(&v))
        }),
        live(conn, "pushover", "messages", "https://pushover.net/apps", "pushover_token", |k| {
            get_json(client.get(with_param("https://api.pushover.net/1/apps/limits.json", "token", k)?))
                .map(|v| parse_pushover(&v))
        }),
    ]
}

/// The services that need a top-up, as one line per service.
pub fn alerts(list: &[Credit]) -> Vec<String> {
    list.iter()
        .filter(|c| c.key_set && matches!(c.status.as_str(), "low" | "empty"))
        .map(|c| {
            let name = match c.service.as_str() {
                "claude" => "Claude",
                "serper" => "Serper",
                "millionverifier" => "MillionVerifier",
                "pushover" => "Pushover",
                other => other,
            };
            if c.status == "empty" {
                format!("{name} n'a plus de crédit")
            } else {
                let left = c.remaining.map(|r| if c.unit == "usd" { format!("{r:.2} $") } else { format!("{r:.0}") }).unwrap_or_default();
                match c.days_left {
                    Some(d) => format!("{name} bientôt à sec ({left}, environ {d:.0} jours)"),
                    None => format!("{name} bientôt à sec ({left})"),
                }
            }
        })
        .collect()
}

/// Once a day from CHECK_HOUR: check every balance, notify when one is low.
/// Called from the engine tick (blocking HTTP, a few requests a day).
pub fn daily_check(conn: &Connection, app: &tauri::AppHandle, local: chrono::DateTime<chrono::Local>) {
    if cfg!(test) {
        return;
    }
    use chrono::Timelike;
    let today = local.format("%Y-%m-%d").to_string();
    if local.hour() < CHECK_HOUR || super::residency::engine::get(conn, DAY_KEY).as_deref() == Some(today.as_str()) {
        return;
    }
    super::residency::engine::put(conn, DAY_KEY, &today);
    let lines = alerts(&overview(conn));
    if lines.is_empty() {
        return;
    }
    let body = format!("{}. Admin, Crédits pour recharger.", lines.join(", "));
    super::residency::log(conn, None, "warn", "credits_low", &body);
    use tauri_plugin_notification::NotificationExt;
    let _ = app.notification().builder().title("Crédits à recharger").body(&body).show();
}

#[tauri::command(async)]
pub fn credits_overview(state: State<'_, AppState>) -> Result<Vec<Credit>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    Ok(overview(&conn))
}

/// The Claude balance shown in the Anthropic console, typed after a top-up.
#[tauri::command(async)]
pub fn credits_set_claude_balance(state: State<'_, AppState>, usd: f64) -> Result<Vec<Credit>, String> {
    if !usd.is_finite() || usd < 0.0 {
        return Err("Montant invalide".into());
    }
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    super::residency::engine::put(&conn, ANTHROPIC_BALANCE_KEY, &format!("{usd}"));
    super::residency::engine::put(&conn, ANTHROPIC_BALANCE_AT_KEY, &chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string());
    *ANTHROPIC_EMPTY.lock().unwrap_or_else(|e| e.into_inner()) = None;
    Ok(overview(&conn))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_balances() {
        assert_eq!(parse_serper(&serde_json::json!({"balance": 49850, "rateLimit": 50})), Some(49850.0));
        assert_eq!(parse_millionverifier(&serde_json::json!({"credits": "1200"})), Some(1200.0));
        assert_eq!(parse_pushover(&serde_json::json!({"limit": 10000, "remaining": 7496})), Some(7496.0));
    }

    #[test]
    fn burn_rate_ignores_top_ups() {
        let rows = vec![
            ("2026-11-01".to_string(), 1000.0),
            ("2026-11-02".to_string(), 800.0),
            ("2026-11-03".to_string(), 50_000.0),
            ("2026-11-04".to_string(), 49_800.0),
        ];
        assert_eq!(rate(&rows), Some(400.0 / 3.0));
        assert_eq!(rate(&rows[..1]), None);
    }

    #[test]
    fn low_and_empty() {
        assert_eq!(status_for("serper", 0.0, None), "empty");
        assert_eq!(status_for("serper", 1500.0, None), "low");
        assert_eq!(status_for("serper", 40_000.0, Some(5.0)), "low");
        assert_eq!(status_for("claude", 20.0, Some(60.0)), "ok");
        let c = Credit {
            service: "serper".into(),
            key_set: true,
            remaining: Some(1500.0),
            unit: "searches".into(),
            status: "low".into(),
            detail: None,
            per_day: Some(150.0),
            days_left: Some(10.0),
            spent_month: None,
            manage_url: String::new(),
        };
        assert_eq!(alerts(&[c]), vec!["Serper bientôt à sec (1500, environ 10 jours)".to_string()]);
    }
}
