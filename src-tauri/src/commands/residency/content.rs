//! Emails of a residency sequence: the personalised first email (Claude, with
//! one or two real facts about the club), the follow-ups rendered from the
//! sequence templates, and the variables both rely on.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::json;
use std::collections::HashMap;

use super::claude::{self, Llm, LlmRequest, SystemBlock};
use super::insrt::{Insrt, Recipient};
use super::targeting::{target_info, TargetInfo};
use super::{Config, Step};
use crate::commands::email::render_template;

/// Placeholders left after rendering. Same rule as unresolvedVars() in
/// ComposeModal.tsx: a message with any of these is never scheduled.
pub fn unresolved(text: &str) -> Vec<String> {
    let re = regex::Regex::new(r"\{\{\s*(\w+)\s*\}\}").expect("valid regex");
    let mut out: Vec<String> = re.captures_iter(text).map(|c| c[1].to_string()).collect();
    out.sort();
    out.dedup();
    out
}

fn ask_word(ask: &str, lang: &str) -> &'static str {
    match (ask, lang) {
        ("bimonthly", "en") => "bi-monthly",
        ("bimonthly", _) => "bimestrielle",
        (_, "en") => "monthly",
        _ => "mensuelle",
    }
}

/// How often, said the way people talk ("une date tous les mois", not
/// "une date mensuelle").
fn rhythm_words(ask: &str, lang: &str) -> &'static str {
    match (ask, lang) {
        ("bimonthly", "en") => "every other month",
        ("bimonthly", _) => "tous les deux mois",
        (_, "en") => "every month",
        _ => "tous les mois",
    }
}

/// The campaign's plain asset links, one per line, when there is no personal
/// press link (insrt not configured).
pub fn assets_block(cfg: &Config) -> Option<String> {
    let lines: Vec<String> = cfg
        .assets
        .iter()
        .filter(|a| a.url.trim().starts_with("http"))
        .map(|a| {
            if a.title.trim().is_empty() {
                a.url.trim().to_string()
            } else {
                format!("{} : {}", a.title.trim(), a.url.trim())
            }
        })
        .collect();
    (!lines.is_empty()).then(|| format!("\n{}", lines.join("\n")))
}

/// Variables for one target: {{name}} {{venue}} {{city}} {{artist}} {{ask}}
/// {{press_link}} and {{first_name}} when a person is known.
pub fn vars_for(info: &TargetInfo, cfg: &Config, artist: &str, press_url: Option<&str>) -> HashMap<String, String> {
    let mut v = HashMap::new();
    v.insert(
        "name".into(),
        info.promoter.clone().filter(|p| !p.trim().is_empty()).unwrap_or_else(|| info.venue.clone()),
    );
    v.insert("venue".into(), info.venue.clone());
    if !info.city.trim().is_empty() {
        v.insert("city".into(), info.city.clone());
    }
    if !artist.trim().is_empty() {
        v.insert("artist".into(), artist.to_string());
    }
    v.insert("ask".into(), ask_word(&cfg.ask, &info.lang).to_string());
    v.insert("rhythm".into(), rhythm_words(&cfg.ask, &info.lang).to_string());
    if let Some(f) = &info.first_name {
        v.insert("first_name".into(), f.clone());
    }
    match press_url.filter(|u| !u.trim().is_empty()) {
        Some(u) => {
            v.insert("press_link".into(), u.trim().to_string());
        }
        None => {
            if let Some(b) = assets_block(cfg) {
                v.insert("press_link".into(), b);
            }
        }
    }
    v
}

/// Subject and body of a sequence step (0-based index) for a language, the
/// step's extra asset appended.
pub fn render_step(cfg: &Config, idx: usize, lang: &str, vars: &HashMap<String, String>) -> Option<(String, String)> {
    let s: &Step = cfg.steps.get(idx)?;
    let (subj, body) = if lang == "en" && !s.body_en.trim().is_empty() {
        (&s.subject_en, &s.body_en)
    } else {
        (&s.subject, &s.body)
    };
    let mut body = render_template(body, vars);
    if let Some(extra) = s.extra_asset.as_deref().filter(|e| !e.trim().is_empty()) {
        if let Some(a) = cfg.assets.iter().find(|a| a.title.trim() == extra.trim() && !a.url.trim().is_empty()) {
            let line = format!("{} : {}", a.title.trim(), a.url.trim());
            // Before the signature line when there is one.
            match body.rfind("\n\n") {
                Some(i) => body.insert_str(i, &format!("\n\n{}", line)),
                None => body.push_str(&format!("\n\n{}", line)),
            }
        }
    }
    Some((render_template(subj, vars), body))
}

// ---------------- Claude first email ----------------

/// Thibault's house voice: the "sound human" rule and the AI tells he never
/// writes. Shared by every text the agent writes under his name (first emails
/// here, reply drafts in commands/reply_desk.rs), so the banned list lives once.
macro_rules! house_voice {
    () => {
        "- Sound human: vary sentence length, use spoken turns (\"honnêtement\", \"pas mal\", \"juste\"), contractions in English. \
At most one exclamation mark, none is fine.
- Never use these (AI tells): \"Je me permets\", \"N'hésitez pas\", \"à taille humaine\", \"correspond parfaitement\", \
\"L'idée :\", \"passionné\", \"univers\", \"synergie\", \"collaboration fructueuse\", \"je serais ravi de\", \"Cordialement\", \
\"I hope this email finds you well\", \"I'm reaching out\", \"excited\", \"thrilled\", \"passionate\", \"vibrant\", \"elevate\", \
\"resonate\", \"journey\", \"unique opportunity\". No colon used to introduce an explanation, no neat three-part structure, \
no em dashes or en dashes.
"
    };
}

/// The same rules as a plain string, for prompts built outside this module.
pub const HOUSE_VOICE: &str = house_voice!();

const DRAFT_SYSTEM: &str = concat!("You write the first email from a DJ/producer to a club, proposing a residency. \
It goes out under the artist's own name to the club's booking or programming team, so it must read like \
the artist typed it himself between two studio sessions: warm, direct, a bit casual, confident without selling.

Voice (copy it, in French or English as requested):
---
Hello,

Moi c'est Thibault, je mixe sous le nom de Piers (house, tech house). Je rentre d'ADE avec pas mal d'envie et un peu d'élan.

Je cherche un club à Paris où poser une date tous les mois, quelque chose qui s'installe dans le temps. \
Et Badaboum, honnêtement, c'est un des premiers endroits qui me sont venus en tête. Voir passer DJ Seinfeld et Shonky \
chez vous cette année, ça m'a convaincu.

En parallèle je filme tout pour PIERS FM, ma série où je montre les morceaux en cours, les edits et les coulisses. \
Chaque date chez vous deviendrait un épisode, donc un rendez-vous qui se voit aussi en ligne.

Tout est là si vous voulez écouter (le Vol. 4, c'est mon dernier mix) :
https://example.link

Ça vous dirait qu'on en parle autour d'un café, ou juste au téléphone ?

À bientôt,
Thibault
---

Rules:
- 90 to 150 words in the body. Plain text, short paragraphs, no markdown, no emojis, no lists.
- Open with \"Hello,\" (or \"Hello <first name>,\" when a first name is given). The sender is Thibault, who plays as the artist: introduce himself as \"Thibault\" playing as the artist name. Close with \"À bientôt,\" / \"Cheers,\" then \"Thibault\" on its own line (the full signature with phone and website is appended automatically, never write it).
- One concrete, true detail about this club taken only from the facts provided (artists who played there, its programming), \
said the way a DJ would say it. Never invent a fact, a name or a number. If no fact is usable, skip it rather than flatter.
- Mention PIERS FM and that each date becomes an episode. Mention Vol. 4 as the latest mix.
- The press link exactly once, copied verbatim, on its own line.
- End with one simple, relaxed question.
", house_voice!(), "- Subject: under 60 characters, lowercase feel, like \"<club> x <artist>, une idée de résidence\". No clickbait.
- Write in the language requested.

Answer with JSON only: {\"subject\": \"...\", \"body\": \"...\"}.");

fn draft_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {"subject": {"type": "string"}, "body": {"type": "string"}},
        "required": ["subject", "body"],
        "additionalProperties": false
    })
}

/// The per-campaign block shared by every draft of the campaign (cached prefix).
pub fn campaign_block(cfg: &Config, artist: &str) -> String {
    let mut s = format!("Artist: {}\nAsk: {} residency\n", artist, if cfg.ask == "bimonthly" { "bi-monthly (every two months)" } else { "monthly" });
    if !cfg.pitch_fr.trim().is_empty() {
        s.push_str(&format!("\nPitch (FR):\n{}\n", cfg.pitch_fr.trim()));
    }
    if !cfg.pitch_en.trim().is_empty() {
        s.push_str(&format!("\nPitch (EN):\n{}\n", cfg.pitch_en.trim()));
    }
    if !cfg.concept.trim().is_empty() {
        s.push_str(&format!("\nContent plan:\n{}\n", cfg.concept.trim()));
    }
    let assets: Vec<String> = cfg
        .assets
        .iter()
        .map(|a| format!("- [{}] {}{}", a.kind, a.title, if a.note.trim().is_empty() { String::new() } else { format!(" ({})", a.note.trim()) }))
        .collect();
    if !assets.is_empty() {
        s.push_str(&format!("\nAssets behind the press link:\n{}\n", assets.join("\n")));
    }
    // A sample of the house style: the step-1 template.
    if let Some(st) = cfg.steps.first() {
        s.push_str(&format!("\nTone reference (French template, adapt, do not copy):\n{}\n", st.body));
    }
    s
}

fn facts_of(info: &TargetInfo) -> String {
    let mut f = vec![format!("Club: {}", info.venue)];
    if !info.city.is_empty() {
        f.push(format!("City: {} ({})", info.city, info.country));
    }
    if let Some(c) = info.capacity {
        f.push(format!("Estimated capacity: {}", c));
    }
    if info.events > 0 {
        f.push(format!("Events listed on Resident Advisor over the last period: {}", info.events));
    }
    if let Some(t) = &info.venue_type {
        f.push(format!("Type: {}", t));
    }
    if !info.artists.is_empty() {
        let a: Vec<String> = info.artists.iter().map(|(n, d)| format!("{} ({})", n, d.get(..7).unwrap_or(d))).collect();
        f.push(format!("Artists close to ours who played there: {}", a.join(", ")));
    }
    if let Some(p) = &info.top_promoter {
        f.push(format!("Main promoter there: {}", p));
    }
    if let Some(w) = &info.website {
        f.push(format!("Website: {}", w));
    }
    f.join("\n")
}

pub struct Draft {
    pub subject: String,
    pub body: String,
    pub model: String,
}

/// Ask Claude for the first email. One retry when the answer does not parse.
pub fn draft_first_email(
    llm: &dyn Llm,
    model: &str,
    cfg: &Config,
    info: &TargetInfo,
    artist: &str,
    press_link: &str,
    usage_sink: &mut dyn FnMut(&str, &claude::Usage),
) -> Result<Draft, String> {
    let lang_name = if info.lang == "en" { "English" } else { "French" };
    let user = format!(
        "Language: {}\nContact first name: {}\nPress link to include: {}\n\nFacts about the club:\n{}",
        lang_name,
        info.first_name.clone().unwrap_or_else(|| "unknown".into()),
        press_link,
        facts_of(info)
    );
    let req = LlmRequest {
        model: model.to_string(),
        max_tokens: 400,
        system: vec![
            SystemBlock { text: DRAFT_SYSTEM.into(), cache: false },
            SystemBlock { text: campaign_block(cfg, artist), cache: true },
        ],
        user,
        schema: Some(draft_schema()),
    };
    let mut last = String::new();
    for _ in 0..2 {
        let resp = llm.complete(&req)?;
        usage_sink(&resp.model, &resp.usage);
        match parse_draft(&resp.text) {
            Some((subject, mut body)) => {
                if !press_link.trim().is_empty() && !body.contains(press_link.trim()) {
                    body.push_str(&format!("\n\n{}", press_link.trim()));
                }
                return Ok(Draft { subject, body, model: if resp.model.is_empty() { model.to_string() } else { resp.model } });
            }
            None => {
                last = format!("réponse inattendue ({}): {}", resp.stop_reason, resp.text.chars().take(120).collect::<String>())
            }
        }
    }
    Err(last)
}

pub fn parse_draft(text: &str) -> Option<(String, String)> {
    let v = claude::extract_json(text)?;
    let subject = v.get("subject")?.as_str()?.trim().to_string();
    let body = v.get("body")?.as_str()?.trim().to_string();
    (!subject.is_empty() && !body.is_empty()).then(|| (claude::no_dashes(&subject), claude::no_dashes(&body)))
}

// ---------------- Generating the drafts of a campaign ----------------

#[derive(Debug, Serialize, Default)]
pub struct GenResult {
    pub created: usize,
    pub with_ai: usize,
    pub press_links: usize,
    pub failed: usize,
    pub errors: Vec<String>,
}

pub fn artist_name(conn: &Connection, campaign_id: i64) -> String {
    conn.query_row(
        "SELECT a.name FROM campaigns c JOIN artists a ON a.id = c.artist_id WHERE c.id = ?1",
        params![campaign_id],
        |r| r.get::<_, String>(0),
    )
    .ok()
    .or_else(|| crate::commands::email::sole_artist(conn))
    .unwrap_or_default()
}

/// The contact's personal press link: the one already stored on a message of
/// this campaign, else created through the insrt API (idempotent per contact).
fn press_link_for(
    conn: &Connection,
    campaign_id: i64,
    cfg: &Config,
    info: &TargetInfo,
    insrt: Option<&dyn Insrt>,
) -> Result<Option<String>, String> {
    let existing: Option<String> = conn
        .query_row(
            "SELECT press_url FROM campaign_messages WHERE campaign_id = ?1 AND contact_id = ?2
               AND press_url IS NOT NULL AND press_url <> '' ORDER BY step LIMIT 1",
            params![campaign_id, info.contact_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    if existing.is_some() {
        return Ok(existing);
    }
    let (Some(client), Some(kit)) = (insrt, cfg.press_kit_id.as_deref().filter(|k| !k.trim().is_empty())) else {
        return Ok(None);
    };
    let recipient = Recipient {
        name: info.first_name.clone().or_else(|| info.promoter.clone()).unwrap_or_else(|| info.venue.clone()),
        email: info.email.clone(),
        organisation: info.venue.clone(),
    };
    client.press_link(kit, info.contact_id, &recipient, &info.lang).map(|l| Some(l.url))
}

/// Step-1 drafts for the audience: every planned contact without one (or the
/// given contacts, regenerated). Sent messages are never touched.
pub fn generate(
    conn: &Connection,
    campaign_id: i64,
    cfg: &Config,
    only: Option<&[i64]>,
    regenerate: bool,
    llm: Option<&dyn Llm>,
    insrt: Option<&dyn Insrt>,
) -> Result<GenResult, String> {
    let ids: Vec<i64> = match only {
        Some(ids) => ids.to_vec(),
        None => {
            let mut st = conn
                .prepare(
                    "SELECT cc.contact_id FROM campaign_contacts cc
                     WHERE cc.campaign_id = ?1 AND cc.state = 'planned'
                       AND NOT EXISTS (SELECT 1 FROM campaign_messages m
                                       WHERE m.campaign_id = cc.campaign_id AND m.contact_id = cc.contact_id AND m.step = 1)
                     ORDER BY cc.id",
                )
                .map_err(|e| e.to_string())?;
            let it = st.query_map(params![campaign_id], |r| r.get(0)).map_err(|e| e.to_string())?;
            it.flatten().collect()
        }
    };
    let artist = artist_name(conn, campaign_id);
    let model = claude::drafting_model(conn);
    let mut res = GenResult::default();
    for cid in ids {
        let status: Option<String> = conn
            .query_row(
                "SELECT status FROM campaign_messages WHERE campaign_id = ?1 AND contact_id = ?2 AND step = 1",
                params![campaign_id, cid],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if matches!(status.as_deref(), Some("sent")) || (status.is_some() && !regenerate) {
            continue;
        }
        let Some(info) = target_info(conn, cid, cfg) else { continue };
        let press = match press_link_for(conn, campaign_id, cfg, &info, insrt) {
            Ok(p) => p,
            Err(e) => {
                res.errors.push(format!("{}: {}", info.venue, e));
                None
            }
        };
        if press.is_some() {
            res.press_links += 1;
        }
        let vars = vars_for(&info, cfg, &artist, press.as_deref());
        let link_text = vars.get("press_link").cloned().unwrap_or_default();
        let mut note: Option<String> = None;
        let mut generated_by = "template".to_string();
        let ai = match llm {
            Some(l) if !link_text.is_empty() => {
                let mut sink = |m: &str, u: &claude::Usage| claude::log_usage(conn, "draft", Some(campaign_id), m, u);
                match draft_first_email(l, &model, cfg, &info, &artist, link_text.trim(), &mut sink) {
                    Ok(d) => Some(d),
                    Err(e) => {
                        note = Some(format!("IA indisponible, modèle utilisé : {}", e));
                        res.errors.push(format!("{}: {}", info.venue, e));
                        None
                    }
                }
            }
            _ => None,
        };
        let (subject, body) = match ai {
            Some(d) => {
                res.with_ai += 1;
                generated_by = d.model;
                // A placeholder the model kept ({{artist}}) resolves like a template.
                (render_template(&d.subject, &vars), render_template(&d.body, &vars))
            }
            None => match render_step(cfg, 0, &info.lang, &vars) {
                Some(x) => x,
                None => {
                    res.failed += 1;
                    continue;
                }
            },
        };
        let missing = unresolved(&format!("{}\n{}", subject, body));
        if !missing.is_empty() {
            note = Some(format!("Variables à compléter : {}", missing.join(", ")));
        }
        conn.execute(
            "INSERT INTO campaign_messages (campaign_id, contact_id, step, subject, body, lang, status, press_url, note, generated_by)
             VALUES (?1, ?2, 1, ?3, ?4, ?5, 'draft', ?6, ?7, ?8)
             ON CONFLICT(campaign_id, contact_id, step) DO UPDATE SET
                subject = excluded.subject, body = excluded.body, lang = excluded.lang, status = 'draft',
                scheduled_for = NULL, press_url = COALESCE(excluded.press_url, campaign_messages.press_url),
                note = excluded.note, generated_by = excluded.generated_by, updated_at = datetime('now')
             WHERE campaign_messages.status <> 'sent'",
            params![campaign_id, cid, subject, body, info.lang, press, note, generated_by],
        )
        .map_err(|e| e.to_string())?;
        res.created += 1;
    }
    Ok(res)
}

/// The follow-up after step `sent_step` (1-based), rendered from the templates
/// and scheduled at J0 + offset. Returns the new message id. A template whose
/// variables do not all resolve is stored as a draft (never scheduled).
pub fn create_next_step(
    conn: &Connection,
    campaign_id: i64,
    cfg: &Config,
    contact_id: i64,
    sent_step: i64,
    j0: chrono::DateTime<chrono::Utc>,
) -> Option<i64> {
    let idx = sent_step as usize; // 0-based index of the next step
    let step = cfg.steps.get(idx)?;
    let info = target_info(conn, contact_id, cfg)?;
    let press: Option<String> = conn
        .query_row(
            "SELECT press_url FROM campaign_messages WHERE campaign_id = ?1 AND contact_id = ?2
               AND press_url IS NOT NULL ORDER BY step LIMIT 1",
            params![campaign_id, contact_id],
            |r| r.get(0),
        )
        .optional()
        .ok()
        .flatten();
    let lang: String = conn
        .query_row(
            "SELECT lang FROM campaign_messages WHERE campaign_id = ?1 AND contact_id = ?2 AND step = 1",
            params![campaign_id, contact_id],
            |r| r.get(0),
        )
        .unwrap_or_else(|_| info.lang.clone());
    let artist = artist_name(conn, campaign_id);
    let mut info = info;
    info.lang = lang.clone();
    let vars = vars_for(&info, cfg, &artist, press.as_deref());
    let (subject, body) = render_step(cfg, idx, &lang, &vars)?;
    let missing = unresolved(&format!("{}\n{}", subject, body));
    let (status, note) = if missing.is_empty() {
        ("scheduled", None)
    } else {
        ("draft", Some(format!("Variables à compléter : {}", missing.join(", "))))
    };
    let when = (j0 + chrono::Duration::days(step.offset_days)).format("%Y-%m-%d %H:%M:%S").to_string();
    conn.execute(
        "INSERT OR IGNORE INTO campaign_messages
            (campaign_id, contact_id, step, subject, body, lang, status, scheduled_for, press_url, note, generated_by)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'template')",
        params![campaign_id, contact_id, sent_step + 1, subject, body, lang, status, when, press, note],
    )
    .ok()
    .filter(|n| *n > 0)
    .map(|_| conn.last_insert_rowid())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::residency::claude::fake::FakeLlm;
    use crate::commands::residency::insrt::{ActivityPage, PressKit, PressLink};
    use crate::commands::residency::{testdb, Asset};

    struct FakeInsrt;
    impl Insrt for FakeInsrt {
        fn press_kits(&self, _: &str) -> Result<Vec<PressKit>, String> {
            Ok(vec![])
        }
        fn press_link(&self, kit: &str, cid: i64, _: &Recipient, locale: &str) -> Result<PressLink, String> {
            Ok(PressLink { link_id: format!("ln{}", cid), token: None, url: format!("https://insrt.studio/p/{}-{}-{}", kit, cid, locale) })
        }
        fn activity(&self, _: &str, _: &str) -> Result<ActivityPage, String> {
            Ok(ActivityPage { events: vec![], next: None })
        }
    }

    fn setup() -> (Connection, Config) {
        let c = testdb::conn();
        c.execute_batch(
            "INSERT INTO artists (id, name) VALUES (1, 'Piers');
             INSERT INTO campaigns (id, name, kind, artist_id) VALUES (1, 'R', 'residency', 1);
             INSERT INTO contacts (id, category, name, venue, area, email, status) VALUES
                (10, 'venue', 'Club A', 'Club A', 'Paris', 'booking@cluba.fr', 'to_contact');
             INSERT INTO campaign_contacts (campaign_id, contact_id, state) VALUES (1, 10, 'planned');",
        )
        .unwrap();
        (c, Config::default())
    }

    #[test]
    fn templates_and_unresolved_vars() {
        let (c, mut cfg) = setup();
        cfg.assets.clear();
        let info = target_info(&c, 10, &cfg).unwrap();
        // No press link, no assets: {{press_link}} stays unresolved.
        let vars = vars_for(&info, &cfg, "Piers", None);
        let (_, body) = render_step(&cfg, 0, "fr", &vars).unwrap();
        assert_eq!(unresolved(&body), vec!["press_link".to_string()]);
        cfg.assets.push(Asset { kind: "mix".into(), title: "Mix ADE".into(), url: "https://sc.com/mix".into(), note: String::new() });
        cfg.assets.push(Asset { kind: "video".into(), title: "Live".into(), url: "https://yt.com/v".into(), note: String::new() });
        cfg.steps[1].extra_asset = Some("Live".into());
        let vars = vars_for(&info, &cfg, "Piers", None);
        let (subj, body) = render_step(&cfg, 0, "fr", &vars).unwrap();
        assert!(unresolved(&body).is_empty(), "{}", body);
        assert!(body.contains("Mix ADE : https://sc.com/mix") && subj.contains("Club A") && body.contains("tous les mois"));
        let (_, b2) = render_step(&cfg, 1, "en", &vars).unwrap();
        assert!(b2.starts_with("Hello,") && b2.contains("Live : https://yt.com/v"));
        assert!(!b2.contains('—'));
        // First name only when known.
        assert!(render_template("Bonjour {{first_name}}", &vars).contains("{{first_name}}"));
    }

    #[test]
    fn generate_with_fake_claude_and_insrt() {
        let (c, mut cfg) = setup();
        cfg.press_kit_id = Some("pk1".into());
        let llm = FakeLlm::new(&[
            "not json at all",
            r#"{"subject":"Résidence au Club A","body":"Bonjour,\n\nJ'ai vu passer vos soirées — superbe.\n\n{{artist}}"}"#,
        ]);
        let r = generate(&c, 1, &cfg, None, false, Some(&llm), Some(&FakeInsrt)).unwrap();
        assert_eq!((r.created, r.with_ai, r.press_links), (1, 1, 1));
        let (body, press, status, by): (String, String, String, String) = c
            .query_row("SELECT body, press_url, status, generated_by FROM campaign_messages WHERE step = 1", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })
            .unwrap();
        // Dashes removed, press link appended since the model forgot it.
        assert!(!body.contains('—'));
        assert!(body.ends_with("https://insrt.studio/p/pk1-10-fr"));
        assert_eq!(press, "https://insrt.studio/p/pk1-10-fr");
        assert_eq!(status, "draft");
        assert_eq!(by, "claude-haiku-5-5");
        // Two calls (one retry), both logged with their tokens.
        let calls: i64 = c.query_row("SELECT COUNT(*) FROM ai_usage", [], |r| r.get(0)).unwrap();
        assert_eq!(calls, 2);
        let seen = llm.seen.lock().unwrap();
        assert!(seen[0].system[1].cache && seen[0].system[1].text.contains("Artist: Piers"));
        assert_eq!(seen[0].max_tokens, 400);
        drop(seen);
        // A second run does nothing (draft exists), regenerate rewrites it.
        assert_eq!(generate(&c, 1, &cfg, None, false, None, None).unwrap().created, 0);
        let r = generate(&c, 1, &cfg, Some(&[10]), true, None, None).unwrap();
        assert_eq!(r.created, 1);
        let body: String = c.query_row("SELECT body FROM campaign_messages WHERE step = 1", [], |r| r.get(0)).unwrap();
        assert!(body.contains("https://insrt.studio/p/pk1-10-fr"), "{}", body);
    }
}
