//! "Bibliothèque de l'agent": what the agent may rely on when it writes under
//! Thibault's name. Every email stays in his voice and is signed Thibault; the
//! agent only has an internal name shown inside the app (setting `agent_name`,
//! "Ravel" by default), never in an email.
//!
//! - Artist facts: short / long bio FR+EN, key numbers, mixes and links
//!   (prefilled once from the artist record), plus the clubs he really played
//!   (Agenda, commands/routing.rs "a joué à"), read live.
//! - Validated answers by topic: fee and conditions, rider, travel, availability
//!   policy, formats, payment terms, promo assets, press quotes, call slots,
//!   holding answers, light follow-ups.
//! - Objections with Thibault's preferred answer.
//!
//! Each entry: kind, topic, title, FR text, EN text, status draft | approved.
//! Seeds are written once as drafts (good defaults in his tone): only what he
//! approves reaches a prompt. The approved entries go into the Claude prompts
//! of the reply desk and of the residency first email as one stable system
//! block (fixed ordering, no timestamps) placed before the cache breakpoint,
//! so the prompt cache keeps hitting; each draft records the entries it used.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::AppState;

pub const DEFAULT_AGENT_NAME: &str = "Ravel";
/// Bumped when new seeds are added: each seed is written once, so an entry
/// Thibault deleted never comes back.
const SEED_FLAG: &str = "playbook_seeded_v1";

pub const KINDS: [&str; 3] = ["fact", "answer", "objection"];

/// Topics per kind, in display (and prompt) order.
pub const TOPICS: &[(&str, &str)] = &[
    ("fact", "bio_short"),
    ("fact", "bio_long"),
    ("fact", "key_numbers"),
    ("fact", "mixes_links"),
    ("fact", "other_fact"),
    ("answer", "fee"),
    ("answer", "rider"),
    ("answer", "travel"),
    ("answer", "availability"),
    ("answer", "formats"),
    ("answer", "payment"),
    ("answer", "promo_assets"),
    ("answer", "press_quotes"),
    ("answer", "call"),
    ("answer", "holding"),
    ("answer", "follow_up"),
    ("answer", "other_answer"),
    ("objection", "obj_unknown"),
    ("objection", "obj_budget"),
    ("objection", "obj_full"),
    ("objection", "obj_send_mix"),
    ("objection", "obj_locals_only"),
    ("objection", "obj_other"),
];

/// English label of a topic, for the prompt.
pub fn topic_label(topic: &str) -> &'static str {
    match topic {
        "bio_short" => "Short bio",
        "bio_long" => "Long bio",
        "key_numbers" => "Key numbers",
        "mixes_links" => "Mixes and links",
        "fee" => "Fee and conditions",
        "rider" => "Technical rider",
        "travel" => "Travel and hotel",
        "availability" => "Availability policy",
        "formats" => "Formats (warm-up, closing, all night, residency)",
        "payment" => "Payment terms",
        "promo_assets" => "Promo assets",
        "press_quotes" => "Press quotes",
        "call" => "Call slots",
        "holding" => "Holding answer (out of office, they will come back)",
        "follow_up" => "Light follow-up",
        "obj_unknown" => "Objection: they do not know the artist",
        "obj_budget" => "Objection: tight budget",
        "obj_full" => "Objection: fully booked this quarter",
        "obj_send_mix" => "Objection: send us a mix",
        "obj_locals_only" => "Objection: local artists only",
        "obj_other" => "Objection",
        "other_fact" => "Fact",
        _ => "Answer",
    }
}

fn kind_rank(kind: &str) -> usize {
    KINDS.iter().position(|k| *k == kind).unwrap_or(KINDS.len())
}

fn topic_rank(topic: &str) -> usize {
    TOPICS.iter().position(|(_, t)| *t == topic).unwrap_or(TOPICS.len())
}

pub fn migrate(conn: &Connection) {
    for stmt in [
        "CREATE TABLE IF NOT EXISTS playbook_entries (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            kind        TEXT NOT NULL,
            topic       TEXT NOT NULL,
            title       TEXT NOT NULL DEFAULT '',
            text_fr     TEXT NOT NULL DEFAULT '',
            text_en     TEXT NOT NULL DEFAULT '',
            status      TEXT NOT NULL DEFAULT 'draft',
            seed_key    TEXT UNIQUE,
            position    INTEGER NOT NULL DEFAULT 0,
            created_at  TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
        )",
        "CREATE INDEX IF NOT EXISTS idx_playbook_status ON playbook_entries(status)",
        // Which entries a draft relied on (JSON array of ids).
        "ALTER TABLE reply_desk ADD COLUMN playbook_ids TEXT",
        "ALTER TABLE campaign_messages ADD COLUMN playbook_ids TEXT",
    ] {
        let _ = conn.execute(stmt, []);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Entry {
    #[serde(default)]
    pub id: i64,
    pub kind: String,
    pub topic: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub text_fr: String,
    #[serde(default)]
    pub text_en: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub seed_key: Option<String>,
    #[serde(default)]
    pub position: i64,
    #[serde(default)]
    pub updated_at: String,
}

impl Entry {
    pub fn approved(&self) -> bool {
        self.status == "approved"
    }
    fn has_text(&self) -> bool {
        !self.text_fr.trim().is_empty() || !self.text_en.trim().is_empty()
    }
}

const COLS: &str = "id, kind, topic, title, text_fr, text_en, status, seed_key, position, updated_at";

fn row(r: &rusqlite::Row) -> rusqlite::Result<Entry> {
    Ok(Entry {
        id: r.get(0)?,
        kind: r.get(1)?,
        topic: r.get(2)?,
        title: r.get(3)?,
        text_fr: r.get(4)?,
        text_en: r.get(5)?,
        status: r.get(6)?,
        seed_key: r.get(7)?,
        position: r.get(8)?,
        updated_at: r.get(9)?,
    })
}

/// Stable order: kind, topic order, position, id. The prompt block follows it,
/// so the cached prefix only changes when an approved entry does.
fn sort(entries: &mut [Entry]) {
    entries.sort_by(|a, b| {
        (kind_rank(&a.kind), topic_rank(&a.topic), a.position, a.id).cmp(&(kind_rank(&b.kind), topic_rank(&b.topic), b.position, b.id))
    });
}

pub fn list(conn: &Connection) -> Vec<Entry> {
    let mut out: Vec<Entry> = conn
        .prepare(&format!("SELECT {} FROM playbook_entries", COLS))
        .and_then(|mut st| st.query_map([], row).map(|it| it.flatten().collect()))
        .unwrap_or_default();
    sort(&mut out);
    out
}

/// Only these reach a prompt.
pub fn approved(conn: &Connection) -> Vec<Entry> {
    list(conn).into_iter().filter(|e| e.approved() && e.has_text()).collect()
}

// ---------------- Seeds ----------------

struct Seed {
    kind: &'static str,
    topic: &'static str,
    title: &'static str,
    fr: &'static str,
    en: &'static str,
}

/// Defaults in Thibault's voice, written as drafts. No number, no gear and no
/// place he did not give: the facts come from the artist record.
const SEEDS: &[Seed] = &[
    Seed {
        kind: "answer",
        topic: "fee",
        title: "",
        fr: "Le cachet dépend du format (warm-up, closing ou all night) et de la jauge. Dès que j'ai la date et le format en tête, je vous fais une proposition claire, et quand le projet nous plaît vraiment on trouve toujours un terrain d'entente.",
        en: "My fee depends on the format (warm-up, closing or all night) and on the size of the room. Once I know the date and the format I'll send you a clear proposal, and when the project really fits we always find common ground.",
    },
    Seed {
        kind: "answer",
        topic: "rider",
        title: "",
        fr: "Côté technique je m'adapte à la plupart des cabines club. Je vous envoie la fiche technique complète avec la confirmation, et on vérifie ensemble ce qui manque s'il manque quelque chose.",
        en: "On the tech side I adapt to most club booths. I'll send the full rider with the confirmation and we can check together if anything is missing.",
    },
    Seed {
        kind: "answer",
        topic: "travel",
        title: "",
        fr: "Quand je viens de loin, on prévoit le train ou le vol et une nuit d'hôtel pas trop loin du club. Rien de compliqué, on cale ça ensemble une fois la date posée.",
        en: "When I travel, we plan the train or flight and one night in a hotel not too far from the club. Nothing complicated, we sort it out together once the date is set.",
    },
    Seed {
        kind: "answer",
        topic: "availability",
        title: "",
        fr: "Je regarde les dates au cas par cas. Je bloque un soir seulement une fois qu'on est d'accord sur le format, et je vous le confirme par écrit dans la journée.",
        en: "I look at dates case by case. I only block a night once we agree on the format, and I confirm it in writing the same day.",
    },
    Seed {
        kind: "answer",
        topic: "formats",
        title: "",
        fr: "Je joue en warm-up, en closing ou en all night. Le bon format, c'est celui qui sert la soirée, je m'adapte à votre programmation. Pour une résidence, une date par mois laisse le temps de construire quelque chose.",
        en: "I play warm-up, closing or all night sets. The right format is the one that serves the night, I fit into your programming. For a residency, one night a month leaves time to build something.",
    },
    Seed {
        kind: "answer",
        topic: "payment",
        title: "",
        fr: "En général un acompte à la signature et le solde sur facture après la soirée. Si vous avez vos habitudes, on s'aligne.",
        en: "Usually a deposit when we sign and the balance on invoice after the night. If you have your own process, happy to follow it.",
    },
    Seed {
        kind: "answer",
        topic: "promo_assets",
        title: "",
        fr: "Dès que la date est calée je vous envoie photos HD, logo, bio courte et longue et un extrait vidéo pour les réseaux. Tout est aussi sur mon lien presse.",
        en: "As soon as the date is set I'll send HD photos, logo, short and long bio and a video clip for socials. Everything is also on my press link.",
    },
    Seed {
        kind: "answer",
        topic: "call",
        title: "",
        fr: "Le plus simple, c'est un appel rapide. Je suis souvent dispo en semaine en fin de matinée ou en fin de journée, dites-moi ce qui vous arrange.",
        en: "The easiest is a quick call. I'm usually free on weekdays late morning or late afternoon, just tell me what works for you.",
    },
    Seed {
        kind: "answer",
        topic: "holding",
        title: "",
        fr: "Merci pour le retour, pas de souci. Je reste dispo de mon côté, on en reparle quand vous avez vu ça en interne.",
        en: "Thanks for getting back to me, no rush at all. I'm around whenever you've had a chance to talk it over.",
    },
    Seed {
        kind: "answer",
        topic: "follow_up",
        title: "",
        fr: "Je reviens vers vous sans insister, juste pour savoir si vous avez pu en parler. Une seule question simple, pour que répondre prenne deux secondes.",
        en: "Just checking in without pushing, to see if you had a chance to talk it over. One simple question, so answering takes two seconds.",
    },
    Seed {
        kind: "objection",
        topic: "obj_unknown",
        title: "On ne vous connaît pas",
        fr: "C'est normal, je commence à jouer en dehors de mon cercle. Le plus parlant, c'est d'écouter le dernier mix. Et si ça vous parle, on peut commencer par un warm-up, sans risque pour vous.",
        en: "Fair enough, I'm starting to play outside my own circle. The best way in is the latest mix. If it speaks to you, we could start with a warm-up, no risk on your side.",
    },
    Seed {
        kind: "objection",
        topic: "obj_budget",
        title: "Budget serré",
        fr: "Je comprends, c'est le cas de beaucoup de clubs en ce moment. Si le projet me plaît, on trouve une formule, un set plus court ou un soir de semaine. Dites-moi ce qui est possible de votre côté.",
        en: "I get it, a lot of clubs are in the same spot right now. If the project fits, we'll find a way, a shorter set or a weeknight. Tell me what works on your side.",
    },
    Seed {
        kind: "objection",
        topic: "obj_full",
        title: "Complet ce trimestre",
        fr: "Pas de souci, je préfère qu'on se cale bien plutôt que vite. Je reviens vers vous pour la saison suivante, et si une place se libère d'ici là je suis preneur.",
        en: "No problem, I'd rather get it right than rush it. I'll come back to you for the next season, and if a slot opens up in the meantime I'm in.",
    },
    Seed {
        kind: "objection",
        topic: "obj_send_mix",
        title: "Envoyez-nous un mix",
        fr: "Avec plaisir, voilà le dernier, c'est celui qui ressemble le plus à ce que je joue en club en ce moment.",
        en: "Sure, here's the latest one, it's the closest to what I play in clubs right now.",
    },
    Seed {
        kind: "objection",
        topic: "obj_locals_only",
        title: "On ne programme que des locaux",
        fr: "Je comprends tout à fait, c'est une vraie ligne. Si un jour vous ouvrez une soirée à des artistes de passage, gardez-moi en tête. En attendant je passerai volontiers découvrir le club.",
        en: "Totally get it, that's a real line. If you ever open a night to visiting artists, keep me in mind. In the meantime I'd happily come by to discover the club.",
    },
];

/// The artist record fields the fact seeds start from.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ArtistFacts {
    pub id: Option<i64>,
    pub name: String,
    pub tagline: Option<String>,
    pub bio: Option<String>,
    pub genres: Option<String>,
    pub city: Option<String>,
    pub fee_range: Option<String>,
    pub tech_rider: Option<String>,
    pub mix_url: Option<String>,
    pub soundcloud: Option<String>,
    pub links: Option<String>,
    pub stats: Option<String>,
    pub achievements: Option<String>,
    pub press_quotes: Option<String>,
    /// "Badaboum, Paris (mars 2026)", from the Agenda.
    pub played_at: Vec<String>,
}

fn clean(s: &Option<String>) -> Option<String> {
    s.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(String::from)
}

pub fn artist_facts(conn: &Connection) -> ArtistFacts {
    let mut f = conn
        .query_row(
            "SELECT id, name, tagline, bio, genres, city, fee_range, tech_rider, mix_url, soundcloud, links, stats, achievements, press_quotes
             FROM artists ORDER BY id LIMIT 1",
            [],
            |r| {
                Ok(ArtistFacts {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    tagline: r.get(2)?,
                    bio: r.get(3)?,
                    genres: r.get(4)?,
                    city: r.get(5)?,
                    fee_range: r.get(6)?,
                    tech_rider: r.get(7)?,
                    mix_url: r.get(8)?,
                    soundcloud: r.get(9)?,
                    links: r.get(10)?,
                    stats: r.get(11)?,
                    achievements: r.get(12)?,
                    press_quotes: r.get(13)?,
                    played_at: vec![],
                })
            },
        )
        .optional()
        .ok()
        .flatten()
        .unwrap_or_default();
    f.played_at = played_lines(conn, f.id, "fr");
    f
}

fn played_lines(conn: &Connection, artist_id: Option<i64>, lang: &str) -> Vec<String> {
    crate::commands::routing::played_at(conn, artist_id, chrono::Local::now().date_naive())
        .unwrap_or_default()
        .iter()
        .take(10)
        .map(|p| p.fact(lang))
        .collect()
}

/// Write the seeds once (drafts). Fact seeds take what the artist record
/// already holds; an empty one waits for Thibault to fill it.
pub fn ensure_seeds(conn: &Connection) {
    let done: Option<String> =
        conn.query_row("SELECT value FROM settings WHERE key = ?1", params![SEED_FLAG], |r| r.get(0)).optional().ok().flatten();
    if done.is_some() {
        return;
    }
    let a = artist_facts(conn);
    let short = clean(&a.tagline).unwrap_or_else(|| {
        let mut s = a.name.trim().to_string();
        if let Some(g) = clean(&a.genres) {
            s.push_str(&format!(", DJ et producteur ({})", g));
        }
        if let Some(c) = clean(&a.city) {
            s.push_str(&format!(", basé à {}", c));
        }
        if s.is_empty() { s } else { format!("{}.", s) }
    });
    let numbers = [clean(&a.stats), clean(&a.achievements)].into_iter().flatten().collect::<Vec<_>>().join("\n");
    let links = [clean(&a.mix_url), clean(&a.soundcloud), clean(&a.links)].into_iter().flatten().collect::<Vec<_>>().join("\n");
    let facts: [(&str, String); 4] = [
        ("bio_short", short),
        ("bio_long", clean(&a.bio).unwrap_or_default()),
        ("key_numbers", numbers),
        ("mixes_links", links),
    ];
    let mut pos = 0;
    for (topic, fr) in facts {
        let _ = conn.execute(
            "INSERT OR IGNORE INTO playbook_entries (kind, topic, text_fr, status, seed_key, position) VALUES ('fact', ?1, ?2, 'draft', ?1, ?3)",
            params![topic, fr, pos],
        );
        pos += 1;
    }
    let quotes = clean(&a.press_quotes).unwrap_or_default();
    let _ = conn.execute(
        "INSERT OR IGNORE INTO playbook_entries (kind, topic, text_fr, status, seed_key, position) VALUES ('answer', 'press_quotes', ?1, 'draft', 'press_quotes', 0)",
        params![quotes],
    );
    for s in SEEDS {
        let _ = conn.execute(
            "INSERT OR IGNORE INTO playbook_entries (kind, topic, title, text_fr, text_en, status, seed_key, position)
             VALUES (?1, ?2, ?3, ?4, ?5, 'draft', ?2, 0)",
            params![s.kind, s.topic, s.title, s.fr, s.en],
        );
    }
    let _ = conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, '1') ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![SEED_FLAG],
    );
}

// ---------------- Editing ----------------

pub fn save(conn: &Connection, e: &Entry) -> Result<i64, String> {
    if !KINDS.contains(&e.kind.as_str()) {
        return Err("Type d'entrée inconnu.".into());
    }
    if !TOPICS.iter().any(|(k, t)| *k == e.kind && *t == e.topic) {
        return Err("Sujet inconnu.".into());
    }
    let status = if e.status == "approved" { "approved" } else { "draft" };
    if status == "approved" && !e.has_text() {
        return Err("Une entrée vide ne peut pas être validée.".into());
    }
    let fr = crate::commands::residency::claude::no_dashes(e.text_fr.trim());
    let en = crate::commands::residency::claude::no_dashes(e.text_en.trim());
    if e.id > 0 {
        let n = conn
            .execute(
                "UPDATE playbook_entries SET kind = ?2, topic = ?3, title = ?4, text_fr = ?5, text_en = ?6, status = ?7,
                        updated_at = datetime('now') WHERE id = ?1",
                params![e.id, e.kind, e.topic, e.title.trim(), fr, en, status],
            )
            .map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("Entrée introuvable.".into());
        }
        Ok(e.id)
    } else {
        let pos: i64 = conn
            .query_row("SELECT COALESCE(MAX(position), 0) + 1 FROM playbook_entries WHERE topic = ?1", params![e.topic], |r| r.get(0))
            .unwrap_or(1);
        conn.execute(
            "INSERT INTO playbook_entries (kind, topic, title, text_fr, text_en, status, position) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![e.kind, e.topic, e.title.trim(), fr, en, status, pos],
        )
        .map_err(|e| e.to_string())?;
        Ok(conn.last_insert_rowid())
    }
}

pub fn set_status(conn: &Connection, id: i64, status: &str) -> Result<(), String> {
    let st = if status == "approved" { "approved" } else { "draft" };
    let e = conn
        .query_row(&format!("SELECT {} FROM playbook_entries WHERE id = ?1", COLS), params![id], row)
        .map_err(|_| "Entrée introuvable.".to_string())?;
    if st == "approved" && !e.has_text() {
        return Err("Une entrée vide ne peut pas être validée.".into());
    }
    conn.execute("UPDATE playbook_entries SET status = ?2, updated_at = datetime('now') WHERE id = ?1", params![id, st])
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ---------------- Prompt context ----------------

/// The block injected in the prompts. Pure: entries (already filtered to the
/// approved ones and sorted) + the clubs played. Both languages are given so
/// the text does not depend on the thread language (one cached prefix).
pub fn build_block(entries: &[Entry], played: &[String]) -> String {
    let mut s = String::from(
        "PLAYBOOK. Thibault approved every entry below. Treat it as the source of truth for facts, conditions and \
his preferred answers: keep their substance, adapt the wording to the thread, translate when needed, never contradict \
them and never add a condition they do not state. Each entry has an id like [P12]; when the answer format has a \
playbook_used field, list there the ids you relied on.\n",
    );
    let sections = [
        ("fact", "Artist facts"),
        ("answer", "Validated answers"),
        ("objection", "Objections (their objection, then Thibault's preferred answer)"),
    ];
    for (kind, head) in sections {
        let items: Vec<&Entry> = entries.iter().filter(|e| e.kind == kind).collect();
        if items.is_empty() {
            continue;
        }
        s.push_str(&format!("\n{}:\n", head));
        for e in items {
            let title = if e.title.trim().is_empty() { topic_label(&e.topic).to_string() } else { format!("{} ({})", topic_label(&e.topic), e.title.trim()) };
            s.push_str(&format!("[P{}] {}\n", e.id, title));
            if !e.text_fr.trim().is_empty() {
                s.push_str(&format!("FR: {}\n", e.text_fr.trim()));
            }
            if !e.text_en.trim().is_empty() {
                s.push_str(&format!("EN: {}\n", e.text_en.trim()));
            }
        }
    }
    if !played.is_empty() {
        s.push_str("\nClubs the artist really played (Agenda), may be cited:\n");
        for p in played {
            s.push_str(&format!("- {}\n", p));
        }
    }
    s
}

/// What a prompt gets from the library: the block and the ids it holds.
#[derive(Debug, Clone, Default)]
pub struct Context {
    pub text: String,
    pub ids: Vec<i64>,
}

/// None when nothing is approved (the prompt then stays exactly as before).
pub fn prompt_context(conn: &Connection) -> Option<Context> {
    let entries = approved(conn);
    if entries.is_empty() {
        return None;
    }
    let artist: Option<i64> = conn.query_row("SELECT id FROM artists ORDER BY id LIMIT 1", [], |r| r.get(0)).ok();
    let played = played_lines(conn, artist, "en");
    Some(Context { text: build_block(&entries, &played), ids: entries.iter().map(|e| e.id).collect() })
}

/// The ids a model cited, kept only when they are approved entries.
pub fn used_ids(cited: &[i64], ctx: Option<&Context>) -> Vec<i64> {
    let Some(ctx) = ctx else { return vec![] };
    let mut out: Vec<i64> = cited.iter().copied().filter(|i| ctx.ids.contains(i)).collect();
    out.sort();
    out.dedup();
    out
}

pub fn ids_json(ids: &[i64]) -> String {
    serde_json::to_string(ids).unwrap_or_else(|_| "[]".into())
}

pub fn parse_ids(s: Option<&str>) -> Vec<i64> {
    s.and_then(|s| serde_json::from_str::<Vec<i64>>(s).ok()).unwrap_or_default()
}

/// Does an approved entry of one of `topics` back this draft? With `cited`,
/// the model must have cited it; without, its existence is enough (template
/// follow-ups).
pub fn covers(conn: &Connection, topics: &[&str], cited: Option<&[i64]>) -> bool {
    approved(conn)
        .iter()
        .any(|e| topics.contains(&e.topic.as_str()) && cited.map_or(true, |c| c.contains(&e.id)))
}

pub fn agent_name(conn: &Connection) -> String {
    conn.query_row("SELECT value FROM settings WHERE key = 'agent_name'", [], |r| r.get::<_, Option<String>>(0))
        .ok()
        .flatten()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_AGENT_NAME.to_string())
}

// ---------------- Commands ----------------

#[derive(Serialize)]
pub struct TopicInfo {
    pub kind: &'static str,
    pub topic: &'static str,
}

#[derive(Serialize)]
pub struct PlaybookView {
    pub entries: Vec<Entry>,
    pub topics: Vec<TopicInfo>,
    pub agent_name: String,
    pub artist: ArtistFacts,
}

fn conn(state: &State<'_, AppState>) -> Result<r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>, String> {
    state.pool.get().map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub fn playbook_list(state: State<'_, AppState>) -> Result<PlaybookView, String> {
    let c = conn(&state)?;
    ensure_seeds(&c);
    Ok(PlaybookView {
        entries: list(&c),
        topics: TOPICS.iter().map(|(k, t)| TopicInfo { kind: k, topic: t }).collect(),
        agent_name: agent_name(&c),
        artist: artist_facts(&c),
    })
}

#[tauri::command(async)]
pub fn playbook_save(state: State<'_, AppState>, entry: Entry) -> Result<i64, String> {
    save(&*conn(&state)?, &entry)
}

#[tauri::command(async)]
pub fn playbook_set_status(state: State<'_, AppState>, id: i64, status: String) -> Result<(), String> {
    set_status(&*conn(&state)?, id, &status)
}

#[tauri::command(async)]
pub fn playbook_delete(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    conn(&state)?.execute("DELETE FROM playbook_entries WHERE id = ?1", params![id]).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::residency::testdb;

    #[test]
    fn seeds_once_as_drafts_and_prefilled() {
        let c = testdb::conn();
        c.execute(
            "INSERT INTO artists (id, name, genres, city, mix_url, bio) VALUES (1, 'Piers', 'house, tech house', 'Paris', 'https://sc.com/piers/vol4', 'Une bio.')",
            [],
        )
        .unwrap();
        ensure_seeds(&c);
        let all = list(&c);
        assert!(all.len() >= SEEDS.len() + 5);
        assert!(all.iter().all(|e| e.status == "draft"), "seeds are never approved by default");
        assert!(approved(&c).is_empty());
        let short = all.iter().find(|e| e.topic == "bio_short").unwrap();
        assert_eq!(short.text_fr, "Piers, DJ et producteur (house, tech house), basé à Paris.");
        assert!(all.iter().any(|e| e.topic == "mixes_links" && e.text_fr.contains("vol4")));
        // No em dash in any French default.
        assert!(SEEDS.iter().all(|s| !s.fr.contains('—') && !s.en.contains('—')));
        // A deleted seed does not come back.
        c.execute("DELETE FROM playbook_entries WHERE topic = 'obj_budget'", []).unwrap();
        ensure_seeds(&c);
        assert!(!list(&c).iter().any(|e| e.topic == "obj_budget"));
        // Sorted: facts, answers, objections.
        let kinds: Vec<usize> = list(&c).iter().map(|e| kind_rank(&e.kind)).collect();
        assert!(kinds.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn save_and_approve_rules() {
        let c = testdb::conn();
        let id = save(&c, &Entry { kind: "answer".into(), topic: "fee".into(), text_fr: "Entre 800 et 1200 € — selon le format.".into(), ..Default::default() }).unwrap();
        let e = list(&c).into_iter().find(|e| e.id == id).unwrap();
        assert_eq!(e.status, "draft");
        assert!(!e.text_fr.contains('—'));
        assert!(save(&c, &Entry { kind: "answer".into(), topic: "nope".into(), ..Default::default() }).is_err());
        let empty = save(&c, &Entry { kind: "objection".into(), topic: "obj_other".into(), title: "X".into(), ..Default::default() }).unwrap();
        assert!(set_status(&c, empty, "approved").is_err());
        set_status(&c, id, "approved").unwrap();
        assert_eq!(approved(&c).len(), 1);
        set_status(&c, id, "draft").unwrap();
        assert!(approved(&c).is_empty());
    }

    #[test]
    fn prompt_block_uses_approved_entries_only_in_stable_order() {
        let c = testdb::conn();
        let obj = save(&c, &Entry { kind: "objection".into(), topic: "obj_full".into(), title: "Complet".into(), text_fr: "On se cale plus tard.".into(), status: "approved".into(), ..Default::default() }).unwrap();
        let fee = save(&c, &Entry { kind: "answer".into(), topic: "fee".into(), text_fr: "Selon le format.".into(), text_en: "Depends on the format.".into(), status: "approved".into(), ..Default::default() }).unwrap();
        let draft = save(&c, &Entry { kind: "answer".into(), topic: "rider".into(), text_fr: "SECRET BROUILLON".into(), ..Default::default() }).unwrap();
        let ctx = prompt_context(&c).unwrap();
        assert!(!ctx.text.contains("SECRET BROUILLON"));
        assert!(!ctx.ids.contains(&draft));
        assert_eq!(ctx.ids, vec![fee, obj], "answers before objections, whatever the insertion order");
        let p_fee = ctx.text.find(&format!("[P{}]", fee)).unwrap();
        let p_obj = ctx.text.find(&format!("[P{}]", obj)).unwrap();
        assert!(p_fee < p_obj);
        assert!(ctx.text.contains("EN: Depends on the format.") && ctx.text.contains("(Complet)"));
        // Same entries, same text (cache-stable), whatever updated_at says.
        c.execute("UPDATE playbook_entries SET updated_at = '2030-01-01 00:00:00'", []).unwrap();
        assert_eq!(prompt_context(&c).unwrap().text, ctx.text);
        assert_eq!(used_ids(&[obj, 999, obj], Some(&ctx)), vec![obj]);
        assert!(covers(&c, &["obj_full"], Some(&[obj])));
        assert!(!covers(&c, &["obj_full"], Some(&[fee])));
        assert!(!covers(&c, &["rider"], None), "a draft entry covers nothing");
        // Nothing approved: no block at all.
        c.execute("UPDATE playbook_entries SET status = 'draft'", []).unwrap();
        assert!(prompt_context(&c).is_none());
    }
}
