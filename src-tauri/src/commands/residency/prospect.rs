//! Prospection on autopilot, for collectives.
//!
//! Collectives are the targets most likely to back the Piers project, and the
//! pool ran dry because every step waited for a click: enrich, promote, add to
//! a campaign, draft. Once a day (first engine tick after PROSPECT_HOUR local)
//! this module:
//!
//! 1. Refills every collectives campaign whose autopilot is on with up to
//!    REFILL_PER_DAY new targets of its zone (same builder rules as the
//!    campaign page: style match, booking address, suppressions, nobody
//!    already written to) and drafts their first email.
//! 2. Opens a "Collectifs <pays>" campaign for a country no collectives
//!    campaign covers yet, when at least NEW_CAMPAIGN_MIN collectives there
//!    are ready (validated copy of the latest collectives campaign, autopilot
//!    off), at most NEW_CAMPAIGNS_PER_DAY a day.
//! 3. Starts the web lookup of the next ENRICH_PER_DAY best-fitting
//!    collectives never looked up, so tomorrow's refill has fresh addresses.
//!
//! First emails it drafts are drafts: they go out once validated ("Tout
//! valider" on the campaign), like any other. Turned off with the setting
//! `prospect_autopilot` = "0".

use chrono::{DateTime, Timelike, Utc};
use rusqlite::{params, Connection};
use serde::Serialize;

use super::targeting::{self, BuildOptions, Pick};
use super::{Config, Zone};
use crate::db::DbPool;

pub const ENABLED_KEY: &str = "prospect_autopilot";
const DAY_KEY: &str = "prospect_last_day";
pub const PROSPECT_HOUR: u32 = 9;
pub const REFILL_PER_DAY: usize = 10;
pub const NEW_CAMPAIGN_MIN: usize = 5;
pub const NEW_CAMPAIGN_TARGETS: usize = 15;
pub const NEW_CAMPAIGNS_PER_DAY: usize = 2;
pub const ENRICH_PER_DAY: usize = 150;

#[derive(Debug, Clone, Default, Serialize)]
pub struct ProspectReport {
    /// (campaign name, targets added) per refilled campaign.
    pub refilled: Vec<(String, usize)>,
    /// Names of the campaigns opened.
    pub created: Vec<String>,
    pub drafts: usize,
    pub enrich_started: usize,
    pub notes: Vec<String>,
}

impl ProspectReport {
    pub fn summary(&self) -> Option<String> {
        let added: usize = self.refilled.iter().map(|r| r.1).sum();
        let mut parts = Vec::new();
        if self.drafts > 0 {
            parts.push(format!("{} premier(s) mail(s) à valider", self.drafts));
        }
        if !self.created.is_empty() {
            parts.push(format!("campagne(s) ouverte(s) : {}", self.created.join(", ")));
        }
        if added > 0 && self.drafts == 0 {
            parts.push(format!("{} collectif(s) ajouté(s)", added));
        }
        if self.enrich_started > 0 {
            parts.push(format!("{} collectifs en cours de recherche", self.enrich_started));
        }
        (!parts.is_empty()).then(|| parts.join(", "))
    }
}

/// One-time fix: collectives filed as "looked up" by runs where web search
/// never answered (no RA id, no site, no Instagram found) go back in the queue.
pub fn migrate(conn: &Connection) {
    migrate_strict(conn);
    const KEY: &str = "prospect_reset_unsearched_v1";
    if super::engine::get(conn, KEY).is_some() {
        return;
    }
    let n = conn
        .execute(
            "UPDATE vi_promoter_orgs SET enriched_at = NULL
             WHERE enriched_at IS NOT NULL AND ra_promoter_id IS NULL
               AND trim(COALESCE(site_web, '')) = '' AND trim(COALESCE(instagram, '')) = ''
               AND NOT EXISTS (SELECT 1 FROM vi_promoter_contacts c WHERE c.promoter_id = vi_promoter_orgs.id)",
            [],
        )
        .unwrap_or(0);
    super::engine::put(conn, KEY, &n.to_string());
}

/// One-time pass of strict_cleanup over what the loose rule had stored.
fn migrate_strict(conn: &Connection) {
    const KEY: &str = "collective_strict_v1";
    if super::engine::get(conn, KEY).is_some() {
        return;
    }
    let c = strict_cleanup(conn, true);
    super::engine::put(conn, KEY, &format!("{} sites, {} emails, {} instagram", c.sites, c.emails, c.handles));
}

/// What strict_cleanup removed.
#[derive(Debug, Default, PartialEq)]
pub struct Cleanup {
    pub sites: usize,
    pub emails: usize,
    pub handles: usize,
    /// "Sonic Flux: e-flux.com, @e_flux".
    pub lines: Vec<String>,
}

/// Re-check every collective's stored site and Instagram with the strict rule
/// (commands::strict_name). A site that does not spell the name loses its
/// address book (the emails were crawled off someone else's company) and a
/// handle that does not, or a bare short word, is dropped. The collective
/// goes back in the search queue. Collectives already messaged are left as is.
pub fn strict_cleanup(conn: &Connection, apply: bool) -> Cleanup {
    use crate::commands::enrich::domain_core;
    use crate::commands::strict_name::{is_bare_weak, spells_name};
    let mut out = Cleanup::default();
    let rows: Vec<(i64, String, Option<String>, Option<String>, Option<String>)> = conn
        .prepare(
            "SELECT o.id, o.nom, o.ville, NULLIF(trim(o.site_web), ''), NULLIF(trim(o.instagram), '')
             FROM vi_promoter_orgs o
             WHERE (NULLIF(trim(o.site_web), '') IS NOT NULL OR NULLIF(trim(o.instagram), '') IS NOT NULL)
               AND NOT EXISTS (SELECT 1 FROM social_touches s WHERE s.promoter_id = o.id
                               OR (o.crm_contact_id IS NOT NULL AND s.contact_id = o.crm_contact_id))
               AND (o.crm_contact_id IS NULL OR NOT EXISTS (SELECT 1 FROM campaign_contacts cc WHERE cc.contact_id = o.crm_contact_id))",
        )
        .and_then(|mut st| {
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))
                .map(|it| it.flatten().collect())
        })
        .unwrap_or_default();
    for (id, nom, ville, site, insta) in rows {
        let city = ville.as_deref();
        let bad_site = site.as_deref().filter(|s| {
            let core = domain_core(s);
            !s.contains("instagram.com") && !core.is_empty() && !spells_name(&core, &nom, city)
        });
        let bad_handle = insta
            .as_deref()
            .filter(|h| {
                let core = crate::db::normalise(h);
                !spells_name(&core, &nom, city) || is_bare_weak(&core, &nom, city)
            });
        if bad_site.is_none() && bad_handle.is_none() {
            continue;
        }
        let mut what = Vec::new();
        if let Some(s) = bad_site {
            let n: usize = conn
                .query_row(
                    "SELECT COUNT(*) FROM vi_promoter_contacts WHERE promoter_id = ?1 AND source_method = 'search'",
                    [id],
                    |r| r.get(0),
                )
                .unwrap_or(0);
            out.sites += 1;
            out.emails += n;
            what.push(format!("{s} ({n} emails)"));
            if apply {
                let _ = conn.execute("DELETE FROM vi_promoter_contacts WHERE promoter_id = ?1 AND source_method = 'search'", [id]);
                let _ = conn.execute("UPDATE vi_promoter_orgs SET site_web = NULL WHERE id = ?1", [id]);
            }
        }
        if let Some(h) = bad_handle {
            out.handles += 1;
            what.push(format!("@{h}"));
            if apply {
                let _ = conn.execute("UPDATE vi_promoter_orgs SET instagram = NULL WHERE id = ?1", [id]);
            }
        }
        if apply {
            let _ = conn.execute(
                "UPDATE vi_promoter_orgs SET enriched_at = NULL, instagram_checked_at = NULL, updated_at = datetime('now') WHERE id = ?1",
                [id],
            );
        }
        out.lines.push(format!("{nom}: {}", what.join(", ")));
    }
    out
}

/// A campaign written to collectives (the warm-up / last-minute cover copy).
pub fn is_collective(name: &str, cfg: &Config) -> bool {
    let n = name.to_lowercase();
    let subj = cfg.steps.first().map(|s| s.subject.to_lowercase()).unwrap_or_default();
    n.contains("collecti") || subj.contains("warm-up")
}

/// French name of a country, for a campaign title.
pub fn country_fr(iso: &str) -> String {
    const NAMES: &[(&str, &str)] = &[
        ("NL", "Pays-Bas"), ("DE", "Allemagne"), ("ES", "Espagne"), ("IT", "Italie"), ("PT", "Portugal"),
        ("AT", "Autriche"), ("HU", "Hongrie"), ("PL", "Pologne"), ("CZ", "Tchéquie"), ("DK", "Danemark"),
        ("SE", "Suède"), ("NO", "Norvège"), ("FI", "Finlande"), ("GR", "Grèce"), ("RO", "Roumanie"),
        ("HR", "Croatie"), ("SI", "Slovénie"), ("SK", "Slovaquie"), ("BG", "Bulgarie"), ("RS", "Serbie"),
        ("GB", "Royaume-Uni"), ("IE", "Irlande"), ("BE", "Belgique"), ("CH", "Suisse"), ("FR", "France"),
        ("LU", "Luxembourg"), ("US", "États-Unis"), ("CA", "Canada"), ("AU", "Australie"), ("GE", "Géorgie"),
        ("EE", "Estonie"), ("LV", "Lettonie"), ("LT", "Lituanie"), ("MT", "Malte"), ("CY", "Chypre"),
    ];
    NAMES.iter().find(|(c, _)| *c == iso).map(|(_, n)| n.to_string()).unwrap_or_else(|| iso.to_string())
}

/// Collective candidates of a campaign's zone, best first.
fn collective_picks(conn: &Connection, campaign_id: i64, cfg: &Config, max: usize) -> Result<Vec<Pick>, String> {
    let opts = BuildOptions {
        include_promoters: Some(true),
        include_unmatched: Some(false),
        limit: Some(300),
        ..Default::default()
    };
    let built = targeting::build(conn, campaign_id, cfg, &opts)?;
    Ok(built
        .candidates
        .into_iter()
        .filter(|c| c.source == "promoter" && !c.email.trim().is_empty() && c.email_rank <= 2)
        .filter(|c| c.promoter_id.is_some_and(|id| grassroots(conn, id)))
        .take(max)
        .map(|c| Pick {
            venue_id: None,
            contact_id: c.contact_id,
            email: c.email,
            first_name: c.first_name,
            promoter_id: c.promoter_id,
        })
        .collect())
}

/// Biggest footprint a collective picked without a click may have: past it,
/// it is a touring brand or a label night (Cocoon, The Night League), not a
/// crew that books a warm-up.
pub const MAX_VENUES: i64 = 8;
pub const MAX_EVENTS: i64 = 300;

/// Stricter than the campaign page, since nobody looks before the draft: a
/// local-sized crew that already booked at least one reference artist (a fit
/// score alone also comes from genre overlap, which let drum and bass in).
pub fn grassroots(conn: &Connection, promoter_id: i64) -> bool {
    let row: Option<(i64, i64, Option<String>)> = conn
        .query_row(
            "SELECT nb_venues, nb_events_total, fit_detail FROM vi_promoter_orgs WHERE id = ?1",
            params![promoter_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .ok();
    let Some((venues, events, detail)) = row else { return false };
    let matched = detail
        .and_then(|d| serde_json::from_str::<serde_json::Value>(&d).ok())
        .and_then(|v| v["matched"].as_array().map(|a| a.len()))
        .unwrap_or(0);
    venues <= MAX_VENUES && events <= MAX_EVENTS && matched >= 1
}

/// Add picks and draft their first email. Returns (added, drafted).
fn add_and_draft(conn: &Connection, campaign_id: i64, cfg: &Config, picks: &[Pick]) -> Result<(usize, usize), String> {
    if picks.is_empty() {
        return Ok((0, 0));
    }
    let added = targeting::add_targets(conn, campaign_id, picks)?;
    let llm = if cfg!(test) { None } else { super::claude::client(conn) };
    let ins = if cfg!(test) { None } else { super::insrt::client(conn) };
    let g = super::content::generate(conn, campaign_id, cfg, None, false, llm.as_deref(), ins.as_deref())?;
    Ok((added, g.created))
}

/// Active residency campaigns: (id, name, config, autopilot).
fn campaigns(conn: &Connection) -> Vec<(i64, String, Config, bool)> {
    conn.prepare(
        "SELECT c.id, c.name, r.config, r.autopilot FROM campaigns c JOIN residency_campaigns r ON r.campaign_id = c.id
         WHERE c.status = 'active' AND c.kind = 'residency' ORDER BY c.id",
    )
    .and_then(|mut st| {
        st.query_map([], |r| {
            let raw: String = r.get(2)?;
            Ok((r.get(0)?, r.get(1)?, serde_json::from_str(&raw).unwrap_or_default(), r.get::<_, i64>(3)? != 0))
        })
        .map(|it| it.flatten().collect())
    })
    .unwrap_or_default()
}

/// Countries with collectives that fit, by count, none of them covered by a
/// collectives campaign.
fn uncovered_countries(conn: &Connection, covered: &[String]) -> Vec<String> {
    let rows: Vec<(String, i64)> = conn
        .prepare(
            "SELECT o.pays, COUNT(*) FROM vi_promoter_orgs o
             WHERE o.statut = 'actif' AND COALESCE(o.org_type, 'collectif') = 'collectif' AND o.fit_score >= 15
               AND trim(COALESCE(o.pays, '')) <> ''
               AND EXISTS (SELECT 1 FROM vi_promoter_contacts pc WHERE pc.promoter_id = o.id AND pc.type = 'email')
             GROUP BY o.pays ORDER BY 2 DESC",
        )
        .and_then(|mut st| st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map(|it| it.flatten().collect()))
        .unwrap_or_default();
    rows.into_iter()
        .filter(|(_, n)| *n as usize >= NEW_CAMPAIGN_MIN)
        .map(|(p, _)| super::zone::iso_country(&p))
        .filter(|c| c.len() == 2 && !covered.contains(c))
        .collect()
}

/// Open "Collectifs <pays>" from the template campaign's validated copy, when
/// enough collectives there pass the builder. Autopilot stays off.
fn open_country(conn: &Connection, iso: &str, template: &Config, report: &mut ProspectReport) -> Result<bool, String> {
    let name = format!("Collectifs {}", country_fr(iso));
    let exists: bool = conn
        .query_row("SELECT EXISTS(SELECT 1 FROM campaigns WHERE name = ?1)", params![name], |r| r.get(0))
        .unwrap_or(false);
    if exists {
        return Ok(false);
    }
    let mut cfg = template.clone();
    cfg.zone = Zone { presets: vec![], cities: vec![], countries: vec![iso.to_string()] };
    cfg.lang = "auto".into();
    let artist_id = crate::commands::artists::default_artist_id(conn);
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    tx.execute(
        "INSERT INTO campaigns (name, purpose, artist_id, status, color, kind) VALUES (?1, ?2, ?3, 'active', '#ff5a1f', 'residency')",
        params![name, "Collectifs : warm-up et dépannage", artist_id],
    )
    .map_err(|e| e.to_string())?;
    let id = tx.last_insert_rowid();
    super::save_config(&tx, id, &cfg)?;
    let picks = collective_picks(&tx, id, &cfg, NEW_CAMPAIGN_TARGETS)?;
    if picks.len() < NEW_CAMPAIGN_MIN {
        // Not enough once the builder's rules apply: nothing is kept.
        drop(tx);
        return Ok(false);
    }
    tx.commit().map_err(|e| e.to_string())?;
    let (added, drafted) = add_and_draft(conn, id, &cfg, &picks)?;
    super::log(
        conn,
        Some(id),
        "info",
        "prospect_created",
        &format!("Ouverte par la prospection auto : {} collectif(s), {} premier(s) mail(s) à valider", added, drafted),
    );
    report.created.push(name);
    report.drafts += drafted;
    Ok(true)
}

/// Refill and open campaigns (steps 1 and 2). Pure database work.
pub fn refill_all(conn: &Connection) -> ProspectReport {
    let mut report = ProspectReport::default();
    let all = campaigns(conn);
    let collective: Vec<&(i64, String, Config, bool)> = all.iter().filter(|(_, n, c, _)| is_collective(n, c)).collect();
    for (id, name, cfg, autopilot) in &collective {
        if !*autopilot {
            continue;
        }
        match collective_picks(conn, *id, cfg, REFILL_PER_DAY).and_then(|p| add_and_draft(conn, *id, cfg, &p)) {
            Ok((added, drafted)) if added > 0 => {
                super::log(
                    conn,
                    Some(*id),
                    "info",
                    "prospect_refill",
                    &format!("Prospection auto : {} collectif(s) ajouté(s), {} premier(s) mail(s) à valider", added, drafted),
                );
                report.refilled.push((name.clone(), added));
                report.drafts += drafted;
            }
            Ok(_) => {}
            Err(e) => report.notes.push(format!("{} : {}", name, e)),
        }
    }
    let Some(template) = collective.last().map(|c| c.2.clone()) else { return report };
    let covered: Vec<String> = collective.iter().flat_map(|(_, _, c, _)| c.zone.resolve().1).collect();
    let mut opened = 0;
    for iso in uncovered_countries(conn, &covered) {
        if opened >= NEW_CAMPAIGNS_PER_DAY {
            break;
        }
        match open_country(conn, &iso, &template, &mut report) {
            Ok(true) => opened += 1,
            Ok(false) => {}
            Err(e) => report.notes.push(format!("{} : {}", iso, e)),
        }
    }
    report
}

/// Collectives never looked up, best fit first (step 3).
fn enrich_queue(conn: &Connection) -> Vec<i64> {
    conn.prepare(
        "SELECT o.id FROM vi_promoter_orgs o
         WHERE o.statut = 'actif' AND o.house_promoter = 0 AND COALESCE(o.org_type, 'collectif') = 'collectif'
           AND o.fit_score >= 15 AND o.enriched_at IS NULL
           AND o.nb_venues <= ?2 AND o.nb_events_total <= ?3
           AND COALESCE(json_array_length(json_extract(o.fit_detail, '$.matched')), 0) >= 1
           AND NOT EXISTS (SELECT 1 FROM vi_promoter_contacts c WHERE c.promoter_id = o.id AND c.type = 'email')
         ORDER BY o.fit_score DESC, o.nb_events_total DESC LIMIT ?1",
    )
    .and_then(|mut st| {
        st.query_map(params![ENRICH_PER_DAY as i64, MAX_VENUES, MAX_EVENTS], |r| r.get(0)).map(|it| it.flatten().collect())
    })
    .unwrap_or_default()
}

/// Engine tick: once a day after PROSPECT_HOUR local, refill, open and feed.
pub fn daily(pool: &DbPool, app: &tauri::AppHandle, now: DateTime<Utc>) {
    let Ok(conn) = pool.get() else { return };
    if super::engine::get(&conn, ENABLED_KEY).as_deref() == Some("0") {
        return;
    }
    let local = chrono::DateTime::<chrono::Local>::from(now);
    let today = local.format("%Y-%m-%d").to_string();
    if local.hour() < PROSPECT_HOUR || super::engine::get(&conn, DAY_KEY).as_deref() == Some(today.as_str()) {
        return;
    }
    super::engine::put(&conn, DAY_KEY, &today);
    let mut report = refill_all(&conn);

    // 3. Feed tomorrow: the web lookup needs a Serper key and no run going.
    let key_set = crate::secrets::read_setting(&conn, "serper_api_key").is_some_and(|k| !k.trim().is_empty());
    let running: bool = conn
        .query_row("SELECT EXISTS(SELECT 1 FROM vi_runs WHERE statut = 'en_cours')", [], |r| r.get(0))
        .unwrap_or(true);
    if key_set && !running {
        let ids = enrich_queue(&conn);
        if !ids.is_empty() {
            match crate::commands::promoters::start_enrich_run(&conn, &ids) {
                Ok(run_id) => {
                    use tauri::Manager;
                    let cancels = app.state::<crate::AppState>().cancels.clone();
                    crate::commands::harvest::spawn_worker(pool.clone(), cancels, app.clone(), run_id);
                    report.enrich_started = ids.len();
                }
                Err(e) => report.notes.push(e),
            }
        }
    } else if !key_set {
        report.notes.push("recherche web non réglée (clé Serper) : aucun nouveau collectif ne sera trouvé".into());
    }
    let summary = report.summary();
    let line = match (&summary, report.notes.is_empty()) {
        (Some(s), _) => s.clone(),
        (None, true) => "rien de nouveau aujourd'hui".into(),
        (None, false) => report.notes.join(" ; "),
    };
    super::log(&conn, None, "info", "prospect_daily", &format!("Prospection auto : {}", line));
    if let Some(s) = summary.filter(|_| report.drafts > 0 || !report.created.is_empty()) {
        notify(app, &s);
    }
}

fn notify(app: &tauri::AppHandle, body: &str) {
    use tauri_plugin_notification::NotificationExt;
    let _ = app.notification().builder().title("Prospection collectifs").body(body).show();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::residency::testdb;

    #[test]
    fn collective_campaign_detection_and_names() {
        let mut cfg = Config::default();
        assert!(is_collective("Collectifs Royaume-Uni et Irlande", &cfg));
        assert!(!is_collective("Résidence Belgique", &cfg));
        cfg.steps[0].subject = "Warm-up ou dépannage chez {{venue}} ?".into();
        assert!(is_collective("Demande Sur Collectif", &cfg));
        assert!(is_collective("Autre nom", &cfg));
        assert_eq!(country_fr("NL"), "Pays-Bas");
        assert_eq!(country_fr("ZZ"), "ZZ");
    }

    #[test]
    fn only_local_crews_with_a_reference_artist() {
        let c = testdb::conn();
        c.execute_batch(
            r#"INSERT INTO vi_promoter_orgs (id, cle, nom, nom_normalise, nb_venues, nb_events_total, fit_detail) VALUES
                (1, 'a', 'Crew', 'crew', 3, 40, '{"matched":["Franky Rizardo"]}'),
                (2, 'b', 'Brand', 'brand', 24, 54, '{"matched":["Ricardo Villalobos"]}'),
                (3, 'c', 'Giant', 'giant', 4, 1301, '{"matched":["ANOTR"]}'),
                (4, 'd', 'DnB', 'dnb', 13, 22, '{"matched":[]}'),
                (5, 'e', 'NoRef', 'noref', 2, 10, '{"matched":[]}');"#,
        )
        .unwrap();
        assert!(grassroots(&c, 1));
        assert!(!grassroots(&c, 2) && !grassroots(&c, 3) && !grassroots(&c, 4) && !grassroots(&c, 5));
    }

    #[test]
    fn nothing_to_do_on_an_empty_base() {
        let c = testdb::conn();
        let r = refill_all(&c);
        assert!(r.refilled.is_empty() && r.created.is_empty() && r.summary().is_none());
    }

    #[test]
    fn summary_reads_well() {
        let r = ProspectReport {
            refilled: vec![("Collectifs Royaume-Uni".into(), 4)],
            created: vec!["Collectifs Pays-Bas".into()],
            drafts: 12,
            enrich_started: 150,
            notes: vec![],
        };
        assert_eq!(
            r.summary().unwrap(),
            "12 premier(s) mail(s) à valider, campagne(s) ouverte(s) : Collectifs Pays-Bas, 150 collectifs en cours de recherche"
        );
    }
}
