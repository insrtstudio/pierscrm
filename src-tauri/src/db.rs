use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use std::path::Path;

pub type DbPool = Pool<SqliteConnectionManager>;

pub fn init_pool(db_path: &Path) -> Result<DbPool, String> {
    let manager = SqliteConnectionManager::file(db_path).with_init(|c| {
        c.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;",
        )
    });
    let pool = Pool::builder()
        .max_size(8)
        .build(manager)
        .map_err(|e| e.to_string())?;
    {
        let conn = pool.get().map_err(|e| e.to_string())?;
        migrate(&conn)?;
        seed_defaults(&conn)?;
    }
    Ok(pool)
}

fn migrate(conn: &rusqlite::Connection) -> Result<(), String> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS artists (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            name          TEXT NOT NULL,
            real_name     TEXT,
            tagline       TEXT,
            bio           TEXT,
            genres        TEXT,
            city          TEXT,
            country       TEXT,
            avatar        TEXT,
            email         TEXT,
            phone         TEXT,
            booking_email TEXT,
            website       TEXT,
            instagram     TEXT,
            soundcloud    TEXT,
            spotify       TEXT,
            apple_music   TEXT,
            beatport      TEXT,
            youtube       TEXT,
            press_quotes  TEXT,
            achievements  TEXT,
            links         TEXT,
            created_at    TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at    TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS contacts (
            id              INTEGER PRIMARY KEY AUTOINCREMENT,
            artist_id       INTEGER REFERENCES artists(id) ON DELETE SET NULL,
            category        TEXT NOT NULL DEFAULT 'venue',
            priority        TEXT,
            name            TEXT NOT NULL,
            promoter        TEXT,
            venue           TEXT,
            type            TEXT,
            area            TEXT,
            scale           TEXT,
            date            TEXT,
            time            TEXT,
            format          TEXT,
            reason          TEXT,
            contact_channel TEXT,
            email           TEXT,
            email_status    TEXT,
            status          TEXT NOT NULL DEFAULT 'to_contact',
            first_contact   TEXT,
            follow_up       TEXT,
            notes           TEXT,
            website         TEXT,
            tags            TEXT,
            created_at      TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at      TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS templates (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            name       TEXT NOT NULL,
            subject    TEXT NOT NULL,
            body       TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS campaigns (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            name        TEXT NOT NULL,
            purpose     TEXT,
            event_name  TEXT,
            artist_id   INTEGER REFERENCES artists(id) ON DELETE SET NULL,
            target_date TEXT,
            status      TEXT NOT NULL DEFAULT 'active',
            color       TEXT,
            created_at  TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS emails (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            contact_id  INTEGER REFERENCES contacts(id) ON DELETE SET NULL,
            campaign_id INTEGER REFERENCES campaigns(id) ON DELETE SET NULL,
            to_addr     TEXT NOT NULL,
            subject     TEXT NOT NULL,
            body        TEXT NOT NULL,
            status      TEXT NOT NULL DEFAULT 'sent',
            error       TEXT,
            track_token TEXT,
            opened_at   TEXT,
            open_count  INTEGER NOT NULL DEFAULT 0,
            sent_at     TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS events (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            artist_id  INTEGER REFERENCES artists(id) ON DELETE CASCADE,
            contact_id INTEGER REFERENCES contacts(id) ON DELETE SET NULL,
            title      TEXT NOT NULL,
            venue      TEXT,
            city       TEXT,
            date       TEXT NOT NULL,
            start_time TEXT,
            end_time   TEXT,
            status     TEXT NOT NULL DEFAULT 'confirmed',
            fee        REAL,
            notes      TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS budget_items (
            id       INTEGER PRIMARY KEY AUTOINCREMENT,
            category TEXT,
            item     TEXT NOT NULL,
            min_cost REAL NOT NULL DEFAULT 0,
            max_cost REAL NOT NULL DEFAULT 0,
            actual   REAL,
            kind     TEXT NOT NULL DEFAULT 'expense',
            notes    TEXT,
            sort     INTEGER NOT NULL DEFAULT 0
        );

        CREATE TABLE IF NOT EXISTS tasks (
            id       INTEGER PRIMARY KEY AUTOINCREMENT,
            period   TEXT,
            title    TEXT NOT NULL,
            done     INTEGER NOT NULL DEFAULT 0,
            owner    TEXT,
            due_date TEXT,
            sort     INTEGER NOT NULL DEFAULT 0
        );

        CREATE TABLE IF NOT EXISTS kpis (
            id        INTEGER PRIMARY KEY AUTOINCREMENT,
            artist_id INTEGER REFERENCES artists(id) ON DELETE CASCADE,
            goal   TEXT,
            kpi    TEXT,
            target TEXT,
            actual TEXT,
            sort   INTEGER NOT NULL DEFAULT 0
        );

        CREATE TABLE IF NOT EXISTS settings (
            key   TEXT PRIMARY KEY,
            value TEXT
        );

        CREATE TABLE IF NOT EXISTS visa_countries (
            code            TEXT PRIMARY KEY,
            name            TEXT NOT NULL,
            work_rules      TEXT,
            visa_types      TEXT,
            processing_time TEXT,
            required_docs   TEXT,
            notes           TEXT,
            official_link   TEXT,
            updated_at      TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS visa_dossiers (
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            artist_id    INTEGER REFERENCES artists(id) ON DELETE SET NULL,
            country_code TEXT,
            country_name TEXT,
            title        TEXT NOT NULL,
            purpose      TEXT,
            event_date   TEXT,
            entry_date   TEXT,
            status       TEXT NOT NULL DEFAULT 'draft',
            checklist    TEXT,
            notes        TEXT,
            created_at   TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at   TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_contacts_status   ON contacts(status);
        CREATE INDEX IF NOT EXISTS idx_contacts_category ON contacts(category);
        CREATE INDEX IF NOT EXISTS idx_contacts_artist   ON contacts(artist_id);
        CREATE INDEX IF NOT EXISTS idx_kpis_artist       ON kpis(artist_id);
        CREATE INDEX IF NOT EXISTS idx_emails_contact    ON emails(contact_id);
        CREATE INDEX IF NOT EXISTS idx_events_date       ON events(date);
        CREATE INDEX IF NOT EXISTS idx_events_artist     ON events(artist_id);
        "#,
    )
    .map_err(|e| e.to_string())?;

    // Best-effort forward-compat migrations for databases created by earlier builds.
    for stmt in [
        "ALTER TABLE contacts ADD COLUMN artist_id INTEGER",
        "ALTER TABLE kpis ADD COLUMN artist_id INTEGER",
        "ALTER TABLE emails ADD COLUMN track_token TEXT",
        "ALTER TABLE emails ADD COLUMN opened_at TEXT",
        "ALTER TABLE emails ADD COLUMN open_count INTEGER NOT NULL DEFAULT 0",
        "ALTER TABLE emails ADD COLUMN campaign_id INTEGER",
        "ALTER TABLE campaigns ADD COLUMN event_name TEXT",
    ] {
        let _ = conn.execute(stmt, []);
    }
    // These indexes depend on columns added by the ALTERs above, so create them last.
    let _ = conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_emails_token ON emails(track_token)",
        [],
    );
    let _ = conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_emails_campaign ON emails(campaign_id)",
        [],
    );
    Ok(())
}

/// Seed a couple of default email templates on first run only.
fn seed_defaults(conn: &rusqlite::Connection) -> Result<(), String> {
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM templates", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if count == 0 {
        conn.execute(
            "INSERT INTO templates (name, subject, body) VALUES (?1, ?2, ?3)",
            rusqlite::params![
                "Slot pitch",
                "opening set for {{event}}?",
                "Hey {{name}},\n\nI'm Piers, a producer/DJ from France. Been keeping an eye on {{event}} and really like what you're doing with it.\n\nI'm around in Amsterdam for the whole week and wanted to ask — any chance there's still an opening slot on {{date}}? Happy to do a short B2B too. No fee, I just want to play the right room.\n\nHere's a recent mix if you want a listen: {{mix}}\n\nEither way, hope it's a good one.\nPiers"
            ],
        )
        .map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO templates (name, subject, body) VALUES (?1, ?2, ?3)",
            rusqlite::params![
                "Venue ask",
                "something at {{venue}} during ADE week?",
                "Hi {{name}},\n\nQuick one — are you doing anything on the early-week dates (Tue 20 / Wed 21 / Thu 22)?\n\nI'm Piers, a French producer/DJ. With my label Insrt.Studio we'd love to put on a small free-entry showcase at {{venue}}, early evening, a few of us playing. We'd bring our own crowd and you'd keep the bar — no fees either way.\n\nIf it sounds worth a chat I'm easy to reach. Here's a mix in the meantime: {{mix}}\n\nAnd if the timing's off, no worries at all — would still love to stay in touch.\nPiers — Insrt.Studio"
            ],
        )
        .map_err(|e| e.to_string())?;
    }

    seed_visa_countries(conn)?;
    Ok(())
}

/// Seed a starter set of destination countries for touring artists.
/// IMPORTANT: this is high-level orientation only — NOT legal advice. Every field
/// is editable in-app and must be verified against official government sources.
fn seed_visa_countries(conn: &rusqlite::Connection) -> Result<(), String> {
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM visa_countries", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if count > 0 {
        return Ok(());
    }
    let common_docs = "Valid passport (6+ months validity)\nSigned performance/booking contract\nInvitation letter from promoter/venue\nProof of accommodation\nReturn/onward travel proof\nProof of funds\nArtist bio / EPK\nRecent photo (ID format)";
    let disclaimer = "Orientation only — verify with the official government/embassy source before every trip. Rules depend on the artist's nationality and the exact purpose of travel.";

    let rows: &[(&str, &str, &str, &str, &str, &str)] = &[
        ("NL", "Netherlands",
         "Schengen area. Short performances may fall under short-stay rules for many nationalities; paid work can require a work permit (TWV/GVVA) arranged by the employer/promoter. Confirm whether a permit exemption applies for short artistic performances.",
         "Schengen short-stay (C) · Single permit (GVVA) for longer paid work",
         "Schengen visa (if required): a few weeks. Work permits: longer — plan ahead.",
         "https://ind.nl/en"),
        ("GB", "United Kingdom",
         "Not in Schengen. Performers often use the Permitted Paid Engagement route or a Creative Worker visa depending on length and payment. Some short engagements have specific allowances — confirm current rules.",
         "Permitted Paid Engagement · Creative Worker (Temporary Work) · Standard Visitor",
         "Varies by route; Creative Worker can need a Certificate of Sponsorship.",
         "https://www.gov.uk/browse/visas-immigration"),
        ("US", "United States",
         "Performance visas are typically required for paid work. The O and P categories cover artists/entertainers and require a petition (I-129) approved before the visa interview. Start very early — timelines are long.",
         "O-1 (extraordinary ability) · P-1/P-2/P-3 (performers) · Visa Waiver (ESTA) NOT valid for paid work",
         "Often 2–4+ months; premium processing may be available.",
         "https://travel.state.gov"),
        ("DE", "Germany",
         "Schengen area. Short artistic performances may be possible under short-stay rules for many nationalities; paid engagements can require the right permit. Confirm the current artist/performance provisions.",
         "Schengen short-stay (C) · National visa / work permit for longer stays",
         "Schengen: a few weeks. National permits: longer.",
         "https://www.auswaertiges-amt.de/en"),
        ("FR", "France",
         "Schengen area. For non-EU artists, a specific 'artist/performer' authorisation or work permit may be required for paid performances. EU nationals generally have freedom of movement.",
         "Schengen short-stay (C) · Talent/artist residence · Work permit (autorisation de travail)",
         "Varies; artist authorisations should be requested well ahead.",
         "https://france-visas.gouv.fr/en/"),
        ("ES", "Spain",
         "Schengen area. Paid performances by non-EU artists may require a work authorisation; short promo/non-paid activity may differ. Verify current requirements.",
         "Schengen short-stay (C) · Work permit for performances",
         "Varies by consulate.",
         "https://www.exteriores.gob.es/"),
        ("CH", "Switzerland",
         "Schengen for short stays but NOT in the EU. Paid work — even short — is often subject to notification/permit procedures (e.g. online notification for short assignments). Confirm cantonal rules.",
         "Schengen short-stay (C) · Short-work notification / permit",
         "Notification procedures can be quick but must be filed in advance.",
         "https://www.sem.admin.ch/"),
        ("CA", "Canada",
         "Some performers at certain events may be work-permit exempt; others require a work permit and possibly an LMIA. Rules depend on venue type and whether the event is a festival. Verify current exemptions.",
         "Work permit (may need LMIA) · Business visitor (limited) · eTA/visa for entry",
         "Varies; apply early, especially if an LMIA is needed.",
         "https://www.canada.ca/en/immigration-refugees-citizenship.html"),
    ];

    for (code, name, rules, types, timing, link) in rows {
        conn.execute(
            "INSERT INTO visa_countries (code, name, work_rules, visa_types, processing_time, required_docs, notes, official_link)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            rusqlite::params![code, name, rules, types, timing, common_docs, disclaimer, link],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}
