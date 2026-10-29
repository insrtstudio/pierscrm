//! Secrets (SMTP password, API keys, tokens) live in the macOS Keychain instead
//! of the plain-text `settings` table.
//!
//! Everything goes through `read_setting` / `write_setting` / `ui_setting`, so
//! callers keep asking for "serper_api_key" as before and never care where the
//! value lives. All secrets share ONE keychain item (a JSON map) under the
//! service "fr.insrt.pierscrm": the app is ad-hoc signed, so macOS may ask once
//! per item after each update; one item means one prompt, not five.
//!
//! The frontend never gets a secret back: `ui_setting` returns `MASK` when one
//! is set, and writing `MASK` back is a no-op (saving an untouched form keeps
//! the stored value).
//!
//! Safety rules: a secret is only blanked in SQLite after the keychain copy has
//! been written AND read back; if the keychain is unavailable the DB value is
//! kept (and still read), so a keychain failure never loses a key.

use crate::models::SmtpConfig;
use rusqlite::{params, Connection};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

pub const SERVICE: &str = "fr.insrt.pierscrm";
const ACCOUNT: &str = "secrets";

/// Placeholder sent to the UI for a configured secret. Never a real value.
pub const MASK: &str = "\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}";

/// Settings keys whose value is a secret.
pub const SECRET_KEYS: &[&str] = &[
    "serper_api_key",
    "youtube_api_key",
    "spotify_client_secret",
    "meta_access_token",
];

/// Vault key of the SMTP password (stored inside the `smtp` JSON setting before).
pub const SMTP_PASSWORD: &str = "smtp_password";

pub fn is_secret(key: &str) -> bool {
    SECRET_KEYS.contains(&key)
}

// ---------------- Storage backend ----------------

/// Raw storage of the secrets blob. The real one is the Keychain; tests use an
/// in-memory fake so they never touch the user's keychain.
pub trait SecretBackend: Send + Sync {
    fn load(&self) -> Result<Option<String>, String>;
    fn store(&self, blob: &str) -> Result<(), String>;
}

#[cfg(not(test))]
struct KeychainBackend;

#[cfg(not(test))]
impl KeychainBackend {
    fn entry() -> Result<keyring::Entry, String> {
        keyring::Entry::new(SERVICE, ACCOUNT).map_err(|e| e.to_string())
    }
}

#[cfg(not(test))]
impl SecretBackend for KeychainBackend {
    fn load(&self) -> Result<Option<String>, String> {
        match Self::entry()?.get_password() {
            Ok(s) => Ok(Some(s)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }
    fn store(&self, blob: &str) -> Result<(), String> {
        Self::entry()?.set_password(blob).map_err(|e| e.to_string())
    }
}

/// Secrets map cached in memory after the first read (one keychain access per
/// launch instead of one per API call).
pub struct Vault {
    backend: Box<dyn SecretBackend>,
    cache: Mutex<Option<BTreeMap<String, String>>>,
    /// Set when a read failed (keychain denied or locked): later reads skip the
    /// keychain instead of re-prompting on every API call of a long enrich run.
    /// A write (explicit user action) tries again and clears it on success.
    read_failed: AtomicBool,
}

impl Vault {
    pub fn new(backend: Box<dyn SecretBackend>) -> Self {
        Self { backend, cache: Mutex::new(None), read_failed: AtomicBool::new(false) }
    }

    fn load_map(&self) -> Result<BTreeMap<String, String>, String> {
        match self.backend.load()? {
            Some(blob) if !blob.trim().is_empty() => {
                serde_json::from_str(&blob).map_err(|e| format!("trousseau illisible: {}", e))
            }
            _ => Ok(BTreeMap::new()),
        }
    }

    fn with_map<T>(&self, f: impl FnOnce(&mut BTreeMap<String, String>) -> T) -> Result<T, String> {
        let mut guard = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_none() {
            *guard = Some(self.load_map()?);
        }
        Ok(f(guard.as_mut().expect("loaded")))
    }

    pub fn get(&self, key: &str) -> Result<Option<String>, String> {
        if self.read_failed.load(Ordering::Relaxed) {
            return Err("trousseau indisponible".into());
        }
        let r = self.with_map(|m| m.get(key).cloned());
        if r.is_err() {
            self.read_failed.store(true, Ordering::Relaxed);
        }
        r
    }

    /// Set (Some) or remove (None) one secret and persist the whole map. The
    /// cache only changes once the backend accepted the write.
    pub fn put(&self, key: &str, value: Option<&str>) -> Result<(), String> {
        let mut guard = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_none() {
            *guard = Some(self.load_map()?);
        }
        let mut next = guard.clone().expect("loaded");
        match value {
            Some(v) => next.insert(key.to_string(), v.to_string()),
            None => next.remove(key),
        };
        let blob = serde_json::to_string(&next).map_err(|e| e.to_string())?;
        self.backend.store(&blob)?;
        *guard = Some(next);
        self.read_failed.store(false, Ordering::Relaxed);
        Ok(())
    }

    /// Re-read the backend (bypassing the cache) and check the stored value.
    pub fn verify(&self, key: &str, expected: &str) -> bool {
        matches!(self.load_map(), Ok(m) if m.get(key).map(String::as_str) == Some(expected))
    }
}

static VAULT: OnceLock<Vault> = OnceLock::new();

/// The process-wide vault: the Keychain in the app, an in-memory fake in tests.
pub fn vault() -> &'static Vault {
    VAULT.get_or_init(|| {
        #[cfg(not(test))]
        let backend: Box<dyn SecretBackend> = Box::new(KeychainBackend);
        #[cfg(test)]
        let backend: Box<dyn SecretBackend> = Box::new(tests::FakeBackend::default());
        Vault::new(backend)
    })
}

// ---------------- Settings routing ----------------

fn db_get(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM settings WHERE key=?1", params![key], |r| {
        r.get::<_, Option<String>>(0)
    })
    .ok()
    .flatten()
}

fn db_set(conn: &Connection, key: &str, value: &str) -> Result<(), String> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

fn non_empty(v: Option<String>) -> Option<String> {
    v.filter(|s| !s.trim().is_empty())
}

pub fn read_setting_with(vault: &Vault, conn: &Connection, key: &str) -> Option<String> {
    if is_secret(key) {
        match vault.get(key) {
            Ok(Some(v)) if !v.trim().is_empty() => return Some(v),
            Ok(_) => {}
            Err(e) => eprintln!("[secrets] keychain read failed for {}: {}", key, e),
        }
        // Not migrated yet (or keychain unavailable): the DB copy still works.
    }
    db_get(conn, key)
}

pub fn write_setting_with(vault: &Vault, conn: &Connection, key: &str, value: &str) -> Result<(), String> {
    if !is_secret(key) {
        return db_set(conn, key, value);
    }
    if value == MASK {
        return Ok(()); // untouched masked field: keep the stored secret
    }
    let v = value.trim();
    if v.is_empty() {
        if let Err(e) = vault.put(key, None) {
            eprintln!("[secrets] keychain delete failed for {}: {}", key, e);
        }
        return db_set(conn, key, "");
    }
    match vault.put(key, Some(v)) {
        Ok(()) => db_set(conn, key, ""),
        Err(e) => {
            // Keep the app usable: the next launch migrates it to the keychain.
            eprintln!("[secrets] keychain write failed for {}, kept in DB: {}", key, e);
            db_set(conn, key, v)
        }
    }
}

/// What `get_setting` returns to the frontend: MASK instead of a secret value.
pub fn ui_setting_with(vault: &Vault, conn: &Connection, key: &str) -> Option<String> {
    let v = read_setting_with(vault, conn, key);
    if is_secret(key) {
        return non_empty(v).map(|_| MASK.to_string());
    }
    v
}

/// Raw read (empty strings included, like the old `SELECT value FROM settings`).
pub fn read_setting(conn: &Connection, key: &str) -> Option<String> {
    read_setting_with(vault(), conn, key)
}

pub fn write_setting(conn: &Connection, key: &str, value: &str) -> Result<(), String> {
    write_setting_with(vault(), conn, key, value)
}

pub fn ui_setting(conn: &Connection, key: &str) -> Option<String> {
    ui_setting_with(vault(), conn, key)
}

// ---------------- SMTP password ----------------

pub fn smtp_fill_with(vault: &Vault, cfg: &mut SmtpConfig) {
    if cfg.password.is_empty() {
        match vault.get(SMTP_PASSWORD) {
            Ok(Some(p)) => cfg.password = p,
            Ok(None) => {}
            Err(e) => eprintln!("[secrets] keychain read failed for smtp: {}", e),
        }
    }
}

/// Before the `smtp` JSON is written: move the password to the vault and blank it.
pub fn smtp_store_with(vault: &Vault, cfg: &mut SmtpConfig) -> Result<(), String> {
    if cfg.password == MASK {
        cfg.password.clear();
        return Ok(());
    }
    if cfg.password.is_empty() {
        if let Err(e) = vault.put(SMTP_PASSWORD, None) {
            eprintln!("[secrets] keychain delete failed for smtp: {}", e);
        }
        return Ok(());
    }
    match vault.put(SMTP_PASSWORD, Some(&cfg.password)) {
        Ok(()) => cfg.password.clear(),
        Err(e) => eprintln!("[secrets] keychain write failed for smtp, kept in DB: {}", e),
    }
    Ok(())
}

/// Fill a config loaded from SQLite with the keychain password.
pub fn smtp_fill(cfg: &mut SmtpConfig) {
    smtp_fill_with(vault(), cfg)
}

pub fn smtp_store(cfg: &mut SmtpConfig) -> Result<(), String> {
    smtp_store_with(vault(), cfg)
}

/// The config as the Settings page sees it (password replaced by MASK).
pub fn smtp_masked(mut cfg: SmtpConfig) -> SmtpConfig {
    if !cfg.password.is_empty() {
        cfg.password = MASK.to_string();
    }
    cfg
}

// ---------------- Startup migration ----------------

#[derive(Debug, Default, PartialEq)]
pub struct MigrationReport {
    pub moved: Vec<String>,
    pub failed: Vec<String>,
}

fn move_one(vault: &Vault, key: &str, value: &str) -> bool {
    match vault.put(key, Some(value)) {
        Ok(()) if vault.verify(key, value) => true,
        Ok(()) => {
            eprintln!("[secrets] keychain read-back mismatch for {}, DB value kept", key);
            false
        }
        Err(e) => {
            eprintln!("[secrets] keychain migration failed for {}, DB value kept: {}", key, e);
            false
        }
    }
}

/// Move every secret still in clear in `settings` to the vault. A value is
/// blanked in the DB only after a verified keychain write.
pub fn migrate_with(vault: &Vault, conn: &Connection) -> MigrationReport {
    let mut report = MigrationReport::default();
    for key in SECRET_KEYS {
        let Some(v) = non_empty(db_get(conn, key)) else { continue };
        let v = v.trim().to_string();
        if move_one(vault, key, &v) && db_set(conn, key, "").is_ok() {
            report.moved.push(key.to_string());
        } else {
            report.failed.push(key.to_string());
        }
    }
    if let Some(json) = non_empty(db_get(conn, "smtp")) {
        if let Ok(mut cfg) = serde_json::from_str::<SmtpConfig>(&json) {
            if !cfg.password.is_empty() && cfg.password != MASK {
                if move_one(vault, SMTP_PASSWORD, &cfg.password) {
                    cfg.password.clear();
                    let ok = serde_json::to_string(&cfg)
                        .map_err(|e| e.to_string())
                        .and_then(|j| db_set(conn, "smtp", &j))
                        .is_ok();
                    if ok {
                        report.moved.push(SMTP_PASSWORD.to_string());
                    } else {
                        report.failed.push(SMTP_PASSWORD.to_string());
                    }
                } else {
                    report.failed.push(SMTP_PASSWORD.to_string());
                }
            }
        }
    }
    report
}

pub fn migrate(conn: &Connection) -> MigrationReport {
    let r = migrate_with(vault(), conn);
    if !r.moved.is_empty() || !r.failed.is_empty() {
        eprintln!("[secrets] moved to keychain: {:?}, kept in DB: {:?}", r.moved, r.failed);
    }
    r
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::Arc;

    #[derive(Default, Clone)]
    pub struct FakeBackend {
        pub blob: Arc<Mutex<Option<String>>>,
        pub fail: Arc<AtomicBool>,
        /// Accept writes but drop them (read-back check must catch it).
        pub lossy: Arc<AtomicBool>,
    }

    impl SecretBackend for FakeBackend {
        fn load(&self) -> Result<Option<String>, String> {
            if self.fail.load(Ordering::SeqCst) {
                return Err("keychain locked".into());
            }
            Ok(self.blob.lock().unwrap().clone())
        }
        fn store(&self, blob: &str) -> Result<(), String> {
            if self.fail.load(Ordering::SeqCst) {
                return Err("keychain locked".into());
            }
            if !self.lossy.load(Ordering::SeqCst) {
                *self.blob.lock().unwrap() = Some(blob.to_string());
            }
            Ok(())
        }
    }

    fn setup() -> (Vault, FakeBackend, Connection) {
        let fake = FakeBackend::default();
        let vault = Vault::new(Box::new(fake.clone()));
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);").unwrap();
        (vault, fake, conn)
    }

    #[test]
    fn non_secret_keys_stay_in_db() {
        let (v, fake, c) = setup();
        write_setting_with(&v, &c, "tracking_base_url", "https://t.example").unwrap();
        assert_eq!(db_get(&c, "tracking_base_url").as_deref(), Some("https://t.example"));
        assert_eq!(ui_setting_with(&v, &c, "tracking_base_url").as_deref(), Some("https://t.example"));
        assert!(fake.blob.lock().unwrap().is_none());
    }

    #[test]
    fn secret_goes_to_vault_and_ui_sees_mask() {
        let (v, _fake, c) = setup();
        write_setting_with(&v, &c, "serper_api_key", " abc123 ").unwrap();
        assert_eq!(db_get(&c, "serper_api_key").as_deref(), Some(""));
        assert_eq!(read_setting_with(&v, &c, "serper_api_key").as_deref(), Some("abc123"));
        assert_eq!(ui_setting_with(&v, &c, "serper_api_key").as_deref(), Some(MASK));
        // Saving the untouched mask keeps the value.
        write_setting_with(&v, &c, "serper_api_key", MASK).unwrap();
        assert_eq!(read_setting_with(&v, &c, "serper_api_key").as_deref(), Some("abc123"));
        // Clearing the field removes it.
        write_setting_with(&v, &c, "serper_api_key", "").unwrap();
        assert_eq!(non_empty(read_setting_with(&v, &c, "serper_api_key")), None);
        assert_eq!(ui_setting_with(&v, &c, "serper_api_key"), None);
    }

    #[test]
    fn keychain_failure_falls_back_to_db() {
        let (v, fake, c) = setup();
        fake.fail.store(true, Ordering::SeqCst);
        write_setting_with(&v, &c, "youtube_api_key", "yt").unwrap();
        assert_eq!(db_get(&c, "youtube_api_key").as_deref(), Some("yt"));
        assert_eq!(read_setting_with(&v, &c, "youtube_api_key").as_deref(), Some("yt"));
        // A failed read is not retried on every call (no prompt storm)...
        fake.fail.store(false, Ordering::SeqCst);
        assert!(v.get("youtube_api_key").is_err());
        // ...until a write succeeds.
        write_setting_with(&v, &c, "youtube_api_key", "yt2").unwrap();
        assert_eq!(read_setting_with(&v, &c, "youtube_api_key").as_deref(), Some("yt2"));
        assert_eq!(db_get(&c, "youtube_api_key").as_deref(), Some(""));
    }

    #[test]
    fn migration_moves_verified_and_blanks_db() {
        let (v, fake, c) = setup();
        c.execute_batch(
            "INSERT INTO settings VALUES ('meta_access_token','EAAB'),('spotify_client_id','cid'),
             ('smtp','{\"host\":\"h\",\"port\":587,\"username\":\"u\",\"password\":\"pw\",\"from_name\":\"\",\"from_email\":\"\",\"encryption\":\"starttls\"}');",
        )
        .unwrap();
        let r = migrate_with(&v, &c);
        assert_eq!(r.moved, vec!["meta_access_token".to_string(), SMTP_PASSWORD.to_string()]);
        assert!(r.failed.is_empty());
        assert_eq!(db_get(&c, "meta_access_token").as_deref(), Some(""));
        assert_eq!(db_get(&c, "spotify_client_id").as_deref(), Some("cid"));
        let cfg: SmtpConfig = serde_json::from_str(&db_get(&c, "smtp").unwrap()).unwrap();
        assert_eq!(cfg.password, "");
        assert_eq!(cfg.host, "h");
        let blob = fake.blob.lock().unwrap().clone().unwrap();
        assert!(blob.contains("EAAB") && blob.contains("pw"));
        // Readers are unaffected.
        assert_eq!(read_setting_with(&v, &c, "meta_access_token").as_deref(), Some("EAAB"));
        let mut cfg = cfg;
        smtp_fill_with(&v, &mut cfg);
        assert_eq!(cfg.password, "pw");
        // Idempotent.
        assert_eq!(migrate_with(&v, &c), MigrationReport::default());
    }

    #[test]
    fn migration_never_loses_a_secret() {
        let (v, fake, c) = setup();
        c.execute("INSERT INTO settings VALUES ('serper_api_key','k1')", []).unwrap();
        fake.fail.store(true, Ordering::SeqCst);
        let r = migrate_with(&v, &c);
        assert_eq!(r.failed, vec!["serper_api_key".to_string()]);
        assert_eq!(db_get(&c, "serper_api_key").as_deref(), Some("k1"));

        // A write that "succeeds" but does not persist is caught by the read-back.
        let (v, fake, c) = setup();
        c.execute("INSERT INTO settings VALUES ('serper_api_key','k1')", []).unwrap();
        fake.lossy.store(true, Ordering::SeqCst);
        let r = migrate_with(&v, &c);
        assert_eq!(r.failed, vec!["serper_api_key".to_string()]);
        assert_eq!(db_get(&c, "serper_api_key").as_deref(), Some("k1"));
        assert_eq!(read_setting_with(&v, &c, "serper_api_key").as_deref(), Some("k1"));
    }

    #[test]
    fn smtp_mask_roundtrip() {
        let (v, _fake, _c) = setup();
        let mut cfg = SmtpConfig { password: "secret".into(), ..Default::default() };
        smtp_store_with(&v, &mut cfg).unwrap();
        assert_eq!(cfg.password, "");
        smtp_fill_with(&v, &mut cfg);
        assert_eq!(cfg.password, "secret");
        let masked = smtp_masked(cfg);
        assert_eq!(masked.password, MASK);
        // Saving the masked config back keeps the keychain password.
        let mut again = masked;
        smtp_store_with(&v, &mut again).unwrap();
        assert_eq!(again.password, "");
        smtp_fill_with(&v, &mut again);
        assert_eq!(again.password, "secret");
    }
}
