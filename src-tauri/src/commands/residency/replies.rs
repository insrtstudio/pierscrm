//! Reading the answers to a residency campaign.
//!
//! Rules first, Claude only when rules cannot tell: an unsubscribe request or
//! an automatic out-of-office (headers / subject / wording) never costs a
//! call. Human replies are classified by claude-haiku-5-5 in batches, with a
//! JSON schema, one retry on a bad answer. What each answer triggers:
//! - interested / question -> in discussion + Action "Répondre à X" (suggested reply)
//! - later                 -> follow-up date + Action on that date
//! - not_interested        -> declined
//! - unsubscribe           -> suppression + declined
//! - wrong_person          -> Action "Trouver le bon contact" (address mentioned, if any)
//! - out_of_office         -> not a reply: the next step moves after the return date (else +7 days)
//! The agent never answers a human itself.

use chrono::{Datelike, Duration, NaiveDate, TimeZone, Utc};
use rusqlite::{params, Connection};
use serde::Serialize;
use serde_json::{json, Value};

use super::claude::{self, Llm, LlmRequest, SystemBlock, DEFAULT_MODEL};
use super::engine::{fmt, skip_remaining};
use crate::commands::{outbox, status};

pub const INTENTS: &[&str] =
    &["interested", "later", "not_interested", "wrong_person", "out_of_office", "question", "unsubscribe"];
pub const SENTIMENTS: &[&str] = &["positive", "neutral", "negative"];
/// Replies per Claude call.
pub const BATCH: usize = 8;

#[derive(Debug, Clone)]
pub struct Pending {
    pub reply_id: i64,
    pub contact_id: i64,
    pub campaign_id: i64,
    pub kind: String,
    pub from: String,
    pub subject: String,
    pub text: String,
    pub email_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Insight {
    pub reply_id: i64,
    pub sentiment: String,
    pub intent: String,
    pub summary: String,
    pub suggested_reply: String,
    pub follow_up_on: Option<String>,
    pub mentioned_email: Option<String>,
    pub return_date: Option<String>,
    pub model: String,
}

/// Replies to residency campaign emails not read yet: matched to a contact
/// that received a residency message, received after that message.
pub fn pending(conn: &Connection) -> Vec<Pending> {
    let Ok(mut st) = conn.prepare(
        "SELECT r.id, k.id, m.campaign_id, r.kind, r.from_addr, COALESCE(r.subject,''), COALESCE(r.body_excerpt,''),
                COALESCE(r.received_at, r.created_at), r.email_id
         FROM email_replies r
         JOIN contacts k ON k.id = r.contact_id
              OR (r.contact_id IS NULL AND k.email IS NOT NULL AND lower(trim(k.email)) = lower(trim(r.from_addr)))
         JOIN campaign_messages m ON m.contact_id = k.id AND m.status = 'sent'
         JOIN emails e ON e.id = m.sent_email_id
         JOIN campaigns c ON c.id = m.campaign_id AND c.kind = 'residency'
         WHERE r.kind IN ('reply','auto_reply','unsubscribe')
           AND NOT EXISTS (SELECT 1 FROM reply_insights i WHERE i.email_reply_id = r.id)
           AND COALESCE(r.received_at, r.created_at) >= e.sent_at
         GROUP BY r.id
         ORDER BY r.id",
    ) else {
        return Vec::new();
    };
    st.query_map([], |r| {
        Ok(Pending {
            reply_id: r.get(0)?,
            contact_id: r.get(1)?,
            campaign_id: r.get(2)?,
            kind: r.get(3)?,
            from: r.get(4)?,
            subject: r.get(5)?,
            text: r.get(6)?,
            email_id: r.get(8)?,
        })
    })
    .map(|it| it.flatten().collect())
    .unwrap_or_default()
}

// ---------------- Rules ----------------

const OOO_WORDS: &[&str] = &[
    "out of office", "out of the office", "automatic reply", "auto-reply", "autoreply", "on holiday", "on vacation",
    "currently away", "away from the office", "réponse automatique", "absent du bureau", "je suis absent",
    "je suis actuellement absent", "en congés", "en congé", "de retour le", "back on", "i will be back",
    "i'll be back", "limited access to email", "accès limité à mes emails",
];
const UNSUB_WORDS: &[&str] = &[
    "unsubscribe", "désinscri", "desinscri", "désabonn", "desabonn", "ne plus recevoir", "retirez-moi",
    "retirer de votre liste", "remove me", "stop emailing", "ne m'écrivez plus",
];

fn month_num(w: &str) -> Option<u32> {
    let w = crate::db::normalise(w);
    let months = [
        ("janvier", 1), ("january", 1), ("jan", 1), ("fevrier", 2), ("february", 2), ("feb", 2), ("fev", 2),
        ("mars", 3), ("march", 3), ("mar", 3), ("avril", 4), ("april", 4), ("apr", 4), ("avr", 4), ("mai", 5),
        ("may", 5), ("juin", 6), ("june", 6), ("jun", 6), ("juillet", 7), ("july", 7), ("jul", 7), ("juil", 7),
        ("aout", 8), ("august", 8), ("aug", 8), ("septembre", 9), ("september", 9), ("sept", 9), ("sep", 9),
        ("octobre", 10), ("october", 10), ("oct", 10), ("novembre", 11), ("november", 11), ("nov", 11),
        ("decembre", 12), ("december", 12), ("dec", 12),
    ];
    months.iter().find(|(n, _)| *n == w).map(|(_, m)| *m)
}

/// The return date of an out-of-office: the latest date mentioned that falls
/// within the next 120 days ("de retour le 12 novembre", "back on 03/11",
/// "until November 12th", "2026-11-12").
pub fn parse_return_date(text: &str, today: NaiveDate) -> Option<NaiveDate> {
    let mut found: Vec<NaiveDate> = Vec::new();
    let year_for = |m: u32, d: u32| -> Option<NaiveDate> {
        let y = today.year();
        let this = NaiveDate::from_ymd_opt(y, m, d)?;
        if this >= today {
            Some(this)
        } else {
            NaiveDate::from_ymd_opt(y + 1, m, d)
        }
    };
    let iso = regex::Regex::new(r"\b(20\d\d)-(\d{1,2})-(\d{1,2})\b").expect("re");
    for c in iso.captures_iter(text) {
        if let (Ok(y), Ok(m), Ok(d)) = (c[1].parse(), c[2].parse(), c[3].parse()) {
            if let Some(dt) = NaiveDate::from_ymd_opt(y, m, d) {
                found.push(dt);
            }
        }
    }
    let num = regex::Regex::new(r"\b(\d{1,2})[/.](\d{1,2})(?:[/.](\d{2,4}))?\b").expect("re");
    for c in num.captures_iter(text) {
        let (Ok(d), Ok(m)) = (c[1].parse::<u32>(), c[2].parse::<u32>()) else { continue };
        let dt = match c.get(3).and_then(|y| y.as_str().parse::<i32>().ok()) {
            Some(y) => NaiveDate::from_ymd_opt(if y < 100 { 2000 + y } else { y }, m, d),
            None => year_for(m, d),
        };
        if let Some(dt) = dt {
            found.push(dt);
        }
    }
    let words: Vec<&str> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    for i in 0..words.len() {
        let day = |w: &str| -> Option<u32> {
            let digits: String = w.chars().take_while(|c| c.is_ascii_digit()).collect();
            let rest = &w[digits.len()..];
            (["", "st", "nd", "rd", "th", "er"].contains(&rest.to_lowercase().as_str()))
                .then(|| digits.parse().ok())
                .flatten()
                .filter(|d| (1..=31).contains(d))
        };
        // "12 novembre" / "1er décembre"
        if let (Some(d), Some(m)) = (day(words[i]), words.get(i + 1).and_then(|w| month_num(w))) {
            if let Some(dt) = year_for(m, d) {
                found.push(dt);
            }
        }
        // "November 12"
        if let (Some(m), Some(d)) = (month_num(words[i]), words.get(i + 1).and_then(|w| day(w))) {
            if let Some(dt) = year_for(m, d) {
                found.push(dt);
            }
        }
    }
    found.into_iter().filter(|d| *d >= today && *d <= today + Duration::days(120)).max()
}

fn contains_any(hay: &str, words: &[&str]) -> bool {
    let h = hay.to_lowercase();
    words.iter().any(|w| h.contains(w))
}

/// The answer when rules are enough (no Claude call).
pub fn rule_classify(p: &Pending, today: NaiveDate) -> Option<Insight> {
    let all = format!("{}\n{}", p.subject, p.text);
    let base = |intent: &str, sentiment: &str, summary: String| Insight {
        reply_id: p.reply_id,
        sentiment: sentiment.into(),
        intent: intent.into(),
        summary,
        suggested_reply: String::new(),
        follow_up_on: None,
        mentioned_email: None,
        return_date: None,
        model: "rules".into(),
    };
    if p.kind == "unsubscribe" || (p.text.chars().count() < 400 && contains_any(&all, UNSUB_WORDS)) {
        return Some(base("unsubscribe", "negative", "Demande de désinscription.".into()));
    }
    if p.kind == "auto_reply" || contains_any(&all, OOO_WORDS) {
        let ret = parse_return_date(&all, today);
        let mut i = base(
            "out_of_office",
            "neutral",
            match ret {
                Some(d) => format!("Absent, de retour le {}.", d.format("%d/%m")),
                None => "Réponse automatique d'absence.".into(),
            },
        );
        i.return_date = ret.map(|d| d.to_string());
        return Some(i);
    }
    None
}

// ---------------- Claude ----------------

const CLASSIFY_SYSTEM: &str = "You read replies that music venues sent to a DJ/producer who proposed a monthly residency \
at their club. For each reply, return:
- sentiment: positive, neutral or negative
- intent: interested (wants to talk / open to it), question (asks something before deciding: fee, dates, \
references, technical), later (not now, maybe another season or a given month), not_interested (declines), \
wrong_person (not the right contact, possibly giving another address), out_of_office (automatic absence), \
unsubscribe (asks to stop receiving emails)
- summary: one short line in French describing what they said (no em dash)
- suggested_reply: for interested or question, a short, warm draft answer in the language of the reply, \
signed with the artist name, that moves toward a call or a date; empty string otherwise. Never invent facts \
(fees, dates) that are not in the reply.
- follow_up_on: for later, the date (YYYY-MM-DD) to come back to them, derived from what they wrote and today's \
date; empty string otherwise
- mentioned_email: an email address the reply points to (e.g. the right person), else empty string
- return_date: for out_of_office, the return date YYYY-MM-DD if stated, else empty string
Answer with JSON only.";

fn classify_schema() -> Value {
    let s = |d: &str| json!({"type": "string", "description": d});
    json!({
        "type": "object",
        "properties": {
            "items": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "reply_id": {"type": "integer"},
                        "sentiment": {"type": "string", "enum": SENTIMENTS},
                        "intent": {"type": "string", "enum": INTENTS},
                        "summary": s("one line, French"),
                        "suggested_reply": s("draft answer or empty"),
                        "follow_up_on": s("YYYY-MM-DD or empty"),
                        "mentioned_email": s("address or empty"),
                        "return_date": s("YYYY-MM-DD or empty")
                    },
                    "required": ["reply_id", "sentiment", "intent", "summary", "suggested_reply", "follow_up_on", "mentioned_email", "return_date"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["items"],
        "additionalProperties": false
    })
}

fn opt_date(v: Option<&Value>) -> Option<String> {
    let s = v?.as_str()?.trim();
    NaiveDate::parse_from_str(s.get(..10).unwrap_or(s), "%Y-%m-%d").ok().map(|d| d.to_string())
}

/// Robust parsing of a batch answer: unknown ids, bad enums and missing
/// fields are dropped or defaulted instead of failing the batch.
pub fn parse_batch(text: &str, ids: &[i64], model: &str) -> Option<Vec<Insight>> {
    let v = claude::extract_json(text)?;
    let items = v.get("items").and_then(Value::as_array).cloned().or_else(|| v.as_array().cloned())?;
    let mut out = Vec::new();
    for it in items {
        let Some(id) = it.get("reply_id").and_then(Value::as_i64) else { continue };
        if !ids.contains(&id) || out.iter().any(|i: &Insight| i.reply_id == id) {
            continue;
        }
        let intent = it.get("intent").and_then(Value::as_str).unwrap_or("").trim().to_lowercase();
        if !INTENTS.contains(&intent.as_str()) {
            continue;
        }
        let mut sentiment = it.get("sentiment").and_then(Value::as_str).unwrap_or("neutral").trim().to_lowercase();
        if !SENTIMENTS.contains(&sentiment.as_str()) {
            sentiment = "neutral".into();
        }
        let txt = |k: &str| claude::no_dashes(it.get(k).and_then(Value::as_str).unwrap_or("").trim());
        let mentioned = txt("mentioned_email");
        out.push(Insight {
            reply_id: id,
            sentiment,
            intent,
            summary: txt("summary"),
            suggested_reply: txt("suggested_reply"),
            follow_up_on: opt_date(it.get("follow_up_on")),
            mentioned_email: outbox::addresses_in(&mentioned).into_iter().next(),
            return_date: opt_date(it.get("return_date")),
            model: model.to_string(),
        });
    }
    Some(out)
}

fn classify_batch(conn: &Connection, llm: &dyn Llm, batch: &[Pending], today: NaiveDate, artist: &str) -> Result<Vec<Insight>, String> {
    let ids: Vec<i64> = batch.iter().map(|p| p.reply_id).collect();
    let mut user = format!("Today: {}\nArtist: {}\n", today, artist);
    for p in batch {
        let venue: String = conn
            .query_row("SELECT COALESCE(NULLIF(venue,''), name) FROM contacts WHERE id = ?1", params![p.contact_id], |r| r.get(0))
            .unwrap_or_default();
        let text: String = p.text.chars().take(2000).collect();
        user.push_str(&format!(
            "\n--- reply_id {} ---\nVenue: {}\nFrom: {}\nSubject: {}\n{}\n",
            p.reply_id,
            venue,
            p.from,
            p.subject,
            if text.trim().is_empty() { "(no text, subject only)".into() } else { text }
        ));
    }
    let req = LlmRequest {
        model: DEFAULT_MODEL.into(),
        // ~200 per reply, structured JSON.
        max_tokens: (200 * batch.len() as u32).max(200) + 100,
        system: vec![SystemBlock { text: CLASSIFY_SYSTEM.into(), cache: true }],
        user,
        schema: Some(classify_schema()),
    };
    let mut last = String::new();
    for _ in 0..2 {
        let resp = llm.complete(&req)?;
        let model = if resp.model.is_empty() { DEFAULT_MODEL.to_string() } else { resp.model.clone() };
        claude::log_usage(conn, "classify", Some(batch[0].campaign_id), &model, &resp.usage);
        match parse_batch(&resp.text, &ids, &model) {
            Some(v) if !v.is_empty() => return Ok(v),
            _ => last = "classification illisible".into(),
        }
    }
    Err(last)
}

// ---------------- Applying an insight ----------------

fn store(conn: &Connection, p: &Pending, i: &Insight) -> bool {
    conn.execute(
        "INSERT OR IGNORE INTO reply_insights
            (email_reply_id, contact_id, campaign_id, sentiment, intent, summary, suggested_reply, follow_up_on, mentioned_email, model)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            i.reply_id,
            p.contact_id,
            p.campaign_id,
            i.sentiment,
            i.intent,
            i.summary,
            (!i.suggested_reply.is_empty()).then_some(&i.suggested_reply),
            i.follow_up_on.as_ref().or(i.return_date.as_ref()),
            i.mentioned_email,
            i.model
        ],
    )
    .unwrap_or(0)
        > 0
}

/// An out-of-office that came in as a "reply" (no auto headers) was counted
/// as one by the sync: undo that, it is not an answer.
fn unreply(conn: &Connection, p: &Pending) {
    if let Some(eid) = p.email_id {
        let _ = conn.execute(
            "UPDATE emails SET replied_at = NULL WHERE id = ?1
               AND NOT EXISTS (SELECT 1 FROM email_replies r WHERE r.email_id = ?1 AND r.kind = 'reply' AND r.id <> ?2
                               AND NOT EXISTS (SELECT 1 FROM reply_insights i WHERE i.email_reply_id = r.id AND i.intent = 'out_of_office'))",
            params![eid, p.reply_id],
        );
    }
    let _ = conn.execute(
        "UPDATE campaign_contacts SET state = 'sent', updated_at = datetime('now')
         WHERE campaign_id = ?1 AND contact_id = ?2 AND state = 'replied'",
        params![p.campaign_id, p.contact_id],
    );
    status::on_reply_withdrawn(conn, p.contact_id);
}

/// Move the next pending step after an absence (return date + 1 day, else +7 days).
fn reschedule_after(conn: &Connection, p: &Pending, ret: Option<NaiveDate>, today: NaiveDate) {
    let target = ret.map(|d| d + Duration::days(1)).unwrap_or(today + Duration::days(7));
    let when = fmt(Utc.from_utc_datetime(&target.and_hms_opt(8, 0, 0).expect("valid")));
    let n = conn
        .execute(
            "UPDATE campaign_messages SET scheduled_for = CASE WHEN scheduled_for IS NULL OR scheduled_for < ?3 THEN ?3 ELSE scheduled_for END,
                    note = 'Reporté après absence', updated_at = datetime('now')
             WHERE id = (SELECT id FROM campaign_messages WHERE campaign_id = ?1 AND contact_id = ?2
                         AND status IN ('scheduled','draft') ORDER BY step LIMIT 1)",
            params![p.campaign_id, p.contact_id, when],
        )
        .unwrap_or(0);
    if n > 0 {
        super::log(conn, Some(p.campaign_id), "info", "rescheduled", &format!("Absence de {} : relance reportée au {}", p.from, target));
    }
}

pub fn apply(conn: &Connection, p: &Pending, i: &Insight, today: NaiveDate) {
    if !store(conn, p, i) {
        return;
    }
    let addr = p.from.trim().to_lowercase();
    match i.intent.as_str() {
        "out_of_office" => {
            if p.kind == "reply" {
                unreply(conn, p);
            }
            let ret = i.return_date.as_deref().and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok());
            reschedule_after(conn, p, ret, today);
            return;
        }
        "interested" | "question" => {
            status::on_reply(conn, Some(p.contact_id), &addr);
            skip_remaining(conn, p.campaign_id, p.contact_id, "réponse reçue");
        }
        "later" => {
            status::on_reply(conn, Some(p.contact_id), &addr);
            let when = i.follow_up_on.clone().unwrap_or_else(|| (today + Duration::days(30)).to_string());
            status::follow_up_on(conn, p.contact_id, &when);
            let _ = conn.execute(
                "UPDATE reply_insights SET follow_up_on = ?2 WHERE email_reply_id = ?1",
                params![i.reply_id, when],
            );
            skip_remaining(conn, p.campaign_id, p.contact_id, "plus tard");
        }
        "not_interested" => {
            status::on_refused(conn, Some(p.contact_id), &addr);
            skip_remaining(conn, p.campaign_id, p.contact_id, "refus");
        }
        "unsubscribe" => {
            outbox::suppress(conn, &addr, "unsubscribe", Some("résidence"));
            status::on_refused(conn, Some(p.contact_id), &addr);
            skip_remaining(conn, p.campaign_id, p.contact_id, "désinscription");
        }
        "wrong_person" => {
            skip_remaining(conn, p.campaign_id, p.contact_id, "mauvais contact");
        }
        _ => {}
    }
    super::log(
        conn,
        Some(p.campaign_id),
        "info",
        &format!("reply_{}", i.intent),
        &format!("{} : {} ({})", p.from, i.summary, i.model),
    );
}

/// Classify every pending reply. Without an API key the human replies stay
/// unclassified (their sequence is still stopped by the engine's stop rule)
/// and show in Actions as "à lire".
pub fn classify_pending(conn: &Connection, llm: Option<&dyn Llm>, today: NaiveDate) -> usize {
    let mut done = 0;
    let mut for_llm: Vec<Pending> = Vec::new();
    for p in pending(conn) {
        match rule_classify(&p, today) {
            Some(i) => {
                apply(conn, &p, &i, today);
                done += 1;
            }
            None => for_llm.push(p),
        }
    }
    let Some(llm) = llm else { return done };
    for batch in for_llm.chunks(BATCH) {
        let artist = super::content::artist_name(conn, batch[0].campaign_id);
        match classify_batch(conn, llm, batch, today, &artist) {
            Ok(insights) => {
                for i in insights {
                    if let Some(p) = batch.iter().find(|p| p.reply_id == i.reply_id) {
                        apply(conn, p, &i, today);
                        done += 1;
                    }
                }
            }
            Err(e) => super::log(conn, Some(batch[0].campaign_id), "warn", "classify_failed", &e),
        }
    }
    done
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::residency::claude::fake::FakeLlm;
    use crate::commands::residency::engine::tests::setup;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn return_dates() {
        let today = d("2026-10-29");
        assert_eq!(parse_return_date("Je suis absent, de retour le 12 novembre.", today), Some(d("2026-11-12")));
        assert_eq!(parse_return_date("Back on 03/11, limited access", today), Some(d("2026-11-03")));
        assert_eq!(parse_return_date("Out of office until November 9th", today), Some(d("2026-11-09")));
        assert_eq!(parse_return_date("away 2026-11-20", today), Some(d("2026-11-20")));
        assert_eq!(parse_return_date("de retour le 1er janvier", today), Some(d("2027-01-01")));
        assert_eq!(parse_return_date("absent", today), None);
    }

    #[test]
    fn parse_batch_is_robust() {
        let ids = [1, 2, 3];
        let txt = r#"```json
        {"items":[
          {"reply_id":1,"sentiment":"positive","intent":"interested","summary":"Partant — pour un call","suggested_reply":"Merci !","follow_up_on":"","mentioned_email":"","return_date":""},
          {"reply_id":2,"sentiment":"weird","intent":"later","summary":"Pas avant mars","suggested_reply":"","follow_up_on":"2027-03-01","mentioned_email":"","return_date":""},
          {"reply_id":9,"sentiment":"neutral","intent":"question","summary":"x"},
          {"reply_id":3,"sentiment":"neutral","intent":"bogus"},
          {"reply_id":3,"sentiment":"neutral","intent":"wrong_person","summary":"Écrire à Léa","mentioned_email":"Lea <lea@club.fr>"}
        ]}
        ```"#;
        let v = parse_batch(txt, &ids, "claude-haiku-5-5").unwrap();
        assert_eq!(v.len(), 3);
        assert_eq!(v[0].summary, "Partant, pour un call");
        assert_eq!(v[1].sentiment, "neutral");
        assert_eq!(v[1].follow_up_on.as_deref(), Some("2027-03-01"));
        assert_eq!(v[2].mentioned_email.as_deref(), Some("lea@club.fr"));
        assert!(parse_batch("nope", &ids, "m").is_none());
    }

    fn sent_step1(c: &Connection, cid: i64) {
        c.execute(
            "INSERT INTO emails (contact_id, campaign_id, to_addr, subject, body, status, sent_at) VALUES (?1, 1, 'x', 's', 'b', 'sent', '2026-10-20 10:00:00')",
            params![cid],
        )
        .unwrap();
        let eid = c.last_insert_rowid();
        c.execute(
            "UPDATE campaign_messages SET status = 'sent', sent_email_id = ?2 WHERE contact_id = ?1 AND step = 1",
            params![cid, eid],
        )
        .unwrap();
        c.execute(
            "INSERT INTO campaign_messages (campaign_id, contact_id, step, subject, body, status, scheduled_for)
             VALUES (1, ?1, 2, 's2', 'b2', 'scheduled', '2026-10-25 10:00:00')",
            params![cid],
        )
        .unwrap();
    }

    #[test]
    fn classification_applies_effects() {
        let c = setup();
        sent_step1(&c, 10);
        sent_step1(&c, 11);
        c.execute_batch(
            "INSERT INTO email_replies (id, email_id, contact_id, from_addr, subject, message_id, kind, received_at, body_excerpt) VALUES
               (1, 1, 10, 'booking@cluba.fr', 'Re: Résidence', '<a>', 'reply', '2026-10-21 09:00:00', 'Avec plaisir, appelons-nous jeudi ?'),
               (2, 2, 11, 'booking@clubb.co.uk', 'Automatic reply', '<b>', 'auto_reply', '2026-10-21 09:00:00', 'I am away until November 9th.');
             UPDATE contacts SET status = 'in_discussion' WHERE id = 10;",
        )
        .unwrap();
        let llm = FakeLlm::new(&[r#"{"items":[{"reply_id":1,"sentiment":"positive","intent":"interested","summary":"Partant pour un appel jeudi","suggested_reply":"Merci, jeudi 15h ?\n\nPiers","follow_up_on":"","mentioned_email":"","return_date":""}]}"#]);
        let n = classify_pending(&c, Some(&llm), d("2026-10-29"));
        assert_eq!(n, 2);
        // One Claude call only (the auto-reply was handled by rules), on Haiku.
        let seen = llm.seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].model, "claude-haiku-5-5");
        assert!(seen[0].user.contains("reply_id 1") && !seen[0].user.contains("reply_id 2"));
        drop(seen);
        let (intent, sugg): (String, String) = c
            .query_row("SELECT intent, suggested_reply FROM reply_insights WHERE email_reply_id = 1", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!(intent, "interested");
        assert!(sugg.contains("jeudi"));
        let st: String = c.query_row("SELECT status FROM campaign_messages WHERE contact_id = 10 AND step = 2", [], |r| r.get(0)).unwrap();
        assert_eq!(st, "skipped");
        // Out-of-office: step 2 moved to the day after the return, still scheduled.
        let (st, when): (String, String) = c
            .query_row("SELECT status, scheduled_for FROM campaign_messages WHERE contact_id = 11 AND step = 2", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!(st, "scheduled");
        assert_eq!(when, "2026-11-10 08:00:00");
        assert_eq!(crate::commands::residency::engine::stop_reason(&c, 1, 11), None);
        // Nothing left to classify; a second run is a no-op.
        assert_eq!(classify_pending(&c, Some(&llm), d("2026-10-29")), 0);
    }

    #[test]
    fn unsubscribe_and_refusal_by_rules_and_llm() {
        let c = setup();
        sent_step1(&c, 10);
        sent_step1(&c, 11);
        c.execute_batch(
            "INSERT INTO email_replies (id, email_id, contact_id, from_addr, subject, message_id, kind, received_at, body_excerpt) VALUES
               (1, 1, 10, 'booking@cluba.fr', 'Re: Résidence', '<a>', 'reply', '2026-10-21 09:00:00', 'Merci de me retirer de votre liste.'),
               (2, 2, 11, 'booking@clubb.co.uk', 'Re: residency', '<b>', 'reply', '2026-10-21 09:00:00', 'Thanks but we are fully booked with residents.');",
        )
        .unwrap();
        // The model first answers garbage, then a valid batch (retry once).
        let llm = FakeLlm::new(&[
            "sorry",
            r#"{"items":[{"reply_id":2,"sentiment":"negative","intent":"not_interested","summary":"Complet avec leurs résidents","suggested_reply":"","follow_up_on":"","mentioned_email":"","return_date":""}]}"#,
        ]);
        assert_eq!(classify_pending(&c, Some(&llm), d("2026-10-29")), 2);
        assert!(crate::commands::outbox::suppression_reason(&c, "booking@cluba.fr").is_some());
        let s: String = c.query_row("SELECT status FROM contacts WHERE id = 11", [], |r| r.get(0)).unwrap();
        assert_eq!(s, "declined");
        let calls: i64 = c.query_row("SELECT COUNT(*) FROM ai_usage WHERE purpose = 'classify'", [], |r| r.get(0)).unwrap();
        assert_eq!(calls, 2);
    }
}
