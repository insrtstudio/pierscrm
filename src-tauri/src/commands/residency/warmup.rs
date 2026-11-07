//! The switch from the residency pitch to "warm-up or last-minute cover"
//! (November 2026). One shot at the first launch of the version carrying it,
//! so the old code never runs on the new copy. For each club campaign still
//! pitching a residency:
//! - the configuration takes the warm-up pitch and sequence, the name follows;
//! - first emails not sent yet are rewritten. One Claude wrote or Thibault
//!   edited keeps its true detail about the club: only the ask changes;
//! - follow-ups of clubs already written to change angle in their conversation;
//! - status and notes do not move: a follow-up held for review stays held.
//! What it replaced is kept under the `theme_warmup_backup` setting. Autopilots
//! cut for the rewrite (log code `paused_for_rewrite`) go back on.

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::json;

use super::content;
use super::targeting::target_info;
use crate::commands::email::render_template;
use super::{default_steps, load, log, save_config};

const DONE_KEY: &str = "theme_warmup_done";
const BACKUP_KEY: &str = "theme_warmup_backup";

pub const PITCH_FR: &str = "Moi c'est Thibault, je joue sous le nom de Piers (house, tech house) depuis plusieurs années. \
Je cherche à jouer en club, un warm-up en début de soirée ou un remplacement quand un DJ annule au dernier moment.";
pub const PITCH_EN: &str = "I'm Thibault, I play as Piers (house, tech house) and have for years. I'm looking to play at clubs, \
a warm-up at the start of the night or a cover when a DJ cancels at the last minute.";

const ASK_FR: &str = "Un warm-up, ça me va carrément, j'adore ouvrir une soirée. Et si un jour un DJ vous lâche à la dernière minute, écrivez-moi, je peux dépanner.";
const ASK_EN: &str = "Happy to do a warm-up, I actually love opening a night. And if a DJ ever bails on you last minute, just hit me up, I can step in.";
const CLOSE_FR: &str = "Si ça vous parle, je vous envoie mes dispos.";
const CLOSE_EN: &str = "If that sounds good, I'll send over my dates.";

/// A club campaign still pitching a residency.
pub fn pitches_residency(name: &str, first_subject: &str) -> bool {
    let n = name.to_lowercase();
    if n.contains("collecti") {
        return false;
    }
    let s = first_subject.to_lowercase();
    n.contains("résidence") || n.contains("residence") || n.contains("residency")
        || ["résidence", "residency", "une date par mois", "monthly night"].iter().any(|w| s.contains(w))
}

/// "Résidence Belgique" reads "Warm-up Belgique".
pub fn new_name(name: &str) -> String {
    let t = name.trim();
    if t.eq_ignore_ascii_case("Residence Parisienne") {
        return "Warm-up Paris".into();
    }
    for prefix in ["Résidence ", "Residence "] {
        if let Some(rest) = t.strip_prefix(prefix) {
            return format!("Warm-up {}", rest);
        }
    }
    format!("Warm-up {}", t)
}

/// The follow-up that changes angle inside a conversation about a residency.
pub fn pivot_body(lang: &str, link: &str) -> String {
    if lang == "en" {
        format!(
            "Hey,\n\nI wrote to you about a residency, which was maybe a lot all at once.\n\n\
             Simpler idea, if you ever need someone for a warm-up, I'd be happy to. And if a DJ bails on you last minute, just hit me up, I can step in.\n\n\
             The mix is still up here.\n{}\n\nCheers,\nThibault",
            link
        )
    } else {
        format!(
            "Hello,\n\nJe vous avais parlé d'une résidence, c'était peut-être beaucoup d'un coup.\n\n\
             Plus simple, si vous cherchez quelqu'un pour un warm-up, ça me va carrément. Et si un DJ vous lâche à la dernière minute, écrivez-moi, je peux dépanner.\n\n\
             Le mix est toujours ici.\n{}\n\nÀ bientôt,\nThibault",
            link
        )
    }
}

fn says_residency(text: &str) -> bool {
    let t = text.to_lowercase();
    ["résidence", "residence", "residency", "tous les mois", "tous les deux mois", "every month", "a month", "every two months", "date par mois", "monthly", "regular"]
        .iter()
        .any(|w| t.contains(w))
}

fn is_ask(paragraph: &str) -> bool {
    let t = paragraph.to_lowercase();
    says_residency(&t) || t.contains("looking for") || t.contains("je cherche")
}

fn is_episode(paragraph: &str) -> bool {
    let t = paragraph.to_lowercase();
    t.contains("piers fm") && (t.contains("episode") || t.contains("épisode"))
}

/// A personal first email with its ask swapped: greeting, the detail about the
/// club, the mix and the signature stay. None when it does not have that shape
/// or still says "residency" afterwards: the template is then the honest choice.
pub fn retheme_first(body: &str, lang: &str) -> Option<String> {
    let en = lang == "en";
    let mut out: Vec<String> = Vec::new();
    let mut asked = false;
    for p in body.trim().split("\n\n") {
        let last_line_link = p.contains("http");
        if !last_line_link && is_ask(p) {
            if !asked {
                out.push((if en { ASK_EN } else { ASK_FR }).to_string());
                asked = true;
            }
        } else if !last_line_link && is_episode(p) {
            if !asked {
                out.push((if en { ASK_EN } else { ASK_FR }).to_string());
                asked = true;
            }
        } else {
            out.push(p.to_string());
        }
    }
    if !asked {
        return None;
    }
    // The line before the signature no longer asks for a call about a residency.
    let link_at = out.iter().rposition(|p| p.contains("http"))?;
    if out.len() < link_at + 2 {
        return None;
    }
    let sign_at = out.len() - 1;
    if sign_at > link_at + 1 {
        out.splice(link_at + 1..sign_at, [(if en { CLOSE_EN } else { CLOSE_FR }).to_string()]);
    } else {
        out.insert(sign_at, (if en { CLOSE_EN } else { CLOSE_FR }).to_string());
    }
    // The introduction went with the ask: nobody says who is writing any more.
    let ask = if en { ASK_EN } else { ASK_FR };
    if out.get(1).map(String::as_str) == Some(ask) {
        return None;
    }
    let text = out.join("\n\n");
    (!says_residency(&text)).then_some(text)
}

struct Pending {
    id: i64,
    contact_id: i64,
    step: i64,
    subject: String,
    body: String,
    lang: String,
    press_url: Option<String>,
    generated_by: Option<String>,
}

/// Rewrites one campaign. Returns (first emails, follow-ups) rewritten, and
/// appends what it replaced to `backup`.
fn switch_campaign(conn: &Connection, id: i64, name: &str, backup: &mut Vec<serde_json::Value>) -> Result<(usize, usize), String> {
    let mut res = load(conn, id).ok_or("campagne introuvable")?;
    backup.push(json!({"campaign_id": id, "name": name, "config": res.config}));
    res.config.pitch_fr = PITCH_FR.into();
    res.config.pitch_en = PITCH_EN.into();
    res.config.concept = String::new();
    res.config.ask = "warmup".into();
    res.config.steps = default_steps();
    save_config(conn, id, &res.config)?;
    conn.execute(
        "UPDATE campaigns SET name = ?2, purpose = 'Clubs : warm-up et dépannage' WHERE id = ?1",
        params![id, new_name(name)],
    )
    .map_err(|e| e.to_string())?;

    let rows: Vec<Pending> = conn
        .prepare(
            "SELECT id, contact_id, step, subject, body, lang, press_url, generated_by FROM campaign_messages
             WHERE campaign_id = ?1 AND status IN ('draft','scheduled') ORDER BY id",
        )
        .and_then(|mut st| {
            st.query_map(params![id], |r| {
                Ok(Pending {
                    id: r.get(0)?,
                    contact_id: r.get(1)?,
                    step: r.get(2)?,
                    subject: r.get(3)?,
                    body: r.get(4)?,
                    lang: r.get(5)?,
                    press_url: r.get(6)?,
                    generated_by: r.get(7)?,
                })
            })
            .map(|it| it.flatten().collect())
        })
        .map_err(|e| e.to_string())?;

    let (mut firsts, mut followups) = (0, 0);
    let artist = content::artist_name(conn, id);
    for m in rows {
        backup.push(json!({"message_id": m.id, "subject": m.subject, "body": m.body}));
        if m.step == 1 {
            let Some(info) = target_info(conn, m.contact_id, &res.config) else { continue };
            let mut info = info;
            info.lang = m.lang.clone();
            let vars = content::vars_for(&info, &res.config, &artist, m.press_url.as_deref());
            let Some((subject, template)) = content::render_step(&res.config, 0, &m.lang, &vars) else { continue };
            let personal = m.generated_by.as_deref().map_or(false, |g| g != "template");
            let body = personal.then(|| retheme_first(&m.body, &m.lang)).flatten().unwrap_or(template);
            conn.execute(
                "UPDATE campaign_messages SET subject = ?2, body = ?3, updated_at = datetime('now') WHERE id = ?1",
                params![m.id, subject, render_template(&body, &vars)],
            )
            .map_err(|e| e.to_string())?;
            firsts += 1;
        } else {
            // The conversation keeps its subject, the same link stays.
            let link = m
                .press_url
                .clone()
                .filter(|u| !u.trim().is_empty())
                .or_else(|| content::assets_block(&res.config))
                .unwrap_or_default();
            conn.execute(
                "UPDATE campaign_messages SET body = ?2, updated_at = datetime('now') WHERE id = ?1",
                params![m.id, pivot_body(&m.lang, link.trim())],
            )
            .map_err(|e| e.to_string())?;
            followups += 1;
        }
    }
    Ok((firsts, followups))
}

/// Every residency campaign, in one transaction: all of it or none of it.
pub fn switch_all(conn: &Connection) -> Result<(usize, usize, usize), String> {
    let campaigns: Vec<(i64, String)> = conn
        .prepare("SELECT c.id, c.name FROM campaigns c JOIN residency_campaigns r ON r.campaign_id = c.id WHERE c.kind = 'residency' ORDER BY c.id")
        .and_then(|mut st| st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map(|it| it.flatten().collect()))
        .map_err(|e| e.to_string())?;
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    let mut backup = Vec::new();
    let (mut n, mut firsts, mut followups) = (0, 0, 0);
    let mut switched = Vec::new();
    for (id, name) in campaigns {
        let subject = load(&tx, id).and_then(|r| r.config.steps.first().map(|s| s.subject.clone())).unwrap_or_default();
        if !pitches_residency(&name, &subject) {
            continue;
        }
        let (f, u) = switch_campaign(&tx, id, &name, &mut backup)?;
        firsts += f;
        followups += u;
        n += 1;
        switched.push(id);
    }
    tx.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![BACKUP_KEY, serde_json::Value::Array(backup).to_string()],
    )
    .map_err(|e| e.to_string())?;
    for id in switched {
        let cut: bool = tx
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM residency_log WHERE campaign_id = ?1 AND code = 'paused_for_rewrite')",
                params![id],
                |r| r.get(0),
            )
            .unwrap_or(false);
        if cut {
            tx.execute("UPDATE residency_campaigns SET autopilot = 1, updated_at = datetime('now') WHERE campaign_id = ?1", params![id])
                .map_err(|e| e.to_string())?;
        }
        log(&tx, Some(id), "info", "theme_warmup", "Campagne passée de la résidence au warm-up et au dépannage");
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok((n, firsts, followups))
}

pub fn migrate(conn: &Connection) {
    let done: Option<String> = conn
        .query_row("SELECT value FROM settings WHERE key = ?1", params![DONE_KEY], |r| r.get(0))
        .optional()
        .ok()
        .flatten();
    if done.is_some() {
        return;
    }
    match switch_all(conn) {
        Ok(_) => {
            let _ = conn.execute("INSERT OR REPLACE INTO settings (key, value) VALUES (?1, '1')", params![DONE_KEY]);
        }
        // Nothing changed (one transaction): tried again at the next launch.
        Err(e) => eprintln!("[warmup] {}", e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::residency::engine::{pick_next, tests::setup};
    use crate::commands::residency::prospect::is_collective;
    use chrono::{DateTime, Utc};

    const PERSONAL: &str = "Hello,\n\nThibault here, I play under the name Piers. Seeing Traumer and Wehbba go through Culture Box is what made me reach out.\n\n\
I'm looking for a club in Copenhagen to play regularly, one date a month, something that settles in. I also film everything for PIERS FM, so each night becomes an episode.\n\n\
My latest mix (PIERS FM Vol. 4) is right below.\nhttps://p/1\n\nAre you free to talk this week?\n\nCheers,\nThibault";

    #[test]
    fn a_personal_email_keeps_its_detail_and_loses_the_ask() {
        let t = retheme_first(PERSONAL, "en").unwrap();
        assert!(t.contains("Traumer and Wehbba"));
        assert!(t.contains("Happy to do a warm-up"));
        assert!(t.contains("https://p/1") && t.ends_with("Cheers,\nThibault"));
        assert!(t.contains("send over my dates") && !t.contains("free to talk"));
        assert!(!t.to_lowercase().contains("a month") && !t.contains("episode"));
        // Nothing to swap, or a residency left in the detail: the template decides.
        assert_eq!(retheme_first("Hello,\n\nUn mot.\n\nhttps://p/1\n\nThibault", "fr"), None);
        assert_eq!(retheme_first("Hello,\n\nThibault here, looking for a regular night at Arch.\n\nhttps://p/1\n\nThibault", "en"), None);
        assert_eq!(retheme_first("Hello,\n\nJe cherche une date.\n\nhttps://x/residency\n\nThibault", "fr"), None);
    }

    #[test]
    fn who_switches() {
        assert!(pitches_residency("Résidence Belgique", "x"));
        assert!(pitches_residency("Autre", "{{venue}} x {{artist}}, une date par mois ?"));
        assert!(!pitches_residency("Demande Sur Collectif", "Warm-up ou dépannage chez {{venue}} ?"));
        assert!(!pitches_residency("Warm-up Belgique", "Warm-up ou dépannage chez {{venue}} ?"));
        assert_eq!(new_name("Résidence Pays-Bas"), "Warm-up Pays-Bas");
        assert_eq!(new_name("Residence Parisienne"), "Warm-up Paris");
    }

    #[test]
    fn the_switch_rewrites_what_is_pending_and_nothing_else() {
        let c = setup();
        c.execute("UPDATE campaigns SET name = 'Résidence Test'", []).unwrap();
        c.execute("UPDATE residency_campaigns SET autopilot = 0", []).unwrap();
        log(&c, Some(1), "info", "paused_for_rewrite", "x");
        // Contact 10: first email not sent, personal. Contact 11: written to,
        // its follow-up held for review, plus the sent one that must not move.
        c.execute_batch(
            "UPDATE campaign_messages SET press_url = 'https://p/1', generated_by = 'claude-haiku-5-5', lang = 'en', subject = 'a residency idea'
               WHERE contact_id = 10;
             UPDATE campaign_messages SET status = 'sent', subject = 'Club B x Piers, a residency idea', body = 'Sent residency' WHERE contact_id = 11;
             INSERT INTO campaign_messages (campaign_id, contact_id, step, subject, body, lang, status, note, press_url, generated_by)
             VALUES (1, 11, 2, 'Re: Club B x Piers, a residency idea', 'Je remonte', 'fr', 'draft', 'En attente : style', 'https://p/2', 'template');",
        )
        .unwrap();
        c.execute("UPDATE campaign_messages SET body = ?1 WHERE contact_id = 10", params![PERSONAL]).unwrap();

        // The test database was created by the same migrations: nothing to do then.
        c.execute("DELETE FROM settings WHERE key = 'theme_warmup_done'", []).unwrap();
        migrate(&c);

        let name: String = c.query_row("SELECT name FROM campaigns WHERE id = 1", [], |r| r.get(0)).unwrap();
        assert_eq!(name, "Warm-up Test");
        let cfg = load(&c, 1).unwrap().config;
        assert_eq!(cfg.ask, "warmup");
        assert!(cfg.steps[0].subject.contains("Warm-up"));
        // Staying a club campaign: the collectives refill does not take it.
        assert!(!is_collective(&name, &cfg));

        let (s, b): (String, String) = c
            .query_row("SELECT subject, body FROM campaign_messages WHERE contact_id = 10 AND step = 1", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!(s, "Warm-up or last-minute cover at Club A?");
        assert!(b.contains("Traumer") && b.contains("warm-up") && !b.contains("episode"));

        let (s, b, st, note): (String, String, String, String) = c
            .query_row("SELECT subject, body, status, note FROM campaign_messages WHERE contact_id = 11 AND step = 2", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })
            .unwrap();
        assert_eq!(s, "Re: Club B x Piers, a residency idea");
        assert!(b.contains("beaucoup d'un coup") && b.contains("https://p/2"));
        assert_eq!((st.as_str(), note.as_str()), ("draft", "En attente : style"));
        assert!(!b.contains(':') || b.contains("https:"), "no colon in the voice: {b}");

        // What was sent is history.
        let sent: String = c.query_row("SELECT body FROM campaign_messages WHERE contact_id = 11 AND step = 1", [], |r| r.get(0)).unwrap();
        assert_eq!(sent, "Sent residency");

        // The autopilot cut for the rewrite is back, and the text goes out.
        let on: i64 = c.query_row("SELECT autopilot FROM residency_campaigns WHERE campaign_id = 1", [], |r| r.get(0)).unwrap();
        assert_eq!(on, 1);
        let now: DateTime<Utc> = DateTime::parse_from_rfc3339("2026-11-09T09:30:00Z").unwrap().with_timezone(&Utc);
        assert!(pick_next(&c, now).is_some());

        // The old text can be found again, and a second launch changes nothing.
        let backup: String = c.query_row("SELECT value FROM settings WHERE key = 'theme_warmup_backup'", [], |r| r.get(0)).unwrap();
        assert!(backup.contains("Je remonte") && backup.contains("Traumer"));
        c.execute("UPDATE campaign_messages SET body = 'edited' WHERE contact_id = 11 AND step = 2", []).unwrap();
        migrate(&c);
        let b: String = c.query_row("SELECT body FROM campaign_messages WHERE contact_id = 11 AND step = 2", [], |r| r.get(0)).unwrap();
        assert_eq!(b, "edited");
    }

    #[test]
    fn a_follow_up_stays_in_the_conversation_it_answers() {
        let c = setup();
        c.execute("UPDATE campaign_messages SET subject = 'Club A x Piers, a residency idea', status = 'sent' WHERE contact_id = 10", []).unwrap();
        let mut cfg = load(&c, 1).unwrap().config;
        cfg.steps = default_steps();
        save_config(&c, 1, &cfg).unwrap();
        let start = chrono::Utc::now();
        content::create_next_step(&c, 1, &cfg, 10, 1, start).unwrap();
        let s: String = c.query_row("SELECT subject FROM campaign_messages WHERE contact_id = 10 AND step = 2", [], |r| r.get(0)).unwrap();
        assert_eq!(s, "Re: Club A x Piers, a residency idea");
    }
}
