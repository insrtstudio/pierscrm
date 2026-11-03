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
//! Verdicts are cached per address (`email_checks`, CHECK_TTL_DAYS). What a
//! verdict does to a send (`gate_first_email`, booking@ reputation first):
//! - invalid: never sent, dropped like a bounce, the campaign moves on;
//! - risky (catch-all server, may bounce later): at most RISKY_PER_DAY first
//!   emails a day, and none while the recent bounce rate is above
//!   RISKY_MAX_BOUNCE_PCT; the rest wait for the next day;
//! - unknown from the service (the server did not answer): waits and is
//!   checked again the next day, then counts as risky after UNKNOWN_GRACE_DAYS;
//! - no service set: only the local layer applies, unknown goes.
//! Every send to an address never mailed before (manual, bulk, campaign) also
//! goes through `guard_new_address`, and the collectives refill only picks
//! addresses that passed (`pick_ok`).

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
/// First emails a day to catch-all addresses.
pub const RISKY_PER_DAY: i64 = 3;
/// No catch-all first email while the bounce rate of the last 14 days is above this.
pub const RISKY_MAX_BOUNCE_PCT: f64 = 2.0;
/// An address the service could not settle waits this long, then counts as risky.
pub const UNKNOWN_GRACE_DAYS: i64 = 2;

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
    // When the address was first checked (UNKNOWN_GRACE_DAYS counts from it).
    let _ = conn.execute("ALTER TABLE email_checks ADD COLUMN first_seen TEXT", []);
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
        "INSERT INTO email_checks (address, status, reason, source, checked_at, first_seen) VALUES (?1, ?2, ?3, ?4, ?5, ?5)
         ON CONFLICT(address) DO UPDATE SET status = excluded.status, reason = excluded.reason,
            source = excluded.source, checked_at = excluded.checked_at,
            first_seen = COALESCE(email_checks.first_seen, excluded.first_seen)",
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
        // A local-only "unknown" says nothing about the mailbox: once the
        // service is set, ask it.
        if !(service.is_some() && v.source == "local" && v.status != Status::Invalid) {
            return v;
        }
    }
    let v = check(addr, resolver, service);
    store(conn, addr, &v, now);
    v
}

/// Suppression note of an address found invalid before sending (push notices
/// and the bounce rate tell it apart from a real bounce).
pub const INVALID_NOTE: &str = "vérifiée avant envoi";

/// What to do with a first email.
#[derive(Debug, Clone, PartialEq)]
pub enum Gate {
    Send,
    /// Keep it scheduled, try again later (reason shown on the message).
    Wait(String),
    /// Never send: the address was suppressed (reason).
    Drop(String),
}

fn days_since_first_seen(conn: &Connection, addr: &str, now: DateTime<Utc>) -> i64 {
    conn.query_row("SELECT COALESCE(first_seen, checked_at) FROM email_checks WHERE address = ?1", params![norm(addr)], |r| {
        r.get::<_, String>(0)
    })
    .ok()
    .and_then(|at| super::parse_utc(&at))
    .map(|at| (now - at).num_days())
    .unwrap_or(0)
}

/// First emails sent today to addresses checked risky.
pub fn risky_sent_today(conn: &Connection) -> i64 {
    conn.query_row(
        "SELECT COUNT(DISTINCT lower(e.to_addr)) FROM emails e
         JOIN email_checks c ON c.address = lower(trim(e.to_addr))
         WHERE e.status = 'sent' AND c.status = 'risky'
           AND date(e.sent_at, 'localtime') = date('now', 'localtime')",
        [],
        |r| r.get(0),
    )
    .unwrap_or(0)
}

/// Bounces over sends of the last 14 days (or since the health reset), in
/// percent (0 with few sends).
pub fn recent_bounce_pct(conn: &Connection) -> f64 {
    let (sent, bounced): (i64, i64) = conn
        .query_row(
            "WITH since AS (SELECT max(datetime('now', '-14 days'),
                    COALESCE((SELECT value FROM settings WHERE key = 'deliverability_health_reset_at'), '')) AS at)
             SELECT (SELECT COUNT(*) FROM emails, since WHERE status = 'sent' AND sent_at >= since.at),
                    (SELECT COUNT(*) FROM suppressions, since WHERE reason = 'bounce' AND created_at >= since.at)",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap_or((0, 0));
    if sent < 20 {
        return 0.0;
    }
    bounced as f64 * 100.0 / sent as f64
}

/// The rule of the module header for one verdict.
pub fn decide(v: &Verdict, service_set: bool, waited_days: i64, risky_today: i64, bounce_pct: f64) -> Gate {
    let risky = |why: &str| {
        if bounce_pct > RISKY_MAX_BOUNCE_PCT {
            Gate::Wait(format!("{why}, en attente tant que le taux de rebond dépasse {RISKY_MAX_BOUNCE_PCT} %"))
        } else if risky_today >= RISKY_PER_DAY {
            Gate::Wait(format!("{why}, quota du jour atteint ({RISKY_PER_DAY} par jour)"))
        } else {
            Gate::Send
        }
    };
    match v.status {
        Status::Valid => Gate::Send,
        Status::Invalid => Gate::Drop(v.reason.clone()),
        Status::Risky => risky("adresse non vérifiable (serveur catch-all)"),
        Status::Unknown if !service_set || v.source == "local" => Gate::Send,
        Status::Unknown if waited_days >= UNKNOWN_GRACE_DAYS => risky("adresse non vérifiable"),
        Status::Unknown => Gate::Wait(format!("vérification à refaire demain ({})", v.reason)),
    }
}

/// Before a first email. An invalid address is suppressed like a bounce
/// (dropped from the contact, notified, the sequence stops).
pub fn gate_first_email(conn: &Connection, addr: &str, now: DateTime<Utc>) -> Gate {
    // The engine tests run pick_next offline; the rule itself is tested below.
    if cfg!(test) {
        return Gate::Send;
    }
    let Ok(resolver) = super::dns::DohResolver::new() else { return Gate::Send };
    let svc = service(conn);
    let v = verdict_for(conn, addr, &resolver, svc.as_deref(), now);
    let g = decide(&v, svc.is_some(), days_since_first_seen(conn, addr, now), risky_sent_today(conn), recent_bounce_pct(conn));
    if let Gate::Drop(why) = &g {
        crate::commands::outbox::suppress(conn, addr, "invalid", Some(&format!("{} : {}", INVALID_NOTE, why)));
    }
    g
}

/// Any send (Compose, bulk, campaigns) to an address never mailed before:
/// Some(reason) when it must not go. Only a dead address is refused here;
/// the risky quota is the autopilot's business.
pub fn guard_new_address(conn: &Connection, addr: &str, now: DateTime<Utc>) -> Option<String> {
    if cfg!(test) {
        return None;
    }
    let mailed: bool = conn
        .query_row("SELECT EXISTS (SELECT 1 FROM emails WHERE lower(trim(to_addr)) = ?1 AND status = 'sent')", params![norm(addr)], |r| {
            r.get(0)
        })
        .unwrap_or(false);
    if mailed {
        return None;
    }
    let resolver = super::dns::DohResolver::new().ok()?;
    let svc = service(conn);
    let v = verdict_for(conn, addr, &resolver, svc.as_deref(), now);
    if v.status != Status::Invalid {
        return None;
    }
    crate::commands::outbox::suppress(conn, addr, "invalid", Some(&format!("{} : {}", INVALID_NOTE, v.reason)));
    Some(v.reason)
}

/// Collectives refill: whether an address may be added to a campaign. An
/// invalid one is suppressed so it is never picked again.
pub fn pick_ok(conn: &Connection, addr: &str, now: DateTime<Utc>) -> bool {
    guard_new_address(conn, addr, now).is_none()
}

/// Engine tick: check ahead the addresses of first emails due within a day,
/// so the send itself rarely waits on the network.
pub fn precheck(conn: &Connection, now: DateTime<Utc>) -> usize {
    if cfg!(test) {
        return 0;
    }
    let horizon = super::fmt(now + Duration::days(2));
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
    let svc = service(conn);
    let todo: Vec<String> = addrs
        .into_iter()
        .filter(|a| match cached(conn, a, now) {
            None => true,
            Some(v) => svc.is_some() && v.source == "local" && v.status != Status::Invalid,
        })
        .take(PRECHECK_PER_TICK)
        .collect();
    if todo.is_empty() {
        return 0;
    }
    let Ok(resolver) = super::dns::DohResolver::new() else { return 0 };
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
    fn send_rule() {
        let v = |status, source| Verdict { status, reason: "r".into(), source };
        assert_eq!(decide(&v(Status::Valid, "service"), true, 0, 9, 9.0), Gate::Send);
        assert!(matches!(decide(&v(Status::Invalid, "local"), false, 0, 0, 0.0), Gate::Drop(_)));
        assert_eq!(decide(&v(Status::Risky, "service"), true, 0, 0, 0.5), Gate::Send);
        assert!(matches!(decide(&v(Status::Risky, "service"), true, 0, RISKY_PER_DAY, 0.5), Gate::Wait(_)));
        assert!(matches!(decide(&v(Status::Risky, "service"), true, 0, 0, 3.0), Gate::Wait(_)));
        assert_eq!(decide(&v(Status::Unknown, "local"), false, 0, 0, 0.0), Gate::Send);
        assert!(matches!(decide(&v(Status::Unknown, "service"), true, 0, 0, 0.0), Gate::Wait(_)));
        assert_eq!(decide(&v(Status::Unknown, "service"), true, UNKNOWN_GRACE_DAYS, 0, 0.0), Gate::Send);
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
