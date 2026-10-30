use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use std::path::Path;

pub type DbPool = Pool<SqliteConnectionManager>;

/// Normalise a name for matching / dedup: lowercase, fold Latin diacritics to
/// ASCII, drop every non-alphanumeric character. Used on both sides of the
/// artist match and for venue dedup, so both sides collapse identically.
/// (Manual folding, no extra crate: covers the reference-artist seed and the
/// Latin-script venues this module targets.)
pub fn normalise(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars().flat_map(char::to_lowercase) {
        let folded = match ch {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => 'a',
            'ç' | 'ć' | 'č' | 'ĉ' | 'ċ' => 'c',
            'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => 'e',
            'ì' | 'í' | 'î' | 'ï' | 'ī' | 'ĭ' | 'į' | 'ı' => 'i',
            'ñ' | 'ń' | 'ň' | 'ņ' => 'n',
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ŏ' | 'ő' => 'o',
            'ù' | 'ú' | 'û' | 'ü' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => 'u',
            'ý' | 'ÿ' => 'y',
            'ß' => 's',
            'š' | 'ś' | 'ş' => 's',
            'ž' | 'ź' | 'ż' => 'z',
            'ł' => 'l',
            'đ' => 'd',
            c => c,
        };
        if folded.is_ascii_alphanumeric() {
            out.push(folded);
        }
    }
    out
}

pub fn init_pool(db_path: &Path) -> Result<DbPool, String> {
    let manager = SqliteConnectionManager::file(db_path).with_init(|c| {
        // Per-connection tuning. `cache_size` is negative = KiB, so -16384 is a
        // 16 MB page cache per connection (the whole DB fits in it several times
        // over). `mmap_size` maps the file once per process, so the read path
        // stops paying a copy per page. `temp_store = MEMORY` keeps the ORDER BY
        // sorters used by the venue list out of a temp file on disk.
        c.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;
             PRAGMA synchronous = NORMAL;
             PRAGMA cache_size = -16384;
             PRAGMA temp_store = MEMORY;
             PRAGMA mmap_size = 268435456;",
        )
    });
    // Headroom for several concurrent workers + the UI's polling queries. WAL lets
    // readers and one writer coexist; a shorter connection timeout means a starved
    // getter fails fast (and is handled) instead of hanging for 30s.
    let pool = Pool::builder()
        .max_size(16)
        .connection_timeout(std::time::Duration::from_secs(10))
        .build(manager)
        .map_err(|e| e.to_string())?;
    {
        let conn = pool.get().map_err(|e| e.to_string())?;
        migrate(&conn)?;
        seed_defaults(&conn)?;
        maintenance(&conn);
    }
    Ok(pool)
}

/// Startup housekeeping. Harvest runs append a lot to the WAL and never fold it
/// back, so it grows unbounded (12 MB against a 28 MB database here) and every
/// reader has to walk it. Truncating at launch, when nothing else holds the
/// database, keeps reads on the main file. `PRAGMA optimize` refreshes the
/// planner statistics only for tables that changed enough to need it, which is
/// what makes the query planner pick the new composite indexes instead of a
/// full scan. Both are best-effort: a failure here must never block startup.
///
/// Crash recovery: nothing can be running at launch, so a run still 'en_cours'
/// was cut short by a quit or crash. It is marked 'arrete' (resumable) and its
/// claimed tasks go back to the queue, otherwise the UI shows it running forever.
fn maintenance(conn: &rusqlite::Connection) {
    let _ = conn.execute_batch(
        "UPDATE vi_runs SET statut='arrete', finished_at=COALESCE(finished_at, datetime('now'))
           WHERE statut='en_cours';
         UPDATE vi_tasks SET statut='en_attente', locked_at=NULL WHERE statut='en_cours';",
    );
    let _ = conn.execute_batch(
        "PRAGMA wal_checkpoint(TRUNCATE);
         PRAGMA analysis_limit = 1000;
         PRAGMA optimize;",
    );
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
            mix_url       TEXT,
            tech_rider    TEXT,
            fee_range     TEXT,
            stats         TEXT,
            audience_cities TEXT,
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

        -- ===================================================================
        -- Venue Intelligence (module vi_*). Tables prefixed vi_ to isolate the
        -- module. INTEGER PK + unique slug instead of uuid (repo convention),
        -- enums stored as TEXT, JSON stored as TEXT, timestamps as TEXT ISO.
        -- French column names kept from the spec (snake_case, repo convention).
        -- ===================================================================
        CREATE TABLE IF NOT EXISTS vi_venues (
            id                INTEGER PRIMARY KEY AUTOINCREMENT,
            slug              TEXT UNIQUE,
            nom               TEXT NOT NULL,
            nom_normalise     TEXT NOT NULL,
            ville             TEXT,
            pays              TEXT,
            region_cible      TEXT,
            type              TEXT,
            capacite_est      INTEGER,
            latitude          REAL,
            longitude         REAL,
            adresse           TEXT,
            site_web          TEXT,
            page_contact      TEXT,
            instagram         TEXT,
            ra_venue_id       INTEGER UNIQUE,
            ra_url            TEXT,
            saisonnalite      TEXT,
            segment           TEXT,
            accepte_demos     TEXT NOT NULL DEFAULT 'inconnu',
            statut            TEXT NOT NULL DEFAULT 'candidat',
            priorite          TEXT NOT NULL DEFAULT 'C',
            score_qualif      INTEGER NOT NULL DEFAULT 0,
            nb_events_periode INTEGER NOT NULL DEFAULT 0,
            notes             TEXT,
            crm_contact_id    INTEGER REFERENCES contacts(id) ON DELETE SET NULL,
            verified_at       TEXT,
            created_at        TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at        TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS vi_evidence (
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            venue_id     INTEGER NOT NULL REFERENCES vi_venues(id) ON DELETE CASCADE,
            artiste      TEXT NOT NULL,
            artiste_tier INTEGER,
            date_event   TEXT NOT NULL,
            titre_event  TEXT,
            source_url   TEXT NOT NULL,
            source_type  TEXT NOT NULL DEFAULT 'ra',
            created_at   TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(venue_id, artiste, date_event)
        );

        CREATE TABLE IF NOT EXISTS vi_contacts (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            venue_id      INTEGER NOT NULL REFERENCES vi_venues(id) ON DELETE CASCADE,
            type          TEXT,
            valeur        TEXT NOT NULL,
            role_devine   TEXT,
            score         INTEGER NOT NULL DEFAULT 0,
            personne_nom  TEXT,
            personne_role TEXT,
            source_url    TEXT NOT NULL,
            source_method TEXT,
            confiance     REAL,
            verifie       INTEGER NOT NULL DEFAULT 0,
            created_at    TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at    TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(venue_id, type, valeur)
        );

        CREATE TABLE IF NOT EXISTS vi_promoters (
            id       INTEGER PRIMARY KEY AUTOINCREMENT,
            venue_id INTEGER NOT NULL REFERENCES vi_venues(id) ON DELETE CASCADE,
            nom      TEXT NOT NULL,
            ra_url   TEXT,
            nb_events INTEGER NOT NULL DEFAULT 0,
            UNIQUE(venue_id, nom)
        );

        CREATE TABLE IF NOT EXISTS vi_runs (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            type        TEXT NOT NULL,
            params      TEXT,
            statut      TEXT NOT NULL DEFAULT 'en_attente',
            started_at  TEXT,
            finished_at TEXT,
            stats       TEXT,
            erreur      TEXT,
            created_by  TEXT,
            created_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS vi_tasks (
            id             INTEGER PRIMARY KEY AUTOINCREMENT,
            run_id         INTEGER NOT NULL REFERENCES vi_runs(id) ON DELETE CASCADE,
            type           TEXT NOT NULL,
            payload        TEXT,
            statut         TEXT NOT NULL DEFAULT 'en_attente',
            tentatives     INTEGER NOT NULL DEFAULT 0,
            max_tentatives INTEGER NOT NULL DEFAULT 3,
            derniere_erreur TEXT,
            locked_at      TEXT,
            created_at     TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at     TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS radar_curators (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            source        TEXT NOT NULL,           -- spotify | deezer
            external_id   TEXT NOT NULL,
            kind          TEXT NOT NULL DEFAULT 'playlist', -- playlist | label | ar | publisher
            nom           TEXT NOT NULL,
            nom_normalise TEXT NOT NULL,
            owner_name    TEXT,
            url           TEXT,
            followers     INTEGER,
            nb_tracks     INTEGER,
            genre         TEXT,
            pays          TEXT,
            description   TEXT,
            site_web      TEXT,
            statut        TEXT NOT NULL DEFAULT 'candidat',
            score         INTEGER NOT NULL DEFAULT 0,
            editorial     INTEGER NOT NULL DEFAULT 0, -- 1 = official/editorial (no email expected)
            enriched_at   TEXT,
            crm_contact_id INTEGER REFERENCES contacts(id) ON DELETE SET NULL,
            created_at    TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at    TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(source, external_id)
        );

        CREATE TABLE IF NOT EXISTS radar_contacts (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            curator_id    INTEGER NOT NULL REFERENCES radar_curators(id) ON DELETE CASCADE,
            type          TEXT,
            valeur        TEXT NOT NULL,
            role_devine   TEXT,
            score         INTEGER NOT NULL DEFAULT 0,
            source_url    TEXT,
            source_method TEXT,
            verifie       INTEGER NOT NULL DEFAULT 0,
            created_at    TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(curator_id, type, valeur)
        );

        CREATE INDEX IF NOT EXISTS idx_radar_statut ON radar_curators(statut);
        CREATE INDEX IF NOT EXISTS idx_radar_norm ON radar_curators(nom_normalise);
        CREATE INDEX IF NOT EXISTS idx_radar_contacts ON radar_contacts(curator_id);

        CREATE TABLE IF NOT EXISTS pulse_tracked_tracks (
            id               INTEGER PRIMARY KEY AUTOINCREMENT,
            artist_id        INTEGER REFERENCES artists(id) ON DELETE SET NULL,
            track_spotify_id TEXT NOT NULL UNIQUE,
            name             TEXT,
            release_date     TEXT,
            is_active        INTEGER NOT NULL DEFAULT 1,
            created_at       TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS pulse_artist_snapshots (
            id                INTEGER PRIMARY KEY AUTOINCREMENT,
            artist_spotify_id TEXT NOT NULL,
            snapshot_date     TEXT NOT NULL,
            popularity        INTEGER,
            followers         INTEGER,
            monthly_listeners INTEGER,
            created_at        TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(artist_spotify_id, snapshot_date)
        );

        CREATE TABLE IF NOT EXISTS pulse_track_snapshots (
            id               INTEGER PRIMARY KEY AUTOINCREMENT,
            track_spotify_id TEXT NOT NULL,
            track_name       TEXT,
            snapshot_date    TEXT NOT NULL,
            popularity       INTEGER,
            created_at       TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(track_spotify_id, snapshot_date)
        );

        CREATE TABLE IF NOT EXISTS pulse_playlist_watchlist (
            id                  INTEGER PRIMARY KEY AUTOINCREMENT,
            playlist_spotify_id TEXT NOT NULL UNIQUE,
            name                TEXT,
            owner_name          TEXT,
            notes               TEXT,
            is_active           INTEGER NOT NULL DEFAULT 1,
            created_at          TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS pulse_playlist_snapshots (
            id                 INTEGER PRIMARY KEY AUTOINCREMENT,
            playlist_id        INTEGER NOT NULL REFERENCES pulse_playlist_watchlist(id) ON DELETE CASCADE,
            snapshot_date      TEXT NOT NULL,
            followers          INTEGER,
            track_count        INTEGER,
            contains_our_track INTEGER NOT NULL DEFAULT 0,
            created_at         TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(playlist_id, snapshot_date)
        );

        CREATE TABLE IF NOT EXISTS pulse_meta_spend (
            id              INTEGER PRIMARY KEY AUTOINCREMENT,
            campaign_id     TEXT NOT NULL,
            campaign_name   TEXT,
            spend_date      TEXT NOT NULL,
            spend           REAL NOT NULL DEFAULT 0,
            impressions     INTEGER,
            clicks          INTEGER,
            results         INTEGER,
            cost_per_result REAL,
            created_at      TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(campaign_id, spend_date)
        );

        CREATE INDEX IF NOT EXISTS idx_pulse_track_snap ON pulse_track_snapshots(track_spotify_id, snapshot_date);
        CREATE INDEX IF NOT EXISTS idx_pulse_spend_date ON pulse_meta_spend(spend_date);

        CREATE TABLE IF NOT EXISTS vi_ra_areas (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            slug        TEXT UNIQUE,
            ra_area_id  INTEGER,
            libelle     TEXT,
            pays        TEXT,
            actif       INTEGER NOT NULL DEFAULT 1,
            resolved_at TEXT
        );

        CREATE TABLE IF NOT EXISTS vi_reference_artists (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            nom           TEXT NOT NULL,
            nom_normalise TEXT NOT NULL UNIQUE,
            tier          INTEGER NOT NULL,
            genres        TEXT,
            actif         INTEGER NOT NULL DEFAULT 1
        );

        -- RGPD: contacts explicitly erased land here so a later run never re-adds them.
        CREATE TABLE IF NOT EXISTS vi_exclusions (
            id                INTEGER PRIMARY KEY AUTOINCREMENT,
            valeur_normalisee TEXT NOT NULL UNIQUE,
            type              TEXT NOT NULL DEFAULT 'email',
            raison            TEXT,
            created_at        TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_vi_venues_statut ON vi_venues(statut);
        CREATE INDEX IF NOT EXISTS idx_vi_venues_norm   ON vi_venues(nom_normalise);
        CREATE INDEX IF NOT EXISTS idx_vi_venues_ra     ON vi_venues(ra_venue_id);
        CREATE INDEX IF NOT EXISTS idx_vi_evidence_venue ON vi_evidence(venue_id);
        CREATE INDEX IF NOT EXISTS idx_vi_contacts_venue ON vi_contacts(venue_id);
        CREATE INDEX IF NOT EXISTS idx_vi_promoters_venue ON vi_promoters(venue_id);
        CREATE INDEX IF NOT EXISTS idx_vi_tasks_run_statut ON vi_tasks(run_id, statut);
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
        "ALTER TABLE vi_venues ADD COLUMN telephone TEXT",
        "ALTER TABLE vi_venues ADD COLUMN enriched_at TEXT",
        "ALTER TABLE artists ADD COLUMN mix_url TEXT",
        "ALTER TABLE artists ADD COLUMN tech_rider TEXT",
        "ALTER TABLE artists ADD COLUMN fee_range TEXT",
        "ALTER TABLE artists ADD COLUMN stats TEXT",
        "ALTER TABLE artists ADD COLUMN audience_cities TEXT",
        "ALTER TABLE artists ADD COLUMN spotify_artist_id TEXT",
        "ALTER TABLE contacts ADD COLUMN followup_dismissed INTEGER NOT NULL DEFAULT 0",
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

    // ---- Performance indexes (see the comment on each group) ----
    //
    // The venue list and its stat bar cross-reference vi_venues against the CRM
    // by comparing lower(trim(...)) of the names on both sides. Without an index
    // on that exact expression SQLite has no choice but to rescan the whole
    // contacts table once per venue: at 26 870 venues x 3 128 contacts the
    // "contacted" counter alone measured 75 s of pure CPU, which is what froze
    // the window when the page opened. SQLite can index an expression built from
    // deterministic built-ins, so indexing lower(trim(col)) turns each of those
    // rescans into a single lookup (75 s -> 0.05 s).
    for stmt in [
        "CREATE INDEX IF NOT EXISTS idx_contacts_venue_lc ON contacts(lower(trim(venue)))",
        "CREATE INDEX IF NOT EXISTS idx_contacts_name_lc  ON contacts(lower(trim(name)))",
        "CREATE INDEX IF NOT EXISTS idx_events_venue_lc   ON events(lower(trim(venue)))",
        // Matches vi_list_venues' ORDER BY exactly. Beyond skipping the sort,
        // this lets SQLite stop reading once LIMIT rows are out, so the six
        // correlated subqueries in the SELECT list are evaluated for one page of
        // results instead of every matching venue (4.8 s -> 0.014 s).
        "CREATE INDEX IF NOT EXISTS idx_vi_venues_sort ON vi_venues(statut, score_qualif DESC, nb_events_periode DESC)",
        "CREATE INDEX IF NOT EXISTS idx_vi_venues_pays ON vi_venues(pays)",
        "CREATE INDEX IF NOT EXISTS idx_vi_venues_crm  ON vi_venues(crm_contact_id)",
        // Cover the per-row lookups the venue list and fiche do: best email by
        // score, top promoter by event count, distinct artists for scoring.
        "CREATE INDEX IF NOT EXISTS idx_vi_contacts_vt   ON vi_contacts(venue_id, type, score DESC)",
        "CREATE INDEX IF NOT EXISTS idx_vi_promoters_vn  ON vi_promoters(venue_id, nb_events DESC)",
        "CREATE INDEX IF NOT EXISTS idx_vi_evidence_va   ON vi_evidence(venue_id, artiste)",
        // Same shape of correlated EXISTS on the Radar page.
        "CREATE INDEX IF NOT EXISTS idx_radar_contacts_ct ON radar_contacts(curator_id, type)",
        // Radar's "address shared by several curators" check looks addresses up by value.
        "CREATE INDEX IF NOT EXISTS idx_radar_contacts_val ON radar_contacts(valeur, curator_id)",
        // Contacts list filters on these two before ordering.
        "CREATE INDEX IF NOT EXISTS idx_contacts_cat_status ON contacts(category, status)",
    ] {
        let _ = conn.execute(stmt, []);
    }

    // ---- Outbox: sending guardrails, reply detection, campaign audience ----
    //
    // `message_id` is the Message-ID header set at send time, which is what an
    // IMAP reply's In-Reply-To / References point back to. `replied_at` is the
    // first detected reply to that email. `suppressions` holds addresses that
    // must never be mailed again (hard bounce, unsubscribe, refusal, by hand),
    // stored trimmed + lowercased so the UNIQUE constraint dedupes them.
    // `email_replies` keeps one row per inbound reply (deduped on its own
    // Message-ID, so re-scanning the inbox is harmless). `campaign_contacts` is a
    // campaign's planned audience, with the state of each recipient. All of it is
    // additive and idempotent: safe to run on every launch.
    for stmt in [
        "ALTER TABLE emails ADD COLUMN message_id TEXT",
        "ALTER TABLE emails ADD COLUMN replied_at TEXT",
        "CREATE TABLE IF NOT EXISTS suppressions (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            address    TEXT NOT NULL UNIQUE,
            reason     TEXT NOT NULL DEFAULT 'manual',
            note       TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        )",
        "CREATE TABLE IF NOT EXISTS email_replies (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            email_id    INTEGER REFERENCES emails(id) ON DELETE SET NULL,
            contact_id  INTEGER REFERENCES contacts(id) ON DELETE SET NULL,
            from_addr   TEXT NOT NULL,
            subject     TEXT,
            message_id  TEXT UNIQUE,
            kind        TEXT NOT NULL DEFAULT 'reply',
            matched_by  TEXT,
            received_at TEXT,
            created_at  TEXT NOT NULL DEFAULT (datetime('now'))
        )",
        "CREATE TABLE IF NOT EXISTS campaign_contacts (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            campaign_id INTEGER NOT NULL REFERENCES campaigns(id) ON DELETE CASCADE,
            contact_id  INTEGER NOT NULL REFERENCES contacts(id) ON DELETE CASCADE,
            state       TEXT NOT NULL DEFAULT 'planned',
            added_at    TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at  TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE (campaign_id, contact_id)
        )",
        "CREATE INDEX IF NOT EXISTS idx_emails_message_id ON emails(message_id)",
        "CREATE INDEX IF NOT EXISTS idx_emails_sent_at    ON emails(sent_at)",
        // Address matching (follow-ups, replies, quota) compares normalised
        // addresses on both sides.
        "CREATE INDEX IF NOT EXISTS idx_emails_to_lc      ON emails(lower(trim(to_addr)))",
        "CREATE INDEX IF NOT EXISTS idx_contacts_email_lc ON contacts(lower(trim(email)))",
        "CREATE INDEX IF NOT EXISTS idx_email_replies_email ON email_replies(email_id)",
        "CREATE INDEX IF NOT EXISTS idx_campaign_contacts_contact ON campaign_contacts(contact_id)",
        // Campaigns sent before the audience table existed: their recipients
        // become 'sent' rows, so the card counts are right from day one.
        "INSERT OR IGNORE INTO campaign_contacts (campaign_id, contact_id, state, added_at)
         SELECT e.campaign_id, e.contact_id, 'sent', MIN(e.sent_at)
         FROM emails e
         JOIN campaigns c ON c.id = e.campaign_id
         JOIN contacts k ON k.id = e.contact_id
         WHERE e.status = 'sent'
         GROUP BY e.campaign_id, e.contact_id",
    ] {
        let _ = conn.execute(stmt, []);
    }

    // Labels used to be stored under the source name 'musicbrainz' while the UI
    // called that source 'labels', so filtering on it never matched anything.
    // Align the existing rows with the name the app uses everywhere else.
    let _ = conn.execute(
        "UPDATE radar_curators SET source='labels' WHERE source='musicbrainz'",
        [],
    );

    // The curator score was computed as a REAL and stored as such in an INTEGER
    // column (SQLite keeps the expression's type). Readers that expect an integer
    // error out on those rows. Normalise the ones already written.
    let _ = conn.execute(
        "UPDATE radar_curators SET score = CAST(score AS INTEGER) WHERE typeof(score) != 'integer'",
        [],
    );

    // Fresh indexes are invisible to the planner until it has statistics for
    // them, and this database had never been ANALYZEd. Run it once, recorded in
    // settings so later launches fall through to the cheap PRAGMA optimize.
    let analyzed: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM settings WHERE key='db_analyze_v1'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(1);
    if analyzed == 0 {
        let _ = conn.execute_batch("ANALYZE");
        let _ = conn.execute(
            "INSERT OR REPLACE INTO settings (key, value) VALUES ('db_analyze_v1', datetime('now'))",
            [],
        );
    }

    // Patch existing Venue Intelligence areas: fix the Côte d'Azur slug and fill in
    // area ids resolved after those rows were first seeded (only touches NULLs).
    let _ = conn.execute(
        "UPDATE vi_ra_areas SET slug='fr/nice', libelle='Nice' WHERE slug='fr/cotedazur'",
        [],
    );
    for (slug, libelle, id) in VI_AREAS {
        if let Some(area_id) = id {
            let _ = conn.execute(
                "UPDATE vi_ra_areas SET ra_area_id=?2, resolved_at=datetime('now'),
                    libelle=COALESCE(libelle, ?3)
                 WHERE slug=?1 AND ra_area_id IS NULL",
                rusqlite::params![slug, area_id, libelle],
            );
        }
    }

    // The initial planning sheet was ingested with its header row as data:
    // a budget line "Item / Notes", a KPI "Goal / Core KPI" and a task
    // "Period / Actions" showed as real entries. Exact-match only, so nothing a
    // user typed can be caught.
    let _ = conn.execute(
        "DELETE FROM budget_items WHERE item='Item' AND notes='Notes'
           AND COALESCE(min_cost,0)=0 AND COALESCE(max_cost,0)=0 AND actual IS NULL",
        [],
    );
    let _ = conn.execute(
        "DELETE FROM kpis WHERE goal='Goal' AND kpi='Core KPI'
           AND target='Realistic target' AND actual='Actual'",
        [],
    );
    let _ = conn.execute(
        "DELETE FROM tasks WHERE period='Period' AND title='Actions' AND owner='Owner'",
        [],
    );

    // ---- Module links (audit 2026-10-29 items 5, 7, 9, 10, 15) ----
    // Organisations, contacts -> artist, budget lines -> events, visa dossiers
    // -> events, free-text dates -> ISO. Idempotent, see crm_links.rs.
    crate::crm_links::migrate(conn);

    // ---- Audience tracks -> artist (audit item 11) ----
    // Tracks added before the artist picker existed have no artist. With a
    // single artist they can only be theirs; with several, left for the user.
    let _ = conn.execute(
        "UPDATE pulse_tracked_tracks SET artist_id=(SELECT id FROM artists)
         WHERE artist_id IS NULL AND (SELECT COUNT(*) FROM artists)=1",
        [],
    );

    // ---- Venue Intelligence / Radar data quality (audit 2026-10-29 items 1, 12, 14, 18) ----
    // vi_venue_events (idempotent event counts), vi_venues.ville_norm (accent-
    // folded city for artist targeting), YouTube curators as kind 'channel',
    // one-shot cleanup of scraping-artefact addresses. See commands/vi.rs.
    crate::commands::vi::migrate_quality(conn);

    // ---- Promoters as targets, fit score, booking timing ----
    // vi_promoter_orgs (one identity per promoter across rooms), lineups and
    // promoters per RA event, artist_comparables, fit / lead-time columns.
    // Additive, see commands/promoters.rs and commands/fit.rs.
    crate::commands::promoters::migrate(conn);

    // ---- Residency campaigns (autopilot, reply intelligence, EPK activity, reports) ----
    // campaigns.kind, residency_campaigns, campaign_messages, reply_insights,
    // press_activity, campaign_reports, residency_log, ai_usage and
    // email_replies.body_excerpt. Additive and idempotent, see
    // commands/residency/mod.rs.
    crate::commands::residency::migrate(conn);
    crate::commands::residency::refresh_default_templates(conn);

    // ---- Gigs published on insrt.studio (audit item 19) ----
    // insrt_gigs: per-event "publier sur insrt" switch, ticket link and last
    // push. Additive, see commands/insrt_gigs.rs.
    crate::commands::insrt_gigs::migrate(conn);

    // ---- Tour routing, rebook loop, played at ----
    // gig_recaps (the night's assets, "a joué à" switch) and gig_followups
    // (thank-you / rebook / routing lines handled). Additive, see
    // commands/routing.rs.
    crate::commands::routing::migrate(conn);
    // ---- Réponses à traiter (reply desk) ----
    // reply_desk: replies awaiting Thibault and stalled threads, with their
    // drafts. Additive, see commands/reply_desk.rs.
    crate::commands::reply_desk::migrate(conn);
    // ---- Deliverability (sending budget, sender health) ----
    // emails.from_addr: the sending address of each email. Additive, see
    // commands/deliverability/mod.rs.
    crate::commands::deliverability::migrate(conn);
    // ---- Funnel, subject A/B tests, Monday digest ----
    // emails.subject_variant / template_id / step, campaign_messages.subject_variant,
    // ab_subjects, weekly_digests. Additive, see commands/funnel.rs.
    crate::commands::funnel::migrate(conn);
    // ---- Offers, documents, payments, advancing (commands/offers) ----
    // offers, invoices, advancing. Additive and idempotent.
    crate::commands::offers::migrate(conn);
    // ---- iPhone notifications (Pushover) ----
    // push_log: every notice once (dedupe + queue). Additive, see
    // commands/push.rs.
    crate::commands::push::migrate(conn);
    // ---- Agent library + graduated autonomy ----
    // playbook_entries, autosends, reply_desk / campaign_messages columns.
    // Additive, see commands/playbook.rs and commands/autonomy.rs.
    crate::commands::playbook::migrate(conn);
    crate::commands::autonomy::migrate(conn);

    Ok(())
}

#[cfg(test)]
pub(crate) fn migrate_for_tests(conn: &rusqlite::Connection) -> Result<(), String> {
    migrate(conn)
}

/// Vetted pack of professional booking templates, kept available on every launch.
/// Inserted by name only when absent, so user edits are never overwritten and
/// existing installs receive the pack too. Written to industry standards for
/// artists in development: short (the "4-line" formula), one link, honest and
/// humble tone, fit-first, plus the no-fee showcase that unlocks first dates.
/// Multi-artist via {{artist}}; the recipient's promoter name is {{name}}.
fn seed_defaults(conn: &rusqlite::Connection) -> Result<(), String> {
    const PACK: &[(&str, &str, &str)] = &[
        (
            "Prise de contact",
            "{{artist}} · booking {{venue}}",
            "Hi {{name}},\n\nI look after bookings at Insrt, and I'm writing about {{artist}}, a house / tech-house artist we work with. I'm reaching out to {{venue}} in particular because your programming genuinely fits their sound, this isn't a mass mailout.\n\nHere's a recent mix so you can judge the fit for yourself: {{mix}}\n\nThey'd love to play, even an early or opening slot, and we bring our own crowd. If it could work I'll gladly send a full EPK and past dates.\n\nThanks for your time,",
        ),
        (
            "Showcase (sans cachet)",
            "showcase idea for {{venue}}",
            "Hi {{name}},\n\nA quick idea for {{venue}}: a small free-entry showcase with {{artist}} and a couple of friends, early evening, house / tech-house. We bring our own crowd, you keep the bar, no fee either way. We just want a good room and a proper sound system.\n\nHere's {{artist}}'s latest mix so you get the sound: {{mix}}\n\nIf it's worth a chat I'll send over a full plan, times, artists and references. And if the timing is off, no worries at all, I'd love to stay on your radar.\n\nCheers,",
        ),
        (
            "Relance",
            "re: {{artist}} at {{venue}}",
            "Hi {{name}},\n\nJust a short follow-up on my note about {{artist}} at {{venue}}, I know the inbox gets busy.\n\nSince I wrote: {{news}}. Still keen if a slot could work, even a small or early one.\n\nNo pressure at all, and thanks for reading.\n\nBest,",
        ),
    ];
    for (name, subject, body) in PACK {
        conn.execute(
            "INSERT INTO templates (name, subject, body)
             SELECT ?1, ?2, ?3 WHERE NOT EXISTS (SELECT 1 FROM templates WHERE name = ?1)",
            rusqlite::params![name, subject, body],
        )
        .map_err(|e| e.to_string())?;
    }

    seed_visa_countries(conn)?;
    seed_venue_intelligence(conn)?;
    Ok(())
}

/// Seed the Venue Intelligence reference data on first run only:
/// the reference-artist list (tiers 1/2/3, editable later from the UI) and the
/// target RA areas (ra_area_id left NULL, resolved during harvest or by hand).
fn seed_venue_intelligence(conn: &rusqlite::Connection) -> Result<(), String> {
    // ---- Reference artists ----
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM vi_reference_artists", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if count == 0 {
        let tier1 = [
            "Black Coffee", "Solomun", "Jamie Jones", "Marco Carola", "Fisher", "Peggy Gou",
            "The Martinez Brothers", "Loco Dice", "Seth Troxler", "Michael Bibi", "Chris Stussy",
            "Dennis Ferrer", "Carl Cox", "Adam Beyer", "Eric Prydz", "Maceo Plex", "Dixon",
            "Keinemusik", "&ME", "Rampa", "Adam Port", "Damian Lazarus", "Hot Since 82",
            "Joseph Capriati", "Richy Ahmed", "Patrick Topping", "Archie Hamilton",
            "East End Dubs", "Enzo Siragusa", "Sidney Charles", "Rossi.", "Prunk", "Toman",
            "Mall Grab", "DJ Boring", "Folamour", "DJ Seinfeld", "Ross From Friends",
            "Honey Dijon", "The Blessed Madonna", "Kerri Chandler", "Louie Vega",
            "Masters At Work", "Todd Terry", "MK", "Gorgon City", "Sonny Fodera", "Cloonee",
            "Dom Dolla", "John Summit", "Chris Lake", "Vintage Culture", "Meduza", "HUGEL",
            "Purple Disco Machine", "Claptone", "Bedouin", "Acid Pauli", "Satori", "Monolink",
            "Wehbba",
        ];
        let tier2 = [
            "Apollonia", "Dyed Soundorom", "Dan Ghenacia", "Shonky", "Traumer", "Cassy",
            "Nicolas Lutz", "Sonja Moonear", "Raresh", "Petre Inspirescu", "Rhadoo", "Praslea",
            "Zip", "Ricardo Villalobos", "Binh", "Vera", "Yulia Niko", "ANOTR", "Kolter",
            "Adiel", "Deborah De Luca", "Anfisa Letyago", "Amémé", "Salomé", "Trikk",
            "Mind Against", "Tale Of Us", "Agents Of Time", "Adriatique", "Argy", "Colyn",
            "Massano", "Kevin de Vries", "Innellea", "Mathame",
        ];
        let tier3 = [
            "Moodymann", "Theo Parrish", "Gerd Janson", "Motor City Drum Ensemble", "Palms Trax",
            "Young Marco", "Hunee", "Antal", "Job Jobse", "Jane Fitz", "Eris Drew", "Octo Octa",
            "Roman Flügel", "DJ Harvey", "Leon Vynehall", "Floating Points", "Laurent Garnier",
            "D'Julz", "Chloé", "Molly", "Clara 3000", "Kiddy Smile",
        ];
        let mut stmt = conn
            .prepare(
                "INSERT OR IGNORE INTO vi_reference_artists (nom, nom_normalise, tier) VALUES (?1, ?2, ?3)",
            )
            .map_err(|e| e.to_string())?;
        for (tier, list) in [(1i64, &tier1[..]), (2, &tier2[..]), (3, &tier3[..])] {
            for nom in list {
                stmt.execute(rusqlite::params![nom, normalise(nom), tier])
                    .map_err(|e| e.to_string())?;
            }
        }
    }

    // Extra reference artists requested later (added on every run, harmless if present).
    for (nom, tier) in EXTRA_REF_ARTISTS {
        let _ = conn.execute(
            "INSERT OR IGNORE INTO vi_reference_artists (nom, nom_normalise, tier) VALUES (?1, ?2, ?3)",
            rusqlite::params![nom, normalise(nom), tier],
        );
    }

    // ---- RA areas (target zones), area ids pre-resolved via the RA API. ----
    // Idempotent by slug (UNIQUE): existing rows are preserved, new areas (the
    // Europe-wide expansion) are added on every launch, so existing installs get
    // them too without wiping the user's manual edits.
    {
        let mut stmt = conn
            .prepare(
                "INSERT OR IGNORE INTO vi_ra_areas (slug, pays, libelle, ra_area_id, resolved_at)
                 VALUES (?1, ?2, ?3, ?4, CASE WHEN ?4 IS NOT NULL THEN datetime('now') END)",
            )
            .map_err(|e| e.to_string())?;
        for (slug, libelle, id) in VI_AREAS {
            let pays = slug.split_once('/').map(|(c, _)| c.to_uppercase()).unwrap_or_default();
            stmt.execute(rusqlite::params![slug, pays, libelle, id])
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Requested house / tech-house names (Cloonee already in tier 1). Users add more
/// from the "Artistes de référence" tab.
const EXTRA_REF_ARTISTS: &[(&str, i64)] = &[("Franky Rizardo", 2), ("Mason Collective", 2)];

/// Target RA areas with their resolved area id (None when RA has no such area,
/// resolve or enter by hand in the app). Resolved once via the RA `area` query.
const VI_AREAS: &[(&str, &str, Option<i64>)] = &[
    ("fr/paris", "Paris", Some(44)),
    ("fr/lyon", "Lyon", Some(337)),
    ("fr/marseille", "Marseille", Some(338)),
    ("fr/bordeaux", "Bordeaux", Some(617)),
    ("fr/nantes", "Nantes", Some(536)),
    ("fr/lille", "Lille", Some(619)),
    ("fr/toulouse", "Toulouse", Some(618)),
    ("fr/nice", "Nice", Some(614)),
    ("be/brussels", "Brussels", Some(405)),
    ("be/ghent", "Ghent", Some(545)),
    ("be/antwerp", "Antwerp", Some(404)),
    ("ch/zurich", "Zurich", Some(390)),
    ("ch/geneva", "Geneva", Some(392)),
    ("ch/lausanne", "Lausanne", Some(393)),
    ("ch/basel", "Basel", Some(391)),
    ("es/barcelona", "Barcelona", Some(20)),
    ("es/madrid", "Madrid", Some(41)),
    ("es/valencia", "Valencia", Some(607)),
    ("es/ibiza", "Ibiza", Some(25)),
    ("es/mallorca", "Mallorca", Some(661)),
    ("es/malaga", "Malaga", Some(608)),
    ("it/milan", "Milan", Some(347)),
    ("it/rome", "Rome", Some(351)),
    ("it/florence", "Florence", Some(352)),
    ("it/naples", "Naples", Some(406)),
    ("it/sardinia", "Sardinia", Some(673)),
    ("it/bologna", "Bologna", Some(350)),
    ("it/turin", "Turin", Some(348)),
    ("it/riminiravenna", "Rimini Ravenna", None),
    ("gr/athens", "Athens", Some(549)),
    ("gr/mykonos", "Mykonos", Some(659)),
    ("gr/thessaloniki", "Thessaloniki", Some(657)),
    ("gr/crete", "Crete", Some(658)),
    ("gr/santorini", "Santorini", None),
    // ---- Europe-wide expansion (RA area ids resolved live) ----
    ("nl/amsterdam", "Amsterdam", Some(29)),
    ("nl/rotterdam", "Rotterdam", Some(174)),
    ("nl/utrecht", "Utrecht", Some(175)),
    ("nl/thehague", "The Hague", Some(178)),
    ("nl/eindhoven", "Eindhoven", Some(177)),
    ("de/berlin", "Berlin", Some(34)),
    ("de/cologne", "Cologne", Some(143)),
    ("de/frankfurt", "Frankfurt", Some(147)),
    ("de/hamburg", "Hamburg", Some(148)),
    ("de/munich", "Munich", Some(151)),
    ("de/leipzig", "Leipzig", Some(149)),
    ("de/stuttgart", "Stuttgart", Some(152)),
    ("de/dusseldorf", "Dusseldorf", Some(145)),
    ("de/mannheim", "Mannheim", Some(153)),
    ("uk/london", "London", Some(13)),
    ("uk/manchester", "Manchester", Some(344)),
    ("uk/glasgow", "Glasgow", Some(340)),
    ("uk/leeds", "Leeds", Some(346)),
    ("uk/bristol", "Bristol", Some(446)),
    ("uk/birmingham", "Birmingham", Some(516)),
    ("uk/liverpool", "Liverpool", Some(343)),
    ("uk/sheffield", "Sheffield", Some(520)),
    ("uk/brighton", "Brighton", Some(535)),
    ("uk/edinburgh", "Edinburgh", Some(341)),
    ("uk/newcastle", "Newcastle", Some(345)),
    ("uk/nottingham", "Nottingham", Some(544)),
    ("pt/lisbon", "Lisbon", Some(53)),
    ("at/vienna", "Vienna", Some(450)),
    ("ie/dublin", "Dublin", Some(386)),
    ("se/stockholm", "Stockholm", Some(396)),
    ("dk/copenhagen", "Copenhagen", Some(402)),
    ("no/oslo", "Oslo", Some(408)),
    ("fi/helsinki", "Helsinki", Some(407)),
    ("pl/warsaw", "Warsaw", Some(454)),
    ("pl/krakow", "Krakow", Some(455)),
    ("pl/wroclaw", "Wroclaw", Some(668)),
    ("cz/prague", "Prague", Some(451)),
    ("hu/budapest", "Budapest", Some(449)),
    ("ro/bucharest", "Bucharest", Some(381)),
    ("hr/zagreb", "Zagreb", Some(559)),
    ("rs/belgrade", "Belgrade", Some(562)),
    ("si/ljubljana", "Ljubljana", Some(563)),
    ("sk/bratislava", "Bratislava", Some(568)),
    ("bg/sofia", "Sofia", Some(558)),
    ("ee/tallinn", "Tallinn", Some(566)),
    ("lv/riga", "Riga", Some(560)),
    ("lt/vilnius", "Vilnius", Some(561)),
    ("lu/luxembourg", "Luxembourg", Some(199)),
    ("es/seville", "Seville", None),
    ("es/bilbao", "Bilbao", Some(612)),
    ("fr/strasbourg", "Strasbourg", Some(615)),
    ("fr/montpellier", "Montpellier", Some(339)),
    ("fr/rennes", "Rennes", Some(620)),
];

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
