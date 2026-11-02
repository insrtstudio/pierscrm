//! "La campagne arrive au bout": a residency campaign whose autopilot has less
//! than a day of first emails left gets the next zone to open, a country
//! preset or a list of cities of one country that no active residency
//! campaign covers yet, ranked by the clubs `targeting::build` would list
//! there (same filters: status, capacity, nights, a usable address, nothing
//! suppressed, declined or mailed recently).
//!
//! One click creates that campaign from the source's config (pitch adapted to
//! the zone, sequence, assets, press kit, window, daily cap), builds and adds
//! the target and writes the step-1 drafts. It stops there: first emails of a
//! new campaign are not an autonomy category (commands/autonomy.rs covers
//! replies only), so they always wait for the user's review, and the
//! autopilot stays off until the user switches it on (with its confirmation).

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::collections::{HashMap, HashSet};

use super::claude::Llm;
use super::commands::zone_words;
use super::content;
use super::insrt::Insrt;
use super::targeting::{self, BuildOptions, Candidate, Pick};
use super::zone::{city_label, iso_country, Zone};
use super::{load, save_config, Config};
use crate::commands::harvest::own_email_sql;
use crate::db::normalise;

/// Most cities one proposed campaign spans (the rest comes with the next one).
pub const MAX_CITIES: usize = 10;
/// Fewer eligible clubs than this is not worth a campaign.
pub const MIN_ELIGIBLE: i64 = 3;
/// Countries probed with the full builder (the cheap pre-count ranks them).
const TOP_COUNTRIES: usize = 6;
pub const MAX_SUGGESTIONS: usize = 5;
/// "Plus tard" hides the proposal of a campaign for this long.
pub const DISMISS_DAYS: i64 = 7;

/// Countries with a zone preset: the whole country is proposed at once.
fn preset_of(iso: &str) -> Option<&'static str> {
    match iso {
        "BE" => Some("belgium"),
        "CH" => Some("switzerland"),
        _ => None,
    }
}

/// (French name, French preposition, English name).
fn country_names(iso: &str) -> (String, &'static str, String) {
    const NAMES: &[(&str, &str, &str, &str)] = &[
        ("FR", "France", "en", "France"), ("BE", "Belgique", "en", "Belgium"), ("CH", "Suisse", "en", "Switzerland"),
        ("GB", "Royaume-Uni", "au", "the United Kingdom"), ("DE", "Allemagne", "en", "Germany"),
        ("NL", "Pays-Bas", "aux", "the Netherlands"), ("ES", "Espagne", "en", "Spain"), ("IT", "Italie", "en", "Italy"),
        ("PT", "Portugal", "au", "Portugal"), ("LU", "Luxembourg", "au", "Luxembourg"), ("AT", "Autriche", "en", "Austria"),
        ("IE", "Irlande", "en", "Ireland"), ("DK", "Danemark", "au", "Denmark"), ("SE", "Suède", "en", "Sweden"),
        ("PL", "Pologne", "en", "Poland"), ("CZ", "Tchéquie", "en", "Czechia"), ("MC", "Monaco", "à", "Monaco"),
    ];
    NAMES
        .iter()
        .find(|(k, ..)| *k == iso)
        .map(|(_, fr, prep, en)| (fr.to_string(), *prep, en.to_string()))
        .unwrap_or_else(|| (iso.to_string(), "en", iso.to_string()))
}

/// "Lyon, Marseille et Strasbourg" / "Lyon, Marseille, Strasbourg et 4 autres villes".
fn list_words(items: &[String], and: &str, more: impl Fn(usize) -> String) -> String {
    match items.len() {
        0 => String::new(),
        1 => items[0].clone(),
        2 | 3 => format!("{} {} {}", items[..items.len() - 1].join(", "), and, items[items.len() - 1]),
        n => format!("{} {} {}", items[..3].join(", "), and, more(n - 3)),
    }
}

/// One proposed campaign.
#[derive(Debug, Clone, Serialize)]
pub struct Suggestion {
    /// Stable id of the zone ("FR:lyon,marseille" / "preset:switzerland").
    pub key: String,
    pub country: String,
    pub country_fr: String,
    pub country_en: String,
    pub zone: Zone,
    /// Cities in order of eligible clubs, as written in French.
    pub cities: Vec<String>,
    /// Clubs `targeting::build` lists there today.
    pub eligible: i64,
    /// Clubs of the zone that would join once their email is found.
    pub no_email: i64,
    /// Best-ranked clubs, for the detail line.
    pub top: Vec<String>,
    /// Name of the campaign the click creates.
    pub name: String,
    /// "à Lyon, Marseille et Strasbourg" / "en Suisse".
    pub place_fr: String,
    pub place_en: String,
}

impl Suggestion {
    /// Words for the pre-filled pitch ("un club de {fr}" / "a club in {en}").
    fn pitch_words(&self) -> (String, String) {
        if !self.zone.presets.is_empty() {
            return zone_words(&self.zone);
        }
        match self.cities.len() {
            1 => (self.cities[0].clone(), city_label(&self.cities[0], "en")),
            _ => {
                let top: Vec<String> = self.cities.iter().take(3).cloned().collect();
                let en: Vec<String> = top.iter().map(|c| city_label(c, "en")).collect();
                (format!("{} ({})", self.country_fr, top.join(", ")), format!("{} ({})", self.country_en, en.join(", ")))
            }
        }
    }
}

// ---------------- Detection ----------------

/// Audience members still waiting for their first email (draft or
/// scheduled step 1, or none yet).
pub fn remaining_first(conn: &Connection, campaign_id: i64) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM campaign_contacts cc
         WHERE cc.campaign_id = ?1 AND cc.state = 'planned'
           AND NOT EXISTS (SELECT 1 FROM campaign_messages m WHERE m.campaign_id = cc.campaign_id
                           AND m.contact_id = cc.contact_id AND m.step = 1 AND m.status IN ('sent','skipped'))",
        params![campaign_id],
        |r| r.get(0),
    )
    .unwrap_or(0)
}

/// Remaining first emails when the autopilot is on and less than one day of
/// sends is left, None otherwise.
pub fn running_dry(conn: &Connection, campaign_id: i64) -> Option<i64> {
    let r = load(conn, campaign_id)?;
    if !r.autopilot {
        return None;
    }
    let n = remaining_first(conn, campaign_id);
    (n < r.config.daily_max.max(1)).then_some(n)
}

/// Folded cities and ISO countries every active residency campaign covers.
pub fn covered(conn: &Connection) -> (HashSet<String>, HashSet<String>) {
    let mut cities = HashSet::new();
    let mut countries = HashSet::new();
    for id in super::active_ids(conn) {
        let Some(r) = load(conn, id) else { continue };
        let (c, k) = r.config.zone.resolve();
        cities.extend(c);
        countries.extend(k);
    }
    (cities, countries)
}

/// Contacts already in the audience of an active residency campaign.
fn in_some_audience(conn: &Connection) -> HashSet<i64> {
    let Ok(mut st) = conn.prepare(
        "SELECT cc.contact_id FROM campaign_contacts cc JOIN campaigns c ON c.id = cc.campaign_id
         WHERE c.kind = 'residency' AND c.status = 'active'",
    ) else {
        return HashSet::new();
    };
    st.query_map([], |r| r.get(0)).map(|it| it.flatten().collect()).unwrap_or_default()
}

/// The builder over `zone` with the source's filters, minus covered cities
/// and contacts another residency campaign already holds.
fn eligible(
    conn: &Connection,
    source_id: i64,
    cfg: &Config,
    zone: Zone,
    covered_cities: &HashSet<String>,
    taken: &HashSet<i64>,
) -> Result<(Vec<Candidate>, i64), String> {
    let probe = Config { zone, ..cfg.clone() };
    let r = targeting::build(conn, source_id, &probe, &BuildOptions { limit: Some(1000), ..Default::default() })?;
    let list = r
        .candidates
        .into_iter()
        .filter(|c| !covered_cities.contains(&normalise(c.city.as_deref().unwrap_or(""))))
        .filter(|c| c.contact_id.map_or(true, |id| !taken.contains(&id)))
        .collect();
    Ok((list, r.excluded_no_email))
}

/// How long computed suggestions are reused while nothing they depend on moved.
const CACHE_SECS: u64 = 600;

type Cached = (std::time::Instant, String, Vec<Suggestion>);
static CACHE: std::sync::Mutex<Vec<Cached>> = std::sync::Mutex::new(Vec::new());

/// What the suggestions depend on besides the source's filters: the clubs and
/// their addresses, every audience, suppressions, sends and the active
/// campaigns' zones. Cheap counters, so a change anywhere recomputes.
fn fingerprint(conn: &Connection) -> String {
    conn.query_row(
        "SELECT (SELECT COALESCE(MAX(id), 0) FROM vi_contacts) || '|' ||
                (SELECT COALESCE(MAX(updated_at), '') FROM vi_venues) || '|' ||
                (SELECT COUNT(*) FROM campaign_contacts) || '|' ||
                (SELECT COUNT(*) FROM suppressions) || '|' ||
                (SELECT COUNT(*) FROM emails WHERE status = 'sent') || '|' ||
                (SELECT COALESCE(MAX(updated_at), '') FROM contacts) || '|' ||
                (SELECT COALESCE(group_concat(c.id || ':' || c.status || ':' || length(r.config)), '')
                   FROM campaigns c JOIN residency_campaigns r ON r.campaign_id = c.id)",
        [],
        |r| r.get(0),
    )
    .unwrap_or_default()
}

/// Next zones to open for `source_id`, best first. Campaigns with the same
/// filters get the same answer (every active audience is excluded either
/// way), so the Actions list running dry on three campaigns computes it once.
pub fn suggestions(conn: &Connection, source_id: i64) -> Result<Vec<Suggestion>, String> {
    // Tests run in parallel on databases that look alike: no sharing there.
    if cfg!(test) {
        return compute_suggestions(conn, source_id);
    }
    let r = load(conn, source_id).ok_or_else(|| "Campagne résidence introuvable.".to_string())?;
    let c = &r.config;
    let key = format!(
        "{}|{}|{}|{}|{}",
        c.capacity_min,
        c.capacity_max,
        c.min_events,
        c.lang,
        fingerprint(conn)
    );
    if let Ok(cache) = CACHE.lock() {
        if let Some((_, _, v)) =
            cache.iter().find(|(at, k, _)| *k == key && at.elapsed().as_secs() < CACHE_SECS)
        {
            return Ok(v.clone());
        }
    }
    let out = compute_suggestions(conn, source_id)?;
    if let Ok(mut cache) = CACHE.lock() {
        cache.retain(|(at, k, _)| *k != key && at.elapsed().as_secs() < CACHE_SECS);
        cache.push((std::time::Instant::now(), key, out.clone()));
    }
    Ok(out)
}

fn compute_suggestions(conn: &Connection, source_id: i64) -> Result<Vec<Suggestion>, String> {
    let r = load(conn, source_id).ok_or_else(|| "Campagne résidence introuvable.".to_string())?;
    let cfg = &r.config;
    let (cov_cities, cov_countries) = covered(conn);
    let taken = in_some_audience(conn);

    // Cheap pre-count per country (clubs with an address, outside covered
    // cities) to pick the few countries worth the full builder.
    let cap_min = cfg.capacity_min;
    let cap_max = cfg.capacity_max.max(cap_min);
    let sql = format!(
        "SELECT v.pays, v.ville_norm, COUNT(*) FROM vi_venues v
         WHERE v.statut IN ('qualifie','valide') AND v.pays IS NOT NULL AND trim(v.pays) <> ''
           AND (v.capacite_est IS NULL OR v.capacite_est BETWEEN {cap_min} AND {cap_max})
           AND v.nb_events_periode >= {min}
           AND EXISTS (SELECT 1 FROM vi_contacts c WHERE c.venue_id = v.id AND c.type = 'email' AND {own})
         GROUP BY v.pays, v.ville_norm",
        min = cfg.min_events.max(0),
        own = own_email_sql("c.valeur"),
    );
    let mut per_country: HashMap<String, i64> = HashMap::new();
    {
        let mut st = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let rows = st
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, i64>(2)?)))
            .map_err(|e| e.to_string())?;
        for (pays, city, n) in rows.flatten() {
            let iso = iso_country(&pays);
            if cov_countries.contains(&iso) || city.as_ref().map_or(false, |c| cov_cities.contains(c)) {
                continue;
            }
            *per_country.entry(iso).or_default() += n;
        }
    }
    let mut countries: Vec<(String, i64)> = per_country.into_iter().filter(|(_, n)| *n > 0).collect();
    countries.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    countries.truncate(TOP_COUNTRIES);

    let mut out = Vec::new();
    for (iso, _) in countries {
        // Full builder over the country, grouped by city.
        let (cands, _) = eligible(conn, source_id, cfg, Zone { countries: vec![iso.clone()], ..Default::default() }, &cov_cities, &taken)?;
        let mut by_city: Vec<(String, String, i64)> = Vec::new(); // (folded, label, n)
        for c in &cands {
            let label = c.city.clone().unwrap_or_default();
            let folded = normalise(&label);
            if folded.len() < 2 {
                continue;
            }
            match by_city.iter_mut().find(|x| x.0 == folded) {
                Some(x) => x.2 += 1,
                None => by_city.push((folded, label.trim().to_string(), 1)),
            }
        }
        by_city.sort_by(|a, b| b.2.cmp(&a.2).then(a.1.cmp(&b.1)));
        if by_city.is_empty() {
            continue;
        }
        let (fr_name, prep, en_name) = country_names(&iso);
        // A preset takes the whole country: only when none of its cities is
        // already another campaign's.
        let any_city_covered = covered_in_country(conn, &iso, &cov_cities);
        let (zone, key, cities) = match preset_of(&iso).filter(|_| !any_city_covered) {
            Some(p) => (
                Zone { presets: vec![p.to_string()], ..Default::default() },
                format!("preset:{}", p),
                by_city.iter().map(|x| city_label(&x.1, "fr")).collect::<Vec<_>>(),
            ),
            None => {
                let picked: Vec<&(String, String, i64)> = by_city.iter().take(MAX_CITIES).collect();
                (
                    Zone { cities: picked.iter().map(|x| x.1.clone()).collect(), ..Default::default() },
                    format!("{}:{}", iso, picked.iter().map(|x| x.0.as_str()).collect::<Vec<_>>().join(",")),
                    picked.iter().map(|x| city_label(&x.1, "fr")).collect::<Vec<_>>(),
                )
            }
        };
        // Exact counts over the zone the click would create.
        let (cands, no_email) = eligible(conn, source_id, cfg, zone.clone(), &cov_cities, &taken)?;
        let n = cands.len() as i64;
        if n < MIN_ELIGIBLE {
            continue;
        }
        let is_preset = !zone.presets.is_empty();
        let en_cities: Vec<String> = cities.iter().map(|c| city_label(c, "en")).collect();
        let place_fr = if is_preset {
            format!("{} {}", prep, fr_name)
        } else {
            format!("à {}", list_words(&cities, "et", |k| format!("{} autre{} ville{}", k, if k > 1 { "s" } else { "" }, if k > 1 { "s" } else { "" })))
        };
        let place_en = if is_preset {
            format!("in {}", en_name)
        } else {
            format!("in {}", list_words(&en_cities, "and", |k| format!("{} more cit{}", k, if k > 1 { "ies" } else { "y" })))
        };
        let name = if is_preset {
            format!("Résidence {}", fr_name)
        } else if cities.len() == 1 {
            format!("Résidence {}", cities[0])
        } else {
            let head: Vec<String> = cities.iter().take(3).cloned().collect();
            format!("Résidence {} ({}{})", fr_name, head.join(", "), if cities.len() > 3 { "..." } else { "" })
        };
        out.push(Suggestion {
            key,
            country: iso,
            country_fr: fr_name,
            country_en: en_name,
            zone,
            cities,
            eligible: n,
            no_email,
            top: cands.iter().take(3).map(|c| c.name.clone()).collect(),
            name,
            place_fr,
            place_en,
        });
    }
    out.sort_by(|a, b| b.eligible.cmp(&a.eligible).then(a.key.cmp(&b.key)));
    out.truncate(MAX_SUGGESTIONS);
    Ok(out)
}

/// Does an active campaign already aim at one of this country's cities?
fn covered_in_country(conn: &Connection, iso: &str, cov_cities: &HashSet<String>) -> bool {
    if cov_cities.is_empty() {
        return false;
    }
    let Ok(mut st) = conn.prepare("SELECT DISTINCT ville_norm FROM vi_venues WHERE pays = ?1 AND ville_norm IS NOT NULL") else {
        return false;
    };
    super::zone::pays_values(iso).iter().any(|p| {
        st.query_map(params![p], |r| r.get::<_, String>(0))
            .map(|it| it.flatten().any(|c| cov_cities.contains(&c)))
            .unwrap_or(false)
    })
}

// ---------------- What Actions shows ----------------

#[derive(Debug, Clone)]
pub enum Proposal {
    /// The campaign created from the last proposal still waits for review
    /// (autopilot off): its drafts come first.
    Review { campaign_id: i64, name: String, drafts: i64 },
    Expand { remaining: i64, suggestion: Suggestion },
}

/// Campaign created by the last expansion of `source_id`, while it is still
/// active with its autopilot off.
fn pending_child(conn: &Connection, source_id: i64) -> Option<(i64, String, i64)> {
    conn.query_row(
        "SELECT c.id, c.name, (SELECT COUNT(*) FROM campaign_messages m WHERE m.campaign_id = c.id AND m.status = 'draft')
         FROM residency_expansions x JOIN campaigns c ON c.id = x.campaign_id
         JOIN residency_campaigns r ON r.campaign_id = c.id
         WHERE x.source_campaign_id = ?1 AND x.kind = 'created' AND c.status = 'active' AND r.autopilot = 0
         ORDER BY x.id DESC LIMIT 1",
        params![source_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )
    .optional()
    .ok()
    .flatten()
}

fn dismissed_recently(conn: &Connection, source_id: i64) -> bool {
    conn.query_row(
        &format!(
            "SELECT EXISTS(SELECT 1 FROM residency_expansions WHERE source_campaign_id = ?1 AND kind = 'dismissed'
               AND created_at >= datetime('now', '-{} days'))",
            DISMISS_DAYS
        ),
        params![source_id],
        |r| r.get(0),
    )
    .unwrap_or(false)
}

/// What Agenda › Actions proposes for a campaign, if anything.
pub fn proposal(conn: &Connection, source_id: i64) -> Option<Proposal> {
    let remaining = running_dry(conn, source_id)?;
    if let Some((campaign_id, name, drafts)) = pending_child(conn, source_id) {
        return Some(Proposal::Review { campaign_id, name, drafts });
    }
    if dismissed_recently(conn, source_id) {
        return None;
    }
    let suggestion = suggestions(conn, source_id).ok()?.into_iter().next()?;
    Some(Proposal::Expand { remaining, suggestion })
}

pub fn dismiss(conn: &Connection, source_id: i64) -> Result<(), String> {
    conn.execute(
        "INSERT INTO residency_expansions (source_campaign_id, kind) VALUES (?1, 'dismissed')",
        params![source_id],
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

// ---------------- One click: the new campaign ----------------

#[derive(Debug, Clone, Serialize)]
pub struct Created {
    pub campaign_id: i64,
    pub name: String,
    pub targets: usize,
    pub drafts: usize,
    pub with_ai: usize,
    pub errors: Vec<String>,
}

/// The source's pitch with its zone words swapped for the new ones, or the
/// default pitch when the source does not name its zone.
fn adapt_pitch(src: &str, src_words: &str, new_words: &str, default: String) -> String {
    if !src.trim().is_empty() && !src_words.trim().is_empty() && src.contains(src_words) {
        src.replace(src_words, new_words)
    } else {
        default
    }
}

/// Create the campaign for `zone` from `source_id`: config cloned and adapted,
/// target built and added, step-1 drafts written. Autopilot off, every first
/// email a draft. Refused when an active campaign already covers part of the
/// zone (a second click does not create a twin).
pub fn create(
    conn: &Connection,
    source_id: i64,
    zone: Zone,
    name: Option<String>,
    llm: Option<&dyn Llm>,
    insrt: Option<&dyn Insrt>,
) -> Result<Created, String> {
    let src = load(conn, source_id).ok_or_else(|| "Campagne résidence introuvable.".to_string())?;
    let (cities, countries) = zone.resolve();
    if cities.is_empty() && countries.is_empty() {
        return Err("Choisissez une zone cible.".into());
    }
    let (cov_cities, cov_countries) = covered(conn);
    if cities.iter().any(|c| cov_cities.contains(c)) || countries.iter().any(|c| cov_countries.contains(c)) {
        return Err("Une campagne résidence active couvre déjà cette zone.".into());
    }
    let sugg = suggestions(conn, source_id)?.into_iter().find(|s| s.zone == zone);
    let name = name
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .or_else(|| sugg.as_ref().map(|s| s.name.clone()))
        .unwrap_or_else(|| format!("Résidence {}", zone_words(&zone).0));
    let (fr, en) = match &sugg {
        Some(s) => s.pitch_words(),
        None => zone_words(&zone),
    };
    let (src_fr, src_en) = zone_words(&src.config.zone);

    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    tx.execute(
        "INSERT INTO campaigns (name, purpose, artist_id, status, color, kind)
         SELECT ?2, COALESCE(purpose, 'Résidence mensuelle ou bimestrielle'), artist_id, 'active', COALESCE(color, '#ff5a1f'), 'residency'
         FROM campaigns WHERE id = ?1",
        params![source_id, name],
    )
    .map_err(|e| e.to_string())?;
    let id = tx.last_insert_rowid();
    let mut cfg = src.config.clone();
    cfg.zone = zone;
    cfg.pitch_fr = adapt_pitch(&src.config.pitch_fr, &src_fr, &fr, super::default_pitch_fr(&fr));
    cfg.pitch_en = adapt_pitch(&src.config.pitch_en, &src_en, &en, super::default_pitch_en(&en));
    // Each target's own language (a Zurich club gets English).
    cfg.lang = "auto".into();
    save_config(&tx, id, &cfg)?;
    tx.execute(
        "INSERT INTO residency_expansions (source_campaign_id, campaign_id, zone, kind) VALUES (?1, ?2, ?3, 'created')",
        params![source_id, id, serde_json::to_string(&cfg.zone).unwrap_or_default()],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    super::log(conn, Some(id), "info", "created", &format!("Campagne résidence créée depuis la campagne {}", source_id));

    // The target, as the "Construire la cible" button would pre-pick it.
    let taken = in_some_audience(conn);
    let built = targeting::build(conn, id, &cfg, &BuildOptions::default())?;
    let picks: Vec<Pick> = built
        .candidates
        .iter()
        .filter(|c| c.email_rank < 4 && c.contact_id.map_or(true, |k| !taken.contains(&k)))
        .map(|c| Pick {
            venue_id: c.venue_id,
            contact_id: c.contact_id,
            email: c.email.clone(),
            first_name: c.first_name.clone(),
            promoter_id: c.promoter_id,
        })
        .collect();
    let targets = targeting::add_targets(conn, id, &picks)?;
    super::log(conn, Some(id), "info", "targets_added", &format!("{} cible(s) ajoutée(s)", targets));
    let g = content::generate(conn, id, &cfg, None, false, llm, insrt)?;
    super::log(
        conn,
        Some(id),
        "info",
        "drafts",
        &format!("{} brouillon(s), dont {} rédigé(s) par l'IA", g.created, g.with_ai),
    );
    super::log(conn, Some(source_id), "info", "expanded", &format!("Nouvelle campagne « {} » proposée et créée ({} cibles)", name, targets));
    Ok(Created { campaign_id: id, name, targets, drafts: g.created, with_ai: g.with_ai, errors: g.errors })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::residency::claude::fake::FakeLlm;
    use crate::commands::residency::insrt::{ActivityPage, PressKit, PressLink, Recipient};
    use crate::commands::residency::testdb;

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

    /// "Résidence Paris" on autopilot with two first emails left, an active
    /// "Résidence Belgique", and Discovery clubs in Paris, Lyon, Marseille,
    /// Strasbourg, Lille and Switzerland.
    fn setup() -> Connection {
        let c = testdb::conn();
        c.execute_batch(
            "INSERT INTO artists (id, name) VALUES (1, 'Piers');
             INSERT INTO campaigns (id, name, kind, artist_id, color, purpose) VALUES
                (1, 'Résidence Paris', 'residency', 1, '#123456', 'Résidence'),
                (2, 'Résidence Belgique', 'residency', 1, NULL, NULL);
             INSERT INTO contacts (id, category, name, venue, area, email, status) VALUES
                (10, 'venue', 'Club P1', 'Club P1', 'Paris', 'booking@p1.fr', 'to_contact'),
                (11, 'venue', 'Club P2', 'Club P2', 'Paris', 'booking@p2.fr', 'to_contact');
             INSERT INTO campaign_contacts (campaign_id, contact_id, state) VALUES (1, 10, 'planned'), (1, 11, 'planned');
             INSERT INTO vi_venues (id, nom, nom_normalise, ville, ville_norm, pays, statut, capacite_est, nb_events_periode, score_qualif) VALUES
                (1, 'Paris Club', 'parisclub', 'Paris', 'paris', 'FR', 'qualifie', 300, 50, 50),
                (2, 'Lyon Un', 'lyonun', 'Lyon', 'lyon', 'FR', 'qualifie', 300, 80, 70),
                (3, 'Lyon Deux', 'lyondeux', 'Lyon', 'lyon', 'FR', 'qualifie', 400, 60, 60),
                (4, 'Lyon Trois', 'lyontrois', 'Lyon', 'lyon', 'FR', 'valide', NULL, 40, 50),
                (5, 'Marseille Un', 'marseilleun', 'Marseille', 'marseille', 'FR', 'qualifie', 500, 70, 60),
                (6, 'Marseille Deux', 'marseilledeux', 'Marseille', 'marseille', 'FR', 'qualifie', 200, 30, 40),
                (7, 'Strasbourg Un', 'strasbourgun', 'Strasbourg', 'strasbourg', 'FR', 'qualifie', 300, 30, 40),
                (8, 'Strasbourg Muet', 'strasbourgmuet', 'Strasbourg', 'strasbourg', 'FR', 'qualifie', 300, 30, 40),
                (9, 'Lille Parti', 'lilleparti', 'Lille', 'lille', 'FR', 'qualifie', 300, 30, 40),
                (10, 'Lyon Arena', 'lyonarena', 'Lyon', 'lyon', 'FR', 'qualifie', 6000, 300, 90),
                (11, 'Brussels Club', 'brusselsclub', 'Brussels', 'brussels', 'BE', 'qualifie', 300, 90, 60),
                (12, 'Geneva Club', 'genevaclub', 'Geneva', 'geneva', 'CH', 'qualifie', 300, 90, 60),
                (13, 'Zurich Club', 'zurichclub', 'Zurich', 'zurich', 'CH', 'qualifie', 300, 90, 60);
             INSERT INTO vi_contacts (venue_id, type, valeur, score, source_url) VALUES
                (1, 'email', 'booking@parisclub.fr', 30, 'x'),
                (2, 'email', 'booking@lyonun.fr', 30, 'x'), (3, 'email', 'prog@lyondeux.fr', 30, 'x'),
                (4, 'email', 'contact@lyontrois.fr', 30, 'x'),
                (5, 'email', 'booking@marseilleun.fr', 30, 'x'), (6, 'email', 'booking@marseilledeux.fr', 30, 'x'),
                (7, 'email', 'booking@strasbourgun.fr', 30, 'x'),
                (9, 'email', 'gone@lilleparti.fr', 30, 'x'),
                (10, 'email', 'booking@lyonarena.fr', 30, 'x'),
                (11, 'email', 'booking@brusselsclub.be', 30, 'x'),
                (12, 'email', 'booking@genevaclub.ch', 30, 'x'), (13, 'email', 'booking@zurichclub.ch', 30, 'x');
             INSERT INTO suppressions (address, reason) VALUES ('gone@lilleparti.fr', 'bounce');
             -- Every club books the style (targeting.rs, style_match).
             UPDATE vi_venues SET fit_score = 40;",
        )
        .unwrap();
        let mut paris = Config::default();
        paris.pitch_fr = crate::commands::residency::default_pitch_fr("Paris");
        paris.pitch_en = crate::commands::residency::default_pitch_en("Paris");
        paris.press_kit_id = Some("pk1".into());
        paris.press_kit_title = Some("EPK Piers".into());
        paris.daily_max = 6;
        paris.lang = "fr".into();
        save_config(&c, 1, &paris).unwrap();
        let be = Config { zone: Zone { presets: vec!["belgium".into()], ..Default::default() }, ..Config::default() };
        save_config(&c, 2, &be).unwrap();
        c.execute("UPDATE residency_campaigns SET autopilot = 1 WHERE campaign_id = 1", []).unwrap();
        c
    }

    #[test]
    fn detects_a_campaign_running_dry() {
        let c = setup();
        assert_eq!(running_dry(&c, 1), Some(2));
        // A sent first email no longer counts; the autopilot off means no proposal.
        c.execute(
            "INSERT INTO campaign_messages (campaign_id, contact_id, step, status) VALUES (1, 10, 1, 'sent')",
            [],
        )
        .unwrap();
        assert_eq!(remaining_first(&c, 1), 1);
        assert_eq!(running_dry(&c, 2), None);
        // Enough targets left for a day: not dry.
        c.execute("UPDATE residency_campaigns SET config = json_set(config, '$.daily_max', 1) WHERE campaign_id = 1", []).unwrap();
        assert_eq!(running_dry(&c, 1), None);
    }

    #[test]
    fn suggests_uncovered_cities_ranked_by_eligible_clubs() {
        let c = setup();
        let s = suggestions(&c, 1).unwrap();
        // France: Paris is covered, Lille's only address bounced, the arena
        // is too big. Belgium is covered; Switzerland has 2 clubs (< 3).
        assert_eq!(s.len(), 1, "{:?}", s);
        let fr = &s[0];
        assert_eq!(fr.country, "FR");
        assert_eq!(fr.cities, vec!["Lyon", "Marseille", "Strasbourg"]);
        assert_eq!(fr.zone.cities, vec!["Lyon", "Marseille", "Strasbourg"]);
        assert_eq!(fr.eligible, 6);
        assert_eq!(fr.no_email, 1, "Strasbourg Muet has no address");
        assert_eq!(fr.place_fr, "à Lyon, Marseille et Strasbourg");
        assert_eq!(fr.place_en, "in Lyon, Marseille and Strasbourg");
        assert_eq!(fr.name, "Résidence France (Lyon, Marseille, Strasbourg)");
        assert!(!fr.place_fr.contains('—') && !fr.name.contains('—'));

        // A third Swiss club: the Switzerland preset joins, after France.
        c.execute_batch(
            "INSERT INTO vi_venues (id, nom, nom_normalise, ville, ville_norm, pays, statut, capacite_est, nb_events_periode, score_qualif)
               VALUES (14, 'Lausanne Club', 'lausanneclub', 'Lausanne', 'lausanne', 'CH', 'qualifie', 300, 90, 60);
             INSERT INTO vi_contacts (venue_id, type, valeur, score, source_url) VALUES (14, 'email', 'booking@lausanneclub.ch', 30, 'x');
             UPDATE vi_venues SET fit_score = 40 WHERE id = 14;",
        )
        .unwrap();
        let s = suggestions(&c, 1).unwrap();
        assert_eq!(s.iter().map(|x| x.key.as_str()).collect::<Vec<_>>(), vec!["FR:lyon,marseille,strasbourg", "preset:switzerland"]);
        assert_eq!(s[1].place_fr, "en Suisse");
        assert_eq!(s[1].name, "Résidence Suisse");
        assert_eq!(list_words(&["A".into(), "B".into(), "C".into(), "D".into(), "E".into()], "et", |k| format!("{} autres villes", k)), "A, B, C et 2 autres villes");

        // Actions shows the best one for the campaign running dry.
        let a = crate::commands::residency::commands::actions(&c).unwrap();
        let x = a.iter().find(|x| x.kind == "expand").expect("expand item");
        assert_eq!((x.campaign_id, x.count, x.remaining), (Some(1), Some(6), Some(2)));
        assert_eq!(x.expand.as_ref().unwrap().key, "FR:lyon,marseille,strasbourg");
        // "Plus tard" hides it for a week.
        dismiss(&c, 1).unwrap();
        assert!(proposal(&c, 1).is_none());
    }

    /// Three campaigns running dry towards the same zone: one card naming the
    /// three, created from the one with the fewest first emails left; once
    /// created, its review shows once.
    #[test]
    fn one_card_per_zone_when_several_campaigns_run_dry() {
        let c = setup();
        c.execute_batch(
            "INSERT INTO campaigns (id, name, kind, artist_id) VALUES (3, 'Résidence Bretagne', 'residency', 1);
             INSERT INTO contacts (id, category, name, venue, area, email, status) VALUES
                (12, 'venue', 'Club B1', 'Club B1', 'Brussels', 'booking@b1.be', 'to_contact');
             INSERT INTO campaign_contacts (campaign_id, contact_id, state) VALUES (2, 12, 'planned');",
        )
        .unwrap();
        let mut bzh = load(&c, 1).unwrap().config;
        bzh.zone = Zone { cities: vec!["Rennes".into()], ..Default::default() };
        save_config(&c, 3, &bzh).unwrap();
        c.execute("UPDATE residency_campaigns SET autopilot = 1", []).unwrap();
        // Paris 2 left, Belgique 1, Bretagne 0: all three dry, same best zone.
        let key = |id| match proposal(&c, id) {
            Some(Proposal::Expand { suggestion, .. }) => suggestion.key,
            other => panic!("{:?}", other),
        };
        assert_eq!(key(1), key(2));
        assert_eq!(key(1), key(3));
        let a = crate::commands::residency::commands::actions(&c).unwrap();
        let ex: Vec<_> = a.iter().filter(|x| x.kind == "expand").collect();
        assert_eq!(ex.len(), 1, "{:?}", ex);
        let x = ex[0];
        let src: Vec<(i64, i64)> = x.sources.as_ref().unwrap().iter().map(|s| (s.id, s.remaining)).collect();
        assert_eq!(src, vec![(1, 2), (2, 1), (3, 0)]);
        assert_eq!((x.campaign_id, x.remaining, x.key.as_str()), (Some(3), Some(0), "ex3"));

        // Created from Bretagne: one review, the other two have nothing left to open.
        let zone = x.expand.as_ref().unwrap().zone.clone();
        let r = create(&c, 3, zone, None, None, None).unwrap();
        let a = crate::commands::residency::commands::actions(&c).unwrap();
        let reviews: Vec<_> = a.iter().filter(|x| x.kind == "expand_review").collect();
        assert_eq!(reviews.len(), 1);
        assert_eq!(reviews[0].campaign_id, Some(r.campaign_id));
        assert!(!a.iter().any(|x| x.kind == "expand"));
    }

    #[test]
    fn one_click_creates_the_campaign_up_to_drafts() {
        let c = setup();
        let zone = suggestions(&c, 1).unwrap().remove(0).zone;
        let answers: Vec<String> = (0..6)
            .map(|i| format!(r#"{{"subject":"Résidence {i}","body":"Hello,\n\nUne idée pour vous.\n\nThibault"}}"#))
            .collect();
        let refs: Vec<&str> = answers.iter().map(String::as_str).collect();
        let llm = FakeLlm::new(&refs);
        let r = create(&c, 1, zone.clone(), None, Some(&llm), Some(&FakeInsrt)).unwrap();
        assert_eq!((r.targets, r.drafts, r.with_ai), (6, 6, 6), "{:?}", r.errors);
        assert_eq!(r.name, "Résidence France (Lyon, Marseille, Strasbourg)");
        let new = load(&c, r.campaign_id).unwrap();
        let src = load(&c, 1).unwrap();
        // Config cloned, zone and pitch adapted, autopilot left off.
        assert!(!new.autopilot && new.autopilot_confirmed_at.is_none());
        assert_eq!(new.config.zone, zone);
        assert_eq!(new.config.steps, src.config.steps);
        assert_eq!(new.config.assets, src.config.assets);
        assert_eq!(new.config.press_kit_id.as_deref(), Some("pk1"));
        assert_eq!((new.config.daily_max, new.config.window.clone()), (6, src.config.window.clone()));
        assert_eq!(new.config.lang, "auto");
        assert!(new.config.pitch_fr.contains("un club de France (Lyon, Marseille, Strasbourg)"), "{}", new.config.pitch_fr);
        assert!(new.config.pitch_en.contains("club in France (Lyon, Marseille, Strasbourg)"));
        assert!(!new.config.pitch_fr.contains('—'));
        let (kind, color, artist): (String, String, i64) = c
            .query_row("SELECT kind, color, artist_id FROM campaigns WHERE id = ?1", [r.campaign_id], |x| Ok((x.get(0)?, x.get(1)?, x.get(2)?)))
            .unwrap();
        assert_eq!((kind.as_str(), color.as_str(), artist), ("residency", "#123456", 1));
        // Every first email waits as a draft, with its private EPK link; nothing sent.
        let (drafts, other): (i64, i64) = c
            .query_row(
                "SELECT SUM(status = 'draft'), SUM(status <> 'draft') FROM campaign_messages WHERE campaign_id = ?1",
                [r.campaign_id],
                |x| Ok((x.get(0)?, x.get(1)?)),
            )
            .unwrap();
        assert_eq!((drafts, other), (6, 0));
        let linked: i64 = c
            .query_row("SELECT COUNT(*) FROM campaign_messages WHERE campaign_id = ?1 AND press_url LIKE 'https://insrt.studio/p/pk1-%'", [r.campaign_id], |x| x.get(0))
            .unwrap();
        assert_eq!(linked, 6);
        let sent: i64 = c.query_row("SELECT COUNT(*) FROM emails", [], |x| x.get(0)).unwrap();
        assert_eq!(sent, 0);

        // The zone is covered now: a second click is refused, and the source
        // points at the new campaign's drafts instead of a new zone.
        assert!(create(&c, 1, zone, None, None, None).is_err());
        match proposal(&c, 1) {
            Some(Proposal::Review { campaign_id, drafts, .. }) => assert_eq!((campaign_id, drafts), (r.campaign_id, 6)),
            other => panic!("{:?}", other),
        }
        assert!(suggestions(&c, 1).unwrap().is_empty());
        // Once its autopilot is on, the source may propose the next zone again.
        c.execute("UPDATE residency_campaigns SET autopilot = 1 WHERE campaign_id = ?1", [r.campaign_id]).unwrap();
        assert!(proposal(&c, 1).is_none(), "nothing left to open");
    }
}
