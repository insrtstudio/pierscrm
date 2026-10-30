//! "Construire la cible": the clubs of the campaign's zone worth pitching a
//! residency to, from Discovery (vi_venues) plus the venue contacts already in
//! the pipeline, each with exactly one address and the reasons it is listed.

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

use super::zone::{iso_country, language_for, pays_values};
use super::Config;
use crate::commands::harvest::own_email_sql;
use crate::db::normalise;

/// Days during which an address already mailed (by any campaign) is left alone.
pub const RECENT_CONTACT_DAYS: i64 = 90;

// ---------------- Best email per venue (pure) ----------------

const BOOKING_WORDS: &[&str] = &[
    "booking", "bookings", "book", "programmation", "prog", "programming", "programmer", "programmateur",
    "programmatrice", "artist", "artists", "artiste", "artistes", "talent", "talents", "music", "musique",
    "lineup", "dj", "djs", "agenda",
];
const GENERIC_WORDS: &[&str] = &[
    "info", "infos", "information", "contact", "contacts", "hello", "bonjour", "hi", "hallo", "hola", "team",
    "mail", "office", "general", "club", "welcome",
];
const OFF_TOPIC_WORDS: &[&str] = &[
    "presse", "press", "com", "communication", "reservation", "reservations", "resa", "reserve", "accueil",
    "mediation", "librairie", "rh", "hr", "jobs", "job", "recrutement", "careers", "compta", "comptabilite",
    "accounting", "location", "privatisation", "privat", "private", "evenementiel", "ticket", "tickets",
    "billetterie", "shop", "boutique", "marketing", "partenariat", "partners", "sponsoring", "media",
];

/// How good an address is for a booking pitch: 0 (booking/programmation) to
/// 4 (press, reservations, HR...). Lower is better.
pub fn email_rank(addr: &str, person: Option<&str>) -> u8 {
    let local = addr.split('@').next().unwrap_or("").to_lowercase();
    let tokens: Vec<&str> = local.split(|c: char| !c.is_ascii_alphanumeric()).filter(|t| !t.is_empty()).collect();
    let has = |words: &[&str]| tokens.iter().any(|t| words.contains(t));
    if has(BOOKING_WORDS) || local.contains("booking") || local.contains("programmation") || local.starts_with("prog") {
        return 0;
    }
    if has(OFF_TOPIC_WORDS) {
        return 4;
    }
    if has(GENERIC_WORDS) {
        return 2;
    }
    // A named person (scraped name, or a "first.last" / "f.last" address).
    let looks_personal = tokens.len() == 2 && tokens.iter().all(|t| t.chars().all(|c| c.is_ascii_alphabetic()));
    if person.map_or(false, |p| !p.trim().is_empty()) || looks_personal {
        return 1;
    }
    3
}

fn domain_of(s: &str) -> String {
    let s = s.trim().to_lowercase();
    let s = s.split('@').nth(1).map(str::to_string).unwrap_or(s);
    let s = s.trim_start_matches("https://").trim_start_matches("http://").trim_start_matches("www.");
    s.split('/').next().unwrap_or("").to_string()
}

#[derive(Debug, Clone)]
pub struct EmailCand {
    pub addr: String,
    pub score: i64,
    pub person: Option<String>,
}

/// Website builders, ticketing and listing platforms: an address on these
/// domains was scraped off the platform's own page, never the club's inbox
/// (seen in the real data: contact@webflow.com, promotersupport@ra.co).
const PLATFORM_DOMAINS: &[&str] = &[
    "webflow.com", "ra.co", "residentadvisor.net", "wix.com", "wixpress.com", "squarespace.com", "shopify.com",
    "eventbrite.com", "eventbrite.fr", "eventbrite.co.uk", "dice.fm", "shotgun.live", "skiddle.com", "fatsoma.com",
    "xceed.me", "feverup.com", "billetweb.fr", "weezevent.com", "ticketmaster.fr", "ticketmaster.co.uk",
    "seetickets.com", "facebook.com", "instagram.com", "google.com", "wordpress.com", "godaddy.com", "ovh.net",
    "sentry.io", "mailchimp.com", "hubspot.com",
];

pub fn is_platform_address(addr: &str) -> bool {
    let d = domain_of(addr);
    PLATFORM_DOMAINS.iter().any(|p| d == *p || d.ends_with(&format!(".{}", p)))
}

/// Exactly one address per venue: best rank first, then an address on the
/// venue's own website domain, then the enrichment score. Platform addresses
/// are never picked.
pub fn best_email<'a>(cands: &'a [EmailCand], site: Option<&str>) -> Option<&'a EmailCand> {
    let site_dom = site.map(domain_of).filter(|d| !d.is_empty());
    cands.iter().filter(|c| !is_platform_address(&c.addr)).min_by_key(|c| {
        let off_site = match &site_dom {
            Some(d) => !(domain_of(&c.addr) == *d || d.ends_with(&domain_of(&c.addr))),
            None => true,
        };
        (email_rank(&c.addr, c.person.as_deref()), off_site, -c.score, c.addr.clone())
    })
}

/// First name for {{first_name}}: the first word of a scraped person name.
pub fn first_name_of(person: Option<&str>) -> Option<String> {
    let p = person?.trim();
    let first = p.split_whitespace().next()?;
    (first.chars().count() >= 2 && first.chars().all(|c| c.is_alphabetic() || c == '-'))
        .then(|| first.to_string())
}

// ---------------- Builder ----------------

#[derive(Debug, Clone, Deserialize, Default)]
pub struct BuildOptions {
    #[serde(default)]
    pub capacity_min: Option<i64>,
    #[serde(default)]
    pub capacity_max: Option<i64>,
    /// Venues with no capacity estimate are listed (flagged) unless false.
    #[serde(default)]
    pub include_unknown_capacity: Option<bool>,
    #[serde(default)]
    pub min_events: Option<i64>,
    #[serde(default)]
    pub limit: Option<i64>,
    /// Also list promoters / collectives of the zone (promoters.rs).
    #[serde(default)]
    pub include_promoters: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Candidate {
    /// "v:<venue id>" or "c:<contact id>".
    pub key: String,
    pub venue_id: Option<i64>,
    pub contact_id: Option<i64>,
    pub name: String,
    pub city: Option<String>,
    pub country: String,
    pub capacity: Option<i64>,
    pub capacity_unknown: bool,
    pub events: i64,
    pub score_qualif: i64,
    pub rank: f64,
    /// Reference artists already booked there (vi_evidence), most frequent first.
    pub artists: Vec<String>,
    pub artist_count: i64,
    pub email: String,
    /// 0 booking ... 4 off-topic (see email_rank).
    pub email_rank: u8,
    pub first_name: Option<String>,
    pub lang: String,
    /// "discovery" | "pipeline" | "promoter"
    pub source: String,
    /// Discovery promoter (vi_promoter_orgs) when source is "promoter".
    pub promoter_id: Option<i64>,
    /// "Books artists like ours" score (fit.rs), when computed.
    pub fit_score: Option<i64>,
    /// "a programmé X, Y ; 14 soirées house en 12 mois".
    pub fit_explain: Option<String>,
    /// "Bon moment pour écrire" (timing.rs).
    pub timing: crate::commands::timing::Timing,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct BuildResult {
    pub candidates: Vec<Candidate>,
    /// Why matching venues were left out.
    pub excluded_suppressed: i64,
    pub excluded_recent: i64,
    pub excluded_declined: i64,
    pub excluded_in_audience: i64,
    pub excluded_placeholder: i64,
    pub excluded_no_email: i64,
}

enum Skip {
    Suppressed,
    Recent,
    Declined,
}

/// Why an address / contact must not be pitched now, if any.
fn skip_reason(conn: &Connection, addr: &str, contact_id: Option<i64>) -> Option<Skip> {
    let a = addr.trim().to_lowercase();
    if crate::commands::outbox::suppression_reason(conn, &a).is_some() {
        return Some(Skip::Suppressed);
    }
    let declined: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM contacts WHERE status = 'declined'
                AND (id = ?2 OR (email IS NOT NULL AND lower(trim(email)) = ?1)))",
            params![a, contact_id],
            |r| r.get(0),
        )
        .unwrap_or(false);
    if declined {
        return Some(Skip::Declined);
    }
    let recent: bool = conn
        .query_row(
            &format!(
                "SELECT EXISTS(SELECT 1 FROM emails WHERE status = 'sent'
                    AND sent_at >= datetime('now', '-{} days')
                    AND (lower(trim(to_addr)) = ?1 OR (?2 IS NOT NULL AND contact_id = ?2)))",
                RECENT_CONTACT_DAYS
            ),
            params![a, contact_id],
            |r| r.get(0),
        )
        .unwrap_or(false);
    recent.then_some(Skip::Recent)
}

/// Discovery rows that are not a club room: hotels, breweries, universities,
/// festivals, and "venues" whose name is a street address (RA's fallback when a
/// promoter types the address instead of a room).
pub(crate) fn not_a_club(nom: &str) -> bool {
    let words = nom.split(|c: char| !c.is_alphanumeric()).map(crate::db::normalise).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ");
    let n = format!(" {} ", words);
    const WORDS: &[&str] = &[
        " hotel ", " hostel ", " brewery ", " brouwerij ", " beer co ", " university ", " universite ",
        " universiteit ", " vub ", " ulb ", " campus ", " festival ", " jazz ",
    ];
    if WORDS.iter().any(|w| n.contains(w)) {
        return true;
    }
    static ADDRESS: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    ADDRESS
        .get_or_init(|| {
            regex::Regex::new(r"(?i)(\b\d{4,5}\b.*\b[a-z]{3,}\b|\b(rue|avenue|boulevard|chaussee|straat|street|road|strasse|laan|weg|plein|place)\b.*\d|\d+\s*,?\s*(rue|avenue|boulevard|street|road)\b|^(avenue|rue|boulevard|chaussee)\b|straat\s*\d)")
                .unwrap()
        })
        .is_match(&words)
}

fn rank_of(score_qualif: i64, artists: i64, events: i64) -> f64 {
    score_qualif as f64 + 12.0 * artists as f64 + (events.min(400) as f64) / 8.0
}

/// Weight of the fit score (0-100) in the ranking. A venue without a score
/// keeps exactly its previous rank.
pub const FIT_WEIGHT: f64 = 1.0;

pub fn rank_with_fit(base: f64, fit: Option<i64>) -> f64 {
    base + FIT_WEIGHT * fit.unwrap_or(0) as f64
}

fn timing_fields(lead: Option<i64>, profile: Option<&str>) -> crate::commands::timing::Timing {
    crate::commands::timing::window_for_row(chrono::Utc::now().date_naive(), lead, profile)
}

fn in_audience(conn: &Connection, campaign_id: i64, contact_id: Option<i64>) -> bool {
    contact_id.map_or(false, |cid| {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM campaign_contacts WHERE campaign_id = ?1 AND contact_id = ?2)",
            params![campaign_id, cid],
            |r| r.get(0),
        )
        .unwrap_or(false)
    })
}

pub fn build(conn: &Connection, campaign_id: i64, cfg: &Config, opts: &BuildOptions) -> Result<BuildResult, String> {
    let (cities, countries) = cfg.zone.resolve();
    if cities.is_empty() && countries.is_empty() {
        return Err("Zone cible vide : choisissez une ville ou un pays.".into());
    }
    let cap_min = opts.capacity_min.unwrap_or(cfg.capacity_min);
    let cap_max = opts.capacity_max.unwrap_or(cfg.capacity_max).max(cap_min);
    let unknown_ok = opts.include_unknown_capacity.unwrap_or(true);
    let min_events = opts.min_events.unwrap_or(cfg.min_events).max(0);
    let limit = opts.limit.unwrap_or(150).clamp(1, 1000) as usize;

    let mut zone_parts: Vec<String> = Vec::new();
    let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    if !cities.is_empty() {
        zone_parts.push(format!("v.ville_norm IN ({})", vec!["?"; cities.len()].join(",")));
        for c in &cities {
            args.push(Box::new(c.clone()));
        }
    }
    let pays: Vec<String> = countries.iter().flat_map(|c| pays_values(c)).collect();
    if !pays.is_empty() {
        zone_parts.push(format!("v.pays IN ({})", vec!["?"; pays.len()].join(",")));
        for p in &pays {
            args.push(Box::new(p.clone()));
        }
    }
    let cap_cond = if unknown_ok {
        format!("(v.capacite_est IS NULL OR v.capacite_est BETWEEN {cap_min} AND {cap_max})")
    } else {
        format!("v.capacite_est BETWEEN {cap_min} AND {cap_max}")
    };
    let sql = format!(
        "SELECT v.id, v.nom, v.ville, v.pays, v.capacite_est, v.nb_events_periode, v.score_qualif,
                v.crm_contact_id, v.site_web,
                EXISTS(SELECT 1 FROM vi_contacts c WHERE c.venue_id = v.id AND c.type = 'email' AND {own}),
                v.fit_score, v.fit_detail, v.lead_weeks, v.month_profile
         FROM vi_venues v
         WHERE v.statut IN ('qualifie','valide') AND ({zone}) AND {cap} AND v.nb_events_periode >= {min_events}",
        own = own_email_sql("c.valeur"),
        zone = zone_parts.join(" OR "),
        cap = cap_cond,
    );
    type Row = (
        i64, String, Option<String>, Option<String>, Option<i64>, i64, i64, Option<i64>, Option<String>, bool,
        (Option<i64>, Option<String>, Option<i64>, Option<String>),
    );
    let rows: Vec<Row> = {
        let mut st = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();
        let it = st
            .query_map(refs.as_slice(), |r| {
                Ok((
                    r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?,
                    r.get::<_, i64>(9)? != 0,
                    (r.get(10)?, r.get(11)?, r.get(12)?, r.get(13)?),
                ))
            })
            .map_err(|e| e.to_string())?;
        it.collect::<Result<_, _>>().map_err(|e| e.to_string())?
    };

    let mut out = BuildResult::default();
    let mut seen_addr: HashSet<String> = HashSet::new();
    let email_sql = format!(
        "SELECT valeur, score, personne_nom FROM vi_contacts c
         WHERE c.venue_id = ?1 AND c.type = 'email' AND {}",
        own_email_sql("c.valeur")
    );
    let mut email_st = conn.prepare(&email_sql).map_err(|e| e.to_string())?;
    let mut art_st = conn
        .prepare(
            "SELECT artiste, COUNT(*) FROM vi_evidence WHERE venue_id = ?1
             GROUP BY lower(artiste) ORDER BY COUNT(*) DESC, MAX(date_event) DESC",
        )
        .map_err(|e| e.to_string())?;

    for (id, nom, ville, pays, cap, events, score, crm, site, has_email, (fit, fit_detail, lead, profile)) in rows {
        if crate::commands::vi::is_placeholder_venue(&nom, ville.as_deref()) || not_a_club(&nom) {
            out.excluded_placeholder += 1;
            continue;
        }
        if !has_email {
            out.excluded_no_email += 1;
            continue;
        }
        if in_audience(conn, campaign_id, crm) {
            out.excluded_in_audience += 1;
            continue;
        }
        let cands: Vec<EmailCand> = email_st
            .query_map(params![id], |r| Ok(EmailCand { addr: r.get(0)?, score: r.get(1)?, person: r.get(2)? }))
            .map(|it| it.flatten().collect())
            .unwrap_or_default();
        let Some(best) = best_email(&cands, site.as_deref()) else {
            out.excluded_no_email += 1;
            continue;
        };
        match skip_reason(conn, &best.addr, crm) {
            Some(Skip::Suppressed) => {
                out.excluded_suppressed += 1;
                continue;
            }
            Some(Skip::Recent) => {
                out.excluded_recent += 1;
                continue;
            }
            Some(Skip::Declined) => {
                out.excluded_declined += 1;
                continue;
            }
            None => {}
        }
        if !seen_addr.insert(best.addr.trim().to_lowercase()) {
            // One promoter inbox for several rooms: pitch it once.
            continue;
        }
        let artists: Vec<String> = art_st
            .query_map(params![id], |r| r.get::<_, String>(0))
            .map(|it| it.flatten().collect())
            .unwrap_or_default();
        let country = iso_country(pays.as_deref().unwrap_or(""));
        let timing = timing_fields(lead, profile.as_deref());
        out.candidates.push(Candidate {
            key: format!("v:{}", id),
            venue_id: Some(id),
            contact_id: crm,
            lang: language_for(&country, ville.as_deref(), &cfg.lang).to_string(),
            name: nom,
            city: ville,
            country,
            capacity: cap,
            capacity_unknown: cap.is_none(),
            events,
            score_qualif: score,
            rank: rank_with_fit(rank_of(score, artists.len() as i64, events), fit),
            artist_count: artists.len() as i64,
            artists: artists.into_iter().take(4).collect(),
            email: best.addr.trim().to_string(),
            email_rank: email_rank(&best.addr, best.person.as_deref()),
            first_name: first_name_of(best.person.as_deref()),
            source: "discovery".into(),
            promoter_id: None,
            fit_score: fit,
            fit_explain: crate::commands::fit::explain_stored(fit_detail.as_deref()),
            timing,
        });
    }

    // Venue contacts already in the pipeline that Discovery does not cover
    // (typed by hand, imported): matched on their city.
    let with_promoters = opts.include_promoters == Some(true);
    if with_promoters {
        promoter_candidates(conn, campaign_id, cfg, &cities, &pays, &mut seen_addr, &mut out)?;
    }
    pipeline_candidates(conn, campaign_id, cfg, &cities, &countries, with_promoters, &mut seen_addr, &mut out)?;

    out.candidates.sort_by(|a, b| b.rank.partial_cmp(&a.rank).unwrap_or(std::cmp::Ordering::Equal));
    out.candidates.truncate(limit);
    Ok(out)
}

/// Least nights (12 months if known, else harvested total) for a promoter to
/// be listed: a collective with one or two parties is not a residency lead.
pub const PROMOTER_MIN_EVENTS: i64 = 3;

/// Discovery promoters of the zone (main city in the zone's cities, or
/// country in its countries) with an address of their own. Capacity does not
/// apply: a collective rents rooms of every size.
fn promoter_candidates(
    conn: &Connection,
    campaign_id: i64,
    cfg: &Config,
    cities: &[String],
    pays: &[String],
    seen_addr: &mut HashSet<String>,
    out: &mut BuildResult,
) -> Result<(), String> {
    let mut zone_parts: Vec<String> = Vec::new();
    let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    if !cities.is_empty() {
        zone_parts.push(format!("o.ville_norm IN ({})", vec!["?"; cities.len()].join(",")));
        for c in cities {
            args.push(Box::new(c.clone()));
        }
    }
    if !pays.is_empty() {
        zone_parts.push(format!("o.pays IN ({})", vec!["?"; pays.len()].join(",")));
        for p in pays {
            args.push(Box::new(p.clone()));
        }
    }
    if zone_parts.is_empty() {
        return Ok(());
    }
    let sql = format!(
        "SELECT o.id, o.nom, o.ville, o.pays, o.crm_contact_id, MAX(o.nb_events_12m, o.nb_events_total), o.site_web,
                o.fit_score, o.fit_detail, o.lead_weeks, o.month_profile
         FROM vi_promoter_orgs o
         WHERE o.statut = 'actif' AND o.house_promoter = 0 AND ({zone})
           AND MAX(o.nb_events_12m, o.nb_events_total) >= {PROMOTER_MIN_EVENTS}",
        zone = zone_parts.join(" OR "),
    );
    type Row = (
        i64, String, Option<String>, Option<String>, Option<i64>, i64, Option<String>,
        Option<i64>, Option<String>, Option<i64>, Option<String>,
    );
    let rows: Vec<Row> = {
        let mut st = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();
        let it = st
            .query_map(refs.as_slice(), |r| {
                Ok((
                    r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?,
                    r.get(9)?, r.get(10)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        it.collect::<Result<_, _>>().map_err(|e| e.to_string())?
    };
    let mut email_st = conn
        .prepare("SELECT valeur, score FROM vi_promoter_contacts WHERE promoter_id = ?1 AND type = 'email'")
        .map_err(|e| e.to_string())?;
    for (id, nom, ville, pays, crm, events, site, fit, fit_detail, lead, profile) in rows {
        if in_audience(conn, campaign_id, crm) {
            out.excluded_in_audience += 1;
            continue;
        }
        let cands: Vec<EmailCand> = email_st
            .query_map(params![id], |r| Ok(EmailCand { addr: r.get(0)?, score: r.get(1)?, person: None }))
            .map(|it| it.flatten().collect())
            .unwrap_or_default();
        let Some(best) = best_email(&cands, site.as_deref()) else {
            out.excluded_no_email += 1;
            continue;
        };
        match skip_reason(conn, &best.addr, crm) {
            Some(Skip::Suppressed) => {
                out.excluded_suppressed += 1;
                continue;
            }
            Some(Skip::Recent) => {
                out.excluded_recent += 1;
                continue;
            }
            Some(Skip::Declined) => {
                out.excluded_declined += 1;
                continue;
            }
            None => {}
        }
        if !seen_addr.insert(best.addr.trim().to_lowercase()) {
            continue;
        }
        let country = iso_country(pays.as_deref().unwrap_or(""));
        let timing = timing_fields(lead, profile.as_deref());
        let matched = crate::commands::fit::matched_artists(fit_detail.as_deref());
        out.candidates.push(Candidate {
            key: format!("p:{}", id),
            venue_id: None,
            contact_id: crm,
            lang: language_for(&country, ville.as_deref(), &cfg.lang).to_string(),
            name: nom,
            city: ville,
            country,
            capacity: None,
            capacity_unknown: false,
            events,
            score_qualif: 0,
            rank: rank_with_fit(12.0 * matched.len() as f64 + (events.min(400) as f64) / 8.0, fit),
            artist_count: matched.len() as i64,
            artists: matched.into_iter().take(4).collect(),
            email: best.addr.trim().to_string(),
            email_rank: email_rank(&best.addr, None),
            first_name: None,
            source: "promoter".into(),
            promoter_id: Some(id),
            fit_score: fit,
            fit_explain: crate::commands::fit::explain_stored(fit_detail.as_deref()),
            timing,
        });
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn pipeline_candidates(
    conn: &Connection,
    campaign_id: i64,
    cfg: &Config,
    cities: &[String],
    countries: &[String],
    with_promoters: bool,
    seen_addr: &mut HashSet<String>,
    out: &mut BuildResult,
) -> Result<(), String> {
    // Cities of the zone's countries, as Discovery knows them.
    let mut city_set: HashSet<String> = cities.iter().cloned().collect();
    let mut country_of_city: std::collections::HashMap<String, String> = Default::default();
    for c in countries {
        for p in pays_values(c) {
            let mut st = conn
                .prepare("SELECT DISTINCT ville_norm FROM vi_venues WHERE pays = ?1 AND ville_norm IS NOT NULL")
                .map_err(|e| e.to_string())?;
            let it = st.query_map(params![p], |r| r.get::<_, String>(0)).map_err(|e| e.to_string())?;
            for v in it.flatten() {
                country_of_city.insert(v.clone(), c.clone());
                city_set.insert(v);
            }
        }
    }
    let default_country = cfg.zone.default_country();
    let mut st = conn
        .prepare(
            &format!(
                "SELECT c.id, c.name, c.area, c.email, c.promoter FROM contacts c
                 WHERE c.category IN ({cats}) AND c.email IS NOT NULL AND trim(c.email) <> ''
                   AND c.status NOT IN ('declined','confirmed')
                   AND NOT EXISTS (SELECT 1 FROM vi_venues v WHERE v.crm_contact_id = c.id)
                   AND NOT EXISTS (SELECT 1 FROM vi_promoter_orgs o WHERE o.crm_contact_id = c.id)",
                cats = if with_promoters { "'venue','promoter'" } else { "'venue'" }
            ),
        )
        .map_err(|e| e.to_string())?;
    let rows: Vec<(i64, String, Option<String>, String, Option<String>)> = st
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))
        .map_err(|e| e.to_string())?
        .flatten()
        .collect();
    for (id, name, area, email, promoter) in rows {
        let folded = area.as_deref().map(normalise).unwrap_or_default();
        if folded.is_empty() || !city_set.contains(&folded) {
            continue;
        }
        if in_audience(conn, campaign_id, Some(id)) {
            out.excluded_in_audience += 1;
            continue;
        }
        let addr = email.trim().to_lowercase();
        match skip_reason(conn, &addr, Some(id)) {
            Some(Skip::Suppressed) => {
                out.excluded_suppressed += 1;
                continue;
            }
            Some(Skip::Recent) => {
                out.excluded_recent += 1;
                continue;
            }
            Some(Skip::Declined) => {
                out.excluded_declined += 1;
                continue;
            }
            None => {}
        }
        if !seen_addr.insert(addr) {
            continue;
        }
        let country = country_of_city.get(&folded).cloned().unwrap_or_else(|| default_country.clone());
        out.candidates.push(Candidate {
            key: format!("c:{}", id),
            venue_id: None,
            contact_id: Some(id),
            lang: language_for(&country, area.as_deref(), &cfg.lang).to_string(),
            name,
            city: area,
            country,
            capacity: None,
            capacity_unknown: true,
            events: 0,
            score_qualif: 0,
            rank: 20.0,
            artists: Vec::new(),
            artist_count: 0,
            email_rank: email_rank(&email, promoter.as_deref()),
            email: email.trim().to_string(),
            first_name: first_name_of(promoter.as_deref()),
            source: "pipeline".into(),
            promoter_id: None,
            fit_score: None,
            fit_explain: None,
            timing: timing_fields(None, None),
        });
    }
    Ok(())
}

// ---------------- Add to the audience ----------------

#[derive(Debug, Clone, Deserialize)]
pub struct Pick {
    #[serde(default)]
    pub venue_id: Option<i64>,
    #[serde(default)]
    pub contact_id: Option<i64>,
    pub email: String,
    #[serde(default)]
    pub first_name: Option<String>,
    #[serde(default)]
    pub promoter_id: Option<i64>,
}

/// Promote picked venues into contacts when needed (with the chosen address),
/// then add everyone to the campaign audience as "planned".
pub fn add_targets(conn: &Connection, campaign_id: i64, picks: &[Pick]) -> Result<usize, String> {
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    let mut n = 0;
    for p in picks {
        let cid = match (p.contact_id, p.venue_id, p.promoter_id) {
            (Some(c), _, _) => c,
            (None, Some(v), _) => crate::commands::harvest::promote_venue_inner(&tx, v)?,
            (None, None, Some(o)) => crate::commands::promoters::promote_promoter_inner(&tx, o)?,
            (None, None, None) => continue,
        };
        // The chosen address replaces the promotion's pick as long as nothing
        // was sent to the contact yet.
        let email = p.email.trim();
        if !email.is_empty() {
            let _ = tx.execute(
                "UPDATE contacts SET email = ?2, updated_at = datetime('now')
                 WHERE id = ?1 AND lower(trim(COALESCE(email,''))) <> lower(?2)
                   AND NOT EXISTS (SELECT 1 FROM emails m WHERE m.contact_id = ?1 AND m.status = 'sent')",
                params![cid, email],
            );
        }
        if let Some(f) = p.first_name.as_deref().filter(|f| !f.trim().is_empty()) {
            let _ = tx.execute(
                "UPDATE contacts SET promoter = ?2 WHERE id = ?1 AND (promoter IS NULL OR trim(promoter) = '')",
                params![cid, f.trim()],
            );
        }
        n += tx
            .execute(
                "INSERT OR IGNORE INTO campaign_contacts (campaign_id, contact_id, state) VALUES (?1, ?2, 'planned')",
                params![campaign_id, cid],
            )
            .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(n)
}

// ---------------- What we know about one target ----------------

#[derive(Debug, Clone, Serialize, Default)]
pub struct TargetInfo {
    pub contact_id: i64,
    pub name: String,
    pub venue: String,
    pub city: String,
    pub country: String,
    pub lang: String,
    pub email: String,
    pub first_name: Option<String>,
    pub promoter: Option<String>,
    pub capacity: Option<i64>,
    pub events: i64,
    pub venue_type: Option<String>,
    /// (artist, last date played) from vi_evidence.
    pub artists: Vec<(String, String)>,
    pub top_promoter: Option<String>,
    pub website: Option<String>,
}

pub fn target_info(conn: &Connection, contact_id: i64, cfg: &Config) -> Option<TargetInfo> {
    let (name, venue, area, email, promoter, website): (String, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>) = conn
        .query_row(
            "SELECT name, venue, area, email, promoter, website FROM contacts WHERE id = ?1",
            params![contact_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )
        .ok()?;
    let mut info = TargetInfo {
        contact_id,
        venue: venue.clone().filter(|v| !v.trim().is_empty()).unwrap_or_else(|| name.clone()),
        name,
        city: area.clone().unwrap_or_default(),
        email: email.unwrap_or_default(),
        first_name: first_name_of(promoter.as_deref()),
        promoter,
        website,
        ..Default::default()
    };
    let venue_row: Option<(i64, Option<String>, Option<String>, Option<i64>, i64, Option<String>)> = conn
        .query_row(
            "SELECT id, ville, pays, capacite_est, nb_events_periode, type FROM vi_venues WHERE crm_contact_id = ?1 LIMIT 1",
            params![contact_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )
        .ok();
    let mut country = cfg.zone.default_country();
    if let Some((vid, ville, pays, cap, events, vtype)) = venue_row {
        if let Some(v) = ville.filter(|v| !v.trim().is_empty()) {
            info.city = v;
        }
        if let Some(p) = pays.filter(|p| !p.trim().is_empty()) {
            country = iso_country(&p);
        }
        info.capacity = cap;
        info.events = events;
        info.venue_type = vtype;
        if let Ok(mut st) = conn.prepare(
            "SELECT artiste, MAX(date_event) FROM vi_evidence WHERE venue_id = ?1
             GROUP BY lower(artiste) ORDER BY COUNT(*) DESC, MAX(date_event) DESC LIMIT 5",
        ) {
            info.artists = st
                .query_map(params![vid], |r| Ok((r.get(0)?, r.get(1)?)))
                .map(|it| it.flatten().collect())
                .unwrap_or_default();
        }
        info.top_promoter = conn
            .query_row(
                "SELECT nom FROM vi_promoters WHERE venue_id = ?1 ORDER BY nb_events DESC LIMIT 1",
                params![vid],
                |r| r.get(0),
            )
            .ok();
    }
    info.lang = language_for(&country, Some(&info.city), &cfg.lang).to_string();
    info.country = country;
    Some(info)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::residency::testdb;
    use crate::commands::residency::zone::Zone;

    fn cand(a: &str, score: i64) -> EmailCand {
        EmailCand { addr: a.into(), score, person: None }
    }

    #[test]
    fn best_email_prefers_booking_over_generic_and_press() {
        let c = vec![cand("info@club.fr", 60), cand("presse@club.fr", 80), cand("booking@club.fr", 10)];
        assert_eq!(best_email(&c, Some("https://club.fr")).unwrap().addr, "booking@club.fr");
        let c = vec![cand("contact@club.fr", 30), cand("reservation@club.fr", 90), cand("prog.paris@club.fr", 5)];
        assert_eq!(best_email(&c, None).unwrap().addr, "prog.paris@club.fr");
        // Generic beats press and reservations.
        let c = vec![cand("presse@palais.com", 60), cand("contact@palais.com", 20), cand("reservation@palais.com", 5)];
        assert_eq!(best_email(&c, None).unwrap().addr, "contact@palais.com");
        // Same rank: the venue's own domain wins over an agency, then the score.
        let c = vec![cand("hello@agency.com", 90), cand("hello@glazart.com", 40)];
        assert_eq!(best_email(&c, Some("www.glazart.com")).unwrap().addr, "hello@glazart.com");
        let c = vec![cand("contact@webflow.com", 90), cand("promotersupport@ra.co", 90)];
        assert!(best_email(&c, None).is_none());
        assert_eq!(email_rank("a.mabille@paris-society.com", None), 1);
        assert_eq!(email_rank("talent@x.com", None), 0);
        assert_eq!(email_rank("lesamis@x.com", None), 3);
        assert_eq!(first_name_of(Some("Barbara Dupont")), Some("Barbara".into()));
        assert_eq!(first_name_of(Some(" ")), None);
    }

    #[test]
    fn builder_on_memory_db() {
        let c = testdb::conn();
        c.execute_batch(
            "INSERT INTO campaigns (id, name, kind) VALUES (1, 'R', 'residency');
             INSERT INTO vi_venues (id, nom, nom_normalise, ville, ville_norm, pays, statut, capacite_est, nb_events_periode, score_qualif)
             VALUES (1, 'Club A', 'cluba', 'Paris', 'paris', 'FR', 'qualifie', 400, 120, 60),
                    (2, 'Club B', 'clubb', 'Paris', 'paris', 'FR', 'valide', NULL, 50, 40),
                    (3, 'Arena', 'arena', 'Paris', 'paris', 'FR', 'qualifie', 5000, 300, 90),
                    (4, 'Club C', 'clubc', 'Lyon', 'lyon', 'FR', 'qualifie', 300, 80, 70),
                    (5, 'Club D', 'clubd', 'Pantin', 'pantin', 'FR', 'qualifie', 200, 3, 50),
                    (6, 'TBA', 'tba', 'Paris', 'paris', 'FR', 'qualifie', 300, 500, 99),
                    (7, 'Club E', 'clube', 'Paris', 'paris', 'FR', 'qualifie', 300, 90, 55),
                    (8, 'Club F', 'clubf', 'Brussels', 'brussels', 'BE', 'qualifie', 300, 90, 55);
             INSERT INTO vi_contacts (venue_id, type, valeur, score, source_url) VALUES
                (1, 'email', 'info@cluba.fr', 50, 'x'), (1, 'email', 'booking@cluba.fr', 10, 'x'),
                (2, 'email', 'contact@clubb.fr', 30, 'x'),
                (3, 'email', 'booking@arena.fr', 30, 'x'),
                (4, 'email', 'booking@clubc.fr', 30, 'x'),
                (5, 'email', 'booking@clubd.fr', 30, 'x'),
                (6, 'email', 'tba@tba.fr', 30, 'x'),
                (7, 'email', 'gone@clube.fr', 30, 'x'),
                (8, 'email', 'booking@clubf.be', 30, 'x');
             INSERT INTO vi_evidence (venue_id, artiste, date_event, source_url) VALUES
                (2, 'Artist X', '2026-05-01', 'u'), (2, 'Artist Y', '2026-06-01', 'u'), (2, 'Artist Z', '2026-07-01', 'u');
             INSERT INTO suppressions (address, reason) VALUES ('gone@clube.fr', 'bounce');
             INSERT INTO contacts (id, category, name, area, email, status) VALUES
                (50, 'venue', 'Bar Manuel', 'Paris', 'prog@barmanuel.fr', 'to_contact'),
                (51, 'venue', 'Bar Lyon', 'Lyon', 'prog@barlyon.fr', 'to_contact'),
                (52, 'venue', 'Bar Refus', 'Paris', 'x@refus.fr', 'declined');",
        )
        .unwrap();
        let cfg = Config::default();
        let r = build(&c, 1, &cfg, &BuildOptions::default()).unwrap();
        let keys: Vec<&str> = r.candidates.iter().map(|x| x.key.as_str()).collect();
        // A (capacity in range), B (unknown capacity, flagged, 3 matching artists), the pipeline bar.
        assert_eq!(keys.len(), 3, "{:?}", keys);
        assert!(keys.contains(&"v:1") && keys.contains(&"v:2") && keys.contains(&"c:50"));
        let a = r.candidates.iter().find(|x| x.key == "v:1").unwrap();
        assert_eq!(a.email, "booking@cluba.fr");
        assert_eq!(a.lang, "fr");
        let b = r.candidates.iter().find(|x| x.key == "v:2").unwrap();
        assert!(b.capacity_unknown);
        assert_eq!(b.artist_count, 3);
        assert_eq!(r.excluded_suppressed, 1);
        assert_eq!(r.excluded_placeholder, 1);
        for bad in ["Meininger Hotel Brussels City Center", "La Source Beer Co", "Pilar - VUB", "Avenue des Genets",
                    "Sint-Jacobsnieuwstraat 30, 9000 Gent, België", "Jazz Middelheim"] {
            assert!(not_a_club(bad), "{bad}");
        }
        for ok in ["Funke", "Club Vaag", "AB Club (Ancienne Belgique)", "Brasserie Illegaal", "Le Botanique Witloof Bar", "Rex Club", "La Rotonde Stalingrad", "Péniche Marcounet", "Nouveau Casino", "Fuse"] {
            assert!(!not_a_club(ok), "{ok}");
        }

        // Belgium preset: Brussels in French.
        let mut be = Config::default();
        be.zone = Zone { presets: vec!["belgium".into()], ..Default::default() };
        let r = build(&c, 1, &be, &BuildOptions::default()).unwrap();
        assert_eq!(r.candidates.len(), 1);
        assert_eq!(r.candidates[0].lang, "fr");
        assert_eq!(r.candidates[0].country, "BE");

        // Adding promotes the venue and plans the audience; a second build skips it.
        let r = build(&c, 1, &cfg, &BuildOptions::default()).unwrap();
        let picks: Vec<Pick> = r
            .candidates
            .iter()
            .map(|x| Pick { venue_id: x.venue_id, contact_id: x.contact_id, email: x.email.clone(), first_name: None, promoter_id: x.promoter_id })
            .collect();
        assert_eq!(add_targets(&c, 1, &picks).unwrap(), 3);
        let email: String = c
            .query_row("SELECT k.email FROM contacts k JOIN vi_venues v ON v.crm_contact_id = k.id WHERE v.id = 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(email, "booking@cluba.fr");
        let r = build(&c, 1, &cfg, &BuildOptions::default()).unwrap();
        assert!(r.candidates.is_empty());
        assert_eq!(r.excluded_in_audience, 3);
        let cid: i64 = c.query_row("SELECT crm_contact_id FROM vi_venues WHERE id = 2", [], |r| r.get(0)).unwrap();
        let info = target_info(&c, cid, &cfg).unwrap();
        assert_eq!(info.artists.len(), 3);
        assert_eq!(info.country, "FR");
    }

    #[test]
    fn fit_ranks_and_promoters_join_on_request() {
        let c = testdb::conn();
        c.execute_batch(
            "INSERT INTO campaigns (id, name, kind) VALUES (1, 'R', 'residency');
             INSERT INTO vi_venues (id, nom, nom_normalise, ville, ville_norm, pays, statut, capacite_est, nb_events_periode, score_qualif, fit_score, fit_detail)
             VALUES (1, 'Club A', 'cluba', 'Paris', 'paris', 'FR', 'qualifie', 400, 120, 60, NULL, NULL),
                    (2, 'Club B', 'clubb', 'Paris', 'paris', 'FR', 'qualifie', 400, 120, 50, 70,
                     '{\"score\":70,\"matched\":[\"Prunk\"],\"genre_nights\":14,\"nights\":20,\"genre_label\":\"house\",\"parts\":{\"overlap\":16,\"genre\":25,\"activity\":14,\"capacity\":15}}');
             INSERT INTO vi_contacts (venue_id, type, valeur, score, source_url) VALUES
                (1, 'email', 'booking@cluba.fr', 10, 'x'), (2, 'email', 'booking@clubb.fr', 10, 'x');
             INSERT INTO vi_promoter_orgs (id, cle, nom, nom_normalise, ville, ville_norm, pays, nb_events_total, fit_score)
             VALUES (7, 'ra:7', 'Collectif X', 'collectifx', 'Paris', 'paris', 'FR', 8, 55),
                    (8, 'ra:8', 'Tiny', 'tiny', 'Paris', 'paris', 'FR', 1, 90);
             INSERT INTO vi_promoter_contacts (promoter_id, valeur, score) VALUES
                (7, 'booking@collectifx.fr', 100), (8, 'hello@tiny.fr', 100);",
        )
        .unwrap();
        let cfg = Config::default();
        let r = build(&c, 1, &cfg, &BuildOptions::default()).unwrap();
        // Without a score Club A would lead (60 > 50); the fit puts B first.
        let keys: Vec<&str> = r.candidates.iter().map(|x| x.key.as_str()).collect();
        assert_eq!(keys, vec!["v:2", "v:1"]);
        assert_eq!(r.candidates[0].fit_explain.as_deref(), Some("a programmé Prunk ; 14 soirées house en 12 mois"));
        assert_eq!(r.candidates[1].fit_score, None);
        let opts = BuildOptions { include_promoters: Some(true), ..Default::default() };
        let r = build(&c, 1, &cfg, &opts).unwrap();
        let p = r.candidates.iter().find(|x| x.source == "promoter").expect("promoter listed");
        assert_eq!((p.key.as_str(), p.email.as_str()), ("p:7", "booking@collectifx.fr"));
        assert!(!r.candidates.iter().any(|x| x.key == "p:8"), "too few nights");
        let picks = vec![Pick { venue_id: None, contact_id: None, email: p.email.clone(), first_name: None, promoter_id: Some(7) }];
        assert_eq!(add_targets(&c, 1, &picks).unwrap(), 1);
        let cat: String = c
            .query_row("SELECT k.category FROM contacts k JOIN vi_promoter_orgs o ON o.crm_contact_id = k.id WHERE o.id = 7", [], |r| r.get(0))
            .unwrap();
        assert_eq!(cat, "promoter");
        let r = build(&c, 1, &cfg, &opts).unwrap();
        assert!(!r.candidates.iter().any(|x| x.key == "p:7"));
        assert_eq!(r.excluded_in_audience, 1);
    }

    /// On a copy of the real database (PIERSCRM_DB_COPY): migrations, then the
    /// default builder for each preset. `cargo test real_db_targets -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_db_targets() {
        let Ok(path) = std::env::var("PIERSCRM_DB_COPY") else { return };
        let pool = crate::db::init_pool(std::path::Path::new(&path)).expect("init");
        let c = pool.get().unwrap();
        c.execute("INSERT INTO campaigns (name, kind) VALUES ('test résidence', 'residency')", []).unwrap();
        let id = c.last_insert_rowid();
        for preset in ["paris", "london", "belgium", "switzerland"] {
            let mut cfg = Config::default();
            cfg.zone = Zone { presets: vec![preset.into()], ..Default::default() };
            let t = std::time::Instant::now();
            let r = build(&c, id, &cfg, &BuildOptions { limit: Some(1000), ..Default::default() }).unwrap();
            let unknown = r.candidates.iter().filter(|x| x.capacity_unknown).count();
            let pipeline = r.candidates.iter().filter(|x| x.source == "pipeline").count();
            println!(
                "{preset}: {} cibles ({} capacité inconnue, {} du pipeline) en {:?}; exclues: suppr {} récentes {} refus {} placeholder {} sans email {}",
                r.candidates.len(), unknown, pipeline, t.elapsed(), r.excluded_suppressed, r.excluded_recent,
                r.excluded_declined, r.excluded_placeholder, r.excluded_no_email
            );
            for x in r.candidates.iter().take(10) {
                println!("  {} | {:?} | cap {:?} | {} ev | {} | {} | {:?}", x.name, x.city, x.capacity, x.events, x.email, x.lang, x.artists);
            }
            if preset == "paris" {
                // Template drafts (no Claude) for the first two, on the copy only.
                let mut cfg = cfg.clone();
                cfg.assets.push(crate::commands::residency::Asset {
                    kind: "mix".into(), title: "Mix ADE".into(), url: "https://soundcloud.com/piers/mix".into(), note: String::new(),
                });
                crate::commands::residency::save_config(&c, id, &cfg).unwrap();
                let picks: Vec<Pick> = r.candidates.iter().take(2)
                    .map(|x| Pick { venue_id: x.venue_id, contact_id: x.contact_id, email: x.email.clone(), first_name: x.first_name.clone(), promoter_id: x.promoter_id })
                    .collect();
                add_targets(&c, id, &picks).unwrap();
                let g = crate::commands::residency::content::generate(&c, id, &cfg, None, false, None, None).unwrap();
                let (subj, body): (String, String) = c
                    .query_row("SELECT subject, body FROM campaign_messages WHERE campaign_id = ?1 ORDER BY id LIMIT 1", [id], |r| Ok((r.get(0)?, r.get(1)?)))
                    .unwrap();
                println!("  drafts: {}\n  --- {}\n{}\n  ---", g.created, subj, body);
            }
        }
    }
}
