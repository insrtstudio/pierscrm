//! Address check before a first email goes out.
//!
//! The four bounces of the first campaigns all had a valid mail domain: the
//! mailbox itself did not exist. Checking that from this Mac is not possible
//! (outbound port 25 is closed on home connections), so the check has two
//! layers:
//!
//! - Local, always on: syntax, a misspelt webmail domain (gmial.com), a
//!   throwaway domain, and a domain that cannot receive mail (no MX, no A).
//! - A verification service when its key is set (MillionVerifier, API key in
//!   Réglages, Intégrations): it asks the recipient's server whether the
//!   mailbox exists, without sending anything.
//!
//! Verdicts are cached per address (`email_checks`, CHECK_TTL_DAYS). Only an
//! "invalid" verdict stops a send; "risky" (catch-all servers) and "unknown"
//! (service down, no key) let it go, so the check can never block a campaign.

use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use super::dns::{RType, Resolver};

/// Settings key (secret) of the verification service.
pub const VERIFIER_KEY: &str = "email_verifier_key";
/// A verdict is reused this long.
pub const CHECK_TTL_DAYS: i64 = 45;
/// Addresses checked ahead of time per engine tick.
pub const PRECHECK_PER_TICK: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Valid,
    Risky,
    Unknown,
    Invalid,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Valid => "valid",
            Status::Risky => "risky",
            Status::Unknown => "unknown",
            Status::Invalid => "invalid",
        }
    }
    fn parse(s: &str) -> Status {
        match s {
            "valid" => Status::Valid,
            "risky" => Status::Risky,
            "invalid" => Status::Invalid,
            _ => Status::Unknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Verdict {
    pub status: Status,
    /// Plain-French reason, shown in the log and the notification.
    pub reason: String,
    /// "local" or "service".
    pub source: &'static str,
}

impl Verdict {
    fn local(status: Status, reason: impl Into<String>) -> Self {
        Verdict { status, reason: reason.into(), source: "local" }
    }
}

/// A mailbox check by an outside service.
pub trait MailboxService {
    fn check(&self, addr: &str) -> Result<Verdict, String>;
}

// ---------------- Local layer (pure) ----------------

/// Misspelt webmail domains and what they should be.
const TYPO_DOMAINS: &[(&str, &str)] = &[
    ("gmial.com", "gmail.com"),
    ("gmai.com", "gmail.com"),
    ("gmail.co", "gmail.com"),
    ("gmail.con", "gmail.com"),
    ("gamil.com", "gmail.com"),
    ("gmal.com", "gmail.com"),
    ("gnail.com", "gmail.com"),
    ("hotmial.com", "hotmail.com"),
    ("hotmail.con", "hotmail.com"),
    ("hotmal.com", "hotmail.com"),
    ("hotmai.fr", "hotmail.fr"),
    ("yahooo.com", "yahoo.com"),
    ("yaho.com", "yahoo.com"),
    ("yahoo.con", "yahoo.com"),
    ("outlok.com", "outlook.com"),
    ("outllook.com", "outlook.com"),
    ("outlook.con", "outlook.com"),
    ("icloud.con", "icloud.com"),
    ("iclod.com", "icloud.com"),
    ("wanadoo.fe", "wanadoo.fr"),
    ("orange.fe", "orange.fr"),
];

/// Throwaway inbox domains: nobody books a club from one.
const DISPOSABLE: &[&str] = &[
    "mailinator.com",
    "yopmail.com",
    "yopmail.fr",
    "guerrillamail.com",
    "10minutemail.com",
    "tempmail.com",
    "temp-mail.org",
    "trashmail.com",
    "sharklasers.com",
    "getnada.com",
    "dispostable.com",
    "maildrop.cc",
];

/// Syntax, typo and throwaway checks. None when nothing is wrong locally.
pub fn local_problem(addr: &str) -> Option<Verdict> {
    let a = addr.trim().to_lowercase();
    let Some((local, domain)) = a.rsplit_once('@') else {
        return Some(Verdict::local(Status::Invalid, "adresse mal formée"));
    };
    let ok_local = !local.is_empty()
        && local.len() <= 64
        && !local.starts_with('.')
        && !local.ends_with('.')
        && !local.contains("..")
        && local.chars().all(|c| c.is_ascii_alphanumeric() || "._%+-'&".contains(c));
    let ok_domain = domain.len() >= 4
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !domain.contains("..")
        && domain.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
        && domain.rsplit('.').next().is_some_and(|tld| tld.len() >= 2 && tld.chars().all(|c| c.is_ascii_alphabetic()));
    if !ok_local || !ok_domain {
        return Some(Verdict::local(Status::Invalid, "adresse mal formée"));
    }
    if let Some((_, right)) = TYPO_DOMAINS.iter().find(|(t, _)| *t == domain) {
        return Some(Verdict::local(Status::Invalid, format!("faute de frappe dans le domaine ({} au lieu de {})", domain, right)));
    }
    if DISPOSABLE.contains(&domain) {
        return Some(Verdict::local(Status::Invalid, "adresse jetable"));
    }
    None
}

/// Can the domain receive mail at all: an MX record, else (RFC 5321) an A
/// record. A failed lookup is "unknown", never "invalid".
pub fn domain_verdict(resolver: &dyn Resolver, domain: &str) -> Option<Verdict> {
    match resolver.lookup(domain, RType::Mx) {
        Ok(mx) if !mx.is_empty() => {
            // "0 ." is a null MX (RFC 7505): the domain refuses mail.
            if mx.iter().all(|m| m.split_whitespace().nth(1).is_none_or(|h| h == ".")) {
                return Some(Verdict::local(Status::Invalid, "le domaine refuse le courrier"));
            }
            None
        }
        Ok(_) => match resolver.lookup(domain, RType::A) {
            Ok(a) if !a.is_empty() => None,
            Ok(_) => Some(Verdict::local(Status::Invalid, "le domaine ne reçoit pas de mail (aucun serveur)")),
            Err(e) => Some(Verdict::local(Status::Unknown, format!("DNS injoignable : {}", e))),
        },
        Err(e) => Some(Verdict::local(Status::Unknown, format!("DNS injoignable : {}", e))),
    }
}

/// Full check: local layer, domain, then the service when there is one.
pub fn check(addr: &str, resolver: &dyn Resolver, service: Option<&dyn MailboxService>) -> Verdict {
    if let Some(v) = local_problem(addr) {
        return v;
    }
    let domain = addr.trim().to_lowercase().rsplit_once('@').map(|(_, d)| d.to_string()).unwrap_or_default();
    if let Some(v) = domain_verdict(resolver, &domain) {
        if v.status == Status::Invalid {
            return v;
        }
        // DNS down: the service can still answer.
        if service.is_none() {
            return v;
        }
    }
    match service {
        Some(s) => s.check(addr).unwrap_or_else(|e| Verdict { status: Status::Unknown, reason: e, source: "service" }),
        None => Verdict::local(Status::Unknown, "domaine valide, boîte non vérifiée (pas de service de vérification)"),
    }
}

// ---------------- Verification service ----------------

/// MillionVerifier's single-address API.
pub struct MillionVerifier {
    key: String,
    http: reqwest::blocking::Client,
}

impl MillionVerifier {
    pub fn new(key: String) -> Option<Self> {
        let http = reqwest::blocking::Client::builder().timeout(std::time::Duration::from_secs(25)).build().ok()?;
        Some(MillionVerifier { key, http })
    }
}

/// Verdict from MillionVerifier's JSON (`result`: ok, catch_all, unknown,
/// invalid, disposable, error).
pub fn parse_millionverifier(v: &serde_json::Value) -> Result<Verdict, String> {
    let err = v["error"].as_str().unwrap_or("").trim();
    let result = v["result"].as_str().unwrap_or("");
    if !err.is_empty() && result.is_empty() {
        return Err(format!("service de vérification : {}", err));
    }
    let (status, reason) = match result {
        "ok" => (Status::Valid, "boîte vérifiée"),
        "catch_all" => (Status::Risky, "le serveur accepte toutes les adresses (non vérifiable)"),
        "invalid" => (Status::Invalid, "la boîte n'existe pas"),
        "disposable" => (Status::Invalid, "adresse jetable"),
        "unknown" => (Status::Unknown, "le serveur n'a pas répondu"),
        _ => return Err(format!("service de vérification : réponse inattendue ({})", if err.is_empty() { result } else { err })),
    };
    Ok(Verdict { status, reason: reason.into(), source: "service" })
}

impl MailboxService for MillionVerifier {
    fn check(&self, addr: &str) -> Result<Verdict, String> {
        let url = reqwest::Url::parse_with_params(
            "https://api.millionverifier.com/api/v3/",
            &[("api", self.key.as_str()), ("email", addr), ("timeout", "20")],
        )
        .map_err(|e| e.to_string())?;
        let resp = self.http.get(url).send().map_err(|_| "service de vérification injoignable".to_string())?;
        if !resp.status().is_success() {
            return Err(format!("service de vérification : erreur {}", resp.status().as_u16()));
        }
        let v: serde_json::Value = resp.json().map_err(|e| e.to_string())?;
        parse_millionverifier(&v)
    }
}

pub fn service(conn: &Connection) -> Option<Box<dyn MailboxService>> {
    let key = crate::secrets::read_setting(conn, VERIFIER_KEY).filter(|k| !k.trim().is_empty())?;
    MillionVerifier::new(key.trim().to_string()).map(|s| Box::new(s) as Box<dyn MailboxService>)
}

// ---------------- Cache ----------------

pub fn migrate(conn: &Connection) {
    let _ = conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS email_checks (
            address    TEXT PRIMARY KEY,
            status     TEXT NOT NULL,
            reason     TEXT NOT NULL DEFAULT '',
            source     TEXT NOT NULL DEFAULT 'local',
            checked_at TEXT NOT NULL DEFAULT (datetime('now'))
        );",
    );
}

fn norm(addr: &str) -> String {
    addr.trim().to_lowercase()
}

/// A verdict still fresh for this address. An "unknown" one is only reused
/// for a day, so a service outage does not leave addresses unchecked.
pub fn cached(conn: &Connection, addr: &str, now: DateTime<Utc>) -> Option<Verdict> {
    let row: Option<(String, String, String, String)> = conn
        .query_row(
            "SELECT status, reason, source, checked_at FROM email_checks WHERE address = ?1",
            params![norm(addr)],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()
        .ok()
        .flatten();
    let (status, reason, source, at) = row?;
    let at = super::parse_utc(&at)?;
    let status = Status::parse(&status);
    let ttl = if status == Status::Unknown { Duration::days(1) } else { Duration::days(CHECK_TTL_DAYS) };
    if now - at > ttl {
        return None;
    }
    Some(Verdict { status, reason, source: if source == "service" { "service" } else { "local" } })
}

pub fn store(conn: &Connection, addr: &str, v: &Verdict, now: DateTime<Utc>) {
    let _ = conn.execute(
        "INSERT INTO email_checks (address, status, reason, source, checked_at) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(address) DO UPDATE SET status = excluded.status, reason = excluded.reason,
            source = excluded.source, checked_at = excluded.checked_at",
        params![norm(addr), v.status.as_str(), v.reason, v.source, super::fmt(now)],
    );
}

/// The cached verdict, else a fresh check (stored).
pub fn verdict_for(
    conn: &Connection,
    addr: &str,
    resolver: &dyn Resolver,
    service: Option<&dyn MailboxService>,
    now: DateTime<Utc>,
) -> Verdict {
    if let Some(v) = cached(conn, addr, now) {
        return v;
    }
    let v = check(addr, resolver, service);
    store(conn, addr, &v, now);
    v
}

/// Suppression note of an address found invalid before sending (push notices
/// and the bounce rate tell it apart from a real bounce).
pub const INVALID_NOTE: &str = "vérifiée avant envoi";

/// Before a first email: false when the address must not be mailed. An
/// invalid address is suppressed like a bounce (dropped from the contact,
/// notified, the sequence stops) and the campaign goes on with the next one.
pub fn gate_first_email(conn: &Connection, addr: &str, now: DateTime<Utc>) -> Result<(), String> {
    // The engine tests run pick_next offline; the check itself is tested above.
    if cfg!(test) {
        return Ok(());
    }
    let resolver = super::dns::DohResolver::new().ok();
    let svc = service(conn);
    let v = match &resolver {
        Some(r) => verdict_for(conn, addr, r, svc.as_deref(), now),
        None => return Ok(()),
    };
    if v.status != Status::Invalid {
        return Ok(());
    }
    crate::commands::outbox::suppress(conn, addr, "invalid", Some(&format!("{} : {}", INVALID_NOTE, v.reason)));
    Err(v.reason)
}

/// Engine tick: check ahead the addresses of first emails due within a day,
/// so the send itself rarely waits on the network.
pub fn precheck(conn: &Connection, now: DateTime<Utc>) -> usize {
    if cfg!(test) {
        return 0;
    }
    let horizon = super::fmt(now + Duration::days(1));
    let addrs: Vec<String> = conn
        .prepare(
            "SELECT DISTINCT lower(trim(k.email)) FROM campaign_messages m
             JOIN contacts k ON k.id = m.contact_id
             JOIN residency_campaigns r ON r.campaign_id = m.campaign_id
             WHERE m.status = 'scheduled' AND m.step = 1 AND r.autopilot = 1
               AND (m.scheduled_for IS NULL OR m.scheduled_for <= ?1)
               AND trim(COALESCE(k.email, '')) <> ''
             LIMIT 200",
        )
        .and_then(|mut st| st.query_map(params![horizon], |r| r.get(0)).map(|it| it.flatten().collect()))
        .unwrap_or_default();
    let todo: Vec<String> = addrs.into_iter().filter(|a| cached(conn, a, now).is_none()).take(PRECHECK_PER_TICK).collect();
    if todo.is_empty() {
        return 0;
    }
    let Ok(resolver) = super::dns::DohResolver::new() else { return 0 };
    let svc = service(conn);
    for a in &todo {
        let v = check(a, &resolver, svc.as_deref());
        store(conn, a, &v, now);
    }
    todo.len()
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct CheckStats {
    pub service_set: bool,
    pub checked: i64,
    pub valid: i64,
    pub risky: i64,
    pub unknown: i64,
    pub invalid: i64,
}

pub fn stats(conn: &Connection) -> CheckStats {
    let n = |s: &str| -> i64 {
        conn.query_row("SELECT COUNT(*) FROM email_checks WHERE status = ?1", params![s], |r| r.get(0)).unwrap_or(0)
    };
    let (valid, risky, unknown, invalid) = (n("valid"), n("risky"), n("unknown"), n("invalid"));
    CheckStats {
        service_set: crate::secrets::read_setting(conn, VERIFIER_KEY).is_some_and(|k| !k.trim().is_empty()),
        checked: valid + risky + unknown + invalid,
        valid,
        risky,
        unknown,
        invalid,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct Dns(HashMap<(String, bool), Result<Vec<String>, String>>);
    impl Resolver for Dns {
        fn lookup(&self, name: &str, rtype: RType) -> Result<Vec<String>, String> {
            self.0.get(&(name.to_string(), rtype == RType::Mx)).cloned().unwrap_or(Ok(vec![]))
        }
    }
    fn dns(entries: &[(&str, bool, Result<Vec<&str>, &str>)]) -> Dns {
        Dns(entries
            .iter()
            .map(|(n, mx, r)| {
                ((n.to_string(), *mx), r.clone().map(|v| v.iter().map(|s| s.to_string()).collect()).map_err(|e| e.to_string()))
            })
            .collect())
    }
    struct Svc(&'static str);
    impl MailboxService for Svc {
        fn check(&self, _: &str) -> Result<Verdict, String> {
            parse_millionverifier(&serde_json::json!({ "result": self.0, "error": "" }))
        }
    }

    #[test]
    fn local_layer() {
        assert!(local_problem("booking@club.fr").is_none());
        assert!(local_problem("o'brien@club.ie").is_none());
        assert_eq!(local_problem("booking.club.fr").unwrap().status, Status::Invalid);
        assert_eq!(local_problem("a..b@club.fr").unwrap().status, Status::Invalid);
        assert_eq!(local_problem("a@club").unwrap().status, Status::Invalid);
        assert!(local_problem("x@gmial.com").unwrap().reason.contains("gmail.com"));
        assert_eq!(local_problem("x@yopmail.com").unwrap().reason, "adresse jetable");
    }

    #[test]
    fn domain_layer() {
        let d = dns(&[
            ("club.fr", true, Ok(vec!["10 mx.club.fr."])),
            ("old.fr", true, Ok(vec![])),
            ("old.fr", false, Ok(vec![])),
            ("bare.fr", true, Ok(vec![])),
            ("bare.fr", false, Ok(vec!["1.2.3.4"])),
            ("null.fr", true, Ok(vec!["0 ."])),
            ("down.fr", true, Err("offline")),
        ]);
        assert!(domain_verdict(&d, "club.fr").is_none());
        assert!(domain_verdict(&d, "bare.fr").is_none());
        assert_eq!(domain_verdict(&d, "old.fr").unwrap().status, Status::Invalid);
        assert_eq!(domain_verdict(&d, "null.fr").unwrap().status, Status::Invalid);
        assert_eq!(domain_verdict(&d, "down.fr").unwrap().status, Status::Unknown);
    }

    #[test]
    fn full_check_and_service() {
        let d = dns(&[("club.fr", true, Ok(vec!["10 mx.club.fr."])), ("old.fr", true, Ok(vec![]))]);
        assert_eq!(check("a@club.fr", &d, None).status, Status::Unknown);
        assert_eq!(check("a@club.fr", &d, Some(&Svc("ok"))).status, Status::Valid);
        assert_eq!(check("a@club.fr", &d, Some(&Svc("invalid"))).status, Status::Invalid);
        assert_eq!(check("a@club.fr", &d, Some(&Svc("catch_all"))).status, Status::Risky);
        // A dead domain never reaches the (paid) service.
        assert_eq!(check("a@old.fr", &d, Some(&Svc("ok"))).status, Status::Invalid);
        assert!(parse_millionverifier(&serde_json::json!({ "result": "", "error": "Insufficient credits" })).is_err());
    }

    #[test]
    fn cache_ttl() {
        let c = Connection::open_in_memory().unwrap();
        migrate(&c);
        let now = Utc::now();
        let v = Verdict { status: Status::Valid, reason: "ok".into(), source: "service" };
        store(&c, " A@Club.fr ", &v, now - Duration::days(10));
        assert_eq!(cached(&c, "a@club.fr", now).unwrap().status, Status::Valid);
        assert!(cached(&c, "a@club.fr", now + Duration::days(CHECK_TTL_DAYS)).is_none());
        let u = Verdict { status: Status::Unknown, reason: "?".into(), source: "local" };
        store(&c, "b@club.fr", &u, now - Duration::hours(30));
        assert!(cached(&c, "b@club.fr", now).is_none());
    }
}

/// Réglages, Envoi et délivrabilité: how many addresses were checked and how.
#[tauri::command(async)]
pub fn email_check_stats(state: tauri::State<'_, crate::AppState>) -> Result<CheckStats, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    Ok(stats(&conn))
}
