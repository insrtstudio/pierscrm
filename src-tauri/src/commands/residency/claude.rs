//! Claude (Anthropic Messages API) for the residency agent, over raw HTTPS:
//! there is no official Rust SDK, so this is the documented REST shape
//! (POST /v1/messages, `x-api-key`, `anthropic-version`).
//!
//! Cost first: every call defaults to claude-haiku-5-5 (setting `ai_model` can
//! move drafts to a Sonnet), thinking off and effort "low" where the model
//! allows it, tight max_tokens, the shared system prompt + campaign block
//! marked for prompt caching, structured JSON output where the result is
//! machine-read. Each call's token usage lands in `ai_usage` for the
//! "Coût IA ce mois-ci" estimate.
//!
//! Everything goes through the `Llm` trait so tests use a fake and never
//! reach the network.

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const DEFAULT_MODEL: &str = "claude-haiku-5-5";
pub const API_URL: &str = "https://api.anthropic.com/v1/messages";
pub const API_VERSION: &str = "2026-10-21";

/// One system block. `cache` marks the end of the cached prefix.
#[derive(Debug, Clone)]
pub struct SystemBlock {
    pub text: String,
    pub cache: bool,
}

#[derive(Debug, Clone)]
pub struct LlmRequest {
    pub model: String,
    pub max_tokens: u32,
    pub system: Vec<SystemBlock>,
    pub user: String,
    /// JSON schema for structured output (None = free text).
    pub schema: Option<Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read: i64,
    pub cache_write: i64,
}

#[derive(Debug, Clone)]
pub struct LlmResponse {
    pub text: String,
    pub model: String,
    pub stop_reason: String,
    pub usage: Usage,
}

pub trait Llm: Send + Sync {
    fn complete(&self, req: &LlmRequest) -> Result<LlmResponse, String>;
}

// ---------------- Request shape ----------------

/// The JSON body for a request. Thinking is turned off where the model allows
/// it (tight max_tokens would otherwise be eaten by reasoning): Haiku 5.5
/// accepts `disabled` at effort low, Sonnet 5.5 needs `between_tools`, the
/// Opus/Fable lines cannot turn it off (effort low then, more room).
pub fn request_body(req: &LlmRequest) -> Value {
    let system: Vec<Value> = req
        .system
        .iter()
        .map(|b| {
            if b.cache {
                json!({"type": "text", "text": b.text, "cache_control": {"type": "ephemeral"}})
            } else {
                json!({"type": "text", "text": b.text})
            }
        })
        .collect();
    let mut output_config = json!({"effort": "low"});
    if let Some(schema) = &req.schema {
        output_config["format"] = json!({"type": "json_schema", "schema": schema});
    }
    let thinking_off = thinking_off_value(&req.model);
    let max_tokens = if thinking_off.is_some() { req.max_tokens } else { req.max_tokens * 4 };
    let mut body = json!({
        "model": req.model,
        "max_tokens": max_tokens,
        "system": system,
        "messages": [{"role": "user", "content": req.user}],
        "output_config": output_config,
    });
    if let Some(t) = thinking_off {
        body["thinking"] = t;
    }
    // Server-side refusal fallback where the API offers it (not on Haiku).
    if uses_fallbacks(&req.model) {
        body["fallbacks"] = json!("default");
    }
    body
}

fn thinking_off_value(model: &str) -> Option<Value> {
    if model.starts_with("claude-haiku-5") {
        Some(json!({"type": "disabled"}))
    } else if model.starts_with("claude-sonnet-5-5") {
        Some(json!({"type": "between_tools"}))
    } else {
        None
    }
}

pub fn uses_fallbacks(model: &str) -> bool {
    model.starts_with("claude-sonnet-5-5") || model.starts_with("claude-opus-5") || model.starts_with("claude-fable-5-1")
}

/// Parse a Messages API response body.
pub fn parse_response(raw: &str) -> Result<LlmResponse, String> {
    let v: Value = serde_json::from_str(raw).map_err(|e| format!("réponse Claude illisible: {}", e))?;
    if let Some(err) = v.get("error") {
        return Err(format!(
            "Claude: {}",
            err.get("message").and_then(Value::as_str).unwrap_or("erreur inconnue")
        ));
    }
    let stop_reason = v.get("stop_reason").and_then(Value::as_str).unwrap_or("").to_string();
    if stop_reason == "refusal" {
        return Err("Claude a refusé la demande.".into());
    }
    let text = v
        .get("content")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();
    let u = v.get("usage").cloned().unwrap_or(Value::Null);
    let n = |k: &str| u.get(k).and_then(Value::as_i64).unwrap_or(0);
    Ok(LlmResponse {
        text,
        model: v.get("model").and_then(Value::as_str).unwrap_or("").to_string(),
        stop_reason,
        usage: Usage {
            input_tokens: n("input_tokens"),
            output_tokens: n("output_tokens"),
            cache_read: n("cache_read_input_tokens"),
            cache_write: n("cache_creation_input_tokens"),
        },
    })
}

// ---------------- HTTP client ----------------

pub struct HttpClaude {
    pub api_key: String,
    http: reqwest::blocking::Client,
}

impl HttpClaude {
    pub fn new(api_key: String) -> Self {
        let http = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(90))
            .build()
            .unwrap_or_else(|_| reqwest::blocking::Client::new());
        HttpClaude { api_key, http }
    }
}

impl Llm for HttpClaude {
    fn complete(&self, req: &LlmRequest) -> Result<LlmResponse, String> {
        let body = request_body(req);
        let mut rb = self
            .http
            .post(API_URL)
            .header("x-api-key", self.api_key.trim())
            .header("anthropic-version", API_VERSION)
            .header("content-type", "application/json");
        if uses_fallbacks(&req.model) {
            rb = rb.header("anthropic-beta", "server-side-fallback-2026-07-01");
        }
        // One retry on rate limit / overload / network error.
        let mut last_err = String::new();
        for attempt in 0..2 {
            let resp = rb
                .try_clone()
                .ok_or_else(|| "requête non clonable".to_string())?
                .json(&body)
                .send();
            match resp {
                Ok(r) => {
                    let status = r.status().as_u16();
                    let text = r.text().map_err(|e| e.to_string())?;
                    if status == 429 || status >= 500 {
                        last_err = format!("Claude HTTP {}", status);
                        if attempt == 0 {
                            std::thread::sleep(std::time::Duration::from_secs(3));
                            continue;
                        }
                        return Err(last_err);
                    }
                    if status == 401 || status == 403 {
                        return Err("Clé API Anthropic refusée (Réglages, Intégrations).".into());
                    }
                    return parse_response(&text);
                }
                Err(e) => {
                    last_err = e.to_string();
                    if attempt == 0 {
                        std::thread::sleep(std::time::Duration::from_secs(3));
                    }
                }
            }
        }
        Err(last_err)
    }
}

/// The configured client, or None when no API key is set.
pub fn client(conn: &Connection) -> Option<Box<dyn Llm>> {
    let key = crate::secrets::read_setting(conn, "anthropic_api_key").filter(|k| !k.trim().is_empty())?;
    Some(Box::new(HttpClaude::new(key)))
}

/// Model for drafts and the daily synthesis (setting `ai_model`), Haiku by
/// default. Classification always uses Haiku: it is a routing task.
pub fn drafting_model(conn: &Connection) -> String {
    crate::secrets::read_setting(conn, "ai_model")
        .map(|m| m.trim().to_string())
        .filter(|m| m.starts_with("claude-"))
        .unwrap_or_else(|| DEFAULT_MODEL.to_string())
}

// ---------------- Usage + cost ----------------

/// USD per million tokens (input, output), first-party API rates.
pub fn price_per_mtok(model: &str) -> (f64, f64) {
    if model.starts_with("claude-haiku-5") {
        (0.10, 0.50)
    } else if model.starts_with("claude-haiku-4") {
        (1.0, 5.0)
    } else if model.starts_with("claude-sonnet-5") {
        (2.0, 10.0)
    } else if model.starts_with("claude-sonnet") {
        (3.0, 15.0)
    } else if model.starts_with("claude-opus-5-5") {
        (4.0, 20.0)
    } else if model.starts_with("claude-opus") {
        (5.0, 25.0)
    } else {
        (10.0, 50.0)
    }
}

/// Cost of one call in USD: uncached input, cache reads at 0.1x, 5-minute
/// cache writes at 1.25x, output.
pub fn cost_usd(model: &str, u: &Usage) -> f64 {
    let (inp, out) = price_per_mtok(model);
    (u.input_tokens as f64 * inp
        + u.cache_read as f64 * inp * 0.1
        + u.cache_write as f64 * inp * 1.25
        + u.output_tokens as f64 * out)
        / 1_000_000.0
}

/// Rough USD -> EUR rate for the estimate shown in the app.
pub const USD_TO_EUR: f64 = 0.92;

pub fn log_usage(conn: &Connection, purpose: &str, campaign_id: Option<i64>, model: &str, u: &Usage) {
    let _ = conn.execute(
        "INSERT INTO ai_usage (purpose, campaign_id, model, input_tokens, output_tokens, cache_read, cache_write, cost_usd)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![purpose, campaign_id, model, u.input_tokens, u.output_tokens, u.cache_read, u.cache_write, cost_usd(model, u)],
    );
}

#[derive(Serialize, Default)]
pub struct AiCost {
    pub calls: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_usd: f64,
    pub cost_eur: f64,
}

/// Usage since the start of the current month (local time), optionally for one campaign.
pub fn month_cost(conn: &Connection, campaign_id: Option<i64>) -> AiCost {
    conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(input_tokens + cache_read + cache_write),0), COALESCE(SUM(output_tokens),0),
                COALESCE(SUM(cost_usd),0)
         FROM ai_usage WHERE at >= datetime('now','localtime','start of month','utc')
           AND (?1 IS NULL OR campaign_id = ?1)",
        params![campaign_id],
        |r| {
            let usd: f64 = r.get(3)?;
            Ok(AiCost {
                calls: r.get(0)?,
                input_tokens: r.get(1)?,
                output_tokens: r.get(2)?,
                cost_usd: usd,
                cost_eur: usd * USD_TO_EUR,
            })
        },
    )
    .unwrap_or_default()
}

// ---------------- Text hygiene ----------------

/// No em dash in what the agent writes (house rule for French copy), and no
/// en dash used as one either: a comma reads naturally in their place.
pub fn no_dashes(s: &str) -> String {
    s.replace(" — ", ", ")
        .replace(" – ", ", ")
        .replace("—", ", ")
        .replace(" -- ", ", ")
}

/// The JSON object inside a model answer: the whole text, else the first
/// `{...}` span (tolerates a stray sentence or a code fence around it).
pub fn extract_json(text: &str) -> Option<Value> {
    let t = text.trim();
    if let Ok(v) = serde_json::from_str::<Value>(t) {
        return Some(v);
    }
    let start = t.find('{')?;
    let end = t.rfind('}')?;
    (end > start).then(|| serde_json::from_str(&t[start..=end]).ok()).flatten()
}

#[cfg(test)]
pub mod fake {
    //! A scripted `Llm` for tests: answers are popped in order, requests kept.
    use super::*;
    use std::sync::Mutex;

pub struct FakeLlm {
    pub answers: Mutex<Vec<String>>,
    pub seen: Mutex<Vec<LlmRequest>>,
}
impl FakeLlm {
    pub fn new(answers: &[&str]) -> Self {
        FakeLlm { answers: Mutex::new(answers.iter().rev().map(|s| s.to_string()).collect()), seen: Mutex::new(vec![]) }
    }
}
impl Llm for FakeLlm {
    fn complete(&self, req: &LlmRequest) -> Result<LlmResponse, String> {
        self.seen.lock().unwrap().push(req.clone());
        let text = self.answers.lock().unwrap().pop().ok_or("no more answers")?;
        Ok(LlmResponse {
            text,
            model: req.model.clone(),
            stop_reason: "end_turn".into(),
            usage: Usage { input_tokens: 100, output_tokens: 50, cache_read: 900, cache_write: 0 },
        })
    }
}

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_shape_per_model() {
        let req = LlmRequest {
            model: DEFAULT_MODEL.into(),
            max_tokens: 200,
            system: vec![
                SystemBlock { text: "rules".into(), cache: false },
                SystemBlock { text: "campaign".into(), cache: true },
            ],
            user: "hi".into(),
            schema: Some(json!({"type": "object"})),
        };
        let b = request_body(&req);
        assert_eq!(b["model"], "claude-haiku-5-5");
        assert_eq!(b["max_tokens"], 200);
        assert_eq!(b["thinking"]["type"], "disabled");
        assert_eq!(b["output_config"]["effort"], "low");
        assert_eq!(b["output_config"]["format"]["type"], "json_schema");
        assert!(b["system"][0].get("cache_control").is_none());
        assert_eq!(b["system"][1]["cache_control"]["type"], "ephemeral");
        assert!(b.get("fallbacks").is_none());
        assert!(b.get("temperature").is_none());

        let s = request_body(&LlmRequest { model: "claude-sonnet-5-5".into(), schema: None, ..req.clone() });
        assert_eq!(s["thinking"]["type"], "between_tools");
        assert_eq!(s["fallbacks"], "default");
        assert!(s["output_config"].get("format").is_none());
        let o = request_body(&LlmRequest { model: "claude-opus-5-5".into(), ..req });
        assert!(o.get("thinking").is_none());
        assert_eq!(o["max_tokens"], 800);
    }

    #[test]
    fn response_parsing_and_cost() {
        let raw = r#"{"id":"msg_1","model":"claude-haiku-5-5","stop_reason":"end_turn",
            "content":[{"type":"thinking","thinking":""},{"type":"text","text":"{\"a\":1}"}],
            "usage":{"input_tokens":1000,"output_tokens":200,"cache_read_input_tokens":5000,"cache_creation_input_tokens":0}}"#;
        let r = parse_response(raw).unwrap();
        assert_eq!(r.text, "{\"a\":1}");
        assert_eq!(r.usage.cache_read, 5000);
        // 1000*0.10 + 5000*0.01 + 200*0.50 = 250 per million.
        assert!((cost_usd(&r.model, &r.usage) - 0.000_25).abs() < 1e-12);
        assert!(parse_response(r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#)
            .unwrap_err()
            .contains("Overloaded"));
        assert!(parse_response(r#"{"stop_reason":"refusal","content":[]}"#).is_err());
        assert_eq!(extract_json("Voici:\n```json\n{\"x\": 2}\n```").unwrap()["x"], 2);
        assert!(extract_json("pas de json").is_none());
        assert_eq!(no_dashes("Bonjour — on joue"), "Bonjour, on joue");
    }
}
