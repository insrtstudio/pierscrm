use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct Contact {
    #[serde(default)]
    pub id: Option<i64>,
    #[serde(default)]
    pub artist_id: Option<i64>,
    #[serde(default = "default_category")]
    pub category: String,
    pub priority: Option<String>,
    pub name: String,
    pub promoter: Option<String>,
    pub venue: Option<String>,
    #[serde(rename = "type")]
    pub type_: Option<String>,
    pub area: Option<String>,
    pub scale: Option<String>,
    pub date: Option<String>,
    pub time: Option<String>,
    pub format: Option<String>,
    pub reason: Option<String>,
    pub contact_channel: Option<String>,
    pub email: Option<String>,
    pub email_status: Option<String>,
    #[serde(default = "default_status")]
    pub status: String,
    pub first_contact: Option<String>,
    pub follow_up: Option<String>,
    pub notes: Option<String>,
    pub website: Option<String>,
    pub tags: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

fn default_category() -> String {
    "venue".into()
}
fn default_status() -> String {
    "to_contact".into()
}

impl Contact {
    pub fn from_row(row: &rusqlite::Row) -> rusqlite::Result<Self> {
        Ok(Contact {
            id: row.get("id")?,
            artist_id: row.get("artist_id")?,
            category: row.get("category")?,
            priority: row.get("priority")?,
            name: row.get("name")?,
            promoter: row.get("promoter")?,
            venue: row.get("venue")?,
            type_: row.get("type")?,
            area: row.get("area")?,
            scale: row.get("scale")?,
            date: row.get("date")?,
            time: row.get("time")?,
            format: row.get("format")?,
            reason: row.get("reason")?,
            contact_channel: row.get("contact_channel")?,
            email: row.get("email")?,
            email_status: row.get("email_status")?,
            status: row.get("status")?,
            first_contact: row.get("first_contact")?,
            follow_up: row.get("follow_up")?,
            notes: row.get("notes")?,
            website: row.get("website")?,
            tags: row.get("tags")?,
            created_at: row.get("created_at")?,
            updated_at: row.get("updated_at")?,
        })
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct Template {
    pub id: Option<i64>,
    pub name: String,
    pub subject: String,
    pub body: String,
    #[serde(default)]
    pub created_at: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct EmailLog {
    pub id: Option<i64>,
    pub contact_id: Option<i64>,
    #[serde(default)]
    pub campaign_id: Option<i64>,
    pub to_addr: String,
    pub subject: String,
    pub body: String,
    pub status: String,
    pub error: Option<String>,
    pub track_token: Option<String>,
    pub opened_at: Option<String>,
    #[serde(default)]
    pub open_count: i64,
    pub sent_at: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct Campaign {
    pub id: Option<i64>,
    pub name: String,
    pub purpose: Option<String>,
    pub event_name: Option<String>,
    #[serde(default)]
    pub artist_id: Option<i64>,
    pub target_date: Option<String>,
    #[serde(default = "default_campaign_status")]
    pub status: String,
    pub color: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    // Derived stats (populated by list_campaigns)
    #[serde(default)]
    pub sent_count: i64,
    #[serde(default)]
    pub opened_count: i64,
}

fn default_campaign_status() -> String {
    "active".into()
}

impl Campaign {
    pub fn from_row(row: &rusqlite::Row) -> rusqlite::Result<Self> {
        Ok(Campaign {
            id: row.get("id")?,
            name: row.get("name")?,
            purpose: row.get("purpose")?,
            event_name: row.get("event_name")?,
            artist_id: row.get("artist_id")?,
            target_date: row.get("target_date")?,
            status: row.get("status")?,
            color: row.get("color")?,
            created_at: row.get("created_at")?,
            updated_at: row.get("updated_at")?,
            sent_count: row.get("sent_count").unwrap_or(0),
            opened_count: row.get("opened_count").unwrap_or(0),
        })
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct Event {
    pub id: Option<i64>,
    #[serde(default)]
    pub artist_id: Option<i64>,
    #[serde(default)]
    pub contact_id: Option<i64>,
    pub title: String,
    pub venue: Option<String>,
    pub city: Option<String>,
    pub date: String,
    pub start_time: Option<String>,
    pub end_time: Option<String>,
    #[serde(default = "default_event_status")]
    pub status: String,
    pub fee: Option<f64>,
    pub notes: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

fn default_event_status() -> String {
    "confirmed".into()
}

impl Event {
    pub fn from_row(row: &rusqlite::Row) -> rusqlite::Result<Self> {
        Ok(Event {
            id: row.get("id")?,
            artist_id: row.get("artist_id")?,
            contact_id: row.get("contact_id")?,
            title: row.get("title")?,
            venue: row.get("venue")?,
            city: row.get("city")?,
            date: row.get("date")?,
            start_time: row.get("start_time")?,
            end_time: row.get("end_time")?,
            status: row.get("status")?,
            fee: row.get("fee")?,
            notes: row.get("notes")?,
            created_at: row.get("created_at")?,
            updated_at: row.get("updated_at")?,
        })
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct BudgetItem {
    pub id: Option<i64>,
    pub category: Option<String>,
    pub item: String,
    #[serde(default)]
    pub min_cost: f64,
    #[serde(default)]
    pub max_cost: f64,
    pub actual: Option<f64>,
    #[serde(default = "default_kind")]
    pub kind: String,
    pub notes: Option<String>,
    #[serde(default)]
    pub sort: i64,
}

fn default_kind() -> String {
    "expense".into()
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct Task {
    pub id: Option<i64>,
    pub period: Option<String>,
    pub title: String,
    #[serde(default)]
    pub done: bool,
    pub owner: Option<String>,
    pub due_date: Option<String>,
    #[serde(default)]
    pub sort: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct Kpi {
    pub id: Option<i64>,
    #[serde(default)]
    pub artist_id: Option<i64>,
    pub goal: Option<String>,
    pub kpi: Option<String>,
    pub target: Option<String>,
    pub actual: Option<String>,
    #[serde(default)]
    pub sort: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct Artist {
    pub id: Option<i64>,
    pub name: String,
    pub real_name: Option<String>,
    pub tagline: Option<String>,
    pub bio: Option<String>,
    pub genres: Option<String>,
    pub city: Option<String>,
    pub country: Option<String>,
    pub avatar: Option<String>,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub booking_email: Option<String>,
    pub website: Option<String>,
    pub instagram: Option<String>,
    pub soundcloud: Option<String>,
    pub spotify: Option<String>,
    pub apple_music: Option<String>,
    pub beatport: Option<String>,
    pub youtube: Option<String>,
    pub press_quotes: Option<String>,
    pub achievements: Option<String>,
    pub links: Option<String>,
    #[serde(default)]
    pub mix_url: Option<String>,
    #[serde(default)]
    pub tech_rider: Option<String>,
    #[serde(default)]
    pub fee_range: Option<String>,
    #[serde(default)]
    pub stats: Option<String>,
    #[serde(default)]
    pub audience_cities: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

impl Artist {
    pub fn from_row(row: &rusqlite::Row) -> rusqlite::Result<Self> {
        Ok(Artist {
            id: row.get("id")?,
            name: row.get("name")?,
            real_name: row.get("real_name")?,
            tagline: row.get("tagline")?,
            bio: row.get("bio")?,
            genres: row.get("genres")?,
            city: row.get("city")?,
            country: row.get("country")?,
            avatar: row.get("avatar")?,
            email: row.get("email")?,
            phone: row.get("phone")?,
            booking_email: row.get("booking_email")?,
            website: row.get("website")?,
            instagram: row.get("instagram")?,
            soundcloud: row.get("soundcloud")?,
            spotify: row.get("spotify")?,
            apple_music: row.get("apple_music")?,
            beatport: row.get("beatport")?,
            youtube: row.get("youtube")?,
            press_quotes: row.get("press_quotes")?,
            achievements: row.get("achievements")?,
            links: row.get("links")?,
            mix_url: row.get("mix_url")?,
            tech_rider: row.get("tech_rider")?,
            fee_range: row.get("fee_range")?,
            stats: row.get("stats")?,
            audience_cities: row.get("audience_cities")?,
            created_at: row.get("created_at")?,
            updated_at: row.get("updated_at")?,
        })
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct VisaCountry {
    pub code: String,
    pub name: String,
    pub work_rules: Option<String>,
    pub visa_types: Option<String>,
    pub processing_time: Option<String>,
    pub required_docs: Option<String>,
    pub notes: Option<String>,
    pub official_link: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

impl VisaCountry {
    pub fn from_row(row: &rusqlite::Row) -> rusqlite::Result<Self> {
        Ok(VisaCountry {
            code: row.get("code")?,
            name: row.get("name")?,
            work_rules: row.get("work_rules")?,
            visa_types: row.get("visa_types")?,
            processing_time: row.get("processing_time")?,
            required_docs: row.get("required_docs")?,
            notes: row.get("notes")?,
            official_link: row.get("official_link")?,
            updated_at: row.get("updated_at")?,
        })
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct VisaDossier {
    pub id: Option<i64>,
    #[serde(default)]
    pub artist_id: Option<i64>,
    pub country_code: Option<String>,
    pub country_name: Option<String>,
    pub title: String,
    pub purpose: Option<String>,
    pub event_date: Option<String>,
    pub entry_date: Option<String>,
    #[serde(default = "default_visa_status")]
    pub status: String,
    /// JSON array of { label, done }
    pub checklist: Option<String>,
    pub notes: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

fn default_visa_status() -> String {
    "draft".into()
}

impl VisaDossier {
    pub fn from_row(row: &rusqlite::Row) -> rusqlite::Result<Self> {
        Ok(VisaDossier {
            id: row.get("id")?,
            artist_id: row.get("artist_id")?,
            country_code: row.get("country_code")?,
            country_name: row.get("country_name")?,
            title: row.get("title")?,
            purpose: row.get("purpose")?,
            event_date: row.get("event_date")?,
            entry_date: row.get("entry_date")?,
            status: row.get("status")?,
            checklist: row.get("checklist")?,
            notes: row.get("notes")?,
            created_at: row.get("created_at")?,
            updated_at: row.get("updated_at")?,
        })
    }
}

// ---------------- Venue Intelligence ----------------

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct ViReferenceArtist {
    pub id: Option<i64>,
    pub nom: String,
    #[serde(default)]
    pub nom_normalise: Option<String>,
    pub tier: i64,
    pub genres: Option<String>,
    #[serde(default = "default_true")]
    pub actif: bool,
}

fn default_true() -> bool {
    true
}

impl ViReferenceArtist {
    pub fn from_row(row: &rusqlite::Row) -> rusqlite::Result<Self> {
        Ok(ViReferenceArtist {
            id: row.get("id")?,
            nom: row.get("nom")?,
            nom_normalise: row.get("nom_normalise")?,
            tier: row.get("tier")?,
            genres: row.get("genres")?,
            actif: row.get::<_, i64>("actif")? != 0,
        })
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct ViArea {
    pub id: Option<i64>,
    pub slug: String,
    pub ra_area_id: Option<i64>,
    pub libelle: Option<String>,
    pub pays: Option<String>,
    #[serde(default = "default_true")]
    pub actif: bool,
    #[serde(default)]
    pub resolved_at: Option<String>,
}

impl ViArea {
    pub fn from_row(row: &rusqlite::Row) -> rusqlite::Result<Self> {
        Ok(ViArea {
            id: row.get("id")?,
            slug: row.get("slug")?,
            ra_area_id: row.get("ra_area_id")?,
            libelle: row.get("libelle")?,
            pays: row.get("pays")?,
            actif: row.get::<_, i64>("actif")? != 0,
            resolved_at: row.get("resolved_at")?,
        })
    }
}

#[derive(Debug, Serialize, Clone, Default)]
pub struct ViOverview {
    pub venues_total: i64,
    pub venues_qualifie: i64,
    pub venues_valide: i64,
    pub evidence_total: i64,
    pub contacts_total: i64,
    pub promoters_total: i64,
    pub reference_artists_total: i64,
    pub reference_artists_actifs: i64,
    pub areas_total: i64,
    pub areas_resolues: i64,
    pub runs_total: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct SmtpConfig {
    #[serde(default)]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub from_name: String,
    #[serde(default)]
    pub from_email: String,
    /// "starttls" | "tls" (implicit) | "none"
    #[serde(default = "default_encryption")]
    pub encryption: String,
}

fn default_port() -> u16 {
    587
}
fn default_encryption() -> String {
    "starttls".into()
}
