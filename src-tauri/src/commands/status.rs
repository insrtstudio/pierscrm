//! Single source of truth for pipeline status transitions.
//!
//! Every automatic status change (an email went out, a reply came in, a gig was
//! confirmed or cancelled, a cold sequence ran dry) goes through here, so the
//! rules live in one place instead of ad-hoc UPDATEs in each command. Manual
//! edits from the Pipeline (one contact or a selection) go through `set_manual`.
//!
//! Statuses (see STATUSES in src/lib/types.ts): to_contact, to_evaluate,
//! low_priority, contacted, followed_up, in_discussion, confirmed, declined,
//! no_answer.

use rusqlite::{params, Connection};

/// Every pipeline status the app knows.
pub const STATUSES: &[&str] = &[
    "to_contact", "contacted", "followed_up", "in_discussion", "confirmed", "declined",
    "no_answer", "to_evaluate", "low_priority",
];

/// Days between two emails of a cold sequence (also the default follow-up delay).
pub const FOLLOWUP_DAYS: i64 = 7;
/// Emails sent without a reply before a contact counts as "no answer"
/// (first contact + two follow-ups, the usual booking cadence).
pub const NO_ANSWER_AFTER_EMAILS: i64 = 3;
/// Days to wait after the last of those emails before giving up.
pub const NO_ANSWER_GRACE_DAYS: i64 = 14;
/// A follow-up date set by hand on a contact never emailed from the app shows
/// in the queue for this many days after it falls due (older imported dates
/// are history, not reminders).
pub const MANUAL_FOLLOWUP_WINDOW_DAYS: i64 = 30;

/// SQL predicate: contact `c` matches email `m` by id or by exact address.
/// Several contacts can share one address (a promoter booking two venues), and
/// emails typed by hand may carry no contact id.
const SAME_RECIPIENT: &str = "(m.contact_id = c.id
     OR (c.email IS NOT NULL AND trim(c.email) <> ''
         AND lower(trim(m.to_addr)) = lower(trim(c.email))))";

/// After a successful send. The contact (and every contact sharing that exact
/// address, since they were reached too and would otherwise be cold-mailed again
/// by the next "to contact" batch) moves forward:
/// - to_contact / to_evaluate / low_priority -> contacted
/// - contacted with an earlier sent email, or no_answer -> followed_up
/// - in_discussion, confirmed, declined stay as they are.
/// First contact is dated, the next follow-up is due in FOLLOWUP_DAYS, and a
/// "stop suggesting follow-ups" from before this send is lifted (a dismissal
/// only hides the contact until the next email).
pub fn on_email_sent(conn: &Connection, contact_id: Option<i64>, addr: &str) {
    let sql = format!(
        "UPDATE contacts AS c
         SET status = CASE
                 WHEN c.status IN ('to_contact','to_evaluate','low_priority') THEN 'contacted'
                 WHEN c.status = 'no_answer' THEN 'followed_up'
                 WHEN c.status = 'contacted' AND (
                     SELECT COUNT(*) FROM emails m WHERE m.status = 'sent' AND {same}
                 ) >= 2 THEN 'followed_up'
                 ELSE c.status END,
             first_contact = COALESCE(NULLIF(trim(c.first_contact), ''), date('now')),
             follow_up = CASE WHEN c.status IN ('confirmed','declined') THEN c.follow_up
                              ELSE date('now', '+{days} days') END,
             followup_dismissed = 0,
             updated_at = datetime('now')
         WHERE c.id = ?1
            OR (c.email IS NOT NULL AND trim(c.email) <> '' AND lower(trim(c.email)) = lower(trim(?2)))",
        same = SAME_RECIPIENT,
        days = FOLLOWUP_DAYS
    );
    let _ = conn.execute(&sql, params![contact_id, addr]);
}

/// A reply came in: the conversation is open, follow-ups stop. Contacts already
/// further along (confirmed, declined) keep their status.
pub fn on_reply(conn: &Connection, contact_id: Option<i64>, addr: &str) {
    let _ = conn.execute(
        "UPDATE contacts
         SET status = CASE
                 WHEN status IN ('to_contact','to_evaluate','low_priority','contacted','followed_up','no_answer')
                 THEN 'in_discussion' ELSE status END,
             follow_up = NULL,
             updated_at = datetime('now')
         WHERE id = ?1
            OR (email IS NOT NULL AND trim(email) <> '' AND lower(trim(email)) = lower(trim(?2)))",
        params![contact_id, addr],
    );
}

/// The contact refused or unsubscribed: the booking is closed and nothing more
/// is scheduled. Matches the contact id (a reply can come from another address
/// than the one on file) and every contact on that address. A confirmed gig
/// stays confirmed.
pub fn on_refused(conn: &Connection, contact_id: Option<i64>, addr: &str) {
    let _ = conn.execute(
        "UPDATE contacts
         SET status = CASE WHEN status = 'confirmed' THEN status ELSE 'declined' END,
             follow_up = NULL,
             updated_at = datetime('now')
         WHERE id = ?1
            OR (email IS NOT NULL AND trim(email) <> '' AND lower(trim(email)) = lower(trim(?2)))",
        params![contact_id, addr],
    );
}

/// A "reply" turned out not to be one (an out-of-office without auto headers):
/// undo `on_reply`, back to where the sequence stood (followed_up after two
/// emails, else contacted). Only a contact `on_reply` moved is touched.
pub fn on_reply_withdrawn(conn: &Connection, contact_id: i64) {
    let sql = format!(
        "UPDATE contacts AS c
         SET status = CASE WHEN (SELECT COUNT(*) FROM emails m WHERE m.status = 'sent' AND {same}) >= 2
                           THEN 'followed_up' ELSE 'contacted' END,
             updated_at = datetime('now')
         WHERE c.id = ?1 AND c.status = 'in_discussion'",
        same = SAME_RECIPIENT
    );
    let _ = conn.execute(&sql, params![contact_id]);
}

/// "Not now, come back in March": the next follow-up is due on `day`
/// (YYYY-MM-DD), and a dismissal is lifted so it shows up again then.
pub fn follow_up_on(conn: &Connection, contact_id: i64, day: &str) {
    let _ = conn.execute(
        "UPDATE contacts SET follow_up = ?2, followup_dismissed = 0, updated_at = datetime('now') WHERE id = ?1",
        params![contact_id, day],
    );
}

/// Stop suggesting follow-ups ("no response is a response"). Reversible with
/// `restore_followup`, and lifted by the next send anyway.
pub fn dismiss_followup(conn: &Connection, contact_id: i64) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE contacts SET followup_dismissed = 1, updated_at = datetime('now') WHERE id = ?1",
        params![contact_id],
    )
}

/// Undo a dismissal: the contact is back in the follow-up queue when due.
pub fn restore_followup(conn: &Connection, contact_id: i64) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE contacts SET followup_dismissed = 0, updated_at = datetime('now')
         WHERE id = ?1 AND followup_dismissed <> 0",
        params![contact_id],
    )
}

/// A status picked by hand in the Pipeline (one contact or a selection). The
/// user knows best, so any status is allowed, including moving back from a
/// closed one. Unknown keys are refused instead of stored (they would match no
/// filter or badge). Returns how many rows changed.
pub fn set_manual(conn: &Connection, contact_id: i64, status: &str) -> Result<usize, String> {
    if !STATUSES.contains(&status) {
        return Err(format!("unknown status: {status}"));
    }
    conn.execute(
        "UPDATE contacts SET status = ?2, updated_at = datetime('now') WHERE id = ?1 AND status IS NOT ?2",
        params![contact_id, status],
    )
    .map_err(|e| e.to_string())
}

/// Phrases that only appear in a clear "no" (EN / FR / NL, the venues written
/// to). Kept narrow on purpose: "fully booked", "not this season" or "maybe
/// later" are a "later", not a refusal, and stay in discussion.
const REFUSAL_PHRASES: &[&str] = &[
    "not interested",
    "no longer interested",
    "not a good fit",
    "not the right fit",
    "we will pass",
    "we'll pass",
    "we have to pass",
    "we're going to pass",
    "we are going to pass",
    "have to decline",
    "must decline",
    "decline your offer",
    "decline the offer",
    "please remove us",
    "pas interesse",
    "pas interessee",
    "pas interesses",
    "pas interessees",
    "ne sommes pas interesses",
    "ne correspond pas a notre programmation",
    "ne correspond pas a notre ligne",
    "passer notre tour",
    "passons notre tour",
    "decliner votre proposition",
    "decliner ta proposition",
    "geen interesse",
    "niet geinteresseerd",
    "past niet bij ons",
];

/// Lowercase, accents folded, apostrophes unified, so "Pas intéressés" and
/// "we’ll pass" match the phrase list.
fn fold(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| match c {
            'à' | 'â' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'î' | 'ï' => 'i',
            'ô' | 'ö' => 'o',
            'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            '\u{2019}' | '\u{2018}' => '\'',
            c => c,
        })
        .collect()
}

/// True when a reply (quoted text already stripped) reads as a plain refusal.
/// Long replies are left to a human: they usually say more than "no".
pub fn looks_like_refusal(text: &str) -> bool {
    let t = fold(text);
    if t.trim().is_empty() || t.chars().count() > 800 {
        return false;
    }
    // "Not interested right now, but..." / "pas intéressés pour l'instant" is a later.
    const SOFTENERS: &[&str] = &["right now", "at the moment", "for now", "this season", "next year", "later",
        "pour l'instant", "pour le moment", "cette saison", "l'annee prochaine", "plus tard",
        "op dit moment", "volgend jaar"];
    REFUSAL_PHRASES.iter().any(|p| t.contains(p)) && !SOFTENERS.iter().any(|s| t.contains(s))
}

/// A reply's text arrived (outbox sync): a clear refusal closes the booking.
/// Returns true when it did.
pub fn on_reply_text(conn: &Connection, contact_id: Option<i64>, addr: &str, text: &str) -> bool {
    if !looks_like_refusal(text) {
        return false;
    }
    on_refused(conn, contact_id, addr);
    true
}

/// A gig linked to the contact was confirmed: the booking is closed. Without
/// this the Pipeline, the dashboard "confirmed" counter and the follow-up queue
/// kept treating the venue as still being prospected.
pub fn on_event_confirmed(conn: &Connection, contact_id: i64) {
    let _ = conn.execute(
        "UPDATE contacts SET status = 'confirmed', follow_up = NULL, updated_at = datetime('now')
         WHERE id = ?1 AND status <> 'confirmed'",
        params![contact_id],
    );
}

/// A confirmed gig was cancelled, deleted, or moved to another contact. The
/// contact goes back to "in discussion" unless another of its gigs is still
/// confirmed. Only a status this module set (confirmed) is reverted.
pub fn on_event_cancelled(conn: &Connection, contact_id: i64) {
    let _ = conn.execute(
        "UPDATE contacts SET status = 'in_discussion', updated_at = datetime('now')
         WHERE id = ?1 AND status = 'confirmed'
           AND NOT EXISTS (SELECT 1 FROM events e WHERE e.contact_id = ?1 AND e.status = 'confirmed')",
        params![contact_id],
    );
}

/// Close cold sequences that ran dry: NO_ANSWER_AFTER_EMAILS sent emails, none
/// replied to, the last one more than NO_ANSWER_GRACE_DAYS old, and the contact
/// still waiting (contacted / followed_up). Returns how many contacts moved.
/// Run from the follow-up queue (the 3-email ceiling there is what makes these
/// contacts disappear from it) and after each reply sync.
pub fn apply_no_answer(conn: &Connection) -> usize {
    let sql = format!(
        "UPDATE contacts AS c
         SET status = 'no_answer', follow_up = NULL, updated_at = datetime('now')
         WHERE c.status IN ('contacted','followed_up')
           AND (SELECT COUNT(*) FROM emails m WHERE m.status = 'sent' AND {same}) >= {n}
           AND NOT EXISTS (SELECT 1 FROM emails m WHERE m.replied_at IS NOT NULL AND {same})
           AND NOT EXISTS (SELECT 1 FROM email_replies r WHERE r.contact_id = c.id AND r.kind = 'reply')
           AND (SELECT MAX(m.sent_at) FROM emails m WHERE m.status = 'sent' AND {same})
               < datetime('now', '-{grace} days')",
        same = SAME_RECIPIENT,
        n = NO_ANSWER_AFTER_EMAILS,
        grace = NO_ANSWER_GRACE_DAYS
    );
    conn.execute(&sql, []).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE contacts (id INTEGER PRIMARY KEY, name TEXT, email TEXT, status TEXT,
                first_contact TEXT, follow_up TEXT, followup_dismissed INTEGER NOT NULL DEFAULT 0,
                updated_at TEXT);
             CREATE TABLE emails (id INTEGER PRIMARY KEY, contact_id INTEGER, to_addr TEXT,
                status TEXT, sent_at TEXT NOT NULL DEFAULT (datetime('now')), replied_at TEXT);
             CREATE TABLE events (id INTEGER PRIMARY KEY, contact_id INTEGER, status TEXT);
             CREATE TABLE email_replies (id INTEGER PRIMARY KEY, contact_id INTEGER, kind TEXT);",
        )
        .unwrap();
        c
    }

    fn status(c: &Connection, id: i64) -> String {
        c.query_row("SELECT status FROM contacts WHERE id=?1", [id], |r| r.get(0)).unwrap()
    }

    #[test]
    fn send_moves_pipeline_and_shared_address() {
        let c = db();
        c.execute_batch(
            "INSERT INTO contacts (id, name, email, status, followup_dismissed) VALUES
               (1, 'A', 'x@v.nl', 'to_contact', 1),
               (2, 'B', ' X@V.nl ', 'low_priority', 0),
               (3, 'C', 'y@v.nl', 'confirmed', 0);
             INSERT INTO emails (contact_id, to_addr, status) VALUES (1, 'x@v.nl', 'sent');",
        )
        .unwrap();
        on_email_sent(&c, Some(1), "x@v.nl");
        assert_eq!(status(&c, 1), "contacted");
        assert_eq!(status(&c, 2), "contacted");
        let dismissed: i64 = c
            .query_row("SELECT followup_dismissed FROM contacts WHERE id=1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(dismissed, 0);
        // Second email: contacted -> followed_up.
        c.execute("INSERT INTO emails (contact_id, to_addr, status) VALUES (1, 'x@v.nl', 'sent')", [])
            .unwrap();
        on_email_sent(&c, Some(1), "x@v.nl");
        assert_eq!(status(&c, 1), "followed_up");
        // Confirmed is never downgraded.
        on_email_sent(&c, Some(3), "y@v.nl");
        assert_eq!(status(&c, 3), "confirmed");
    }

    #[test]
    fn reply_and_events() {
        let c = db();
        c.execute(
            "INSERT INTO contacts (id, name, email, status, follow_up) VALUES (1, 'A', 'x@v.nl', 'followed_up', '2026-01-01')",
            [],
        )
        .unwrap();
        on_reply(&c, None, "X@v.nl");
        assert_eq!(status(&c, 1), "in_discussion");
        on_event_confirmed(&c, 1);
        assert_eq!(status(&c, 1), "confirmed");
        // Another confirmed gig keeps the contact confirmed.
        c.execute("INSERT INTO events (id, contact_id, status) VALUES (9, 1, 'confirmed')", [])
            .unwrap();
        on_event_cancelled(&c, 1);
        assert_eq!(status(&c, 1), "confirmed");
        c.execute("UPDATE events SET status='cancelled'", []).unwrap();
        on_event_cancelled(&c, 1);
        assert_eq!(status(&c, 1), "in_discussion");
    }

    #[test]
    fn no_answer_after_three_silent_emails() {
        let c = db();
        c.execute_batch(
            "INSERT INTO contacts (id, name, email, status) VALUES
               (1, 'A', 'a@v.nl', 'followed_up'),
               (2, 'B', 'b@v.nl', 'followed_up'),
               (3, 'C', 'c@v.nl', 'followed_up');
             INSERT INTO emails (contact_id, to_addr, status, sent_at) VALUES
               (1, 'a@v.nl', 'sent', datetime('now','-40 days')),
               (1, 'a@v.nl', 'sent', datetime('now','-30 days')),
               (NULL, 'a@v.nl', 'sent', datetime('now','-20 days')),
               (2, 'b@v.nl', 'sent', datetime('now','-40 days')),
               (2, 'b@v.nl', 'sent', datetime('now','-30 days')),
               (2, 'b@v.nl', 'sent', datetime('now','-3 days')),
               (3, 'c@v.nl', 'sent', datetime('now','-40 days')),
               (3, 'c@v.nl', 'sent', datetime('now','-30 days'));
             INSERT INTO emails (contact_id, to_addr, status, sent_at, replied_at) VALUES
               (3, 'c@v.nl', 'sent', datetime('now','-20 days'), datetime('now','-19 days'));",
        )
        .unwrap();
        assert_eq!(apply_no_answer(&c), 1);
        assert_eq!(status(&c, 1), "no_answer");
        assert_eq!(status(&c, 2), "followed_up"); // last email too recent
        assert_eq!(status(&c, 3), "followed_up"); // replied
    }

    #[test]
    fn refusal_closes_by_id_and_address() {
        let c = db();
        c.execute_batch(
            "INSERT INTO contacts (id, name, email, status, follow_up) VALUES
               (1, 'A', 'x@v.nl', 'in_discussion', '2026-11-01'),
               (2, 'B', 'other@v.nl', 'followed_up', '2026-11-01'),
               (3, 'C', 'x@v.nl', 'confirmed', NULL);",
        )
        .unwrap();
        // Reply from a colleague's address, matched to contact 2 by message id.
        on_refused(&c, Some(2), "x@v.nl");
        assert_eq!(status(&c, 1), "declined");
        assert_eq!(status(&c, 2), "declined");
        assert_eq!(status(&c, 3), "confirmed");
        let fu: Option<String> = c.query_row("SELECT follow_up FROM contacts WHERE id=2", [], |r| r.get(0)).unwrap();
        assert_eq!(fu, None);
    }

    #[test]
    fn refusal_detection() {
        for t in [
            "Thanks, but we're not interested.",
            "Hi, we will pass on this one. Best",
            "Bonjour, nous ne sommes pas intéressés. Bonne continuation",
            "Merci, on va passer notre tour.",
            "Helaas, geen interesse. Groet",
            "We\u{2019}ll pass, cheers",
        ] {
            assert!(looks_like_refusal(t), "{t}");
        }
        let long = "not interested ".repeat(80);
        for t in [
            "",
            "Sounds great, what are your fees?",
            "Not interested right now, maybe next year.",
            "Pas intéressés pour l'instant, recontacte-nous en mars.",
            "We're fully booked until March.",
            long.as_str(),
        ] {
            assert!(!looks_like_refusal(t), "{t}");
        }
    }

    #[test]
    fn reply_text_only_moves_on_refusal() {
        let c = db();
        c.execute("INSERT INTO contacts (id, name, email, status) VALUES (1, 'A', 'x@v.nl', 'in_discussion')", [])
            .unwrap();
        assert!(!on_reply_text(&c, Some(1), "x@v.nl", "Let's talk, call me Monday"));
        assert_eq!(status(&c, 1), "in_discussion");
        assert!(on_reply_text(&c, Some(1), "x@v.nl", "Sorry, not a good fit for us."));
        assert_eq!(status(&c, 1), "declined");
    }

    #[test]
    fn withdrawn_reply_goes_back() {
        let c = db();
        c.execute_batch(
            "INSERT INTO contacts (id, name, email, status) VALUES
               (1, 'A', 'a@v.nl', 'in_discussion'), (2, 'B', 'b@v.nl', 'in_discussion'),
               (3, 'C', 'c@v.nl', 'confirmed');
             INSERT INTO emails (contact_id, to_addr, status) VALUES
               (1, 'a@v.nl', 'sent'), (NULL, 'a@v.nl', 'sent'), (2, 'b@v.nl', 'sent');",
        )
        .unwrap();
        on_reply_withdrawn(&c, 1);
        on_reply_withdrawn(&c, 2);
        on_reply_withdrawn(&c, 3);
        assert_eq!(status(&c, 1), "followed_up");
        assert_eq!(status(&c, 2), "contacted");
        assert_eq!(status(&c, 3), "confirmed");
    }

    #[test]
    fn manual_status_and_dismissal() {
        let c = db();
        c.execute("INSERT INTO contacts (id, name, email, status) VALUES (1, 'A', 'a@v.nl', 'declined')", [])
            .unwrap();
        assert_eq!(set_manual(&c, 1, "in_discussion").unwrap(), 1);
        assert_eq!(status(&c, 1), "in_discussion");
        assert_eq!(set_manual(&c, 1, "in_discussion").unwrap(), 0); // unchanged
        assert!(set_manual(&c, 1, "maybe").is_err());
        let dismissed = |c: &Connection| -> i64 {
            c.query_row("SELECT followup_dismissed FROM contacts WHERE id=1", [], |r| r.get(0)).unwrap()
        };
        dismiss_followup(&c, 1).unwrap();
        assert_eq!(dismissed(&c), 1);
        assert_eq!(restore_followup(&c, 1).unwrap(), 1);
        assert_eq!(dismissed(&c), 0);
        follow_up_on(&c, 1, "2027-03-01");
        let fu: String = c.query_row("SELECT follow_up FROM contacts WHERE id=1", [], |r| r.get(0)).unwrap();
        assert_eq!(fu, "2027-03-01");
    }

    /// The follow-up queue on the real schema: the `follow_up` column decides
    /// when a contact is due, a dismissal hides it until restored, and a date
    /// typed on a contact never emailed from the app still counts.
    #[test]
    fn followup_queue_respects_dates_and_dismissal() {
        let c = Connection::open_in_memory().unwrap();
        crate::db::migrate_for_tests(&c).unwrap();
        c.execute_batch(
            "INSERT INTO contacts (id, name, email, status, follow_up) VALUES
               (1, 'Late', 'a@v.nl', 'contacted', date('now', '+5 days')),
               (2, 'Due', 'b@v.nl', 'contacted', NULL),
               (3, 'Manual', 'c@v.nl', 'contacted', date('now', '-2 days')),
               (4, 'Old sheet', 'd@v.nl', 'contacted', '2024-01-10'),
               (5, 'Free text', 'e@v.nl', 'contacted', 'Wed Oct 21');
             INSERT INTO emails (contact_id, to_addr, subject, body, status, sent_at) VALUES
               (1, 'a@v.nl', 's', 'b', 'sent', datetime('now', '-10 days')),
               (2, 'b@v.nl', 's', 'b', 'sent', datetime('now', '-10 days'));",
        )
        .unwrap();
        let ids = |c: &Connection| -> Vec<i64> {
            let mut v: Vec<i64> =
                crate::commands::contacts::followups(c).unwrap().iter().map(|f| f.contact_id).collect();
            v.sort();
            v
        };
        // 1 waits for its follow_up date; 2 is due 7 days after the email.
        assert_eq!(ids(&c), vec![2, 3]);
        dismiss_followup(&c, 2).unwrap();
        assert_eq!(ids(&c), vec![3]);
        restore_followup(&c, 2).unwrap();
        assert_eq!(ids(&c), vec![2, 3]);
        c.execute("UPDATE contacts SET follow_up = date('now') WHERE id = 1", []).unwrap();
        assert_eq!(ids(&c), vec![1, 2, 3]);
    }
}
