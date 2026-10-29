//! Single source of truth for pipeline status transitions.
//!
//! Every automatic status change (an email went out, a reply came in, a gig was
//! confirmed or cancelled, a cold sequence ran dry) goes through here, so the
//! rules live in one place instead of ad-hoc UPDATEs in each command. Manual
//! edits from the Pipeline still write the status directly.
//!
//! Statuses (see STATUSES in src/lib/types.ts): to_contact, to_evaluate,
//! low_priority, contacted, followed_up, in_discussion, confirmed, declined,
//! no_answer.

use rusqlite::{params, Connection};

/// Days between two emails of a cold sequence (also the default follow-up delay).
pub const FOLLOWUP_DAYS: i64 = 7;
/// Emails sent without a reply before a contact counts as "no answer"
/// (first contact + two follow-ups, the usual booking cadence).
pub const NO_ANSWER_AFTER_EMAILS: i64 = 3;
/// Days to wait after the last of those emails before giving up.
pub const NO_ANSWER_GRACE_DAYS: i64 = 14;

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
/// is scheduled. A confirmed gig stays confirmed.
pub fn on_refused(conn: &Connection, addr: &str) {
    let _ = conn.execute(
        "UPDATE contacts
         SET status = CASE WHEN status = 'confirmed' THEN status ELSE 'declined' END,
             follow_up = NULL,
             updated_at = datetime('now')
         WHERE email IS NOT NULL AND trim(email) <> '' AND lower(trim(email)) = lower(trim(?1))",
        params![addr],
    );
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
}
