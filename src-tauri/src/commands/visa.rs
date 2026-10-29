use crate::models::{VisaCountry, VisaDossier};
use crate::AppState;
use rusqlite::params;
use tauri::State;

// ---------- Country knowledge base ----------

#[tauri::command(async)]
pub fn list_visa_countries(state: State<'_, AppState>) -> Result<Vec<VisaCountry>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT * FROM visa_countries ORDER BY name COLLATE NOCASE")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], VisaCountry::from_row)
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

#[tauri::command(async)]
pub fn save_visa_country(state: State<'_, AppState>, country: VisaCountry) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO visa_countries (code, name, work_rules, visa_types, processing_time, required_docs, notes, official_link, updated_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,datetime('now'))
         ON CONFLICT(code) DO UPDATE SET
            name=excluded.name, work_rules=excluded.work_rules, visa_types=excluded.visa_types,
            processing_time=excluded.processing_time, required_docs=excluded.required_docs,
            notes=excluded.notes, official_link=excluded.official_link, updated_at=datetime('now')",
        params![
            country.code, country.name, country.work_rules, country.visa_types,
            country.processing_time, country.required_docs, country.notes, country.official_link
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command(async)]
pub fn delete_visa_country(state: State<'_, AppState>, code: String) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM visa_countries WHERE code=?1", params![code])
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ---------- Dossiers ----------

#[tauri::command(async)]
pub fn list_dossiers(state: State<'_, AppState>) -> Result<Vec<VisaDossier>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT * FROM visa_dossiers ORDER BY updated_at DESC, id DESC")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], VisaDossier::from_row)
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

#[tauri::command(async)]
pub fn save_dossier(state: State<'_, AppState>, dossier: VisaDossier) -> Result<i64, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    match dossier.id {
        Some(id) => {
            conn.execute(
                "UPDATE visa_dossiers SET
                    artist_id=?2, country_code=?3, country_name=?4, title=?5, purpose=?6,
                    event_date=?7, entry_date=?8, status=?9, checklist=?10, notes=?11,
                    event_id=?12, updated_at=datetime('now')
                 WHERE id=?1",
                params![
                    id, dossier.artist_id, dossier.country_code, dossier.country_name,
                    dossier.title, dossier.purpose, dossier.event_date, dossier.entry_date,
                    dossier.status, dossier.checklist, dossier.notes, dossier.event_id
                ],
            )
            .map_err(|e| e.to_string())?;
            Ok(id)
        }
        None => {
            conn.execute(
                "INSERT INTO visa_dossiers
                    (artist_id, country_code, country_name, title, purpose, event_date, entry_date, status, checklist, notes, event_id)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
                params![
                    dossier.artist_id, dossier.country_code, dossier.country_name, dossier.title,
                    dossier.purpose, dossier.event_date, dossier.entry_date, dossier.status,
                    dossier.checklist, dossier.notes, dossier.event_id
                ],
            )
            .map_err(|e| e.to_string())?;
            Ok(conn.last_insert_rowid())
        }
    }
}

#[tauri::command(async)]
pub fn delete_dossier(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM visa_dossiers WHERE id=?1", params![id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ---------- Agenda link (audit item 10) ----------

/// The whole visa rule, in one place: an artist from a free movement country
/// (EU, EEA, Switzerland) plays anywhere in it without a visa or work permit;
/// any other destination outside the artist's own country needs one (details
/// in the country sheet). Edit this list to change the rule.
const FREE_MOVEMENT: &[&str] = &[
    "AT", "BE", "BG", "HR", "CY", "CZ", "DK", "EE", "FI", "FR", "DE", "GR", "HU", "IE", "IT",
    "LV", "LT", "LU", "MT", "NL", "PL", "PT", "RO", "SK", "SI", "ES", "SE", "IS", "LI", "NO", "CH",
];

/// Country assumed for an artist whose profile has none (Piers is French).
const DEFAULT_ARTIST_COUNTRY: &str = "FR";

/// Whether an artist from `home` (ISO code, None = default) needs a visa or
/// work permit to play in `target` (ISO code).
fn requires_permit(home: Option<&str>, target: &str) -> bool {
    let home = home.unwrap_or(DEFAULT_ARTIST_COUNTRY);
    if home == target {
        return false;
    }
    !(FREE_MOVEMENT.contains(&home) && FREE_MOVEMENT.contains(&target))
}

/// ISO code from what the app stores: Discovery uses "UK" for Great Britain,
/// artists carry a country name ("France").
fn country_code(conn: &rusqlite::Connection, raw: &str) -> Option<String> {
    let v = raw.trim();
    if v.is_empty() {
        return None;
    }
    if v.len() == 2 {
        let up = v.to_uppercase();
        return Some(if up == "UK" { "GB".into() } else { up });
    }
    let low = v.to_lowercase();
    let known = [
        ("france", "FR"), ("netherlands", "NL"), ("pays-bas", "NL"), ("germany", "DE"),
        ("allemagne", "DE"), ("spain", "ES"), ("espagne", "ES"), ("united kingdom", "GB"),
        ("royaume-uni", "GB"), ("uk", "GB"), ("england", "GB"), ("united states", "US"),
        ("usa", "US"), ("états-unis", "US"), ("etats-unis", "US"), ("canada", "CA"),
        ("switzerland", "CH"), ("suisse", "CH"), ("belgium", "BE"), ("belgique", "BE"),
        ("italy", "IT"), ("italie", "IT"), ("portugal", "PT"), ("ireland", "IE"),
        ("irlande", "IE"), ("scotland", "GB"), ("ecosse", "GB"), ("écosse", "GB"),
        ("norway", "NO"), ("norvège", "NO"), ("japan", "JP"), ("japon", "JP"),
        ("australia", "AU"), ("australie", "AU"), ("brazil", "BR"), ("brésil", "BR"),
        ("mexico", "MX"), ("mexique", "MX"), ("morocco", "MA"), ("maroc", "MA"),
        ("turkey", "TR"), ("turquie", "TR"), ("serbia", "RS"), ("serbie", "RS"),
        ("uae", "AE"), ("dubai", "AE"), ("south africa", "ZA"), ("afrique du sud", "ZA"),
    ];
    if let Some((_, c)) = known.iter().find(|(n, _)| *n == low) {
        return Some((*c).into());
    }
    conn.query_row(
        "SELECT code FROM visa_countries WHERE lower(name)=?1",
        params![low],
        |r| r.get(0),
    )
    .ok()
}

/// Country of a gig. Events only have a city, so: the Discovery venue the
/// linked contact was promoted from, else the most common country of the
/// Discovery venues in that city (city of the event, else of the contact).
fn event_country(
    conn: &rusqlite::Connection,
    contact_id: Option<i64>,
    city: Option<&str>,
) -> Option<String> {
    if let Some(cid) = contact_id {
        let pays: Option<String> = conn
            .query_row(
                "SELECT pays FROM vi_venues WHERE crm_contact_id=?1 AND pays IS NOT NULL LIMIT 1",
                params![cid],
                |r| r.get(0),
            )
            .ok();
        if let Some(c) = pays.and_then(|p| country_code(conn, &p)) {
            return Some(c);
        }
    }
    let contact_city: Option<String> = contact_id.and_then(|cid| {
        conn.query_row("SELECT area FROM contacts WHERE id=?1", params![cid], |r| r.get(0))
            .ok()
            .flatten()
    });
    for c in [city.map(str::to_string), contact_city].into_iter().flatten() {
        // "London, UK" / "Tokyo (Japon)": the country is written in the city.
        if let Some(code) = city_suffix(&c).and_then(|t| country_code(conn, t)) {
            return Some(code);
        }
        let c = c.trim().to_lowercase();
        if c.is_empty() {
            continue;
        }
        let pays: Option<String> = conn
            .query_row(
                "SELECT pays FROM vi_venues WHERE lower(trim(ville))=?1 AND pays IS NOT NULL
                 GROUP BY pays ORDER BY COUNT(*) DESC LIMIT 1",
                params![c],
                |r| r.get(0),
            )
            .ok();
        if let Some(code) = pays.and_then(|p| country_code(conn, &p)) {
            return Some(code);
        }
    }
    None
}

/// Text after the last comma or inside the last parentheses of a city field.
fn city_suffix(city: &str) -> Option<&str> {
    let c = city.trim().trim_end_matches(')');
    let cut = c.rfind([',', '('])?;
    Some(c[cut + 1..].trim()).filter(|t| !t.is_empty())
}

/// Whether the artist needs a work visa to play in `target` (ISO code or a
/// Discovery country), see `requires_permit`. (code, name) when it does; the
/// name comes from the visa module, else the code.
pub(crate) fn visa_needed(conn: &rusqlite::Connection, artist_country: Option<&str>, target: &str) -> Option<(String, String)> {
    let code = country_code(conn, target)?;
    let home = artist_country.and_then(|c| country_code(conn, c));
    if !requires_permit(home.as_deref(), &code) {
        return None;
    }
    let name: String = conn
        .query_row("SELECT name FROM visa_countries WHERE code=?1", params![code], |r| r.get(0))
        .unwrap_or_else(|_| code.clone());
    Some((code, name))
}

#[derive(serde::Serialize)]
pub struct EventVisa {
    pub event_id: i64,
    /// Inferred country of the gig (ISO code), when it could be found.
    pub country_code: Option<String>,
    pub country_name: Option<String>,
    /// Dossier linked to this event (or, failing that, one for the same
    /// artist, country and date).
    pub dossier_id: Option<i64>,
    pub dossier_status: Option<String>,
    /// The gig is outside the artist's own country and outside the free
    /// movement area shared with it (`requires_permit`).
    pub needs_visa: bool,
    /// Confirmed gig (holds are listed too): the "no dossier" alert is only
    /// raised for these.
    pub confirmed: bool,
}

/// Visa situation of every non-cancelled event between `from` and `to`.
#[tauri::command(async)]
pub fn list_event_visas(
    state: State<'_, AppState>,
    from: Option<String>,
    to: Option<String>,
) -> Result<Vec<EventVisa>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    event_visas(&conn, from.as_deref(), to.as_deref()).map_err(|e| e.to_string())
}

pub(crate) fn event_visas(
    conn: &rusqlite::Connection,
    from: Option<&str>,
    to: Option<&str>,
) -> rusqlite::Result<Vec<EventVisa>> {
    let listed: Vec<(String, String)> = {
        let mut stmt = conn.prepare("SELECT code, name FROM visa_countries")?;
        let it = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        it.collect::<rusqlite::Result<_>>()?
    };
    type Ev = (i64, Option<i64>, Option<i64>, Option<String>, String, Option<String>, bool);
    let events: Vec<Ev> = {
        let mut stmt = conn.prepare(
            "SELECT e.id, e.artist_id, e.contact_id, e.city, e.date, a.country, e.status = 'confirmed'
             FROM events e LEFT JOIN artists a ON a.id = e.artist_id
             WHERE e.status <> 'cancelled'
               AND (?1 IS NULL OR e.date >= ?1) AND (?2 IS NULL OR e.date <= ?2)",
        )?;
        let it = stmt.query_map(params![from, to], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?))
        })?;
        it.collect::<rusqlite::Result<_>>()?
    };
    let mut out = Vec::with_capacity(events.len());
    for (id, artist_id, contact_id, city, date, artist_country, confirmed) in events {
        let code = event_country(conn, contact_id, city.as_deref());
        let dossier: Option<(i64, String)> = conn
            .query_row(
                "SELECT id, status FROM visa_dossiers
                 WHERE event_id=?1
                    OR (event_id IS NULL AND event_date=?2 AND ?3 IS NOT NULL AND country_code=?3
                        AND (artist_id IS ?4 OR artist_id IS NULL))
                 ORDER BY event_id IS NULL, updated_at DESC LIMIT 1",
                params![id, date, code, artist_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .ok();
        let home = artist_country.as_deref().and_then(|c| country_code(conn, c));
        let needs_visa = code.as_deref().map_or(false, |c| requires_permit(home.as_deref(), c));
        let country_name = code
            .as_ref()
            .and_then(|c| listed.iter().find(|(l, _)| l == c).map(|(_, n)| n.clone()));
        out.push(EventVisa {
            event_id: id,
            country_code: code,
            country_name,
            dossier_id: dossier.as_ref().map(|d| d.0),
            dossier_status: dossier.map(|d| d.1),
            needs_visa,
            confirmed,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permit_rule() {
        // French artist (also the default when the profile has no country).
        for (target, needed) in [("FR", false), ("NL", false), ("CH", false), ("NO", false), ("GB", true), ("US", true), ("JP", true)] {
            assert_eq!(requires_permit(Some("FR"), target), needed, "{target}");
            assert_eq!(requires_permit(None, target), needed, "{target}");
        }
        assert!(requires_permit(Some("US"), "FR"));
        assert!(!requires_permit(Some("US"), "US"));
    }

    #[test]
    fn city_suffix_reads_the_country() {
        assert_eq!(city_suffix("London, UK"), Some("UK"));
        assert_eq!(city_suffix("Tokyo (Japon)"), Some("Japon"));
        assert_eq!(city_suffix("Amsterdam"), None);
        assert_eq!(city_suffix("Paris,"), None);
    }

    #[test]
    fn event_visas_flags_confirmed_gig_abroad_without_dossier() {
        let c = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::migrate_for_tests(&c).unwrap();
        c.execute("INSERT INTO artists (name, country) VALUES ('Piers', 'France')", []).unwrap();
        let a = c.last_insert_rowid();
        for (title, city, status) in [
            ("Fabric", "London, UK", "confirmed"),
            ("Paradiso", "Amsterdam, Netherlands", "confirmed"),
            ("Output", "New York, USA", "hold"),
        ] {
            c.execute(
                "INSERT INTO events (artist_id, title, city, date, status) VALUES (?1, ?2, ?3, '2026-12-01', ?4)",
                params![a, title, city, status],
            )
            .unwrap();
        }
        let v = event_visas(&c, None, None).unwrap();
        let by = |i: usize| (v[i].country_code.clone(), v[i].needs_visa, v[i].confirmed, v[i].dossier_id);
        assert_eq!(by(0), (Some("GB".into()), true, true, None));
        assert_eq!(by(1), (Some("NL".into()), false, true, None));
        assert_eq!(by(2), (Some("US".into()), true, false, None));
        // A dossier linked to the London gig clears its alert.
        c.execute(
            "INSERT INTO visa_dossiers (title, status, checklist, event_id) VALUES ('UK', 'draft', '[]', ?1)",
            params![v[0].event_id],
        )
        .unwrap();
        assert!(event_visas(&c, None, None).unwrap()[0].dossier_id.is_some());
    }
}
