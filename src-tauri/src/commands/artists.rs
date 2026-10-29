use crate::models::Artist;
use crate::AppState;
use rusqlite::params;
use std::io::Read;
use tauri::State;

#[tauri::command(async)]
pub fn list_artists(state: State<'_, AppState>) -> Result<Vec<Artist>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let mut stmt = conn
        // Every column but the base64 avatar (often hundreds of KB), which every
        // `artists` query shipped to every page. The UI loads it per artist
        // through `get_artist_avatar`; `get_artist` still returns it.
        .prepare(
            "SELECT id, name, real_name, tagline, bio, genres, city, country, NULL AS avatar,
                    email, phone, booking_email, website, instagram, soundcloud, spotify,
                    apple_music, beatport, youtube, press_quotes, achievements, links, mix_url,
                    tech_rider, fee_range, stats, audience_cities, spotify_artist_id,
                    created_at, updated_at
             FROM artists ORDER BY name COLLATE NOCASE",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], Artist::from_row)
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

#[tauri::command(async)]
pub fn get_artist(state: State<'_, AppState>, id: i64) -> Result<Option<Artist>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT * FROM artists WHERE id=?1")
        .map_err(|e| e.to_string())?;
    let mut rows = stmt
        .query_map(params![id], Artist::from_row)
        .map_err(|e| e.to_string())?;
    match rows.next() {
        Some(r) => Ok(Some(r.map_err(|e| e.to_string())?)),
        None => Ok(None),
    }
}

/// Just the avatar (data URL) of one artist, for list views.
#[tauri::command(async)]
pub fn get_artist_avatar(state: State<'_, AppState>, id: i64) -> Result<Option<String>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let v: Option<String> = conn
        .query_row("SELECT avatar FROM artists WHERE id=?1", params![id], |r| r.get(0))
        .ok()
        .flatten();
    Ok(v.filter(|s| !s.trim().is_empty()))
}

#[tauri::command(async)]
pub fn save_artist(state: State<'_, AppState>, artist: Artist) -> Result<i64, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let p = params![
        artist.name,
        artist.real_name,
        artist.tagline,
        artist.bio,
        artist.genres,
        artist.city,
        artist.country,
        artist.avatar,
        artist.email,
        artist.phone,
        artist.booking_email,
        artist.website,
        artist.instagram,
        artist.soundcloud,
        artist.spotify,
        artist.apple_music,
        artist.beatport,
        artist.youtube,
        artist.press_quotes,
        artist.achievements,
        artist.links,
        artist.mix_url,
        artist.tech_rider,
        artist.fee_range,
        artist.stats,
        artist.audience_cities,
        artist.spotify_artist_id,
    ];
    match artist.id {
        Some(id) => {
            conn.execute(
                "UPDATE artists SET
                    name=?1, real_name=?2, tagline=?3, bio=?4, genres=?5, city=?6, country=?7,
                    avatar=?8, email=?9, phone=?10, booking_email=?11, website=?12, instagram=?13,
                    soundcloud=?14, spotify=?15, apple_music=?16, beatport=?17, youtube=?18,
                    press_quotes=?19, achievements=?20, links=?21, mix_url=?22, tech_rider=?23,
                    fee_range=?24, stats=?25, audience_cities=?26, spotify_artist_id=?27, updated_at=datetime('now')
                 WHERE id=?28",
                params![
                    artist.name, artist.real_name, artist.tagline, artist.bio, artist.genres,
                    artist.city, artist.country, artist.avatar, artist.email, artist.phone,
                    artist.booking_email, artist.website, artist.instagram, artist.soundcloud,
                    artist.spotify, artist.apple_music, artist.beatport, artist.youtube,
                    artist.press_quotes, artist.achievements, artist.links, artist.mix_url,
                    artist.tech_rider, artist.fee_range, artist.stats, artist.audience_cities,
                    artist.spotify_artist_id, id
                ],
            )
            .map_err(|e| e.to_string())?;
            Ok(id)
        }
        None => {
            conn.execute(
                "INSERT INTO artists
                    (name, real_name, tagline, bio, genres, city, country, avatar, email, phone,
                     booking_email, website, instagram, soundcloud, spotify, apple_music, beatport,
                     youtube, press_quotes, achievements, links, mix_url, tech_rider, fee_range,
                     stats, audience_cities, spotify_artist_id)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26,?27)",
                p,
            )
            .map_err(|e| e.to_string())?;
            Ok(conn.last_insert_rowid())
        }
    }
}

#[tauri::command(async)]
pub fn delete_artist(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM artists WHERE id=?1", params![id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Read an image file from disk and return it as a base64 `data:` URL,
/// so avatars embed cleanly in the UI and in the PDF/print export.
#[tauri::command(async)]
pub fn image_to_data_url(path: String) -> Result<String, String> {
    let mut file = std::fs::File::open(&path).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() > 6 * 1024 * 1024 {
        return Err("Image too large (max 6 MB).".into());
    }
    let mime = match path.rsplit('.').next().map(|s| s.to_lowercase()).as_deref() {
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        Some("svg") => "image/svg+xml",
        _ => "image/png",
    };
    Ok(format!("data:{};base64,{}", mime, base64_encode(&bytes)))
}

/// Minimal base64 encoder (avoids pulling an extra dependency).
fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((input.len() + 2) / 3 * 4);
    for chunk in input.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | (b[2] as u32);
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((n >> 6) & 63) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(n & 63) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// The artist a new contact belongs to when nothing else says so: the only
/// artist of the roster, or None when there are zero or several (then the
/// choice is the user's). Used by the import, the startup backfill and the
/// Discovery / Radar promotion paths, so a one-artist CRM never grows contacts
/// without an artist again (they made the artist page and filter useless).
pub(crate) fn default_artist_id(conn: &rusqlite::Connection) -> Option<i64> {
    let mut stmt = conn.prepare("SELECT id FROM artists LIMIT 2").ok()?;
    let ids: Vec<i64> = stmt
        .query_map([], |r| r.get(0))
        .ok()?
        .filter_map(Result::ok)
        .collect();
    match ids.as_slice() {
        [only] => Some(*only),
        _ => None,
    }
}
