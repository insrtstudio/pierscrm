//! Organisations above contacts (audit item 9): one programmer or group (Paris
//! Society, a promoter collective) behind several venues.
//!
//! Grouping is a *proposal*: `propose_groups` reads the contacts' addresses and
//! suggests organisations, the user accepts, renames or dismisses each one in
//! the Pipeline. Nothing is grouped without that validation. The CRUD commands
//! (list / save / delete) live in contacts.rs.

use crate::models::Contact;
use crate::AppState;
use rusqlite::{params, Connection};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use tauri::State;

/// Webmail and ISP domains: thousands of unrelated people share them, so a
/// common domain there says nothing. Only an exact shared address is evidence.
const FREE_MAIL_DOMAINS: &[&str] = &[
    "gmail.com", "googlemail.com", "hotmail.com", "hotmail.fr", "hotmail.co.uk", "hotmail.it",
    "hotmail.de", "hotmail.es", "hotmail.nl", "outlook.com", "outlook.fr", "outlook.de",
    "outlook.es", "outlook.it", "live.com", "live.fr", "live.nl", "live.co.uk", "msn.com",
    "yahoo.com", "yahoo.fr", "yahoo.co.uk", "yahoo.de", "yahoo.es", "yahoo.it", "ymail.com",
    "icloud.com", "me.com", "mac.com", "aol.com", "aol.fr", "orange.fr", "wanadoo.fr", "free.fr",
    "sfr.fr", "neuf.fr", "laposte.net", "bbox.fr", "numericable.fr", "club-internet.fr",
    "gmx.com", "gmx.de", "gmx.net", "gmx.fr", "web.de", "t-online.de", "freenet.de",
    "proton.me", "protonmail.com", "pm.me", "tutanota.com", "zoho.com", "mail.com", "mail.ru",
    "yandex.com", "yandex.ru", "libero.it", "virgilio.it", "tiscali.it", "alice.it",
    "telenet.be", "skynet.be", "ziggo.nl", "kpnmail.nl", "planet.nl", "home.nl", "btinternet.com",
    "sky.com", "virginmedia.com", "qq.com", "163.com", "naver.com", "seznam.cz", "wp.pl",
    "onet.pl", "o2.pl", "interia.pl",
];

/// A shared address only means "one organisation" between 2 and this many
/// contacts: beyond, it is an address scraped off a directory (audit threshold).
const EMAIL_GROUP_MAX: usize = 3;
/// Same idea for a shared domain: a real group runs a handful of rooms, a
/// domain behind dozens of contacts is a platform the blocklists missed.
const DOMAIN_GROUP_MAX: usize = 25;

const DISMISSED_KEY: &str = "org_proposals_dismissed";

pub fn is_free_mail(domain: &str) -> bool {
    let d = domain.trim().to_lowercase();
    FREE_MAIL_DOMAINS.iter().any(|f| d == *f)
}

/// One contact as the grouping sees it.
#[derive(Debug, Clone, Default)]
pub struct GroupInput {
    pub id: i64,
    pub name: String,
    pub email: Option<String>,
    pub promoter: Option<String>,
    pub organisation_id: Option<i64>,
}

/// A suggested organisation. `contact_ids` are the contacts the acceptance
/// would attach (the ones not in an organisation yet); `organisation_id` is set
/// when the group should join an existing organisation instead of a new one.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OrgProposal {
    /// Stable id of the proposal ("domain:x.com" / "email:a@x.com"), used to dismiss it.
    pub key: String,
    /// "domain" | "email"
    pub kind: String,
    pub name: String,
    pub email_domain: String,
    pub shared_email: Option<String>,
    pub contact_ids: Vec<i64>,
    pub contact_names: Vec<String>,
    pub organisation_id: Option<i64>,
    pub organisation_name: Option<String>,
}

fn norm_email(e: &str) -> Option<(String, String)> {
    let e = e.trim().to_lowercase();
    if !crate::crm_links::is_org_address(&e) {
        return None;
    }
    let domain = e.split_once('@')?.1.to_string();
    Some((e, domain))
}

/// Default name: the promoter most members share, else the domain without its
/// TLD ("parissociety.com" -> "Parissociety"), else the address.
fn suggest_name(members: &[&GroupInput], domain: Option<&str>, email: &str) -> String {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for m in members {
        if let Some(p) = m.promoter.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
            *counts.entry(p).or_default() += 1;
        }
    }
    if let Some((p, _)) = counts
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(a.0)))
    {
        return p.to_string();
    }
    if let Some(d) = domain {
        let base = d.rsplit_once('.').map(|(b, _)| b).unwrap_or(d);
        let base = base.rsplit('.').next().unwrap_or(base);
        let mut c = base.chars();
        if let Some(f) = c.next() {
            return f.to_uppercase().collect::<String>() + c.as_str();
        }
    }
    email.to_string()
}

/// Pure grouping: contacts on the same non-webmail domain form one proposal;
/// on webmail only an exact shared address does. Groups whose members already
/// sit in two different organisations are left alone (sorted by hand), groups
/// fully inside one organisation are done, and a group partly inside one
/// organisation proposes to attach the rest to it. `orgs` is (id, name,
/// email_domain) so a domain already claimed by an organisation joins it.
/// Largest groups first.
pub fn propose_groups(
    contacts: &[GroupInput],
    orgs: &[(i64, String, Option<String>)],
    dismissed: &HashSet<String>,
) -> Vec<OrgProposal> {
    // key -> (kind, domain, shared email, members)
    let mut groups: HashMap<String, (&'static str, String, Option<String>, Vec<&GroupInput>)> = HashMap::new();
    for c in contacts {
        let Some((email, domain)) = c.email.as_deref().and_then(norm_email) else { continue };
        if is_free_mail(&domain) {
            groups
                .entry(format!("email:{email}"))
                .or_insert_with(|| ("email", domain.clone(), Some(email.clone()), Vec::new()))
                .3
                .push(c);
        } else {
            groups
                .entry(format!("domain:{domain}"))
                .or_insert_with(|| ("domain", domain.clone(), None, Vec::new()))
                .3
                .push(c);
        }
    }
    let org_name: HashMap<i64, &str> = orgs.iter().map(|(id, n, _)| (*id, n.as_str())).collect();
    let mut out = Vec::new();
    for (key, (kind, domain, shared, members)) in groups {
        let max = if kind == "email" { EMAIL_GROUP_MAX } else { DOMAIN_GROUP_MAX };
        if members.len() < 2 || members.len() > max || dismissed.contains(&key) {
            continue;
        }
        let in_orgs: HashSet<i64> = members.iter().filter_map(|m| m.organisation_id).collect();
        let claimed: Vec<i64> = if kind == "domain" {
            orgs.iter()
                .filter(|(_, _, d)| d.as_deref().map(|d| d.trim().eq_ignore_ascii_case(&domain)).unwrap_or(false))
                .map(|(id, _, _)| *id)
                .collect()
        } else {
            Vec::new()
        };
        let target = match (in_orgs.len(), claimed.as_slice()) {
            (0, []) => None,
            (0, [one]) => Some(*one),
            (1, _) => in_orgs.iter().next().copied(),
            _ => continue,
        };
        if let (Some(t), [one]) = (target, claimed.as_slice()) {
            if t != *one {
                continue;
            }
        }
        let mut free: Vec<&GroupInput> = members.iter().copied().filter(|m| m.organisation_id.is_none()).collect();
        if free.is_empty() || (target.is_none() && free.len() < 2) {
            continue;
        }
        free.sort_by_key(|m| m.id);
        let email = shared.clone().unwrap_or_default();
        out.push(OrgProposal {
            key,
            kind: kind.to_string(),
            name: suggest_name(&members, (kind == "domain").then_some(domain.as_str()), &email),
            email_domain: domain,
            shared_email: shared,
            contact_ids: free.iter().map(|m| m.id).collect(),
            contact_names: free.iter().map(|m| m.name.clone()).collect(),
            organisation_id: target,
            organisation_name: target.and_then(|t| org_name.get(&t).map(|s| s.to_string())),
        });
    }
    out.sort_by(|a, b| b.contact_ids.len().cmp(&a.contact_ids.len()).then_with(|| a.key.cmp(&b.key)));
    out
}

fn dismissed(conn: &Connection) -> HashSet<String> {
    conn.query_row("SELECT value FROM settings WHERE key=?1", [DISMISSED_KEY], |r| r.get::<_, String>(0))
        .ok()
        .and_then(|v| serde_json::from_str::<Vec<String>>(&v).ok())
        .unwrap_or_default()
        .into_iter()
        .collect()
}

pub fn proposals(conn: &Connection) -> rusqlite::Result<Vec<OrgProposal>> {
    let contacts = conn
        .prepare(
            "SELECT id, name, email, promoter, organisation_id FROM contacts
             WHERE email IS NOT NULL AND email LIKE '%@%'",
        )?
        .query_map([], |r| {
            Ok(GroupInput {
                id: r.get(0)?,
                name: r.get(1)?,
                email: r.get(2)?,
                promoter: r.get(3)?,
                organisation_id: r.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let orgs = conn
        .prepare("SELECT id, name, email_domain FROM organisations")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(propose_groups(&contacts, &orgs, &dismissed(conn)))
}

#[tauri::command(async)]
pub fn list_organisation_proposals(state: State<'_, AppState>) -> Result<Vec<OrgProposal>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    proposals(&conn).map_err(|e| e.to_string())
}

/// Validate a proposal: create the organisation (or reuse `organisation_id`)
/// and attach the given contacts that are not in another organisation yet.
/// Returns the organisation id.
pub fn accept(
    conn: &mut Connection,
    name: &str,
    email_domain: Option<&str>,
    contact_ids: &[i64],
    organisation_id: Option<i64>,
) -> Result<i64, String> {
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let org = match organisation_id {
        Some(id) => id,
        None => {
            let name = name.trim();
            if name.is_empty() {
                return Err("name required".into());
            }
            tx.execute(
                "INSERT INTO organisations (name, email_domain) VALUES (?1, ?2)",
                params![name, email_domain.map(str::trim).filter(|d| !d.is_empty())],
            )
            .map_err(|e| e.to_string())?;
            tx.last_insert_rowid()
        }
    };
    for id in contact_ids {
        tx.execute(
            "UPDATE contacts SET organisation_id=?1, updated_at=datetime('now')
             WHERE id=?2 AND organisation_id IS NULL",
            params![org, id],
        )
        .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(org)
}

#[tauri::command(async)]
pub fn accept_organisation_proposal(
    state: State<'_, AppState>,
    name: String,
    email_domain: Option<String>,
    contact_ids: Vec<i64>,
    organisation_id: Option<i64>,
) -> Result<i64, String> {
    let mut conn = state.pool.get().map_err(|e| e.to_string())?;
    accept(&mut conn, &name, email_domain.as_deref(), &contact_ids, organisation_id)
}

/// "Not an organisation": the proposal stops coming back.
#[tauri::command(async)]
pub fn dismiss_organisation_proposal(state: State<'_, AppState>, key: String) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let mut keys: Vec<String> = dismissed(&conn).into_iter().collect();
    if !keys.contains(&key) {
        keys.push(key);
    }
    keys.sort();
    conn.execute(
        "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
        params![DISMISSED_KEY, serde_json::to_string(&keys).unwrap_or_default()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Pipeline selection bar: put many contacts in one organisation (None detaches).
#[tauri::command(async)]
pub fn set_contacts_organisation(
    state: State<'_, AppState>,
    ids: Vec<i64>,
    organisation_id: Option<i64>,
) -> Result<usize, String> {
    let mut conn = state.pool.get().map_err(|e| e.to_string())?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let mut n = 0;
    {
        let mut stmt = tx
            .prepare(
                "UPDATE contacts SET organisation_id=?2, updated_at=datetime('now')
                 WHERE id=?1 AND organisation_id IS NOT ?2",
            )
            .map_err(|e| e.to_string())?;
        for id in ids {
            n += stmt.execute(params![id, organisation_id]).map_err(|e| e.to_string())?;
        }
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(n)
}

#[derive(Serialize)]
pub struct OrgVenue {
    pub id: i64,
    pub nom: String,
    pub ville: Option<String>,
    pub pays: Option<String>,
    pub contact_id: i64,
}

#[derive(Serialize)]
pub struct OrganisationDetail {
    pub contacts: Vec<Contact>,
    /// Discovery venues promoted into those contacts.
    pub venues: Vec<OrgVenue>,
}

#[tauri::command(async)]
pub fn get_organisation_detail(state: State<'_, AppState>, id: i64) -> Result<OrganisationDetail, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let contacts = conn
        .prepare(
            "SELECT c.*, o.name AS organisation_name FROM contacts c
             LEFT JOIN organisations o ON o.id = c.organisation_id
             WHERE c.organisation_id = ?1 ORDER BY c.name COLLATE NOCASE",
        )
        .map_err(|e| e.to_string())?
        .query_map([id], Contact::from_row)
        .map_err(|e| e.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())?;
    let venues = conn
        .prepare(
            "SELECT v.id, v.nom, v.ville, v.pays, v.crm_contact_id FROM vi_venues v
             JOIN contacts c ON c.id = v.crm_contact_id
             WHERE c.organisation_id = ?1 ORDER BY v.nom COLLATE NOCASE",
        )
        .and_then(|mut s| {
            s.query_map([id], |r| {
                Ok(OrgVenue { id: r.get(0)?, nom: r.get(1)?, ville: r.get(2)?, pays: r.get(3)?, contact_id: r.get(4)? })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
        })
        // Discovery tables may be missing on a fresh profile: no venues then.
        .unwrap_or_default();
    Ok(OrganisationDetail { contacts, venues })
}

/// Bulk send: has this organisation already received this campaign (through
/// any of its contacts)? One programmer behind several venues is mailed once.
pub fn org_reached_in_campaign(conn: &Connection, campaign_id: i64, organisation_id: i64) -> bool {
    conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM emails e JOIN contacts c ON c.id = e.contact_id
                        WHERE e.campaign_id = ?1 AND e.status = 'sent' AND c.organisation_id = ?2)",
        params![campaign_id, organisation_id],
        |r| r.get::<_, i64>(0),
    )
    .map(|v| v != 0)
    .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(id: i64, email: &str, org: Option<i64>) -> GroupInput {
        GroupInput { id, name: format!("C{id}"), email: Some(email.into()), promoter: None, organisation_id: org }
    }
    fn none() -> HashSet<String> {
        HashSet::new()
    }

    #[test]
    fn domain_groups_but_never_webmail() {
        let contacts = vec![
            c(1, "booking@parissociety.com", None),
            c(2, "Prog@ParisSociety.com ", None),
            c(3, "a.martin@gmail.com", None),
            c(4, "b.durand@gmail.com", None),
            c(5, "x@hotmail.fr", None),
            c(6, "y@hotmail.fr", None),
            c(7, "solo@club.nl", None),
            c(8, "info@orange.fr", None),
            c(9, "contact@orange.fr", None),
        ];
        let p = propose_groups(&contacts, &[], &none());
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].key, "domain:parissociety.com");
        assert_eq!(p[0].kind, "domain");
        assert_eq!(p[0].contact_ids, vec![1, 2]);
        assert_eq!(p[0].name, "Parissociety");
        assert_eq!(p[0].organisation_id, None);
    }

    #[test]
    fn exact_webmail_address_groups() {
        let mut a = c(1, "dj.collective@gmail.com", None);
        a.promoter = Some("Collectif Nuit".into());
        let contacts = vec![a, c(2, "DJ.Collective@gmail.com", None), c(3, "other@gmail.com", None)];
        let p = propose_groups(&contacts, &[], &none());
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].kind, "email");
        assert_eq!(p[0].shared_email.as_deref(), Some("dj.collective@gmail.com"));
        assert_eq!(p[0].contact_ids, vec![1, 2]);
        assert_eq!(p[0].name, "Collectif Nuit");
    }

    #[test]
    fn size_limits_and_platforms() {
        // A webmail address shared by 4 contacts is a scraped directory entry.
        let many: Vec<_> = (1..=4).map(|i| c(i, "tickets@gmail.com", None)).collect();
        assert!(propose_groups(&many, &[], &none()).is_empty());
        // Ticketing / compliance addresses are never an organisation.
        let plat = vec![c(1, "custcare@skiddle.com", None), c(2, "help@skiddle.com", None)];
        assert!(propose_groups(&plat, &[], &none()).is_empty());
        let big: Vec<_> = (1..=30).map(|i| c(i, &format!("v{i}@bigplatform.com"), None)).collect();
        assert!(propose_groups(&big, &[], &none()).is_empty());
    }

    #[test]
    fn existing_organisations_and_dismissals() {
        let orgs = vec![(10, "Paris Society".to_string(), Some("parissociety.com".to_string()))];
        // Partly grouped: propose to attach the rest to the existing one.
        let contacts = vec![
            c(1, "a@parissociety.com", Some(10)),
            c(2, "b@parissociety.com", None),
            c(3, "c@parissociety.com", None),
        ];
        let p = propose_groups(&contacts, &orgs, &none());
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].organisation_id, Some(10));
        assert_eq!(p[0].organisation_name.as_deref(), Some("Paris Society"));
        assert_eq!(p[0].contact_ids, vec![2, 3]);
        // A domain claimed by an organisation pulls even a single new contact in.
        let one = vec![c(1, "a@parissociety.com", Some(10)), c(4, "d@parissociety.com", None)];
        assert_eq!(propose_groups(&one, &orgs, &none())[0].contact_ids, vec![4]);
        // Fully grouped: nothing to do.
        let done = vec![c(1, "a@parissociety.com", Some(10)), c(2, "b@parissociety.com", Some(10))];
        assert!(propose_groups(&done, &orgs, &none()).is_empty());
        // Split across two organisations by hand: left alone.
        let split = vec![c(1, "a@x-group.com", Some(1)), c(2, "b@x-group.com", Some(2)), c(3, "c@x-group.com", None)];
        assert!(propose_groups(&split, &[], &none()).is_empty());
        // Dismissed.
        let fresh = vec![c(1, "a@x-group.com", None), c(2, "b@x-group.com", None)];
        let d: HashSet<String> = ["domain:x-group.com".to_string()].into_iter().collect();
        assert!(propose_groups(&fresh, &[], &d).is_empty());
        assert_eq!(propose_groups(&fresh, &[], &none()).len(), 1);
    }

    #[test]
    fn accept_and_campaign_reach() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE organisations (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL,
                email_domain TEXT, notes TEXT, created_at TEXT DEFAULT (datetime('now')));
             CREATE TABLE contacts (id INTEGER PRIMARY KEY, name TEXT, email TEXT, organisation_id INTEGER,
                updated_at TEXT);
             CREATE TABLE emails (id INTEGER PRIMARY KEY, contact_id INTEGER, campaign_id INTEGER, status TEXT);
             INSERT INTO organisations (id, name) VALUES (5, 'Other');
             INSERT INTO contacts (id, name, email, organisation_id) VALUES
                (1, 'A', 'a@ps.com', NULL), (2, 'B', 'b@ps.com', NULL), (3, 'C', 'c@ps.com', 5);",
        )
        .unwrap();
        let org = accept(&mut conn, "Paris Society", Some("ps.com"), &[1, 2, 3], None).unwrap();
        let in_org: i64 = conn
            .query_row("SELECT COUNT(*) FROM contacts WHERE organisation_id=?1", [org], |r| r.get(0))
            .unwrap();
        // Contact 3 already belonged to another organisation: kept there.
        assert_eq!(in_org, 2);
        assert!(accept(&mut conn, "  ", None, &[1], None).is_err());
        assert!(!org_reached_in_campaign(&conn, 7, org));
        conn.execute("INSERT INTO emails (contact_id, campaign_id, status) VALUES (2, 7, 'sent')", []).unwrap();
        assert!(org_reached_in_campaign(&conn, 7, org));
        assert!(!org_reached_in_campaign(&conn, 8, org));
    }
}
