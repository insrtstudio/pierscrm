//! What a "promoter" seen on RA really is.
//!
//! RA lists as promoter whoever runs the night: a party collective, but just
//! as often the club itself (Sacré142 at Sacré), a label (Toy Tonics), an
//! agency or promotion company, a festival, a radio or a magazine. The
//! Collectifs list must only show party collectives, so every org gets a type:
//!
//! - rules (`classify_rules`), over every org after each aggregate refresh:
//!   the club holding most of its nights carries its name, or its name says
//!   club / radio / records / festival / agency;
//! - Claude (`vi_classify_promoters`), over the reachable ones (email,
//!   Instagram or in Outreach): type, and whether the stored Instagram is
//!   really theirs (a handle found on the wrong site is dropped);
//! - by hand (`vi_set_promoter_type`), never overwritten.

use std::collections::HashMap;

use rusqlite::{params, Connection};
use serde::Serialize;
use serde_json::{json, Value};
use tauri::State;

use super::residency::claude::{self, Llm, LlmRequest, SystemBlock};
use crate::db::normalise;
use crate::AppState;

pub const TYPES: [&str; 7] = ["collectif", "club", "label", "agence", "festival", "media", "autre"];

pub(crate) fn migrate(conn: &Connection) {
    for stmt in [
        "ALTER TABLE vi_promoter_orgs ADD COLUMN org_type TEXT",
        // 'rules' | 'ai' | 'manual'
        "ALTER TABLE vi_promoter_orgs ADD COLUMN org_type_source TEXT",
        "ALTER TABLE vi_promoter_orgs ADD COLUMN org_type_note TEXT",
        // A handle judged not theirs, kept so the pass does not store it again.
        "ALTER TABLE vi_promoter_orgs ADD COLUMN instagram_rejected TEXT",
        "CREATE INDEX IF NOT EXISTS idx_vi_promoter_orgs_type ON vi_promoter_orgs(org_type)",
    ] {
        let _ = conn.execute(stmt, []);
    }
    let unclassified: i64 = conn
        .query_row("SELECT COUNT(*) FROM vi_promoter_orgs WHERE org_type IS NULL", [], |r| r.get(0))
        .unwrap_or(0);
    if unclassified > 0 {
        classify_rules(conn);
        reject_implausible_instagrams(conn);
    }
}

// ---------------------------------------------------------------------------
// Rules
// ---------------------------------------------------------------------------

/// Words too common in names to tie an org to a venue or a handle.
const GENERIC: &[&str] = &[
    "club", "clubs", "the", "les", "and", "events", "event", "music", "musique", "records", "presents", "present",
    "night", "nights", "party", "parties", "live", "house", "techno", "sound", "sounds", "collective", "collectif",
    "productions", "production", "prod", "official", "paris", "london", "berlin", "amsterdam", "bruxelles",
    "brussels", "barcelona", "ibiza", "milano", "milan", "lisboa", "festival", "open", "air", "bar", "group",
];

fn words(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric()).map(normalise).filter(|w| !w.is_empty()).collect()
}

fn strong_words(s: &str) -> Vec<String> {
    words(s).into_iter().filter(|w| w.len() >= 4 && !GENERIC.contains(&w.as_str())).collect()
}

/// Same place or brand: one name contains the other ("sacre142" / "sacre"),
/// or they share a distinctive word ("Renate Club" / "Renate").
pub fn names_related(a: &str, b: &str) -> bool {
    let (na, nb) = (normalise(a), normalise(b));
    if na.len() >= 4 && nb.len() >= 4 && (na.contains(&nb) || nb.contains(&na)) {
        return true;
    }
    let wb = strong_words(b);
    strong_words(a).iter().any(|w| wb.contains(w))
}

fn has_word(name_words: &[String], list: &[&str]) -> bool {
    name_words.iter().any(|w| list.contains(&w.as_str()))
}

/// Type from the name, the venues it runs nights at (normalised name, nights)
/// and its links. Returns the type and a short French reason.
pub fn rule_type(name: &str, venues: &[(String, i64)], house: bool, site: Option<&str>, ig: Option<&str>) -> (&'static str, String) {
    let w = words(name);
    let total: i64 = venues.iter().map(|v| v.1.max(0)).sum();
    if house {
        return ("club", "le lieu est listé comme organisateur de ses propres soirées".into());
    }
    if total > 0 {
        for (v, n) in venues {
            let share = *n as f64 / total as f64;
            if share >= 0.5 && names_related(name, v) {
                return ("club", format!("{} % de ses soirées ont lieu au club qui porte son nom ({v})", (share * 100.0).round()));
            }
        }
    }
    if has_word(&w, &["club", "nightclub", "discotheque", "discoteca", "discoclub", "disco", "kneipe", "hotel", "beach", "rooftop", "lounge", "boite"])
        && !has_word(&w, &["collective", "collectif", "crew"])
    {
        return ("club", "son nom désigne un lieu (club, discothèque, bar…)".into());
    }
    let links = format!("{} {}", site.unwrap_or(""), ig.unwrap_or("")).to_lowercase();
    if has_word(&w, &["radio", "fm", "webradio", "magazine", "mag", "podcast", "tv", "zine", "journal"]) || links.contains("radio") {
        return ("media", "radio ou média".into());
    }
    if has_word(&w, &["festival", "fest", "festivals", "openair"]) {
        return ("festival", "festival".into());
    }
    if has_word(&w, &["records", "recordings", "label", "rec"]) || links.contains("records") {
        return ("label", "label".into());
    }
    if has_word(&w, &["agency", "agence", "booking", "bookings", "management", "mgmt", "entertainment", "ltd", "gmbh"]) {
        return ("agence", "agence ou société de production".into());
    }
    ("collectif", String::new())
}

/// Rules over every org whose type was not set by Claude or by hand. Run after
/// each aggregate refresh (promoters::refresh_aggregates).
pub(crate) fn classify_rules(conn: &Connection) {
    let mut venues: HashMap<i64, Vec<(String, i64)>> = HashMap::new();
    if let Ok(mut st) = conn.prepare(
        "SELECT p.org_id, v.nom, p.nb_events FROM vi_promoters p JOIN vi_venues v ON v.id = p.venue_id WHERE p.org_id IS NOT NULL",
    ) {
        if let Ok(rows) = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?))) {
            for (o, v, n) in rows.flatten() {
                venues.entry(o).or_default().push((v, n));
            }
        }
    }
    let orgs: Vec<(i64, String, bool, Option<String>, Option<String>)> = match conn.prepare(
        "SELECT id, nom, house_promoter, site_web, instagram FROM vi_promoter_orgs
         WHERE COALESCE(org_type_source, 'rules') = 'rules'",
    ) {
        Ok(mut st) => st
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? != 0, r.get(3)?, r.get(4)?)))
            .map(|it| it.flatten().collect())
            .unwrap_or_default(),
        Err(_) => return,
    };
    let Ok(tx) = conn.unchecked_transaction() else { return };
    if let Ok(mut up) = tx.prepare(
        "UPDATE vi_promoter_orgs SET org_type = ?2, org_type_source = 'rules', org_type_note = ?3
         WHERE id = ?1 AND (org_type IS NOT ?2 OR org_type_note IS NOT ?3 OR org_type_source IS NULL)",
    ) {
        for (id, nom, house, site, ig) in &orgs {
            let empty = Vec::new();
            let (t, note) = rule_type(nom, venues.get(id).unwrap_or(&empty), *house, site.as_deref(), ig.as_deref());
            let note = (!note.is_empty()).then_some(note);
            let _ = up.execute(params![id, t, note]);
        }
    }
    let _ = tx.commit();
}

/// A handle that shares nothing with the org's name or website is someone
/// else's (found on a wrong site, or a namesake).
pub fn instagram_plausible(handle: &str, name: &str, site: Option<&str>) -> bool {
    let h = normalise(handle);
    if h.len() < 3 {
        return false;
    }
    let n = normalise(name);
    if n.len() >= 4 && (h.contains(&n) || n.contains(&h)) {
        return true;
    }
    // Short names (LWE, RAW): the handle must start with them.
    if n.len() == 3 && h.starts_with(&n) {
        return true;
    }
    if words(name).iter().filter(|w| w.len() >= 4 && !["club", "the", "events", "music", "records"].contains(&w.as_str())).any(|w| h.contains(w.as_str())) {
        return true;
    }
    if let Some(stem) = site.and_then(site_stem) {
        if stem.len() >= 4 && (h.contains(&stem) || stem.contains(&h)) {
            // The site may be wrong too, so it only vouches when it also
            // shares something with the name.
            return names_related(&stem, name);
        }
    }
    false
}

fn site_stem(site: &str) -> Option<String> {
    let host = site.split("://").nth(1).unwrap_or(site).split('/').next()?.trim_start_matches("www.");
    let first = host.split('.').next()?;
    Some(normalise(first))
}

/// Drop a handle judged not theirs, from the org and from its contact when
/// the contact got it from the org.
pub(crate) fn reject_instagram(conn: &Connection, id: i64) {
    let _ = conn.execute(
        "UPDATE contacts SET instagram = NULL
         WHERE id = (SELECT crm_contact_id FROM vi_promoter_orgs WHERE id = ?1)
           AND lower(ltrim(trim(instagram), '@')) = (SELECT lower(ltrim(trim(instagram), '@')) FROM vi_promoter_orgs WHERE id = ?1)",
        [id],
    );
    let _ = conn.execute(
        "UPDATE vi_promoter_orgs SET instagram_rejected = instagram, instagram = NULL,
            instagram_checked_at = datetime('now'), updated_at = datetime('now')
         WHERE id = ?1 AND trim(COALESCE(instagram, '')) <> ''",
        [id],
    );
}

/// Rule check over every stored handle. Returns how many were dropped.
pub(crate) fn reject_implausible_instagrams(conn: &Connection) -> i64 {
    let rows: Vec<(i64, String, String, Option<String>)> = match conn.prepare(
        "SELECT id, instagram, nom, site_web FROM vi_promoter_orgs WHERE trim(COALESCE(instagram, '')) <> ''",
    ) {
        Ok(mut st) => st
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .map(|it| it.flatten().collect())
            .unwrap_or_default(),
        Err(_) => return 0,
    };
    let mut n = 0;
    for (id, h, nom, site) in rows {
        if !instagram_plausible(&h, &nom, site.as_deref()) {
            reject_instagram(conn, id);
            n += 1;
        }
    }
    n
}

// ---------------------------------------------------------------------------
// Claude check over the reachable ones
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Default)]
pub struct ClassifySummary {
    pub checked: i64,
    pub collectif: i64,
    pub other: i64,
    pub instagram_rejected: i64,
    pub failed_batches: i64,
}

const BATCH: usize = 20;

fn system_prompt() -> String {
    "You sort organisations that Resident Advisor lists as promoters of club nights. A booking agent \
wants to pitch a DJ to PARTY COLLECTIVES only: independent crews or brands that throw their own \
nights in various venues.

For each organisation, choose its type:
- collectif: a party collective, crew or night brand (even if it often uses the same club)
- club: a venue itself (club, discotheque, bar, beach club, hotel, concert hall) or the venue's own house brand
- label: a record label (even if it throws label nights)
- agence: an agency, promotion company or corporate events group running many venues or big shows
- festival: a festival brand
- media: a radio, magazine, podcast, web media
- autre: anything else (sound system brand, shop, school, NGO...)

Use the name, the venues where its nights happen (a single venue carrying most nights and sharing \
its name means the club itself), the website domain and the Instagram handle. When unsure between \
collectif and something else, pick the other type only on real evidence.

instagram_ok: is the given Instagram handle plausibly the account of THIS organisation (not a \
namesake, a studio, a different label, a venue it plays at, a person)? null when no handle is given.

reason: at most 12 words, in French, without dashes."
        .into()
}

fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "items": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "id": {"type": "integer"},
                        "type": {"type": "string", "enum": TYPES},
                        "instagram_ok": {"type": ["boolean", "null"]},
                        "reason": {"type": "string"}
                    },
                    "required": ["id", "type", "instagram_ok", "reason"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["items"],
        "additionalProperties": false
    })
}

struct OrgFacts {
    id: i64,
    nom: String,
    ville: Option<String>,
    pays: Option<String>,
    site: Option<String>,
    instagram: Option<String>,
    email_domain: Option<String>,
    venues: Vec<(String, i64)>,
}

fn facts_line(o: &OrgFacts) -> String {
    let total: i64 = o.venues.iter().map(|v| v.1).sum::<i64>().max(1);
    let venues = o
        .venues
        .iter()
        .take(4)
        .map(|(v, n)| format!("{v} ({}%)", (*n as f64 * 100.0 / total as f64).round()))
        .collect::<Vec<_>>()
        .join(", ");
    let city = [o.ville.clone().unwrap_or_default(), o.pays.clone().unwrap_or_default()].join(", ");
    json!({
        "id": o.id,
        "name": o.nom,
        "city": city,
        "venues_share_of_nights": venues,
        "venues_count": o.venues.len(),
        "website": o.site.as_deref().and_then(site_host),
        "instagram": o.instagram,
        "email_domain": o.email_domain,
    })
    .to_string()
}

fn site_host(site: &str) -> Option<String> {
    Some(site.split("://").nth(1).unwrap_or(site).split('/').next()?.trim_start_matches("www.").to_string())
}

fn load_targets(conn: &Connection, force: bool) -> Result<Vec<OrgFacts>, String> {
    let sql = format!(
        "SELECT o.id, o.nom, o.ville, o.pays, o.site_web, NULLIF(trim(o.instagram), ''),
            (SELECT substr(valeur, instr(valeur, '@') + 1) FROM vi_promoter_contacts c
              WHERE c.promoter_id = o.id AND c.type = 'email' ORDER BY c.score DESC LIMIT 1)
         FROM vi_promoter_orgs o
         WHERE o.statut = 'actif'
           AND COALESCE(o.org_type_source, 'rules') <> 'manual'
           AND (EXISTS(SELECT 1 FROM vi_promoter_contacts c WHERE c.promoter_id = o.id AND c.type = 'email')
                OR trim(COALESCE(o.instagram, '')) <> '' OR o.crm_contact_id IS NOT NULL)
           {}
         ORDER BY o.id",
        if force { "" } else { "AND COALESCE(o.org_type_source, 'rules') <> 'ai'" }
    );
    let mut st = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let mut out: Vec<OrgFacts> = st
        .query_map([], |r| {
            Ok(OrgFacts {
                id: r.get(0)?,
                nom: r.get(1)?,
                ville: r.get(2)?,
                pays: r.get(3)?,
                site: r.get(4)?,
                instagram: r.get(5)?,
                email_domain: r.get(6)?,
                venues: Vec::new(),
            })
        })
        .map_err(|e| e.to_string())?
        .flatten()
        .collect();
    let mut vs = conn
        .prepare(
            "SELECT v.nom, p.nb_events FROM vi_promoters p JOIN vi_venues v ON v.id = p.venue_id
             WHERE p.org_id = ?1 ORDER BY p.nb_events DESC LIMIT 6",
        )
        .map_err(|e| e.to_string())?;
    for o in &mut out {
        o.venues = vs
            .query_map([o.id], |r| Ok((r.get(0)?, r.get(1)?)))
            .map(|it| it.flatten().collect())
            .unwrap_or_default();
    }
    Ok(out)
}

/// Apply one answer. Manual types are never touched (excluded upstream too).
fn apply(conn: &Connection, item: &Value, s: &mut ClassifySummary) {
    let Some(id) = item.get("id").and_then(Value::as_i64) else { return };
    let Some(t) = item.get("type").and_then(Value::as_str).filter(|t| TYPES.contains(t)) else { return };
    let reason = item.get("reason").and_then(Value::as_str).map(claude::no_dashes);
    let n = conn
        .execute(
            "UPDATE vi_promoter_orgs SET org_type = ?2, org_type_source = 'ai', org_type_note = ?3, updated_at = datetime('now')
             WHERE id = ?1 AND COALESCE(org_type_source, 'rules') <> 'manual'",
            params![id, t, reason],
        )
        .unwrap_or(0);
    if n == 0 {
        return;
    }
    s.checked += 1;
    if t == "collectif" {
        s.collectif += 1;
    } else {
        s.other += 1;
    }
    if item.get("instagram_ok").and_then(Value::as_bool) == Some(false) {
        reject_instagram(conn, id);
        s.instagram_rejected += 1;
    }
}

pub fn classify_with(conn: &Connection, llm: &dyn Llm, force: bool) -> Result<ClassifySummary, String> {
    let targets = load_targets(conn, force)?;
    let mut s = ClassifySummary::default();
    for chunk in targets.chunks(BATCH) {
        let req = LlmRequest {
            model: claude::DEFAULT_MODEL.into(),
            max_tokens: 2400,
            system: vec![SystemBlock { text: system_prompt(), cache: true }],
            user: chunk.iter().map(facts_line).collect::<Vec<_>>().join("\n"),
            schema: Some(schema()),
        };
        match llm.complete(&req) {
            Ok(resp) => {
                claude::log_usage(conn, "org_type", None, &resp.model, &resp.usage);
                match claude::extract_json(&resp.text).and_then(|v| v.get("items").cloned()) {
                    Some(Value::Array(items)) => {
                        for it in &items {
                            apply(conn, it, &mut s);
                        }
                    }
                    _ => s.failed_batches += 1,
                }
            }
            Err(e) => {
                // A refused key stops the pass; anything else skips the batch.
                if s.checked == 0 && e.contains("Clé API") {
                    return Err(e);
                }
                s.failed_batches += 1;
            }
        }
    }
    Ok(s)
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Claude check of the reachable orgs not checked yet (`force`: all again).
/// Error code the UI translates: "ai.no_key".
#[tauri::command(async)]
pub fn vi_classify_promoters(state: State<'_, AppState>, force: Option<bool>) -> Result<ClassifySummary, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let llm = claude::client(&conn).ok_or_else(|| "ai.no_key".to_string())?;
    classify_with(&conn, llm.as_ref(), force.unwrap_or(false))
}

/// Set the type by hand (`None` gives it back to the rules).
#[tauri::command(async)]
pub fn vi_set_promoter_type(state: State<'_, AppState>, id: i64, org_type: Option<String>) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    match org_type.filter(|t| TYPES.contains(&t.as_str())) {
        Some(t) => conn
            .execute(
                "UPDATE vi_promoter_orgs SET org_type = ?2, org_type_source = 'manual', org_type_note = NULL,
                    updated_at = datetime('now') WHERE id = ?1",
                params![id, t],
            )
            .map(|_| ())
            .map_err(|e| e.to_string()),
        None => {
            conn.execute("UPDATE vi_promoter_orgs SET org_type_source = 'rules' WHERE id = ?1", [id])
                .map_err(|e| e.to_string())?;
            classify_rules(&conn);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::residency::claude::fake::FakeLlm;

    fn v(n: &str, k: i64) -> (String, i64) {
        (n.to_string(), k)
    }

    #[test]
    fn club_behind_its_own_brand() {
        // Sacré142: 675 of its ~712 nights at Sacré.
        let venues = vec![v("Sacré", 675), v("Concorde Atlantique", 20), v("Cabaret Sauvage", 8)];
        assert_eq!(rule_type("Sacré142", &venues, false, None, None).0, "club");
        assert_eq!(rule_type("Renate Club", &[v("Renate", 690), v("Ostkreuz", 5)], false, None, None).0, "club");
        // A crew with a residency elsewhere stays a collective.
        assert_eq!(rule_type("Small Talk", &[v("Fabric", 30), v("Corsica Studios", 20)], false, None, None).0, "collectif");
        assert_eq!(rule_type("Sacred Realism", &[v("KM28", 3), v("Sowieso", 2)], false, None, None).0, "collectif");
    }

    #[test]
    fn names_and_links_tell_the_rest() {
        assert_eq!(rule_type("Groovalizacion", &[], false, None, Some("groovalizacion.radio")).0, "media");
        assert_eq!(rule_type("Psy Fiction Records", &[], false, None, None).0, "label");
        assert_eq!(rule_type("Brunch Electronik Festival", &[], false, None, None).0, "festival");
        assert_eq!(rule_type("Urban Agency", &[], false, None, None).0, "agence");
        assert_eq!(rule_type("Volt Club Milan", &[], false, None, None).0, "club");
        assert_eq!(rule_type("Club Collective Paris", &[], false, None, None).0, "collectif");
        assert_eq!(rule_type("Anything", &[], true, None, None).0, "club");
    }

    #[test]
    fn instagram_must_relate_to_the_org() {
        assert!(instagram_plausible("lightboxlondon", "Lightbox Nights", None));
        assert!(instagram_plausible("the.supermercat", "TheSupermercat", None));
        assert!(instagram_plausible("smalltalk_collective", "Small Talk", None));
        assert!(instagram_plausible("l_w_e", "LWE", None));
        assert!(!instagram_plausible("p61gallery", "BLACKHOLEBERLIN", None));
        assert!(!instagram_plausible("leggiscomodo", "Columbo Music", None));
        // A shared word is enough for the rules ("fiction"): namesakes like
        // Freaky Fiction / psyfictionrecords are left to the Claude check.
        assert!(instagram_plausible("psyfictionrecords", "FREAKY FICTION", None));
        // A site that does not match the name does not vouch for its handle.
        assert!(!instagram_plausible("voidacoustics", "Night Shift", Some("https://voidacoustics.com")));
    }

    #[test]
    fn claude_pass_types_and_drops_bad_handles() {
        let c = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::migrate_for_tests(&c).unwrap();
        c.execute_batch(
            "INSERT INTO contacts (id, category, name, instagram) VALUES (50, 'promoter', 'Metropolis', 'metropolisstudios');
             INSERT INTO vi_promoter_orgs (id, cle, nom, nom_normalise, instagram, crm_contact_id) VALUES
               (1, 'n:a', 'Metropolis Club', 'metropolisclub', 'metropolisstudios', 50),
               (2, 'n:b', 'Good Crew', 'goodcrew', 'goodcrew', NULL),
               (3, 'n:c', 'Hand Set', 'handset', 'handset', NULL);
             UPDATE vi_promoter_orgs SET org_type = 'club', org_type_source = 'manual' WHERE id = 3;",
        )
        .unwrap();
        let fake = FakeLlm::new(&[r#"{"items":[
            {"id":1,"type":"club","instagram_ok":false,"reason":"un club londonien, compte d'un studio"},
            {"id":2,"type":"collectif","instagram_ok":true,"reason":"collectif itinérant"}]}"#]);
        let s = classify_with(&c, &fake, false).unwrap();
        assert_eq!((s.checked, s.collectif, s.other, s.instagram_rejected), (2, 1, 1, 1));
        let (t, ig, rej): (String, Option<String>, Option<String>) = c
            .query_row("SELECT org_type, instagram, instagram_rejected FROM vi_promoter_orgs WHERE id = 1", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap();
        assert_eq!((t.as_str(), ig, rej.as_deref()), ("club", None, Some("metropolisstudios")));
        let cig: Option<String> = c.query_row("SELECT instagram FROM contacts WHERE id = 50", [], |r| r.get(0)).unwrap();
        assert_eq!(cig, None);
        // The manual one was not even sent.
        assert!(!fake.seen.lock().unwrap()[0].user.contains("Hand Set"));
        // Nothing left to check without force.
        assert_eq!(classify_with(&c, &FakeLlm::new(&[]), false).unwrap().checked, 0);
    }
}

/// Dry run on a copy of a real database: PIERSCRM_DB=/path/copy.db cargo test
/// --lib org_type::real_db -- --ignored --nocapture
#[cfg(test)]
mod real_db {
    #[test]
    #[ignore]
    fn real_db() {
        let path = std::env::var("PIERSCRM_DB").expect("PIERSCRM_DB");
        let c = rusqlite::Connection::open(path).unwrap();
        crate::db::migrate_for_tests(&c).unwrap();
        let mut st = c.prepare("SELECT org_type, COUNT(*) FROM vi_promoter_orgs GROUP BY 1 ORDER BY 2 DESC").unwrap();
        for r in st.query_map([], |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, i64>(1)?))).unwrap().flatten() {
            println!("{:?}", r);
        }
        let mut st = c
            .prepare(
                "SELECT nom, org_type, COALESCE(org_type_note,''), COALESCE(instagram,''), COALESCE(instagram_rejected,'')
                 FROM vi_promoter_orgs o WHERE EXISTS(SELECT 1 FROM vi_promoter_contacts c WHERE c.promoter_id = o.id AND c.type = 'email')
                    OR instagram_rejected IS NOT NULL OR nom LIKE 'Sacr%142'
                 ORDER BY nb_events_total DESC LIMIT 140",
            )
            .unwrap();
        for r in st
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?)))
            .unwrap()
            .flatten()
        {
            println!("{:32} {:10} ig={:22} rej={:22} {}", r.0, r.1, r.3, r.4, r.2);
        }
    }
}
