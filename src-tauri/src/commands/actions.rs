//! What the Aujourd'hui page asks of its action lines, beyond its own data:
//! - closing a line handled outside the CRM. Thibault answered by phone, from
//!   his mailbox, in person: `contacts.actions_done_at` says everything up to
//!   now is dealt with, and only what comes in after shows again;
//! - the replies still waiting for an answer (`open_replies`), so a line leaves
//!   as soon as it was answered, ignored, or classified as needing nothing;
//! - a ready draft behind "Répondre" and "Relancer" (`action_draft`).

use chrono::{Duration, NaiveDate};
use rusqlite::{params, Connection};
use serde::Serialize;
use tauri::State;

use crate::commands::email::ThreadRef;
use crate::commands::reply_desk::{self, DraftInput, Kind, Message};
use crate::commands::residency::claude::{self, Llm};
use crate::commands::{autonomy, status};
use crate::AppState;

/// Replies older than this are history, not something to answer now.
pub const REPLY_DAYS: i64 = 10;

pub fn migrate(conn: &Connection) {
    let _ = conn.execute("ALTER TABLE contacts ADD COLUMN actions_done_at TEXT", []);
}

// ---------------- Done outside the CRM ----------------

/// Everything waiting on this contact is dealt with: its open replies leave the
/// desk, its classified replies are handled, and what came in so far stops
/// showing. A new reply brings the contact back.
pub fn mark_done(conn: &Connection, contact_id: i64) -> Result<(), String> {
    let open: Vec<i64> = conn
        .prepare("SELECT id FROM reply_desk WHERE contact_id = ?1 AND state IN ('pending','open','snoozed')")
        .and_then(|mut st| st.query_map(params![contact_id], |r| r.get(0)).map(|it| it.flatten().collect()))
        .map_err(|e| e.to_string())?;
    for id in open {
        autonomy::take_over(conn, id, "Traité hors du CRM par Thibault.")?;
        conn.execute("UPDATE reply_desk SET state = 'ignored', updated_at = datetime('now') WHERE id = ?1", params![id])
            .map_err(|e| e.to_string())?;
    }
    conn.execute(
        "UPDATE reply_insights SET handled = 1, handled_at = datetime('now') WHERE contact_id = ?1 AND handled = 0",
        params![contact_id],
    )
    .map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE contacts SET actions_done_at = datetime('now'), updated_at = datetime('now') WHERE id = ?1",
        params![contact_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// A follow-up made outside the CRM: done now, the next one comes due after the
/// usual delay. ("Ne plus relancer" is `dismiss_followup`.)
pub fn followup_done(conn: &Connection, contact_id: i64, today: NaiveDate) -> Result<(), String> {
    mark_done(conn, contact_id)?;
    status::follow_up_on(conn, contact_id, &(today + Duration::days(status::FOLLOWUP_DAYS)).to_string());
    Ok(())
}

#[tauri::command(async)]
pub fn action_done(state: State<'_, AppState>, contact_id: i64) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    mark_done(&conn, contact_id)?;
    reply_desk::emit_changed();
    Ok(())
}

#[tauri::command(async)]
pub fn action_followup_done(state: State<'_, AppState>, contact_id: i64) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    followup_done(&conn, contact_id, chrono::Local::now().date_naive())?;
    reply_desk::emit_changed();
    Ok(())
}

// ---------------- Replies still waiting ----------------

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct OpenReply {
    pub reply_id: i64,
    pub contact_id: i64,
    pub name: String,
    pub subject: String,
    pub received_at: String,
}

/// The last reply of each contact that still waits for Thibault: no email sent
/// since, not closed by hand, not left on the desk as ignored or answered. A
/// classified reply is not here: the residency actions list what needs an
/// answer, and a refusal or an out-of-office needs none.
pub fn open_replies(conn: &Connection) -> Result<Vec<OpenReply>, String> {
    let sql = format!(
        "SELECT r.id, k.id, COALESCE(NULLIF(trim(k.name),''), r.from_addr), COALESCE(r.subject,''), MAX(COALESCE(r.received_at, r.created_at))
         FROM email_replies r
         JOIN contacts k ON k.id = r.contact_id
              OR (r.contact_id IS NULL AND k.email IS NOT NULL AND lower(trim(k.email)) = lower(trim(r.from_addr)))
         WHERE r.kind = 'reply'
           AND COALESCE(r.received_at, r.created_at) >= datetime('now', '-{days} days')
           AND COALESCE(r.received_at, r.created_at) > COALESCE(k.actions_done_at, '')
           AND k.status <> 'declined'
           AND NOT EXISTS (SELECT 1 FROM reply_insights i WHERE i.email_reply_id = r.id)
           AND NOT EXISTS (SELECT 1 FROM reply_desk d WHERE d.email_reply_id = r.id AND d.state NOT IN ('pending','open','snoozed'))
           AND NOT EXISTS (SELECT 1 FROM emails s WHERE s.status = 'sent' AND s.sent_at > COALESCE(r.received_at, r.created_at)
                           AND (s.contact_id = k.id OR lower(trim(s.to_addr)) = lower(trim(r.from_addr))))
         GROUP BY k.id
         ORDER BY 5 DESC",
        days = REPLY_DAYS
    );
    let mut st = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = st
        .query_map([], |r| {
            Ok(OpenReply {
                reply_id: r.get(0)?,
                contact_id: r.get(1)?,
                name: r.get(2)?,
                subject: r.get(3)?,
                received_at: r.get(4)?,
            })
        })
        .map_err(|e| e.to_string())?;
    Ok(rows.flatten().collect())
}

#[tauri::command(async)]
pub fn list_open_replies(state: State<'_, AppState>) -> Result<Vec<OpenReply>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    open_replies(&conn)
}

// ---------------- Threading ----------------

fn latest_reply(conn: &Connection, contact_id: i64) -> Option<(Message, Option<String>)> {
    conn.query_row(
        "SELECT COALESCE(r.received_at, r.created_at), COALESCE(r.subject,''), COALESCE(r.body_excerpt,''), r.message_id
         FROM email_replies r JOIN contacts k ON k.id = ?1
         WHERE r.kind = 'reply' AND (r.contact_id = k.id OR lower(trim(r.from_addr)) = lower(trim(k.email)))
         ORDER BY COALESCE(r.received_at, r.created_at) DESC, r.id DESC LIMIT 1",
        params![contact_id],
        |r| Ok((Message { at: r.get(0)?, subject: r.get(1)?, body: r.get(2)? }, r.get(3)?)),
    )
    .ok()
}

fn latest_sent(conn: &Connection, contact_id: i64) -> Option<(Message, Option<String>)> {
    conn.query_row(
        "SELECT e.sent_at, e.subject, e.body, e.message_id FROM emails e
         WHERE e.contact_id = ?1 AND e.status = 'sent' ORDER BY e.sent_at DESC, e.id DESC LIMIT 1",
        params![contact_id],
        |r| Ok((Message { at: r.get(0)?, subject: r.get(1)?, body: r.get(2)? }, r.get(3)?)),
    )
    .ok()
}

/// Where an answer to this contact sits, so that it lands in the same
/// conversation on their side.
pub fn thread_of(conn: &Connection, contact_id: i64) -> Option<ThreadRef> {
    let ours = latest_sent(conn, contact_id).and_then(|(_, id)| id);
    // Ids we made up for messages without one are not worth threading on.
    let theirs = latest_reply(conn, contact_id).and_then(|(_, id)| id).filter(|m| !m.starts_with("<imap-"));
    let mut references: Vec<String> = Vec::new();
    for m in [&ours, &theirs].into_iter().flatten() {
        if !m.trim().is_empty() && !references.contains(m) {
            references.push(m.trim().to_string());
        }
    }
    let in_reply_to = theirs.or(ours).filter(|m| !m.trim().is_empty())?;
    Some(ThreadRef { in_reply_to: Some(in_reply_to), references })
}

// ---------------- Drafts ----------------

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ActionDraft {
    pub subject: String,
    pub body: String,
    /// "desk" (the one already written), a model name, or "template".
    pub by: String,
}

/// The draft behind a "Répondre" or "Relancer": the one already waiting on the
/// desk when there is one, else written now from the conversation, by Claude
/// when available, else from the template. Never sent from here: it opens in
/// Compose, for Thibault to read.
pub fn draft_for(conn: &Connection, contact_id: i64, kind: Kind, llm: Option<&dyn Llm>, today: NaiveDate) -> Option<ActionDraft> {
    let waiting: Option<(String, String)> = conn
        .query_row(
            "SELECT subject, body FROM reply_desk WHERE contact_id = ?1 AND state IN ('pending','open','snoozed')
               AND trim(body) <> '' ORDER BY id DESC LIMIT 1",
            params![contact_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();
    if let Some((subject, body)) = waiting {
        return Some(ActionDraft { subject, body, by: "desk".into() });
    }

    let (name, venue, email): (String, String, String) = conn
        .query_row(
            "SELECT name, COALESCE(venue,''), COALESCE(email,'') FROM contacts WHERE id = ?1",
            params![contact_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .ok()?;
    let ours = latest_sent(conn, contact_id).map(|(m, _)| m);
    let theirs = if kind == Kind::Reply { latest_reply(conn, contact_id).map(|(m, _)| m) } else { None };
    // A reply with nothing to answer yet is a follow-up.
    let kind = if kind == Kind::Reply && theirs.is_none() { Kind::Nudge } else { kind };
    let lang = theirs
        .as_ref()
        .and_then(|m| reply_desk::detect_lang(&format!("{}\n{}", m.subject, m.body)))
        .or_else(|| ours.as_ref().and_then(|m| reply_desk::detect_lang(&m.body)))
        .unwrap_or("fr")
        .to_string();
    let facts = reply_desk::facts_for(conn, Some(contact_id), None, today);
    let subject = match theirs.as_ref().or(ours.as_ref()) {
        Some(m) if !m.subject.trim().is_empty() => reply_desk::re_subject(&m.subject),
        _ => format!("{} x {}", facts.artist, if venue.trim().is_empty() { &name } else { &venue }),
    };
    let input = DraftInput {
        kind: Some(kind),
        lang,
        today: Some(today),
        contact: name,
        venue,
        from_addr: email,
        ours,
        theirs,
        facts,
        playbook: crate::commands::playbook::prompt_context(conn),
        // Opens in Compose: nothing here leaves on its own.
        autonomy: false,
    };

    if let Some(llm) = llm {
        let model = claude::drafting_model(conn);
        let req = reply_desk::build_request(&input, &model);
        for _ in 0..2 {
            match llm.complete(&req) {
                Ok(resp) => {
                    let m = if resp.model.is_empty() { model.clone() } else { resp.model.clone() };
                    claude::log_usage(conn, "action_draft", None, &m, &resp.usage);
                    if let Some(a) = reply_desk::parse_answer_full(&resp.text) {
                        return Some(ActionDraft { subject, body: a.body, by: m });
                    }
                }
                Err(e) => {
                    eprintln!("[actions] draft {}: {}", contact_id, e);
                    break;
                }
            }
        }
    }
    Some(ActionDraft { subject, body: reply_desk::template_draft(&input), by: "template".into() })
}

#[tauri::command(async)]
pub fn action_draft(state: State<'_, AppState>, contact_id: i64, kind: String) -> Result<ActionDraft, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let llm = claude::client(&conn);
    let kind = if kind == "reply" { Kind::Reply } else { Kind::Nudge };
    draft_for(&conn, contact_id, kind, llm.as_deref(), chrono::Local::now().date_naive()).ok_or_else(|| "Contact introuvable.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::residency::claude::fake::FakeLlm;
    use crate::commands::residency::engine::tests::setup;
    use chrono::Utc;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }
    fn ago(h: i64) -> String {
        (Utc::now() - Duration::hours(h)).format("%Y-%m-%d %H:%M:%S").to_string()
    }

    /// One email sent to contact 10 two days ago, answered yesterday.
    fn conversation(c: &Connection) {
        c.execute(
            "INSERT INTO emails (id, contact_id, to_addr, subject, body, status, sent_at, message_id)
             VALUES (1, 10, 'booking@cluba.fr', 'Club A x Piers', 'Hello, une résidence ?', 'sent', ?1, '<ours@x>')",
            params![ago(48)],
        )
        .unwrap();
        c.execute(
            "INSERT INTO email_replies (id, email_id, contact_id, from_addr, subject, message_id, kind, received_at, body_excerpt)
             VALUES (1, 1, 10, 'booking@cluba.fr', 'Re: Club A x Piers', '<theirs@x>', 'reply', ?1, 'Bonjour, quel est votre cachet ?')",
            params![ago(24)],
        )
        .unwrap();
    }

    #[test]
    fn a_reply_waits_until_answered_or_closed() {
        let c = setup();
        conversation(&c);
        assert_eq!(open_replies(&c).unwrap().len(), 1);

        // Done outside the CRM: gone, and it stays gone.
        mark_done(&c, 10).unwrap();
        assert!(open_replies(&c).unwrap().is_empty());

        // A new reply after that brings the contact back, once.
        c.execute(
            "INSERT INTO email_replies (id, email_id, contact_id, from_addr, message_id, kind, received_at)
             VALUES (2, 1, 10, 'booking@cluba.fr', '<t2@x>', 'reply', datetime('now', '+1 minute'))",
            [],
        )
        .unwrap();
        let open = open_replies(&c).unwrap();
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].reply_id, 2);
    }

    #[test]
    fn an_answer_or_a_closed_item_clears_the_line_without_a_click() {
        // Answered from the CRM.
        let c = setup();
        conversation(&c);
        c.execute(
            "INSERT INTO emails (contact_id, to_addr, subject, body, status, sent_at) VALUES (10, 'booking@cluba.fr', 'Re', 'b', 'sent', ?1)",
            params![ago(1)],
        )
        .unwrap();
        assert!(open_replies(&c).unwrap().is_empty());

        // Ignored on the desk.
        let c = setup();
        conversation(&c);
        c.execute(
            "INSERT INTO reply_desk (thread_key, kind, contact_id, email_reply_id, to_addr, state, since)
             VALUES ('r:1', 'reply', 10, 1, 'booking@cluba.fr', 'ignored', ?1)",
            params![ago(24)],
        )
        .unwrap();
        assert!(open_replies(&c).unwrap().is_empty());

        // Classified (a refusal needs no answer, the rest are residency actions).
        let c = setup();
        conversation(&c);
        c.execute(
            "INSERT INTO reply_insights (email_reply_id, contact_id, sentiment, intent, handled) VALUES (1, 10, 'neutral', 'question', 1)",
            [],
        )
        .unwrap();
        assert!(open_replies(&c).unwrap().is_empty());
    }

    #[test]
    fn done_closes_the_desk_and_the_insights_of_that_contact_only() {
        let c = setup();
        conversation(&c);
        c.execute_batch(
            "INSERT INTO reply_desk (thread_key, kind, contact_id, email_reply_id, to_addr, state, since) VALUES
               ('r:1', 'reply', 10, 1, 'booking@cluba.fr', 'open', '2026-11-01 10:00:00'),
               ('n:11', 'nudge', 11, NULL, 'booking@clubb.co.uk', 'open', '2026-11-01 10:00:00');
             INSERT INTO reply_insights (email_reply_id, contact_id, sentiment, intent) VALUES (1, 10, 'neutral', 'question');",
        )
        .unwrap();
        mark_done(&c, 10).unwrap();
        let state = |cid: i64| -> String {
            c.query_row("SELECT state FROM reply_desk WHERE contact_id = ?1", params![cid], |r| r.get(0)).unwrap()
        };
        assert_eq!(state(10), "ignored");
        assert_eq!(state(11), "open");
        let handled: i64 = c.query_row("SELECT handled FROM reply_insights", [], |r| r.get(0)).unwrap();
        assert_eq!(handled, 1);
    }

    #[test]
    fn a_followup_done_elsewhere_comes_back_after_the_usual_delay() {
        let c = setup();
        followup_done(&c, 10, d("2026-11-07")).unwrap();
        let due: String = c.query_row("SELECT follow_up FROM contacts WHERE id = 10", [], |r| r.get(0)).unwrap();
        assert_eq!(due, "2026-11-14");
    }

    #[test]
    fn drafts_answer_the_conversation_and_stay_in_it() {
        let c = setup();
        conversation(&c);
        let llm = FakeLlm::new(&[
            r#"{"intent":"question","body":"Hello,\n\nMerci pour le retour.\n\nÀ bientôt,\nThibault","category":"none","confidence":0.4,"playbook_used":[]}"#,
        ]);
        let a = draft_for(&c, 10, Kind::Reply, Some(&llm), d("2026-11-07")).unwrap();
        assert_eq!(a.subject, "Re: Club A x Piers");
        assert!(a.body.contains("Merci pour le retour"));
        // Claude saw their message and ours.
        let seen = llm.seen.lock().unwrap();
        assert!(seen[0].user.contains("quel est votre cachet") && seen[0].user.contains("une résidence"));
        drop(seen);
        let calls: i64 = c.query_row("SELECT COUNT(*) FROM ai_usage WHERE purpose = 'action_draft'", [], |r| r.get(0)).unwrap();
        assert_eq!(calls, 1);

        let t = thread_of(&c, 10).unwrap();
        assert_eq!(t.in_reply_to.as_deref(), Some("<theirs@x>"));
        assert_eq!(t.references, vec!["<ours@x>".to_string(), "<theirs@x>".to_string()]);
    }

    #[test]
    fn drafts_work_without_claude_and_reuse_what_is_written() {
        let c = setup();
        conversation(&c);
        // No key: the template, never an empty form.
        let a = draft_for(&c, 10, Kind::Nudge, None, d("2026-11-07")).unwrap();
        assert_eq!(a.by, "template");
        assert!(a.body.contains("Thibault") && !a.body.trim().is_empty());
        assert_eq!(a.subject, "Re: Club A x Piers");

        // Never emailed from the app: still a subject and a body.
        let a = draft_for(&c, 11, Kind::Nudge, None, d("2026-11-07")).unwrap();
        assert_eq!(a.subject, "Piers x Club B");
        assert!(!a.body.trim().is_empty());
        assert!(thread_of(&c, 11).is_none());

        // The draft waiting on the desk costs nothing and is the one that opens.
        c.execute(
            "INSERT INTO reply_desk (thread_key, kind, contact_id, to_addr, subject, body, state, since)
             VALUES ('r:1', 'reply', 10, 'booking@cluba.fr', 'Re: X', 'Déjà écrit', 'open', '2026-11-01 10:00:00')",
            [],
        )
        .unwrap();
        let llm = FakeLlm::new(&[]);
        let a = draft_for(&c, 10, Kind::Reply, Some(&llm), d("2026-11-07")).unwrap();
        assert_eq!((a.body.as_str(), a.by.as_str()), ("Déjà écrit", "desk"));
        assert!(llm.seen.lock().unwrap().is_empty());
    }
}
