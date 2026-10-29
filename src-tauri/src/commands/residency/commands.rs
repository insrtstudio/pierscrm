//! Tauri commands behind the residency campaign page (/campaigns/:id) and the
//! residency items of Agenda › Actions.

use chrono::Utc;
use rusqlite::{params, Connection};
use serde::Serialize;
use tauri::State;

use super::claude::{self, AiCost};
use super::content::{self, GenResult};
use super::engine::{self, fmt};
use super::insrt::{self, PressKit};
use super::report::{self, HotSignal};
use super::targeting::{self, BuildOptions, BuildResult, Pick};
use super::zone::{Zone, PRESETS};
use super::{load, save_config, Config, Residency};
use crate::models::Campaign;
use crate::AppState;

fn conn(state: &State<'_, AppState>) -> Result<r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>, String> {
    state.pool.get().map_err(|e| e.to_string())
}

fn require(conn: &Connection, id: i64) -> Result<Residency, String> {
    load(conn, id).ok_or_else(|| "Campagne résidence introuvable.".to_string())
}

#[derive(Serialize)]
pub struct Preset {
    pub key: &'static str,
    pub label: &'static str,
}

#[tauri::command(async)]
pub fn residency_presets() -> Vec<Preset> {
    PRESETS.iter().map(|(k, l)| Preset { key: k, label: l }).collect()
}

/// City wording for the pre-filled pitch ("un club de Paris" / "a club in London").
fn zone_words(zone: &Zone) -> (String, String) {
    match zone.presets.first().map(String::as_str) {
        Some("paris") => ("Paris".into(), "Paris".into()),
        Some("london") => ("Londres".into(), "London".into()),
        Some("belgium") => ("Belgique (Bruxelles, Anvers, Gand)".into(), "Belgium (Brussels, Antwerp, Ghent)".into()),
        Some("switzerland") => ("Suisse (Zurich, Genève, Bâle)".into(), "Switzerland (Zurich, Geneva, Basel)".into()),
        _ => {
            let c = zone.cities.first().cloned().unwrap_or_else(|| "Paris".into());
            (c.clone(), c)
        }
    }
}

/// "Nouvelle campagne résidence": a campaign of kind residency with the
/// pitch (FR + EN), the content plan and the artist's mix pre-filled.
#[tauri::command(async)]
pub fn residency_create(state: State<'_, AppState>, name: String, zone: Zone) -> Result<i64, String> {
    let conn = conn(&state)?;
    if zone.is_empty() {
        return Err("Choisissez une zone cible.".into());
    }
    let artist_id = crate::commands::artists::default_artist_id(&conn);
    conn.execute(
        "INSERT INTO campaigns (name, purpose, artist_id, status, color, kind) VALUES (?1, ?2, ?3, 'active', '#ff5a1f', 'residency')",
        params![name.trim(), "Résidence mensuelle ou bimestrielle", artist_id],
    )
    .map_err(|e| e.to_string())?;
    let id = conn.last_insert_rowid();
    let (fr, en) = zone_words(&zone);
    let mut cfg = Config { zone, ..Default::default() };
    cfg.pitch_fr = super::default_pitch_fr(&fr);
    cfg.pitch_en = super::default_pitch_en(&en);
    cfg.concept = super::default_concept_fr();
    if let Some(aid) = artist_id {
        let mix: Option<String> = conn
            .query_row("SELECT mix_url FROM artists WHERE id = ?1", params![aid], |r| r.get(0))
            .ok()
            .flatten();
        if let Some(m) = mix.filter(|m| m.trim().starts_with("http")) {
            cfg.assets.push(super::Asset { kind: "mix".into(), title: "Mix".into(), url: m, note: String::new() });
        }
    }
    save_config(&conn, id, &cfg)?;
    super::log(&conn, Some(id), "info", "created", "Campagne résidence créée");
    Ok(id)
}

#[derive(Serialize)]
pub struct StatusLine {
    pub autopilot: bool,
    pub confirmed: bool,
    pub paused_reason: Option<String>,
    pub paused_kind: Option<String>,
    pub sent_today: i64,
    pub daily_max: i64,
    pub quota_remaining: i64,
    pub next_send_at: Option<String>,
    pub scheduled: i64,
    pub drafts: i64,
    pub sent_total: i64,
    pub smtp_ok: bool,
    pub last_sync_ok: Option<String>,
}

#[derive(Serialize)]
pub struct VisaHint {
    pub country_code: String,
    pub country_name: String,
}

#[derive(Serialize)]
pub struct ResidencyView {
    pub campaign: Campaign,
    pub residency: Residency,
    pub status: StatusLine,
    pub visas: Vec<VisaHint>,
    pub cost: AiCost,
    pub ai_configured: bool,
    pub insrt_configured: bool,
    pub ai_model: String,
}

fn count(conn: &Connection, sql: &str, id: i64) -> i64 {
    conn.query_row(sql, params![id], |r| r.get(0)).unwrap_or(0)
}

fn status_line(conn: &Connection, pool: &crate::db::DbPool, r: &Residency) -> StatusLine {
    let id = r.campaign_id;
    let now = Utc::now();
    let smtp_ok = crate::commands::email::load_smtp_from(pool)
        .map(|c| !c.host.trim().is_empty() && !c.from_email.trim().is_empty() && !c.password.trim().is_empty())
        .unwrap_or(false);
    StatusLine {
        autopilot: r.autopilot,
        confirmed: r.autopilot_confirmed_at.is_some(),
        paused_reason: r.paused_reason.clone(),
        paused_kind: r.paused_kind.clone(),
        sent_today: engine::campaign_sent_today(conn, id),
        daily_max: r.config.daily_max,
        quota_remaining: crate::commands::outbox::quota(conn).remaining,
        next_send_at: engine::next_send_estimate(conn, id, &r.config, now).map(fmt),
        scheduled: count(conn, "SELECT COUNT(*) FROM campaign_messages WHERE campaign_id = ?1 AND status = 'scheduled'", id),
        drafts: count(conn, "SELECT COUNT(*) FROM campaign_messages WHERE campaign_id = ?1 AND status = 'draft'", id),
        sent_total: count(conn, "SELECT COUNT(*) FROM campaign_messages WHERE campaign_id = ?1 AND status = 'sent'", id),
        smtp_ok,
        last_sync_ok: engine::last_sync_ok(conn).map(fmt),
    }
}

#[tauri::command(async)]
pub fn residency_get(state: State<'_, AppState>, campaign_id: i64) -> Result<ResidencyView, String> {
    let conn = conn(&state)?;
    let r = require(&conn, campaign_id)?;
    let campaign = conn
        .query_row(
            "SELECT c.*, (SELECT COUNT(*) FROM emails e WHERE e.campaign_id = c.id AND e.status = 'sent') AS sent_count,
                    (SELECT COUNT(*) FROM emails e WHERE e.campaign_id = c.id AND e.status = 'sent' AND e.opened_at IS NOT NULL) AS opened_count,
                    (SELECT COUNT(*) FROM campaign_contacts cc WHERE cc.campaign_id = c.id) AS audience_count,
                    (SELECT COUNT(*) FROM campaign_contacts cc WHERE cc.campaign_id = c.id AND cc.state = 'planned') AS planned_count
             FROM campaigns c WHERE c.id = ?1",
            params![campaign_id],
            Campaign::from_row,
        )
        .map_err(|e| e.to_string())?;
    // Visa: every country of the zone (presets and codes) the artist needs one for.
    let artist_country: Option<String> = campaign.artist_id.and_then(|a| {
        conn.query_row("SELECT country FROM artists WHERE id = ?1", params![a], |r| r.get(0)).ok().flatten()
    });
    let (_, mut countries) = r.config.zone.resolve();
    if r.config.zone.presets.iter().any(|p| p == "london") {
        countries.push("GB".into());
    }
    countries.dedup();
    let visas = countries
        .iter()
        .filter_map(|c| crate::commands::visa::visa_needed(&conn, artist_country.as_deref(), c))
        .map(|(country_code, country_name)| VisaHint { country_code, country_name })
        .collect();
    let status = status_line(&conn, &state.pool, &r);
    Ok(ResidencyView {
        campaign,
        status,
        visas,
        cost: claude::month_cost(&conn, Some(campaign_id)),
        ai_configured: crate::secrets::read_setting(&conn, "anthropic_api_key").map_or(false, |k| !k.trim().is_empty()),
        insrt_configured: crate::secrets::read_setting(&conn, "insrt_api_token").map_or(false, |k| !k.trim().is_empty()),
        ai_model: claude::drafting_model(&conn),
        residency: r,
    })
}

#[tauri::command(async)]
pub fn residency_save(state: State<'_, AppState>, campaign_id: i64, config: Config) -> Result<(), String> {
    let conn = conn(&state)?;
    require(&conn, campaign_id)?;
    let mut cfg = config;
    cfg.daily_max = cfg.daily_max.clamp(1, 50);
    cfg.capacity_min = cfg.capacity_min.max(0);
    cfg.capacity_max = cfg.capacity_max.max(cfg.capacity_min);
    cfg.steps.sort_by_key(|s| s.offset_days);
    for a in cfg.assets.iter_mut() {
        if !super::ASSET_TYPES.contains(&a.kind.as_str()) {
            a.kind = "link".into();
        }
    }
    save_config(&conn, campaign_id, &cfg)
}

/// Autopilot switch (the big Pause is `on = false`). Switching it on also
/// lifts a pause the user has now dealt with, and restarts the bounce window.
#[tauri::command(async)]
pub fn residency_set_autopilot(state: State<'_, AppState>, campaign_id: i64, on: bool) -> Result<(), String> {
    let conn = conn(&state)?;
    require(&conn, campaign_id)?;
    if on {
        conn.execute(
            "UPDATE residency_campaigns SET autopilot = 1, paused_reason = NULL, paused_kind = NULL,
                    autopilot_confirmed_at = COALESCE(autopilot_confirmed_at, datetime('now')), updated_at = datetime('now')
             WHERE campaign_id = ?1",
            params![campaign_id],
        )
        .map_err(|e| e.to_string())?;
        engine::put(&conn, "residency_bounce_reset_at", &fmt(Utc::now()));
        super::log(&conn, Some(campaign_id), "info", "autopilot_on", "Pilote auto activé");
        engine::wake();
    } else {
        conn.execute(
            "UPDATE residency_campaigns SET autopilot = 0, updated_at = datetime('now') WHERE campaign_id = ?1",
            params![campaign_id],
        )
        .map_err(|e| e.to_string())?;
        super::log(&conn, Some(campaign_id), "pause", "autopilot_off", "Pilote auto mis en pause par l'utilisateur");
    }
    Ok(())
}

// ---------------- Target ----------------

#[tauri::command(async)]
pub fn residency_build_target(state: State<'_, AppState>, campaign_id: i64, options: Option<BuildOptions>) -> Result<BuildResult, String> {
    let conn = conn(&state)?;
    let r = require(&conn, campaign_id)?;
    targeting::build(&conn, campaign_id, &r.config, &options.unwrap_or_default())
}

#[tauri::command(async)]
pub fn residency_add_targets(state: State<'_, AppState>, campaign_id: i64, picks: Vec<Pick>) -> Result<usize, String> {
    let conn = conn(&state)?;
    require(&conn, campaign_id)?;
    let n = targeting::add_targets(&conn, campaign_id, &picks)?;
    super::log(&conn, Some(campaign_id), "info", "targets_added", &format!("{} cible(s) ajoutée(s)", n));
    Ok(n)
}

#[tauri::command(async)]
pub fn residency_remove_target(state: State<'_, AppState>, campaign_id: i64, contact_id: i64) -> Result<(), String> {
    let conn = conn(&state)?;
    conn.execute(
        "DELETE FROM campaign_messages WHERE campaign_id = ?1 AND contact_id = ?2 AND status <> 'sent'",
        params![campaign_id, contact_id],
    )
    .map_err(|e| e.to_string())?;
    conn.execute(
        "DELETE FROM campaign_contacts WHERE campaign_id = ?1 AND contact_id = ?2
           AND NOT EXISTS (SELECT 1 FROM campaign_messages WHERE campaign_id = ?1 AND contact_id = ?2)",
        params![campaign_id, contact_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// One card of the "Suivi" board (and a row of the current audience).
#[derive(Serialize)]
pub struct BoardRow {
    pub contact_id: i64,
    pub name: String,
    pub email: Option<String>,
    pub city: Option<String>,
    pub status: String,
    /// to_send | sent | followed_up | engaged | interested | later | declined | wrong
    pub column: String,
    pub steps_sent: i64,
    pub last_sent_at: Option<String>,
    pub next_at: Option<String>,
    pub next_step: Option<i64>,
    pub opened: bool,
    pub views: i64,
    pub listen_seconds: f64,
    pub feedback: i64,
    pub replied: bool,
    pub intent: Option<String>,
    pub sentiment: Option<String>,
    pub summary: Option<String>,
    pub suppressed: bool,
    pub lang: Option<String>,
}

pub fn board(conn: &Connection, campaign_id: i64) -> Result<Vec<BoardRow>, String> {
    let mut st = conn
        .prepare(
            "SELECT k.id, k.name, k.email, k.area, k.status, cc.state,
                (SELECT COUNT(*) FROM campaign_messages m WHERE m.campaign_id = cc.campaign_id AND m.contact_id = k.id AND m.status = 'sent'),
                (SELECT MAX(e.sent_at) FROM campaign_messages m JOIN emails e ON e.id = m.sent_email_id
                   WHERE m.campaign_id = cc.campaign_id AND m.contact_id = k.id),
                (SELECT COALESCE(m.scheduled_for, 'asap') FROM campaign_messages m WHERE m.campaign_id = cc.campaign_id AND m.contact_id = k.id
                   AND m.status = 'scheduled' ORDER BY m.step LIMIT 1),
                (SELECT m.step FROM campaign_messages m WHERE m.campaign_id = cc.campaign_id AND m.contact_id = k.id
                   AND m.status IN ('scheduled','draft') ORDER BY m.step LIMIT 1),
                EXISTS (SELECT 1 FROM emails e WHERE e.campaign_id = cc.campaign_id AND e.contact_id = k.id AND e.opened_at IS NOT NULL),
                (SELECT COUNT(*) FROM press_activity p WHERE p.contact_id = k.id AND p.type = 'view'),
                COALESCE((SELECT SUM(p.seconds) FROM press_activity p WHERE p.contact_id = k.id AND p.type = 'listen'), 0),
                (SELECT COUNT(*) FROM press_activity p WHERE p.contact_id = k.id AND p.type = 'feedback'),
                EXISTS (SELECT 1 FROM email_replies r JOIN campaign_messages m ON m.sent_email_id = r.email_id
                        WHERE m.campaign_id = cc.campaign_id AND m.contact_id = k.id AND r.kind = 'reply'),
                (SELECT i.intent FROM reply_insights i WHERE i.campaign_id = cc.campaign_id AND i.contact_id = k.id
                   AND i.intent <> 'out_of_office' ORDER BY i.id DESC LIMIT 1),
                (SELECT i.sentiment FROM reply_insights i WHERE i.campaign_id = cc.campaign_id AND i.contact_id = k.id
                   AND i.intent <> 'out_of_office' ORDER BY i.id DESC LIMIT 1),
                (SELECT i.summary FROM reply_insights i WHERE i.campaign_id = cc.campaign_id AND i.contact_id = k.id
                   ORDER BY i.id DESC LIMIT 1),
                EXISTS (SELECT 1 FROM suppressions s WHERE s.address = lower(trim(k.email))),
                (SELECT m.lang FROM campaign_messages m WHERE m.campaign_id = cc.campaign_id AND m.contact_id = k.id AND m.step = 1)
             FROM campaign_contacts cc JOIN contacts k ON k.id = cc.contact_id
             WHERE cc.campaign_id = ?1
             ORDER BY k.name COLLATE NOCASE",
        )
        .map_err(|e| e.to_string())?;
    let rows = st
        .query_map(params![campaign_id], |r| {
            let state: String = r.get(5)?;
            let status: String = r.get(4)?;
            let steps_sent: i64 = r.get(6)?;
            let opened = r.get::<_, i64>(10)? != 0;
            let views: i64 = r.get(11)?;
            let listen: f64 = r.get(12)?;
            let feedback: i64 = r.get(13)?;
            let replied = r.get::<_, i64>(14)? != 0;
            let intent: Option<String> = r.get(15)?;
            let suppressed = r.get::<_, i64>(18)? != 0;
            let column = match intent.as_deref() {
                Some("interested") | Some("question") => "interested",
                Some("later") => "later",
                Some("not_interested") | Some("unsubscribe") => "declined",
                Some("wrong_person") => "wrong",
                _ if status == "declined" => "declined",
                _ if state == "bounced" => "wrong",
                _ if replied || status == "in_discussion" => "interested",
                _ if listen > 0.0 || views > 0 || feedback > 0 || opened => "engaged",
                _ if steps_sent >= 2 => "followed_up",
                _ if steps_sent == 1 => "sent",
                _ => "to_send",
            }
            .to_string();
            Ok(BoardRow {
                contact_id: r.get(0)?,
                name: r.get(1)?,
                email: r.get(2)?,
                city: r.get(3)?,
                status,
                column,
                steps_sent,
                last_sent_at: r.get(7)?,
                next_at: r.get(8)?,
                next_step: r.get(9)?,
                opened,
                views,
                listen_seconds: listen,
                feedback,
                replied,
                intent,
                sentiment: r.get(16)?,
                summary: r.get(17)?,
                suppressed,
                lang: r.get(19)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub fn residency_board(state: State<'_, AppState>, campaign_id: i64) -> Result<Vec<BoardRow>, String> {
    let conn = conn(&state)?;
    board(&conn, campaign_id)
}

// ---------------- Drafts ----------------

/// Step-1 drafts (Claude when configured, templates otherwise). Network calls:
/// runs on the blocking pool.
#[tauri::command]
pub async fn residency_generate(
    state: State<'_, AppState>,
    campaign_id: i64,
    contact_ids: Option<Vec<i64>>,
    regenerate: Option<bool>,
) -> Result<GenResult, String> {
    let pool = state.pool.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let conn = pool.get().map_err(|e| e.to_string())?;
        let r = require(&conn, campaign_id)?;
        let llm = claude::client(&conn);
        let ins = insrt::client(&conn);
        let res = content::generate(
            &conn,
            campaign_id,
            &r.config,
            contact_ids.as_deref(),
            regenerate.unwrap_or(false),
            llm.as_deref(),
            ins.as_deref(),
        )?;
        super::log(
            &conn,
            Some(campaign_id),
            "info",
            "drafts",
            &format!("{} brouillon(s), dont {} rédigé(s) par l'IA", res.created, res.with_ai),
        );
        Ok(res)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Serialize)]
pub struct MessageRow {
    pub id: i64,
    pub contact_id: i64,
    pub name: String,
    pub email: Option<String>,
    pub step: i64,
    pub subject: String,
    pub body: String,
    pub lang: String,
    pub status: String,
    pub scheduled_for: Option<String>,
    pub sent_at: Option<String>,
    pub press_url: Option<String>,
    pub note: Option<String>,
    pub generated_by: Option<String>,
    pub unresolved: Vec<String>,
}

#[tauri::command(async)]
pub fn residency_messages(state: State<'_, AppState>, campaign_id: i64) -> Result<Vec<MessageRow>, String> {
    let conn = conn(&state)?;
    let mut st = conn
        .prepare(
            "SELECT m.id, m.contact_id, k.name, k.email, m.step, m.subject, m.body, m.lang, m.status, m.scheduled_for,
                    e.sent_at, m.press_url, m.note, m.generated_by
             FROM campaign_messages m JOIN contacts k ON k.id = m.contact_id LEFT JOIN emails e ON e.id = m.sent_email_id
             WHERE m.campaign_id = ?1
             ORDER BY CASE m.status WHEN 'draft' THEN 0 WHEN 'scheduled' THEN 1 WHEN 'failed' THEN 2 WHEN 'sent' THEN 3 ELSE 4 END,
                      COALESCE(m.scheduled_for, e.sent_at, m.updated_at), m.step, k.name COLLATE NOCASE",
        )
        .map_err(|e| e.to_string())?;
    let rows = st
        .query_map(params![campaign_id], |r| {
            let subject: String = r.get(5)?;
            let body: String = r.get(6)?;
            Ok(MessageRow {
                id: r.get(0)?,
                contact_id: r.get(1)?,
                name: r.get(2)?,
                email: r.get(3)?,
                step: r.get(4)?,
                unresolved: content::unresolved(&format!("{}\n{}", subject, body)),
                subject,
                body,
                lang: r.get(7)?,
                status: r.get(8)?,
                scheduled_for: r.get(9)?,
                sent_at: r.get(10)?,
                press_url: r.get(11)?,
                note: r.get(12)?,
                generated_by: r.get(13)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

/// Edit a draft or a scheduled message (never a sent one). A message whose
/// variables no longer resolve goes back to draft.
#[tauri::command(async)]
pub fn residency_save_message(state: State<'_, AppState>, id: i64, subject: String, body: String) -> Result<(), String> {
    let conn = conn(&state)?;
    let missing = content::unresolved(&format!("{}\n{}", subject, body));
    let n = conn
        .execute(
            "UPDATE campaign_messages SET subject = ?2, body = ?3,
                    status = CASE WHEN ?4 AND status = 'scheduled' THEN 'draft' ELSE status END,
                    note = CASE WHEN ?4 THEN 'Variables à compléter' ELSE NULL END,
                    generated_by = CASE WHEN generated_by LIKE 'claude%' THEN generated_by || ' (modifié)' ELSE generated_by END,
                    updated_at = datetime('now')
             WHERE id = ?1 AND status IN ('draft','scheduled','failed','skipped')",
            params![id, subject, body, !missing.is_empty()],
        )
        .map_err(|e| e.to_string())?;
    if n == 0 {
        return Err("Message déjà envoyé.".into());
    }
    Ok(())
}

/// scheduled (validate), draft (back to editing) or skipped.
#[tauri::command(async)]
pub fn residency_set_message_status(state: State<'_, AppState>, id: i64, status: String) -> Result<(), String> {
    let conn = conn(&state)?;
    let (subject, body, current): (String, String, String) = conn
        .query_row("SELECT subject, body, status FROM campaign_messages WHERE id = ?1", params![id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .map_err(|e| e.to_string())?;
    if current == "sent" {
        return Err("Message déjà envoyé.".into());
    }
    match status.as_str() {
        "scheduled" => {
            let missing = content::unresolved(&format!("{}\n{}", subject, body));
            if !missing.is_empty() {
                return Err(format!("Variables à compléter : {}", missing.join(", ")));
            }
        }
        "draft" | "skipped" => {}
        _ => return Err("Statut inconnu.".into()),
    }
    conn.execute(
        "UPDATE campaign_messages SET status = ?2, note = NULL, updated_at = datetime('now') WHERE id = ?1",
        params![id, status],
    )
    .map_err(|e| e.to_string())?;
    engine::wake();
    Ok(())
}

#[derive(Serialize)]
pub struct ValidateResult {
    pub scheduled: usize,
    pub unresolved: usize,
}

/// "Tout valider": every draft whose variables resolve becomes scheduled.
pub fn validate_all(conn: &Connection, campaign_id: i64) -> Result<ValidateResult, String> {
    let drafts: Vec<(i64, String, String)> = conn
        .prepare("SELECT id, subject, body FROM campaign_messages WHERE campaign_id = ?1 AND status = 'draft'")
        .map_err(|e| e.to_string())?
        .query_map(params![campaign_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .map_err(|e| e.to_string())?
        .flatten()
        .collect();
    let mut out = ValidateResult { scheduled: 0, unresolved: 0 };
    for (id, s, b) in drafts {
        if content::unresolved(&format!("{}\n{}", s, b)).is_empty() {
            conn.execute(
                "UPDATE campaign_messages SET status = 'scheduled', note = NULL, updated_at = datetime('now') WHERE id = ?1",
                params![id],
            )
            .map_err(|e| e.to_string())?;
            out.scheduled += 1;
        } else {
            out.unresolved += 1;
        }
    }
    Ok(out)
}

#[tauri::command(async)]
pub fn residency_validate_all(state: State<'_, AppState>, campaign_id: i64) -> Result<ValidateResult, String> {
    let conn = conn(&state)?;
    let r = validate_all(&conn, campaign_id)?;
    super::log(&conn, Some(campaign_id), "info", "validated", &format!("{} message(s) validé(s)", r.scheduled));
    engine::wake();
    Ok(r)
}

// ---------------- Reports, log, cost ----------------

#[derive(Serialize)]
pub struct ReportRow {
    pub day: String,
    pub data: serde_json::Value,
    pub synthesis: Option<String>,
    pub created_at: String,
}

#[tauri::command(async)]
pub fn residency_reports(state: State<'_, AppState>, campaign_id: i64) -> Result<Vec<ReportRow>, String> {
    let conn = conn(&state)?;
    let mut st = conn
        .prepare("SELECT day, data, synthesis, created_at FROM campaign_reports WHERE campaign_id = ?1 ORDER BY day DESC LIMIT 120")
        .map_err(|e| e.to_string())?;
    let rows = st
        .query_map(params![campaign_id], |r| {
            let raw: String = r.get(1)?;
            Ok(ReportRow {
                day: r.get(0)?,
                data: serde_json::from_str(&raw).unwrap_or(serde_json::Value::Null),
                synthesis: r.get(2)?,
                created_at: r.get(3)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

/// Rebuild today's report now (button on the Rapports tab).
#[tauri::command]
pub async fn residency_report_now(state: State<'_, AppState>, campaign_id: i64) -> Result<bool, String> {
    let pool = state.pool.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let conn = pool.get().map_err(|e| e.to_string())?;
        let today = chrono::Local::now().date_naive();
        conn.execute(
            "DELETE FROM campaign_reports WHERE campaign_id = ?1 AND day = ?2",
            params![campaign_id, today.to_string()],
        )
        .map_err(|e| e.to_string())?;
        let llm = claude::client(&conn);
        report::build_and_store(&conn, campaign_id, today, llm.as_deref())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Serialize)]
pub struct LogRow {
    pub at: String,
    pub level: String,
    pub code: String,
    pub message: String,
}

#[tauri::command(async)]
pub fn residency_log(state: State<'_, AppState>, campaign_id: Option<i64>, limit: Option<i64>) -> Result<Vec<LogRow>, String> {
    let conn = conn(&state)?;
    let mut st = conn
        .prepare(
            "SELECT at, level, code, message FROM residency_log WHERE (?1 IS NULL OR campaign_id = ?1 OR campaign_id IS NULL)
             ORDER BY id DESC LIMIT ?2",
        )
        .map_err(|e| e.to_string())?;
    let rows = st
        .query_map(params![campaign_id, limit.unwrap_or(100).clamp(1, 1000)], |r| {
            Ok(LogRow { at: r.get(0)?, level: r.get(1)?, code: r.get(2)?, message: r.get(3)? })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub fn ai_cost_month(state: State<'_, AppState>, campaign_id: Option<i64>) -> Result<AiCost, String> {
    let conn = conn(&state)?;
    Ok(claude::month_cost(&conn, campaign_id))
}

/// Press kits of the artist on insrt.studio (also the "Tester" of Settings).
#[tauri::command]
pub async fn residency_press_kits(state: State<'_, AppState>) -> Result<Vec<PressKit>, String> {
    let pool = state.pool.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let conn = pool.get().map_err(|e| e.to_string())?;
        let client = insrt::client(&conn).ok_or_else(|| "Jeton insrt non configuré (Réglages, Intégrations).".to_string())?;
        let slug = crate::commands::email::sole_artist(&conn).map(|a| insrt::slug(&a)).unwrap_or_else(|| "piers".into());
        client.press_kits(&slug)
    })
    .await
    .map_err(|e| e.to_string())?
}

// ---------------- Agenda › Actions ----------------

#[derive(Serialize)]
pub struct ActionItem {
    pub key: String,
    /// plan | reply | unread | later | wrong | hot
    pub kind: String,
    pub day: String,
    pub campaign_id: Option<i64>,
    pub campaign: Option<String>,
    pub contact_id: Option<i64>,
    pub name: String,
    pub insight_id: Option<i64>,
    pub intent: Option<String>,
    pub sentiment: Option<String>,
    pub summary: Option<String>,
    pub suggested_subject: Option<String>,
    pub suggested_body: Option<String>,
    pub mentioned_email: Option<String>,
    pub track_title: Option<String>,
    pub seconds: Option<f64>,
    pub count: Option<i64>,
    pub next_at: Option<String>,
}

impl ActionItem {
    fn new(key: String, kind: &str, day: String, name: String) -> Self {
        ActionItem {
            key,
            kind: kind.into(),
            day,
            campaign_id: None,
            campaign: None,
            contact_id: None,
            name,
            insight_id: None,
            intent: None,
            sentiment: None,
            summary: None,
            suggested_subject: None,
            suggested_body: None,
            mentioned_email: None,
            track_title: None,
            seconds: None,
            count: None,
            next_at: None,
        }
    }
}

pub fn actions(conn: &Connection) -> Result<Vec<ActionItem>, String> {
    let today = chrono::Local::now().date_naive().to_string();
    let mut out = Vec::new();
    let now = Utc::now();
    // Today's autopilot plan, per campaign.
    for id in super::active_ids(conn) {
        let Some(r) = load(conn, id) else { continue };
        let name: String = conn.query_row("SELECT name FROM campaigns WHERE id = ?1", params![id], |x| x.get(0)).unwrap_or_default();
        let due_today: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM campaign_messages WHERE campaign_id = ?1 AND status = 'scheduled'
                   AND (scheduled_for IS NULL OR scheduled_for < datetime('now', 'localtime', 'start of day', '+1 day', 'utc'))",
                params![id],
                |x| x.get(0),
            )
            .unwrap_or(0);
        if due_today == 0 && r.paused_reason.is_none() {
            continue;
        }
        let planned = due_today.min((r.config.daily_max - engine::campaign_sent_today(conn, id)).max(0));
        let mut a = ActionItem::new(format!("rp{}", id), "plan", today.clone(), name.clone());
        a.campaign_id = Some(id);
        a.campaign = Some(name);
        a.count = Some(if r.autopilot && r.paused_reason.is_none() { planned } else { 0 });
        a.summary = r.paused_reason.clone().or_else(|| (!r.autopilot).then(|| "off".to_string()));
        a.next_at = engine::next_send_estimate(conn, id, &r.config, now).map(fmt);
        out.push(a);
    }
    // Classified replies still to handle.
    let mut st = conn
        .prepare(
            "SELECT i.id, i.contact_id, i.campaign_id, c.name, COALESCE(k.name, r.from_addr), i.intent, i.sentiment, i.summary,
                    i.suggested_reply, i.follow_up_on, i.mentioned_email, date(COALESCE(r.received_at, i.created_at)), r.subject
             FROM reply_insights i JOIN email_replies r ON r.id = i.email_reply_id
             LEFT JOIN contacts k ON k.id = i.contact_id LEFT JOIN campaigns c ON c.id = i.campaign_id
             WHERE i.handled = 0 AND i.intent IN ('interested','question','later','wrong_person')
             ORDER BY i.id DESC LIMIT 100",
        )
        .map_err(|e| e.to_string())?;
    type R = (i64, Option<i64>, Option<i64>, Option<String>, String, String, String, Option<String>, Option<String>, Option<String>, Option<String>, String, Option<String>);
    let rows: Vec<R> = st
        .query_map([], |r| {
            Ok((
                r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?, r.get(9)?,
                r.get(10)?, r.get(11)?, r.get(12)?,
            ))
        })
        .map_err(|e| e.to_string())?
        .flatten()
        .collect();
    for (iid, cid, camp, camp_name, name, intent, sentiment, summary, sugg, fu, mentioned, day, subject) in rows {
        let kind = match intent.as_str() {
            "later" => "later",
            "wrong_person" => "wrong",
            _ => "reply",
        };
        let day = if kind == "later" { fu.clone().unwrap_or(day) } else { day };
        let mut a = ActionItem::new(format!("ri{}", iid), kind, day, name);
        a.campaign_id = camp;
        a.campaign = camp_name;
        a.contact_id = cid;
        a.insight_id = Some(iid);
        a.intent = Some(intent);
        a.sentiment = Some(sentiment);
        a.summary = summary;
        a.suggested_body = sugg;
        a.suggested_subject = subject.map(|s| if s.to_lowercase().starts_with("re:") { s } else { format!("Re: {}", s) });
        a.mentioned_email = mentioned;
        out.push(a);
    }
    // Replies nobody could classify (no API key): still to read.
    let mut st = conn
        .prepare(
            "SELECT r.id, k.id, m.campaign_id, k.name, date(COALESCE(r.received_at, r.created_at)), r.subject
             FROM email_replies r JOIN campaign_messages m ON m.sent_email_id = r.email_id
             JOIN contacts k ON k.id = m.contact_id
             WHERE r.kind = 'reply' AND NOT EXISTS (SELECT 1 FROM reply_insights i WHERE i.email_reply_id = r.id)
               AND r.received_at >= datetime('now', '-14 days')
             GROUP BY r.id",
        )
        .map_err(|e| e.to_string())?;
    let rows: Vec<(i64, i64, i64, String, String, Option<String>)> = st
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))
        .map_err(|e| e.to_string())?
        .flatten()
        .collect();
    for (rid, cid, camp, name, day, subject) in rows {
        let mut a = ActionItem::new(format!("ru{}", rid), "unread", day, name);
        a.contact_id = Some(cid);
        a.campaign_id = Some(camp);
        a.suggested_subject = subject.map(|s| format!("Re: {}", s.trim_start_matches("Re: ")));
        out.push(a);
    }
    // Hot signals.
    for h in report::hot_signals(conn, None) {
        let HotSignal { contact_id, campaign_id, name, kind, track_title, seconds, comment, at } = h;
        let mut a = ActionItem::new(format!("hot{}", contact_id), "hot", at.get(..10).unwrap_or(&today).to_string(), name);
        a.contact_id = Some(contact_id);
        a.campaign_id = campaign_id;
        a.intent = Some(kind);
        a.track_title = track_title;
        a.seconds = Some(seconds);
        a.summary = comment;
        out.push(a);
    }
    let _ = today;
    Ok(out)
}

#[tauri::command(async)]
pub fn residency_actions(state: State<'_, AppState>) -> Result<Vec<ActionItem>, String> {
    let conn = conn(&state)?;
    actions(&conn)
}

#[tauri::command(async)]
pub fn residency_mark_handled(state: State<'_, AppState>, insight_id: i64) -> Result<(), String> {
    let conn = conn(&state)?;
    conn.execute("UPDATE reply_insights SET handled = 1 WHERE id = ?1", params![insight_id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::residency::engine::tests::setup;

    #[test]
    fn validate_board_and_actions() {
        let c = setup();
        c.execute("UPDATE campaign_messages SET status = 'draft'", []).unwrap();
        c.execute("UPDATE campaign_messages SET body = 'Bonjour {{first_name}}' WHERE contact_id = 11", []).unwrap();
        let r = validate_all(&c, 1).unwrap();
        assert_eq!((r.scheduled, r.unresolved), (1, 1));
        let b = board(&c, 1).unwrap();
        assert_eq!(b.len(), 2);
        assert!(b.iter().all(|x| x.column == "to_send"));
        // A classified interested reply shows on the board and in Actions.
        c.execute_batch(
            "INSERT INTO email_replies (id, contact_id, from_addr, subject, message_id, kind, received_at)
               VALUES (1, 10, 'booking@cluba.fr', 'Résidence', '<a>', 'reply', datetime('now'));
             INSERT INTO reply_insights (email_reply_id, contact_id, campaign_id, sentiment, intent, summary, suggested_reply)
               VALUES (1, 10, 1, 'positive', 'question', 'Demande le cachet', 'Bonjour, ...');",
        )
        .unwrap();
        let b = board(&c, 1).unwrap();
        assert_eq!(b.iter().find(|x| x.contact_id == 10).unwrap().column, "interested");
        let a = actions(&c).unwrap();
        let reply = a.iter().find(|x| x.kind == "reply").unwrap();
        assert_eq!(reply.suggested_subject.as_deref(), Some("Re: Résidence"));
        assert_eq!(reply.suggested_body.as_deref(), Some("Bonjour, ..."));
        assert!(a.iter().any(|x| x.kind == "plan"));
    }
}
