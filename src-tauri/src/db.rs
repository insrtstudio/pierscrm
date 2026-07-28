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

    Ok(())
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
    let acount: i64 = conn
        .query_row("SELECT COUNT(*) FROM vi_ra_areas", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if acount == 0 {
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
