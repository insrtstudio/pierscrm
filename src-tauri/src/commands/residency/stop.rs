//! Ending a campaign cleanly. A campaign that stops (ended or deleted) must
//! leave nothing behind: no email still due, and no contact handed back to
//! the manual follow-up queue. Without this, every club written to once or
//! twice came back in Aujourd'hui as "Relancer X" the day its campaign went.
//!
//! - `stop`: the campaign is over. What was not sent is cancelled, the history
//!   (emails, replies, figures) stays, the campaign shows as "Terminée".
//! - `reopen`: back to active, autopilot off; the cancelled emails return as
//!   drafts to validate again.
//! - `release_contacts`: what both `stop` and a deletion do for the contacts.
//! - `migrate`: once, the same for the campaigns deleted before this existed.

use rusqlite::{params, Connection};
use serde::Serialize;

/// Note carried by the emails a stop cancelled (what `reopen` brings back).
pub const STOP_NOTE: &str = "campagne arrêtée";

#[derive(Serialize, Debug, Default, PartialEq)]
pub struct Stopped {
    /// Emails not sent yet, cancelled.
    pub cancelled: usize,
    /// Contacts written to without an answer, taken off the follow-up queue.
    pub released: usize,
}

/// Contacts never to be put back in the manual follow-up queue by the end of
/// a campaign: those it wrote to, who never answered, and that no other
/// running campaign is still writing to. A conversation, a confirmed or a
/// closed contact is left alone. `sent_in` selects the contact ids written to.
fn release(conn: &Connection, sent_in: &str, campaign_id: Option<i64>) -> usize {
    conn.execute(
        &format!(
            "UPDATE contacts SET followup_dismissed = 1, updated_at = datetime('now')
             WHERE COALESCE(followup_dismissed, 0) = 0
               AND status NOT IN ('in_discussion', 'confirmed', 'declined', 'no_answer')
               AND id IN ({sent_in})
               AND NOT EXISTS (SELECT 1 FROM email_replies r WHERE r.contact_id = contacts.id AND r.kind = 'reply')
               AND NOT EXISTS (SELECT 1 FROM campaign_messages m JOIN campaigns c ON c.id = m.campaign_id
                               WHERE m.contact_id = contacts.id AND m.status IN ('draft', 'scheduled')
                                 AND c.status = 'active' AND (?1 IS NULL OR m.campaign_id <> ?1))"
        ),
        params![campaign_id],
    )
    .unwrap_or(0)
}

/// See `release`, for the contacts one campaign wrote to.
pub fn release_contacts(conn: &Connection, campaign_id: i64) -> usize {
    release(
        conn,
        "SELECT e.contact_id FROM emails e WHERE e.campaign_id = ?1 AND e.status = 'sent' AND e.contact_id IS NOT NULL",
        Some(campaign_id),
    )
}

/// End a campaign: cancel what is not sent, release its contacts, switch its
/// autopilot off and mark it done. Safe to call twice.
pub fn stop(conn: &Connection, campaign_id: i64) -> Result<Stopped, String> {
    let exists: bool = conn
        .query_row("SELECT EXISTS(SELECT 1 FROM campaigns WHERE id = ?1)", params![campaign_id], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if !exists {
        return Err("Campagne introuvable.".into());
    }
    let cancelled = conn
        .execute(
            "UPDATE campaign_messages SET status = 'skipped', note = ?2, updated_at = datetime('now')
             WHERE campaign_id = ?1 AND status IN ('draft', 'scheduled', 'failed')",
            params![campaign_id, STOP_NOTE],
        )
        .map_err(|e| e.to_string())?;
    let released = release_contacts(conn, campaign_id);
    conn.execute(
        "UPDATE residency_campaigns SET autopilot = 0, paused_reason = NULL, paused_kind = NULL, updated_at = datetime('now')
         WHERE campaign_id = ?1",
        params![campaign_id],
    )
    .map_err(|e| e.to_string())?;
    conn.execute("UPDATE campaigns SET status = 'done', updated_at = datetime('now') WHERE id = ?1", params![campaign_id])
        .map_err(|e| e.to_string())?;
    super::log(
        conn,
        Some(campaign_id),
        "info",
        "campaign_stopped",
        &format!("Campagne terminée : {} envoi(s) annulé(s), {} contact(s) sans relance", cancelled, released),
    );
    Ok(Stopped { cancelled, released })
}

/// Back to active with the autopilot off. The emails the stop cancelled come
/// back as drafts: nothing leaves before they are validated again.
pub fn reopen(conn: &Connection, campaign_id: i64) -> Result<usize, String> {
    conn.execute("UPDATE campaigns SET status = 'active', updated_at = datetime('now') WHERE id = ?1", params![campaign_id])
        .map_err(|e| e.to_string())?;
    let back = conn
        .execute(
            "UPDATE campaign_messages SET status = 'draft', note = NULL, updated_at = datetime('now')
             WHERE campaign_id = ?1 AND status = 'skipped' AND note = ?2",
            params![campaign_id, STOP_NOTE],
        )
        .map_err(|e| e.to_string())?;
    super::log(conn, Some(campaign_id), "info", "campaign_reopened", &format!("Campagne rouverte : {} brouillon(s) à valider", back));
    Ok(back)
}

/// Delete a campaign for good. Its contacts are released first; the emails
/// already sent stay in the history without a campaign.
pub fn delete(conn: &Connection, campaign_id: i64) -> Result<(), String> {
    release_contacts(conn, campaign_id);
    conn.execute("DELETE FROM campaigns WHERE id = ?1", params![campaign_id]).map_err(|e| e.to_string())?;
    conn.execute("UPDATE emails SET campaign_id = NULL WHERE campaign_id = ?1", params![campaign_id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Once: campaigns deleted before `delete` released their contacts left them
/// all due for a manual follow-up. Their emails still carry the id of a
/// campaign that no longer exists: that is how they are found.
pub fn migrate(conn: &Connection) {
    const KEY: &str = "campaign_orphans_released_v1";
    if super::engine::get(conn, KEY).is_some() {
        return;
    }
    let n = release(
        conn,
        "SELECT e.contact_id FROM emails e WHERE e.status = 'sent' AND e.contact_id IS NOT NULL
           AND e.campaign_id IS NOT NULL AND e.campaign_id NOT IN (SELECT id FROM campaigns)",
        None,
    );
    let _ = conn.execute(
        "UPDATE emails SET campaign_id = NULL WHERE campaign_id IS NOT NULL AND campaign_id NOT IN (SELECT id FROM campaigns)",
        [],
    );
    if n > 0 {
        super::log(conn, None, "info", "orphans_released", &format!("{} contact(s) de campagnes supprimées retirés des relances", n));
    }
    super::engine::put(conn, KEY, "1");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::residency::testdb;

    fn seed(c: &Connection) {
        c.execute_batch(
            "INSERT INTO campaigns (id, name, status, kind) VALUES (1, 'Warm-up Belgique', 'active', 'residency'),
                                                                 (2, 'Collectifs', 'active', 'residency');
             INSERT INTO residency_campaigns (campaign_id, config, autopilot) VALUES (1, '{}', 1), (2, '{}', 1);
             INSERT INTO contacts (id, name, email, status) VALUES
                (1, 'Mid', 'a@x.be', 'contacted'), (2, 'Replied', 'b@x.be', 'in_discussion'),
                (3, 'Elsewhere', 'c@x.be', 'contacted'), (4, 'Never', 'd@x.be', 'to_contact');
             INSERT INTO emails (id, contact_id, to_addr, subject, body, status, sent_at, campaign_id) VALUES
                (1, 1, 'a@x.be', 's', 'b', 'sent', '2026-11-10 10:00:00', 1),
                (2, 2, 'b@x.be', 's', 'b', 'sent', '2026-11-10 10:00:00', 1),
                (3, 3, 'c@x.be', 's', 'b', 'sent', '2026-11-10 10:00:00', 1);
             INSERT INTO campaign_messages (campaign_id, contact_id, step, status, sent_email_id) VALUES
                (1, 1, 1, 'sent', 1), (1, 1, 2, 'scheduled', NULL), (1, 4, 1, 'draft', NULL),
                (2, 3, 1, 'scheduled', NULL);",
        )
        .unwrap();
    }
    fn dismissed(c: &Connection, id: i64) -> bool {
        c.query_row("SELECT followup_dismissed FROM contacts WHERE id = ?1", [id], |r| r.get::<_, i64>(0)).unwrap() != 0
    }
    fn status(c: &Connection, id: i64) -> String {
        c.query_row("SELECT status FROM campaigns WHERE id = ?1", [id], |r| r.get(0)).unwrap()
    }

    #[test]
    fn stop_cancels_releases_and_reopens() {
        let c = testdb::conn();
        seed(&c);
        let s = stop(&c, 1).unwrap();
        assert_eq!(s, Stopped { cancelled: 2, released: 1 });
        assert_eq!(status(&c, 1), "done");
        // Written to, no answer: no manual follow-up. In discussion, written
        // to by another running campaign, never written to: untouched.
        assert!(dismissed(&c, 1));
        assert!(!dismissed(&c, 2) && !dismissed(&c, 3) && !dismissed(&c, 4));
        assert!(!crate::commands::residency::load(&c, 1).unwrap().autopilot);
        // Nothing left to send, and a second stop changes nothing.
        let due: i64 = c
            .query_row("SELECT COUNT(*) FROM campaign_messages WHERE campaign_id = 1 AND status IN ('draft','scheduled')", [], |r| r.get(0))
            .unwrap();
        assert_eq!(due, 0);
        assert_eq!(stop(&c, 1).unwrap(), Stopped::default());

        assert_eq!(reopen(&c, 1).unwrap(), 2);
        assert_eq!(status(&c, 1), "active");
        assert!(!crate::commands::residency::load(&c, 1).unwrap().autopilot);
    }

    #[test]
    fn delete_releases_and_orphans_are_cleaned_once() {
        let c = testdb::conn();
        seed(&c);
        delete(&c, 2).unwrap();
        // Contact 3 was only scheduled in campaign 2, written to by campaign 1.
        assert!(!dismissed(&c, 3));
        // A deletion made before this module, in a base whose emails carry no
        // foreign key (the column was added later): rows gone, emails still
        // tagged. The one-shot already ran when the test base was built.
        c.pragma_update(None, "foreign_keys", false).unwrap();
        c.execute_batch("DELETE FROM campaigns WHERE id = 1; DELETE FROM campaign_messages; DELETE FROM settings WHERE key = 'campaign_orphans_released_v1';").unwrap();
        c.pragma_update(None, "foreign_keys", true).unwrap();
        assert!(!dismissed(&c, 1));
        migrate(&c);
        assert!(dismissed(&c, 1) && dismissed(&c, 3));
        assert!(!dismissed(&c, 2) && !dismissed(&c, 4));
        let tagged: i64 = c.query_row("SELECT COUNT(*) FROM emails WHERE campaign_id IS NOT NULL", [], |r| r.get(0)).unwrap();
        assert_eq!(tagged, 0);
    }
}
