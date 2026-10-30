//! "Résidence" campaigns: one page holding the pitch and every asset, a target
//! builder over Discovery, personalised first emails, and an autopilot that
//! sends the validated sequence on its own, stops on any reply, reads the
//! answers (Claude), follows the private EPK link activity (insrt.studio) and
//! writes a daily report.
//!
//! - `zone`      target zone, language, time zone and send window (pure)
//! - `claude`    Messages API client behind the `Llm` trait, usage + cost
//! - `insrt`     insrt.studio CRM API client behind the `Insrt` trait
//! - `targeting` "Construire la cible" over vi_venues + pipeline contacts
//! - `content`   drafts (Claude for step 1, templates for follow-ups)
//! - `engine`    autopilot loop, safety gates, step progression, stop rules
//! - `replies`   reply classification and what each answer triggers
//! - `report`    daily report aggregation + synthesis
//! - `commands`  the Tauri commands the campaign page calls
//!
//! The agent never answers a human: interested / question replies become an
//! Action with a suggested answer the user sends from ComposeModal.

pub mod claude;
pub mod commands;
pub mod content;
pub mod engine;
pub mod insrt;
pub mod replies;
pub mod report;
pub mod targeting;
pub mod zone;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use zone::{Window, Zone};

// ---------------------------------------------------------------------------
// Schema (idempotent, run on every launch from db::migrate)
// ---------------------------------------------------------------------------

/// Residency tables. Additive only: new tables are IF NOT EXISTS, new columns
/// tolerate "duplicate column" on a second run, nothing existing is rewritten.
///
/// - campaigns.kind               'standard' (every existing row) | 'residency'
/// - residency_campaigns          one row per residency campaign: the editable
///                                config as JSON (pitch FR/EN, concept, ask,
///                                assets, press kit, sequence, window, caps,
///                                zone, language) + the autopilot switch and
///                                the reason it is paused, if any
/// - campaign_messages            one row per (campaign, contact, step): the
///                                draft, then the scheduled/sent email
/// - reply_insights               Claude's (or the rules') reading of a reply
/// - press_activity               insrt EPK link events (view/listen/feedback/download)
/// - campaign_reports             one daily report per campaign
/// - residency_log                every autopilot decision (ring, pruned)
/// - ai_usage                     tokens + cost of every Claude call
/// - email_replies.body_excerpt   text of a reply, for classification
pub fn migrate(conn: &Connection) {
    for stmt in [
        "ALTER TABLE campaigns ADD COLUMN kind TEXT NOT NULL DEFAULT 'standard'",
        "ALTER TABLE email_replies ADD COLUMN body_excerpt TEXT",
        "CREATE TABLE IF NOT EXISTS residency_campaigns (
            campaign_id            INTEGER PRIMARY KEY REFERENCES campaigns(id) ON DELETE CASCADE,
            config                 TEXT NOT NULL DEFAULT '{}',
            autopilot              INTEGER NOT NULL DEFAULT 0,
            autopilot_confirmed_at TEXT,
            paused_reason          TEXT,
            paused_kind            TEXT,
            updated_at             TEXT NOT NULL DEFAULT (datetime('now'))
        )",
        "CREATE TABLE IF NOT EXISTS campaign_messages (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            campaign_id   INTEGER NOT NULL REFERENCES campaigns(id) ON DELETE CASCADE,
            contact_id    INTEGER NOT NULL REFERENCES contacts(id) ON DELETE CASCADE,
            step          INTEGER NOT NULL,
            subject       TEXT NOT NULL DEFAULT '',
            body          TEXT NOT NULL DEFAULT '',
            lang          TEXT NOT NULL DEFAULT 'fr',
            status        TEXT NOT NULL DEFAULT 'draft',
            scheduled_for TEXT,
            sent_email_id INTEGER REFERENCES emails(id) ON DELETE SET NULL,
            press_url     TEXT,
            note          TEXT,
            generated_by  TEXT,
            created_at    TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at    TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE (campaign_id, contact_id, step)
        )",
        "CREATE INDEX IF NOT EXISTS idx_campaign_messages_due ON campaign_messages(status, scheduled_for)",
        "CREATE INDEX IF NOT EXISTS idx_campaign_messages_contact ON campaign_messages(contact_id)",
        "CREATE TABLE IF NOT EXISTS reply_insights (
            id              INTEGER PRIMARY KEY AUTOINCREMENT,
            email_reply_id  INTEGER NOT NULL UNIQUE REFERENCES email_replies(id) ON DELETE CASCADE,
            contact_id      INTEGER REFERENCES contacts(id) ON DELETE CASCADE,
            campaign_id     INTEGER REFERENCES campaigns(id) ON DELETE CASCADE,
            sentiment       TEXT NOT NULL,
            intent          TEXT NOT NULL,
            summary         TEXT,
            suggested_reply TEXT,
            follow_up_on    TEXT,
            mentioned_email TEXT,
            model           TEXT,
            handled         INTEGER NOT NULL DEFAULT 0,
            created_at      TEXT NOT NULL DEFAULT (datetime('now'))
        )",
        "CREATE TABLE IF NOT EXISTS press_activity (
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            event_id     TEXT NOT NULL UNIQUE,
            contact_id   INTEGER REFERENCES contacts(id) ON DELETE CASCADE,
            campaign_id  INTEGER REFERENCES campaigns(id) ON DELETE SET NULL,
            link_id      TEXT,
            type         TEXT NOT NULL,
            at           TEXT NOT NULL,
            track_title  TEXT,
            seconds      REAL,
            completion   REAL,
            rating       REAL,
            comment      TEXT,
            created_at   TEXT NOT NULL DEFAULT (datetime('now'))
        )",
        "CREATE INDEX IF NOT EXISTS idx_press_activity_contact ON press_activity(contact_id, at)",
        "CREATE TABLE IF NOT EXISTS campaign_reports (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            campaign_id INTEGER NOT NULL REFERENCES campaigns(id) ON DELETE CASCADE,
            day         TEXT NOT NULL,
            data        TEXT NOT NULL,
            synthesis   TEXT,
            created_at  TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE (campaign_id, day)
        )",
        "CREATE TABLE IF NOT EXISTS residency_log (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            campaign_id INTEGER,
            at          TEXT NOT NULL DEFAULT (datetime('now')),
            level       TEXT NOT NULL DEFAULT 'info',
            code        TEXT NOT NULL,
            message     TEXT NOT NULL
        )",
        "CREATE INDEX IF NOT EXISTS idx_residency_log_campaign ON residency_log(campaign_id, at)",
        "CREATE TABLE IF NOT EXISTS ai_usage (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            at            TEXT NOT NULL DEFAULT (datetime('now')),
            purpose       TEXT NOT NULL,
            campaign_id   INTEGER,
            model         TEXT NOT NULL,
            input_tokens  INTEGER NOT NULL DEFAULT 0,
            output_tokens INTEGER NOT NULL DEFAULT 0,
            cache_read    INTEGER NOT NULL DEFAULT 0,
            cache_write   INTEGER NOT NULL DEFAULT 0,
            cost_usd      REAL NOT NULL DEFAULT 0
        )",
    ] {
        let _ = conn.execute(stmt, []);
    }
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

pub const ASSET_TYPES: &[&str] = &["mix", "video", "photo", "epk", "stat", "link"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Asset {
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub note: String,
}

/// One email of the sequence. `offset_days` counts from the first send (J0).
/// FR and EN versions; `extra_asset` is the title of an asset appended to
/// that step only (e.g. a new video on the second follow-up).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Step {
    pub offset_days: i64,
    #[serde(default)]
    pub subject: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub subject_en: String,
    #[serde(default)]
    pub body_en: String,
    #[serde(default)]
    pub extra_asset: Option<String>,
    /// A/B test: extra subject lines (B, C) next to `subject` (A), see
    /// commands/funnel.rs. Empty = no test.
    #[serde(default)]
    pub subject_variants: Vec<String>,
    #[serde(default)]
    pub subject_variants_en: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Config {
    #[serde(default)]
    pub pitch_fr: String,
    #[serde(default)]
    pub pitch_en: String,
    /// The monthly content plan ("en développement" series).
    #[serde(default)]
    pub concept: String,
    /// "monthly" | "bimonthly"
    #[serde(default = "default_ask")]
    pub ask: String,
    #[serde(default)]
    pub assets: Vec<Asset>,
    #[serde(default)]
    pub press_kit_id: Option<String>,
    #[serde(default)]
    pub press_kit_title: Option<String>,
    #[serde(default = "default_steps")]
    pub steps: Vec<Step>,
    #[serde(default)]
    pub window: Window,
    #[serde(default = "default_daily_max")]
    pub daily_max: i64,
    #[serde(default)]
    pub zone: Zone,
    /// "auto" (per target country/city) | "fr" | "en"
    #[serde(default = "default_lang")]
    pub lang: String,
    #[serde(default = "default_cap_min")]
    pub capacity_min: i64,
    #[serde(default = "default_cap_max")]
    pub capacity_max: i64,
    #[serde(default = "default_min_events")]
    pub min_events: i64,
}

fn default_ask() -> String {
    "monthly".into()
}
fn default_daily_max() -> i64 {
    8
}
fn default_lang() -> String {
    "auto".into()
}
fn default_cap_min() -> i64 {
    150
}
fn default_cap_max() -> i64 {
    800
}
fn default_min_events() -> i64 {
    10
}

impl Default for Config {
    fn default() -> Self {
        Config {
            pitch_fr: String::new(),
            pitch_en: String::new(),
            concept: String::new(),
            ask: default_ask(),
            // PIERS FM Vol. 4 is the reference mix: every new residency
            // campaign starts with it as its lead asset.
            assets: vec![Asset {
                kind: "mix".into(),
                title: "PIERS FM Vol. 4".into(),
                url: "https://www.youtube.com/watch?v=dnURpBKDE7w".into(),
                note: "Mix de référence".into(),
            }],
            press_kit_id: None,
            press_kit_title: None,
            steps: default_steps(),
            window: Window::default(),
            daily_max: default_daily_max(),
            zone: Zone { presets: vec!["paris".into()], ..Default::default() },
            lang: default_lang(),
            capacity_min: default_cap_min(),
            capacity_max: default_cap_max(),
            min_events: default_min_events(),
        }
    }
}

/// The three-email sequence (J0, J+5, J+12). No em dash anywhere in the French.
pub fn default_steps() -> Vec<Step> {
    vec![
        Step {
            offset_days: 0,
            subject: "{{venue}} x {{artist}}, une idée de résidence".into(),
            body: "Hello,\n\n\
                   Moi c'est Thibault, je mixe sous le nom de {{artist}} (house, tech house). Je rentre d'ADE avec pas mal d'envie et un peu d'élan.\n\n\
                   Je cherche un club à {{city}} où poser une date {{rhythm}}, quelque chose qui s'installe dans le temps. \
                   Et {{venue}}, honnêtement, c'est un des premiers endroits qui me sont venus en tête.\n\n\
                   En parallèle je filme tout pour PIERS FM, ma série où je montre les morceaux en cours, les edits et les coulisses. \
                   Chaque date chez vous deviendrait un épisode, donc un rendez-vous qui se voit aussi en ligne.\n\n\
                   Tout est là si vous voulez écouter (le Vol. 4, c'est mon dernier mix) :\n{{press_link}}\n\n\
                   Ça vous dirait qu'on en parle autour d'un café, ou juste au téléphone ?\n\n\
                   À bientôt,\nThibault"
                .into(),
            subject_en: "{{venue}} x {{artist}}, a residency idea".into(),
            body_en: "Hello,\n\n\
                      I'm Thibault, I play as {{artist}} (house, tech house). Just got back from ADE with a lot of drive.\n\n\
                      I'm looking for a club in {{city}} to hold a night {{rhythm}}, something that builds over time. \
                      And honestly, {{venue}} was one of the first places that came to mind.\n\n\
                      I film everything for PIERS FM, my series where I share tracks in progress, edits and what happens behind the decks. \
                      Every night at yours would become an episode, so the night gets seen online too.\n\n\
                      Everything's here if you want a listen (Vol. 4 is my latest mix):\n{{press_link}}\n\n\
                      Up for a quick call or a coffee to talk it through?\n\n\
                      Cheers,\nThibault"
                .into(),
            extra_asset: None,
            ..Default::default()
        },
        Step {
            offset_days: 5,
            subject: "Re: {{venue}} x {{artist}}, une idée de résidence".into(),
            body: "Hello,\n\n\
                   Je remonte mon message pour {{venue}}, j'imagine que vos boîtes mail débordent en ce moment.\n\n\
                   Si ce n'est pas vous qui gérez la prog, vous pouvez me dire à qui écrire ? Ça m'aiderait beaucoup.\n\n\
                   Le mix et le reste sont toujours ici : {{press_link}}\n\n\
                   Merci,\nThibault"
                .into(),
            subject_en: "Re: {{venue}} x {{artist}}, a residency idea".into(),
            body_en: "Hello,\n\n\
                      Bumping this up about {{venue}}, I'm guessing your inbox is pretty full right now.\n\n\
                      If you're not the one handling bookings, could you tell me who to write to? That would help a lot.\n\n\
                      The mix and everything else are still here: {{press_link}}\n\n\
                      Thanks,\nThibault"
                .into(),
            extra_asset: None,
            ..Default::default()
        },
        Step {
            offset_days: 12,
            subject: "Re: {{venue}} x {{artist}}, une idée de résidence".into(),
            body: "Hello,\n\n\
                   Dernier message de ma part, promis. Si ce n'est pas le moment pour {{venue}}, aucun souci, je comprends très bien.\n\n\
                   Et si ça peut se faire plus tard dans la saison, je serais vraiment content d'en reparler.\n\n\
                   {{press_link}}\n\n\
                   Belle journée,\nThibault"
                .into(),
            subject_en: "Re: {{venue}} x {{artist}}, a residency idea".into(),
            body_en: "Hello,\n\n\
                      Last one from me, promise. If it's not the right time for {{venue}}, no worries at all.\n\n\
                      And if it could work later in the season, I'd be really happy to pick it up again.\n\n\
                      {{press_link}}\n\n\
                      Have a good one,\nThibault"
                .into(),
            extra_asset: None,
            ..Default::default()
        },
    ]
}

/// Markers of the first default templates (v0.15.0, stiff and "AI-sounding").
/// A saved campaign whose step still contains one gets the new default for that
/// step; a step the user rewrote is left alone.
const OLD_STEP_MARKERS: [&str; 3] = [
    "Je suis {{artist}}, DJ et producteur. Après ADE",
    "Je me permets de remonter mon message de la semaine dernière",
    "Dernier petit message de ma part",
];

/// One-shot per launch, cheap: refresh untouched v0.15.0 templates in saved campaigns.
pub fn refresh_default_templates(conn: &Connection) {
    let ids: Vec<i64> = match conn.prepare("SELECT campaign_id FROM residency_campaigns") {
        Ok(mut st) => st
            .query_map([], |r| r.get(0))
            .map(|it| it.flatten().collect())
            .unwrap_or_default(),
        Err(_) => return,
    };
    let fresh = default_steps();
    for id in ids {
        let Some(r) = load(conn, id) else { continue };
        let mut cfg = r.config;
        let mut changed = false;
        for (i, marker) in OLD_STEP_MARKERS.iter().enumerate() {
            if let (Some(step), Some(new)) = (cfg.steps.get_mut(i), fresh.get(i)) {
                if step.body.contains(marker) {
                    let extra = step.extra_asset.take();
                    *step = new.clone();
                    step.extra_asset = extra;
                    changed = true;
                }
            }
        }
        if changed {
            let _ = save_config(conn, id, &cfg);
        }
    }
}

/// French pitch pre-filled by "Nouvelle campagne résidence" (the user edits it).
pub fn default_pitch_fr(city: &str) -> String {
    format!(
        "Après ADE, Piers cherche une résidence mensuelle (ou tous les deux mois) dans un club de {city} \
         à taille humaine mais reconnu, entre 150 et 800 personnes.\n\n\
         Le principe : une date régulière où Piers construit la soirée avec le club, et un contenu mensuel \
         qui documente chaque édition, la série « en développement » (préparation, sélection, moments de la \
         nuit, retours du public). Le club gagne une soirée qui grandit d'un mois sur l'autre et une visibilité \
         régulière sur les réseaux de l'artiste.\n\n\
         Ce que Piers apporte : l'élan post ADE, un public qui le suit, un mix récent, des vidéos de ses sets \
         et ses chiffres d'écoute. Ce qu'il demande : une date fixe chaque mois ou tous les deux mois, et la \
         possibilité de filmer.",
        city = city
    )
}

pub fn default_pitch_en(city: &str) -> String {
    format!(
        "After ADE, Piers is looking for a monthly (or every two months) residency at a mid-size but \
         well-known club in {city}, 150 to 800 capacity.\n\n\
         The idea: a regular night that Piers builds together with the club, plus monthly content documenting \
         each edition, the \"in development\" series (prep, selection, moments of the night, crowd feedback). \
         The club gets a night that grows month after month and regular exposure on the artist's channels.\n\n\
         What Piers brings: the post-ADE momentum, a following, a recent mix, videos of his sets and his \
         streaming numbers. What he asks for: a fixed date every month or two, and permission to film.",
        city = city
    )
}

pub fn default_concept_fr() -> String {
    "Série mensuelle « en développement » : un épisode par date (avant, pendant, après), publié sur les réseaux \
     de Piers et du club, plus un mix enregistré à chaque édition."
        .into()
}

// ---------------------------------------------------------------------------
// Load / save
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct Residency {
    pub campaign_id: i64,
    pub config: Config,
    pub autopilot: bool,
    pub autopilot_confirmed_at: Option<String>,
    pub paused_reason: Option<String>,
    pub paused_kind: Option<String>,
}

pub fn load(conn: &Connection, campaign_id: i64) -> Option<Residency> {
    conn.query_row(
        "SELECT r.config, r.autopilot, r.autopilot_confirmed_at, r.paused_reason, r.paused_kind
         FROM residency_campaigns r JOIN campaigns c ON c.id = r.campaign_id
         WHERE r.campaign_id = ?1",
        params![campaign_id],
        |r| {
            let raw: String = r.get(0)?;
            Ok(Residency {
                campaign_id,
                config: serde_json::from_str(&raw).unwrap_or_default(),
                autopilot: r.get::<_, i64>(1)? != 0,
                autopilot_confirmed_at: r.get(2)?,
                paused_reason: r.get(3)?,
                paused_kind: r.get(4)?,
            })
        },
    )
    .optional()
    .ok()
    .flatten()
}

pub fn save_config(conn: &Connection, campaign_id: i64, cfg: &Config) -> Result<(), String> {
    let json = serde_json::to_string(cfg).map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO residency_campaigns (campaign_id, config) VALUES (?1, ?2)
         ON CONFLICT(campaign_id) DO UPDATE SET config = excluded.config, updated_at = datetime('now')",
        params![campaign_id, json],
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

/// Ids of residency campaigns still running (status active).
pub fn active_ids(conn: &Connection) -> Vec<i64> {
    let Ok(mut st) = conn.prepare(
        "SELECT r.campaign_id FROM residency_campaigns r JOIN campaigns c ON c.id = r.campaign_id
         WHERE c.status = 'active' AND c.kind = 'residency' ORDER BY r.campaign_id",
    ) else {
        return Vec::new();
    };
    st.query_map([], |r| r.get(0)).map(|it| it.flatten().collect()).unwrap_or_default()
}

/// Record one engine decision. The log is a ring: only the latest rows stay.
pub fn log(conn: &Connection, campaign_id: Option<i64>, level: &str, code: &str, message: &str) {
    let _ = conn.execute(
        "INSERT INTO residency_log (campaign_id, level, code, message) VALUES (?1, ?2, ?3, ?4)",
        params![campaign_id, level, code, message],
    );
    let _ = conn.execute(
        "DELETE FROM residency_log WHERE id <= (SELECT MAX(id) FROM residency_log) - 5000",
        [],
    );
}

/// Pause a campaign's autopilot with a visible reason. `hard` pauses (SMTP
/// auth error, bounce rate) stay until the user resumes; `soft` ones (SMTP not
/// configured, reply sync stale) clear by themselves once the cause is gone.
pub fn set_pause(conn: &Connection, campaign_id: i64, kind: &str, reason: &str) {
    let changed = conn
        .execute(
            "UPDATE residency_campaigns SET paused_reason = ?2, paused_kind = ?3, updated_at = datetime('now')
             WHERE campaign_id = ?1 AND (paused_reason IS NOT ?2 OR paused_kind IS NOT ?3)
               AND NOT (COALESCE(paused_kind, '') = 'hard' AND ?3 = 'soft')",
            params![campaign_id, reason, kind],
        )
        .unwrap_or(0);
    if changed > 0 {
        log(conn, Some(campaign_id), "pause", &format!("pause_{}", kind), reason);
    }
}

pub fn clear_soft_pause(conn: &Connection, campaign_id: i64) {
    let n = conn
        .execute(
            "UPDATE residency_campaigns SET paused_reason = NULL, paused_kind = NULL, updated_at = datetime('now')
             WHERE campaign_id = ?1 AND paused_kind = 'soft'",
            params![campaign_id],
        )
        .unwrap_or(0);
    if n > 0 {
        log(conn, Some(campaign_id), "info", "resume_auto", "Condition levée, pilote auto reparti.");
    }
}

#[cfg(test)]
pub(crate) mod testdb {
    //! An in-memory database with the real schema (db.rs migrations), for the
    //! residency tests.
    use rusqlite::Connection;

    pub fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        crate::db::migrate_for_tests(&c).unwrap();
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_and_roundtrip() {
        let c = testdb::conn();
        // Migration is idempotent.
        migrate(&c);
        migrate(&c);
        c.execute("INSERT INTO campaigns (name, kind) VALUES ('Résidence Paris', 'residency')", []).unwrap();
        let id = c.last_insert_rowid();
        let mut cfg = Config::default();
        assert_eq!(cfg.steps.len(), 3);
        assert_eq!(cfg.steps.iter().map(|s| s.offset_days).collect::<Vec<_>>(), vec![0, 5, 12]);
        assert_eq!(cfg.daily_max, 8);
        assert_eq!(cfg.window, Window::default());
        cfg.pitch_fr = default_pitch_fr("Paris");
        save_config(&c, id, &cfg).unwrap();
        let r = load(&c, id).unwrap();
        assert_eq!(r.config, cfg);
        assert!(!r.autopilot);
        assert_eq!(active_ids(&c), vec![id]);
        // A config saved by an older build (missing fields) still loads with defaults.
        c.execute("UPDATE residency_campaigns SET config = '{\"pitch_fr\":\"x\"}'", []).unwrap();
        let r = load(&c, id).unwrap();
        assert_eq!(r.config.pitch_fr, "x");
        assert_eq!(r.config.steps.len(), 3);
        // No em dash in the French copy.
        for s in default_steps() {
            assert!(!s.body.contains('—') && !s.subject.contains('—'));
        }
        assert!(!default_pitch_fr("Paris").contains('—'));
        // Pauses: hard is not overwritten by soft, soft clears by itself.
        set_pause(&c, id, "hard", "SMTP auth");
        set_pause(&c, id, "soft", "IMAP");
        assert_eq!(load(&c, id).unwrap().paused_reason.as_deref(), Some("SMTP auth"));
        clear_soft_pause(&c, id);
        assert_eq!(load(&c, id).unwrap().paused_kind.as_deref(), Some("hard"));
    }
}
