//! Labels: sending demos to record labels, one demo at a time.
//!
//! One list of labels (`labels`), one campaign per demo. A demo campaign is a
//! `campaigns` row of kind 'label' with a `residency_campaigns` config, so it
//! rides the same engine as the club campaigns: the message queue
//! (`campaign_messages`), the send window, the quota and deliverability gates,
//! the stop-on-reply rule, the personal insrt link and its listens.
//!
//! What is specific to labels lives here:
//! - the demo goes out in waves: labels are ranked, the first `wave_size` get
//!   a draft, the next wave opens once every label of the current one refused
//!   or stayed silent (`close_days` after the first email);
//! - the first email of each label waits for the user's validation, the
//!   follow-up (J+10 by default) is scheduled by the engine once it left;
//! - a label that only takes demos through a form gets no email: the user
//!   submits by hand and ticks it, the same closing delay applies.
//!
//! A label gets its `contacts` row (category 'label') only when it enters a
//! wave: that row is what the send path, the reply sync and the suppression
//! list know. The user never sees a "promote" step.

use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::commands::residency::insrt::Insrt;
use crate::commands::residency::targeting::target_info;
use crate::commands::residency::zone::{Window, Zone};
use crate::commands::residency::{self, content, engine, load, save_config, Asset, Config, Step};
use crate::db::normalise;
use crate::AppState;

pub const DEFAULT_WAVE_SIZE: i64 = 4;
pub const DEFAULT_CLOSE_DAYS: i64 = 21;
pub const FOLLOWUP_DAYS: i64 = 10;

/// Outcomes that free the demo for the next wave.
const CLOSED: &[&str] = &["declined", "no_answer", "bounced", "withdrawn"];
const OUTCOMES: &[&str] = &["replied", "declined", "no_answer", "bounced", "withdrawn", "signed"];

pub fn migrate(conn: &Connection) {
    for stmt in [
        "CREATE TABLE IF NOT EXISTS labels (
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            name         TEXT NOT NULL,
            name_norm    TEXT NOT NULL UNIQUE,
            country      TEXT,
            genres       TEXT,
            website      TEXT,
            email        TEXT,
            contact_name TEXT,
            demo_policy  TEXT NOT NULL DEFAULT 'email', -- email | form | closed
            demo_url     TEXT,
            tier         INTEGER NOT NULL DEFAULT 2,    -- 1 dream, 2 target, 3 within reach
            notes        TEXT,
            source       TEXT,
            radar_id     INTEGER,
            contact_id   INTEGER REFERENCES contacts(id) ON DELETE SET NULL,
            archived     INTEGER NOT NULL DEFAULT 0,
            created_at   TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at   TEXT NOT NULL DEFAULT (datetime('now'))
        )",
        "CREATE INDEX IF NOT EXISTS idx_labels_contact ON labels(contact_id)",
        "CREATE TABLE IF NOT EXISTS label_demos (
            campaign_id  INTEGER PRIMARY KEY REFERENCES campaigns(id) ON DELETE CASCADE,
            wave_size    INTEGER NOT NULL DEFAULT 4,
            close_days   INTEGER NOT NULL DEFAULT 21,
            current_wave INTEGER NOT NULL DEFAULT 0
        )",
        "CREATE TABLE IF NOT EXISTS label_targets (
            id             INTEGER PRIMARY KEY AUTOINCREMENT,
            campaign_id    INTEGER NOT NULL REFERENCES campaigns(id) ON DELETE CASCADE,
            label_id       INTEGER NOT NULL REFERENCES labels(id) ON DELETE CASCADE,
            rank           INTEGER NOT NULL DEFAULT 0,
            wave           INTEGER,                 -- NULL = waiting for its wave
            outcome        TEXT,                    -- see OUTCOMES
            outcome_by     TEXT,                    -- auto | user
            outcome_at     TEXT,
            manual_sent_at TEXT,                    -- demo submitted through the label's form
            UNIQUE (campaign_id, label_id)
        )",
        "CREATE INDEX IF NOT EXISTS idx_label_targets_campaign ON label_targets(campaign_id, wave, rank)",
    ] {
        let _ = conn.execute(stmt, []);
    }
    // First launch with the module: the labels Radar already found come in.
    if engine::get(conn, "labels_seeded_v1").is_none() {
        let _ = import_radar(conn);
        engine::put(conn, "labels_seeded_v1", "1");
    }
}

/// ISO country code from what the label sources store (English names, a few
/// cities). Unknown values come back upper-cased: English, Paris time.
pub fn iso_of(raw: &str) -> String {
    let code = match normalise(raw).as_str() {
        "fr" | "france" | "paris" | "lyon" | "marseille" | "bordeaux" => "FR",
        "gb" | "uk" | "unitedkingdom" | "greatbritain" | "england" | "scotland" | "wales" | "london" | "manchester"
        | "bristol" | "leeds" | "glasgow" | "brighton" => "GB",
        "ie" | "ireland" | "dublin" => "IE",
        "pt" | "portugal" | "lisbon" | "lisboa" => "PT",
        "be" | "belgium" | "belgique" => "BE",
        "ch" | "switzerland" | "suisse" => "CH",
        "lu" | "luxembourg" => "LU",
        "mc" | "monaco" => "MC",
        "de" | "germany" | "berlin" => "DE",
        "nl" | "netherlands" | "amsterdam" => "NL",
        "es" | "spain" | "barcelona" | "madrid" => "ES",
        "it" | "italy" => "IT",
        "us" | "usa" | "unitedstates" => "US",
        _ => "",
    };
    if code.is_empty() {
        raw.trim().to_uppercase()
    } else {
        code.to_string()
    }
}

fn clean(s: Option<String>) -> Option<String> {
    s.map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

// ---------------------------------------------------------------------------
// The list of labels
// ---------------------------------------------------------------------------

/// Bring the labels Radar holds into the list: new ones are added, a label
/// already there only gets the address it was missing. Returns how many were
/// added.
pub fn import_radar(conn: &Connection) -> Result<usize, String> {
    let sql = format!(
        "SELECT c.id, c.nom, c.pays, c.genre, COALESCE(NULLIF(trim(c.site_web), ''), c.url), c.description, c.source,
                c.crm_contact_id,
                (SELECT x.valeur FROM radar_contacts x WHERE x.curator_id = c.id AND x.type = 'email' AND {}
                 ORDER BY x.score DESC, x.id LIMIT 1)
         FROM radar_curators c WHERE c.kind = 'label' AND c.statut <> 'rejete' ORDER BY c.id",
        crate::commands::radar::pitchable_sql("x.valeur")
    );
    type Row = (i64, String, Option<String>, Option<String>, Option<String>, Option<String>, String, Option<i64>, Option<String>);
    let rows: Vec<Row> = conn
        .prepare(&sql)
        .map_err(|e| e.to_string())?
        .query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?))
        })
        .map_err(|e| e.to_string())?
        .flatten()
        .collect();
    let mut added = 0;
    for (rid, name, country, genres, website, description, source, contact_id, email) in rows {
        let norm = normalise(&name);
        if norm.is_empty() {
            continue;
        }
        let source = if source == "indie_bible" { "Indie Bible" } else { "MusicBrainz" };
        added += conn
            .execute(
                "INSERT OR IGNORE INTO labels (name, name_norm, country, genres, website, email, notes, source, radar_id, contact_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![name.trim(), norm, clean(country), clean(genres), clean(website), clean(email.clone()), clean(description), source, rid, contact_id],
            )
            .map_err(|e| e.to_string())?;
        if let Some(e) = clean(email) {
            let _ = conn.execute(
                "UPDATE labels SET email = ?2, updated_at = datetime('now') WHERE name_norm = ?1 AND COALESCE(trim(email), '') = ''",
                params![norm, e],
            );
        }
    }
    Ok(added)
}

#[derive(Debug, Serialize)]
pub struct LabelRow {
    pub id: i64,
    pub name: String,
    pub country: Option<String>,
    pub genres: Option<String>,
    pub website: Option<String>,
    pub email: Option<String>,
    pub contact_name: Option<String>,
    pub demo_policy: String,
    pub demo_url: Option<String>,
    pub tier: i64,
    pub notes: Option<String>,
    pub source: Option<String>,
    pub archived: bool,
    /// The address is on the suppression list (bounce, unsubscribe).
    pub suppressed: bool,
    /// Demos this label was sent (or is queued for), latest first.
    pub demos: Vec<LabelDemoRef>,
}

#[derive(Debug, Serialize)]
pub struct LabelDemoRef {
    pub campaign_id: i64,
    pub track: String,
    pub state: String,
}

#[tauri::command(async)]
pub fn labels_list(state: State<'_, AppState>) -> Result<Vec<LabelRow>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let mut rows: Vec<LabelRow> = conn
        .prepare(
            "SELECT l.id, l.name, l.country, l.genres, l.website, l.email, l.contact_name, l.demo_policy, l.demo_url,
                    l.tier, l.notes, l.source, l.archived,
                    EXISTS(SELECT 1 FROM suppressions s WHERE s.address = lower(trim(l.email)))
             FROM labels l ORDER BY l.archived, l.tier, l.name COLLATE NOCASE",
        )
        .map_err(|e| e.to_string())?
        .query_map([], |r| {
            Ok(LabelRow {
                id: r.get(0)?,
                name: r.get(1)?,
                country: r.get(2)?,
                genres: r.get(3)?,
                website: r.get(4)?,
                email: r.get(5)?,
                contact_name: r.get(6)?,
                demo_policy: r.get(7)?,
                demo_url: r.get(8)?,
                tier: r.get(9)?,
                notes: r.get(10)?,
                source: r.get(11)?,
                archived: r.get::<_, i64>(12)? != 0,
                suppressed: r.get(13)?,
                demos: Vec::new(),
            })
        })
        .map_err(|e| e.to_string())?
        .flatten()
        .collect();
    // Where each label stands, demo by demo (few demos: one pass per campaign).
    let mut by_label: std::collections::HashMap<i64, Vec<LabelDemoRef>> = std::collections::HashMap::new();
    for id in demo_ids(&conn, false) {
        let track = load(&conn, id).map(|r| r.config.track).unwrap_or_default();
        for t in targets(&conn, id, Utc::now()) {
            by_label.entry(t.label_id).or_default().push(LabelDemoRef { campaign_id: id, track: track.clone(), state: t.state });
        }
    }
    for r in rows.iter_mut() {
        if let Some(d) = by_label.remove(&r.id) {
            r.demos = d;
        }
    }
    Ok(rows)
}

#[derive(Debug, Deserialize)]
pub struct LabelInput {
    pub id: Option<i64>,
    pub name: String,
    pub country: Option<String>,
    pub genres: Option<String>,
    pub website: Option<String>,
    pub email: Option<String>,
    pub contact_name: Option<String>,
    pub demo_policy: String,
    pub demo_url: Option<String>,
    pub tier: i64,
    pub notes: Option<String>,
}

pub fn save_label(conn: &Connection, l: LabelInput) -> Result<i64, String> {
    let name = l.name.trim().to_string();
    let norm = normalise(&name);
    if norm.is_empty() {
        return Err("Le nom du label est obligatoire.".into());
    }
    if !["email", "form", "closed"].contains(&l.demo_policy.as_str()) {
        return Err("Canal de démo inconnu.".into());
    }
    let email = clean(l.email).map(|e| e.to_lowercase());
    if let Some(e) = &email {
        if !e.contains('@') || e.contains(' ') {
            return Err("Adresse email invalide.".into());
        }
    }
    let demo_url = clean(l.demo_url);
    if l.demo_policy == "form" && demo_url.is_none() {
        return Err("Indiquez l'adresse du formulaire de démo.".into());
    }
    let dup: Option<i64> = conn
        .query_row("SELECT id FROM labels WHERE name_norm = ?1", params![norm], |r| r.get(0))
        .optional()
        .map_err(|e| e.to_string())?;
    if dup.is_some() && dup != l.id {
        return Err("Ce label est déjà dans la liste.".into());
    }
    let tier = l.tier.clamp(1, 3);
    let (country, genres, website, person, notes) =
        (clean(l.country), clean(l.genres), clean(l.website), clean(l.contact_name), clean(l.notes));
    let id = match l.id {
        Some(id) => {
            conn.execute(
                "UPDATE labels SET name = ?2, name_norm = ?3, country = ?4, genres = ?5, website = ?6, email = ?7,
                        contact_name = ?8, demo_policy = ?9, demo_url = ?10, tier = ?11, notes = ?12, updated_at = datetime('now')
                 WHERE id = ?1",
                params![id, name, norm, country, genres, website, email, person, l.demo_policy, demo_url, tier, notes],
            )
            .map_err(|e| e.to_string())?;
            id
        }
        None => {
            conn.execute(
                "INSERT INTO labels (name, name_norm, country, genres, website, email, contact_name, demo_policy, demo_url, tier, notes, source)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 'Ajout manuel')",
                params![name, norm, country, genres, website, email, person, l.demo_policy, demo_url, tier, notes],
            )
            .map_err(|e| e.to_string())?;
            conn.last_insert_rowid()
        }
    };
    // The contact behind the label follows, as long as nothing was sent to
    // its previous address.
    let _ = conn.execute(
        "UPDATE contacts SET name = ?2, promoter = ?4, website = ?5,
                email = CASE WHEN ?3 IS NOT NULL AND NOT EXISTS (SELECT 1 FROM emails m WHERE m.contact_id = contacts.id AND m.status = 'sent')
                             THEN ?3 ELSE email END,
                updated_at = datetime('now')
         WHERE id = (SELECT contact_id FROM labels WHERE id = ?1)",
        params![id, name, email, person, website],
    );
    Ok(id)
}

#[tauri::command(async)]
pub fn labels_save(state: State<'_, AppState>, label: LabelInput) -> Result<i64, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    save_label(&conn, label)
}

/// Archive (or bring back) labels: an archived label leaves the list and is
/// never picked for a wave.
#[tauri::command(async)]
pub fn labels_set_archived(state: State<'_, AppState>, ids: Vec<i64>, archived: bool) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    for id in ids {
        conn.execute("UPDATE labels SET archived = ?2, updated_at = datetime('now') WHERE id = ?1", params![id, archived])
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command(async)]
pub fn labels_set_tier(state: State<'_, AppState>, ids: Vec<i64>, tier: i64) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    for id in ids {
        conn.execute("UPDATE labels SET tier = ?2, updated_at = datetime('now') WHERE id = ?1", params![id, tier.clamp(1, 3)])
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command(async)]
pub fn labels_import_radar(state: State<'_, AppState>) -> Result<usize, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    import_radar(&conn)
}

// ---------------------------------------------------------------------------
// Demo campaigns
// ---------------------------------------------------------------------------

/// The two emails of a demo: the first one, and one follow-up ten days later
/// in the same thread. Same voice as the club campaigns (no colon, no dash).
pub fn label_steps() -> Vec<Step> {
    vec![
        Step {
            offset_days: 0,
            subject: "{{track}}, démo pour {{label}}".into(),
            body: "Hello,\n\n\
                   Moi c'est Thibault, je produis sous le nom de {{artist}}, house et tech house. J'aime bien ce que vous sortez chez {{label}}, et je pense que {{track}} peut trouver sa place chez vous.\n\n\
                   Le morceau est en écoute privée juste en dessous, il n'est sorti nulle part.\n{{press_link}}\n\n\
                   Si ça vous parle, je vous envoie les fichiers.\n\n\
                   À bientôt,\nThibault"
                .into(),
            subject_en: "{{track}}, demo for {{label}}".into(),
            body_en: "Hey,\n\n\
                      I'm Thibault, I produce as {{artist}}, house and tech house. Really like what you're putting out on {{label}}, and I think {{track}} could fit in.\n\n\
                      The track is on a private link just below, unreleased.\n{{press_link}}\n\n\
                      If it sounds good to you, I'll send over the files.\n\n\
                      Cheers,\nThibault"
                .into(),
            ..Default::default()
        },
        Step {
            offset_days: FOLLOWUP_DAYS,
            subject: "Re: {{track}}, démo pour {{label}}".into(),
            body: "Hello,\n\n\
                   Petite relance au sujet de {{track}}, je sais que vous recevez beaucoup de démos.\n\n\
                   Le lien d'écoute est toujours là.\n{{press_link}}\n\n\
                   Et si ce n'est pas pour {{label}}, dites-le-moi simplement, aucun souci.\n\n\
                   Merci,\nThibault"
                .into(),
            subject_en: "Re: {{track}}, demo for {{label}}".into(),
            body_en: "Hey,\n\n\
                      Quick bump about {{track}}, I know you get a lot of demos.\n\n\
                      The private link is still up.\n{{press_link}}\n\n\
                      And if it's not one for {{label}}, just tell me, no worries at all.\n\n\
                      Thanks,\nThibault"
                .into(),
            ..Default::default()
        },
    ]
}

/// Ids of the demo campaigns, optionally only those still running.
fn demo_ids(conn: &Connection, active_only: bool) -> Vec<i64> {
    let sql = format!(
        "SELECT d.campaign_id FROM label_demos d JOIN campaigns c ON c.id = d.campaign_id
         WHERE c.kind = 'label' {} ORDER BY d.campaign_id DESC",
        if active_only { "AND c.status = 'active'" } else { "" }
    );
    let Ok(mut st) = conn.prepare(&sql) else { return Vec::new() };
    st.query_map([], |r| r.get(0)).map(|it| it.flatten().collect()).unwrap_or_default()
}

struct Demo {
    wave_size: i64,
    close_days: i64,
    current_wave: i64,
    name: String,
    status: String,
}

fn demo(conn: &Connection, id: i64) -> Result<Demo, String> {
    conn.query_row(
        "SELECT d.wave_size, d.close_days, d.current_wave, c.name, c.status
         FROM label_demos d JOIN campaigns c ON c.id = d.campaign_id WHERE d.campaign_id = ?1 AND c.kind = 'label'",
        params![id],
        |r| Ok(Demo { wave_size: r.get(0)?, close_days: r.get(1)?, current_wave: r.get(2)?, name: r.get(3)?, status: r.get(4)? }),
    )
    .map_err(|_| "Démo introuvable.".to_string())
}

fn listen_asset(track: &str, url: Option<String>) -> Vec<Asset> {
    match clean(url) {
        Some(u) => vec![Asset { kind: "link".into(), title: track.trim().to_string(), url: u, note: String::new() }],
        None => Vec::new(),
    }
}

pub fn create_demo(
    conn: &Connection,
    track: &str,
    press_kit_id: Option<String>,
    press_kit_title: Option<String>,
    listen_url: Option<String>,
) -> Result<i64, String> {
    let track = track.trim();
    if track.is_empty() {
        return Err("Donnez le titre du morceau.".into());
    }
    if let Some(u) = clean(listen_url.clone()) {
        if !u.starts_with("http") {
            return Err("Le lien d'écoute doit commencer par http.".into());
        }
    }
    let artist_id = crate::commands::artists::default_artist_id(conn);
    conn.execute(
        "INSERT INTO campaigns (name, purpose, artist_id, status, color, kind) VALUES (?1, 'Envoi de démo aux labels', ?2, 'active', '#ff5a1f', 'label')",
        params![track, artist_id],
    )
    .map_err(|e| e.to_string())?;
    let id = conn.last_insert_rowid();
    let cfg = Config {
        track: track.to_string(),
        steps: label_steps(),
        assets: listen_asset(track, listen_url),
        press_kit_id: clean(press_kit_id),
        press_kit_title: clean(press_kit_title),
        zone: Zone::default(),
        pitch_fr: String::new(),
        pitch_en: String::new(),
        ..Default::default()
    };
    save_config(conn, id, &cfg)?;
    // The engine sends for this campaign from the start: nothing leaves
    // before a first email is validated by hand.
    conn.execute(
        "UPDATE residency_campaigns SET autopilot = 1, autopilot_confirmed_at = datetime('now') WHERE campaign_id = ?1",
        params![id],
    )
    .map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO label_demos (campaign_id, wave_size, close_days) VALUES (?1, ?2, ?3)",
        params![id, DEFAULT_WAVE_SIZE, DEFAULT_CLOSE_DAYS],
    )
    .map_err(|e| e.to_string())?;
    residency::log(conn, Some(id), "info", "created", "Démo créée");
    Ok(id)
}

#[tauri::command(async)]
pub fn label_demo_create(
    state: State<'_, AppState>,
    track: String,
    press_kit_id: Option<String>,
    press_kit_title: Option<String>,
    listen_url: Option<String>,
) -> Result<i64, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    create_demo(&conn, &track, press_kit_id, press_kit_title, listen_url)
}

// ---------------- Targets ----------------

/// One label of a demo and where it stands.
#[derive(Debug, Clone, Serialize)]
pub struct TargetRow {
    pub id: i64,
    pub label_id: i64,
    pub name: String,
    pub country: Option<String>,
    pub email: Option<String>,
    pub demo_policy: String,
    pub demo_url: Option<String>,
    pub tier: i64,
    pub rank: i64,
    pub wave: Option<i64>,
    /// queued | unreachable | to_submit | to_validate | scheduled | failed |
    /// sent | followed | replied | declined | no_answer | bounced | withdrawn | signed
    pub state: String,
    pub outcome_by: Option<String>,
    pub contact_id: Option<i64>,
    /// The first email while it can still be edited.
    pub draft_id: Option<i64>,
    pub note: Option<String>,
    pub sent_at: Option<String>,
    /// When the follow-up leaves (or left).
    pub followup_at: Option<String>,
    pub followup_sent: bool,
    /// When the label is closed as "no answer".
    pub closes_at: Option<String>,
    pub opens: i64,
    pub views: i64,
    pub listens: i64,
    /// Best share of the track heard in one play (0..1).
    pub completion: Option<f64>,
    pub reply_intent: Option<String>,
    pub reply_summary: Option<String>,
}

fn reachable(policy: &str, email: Option<&str>, suppressed: bool, archived: bool) -> bool {
    if archived {
        return false;
    }
    match policy {
        "form" => true,
        "email" => email.map_or(false, |e| !e.trim().is_empty()) && !suppressed,
        _ => false,
    }
}

pub fn targets(conn: &Connection, campaign_id: i64, _now: DateTime<Utc>) -> Vec<TargetRow> {
    let close_days: i64 = conn
        .query_row("SELECT close_days FROM label_demos WHERE campaign_id = ?1", params![campaign_id], |r| r.get(0))
        .unwrap_or(DEFAULT_CLOSE_DAYS);
    let Ok(mut st) = conn.prepare(
        "SELECT t.id, t.label_id, l.name, l.country, l.email, l.demo_policy, l.demo_url, l.tier, t.rank, t.wave,
                t.outcome, t.outcome_by, l.contact_id, t.manual_sent_at, l.archived,
                EXISTS(SELECT 1 FROM suppressions s WHERE s.address = lower(trim(l.email)))
         FROM label_targets t JOIN labels l ON l.id = t.label_id
         WHERE t.campaign_id = ?1
         ORDER BY (t.wave IS NULL), t.wave, t.rank, t.id",
    ) else {
        return Vec::new();
    };
    type Raw = (TargetRow, Option<String>, Option<String>, bool, bool);
    let raws: Vec<Raw> = st
        .query_map(params![campaign_id], |r| {
            Ok((
                TargetRow {
                    id: r.get(0)?,
                    label_id: r.get(1)?,
                    name: r.get(2)?,
                    country: r.get(3)?,
                    email: r.get(4)?,
                    demo_policy: r.get(5)?,
                    demo_url: r.get(6)?,
                    tier: r.get(7)?,
                    rank: r.get(8)?,
                    wave: r.get(9)?,
                    state: String::new(),
                    outcome_by: r.get(11)?,
                    contact_id: r.get(12)?,
                    draft_id: None,
                    note: None,
                    sent_at: None,
                    followup_at: None,
                    followup_sent: false,
                    closes_at: None,
                    opens: 0,
                    views: 0,
                    listens: 0,
                    completion: None,
                    reply_intent: None,
                    reply_summary: None,
                },
                r.get::<_, Option<String>>(10)?,
                r.get::<_, Option<String>>(13)?,
                r.get::<_, i64>(14)? != 0,
                r.get::<_, bool>(15)?,
            ))
        })
        .map(|it| it.flatten().collect())
        .unwrap_or_default();
    let mut out = Vec::with_capacity(raws.len());
    for (mut t, outcome, manual_sent_at, archived, suppressed) in raws {
        // The two messages of this label, if it has a contact.
        let mut first: Option<(i64, String, Option<String>, Option<String>, i64)> = None;
        let mut second: Option<(String, Option<String>, Option<String>)> = None;
        if let (Some(cid), Some(_)) = (t.contact_id, t.wave) {
            if let Ok(mut ms) = conn.prepare(
                "SELECT m.id, m.step, m.status, m.scheduled_for, m.note, e.sent_at, COALESCE(e.open_count, 0)
                 FROM campaign_messages m LEFT JOIN emails e ON e.id = m.sent_email_id
                 WHERE m.campaign_id = ?1 AND m.contact_id = ?2 ORDER BY m.step",
            ) {
                let rows = ms.query_map(params![campaign_id, cid], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, Option<String>>(3)?,
                        r.get::<_, Option<String>>(4)?,
                        r.get::<_, Option<String>>(5)?,
                        r.get::<_, i64>(6)?,
                    ))
                });
                if let Ok(rows) = rows {
                    for (id, step, status, when, note, sent_at, opens) in rows.flatten() {
                        t.opens += opens;
                        if step == 1 {
                            if status != "sent" {
                                t.note = note;
                            }
                            first = Some((id, status, when, sent_at, opens));
                        } else if step == 2 {
                            second = Some((status, when, sent_at));
                        }
                    }
                }
            }
        }
        let start: Option<String> = match t.demo_policy.as_str() {
            "form" => manual_sent_at.clone(),
            _ => first.as_ref().filter(|f| f.1 == "sent").and_then(|f| f.3.clone()),
        };
        t.sent_at = start.clone();
        if let Some(s) = start.as_deref().and_then(engine::parse_utc) {
            t.closes_at = Some(engine::fmt(s + Duration::days(close_days)));
        }
        if let Some((status, when, sent_at)) = &second {
            t.followup_sent = status == "sent";
            t.followup_at = if t.followup_sent { sent_at.clone() } else if status == "scheduled" { when.clone() } else { None };
        }
        if let (Some(cid), Some(since)) = (t.contact_id, start.as_deref()) {
            if let Ok((views, listens, completion)) = conn.query_row(
                "SELECT COALESCE(SUM(type = 'view'), 0), COALESCE(SUM(type = 'listen'), 0), MAX(CASE WHEN type = 'listen' THEN completion END)
                 FROM press_activity WHERE contact_id = ?1 AND at >= ?2",
                params![cid, since],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, Option<f64>>(2)?)),
            ) {
                t.views = views;
                t.listens = listens;
                t.completion = completion;
            }
            if let Ok(Some((intent, summary))) = conn
                .query_row(
                    "SELECT i.intent, i.summary FROM reply_insights i JOIN email_replies r ON r.id = i.email_reply_id
                     WHERE i.contact_id = ?1 AND i.campaign_id = ?2 AND i.intent <> 'out_of_office'
                     ORDER BY COALESCE(r.received_at, r.created_at) DESC LIMIT 1",
                    params![cid, campaign_id],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)),
                )
                .optional()
            {
                t.reply_intent = Some(intent);
                t.reply_summary = summary;
            }
        }
        t.state = match (outcome.as_deref(), t.wave) {
            (Some(o), _) => o.to_string(),
            (None, None) => {
                if reachable(&t.demo_policy, t.email.as_deref(), suppressed, archived) { "queued" } else { "unreachable" }.to_string()
            }
            (None, Some(_)) if t.demo_policy == "form" => if manual_sent_at.is_some() { "sent" } else { "to_submit" }.to_string(),
            (None, Some(_)) => match first.as_ref().map(|f| f.1.as_str()) {
                Some("draft") => "to_validate",
                Some("scheduled") => "scheduled",
                Some("sent") => {
                    if t.followup_sent {
                        "followed"
                    } else {
                        "sent"
                    }
                }
                Some("failed") | Some("skipped") => "failed",
                _ => "unreachable",
            }
            .to_string(),
        };
        if let Some((id, status, ..)) = &first {
            if status != "sent" {
                t.draft_id = Some(*id);
            }
        }
        out.push(t);
    }
    out
}

/// The contact behind a label, created the first time the label enters a
/// wave. A contact already holding the same address is reused (the same
/// mailbox must not be written to twice).
fn ensure_contact(conn: &Connection, label_id: i64) -> Result<i64, String> {
    let (name, email, website, person, existing): (String, Option<String>, Option<String>, Option<String>, Option<i64>) = conn
        .query_row(
            "SELECT l.name, l.email, l.website, l.contact_name,
                    (SELECT k.id FROM contacts k WHERE k.id = l.contact_id)
             FROM labels l WHERE l.id = ?1",
            params![label_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .map_err(|e| e.to_string())?;
    let email = clean(email).ok_or_else(|| format!("{} : pas d'adresse email.", name))?;
    if let Some(cid) = existing {
        let _ = conn.execute(
            "UPDATE contacts SET email = ?2, updated_at = datetime('now')
             WHERE id = ?1 AND lower(trim(COALESCE(email, ''))) <> lower(?2)
               AND NOT EXISTS (SELECT 1 FROM emails m WHERE m.contact_id = ?1 AND m.status = 'sent')",
            params![cid, email],
        );
        return Ok(cid);
    }
    let same: Option<i64> = conn
        .query_row(
            "SELECT id FROM contacts WHERE lower(trim(email)) = lower(?1) ORDER BY (category IN ('label', 'curator')) DESC, id LIMIT 1",
            params![email],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let cid = match same {
        Some(cid) => cid,
        None => {
            conn.execute(
                "INSERT INTO contacts (category, name, promoter, email, website, contact_channel, status, reason, artist_id)
                 VALUES ('label', ?1, ?2, ?3, ?4, 'email', 'to_contact', 'Labels', ?5)",
                params![name, person, email, website, crate::commands::artists::default_artist_id(conn)],
            )
            .map_err(|e| e.to_string())?;
            conn.last_insert_rowid()
        }
    };
    conn.execute("UPDATE labels SET contact_id = ?2 WHERE id = ?1", params![label_id, cid]).map_err(|e| e.to_string())?;
    Ok(cid)
}

/// The first email of a label, as a draft the user validates. Returns false
/// when the label already has one.
fn draft_first(conn: &Connection, campaign_id: i64, cfg: &Config, contact_id: i64, insrt: Option<&dyn Insrt>) -> Result<bool, String> {
    let info = target_info(conn, contact_id, cfg).ok_or("Contact introuvable.")?;
    let mut note: Option<String> = None;
    let press = match content::press_link_for(conn, campaign_id, cfg, &info, insrt) {
        Ok(p) => p,
        Err(e) => {
            residency::log(conn, Some(campaign_id), "warn", "press_link_failed", &format!("{} : {}", info.name, e));
            None
        }
    };
    let artist = content::artist_name(conn, campaign_id);
    let vars = content::vars_for(&info, cfg, &artist, press.as_deref());
    let (subject, body) = content::render_step(cfg, 0, &info.lang, &vars).ok_or("La démo n'a pas de premier mail.")?;
    let missing = content::unresolved(&format!("{}\n{}", subject, body));
    if !missing.is_empty() {
        note = Some(if missing.iter().any(|m| m == "press_link") {
            "Lien d'écoute manquant".to_string()
        } else {
            format!("Variables à compléter : {}", missing.join(", "))
        });
    }
    let n = conn
        .execute(
            "INSERT OR IGNORE INTO campaign_messages (campaign_id, contact_id, step, subject, body, lang, status, press_url, note, generated_by)
             VALUES (?1, ?2, 1, ?3, ?4, ?5, 'draft', ?6, ?7, 'template')",
            params![campaign_id, contact_id, subject, body, info.lang, press, note],
        )
        .map_err(|e| e.to_string())?;
    Ok(n > 0)
}

/// Open the next wave: the first `wave_size` reachable labels still waiting,
/// in rank order. Email labels get their draft, form labels wait for the
/// user's manual submission. Returns how many labels entered the wave.
pub fn open_wave(conn: &Connection, campaign_id: i64, insrt: Option<&dyn Insrt>) -> Result<usize, String> {
    let d = demo(conn, campaign_id)?;
    let cfg = load(conn, campaign_id).map(|r| r.config).ok_or("Démo introuvable.")?;
    let picks: Vec<(i64, i64, String)> = conn
        .prepare(
            "SELECT t.id, t.label_id, l.demo_policy FROM label_targets t JOIN labels l ON l.id = t.label_id
             WHERE t.campaign_id = ?1 AND t.wave IS NULL AND t.outcome IS NULL AND l.archived = 0
               AND (l.demo_policy = 'form'
                    OR (l.demo_policy = 'email' AND COALESCE(trim(l.email), '') <> ''
                        AND NOT EXISTS (SELECT 1 FROM suppressions s WHERE s.address = lower(trim(l.email)))))
             ORDER BY t.rank, t.id LIMIT ?2",
        )
        .map_err(|e| e.to_string())?
        .query_map(params![campaign_id, d.wave_size.max(1)], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .map_err(|e| e.to_string())?
        .flatten()
        .collect();
    if picks.is_empty() {
        return Err("Aucun label joignable en attente : ajoutez des labels à la liste de cette démo.".into());
    }
    let wave = d.current_wave + 1;
    conn.execute("UPDATE label_demos SET current_wave = ?2 WHERE campaign_id = ?1", params![campaign_id, wave])
        .map_err(|e| e.to_string())?;
    for (tid, label_id, policy) in &picks {
        conn.execute("UPDATE label_targets SET wave = ?2 WHERE id = ?1", params![tid, wave]).map_err(|e| e.to_string())?;
        if policy != "email" {
            continue;
        }
        let cid = ensure_contact(conn, *label_id)?;
        // The contact's status follows the demo in progress: an answer to an
        // earlier demo must not stop this one.
        let _ = conn.execute(
            "UPDATE contacts SET status = 'to_contact', updated_at = datetime('now')
             WHERE id = ?1 AND category IN ('label', 'curator') AND status <> 'to_contact'",
            params![cid],
        );
        conn.execute(
            "INSERT OR IGNORE INTO campaign_contacts (campaign_id, contact_id, state) VALUES (?1, ?2, 'planned')",
            params![campaign_id, cid],
        )
        .map_err(|e| e.to_string())?;
        draft_first(conn, campaign_id, &cfg, cid, insrt)?;
    }
    residency::log(conn, Some(campaign_id), "info", "wave_opened", &format!("Vague {} ouverte : {} label(s)", wave, picks.len()));
    Ok(picks.len())
}

fn set_outcome(conn: &Connection, target_id: i64, outcome: Option<&str>, by: &str) {
    let _ = conn.execute(
        "UPDATE label_targets SET outcome = ?2, outcome_by = CASE WHEN ?2 IS NULL THEN NULL ELSE ?3 END,
                outcome_at = CASE WHEN ?2 IS NULL THEN NULL ELSE datetime('now') END
         WHERE id = ?1",
        params![target_id, outcome, by],
    );
}

/// Close what can be closed and open the next wave when the current one is
/// done. Returns how many labels entered a new wave.
pub fn advance(conn: &Connection, campaign_id: i64, now: DateTime<Utc>, insrt: Option<&dyn Insrt>) -> usize {
    let Ok(d) = demo(conn, campaign_id) else { return 0 };
    if d.status != "active" {
        return 0;
    }
    let rows: Vec<(i64, Option<i64>, String, Option<String>, Option<String>, Option<String>)> = conn
        .prepare(
            "SELECT t.id, l.contact_id, l.demo_policy, t.manual_sent_at, t.outcome, t.outcome_by
             FROM label_targets t JOIN labels l ON l.id = t.label_id
             WHERE t.campaign_id = ?1 AND t.wave IS NOT NULL AND (t.outcome IS NULL OR (t.outcome = 'replied' AND t.outcome_by = 'auto'))",
        )
        .and_then(|mut st| {
            st.query_map(params![campaign_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))
                .map(|it| it.flatten().collect())
        })
        .unwrap_or_default();
    for (tid, contact_id, policy, manual_sent_at, outcome, _) in rows {
        if policy == "form" {
            let due = manual_sent_at.as_deref().and_then(engine::parse_utc).map_or(false, |s| now >= s + Duration::days(d.close_days));
            if due && outcome.is_none() {
                set_outcome(conn, tid, Some("no_answer"), "auto");
            }
            continue;
        }
        let Some(cid) = contact_id else { continue };
        // A refusal read in the reply (classification) beats the plain "replied".
        let refused: bool = conn
            .query_row(
                "SELECT i.intent IN ('not_interested', 'unsubscribe') FROM reply_insights i JOIN email_replies r ON r.id = i.email_reply_id
                 WHERE i.contact_id = ?1 AND i.campaign_id = ?2 AND i.intent <> 'out_of_office'
                 ORDER BY COALESCE(r.received_at, r.created_at) DESC LIMIT 1",
                params![cid, campaign_id],
                |r| r.get(0),
            )
            .unwrap_or(false);
        let new = match engine::stop_reason(conn, campaign_id, cid).as_deref() {
            Some("rebond") => Some("bounced"),
            Some("contact passé en refus") | Some("désinscription") => Some("declined"),
            Some("réponse reçue") | Some("conversation déjà en cours") | Some("date confirmée") => {
                Some(if refused { "declined" } else { "replied" })
            }
            // No address any more, or suppressed since the wave opened.
            Some(_) => Some("bounced"),
            None => match engine::j0(conn, campaign_id, cid) {
                Some(j0) if now >= j0 + Duration::days(d.close_days) => Some("no_answer"),
                _ => None,
            },
        };
        match (outcome.as_deref(), new) {
            (None, Some(o)) => {
                set_outcome(conn, tid, Some(o), "auto");
                if o != "replied" {
                    engine::skip_remaining(conn, campaign_id, cid, "label clos");
                }
            }
            (Some("replied"), Some("declined")) => set_outcome(conn, tid, Some("declined"), "auto"),
            _ => {}
        }
    }
    // Next wave: every label of the current one is closed, some still wait.
    if d.current_wave == 0 {
        return 0;
    }
    let open_in_wave: i64 = conn
        .query_row(
            &format!(
                "SELECT COUNT(*) FROM label_targets WHERE campaign_id = ?1 AND wave = ?2
                 AND (outcome IS NULL OR outcome NOT IN ({}))",
                CLOSED.iter().map(|c| format!("'{}'", c)).collect::<Vec<_>>().join(",")
            ),
            params![campaign_id, d.current_wave],
            |r| r.get(0),
        )
        .unwrap_or(1);
    if open_in_wave > 0 {
        return 0;
    }
    open_wave(conn, campaign_id, insrt).unwrap_or(0)
}

/// Engine tick: every running demo is advanced; a wave that opened by itself
/// is announced (its first emails wait for validation).
pub fn tick(conn: &Connection, app: Option<&tauri::AppHandle>, now: DateTime<Utc>) {
    let ids = demo_ids(conn, true);
    if ids.is_empty() {
        return;
    }
    let client = residency::insrt::client(conn);
    for id in ids {
        let n = advance(conn, id, now, client.as_deref());
        if n == 0 {
            continue;
        }
        if let (Some(app), Ok(d)) = (app, demo(conn, id)) {
            use tauri_plugin_notification::NotificationExt;
            let _ = app
                .notification()
                .builder()
                .title("Labels")
                .body(format!("{} : vague {} ouverte, {} label(s) à valider.", d.name, d.current_wave, n))
                .show();
        }
    }
}

// ---------------- Commands ----------------

#[derive(Debug, Serialize)]
pub struct DemoCard {
    pub id: i64,
    pub track: String,
    pub status: String,
    pub created_at: String,
    pub current_wave: i64,
    pub paused_reason: Option<String>,
    pub total: usize,
    pub queued: usize,
    /// First emails to validate plus forms to submit.
    pub todo: usize,
    pub waiting: usize,
    pub replied: usize,
    pub closed: usize,
    pub listened: usize,
    pub signed: bool,
}

fn card(conn: &Connection, id: i64, now: DateTime<Utc>) -> Option<DemoCard> {
    let d = demo(conn, id).ok()?;
    let r = load(conn, id)?;
    let created_at: String = conn.query_row("SELECT created_at FROM campaigns WHERE id = ?1", params![id], |r| r.get(0)).ok()?;
    let ts = targets(conn, id, now);
    let count = |states: &[&str]| ts.iter().filter(|t| states.contains(&t.state.as_str())).count();
    Some(DemoCard {
        id,
        track: if r.config.track.trim().is_empty() { d.name.clone() } else { r.config.track.clone() },
        status: d.status,
        created_at,
        current_wave: d.current_wave,
        paused_reason: r.paused_reason,
        total: ts.len(),
        queued: count(&["queued", "unreachable"]) - ts.iter().filter(|t| t.state == "unreachable" && t.wave.is_some()).count(),
        todo: count(&["to_validate", "to_submit"]),
        waiting: count(&["scheduled", "sent", "followed"]),
        replied: count(&["replied"]),
        closed: count(CLOSED),
        listened: ts.iter().filter(|t| t.listens > 0).count(),
        signed: ts.iter().any(|t| t.state == "signed"),
    })
}

#[tauri::command(async)]
pub fn label_demos_list(state: State<'_, AppState>) -> Result<Vec<DemoCard>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let now = Utc::now();
    Ok(demo_ids(&conn, false).into_iter().filter_map(|id| card(&conn, id, now)).collect())
}

#[derive(Debug, Serialize)]
pub struct DemoView {
    pub card: DemoCard,
    pub press_kit_id: Option<String>,
    pub press_kit_title: Option<String>,
    pub listen_url: Option<String>,
    pub steps: Vec<Step>,
    pub window: Window,
    pub wave_size: i64,
    pub close_days: i64,
    pub insrt_ready: bool,
    pub smtp_ready: bool,
    pub next_send: Option<String>,
    pub targets: Vec<TargetRow>,
}

#[tauri::command(async)]
pub fn label_demo_get(state: State<'_, AppState>, id: i64) -> Result<DemoView, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let now = Utc::now();
    let d = demo(&conn, id)?;
    let r = load(&conn, id).ok_or("Démo introuvable.")?;
    let smtp_ready = crate::commands::email::load_smtp_from(&state.pool)
        .map(|c| !c.host.trim().is_empty() && !c.from_email.trim().is_empty() && !c.password.trim().is_empty())
        .unwrap_or(false);
    Ok(DemoView {
        card: card(&conn, id, now).ok_or("Démo introuvable.")?,
        press_kit_id: r.config.press_kit_id.clone(),
        press_kit_title: r.config.press_kit_title.clone(),
        listen_url: r.config.assets.first().map(|a| a.url.clone()),
        steps: r.config.steps.clone(),
        window: r.config.window.clone(),
        wave_size: d.wave_size,
        close_days: d.close_days,
        insrt_ready: residency::insrt::client(&conn).is_some(),
        smtp_ready,
        next_send: engine::next_send_estimate(&conn, id, &r.config, now).map(engine::fmt),
        targets: targets(&conn, id, now),
    })
}

#[derive(Debug, Deserialize)]
pub struct DemoPatch {
    pub track: String,
    pub press_kit_id: Option<String>,
    pub press_kit_title: Option<String>,
    pub listen_url: Option<String>,
    pub steps: Vec<Step>,
    pub wave_size: i64,
    pub close_days: i64,
}

pub fn save_demo(conn: &Connection, id: i64, p: DemoPatch) -> Result<(), String> {
    demo(conn, id)?;
    let track = p.track.trim().to_string();
    if track.is_empty() {
        return Err("Donnez le titre du morceau.".into());
    }
    if p.steps.is_empty() || p.steps[0].offset_days != 0 {
        return Err("Le premier mail part à J0.".into());
    }
    if p.steps.len() > 3 {
        return Err("Trois mails au plus par label.".into());
    }
    for w in p.steps.windows(2) {
        if w[1].offset_days <= w[0].offset_days {
            return Err("Chaque relance doit partir après le mail précédent.".into());
        }
    }
    let last = p.steps.last().map_or(0, |s| s.offset_days);
    if p.close_days <= last {
        return Err(format!("La clôture doit venir après la dernière relance (J+{}).", last));
    }
    if !(1..=20).contains(&p.wave_size) {
        return Err("Une vague compte de 1 à 20 labels.".into());
    }
    let mut cfg = load(conn, id).map(|r| r.config).ok_or("Démo introuvable.")?;
    cfg.assets = listen_asset(&track, p.listen_url);
    cfg.track = track.clone();
    cfg.press_kit_id = clean(p.press_kit_id);
    cfg.press_kit_title = clean(p.press_kit_title);
    cfg.steps = p.steps;
    save_config(conn, id, &cfg)?;
    conn.execute("UPDATE campaigns SET name = ?2, updated_at = datetime('now') WHERE id = ?1", params![id, track])
        .map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE label_demos SET wave_size = ?2, close_days = ?3 WHERE campaign_id = ?1",
        params![id, p.wave_size, p.close_days],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command(async)]
pub fn label_demo_save(state: State<'_, AppState>, id: i64, patch: DemoPatch) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    save_demo(&conn, id, patch)
}

/// The ranked list of labels of a demo. Labels still waiting follow the given
/// order (and leave when absent from it); labels already in a wave stay.
pub fn set_targets(conn: &Connection, id: i64, label_ids: &[i64]) -> Result<(), String> {
    demo(conn, id)?;
    let waiting: Vec<(i64, i64)> = conn
        .prepare("SELECT id, label_id FROM label_targets WHERE campaign_id = ?1 AND wave IS NULL")
        .map_err(|e| e.to_string())?
        .query_map(params![id], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(|e| e.to_string())?
        .flatten()
        .collect();
    for (tid, lid) in &waiting {
        if !label_ids.contains(lid) {
            conn.execute("DELETE FROM label_targets WHERE id = ?1", params![tid]).map_err(|e| e.to_string())?;
        }
    }
    for (i, lid) in label_ids.iter().enumerate() {
        conn.execute(
            "INSERT INTO label_targets (campaign_id, label_id, rank) VALUES (?1, ?2, ?3)
             ON CONFLICT(campaign_id, label_id) DO UPDATE SET rank = excluded.rank WHERE label_targets.wave IS NULL",
            params![id, lid, i as i64 + 1],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command(async)]
pub fn label_demo_set_targets(state: State<'_, AppState>, id: i64, label_ids: Vec<i64>) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    set_targets(&conn, id, &label_ids)
}

/// Add labels at the end of a demo's waiting list (from the labels list).
#[tauri::command(async)]
pub fn label_demo_add_labels(state: State<'_, AppState>, id: i64, label_ids: Vec<i64>) -> Result<usize, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    demo(&conn, id)?;
    let mut rank: i64 = conn
        .query_row("SELECT COALESCE(MAX(rank), 0) FROM label_targets WHERE campaign_id = ?1", params![id], |r| r.get(0))
        .unwrap_or(0);
    let mut n = 0;
    for lid in label_ids {
        rank += 1;
        n += conn
            .execute("INSERT OR IGNORE INTO label_targets (campaign_id, label_id, rank) VALUES (?1, ?2, ?3)", params![id, lid, rank])
            .map_err(|e| e.to_string())?;
    }
    Ok(n)
}

/// "Lancer la vague": the first one, or the next one before its time.
#[tauri::command(async)]
pub fn label_demo_open_wave(state: State<'_, AppState>, id: i64) -> Result<usize, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let client = residency::insrt::client(&conn);
    open_wave(&conn, id, client.as_deref())
}

/// Rewrite the first emails still in draft from the current templates (after
/// the templates, the track title or the listening link changed).
#[tauri::command(async)]
pub fn label_demo_redraft(state: State<'_, AppState>, id: i64) -> Result<usize, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let cfg = load(&conn, id).map(|r| r.config).ok_or("Démo introuvable.")?;
    let client = residency::insrt::client(&conn);
    let drafts: Vec<(i64, i64)> = conn
        .prepare("SELECT id, contact_id FROM campaign_messages WHERE campaign_id = ?1 AND step = 1 AND status = 'draft'")
        .map_err(|e| e.to_string())?
        .query_map(params![id], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(|e| e.to_string())?
        .flatten()
        .collect();
    let mut n = 0;
    for (mid, cid) in drafts {
        conn.execute("DELETE FROM campaign_messages WHERE id = ?1", params![mid]).map_err(|e| e.to_string())?;
        if draft_first(&conn, id, &cfg, cid, client.as_deref())? {
            n += 1;
        }
    }
    Ok(n)
}

/// The user's verdict on a label: declined, withdrawn, signed, replied (still
/// talking), or None to let the app decide again.
#[tauri::command(async)]
pub fn label_target_set_outcome(state: State<'_, AppState>, target_id: i64, outcome: Option<String>) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    if let Some(o) = outcome.as_deref() {
        if !OUTCOMES.contains(&o) {
            return Err("Issue inconnue.".into());
        }
    }
    let (campaign_id, contact_id): (i64, Option<i64>) = conn
        .query_row(
            "SELECT t.campaign_id, l.contact_id FROM label_targets t JOIN labels l ON l.id = t.label_id WHERE t.id = ?1",
            params![target_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|_| "Label introuvable dans cette démo.".to_string())?;
    set_outcome(&conn, target_id, outcome.as_deref(), "user");
    if let (Some(o), Some(cid)) = (outcome.as_deref(), contact_id) {
        if o != "replied" {
            engine::skip_remaining(&conn, campaign_id, cid, "label clos");
        }
    }
    if outcome.as_deref() == Some("signed") {
        // The track has a home: nothing else leaves for this demo.
        conn.execute("UPDATE campaigns SET status = 'done', updated_at = datetime('now') WHERE id = ?1", params![campaign_id])
            .map_err(|e| e.to_string())?;
        residency::log(&conn, Some(campaign_id), "info", "signed", "Démo signée, envois arrêtés");
        return Ok(());
    }
    // The verdict may close the wave (the next one opens) or hand the label
    // back to the automatic rules: applied right away, not at the next tick.
    let client = residency::insrt::client(&conn);
    advance(&conn, campaign_id, Utc::now(), client.as_deref());
    Ok(())
}

/// A demo submitted through the label's own form (or taken back).
#[tauri::command(async)]
pub fn label_target_mark_submitted(state: State<'_, AppState>, target_id: i64, done: bool) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE label_targets SET manual_sent_at = CASE WHEN ?2 THEN datetime('now') ELSE NULL END WHERE id = ?1 AND wave IS NOT NULL",
        params![target_id, done],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// active (also lifts a pause of the engine), paused or done.
#[tauri::command(async)]
pub fn label_demo_set_status(state: State<'_, AppState>, id: i64, status: String) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    demo(&conn, id)?;
    if !["active", "paused", "done"].contains(&status.as_str()) {
        return Err("Statut inconnu.".into());
    }
    if status == "active" {
        if let Some(reason) = crate::commands::deliverability::autopilot_block(&conn) {
            return Err(reason);
        }
        conn.execute(
            "UPDATE residency_campaigns SET autopilot = 1, paused_reason = NULL, paused_kind = NULL, updated_at = datetime('now') WHERE campaign_id = ?1",
            params![id],
        )
        .map_err(|e| e.to_string())?;
        engine::put(&conn, "residency_bounce_reset_at", &engine::fmt(Utc::now()));
    }
    conn.execute("UPDATE campaigns SET status = ?2, updated_at = datetime('now') WHERE id = ?1", params![id, status])
        .map_err(|e| e.to_string())?;
    engine::wake();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::residency::testdb;

    fn label(c: &Connection, name: &str, email: Option<&str>, policy: &str) -> i64 {
        save_label(
            c,
            LabelInput {
                id: None,
                name: name.into(),
                country: Some("United Kingdom".into()),
                genres: None,
                website: None,
                email: email.map(String::from),
                contact_name: None,
                demo_policy: policy.into(),
                demo_url: (policy == "form").then(|| "https://label.example/demos".to_string()),
                tier: 2,
                notes: None,
            },
        )
        .unwrap()
    }

    fn sent(c: &Connection, campaign: i64, contact: i64, at: DateTime<Utc>) {
        c.execute(
            "INSERT INTO emails (contact_id, campaign_id, to_addr, subject, body, status, sent_at) VALUES (?1, ?2, 'x@x.io', 's', 'b', 'sent', ?3)",
            params![contact, campaign, engine::fmt(at)],
        )
        .unwrap();
        let eid = c.last_insert_rowid();
        c.execute(
            "UPDATE campaign_messages SET status = 'sent', sent_email_id = ?3 WHERE campaign_id = ?1 AND contact_id = ?2 AND step = 1",
            params![campaign, contact, eid],
        )
        .unwrap();
    }

    fn state_of(c: &Connection, id: i64, name: &str) -> String {
        targets(c, id, Utc::now()).into_iter().find(|t| t.name == name).unwrap().state
    }

    #[test]
    fn waves_open_in_rank_order_and_close_after_silence() {
        let c = testdb::conn();
        c.execute("INSERT INTO artists (name) VALUES ('Piers')", []).unwrap();
        let id = create_demo(&c, "Night Drive", None, None, Some("https://soundcloud.com/piers/night-drive/s-abc".into())).unwrap();
        c.execute("UPDATE label_demos SET wave_size = 2 WHERE campaign_id = ?1", params![id]).unwrap();
        let a = label(&c, "Alpha", Some("demos@alpha.io"), "email");
        let b = label(&c, "Beta", None, "form");
        let n = label(&c, "No Mail", None, "email");
        let g = label(&c, "Gamma", Some("demos@gamma.io"), "email");
        set_targets(&c, id, &[a, n, b, g]).unwrap();
        assert_eq!(state_of(&c, id, "No Mail"), "unreachable");

        // Wave 1: the two first reachable labels, the one without address is passed over.
        assert_eq!(open_wave(&c, id, None).unwrap(), 2);
        assert_eq!(state_of(&c, id, "Alpha"), "to_validate");
        assert_eq!(state_of(&c, id, "Beta"), "to_submit");
        assert_eq!(state_of(&c, id, "Gamma"), "queued");
        let (subject, body, lang): (String, String, String) = c
            .query_row("SELECT subject, body, lang FROM campaign_messages WHERE campaign_id = ?1", params![id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap();
        assert_eq!(lang, "en");
        assert_eq!(subject, "Night Drive, demo for Alpha");
        assert!(body.contains("https://soundcloud.com/piers/night-drive/s-abc"));
        assert!(content::unresolved(&format!("{}\n{}", subject, body)).is_empty());

        // Nothing closes, nothing opens, while the wave is running.
        let t0 = Utc::now();
        let alpha: i64 = c.query_row("SELECT contact_id FROM labels WHERE id = ?1", params![a], |r| r.get(0)).unwrap();
        sent(&c, id, alpha, t0);
        c.execute("UPDATE label_targets SET manual_sent_at = ?2 WHERE label_id = ?1", params![b, engine::fmt(t0)]).unwrap();
        assert_eq!(advance(&c, id, t0 + Duration::days(20), None), 0);
        assert_eq!(state_of(&c, id, "Alpha"), "sent");

        // J+21: both closed as no answer, wave 2 opens with the label left.
        assert_eq!(advance(&c, id, t0 + Duration::days(21), None), 1);
        assert_eq!(state_of(&c, id, "Alpha"), "no_answer");
        assert_eq!(state_of(&c, id, "Beta"), "no_answer");
        assert_eq!(state_of(&c, id, "Gamma"), "to_validate");
        assert_eq!(demo(&c, id).unwrap().current_wave, 2);
    }

    #[test]
    fn a_reply_holds_the_next_wave_until_the_user_decides() {
        let c = testdb::conn();
        let id = create_demo(&c, "Night Drive", None, None, Some("https://example.com/listen".into())).unwrap();
        c.execute("UPDATE label_demos SET wave_size = 1 WHERE campaign_id = ?1", params![id]).unwrap();
        let a = label(&c, "Alpha", Some("demos@alpha.io"), "email");
        let g = label(&c, "Gamma", Some("demos@gamma.io"), "email");
        set_targets(&c, id, &[a, g]).unwrap();
        open_wave(&c, id, None).unwrap();
        let alpha: i64 = c.query_row("SELECT contact_id FROM labels WHERE id = ?1", params![a], |r| r.get(0)).unwrap();
        let t0 = Utc::now() - Duration::days(3);
        sent(&c, id, alpha, t0);
        c.execute(
            "INSERT INTO email_replies (contact_id, from_addr, subject, message_id, kind, received_at) VALUES (?1, 'demos@alpha.io', 'Re', 'm1', 'reply', ?2)",
            params![alpha, engine::fmt(t0 + Duration::days(1))],
        )
        .unwrap();
        assert_eq!(advance(&c, id, Utc::now(), None), 0);
        assert_eq!(state_of(&c, id, "Alpha"), "replied");
        // Even long after: a label that answered is the user's call.
        assert_eq!(advance(&c, id, Utc::now() + Duration::days(60), None), 0);
        assert_eq!(state_of(&c, id, "Gamma"), "queued");
        let tid: i64 = c.query_row("SELECT id FROM label_targets WHERE label_id = ?1", params![a], |r| r.get(0)).unwrap();
        set_outcome(&c, tid, Some("declined"), "user");
        assert_eq!(advance(&c, id, Utc::now(), None), 1);
        assert_eq!(state_of(&c, id, "Gamma"), "to_validate");
    }

    #[test]
    fn radar_labels_come_in_once_with_their_address() {
        let c = testdb::conn();
        c.execute_batch(
            "INSERT INTO radar_curators (source, external_id, kind, nom, nom_normalise, pays, genre, site_web)
               VALUES ('labels', 'mb1', 'label', 'Deep Cuts', 'deepcuts', 'France', 'house', 'https://deepcuts.fr');
             INSERT INTO radar_curators (source, external_id, kind, nom, nom_normalise, statut)
               VALUES ('labels', 'mb2', 'label', 'Rejected', 'rejected', 'rejete');
             INSERT INTO radar_contacts (curator_id, type, valeur, score) VALUES (1, 'email', 'demos@deepcuts.fr', 50);",
        )
        .unwrap();
        assert_eq!(import_radar(&c).unwrap(), 1);
        assert_eq!(import_radar(&c).unwrap(), 0);
        let (email, country): (String, String) =
            c.query_row("SELECT email, country FROM labels WHERE name = 'Deep Cuts'", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!(email, "demos@deepcuts.fr");
        assert_eq!(iso_of(&country), "FR");
        // No em dash, no colon in the default copy.
        for s in label_steps() {
            for text in [&s.subject, &s.body, &s.subject_en, &s.body_en] {
                assert!(!text.contains('—'));
                assert!(!text.replace("Re: ", "").contains(':'));
            }
        }
    }
}
