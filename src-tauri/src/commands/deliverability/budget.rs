//! The global sending budget, shared by every automated path (bulk send,
//! every residency campaign and its follow-ups, Compose): one daily and one
//! hourly cap, a warm-up ramp, randomized spacing between sends and a
//! per-recipient-domain daily cap.
//!
//! Replies don't count: a send to an address that has already written back to
//! us is a conversation, not prospecting, and is never held back.
//!
//! The math (`ramp_limit`, `decide`, `gap_secs`, `org_domain`) is pure; the
//! DB side only counts what was sent.

use chrono::{DateTime, Duration, Local, NaiveDate, TimeZone, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use super::{fmt, get, parse_utc, put};

pub const CONFIG_KEY: &str = "send_budget";
pub const NEXT_AT_KEY: &str = "send_budget_next_at";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BudgetConfig {
    /// Daily limit during the first week of the ramp.
    pub start_per_day: u32,
    /// Growth of the daily limit each week, in percent.
    pub weekly_growth_pct: u32,
    /// The ramp stops here.
    pub ceiling: u32,
    /// Sends in any rolling hour.
    pub hourly_cap: u32,
    /// Sends to one organisation domain per day (freemail exempt).
    pub per_domain_daily: u32,
    /// Random spacing between two automated sends, in seconds.
    pub spacing_min_s: u32,
    pub spacing_max_s: u32,
    /// Shorter spacing for a bulk send the user started and watches.
    pub bulk_spacing_min_s: u32,
    pub bulk_spacing_max_s: u32,
    /// First day of the ramp (YYYY-MM-DD). Empty: the day of the first email
    /// ever sent, else today.
    pub warmup_start: Option<String>,
}

impl Default for BudgetConfig {
    fn default() -> Self {
        BudgetConfig {
            start_per_day: 15,
            weekly_growth_pct: 20,
            ceiling: 150,
            hourly_cap: 12,
            per_domain_daily: 2,
            spacing_min_s: 60,
            spacing_max_s: 180,
            bulk_spacing_min_s: 20,
            bulk_spacing_max_s: 60,
            warmup_start: None,
        }
    }
}

impl BudgetConfig {
    /// Out-of-range values brought back to something safe (a cap can be
    /// raised, never switched off).
    pub fn sanitized(mut self) -> Self {
        self.start_per_day = self.start_per_day.clamp(1, 5000);
        self.weekly_growth_pct = self.weekly_growth_pct.min(200);
        self.ceiling = self.ceiling.clamp(self.start_per_day, 5000);
        self.hourly_cap = self.hourly_cap.clamp(1, 1000);
        self.per_domain_daily = self.per_domain_daily.clamp(1, 100);
        self.spacing_max_s = self.spacing_max_s.min(3600);
        self.spacing_min_s = self.spacing_min_s.min(self.spacing_max_s);
        self.bulk_spacing_max_s = self.bulk_spacing_max_s.min(3600);
        self.bulk_spacing_min_s = self.bulk_spacing_min_s.min(self.bulk_spacing_max_s);
        self.warmup_start = self
            .warmup_start
            .filter(|d| NaiveDate::parse_from_str(d.trim(), "%Y-%m-%d").is_ok());
        self
    }
}

/// Daily limit `days` after the ramp started: start × (1 + g)^week, capped.
pub fn ramp_limit(cfg: &BudgetConfig, days: i64) -> u32 {
    let week = (days.max(0) / 7) as i32;
    let g = 1.0 + cfg.weekly_growth_pct as f64 / 100.0;
    let v = (cfg.start_per_day as f64 * g.powi(week.min(60))).round();
    (v.min(cfg.ceiling as f64) as u32).max(1).min(cfg.ceiling.max(1))
}

/// The ramp, one value per week, until it reaches the ceiling (at most `max`).
pub fn ramp_schedule(cfg: &BudgetConfig, max: usize) -> Vec<u32> {
    let mut out = Vec::new();
    for w in 0..max {
        let v = ramp_limit(cfg, w as i64 * 7);
        out.push(v);
        if v >= cfg.ceiling {
            break;
        }
    }
    out
}

/// Random gap before the next automated send, from a seed.
pub fn gap_secs(cfg: &BudgetConfig, seed: u64) -> i64 {
    gap_between(cfg.spacing_min_s, cfg.spacing_max_s, seed)
}

/// Random gap between two sends of a user-initiated bulk send.
pub fn bulk_gap_secs(cfg: &BudgetConfig) -> i64 {
    gap_between(cfg.bulk_spacing_min_s, cfg.bulk_spacing_max_s, random_seed())
}

fn gap_between(lo: u32, hi: u32, seed: u64) -> i64 {
    let (lo, hi) = (lo as u64, hi.max(lo) as u64);
    // splitmix64: a well-spread value from any seed.
    let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (lo + z % (hi - lo + 1)) as i64
}

fn random_seed() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_i64(Utc::now().timestamp_micros());
    h.finish()
}

// ---------------- Domains ----------------

/// Consumer mailboxes: thousands of unrelated people share them, so the
/// per-domain cap does not apply.
const FREEMAIL: &[&str] = &[
    "gmail.com", "googlemail.com", "hotmail.com", "hotmail.fr", "hotmail.co.uk", "hotmail.be", "outlook.com",
    "outlook.fr", "live.com", "live.fr", "live.be", "msn.com", "yahoo.com", "yahoo.fr", "yahoo.co.uk", "ymail.com",
    "icloud.com", "me.com", "mac.com", "aol.com", "gmx.com", "gmx.de", "gmx.fr", "gmx.ch", "web.de", "orange.fr",
    "wanadoo.fr", "free.fr", "sfr.fr", "laposte.net", "bbox.fr", "proton.me", "protonmail.com", "skynet.be",
    "telenet.be", "bluewin.ch", "btinternet.com", "sky.com", "virginmedia.com", "mail.com", "zoho.com",
];

/// Second-level suffixes where the organisation is the third label.
const TWO_PART_SUFFIXES: &[&str] = &["co.uk", "org.uk", "ac.uk", "me.uk", "ltd.uk", "plc.uk", "com.au", "co.nz", "co.jp", "com.br"];

/// The organisation's domain: `booking@paris.groupe.fr` → `groupe.fr`.
pub fn org_domain(addr: &str) -> Option<String> {
    let d = super::dns::domain_of(addr)?;
    let labels: Vec<&str> = d.split('.').collect();
    let n = labels.len();
    if n <= 2 {
        return Some(d);
    }
    let last2 = format!("{}.{}", labels[n - 2], labels[n - 1]);
    let keep = if TWO_PART_SUFFIXES.contains(&last2.as_str()) { 3 } else { 2 };
    Some(labels[n.saturating_sub(keep)..].join("."))
}

pub fn is_freemail(addr: &str) -> bool {
    super::dns::domain_of(addr).is_some_and(|d| FREEMAIL.contains(&d.as_str()))
}

// ---------------- Decision ----------------

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Usage {
    pub day: u32,
    pub hour: u32,
    /// Oldest counted send in the rolling hour (when it frees a slot).
    pub oldest_in_hour: Option<DateTime<Utc>>,
    /// Counted sends today to the recipient's organisation domain.
    pub domain_today: u32,
    /// Earliest next automated send (randomized spacing).
    pub next_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    Allow,
    /// Try again at `until` (hourly cap, spacing).
    Wait { until: DateTime<Utc>, code: &'static str, reason: String },
    /// Nothing more today.
    Defer { code: &'static str, reason: String },
    /// This recipient's domain had its share today; others may go.
    SkipDomain { reason: String },
}

/// The pure gate. `spacing`: automated paths wait for the random gap; a
/// one-off manual send does not.
pub fn decide(cfg: &BudgetConfig, daily_limit: u32, u: &Usage, now: DateTime<Utc>, spacing: bool, domain: Option<&str>) -> Decision {
    if u.day >= daily_limit {
        return Decision::Defer {
            code: "budget_day",
            reason: format!("Budget d'envoi du jour atteint ({}/{}, montée en charge). Reprise demain.", u.day, daily_limit),
        };
    }
    if u.hour >= cfg.hourly_cap {
        let until = u.oldest_in_hour.map(|t| t + Duration::hours(1)).unwrap_or(now + Duration::hours(1));
        return Decision::Wait {
            until,
            code: "budget_hour",
            reason: format!("Plafond horaire atteint ({}/h). Prochain créneau à {}.", cfg.hourly_cap, until.with_timezone(&Local).format("%H:%M")),
        };
    }
    if let Some(d) = domain {
        if u.domain_today >= cfg.per_domain_daily {
            return Decision::SkipDomain {
                reason: format!("{} a déjà reçu {} e-mail(s) aujourd'hui (maximum {} par domaine et par jour).", d, u.domain_today, cfg.per_domain_daily),
            };
        }
    }
    if spacing {
        if let Some(t) = u.next_at.filter(|t| *t > now) {
            return Decision::Wait {
                until: t,
                code: "budget_spacing",
                reason: format!("Espacement aléatoire entre deux envois : prochain à {}.", t.with_timezone(&Local).format("%H:%M:%S")),
            };
        }
    }
    Decision::Allow
}

// ---------------- DB side ----------------

pub fn load_config(conn: &Connection) -> BudgetConfig {
    get(conn, CONFIG_KEY)
        .and_then(|j| serde_json::from_str::<BudgetConfig>(&j).ok())
        .unwrap_or_default()
        .sanitized()
}

pub fn save_config(conn: &Connection, cfg: BudgetConfig) -> Result<BudgetConfig, String> {
    let cfg = cfg.sanitized();
    put(conn, CONFIG_KEY, &serde_json::to_string(&cfg).map_err(|e| e.to_string())?);
    Ok(cfg)
}

/// "Counted" sends: sent, and not to someone who had already replied.
const COUNTED: &str = "e.status = 'sent' AND NOT EXISTS (
    SELECT 1 FROM email_replies r WHERE r.kind = 'reply'
      AND lower(trim(r.from_addr)) = lower(trim(e.to_addr))
      AND COALESCE(r.received_at, r.created_at) <= e.sent_at)";

/// A send to this address is a conversation (they wrote back before).
pub fn is_conversation(conn: &Connection, addr: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM email_replies WHERE kind = 'reply' AND lower(trim(from_addr)) = ?1)",
        params![crate::commands::outbox::norm_addr(addr)],
        |r| r.get(0),
    )
    .unwrap_or(false)
}

/// Local midnight of `now`'s day, in UTC.
pub fn day_start(now: DateTime<Utc>) -> DateTime<Utc> {
    let d = now.with_timezone(&Local).date_naive();
    Local
        .from_local_datetime(&d.and_hms_opt(0, 0, 0).expect("valid"))
        .earliest()
        .map(|t| t.with_timezone(&Utc))
        .unwrap_or(now - Duration::hours(24))
}

/// First day of the ramp.
pub fn warmup_start(conn: &Connection, cfg: &BudgetConfig, now: DateTime<Utc>) -> NaiveDate {
    if let Some(d) = cfg.warmup_start.as_deref().and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()) {
        return d;
    }
    conn.query_row("SELECT MIN(sent_at) FROM emails WHERE status = 'sent'", [], |r| r.get::<_, Option<String>>(0))
        .ok()
        .flatten()
        .and_then(|s| parse_utc(&s))
        .map(|t| t.with_timezone(&Local).date_naive())
        .unwrap_or_else(|| now.with_timezone(&Local).date_naive())
}

/// Today's limit: the ramp, never above the outbox daily cap.
pub fn daily_limit(conn: &Connection, cfg: &BudgetConfig, now: DateTime<Utc>) -> u32 {
    let days = (now.with_timezone(&Local).date_naive() - warmup_start(conn, cfg, now)).num_days();
    let outbox_cap = crate::commands::outbox::daily_cap(conn).max(1) as u32;
    ramp_limit(cfg, days).min(outbox_cap)
}

/// Counted sends between two instants: their addresses and times.
fn counted_between(conn: &Connection, from: DateTime<Utc>, to: DateTime<Utc>) -> Vec<(String, DateTime<Utc>)> {
    let sql = format!(
        "SELECT e.to_addr, e.sent_at FROM emails e WHERE {} AND e.sent_at >= ?1 AND e.sent_at <= ?2 ORDER BY e.sent_at",
        COUNTED
    );
    let Ok(mut st) = conn.prepare(&sql) else { return Vec::new() };
    st.query_map(params![fmt(from), fmt(to)], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map(|it| it.flatten().filter_map(|(a, t)| parse_utc(&t).map(|t| (a, t))).collect())
        .unwrap_or_default()
}

pub fn usage(conn: &Connection, addr: Option<&str>, now: DateTime<Utc>) -> Usage {
    let today = counted_between(conn, day_start(now), now);
    let hour_from = now - Duration::hours(1);
    let in_hour: Vec<&DateTime<Utc>> = today.iter().map(|(_, t)| t).filter(|t| **t > hour_from).collect();
    // The rolling hour can straddle midnight.
    let (hour, oldest) = if day_start(now) > hour_from {
        let h = counted_between(conn, hour_from, now);
        (h.len() as u32, h.first().map(|(_, t)| *t))
    } else {
        (in_hour.len() as u32, in_hour.first().map(|t| **t))
    };
    let domain_today = match addr.and_then(org_domain) {
        Some(d) => today.iter().filter(|(a, _)| org_domain(a).as_deref() == Some(d.as_str())).count() as u32,
        None => 0,
    };
    Usage {
        day: today.len() as u32,
        hour,
        oldest_in_hour: oldest,
        domain_today,
        next_at: get(conn, NEXT_AT_KEY).and_then(|s| parse_utc(&s)),
    }
}

/// The gate for one recipient. Conversations always pass; freemail skips the
/// domain cap.
pub fn check(conn: &Connection, addr: &str, now: DateTime<Utc>, spacing: bool) -> Decision {
    if is_conversation(conn, addr) {
        return Decision::Allow;
    }
    let cfg = load_config(conn);
    let u = usage(conn, Some(addr), now);
    let dom = (!is_freemail(addr)).then(|| org_domain(addr)).flatten();
    decide(&cfg, daily_limit(conn, &cfg, now), &u, now, spacing, dom.as_deref())
}

/// The gate without a recipient (may anything automated go out now?).
pub fn check_global(conn: &Connection, now: DateTime<Utc>) -> Decision {
    let cfg = load_config(conn);
    let u = usage(conn, None, now);
    decide(&cfg, daily_limit(conn, &cfg, now), &u, now, true, None)
}

/// After a send went out: tag it with the sending address and, unless it was
/// a conversation, schedule the next automated send after a random gap.
pub fn record_send(conn: &Connection, email_id: Option<i64>, from_email: &str, to: &str, sent: bool, now: DateTime<Utc>) {
    if let Some(id) = email_id {
        let _ = conn.execute(
            "UPDATE emails SET from_addr = ?2 WHERE id = ?1",
            params![id, crate::commands::outbox::norm_addr(from_email)],
        );
    }
    if sent && !is_conversation(conn, to) {
        let cfg = load_config(conn);
        put(conn, NEXT_AT_KEY, &fmt(now + Duration::seconds(gap_secs(&cfg, random_seed()))));
    }
}

#[derive(Serialize)]
pub struct BudgetStatus {
    pub config: BudgetConfig,
    pub warmup_start: String,
    /// Days since the ramp started (0 = first day).
    pub warmup_day: i64,
    pub daily_limit: u32,
    pub sent_today: u32,
    pub sent_hour: u32,
    pub next_at: Option<String>,
    /// Weekly limits, from the first week to the ceiling.
    pub ramp: Vec<u32>,
    /// Domains that had their share today.
    pub capped_domains: Vec<String>,
}

pub fn status(conn: &Connection, now: DateTime<Utc>) -> BudgetStatus {
    let cfg = load_config(conn);
    let start = warmup_start(conn, &cfg, now);
    let u = usage(conn, None, now);
    let mut counts: std::collections::BTreeMap<String, u32> = Default::default();
    for (a, _) in counted_between(conn, day_start(now), now) {
        if !is_freemail(&a) {
            if let Some(d) = org_domain(&a) {
                *counts.entry(d).or_default() += 1;
            }
        }
    }
    BudgetStatus {
        warmup_start: start.format("%Y-%m-%d").to_string(),
        warmup_day: (now.with_timezone(&Local).date_naive() - start).num_days().max(0),
        daily_limit: daily_limit(conn, &cfg, now),
        sent_today: u.day,
        sent_hour: u.hour,
        next_at: u.next_at.filter(|t| *t > now).map(fmt),
        ramp: ramp_schedule(&cfg, 30),
        capped_domains: counts.into_iter().filter(|(_, n)| *n >= cfg.per_domain_daily).map(|(d, _)| d).collect(),
        config: cfg,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        crate::commands::residency::testdb::conn()
    }
    fn send(c: &Connection, to: &str, at: DateTime<Utc>) {
        c.execute(
            "INSERT INTO emails (to_addr, subject, body, status, sent_at) VALUES (?1, 's', 'b', 'sent', ?2)",
            params![to, fmt(at)],
        )
        .unwrap();
    }

    #[test]
    fn ramp_math() {
        let cfg = BudgetConfig::default();
        assert_eq!(ramp_limit(&cfg, 0), 15);
        assert_eq!(ramp_limit(&cfg, 6), 15);
        assert_eq!(ramp_limit(&cfg, 7), 18);
        assert_eq!(ramp_limit(&cfg, 14), 22); // 21.6
        assert_eq!(ramp_limit(&cfg, 21), 26); // 25.9
        assert_eq!(ramp_limit(&cfg, 3650), 150);
        assert_eq!(ramp_limit(&cfg, -3), 15);
        let s = ramp_schedule(&cfg, 30);
        assert_eq!(&s[..4], &[15, 18, 22, 26]);
        assert_eq!(*s.last().unwrap(), 150);
        assert!(s.windows(2).all(|w| w[0] <= w[1]));
        let flat = BudgetConfig { weekly_growth_pct: 0, ..cfg.clone() };
        assert_eq!(ramp_limit(&flat, 700), 15);
        // Sanitizing: nothing can switch a cap off.
        let s = BudgetConfig { start_per_day: 0, ceiling: 0, hourly_cap: 0, spacing_min_s: 500, spacing_max_s: 100, ..cfg }.sanitized();
        assert_eq!((s.start_per_day, s.ceiling, s.hourly_cap), (1, 1, 1));
        assert!(s.spacing_min_s <= s.spacing_max_s);
    }

    #[test]
    fn spacing_is_random_within_bounds() {
        let cfg = BudgetConfig::default();
        let gaps: Vec<i64> = (0..500).map(|s| gap_secs(&cfg, s)).collect();
        assert!(gaps.iter().all(|g| (60..=180).contains(g)));
        assert!((0..50).map(|_| bulk_gap_secs(&cfg)).all(|g| (20..=60).contains(&g)));
        let distinct: std::collections::HashSet<_> = gaps.iter().collect();
        assert!(distinct.len() > 50);
        let fixed = BudgetConfig { spacing_min_s: 0, spacing_max_s: 0, ..cfg };
        assert_eq!(gap_secs(&fixed, 42), 0);
    }

    #[test]
    fn domains() {
        assert_eq!(org_domain("a@paris.groupe-x.fr").as_deref(), Some("groupe-x.fr"));
        assert_eq!(org_domain("a@fabric.co.uk").as_deref(), Some("fabric.co.uk"));
        assert_eq!(org_domain("a@book.fabric.co.uk").as_deref(), Some("fabric.co.uk"));
        assert_eq!(org_domain("a@club.ch").as_deref(), Some("club.ch"));
        assert!(is_freemail("Someone@GMAIL.com") && !is_freemail("x@club.be"));
    }

    #[test]
    fn decision_order() {
        let cfg = BudgetConfig::default();
        let now = Utc::now();
        let u = Usage::default();
        assert_eq!(decide(&cfg, 15, &u, now, true, Some("x.fr")), Decision::Allow);
        let full = Usage { day: 15, ..u.clone() };
        assert!(matches!(decide(&cfg, 15, &full, now, true, None), Decision::Defer { code: "budget_day", .. }));
        let t0 = now - Duration::minutes(20);
        let hour = Usage { hour: 12, oldest_in_hour: Some(t0), ..u.clone() };
        assert!(matches!(decide(&cfg, 15, &hour, now, false, None), Decision::Wait { until, code: "budget_hour", .. } if until == t0 + Duration::hours(1)));
        let dom = Usage { domain_today: 2, ..u.clone() };
        assert!(matches!(decide(&cfg, 15, &dom, now, false, Some("x.fr")), Decision::SkipDomain { .. }));
        assert_eq!(decide(&cfg, 15, &dom, now, false, None), Decision::Allow);
        let spaced = Usage { next_at: Some(now + Duration::seconds(30)), ..u.clone() };
        assert!(matches!(decide(&cfg, 15, &spaced, now, true, None), Decision::Wait { code: "budget_spacing", .. }));
        // A manual one-off send does not wait for the spacing.
        assert_eq!(decide(&cfg, 15, &spaced, now, false, None), Decision::Allow);
        let past = Usage { next_at: Some(now - Duration::seconds(1)), ..u };
        assert_eq!(decide(&cfg, 15, &past, now, true, None), Decision::Allow);
    }

    /// Today 12:00 local: every "minutes ago" in the tests stays today.
    fn noon() -> DateTime<Utc> {
        let d = Local::now().date_naive().and_hms_opt(12, 0, 0).unwrap();
        Local.from_local_datetime(&d).earliest().unwrap().with_timezone(&Utc)
    }

    #[test]
    fn shared_budget_on_db() {
        let c = conn();
        let now = noon();
        save_config(&c, BudgetConfig { start_per_day: 5, hourly_cap: 3, per_domain_daily: 2, ..Default::default() }).unwrap();
        // Ramp starts at the first email ever sent.
        assert_eq!(daily_limit(&c, &load_config(&c), now), 5);
        send(&c, "a@groupe.fr", now - Duration::minutes(10));
        send(&c, "b@paris.groupe.fr", now - Duration::minutes(5));
        // Two counted today, both on groupe.fr.
        let u = usage(&c, Some("c@groupe.fr"), now);
        assert_eq!((u.day, u.hour, u.domain_today), (2, 2, 2));
        assert!(matches!(check(&c, "c@groupe.fr", now, false), Decision::SkipDomain { .. }));
        assert_eq!(check(&c, "c@other.fr", now, false), Decision::Allow);
        // Hourly cap: a third send in the hour.
        send(&c, "d@other.fr", now - Duration::minutes(1));
        assert!(matches!(check(&c, "e@third.fr", now, false), Decision::Wait { code: "budget_hour", .. }));
        // A reply from groupe.fr earlier: sends to that address are a conversation,
        // not counted and never held back.
        c.execute(
            "INSERT INTO email_replies (from_addr, message_id, kind, received_at) VALUES ('c@groupe.fr', '<r>', 'reply', ?1)",
            params![fmt(now - Duration::hours(2))],
        )
        .unwrap();
        assert_eq!(check(&c, "c@groupe.fr", now, true), Decision::Allow);
        send(&c, "c@groupe.fr", now);
        assert_eq!(usage(&c, None, now).day, 3);
        // Yesterday does not count today.
        send(&c, "z@old.fr", day_start(now) - Duration::minutes(1));
        assert_eq!(usage(&c, None, now).day, 3);
        // Spacing: recorded after a counted send.
        record_send(&c, None, "booking@insrt.fr", "f@new.fr", true, now);
        let next = parse_utc(&get(&c, NEXT_AT_KEY).unwrap()).unwrap();
        assert!(next >= now + Duration::seconds(59) && next <= now + Duration::seconds(181));
        // The outbox cap still bounds the ramp.
        put(&c, "daily_send_cap", "2");
        assert_eq!(daily_limit(&c, &load_config(&c), now), 2);
        let st = status(&c, now);
        assert_eq!(st.capped_domains, vec!["groupe.fr".to_string()]);
        assert_eq!(st.sent_today, 3);
    }
}
