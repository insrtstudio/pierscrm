use crate::AppState;
use calamine::{open_workbook_auto, Data, Reader};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tauri::State;

const PREVIEW_ROWS: usize = 8;

#[derive(Serialize)]
pub struct SheetPreview {
    pub name: String,
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub total_rows: usize,
    /// 0-based index of the detected header row; import should skip header_row + 1 rows.
    pub header_row: usize,
}

#[derive(Serialize)]
pub struct FilePreview {
    pub kind: String, // "xlsx" | "csv"
    pub sheets: Vec<SheetPreview>,
}

fn cell_to_string(d: &Data) -> String {
    match d {
        Data::Empty => String::new(),
        Data::String(s) => s.trim().to_string(),
        Data::Float(f) => {
            if f.fract() == 0.0 {
                format!("{}", *f as i64)
            } else {
                f.to_string()
            }
        }
        Data::Int(i) => i.to_string(),
        Data::Bool(b) => b.to_string(),
        Data::DateTime(dt) => dt.to_string(),
        Data::DateTimeIso(s) => s.clone(),
        Data::DurationIso(s) => s.clone(),
        Data::Error(e) => format!("{:?}", e),
    }
}

/// CSV text as UTF-8 (BOM stripped). Excel on a French Mac/PC exports
/// Windows-1252, which is not valid UTF-8: decode that instead of failing.
fn decode_text(bytes: &[u8]) -> String {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    if let Ok(s) = std::str::from_utf8(bytes) {
        return s.to_string();
    }
    bytes
        .iter()
        .map(|&b| match b {
            0x80 => '€',
            0x85 => '…',
            0x8C => 'Œ',
            0x91 => '‘',
            0x92 => '’',
            0x93 => '“',
            0x94 => '”',
            0x96 => '–',
            0x97 => '—',
            0x9C => 'œ',
            _ => b as char, // Latin-1 for the rest
        })
        .collect()
}

/// French Excel writes `;`-separated CSV. Pick whichever of `;`, `,`, tab is
/// most frequent on the header line.
fn sniff_delimiter(text: &str) -> u8 {
    let first = text.lines().next().unwrap_or("");
    [b';', b',', b'\t']
        .into_iter()
        .max_by_key(|&d| first.bytes().filter(|&b| b == d).count())
        .filter(|&d| first.bytes().any(|b| b == d))
        .unwrap_or(b',')
}

/// Read the raw rows of a given sheet (or the whole CSV) into strings.
fn read_rows(path: &str, sheet: Option<&str>) -> Result<Vec<Vec<String>>, String> {
    let lower = path.to_lowercase();
    if lower.ends_with(".csv") || lower.ends_with(".tsv") {
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        let text = decode_text(&bytes);
        let delim = if lower.ends_with(".tsv") { b'\t' } else { sniff_delimiter(&text) };
        let mut rdr = csv::ReaderBuilder::new()
            .has_headers(false)
            .flexible(true)
            .delimiter(delim)
            .from_reader(text.as_bytes());
        let mut out = Vec::new();
        for rec in rdr.records() {
            let rec = rec.map_err(|e| e.to_string())?;
            out.push(rec.iter().map(|s| s.trim().to_string()).collect());
        }
        Ok(out)
    } else {
        let mut wb = open_workbook_auto(path).map_err(|e| e.to_string())?;
        let sheet_name = match sheet {
            Some(s) => s.to_string(),
            None => wb
                .sheet_names()
                .first()
                .cloned()
                .ok_or("no sheet in workbook")?,
        };
        let range = wb
            .worksheet_range(&sheet_name)
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for row in range.rows() {
            out.push(row.iter().map(cell_to_string).collect());
        }
        Ok(out)
    }
}

fn normalize(rows: Vec<Vec<String>>) -> SheetPreview {
    // Drop fully-empty leading rows (spreadsheets often have title banners).
    let mut start = 0usize;
    for (i, r) in rows.iter().enumerate() {
        let non_empty = r.iter().filter(|c| !c.is_empty()).count();
        if non_empty >= 2 {
            start = i;
            break;
        }
    }
    let headers = rows
        .get(start)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .map(|(i, h)| if h.is_empty() { format!("Col {}", i + 1) } else { h })
        .collect::<Vec<_>>();
    let data: Vec<Vec<String>> = rows.into_iter().skip(start + 1).collect();
    let total = data.len();
    let sample = data.into_iter().take(PREVIEW_ROWS).collect();
    SheetPreview {
        name: String::new(),
        headers,
        rows: sample,
        total_rows: total,
        header_row: start,
    }
}

#[tauri::command(async)]
pub fn preview_file(path: String) -> Result<FilePreview, String> {
    let lower = path.to_lowercase();
    if lower.ends_with(".csv") || lower.ends_with(".tsv") {
        let mut preview = normalize(read_rows(&path, None)?);
        preview.name = "CSV".into();
        Ok(FilePreview {
            kind: "csv".into(),
            sheets: vec![preview],
        })
    } else {
        let mut wb = open_workbook_auto(&path).map_err(|e| e.to_string())?;
        let names = wb.sheet_names().to_vec();
        let mut sheets = Vec::new();
        for name in names {
            if let Ok(range) = wb.worksheet_range(&name) {
                let rows: Vec<Vec<String>> = range
                    .rows()
                    .map(|r| r.iter().map(cell_to_string).collect())
                    .collect();
                let mut prev = normalize(rows);
                prev.name = name;
                sheets.push(prev);
            }
        }
        Ok(FilePreview {
            kind: "xlsx".into(),
            sheets,
        })
    }
}

#[derive(Deserialize)]
pub struct ImportRequest {
    pub path: String,
    pub sheet: Option<String>,
    pub category: String,
    /// field name -> column index (0-based) in the source header row
    pub mapping: HashMap<String, usize>,
    /// how many leading rows to skip (the header row + any banner), computed on the client
    pub skip_rows: usize,
    /// Artist the imported contacts belong to. Absent: the only artist of the
    /// roster, if there is exactly one. `Some(None)` / null: no artist.
    #[serde(default, deserialize_with = "explicit_option")]
    pub artist_id: Option<Option<i64>>,
}

/// Tells "field absent" (None) from "field sent as null" (Some(None)).
fn explicit_option<'de, D>(d: D) -> Result<Option<Option<i64>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Some(Option::<i64>::deserialize(d)?))
}

const DATE_FIELDS: &[(&str, &str)] = &[("date", ""), ("first_contact", "premier contact"), ("follow_up", "relance")];

#[derive(Serialize)]
pub struct ImportResult {
    pub inserted: usize,
    pub skipped: usize,
    /// Rows already in the pipeline (same name + venue + email): re-importing
    /// a sheet used to duplicate every contact.
    pub duplicates: usize,
}

/// Spreadsheet statuses are free text ("Contacted", "À contacter", "No reply").
/// Stored raw they matched no status filter, no badge and no dashboard bar, so
/// map the common spellings onto the app's keys and fall back to `to_contact`.
fn normalize_status(raw: &str) -> &'static str {
    let s = raw.trim().to_lowercase().replace(['_', '-'], " ");
    let s = s.as_str();
    let has = |n: &str| s.contains(n);
    if has("confirm") || has("booked") || has("signed") || has("valid") {
        "confirmed"
    } else if has("declin") || has("refus") || has("rejet") || has("reject") || s == "no" || s == "non" {
        "declined"
    } else if has("no answer") || has("no reply") || has("no response") || has("sans r") || has("pas de r") {
        "no_answer"
    } else if has("discuss") || has("négo") || has("nego") || has("talk") || has("en cours") || has("replied") || has("répon") {
        "in_discussion"
    } else if has("follow") || has("relanc") {
        "followed_up"
    } else if has("evaluat") || has("évalu") || has("tbd") || has("maybe") {
        "to_evaluate"
    } else if has("low") || has("basse") || has("later") {
        "low_priority"
    } else if (has("contacted") || has("contacté") || has("sent") || has("envoy") || has("emailed"))
        && !has("to contact")
        && !has("à contacter")
        && !has("a contacter")
    {
        "contacted"
    } else {
        "to_contact"
    }
}

const ALLOWED_FIELDS: &[&str] = &[
    "name", "priority", "promoter", "venue", "type", "area", "scale", "date", "time", "format",
    "reason", "contact_channel", "email", "email_status", "status", "first_contact", "follow_up",
    "notes", "website", "tags",
];

#[tauri::command(async)]
pub fn import_file(state: State<'_, AppState>, req: ImportRequest) -> Result<ImportResult, String> {
    // Validate the mapping fields against the whitelist to build a safe SQL statement.
    let mut fields: Vec<(&'static str, usize)> = Vec::new();
    for (field, col) in &req.mapping {
        if let Some(f) = ALLOWED_FIELDS.iter().find(|f| **f == field.as_str()) {
            fields.push((*f, *col));
        }
    }
    if !fields.iter().any(|(f, _)| *f == "name") {
        return Err("A column must be mapped to 'name'.".into());
    }

    let rows = read_rows(&req.path, req.sheet.as_deref())?;
    let data: Vec<Vec<String>> = rows.into_iter().skip(req.skip_rows).collect();

    // Dates are stored ISO (the Pipeline edits them with date pickers): a cell
    // that is not a single readable day is kept, word for word, in the notes.
    let has_notes = fields.iter().any(|(f, _)| *f == "notes");
    let col_names: Vec<String> = ["category", "artist_id"]
        .into_iter()
        .map(String::from)
        .chain(fields.iter().map(|(f, _)| (*f).to_string()))
        .chain((!has_notes).then(|| "notes".to_string()))
        .collect();
    let placeholders: Vec<String> = (1..=col_names.len()).map(|i| format!("?{}", i)).collect();
    let sql = format!(
        "INSERT INTO contacts ({}) VALUES ({})",
        col_names
            .iter()
            .map(|c| if *c == "type" { "type".to_string() } else { c.clone() })
            .collect::<Vec<_>>()
            .join(", "),
        placeholders.join(", ")
    );

    let mut conn = state.pool.get().map_err(|e| e.to_string())?;
    let artist_id = match req.artist_id {
        Some(explicit) => explicit,
        None => crate::commands::artists::default_artist_id(&conn),
    };
    let today = chrono::Local::now().date_naive();
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let mut inserted = 0usize;
    let mut skipped = 0usize;
    let mut duplicates = 0usize;
    let col_of = |name: &str| fields.iter().find(|(f, _)| *f == name).map(|(_, c)| *c);
    let (venue_col, email_col) = (col_of("venue"), col_of("email"));
    {
        let mut stmt = tx.prepare(&sql).map_err(|e| e.to_string())?;
        let mut dup = tx
            .prepare(
                "SELECT 1 FROM contacts
                 WHERE lower(trim(name)) = lower(?1)
                   AND lower(trim(COALESCE(venue, ''))) = lower(?2)
                   AND lower(trim(COALESCE(email, ''))) = lower(?3)
                 LIMIT 1",
            )
            .map_err(|e| e.to_string())?;
        for row in &data {
            // require a non-empty name
            let name_col = fields.iter().find(|(f, _)| *f == "name").map(|(_, c)| *c);
            let name_val = name_col.and_then(|c| row.get(c)).map(|s| s.trim());
            if name_val.map(|s| s.is_empty()).unwrap_or(true) {
                skipped += 1;
                continue;
            }
            let cell = |c: Option<usize>| {
                c.and_then(|c| row.get(c)).map(|s| s.trim().to_string()).unwrap_or_default()
            };
            let exists = dup
                .exists(rusqlite::params![
                    name_val.unwrap_or_default(),
                    cell(venue_col),
                    cell(email_col)
                ])
                .map_err(|e| e.to_string())?;
            if exists {
                duplicates += 1;
                continue;
            }
            let mut vals: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
            vals.push(Box::new(req.category.clone()));
            vals.push(Box::new(artist_id));
            let mut date_notes: Vec<String> = Vec::new();
            let mut notes_slot: Option<usize> = None;
            for (field, col) in &fields {
                let v = row.get(*col).cloned().unwrap_or_default();
                let v = v.trim().to_string();
                if let Some((_, label)) = DATE_FIELDS.iter().find(|(f, _)| f == field) {
                    let (iso, note) = crate::crm_links::convert_date(&v, today, label);
                    date_notes.extend(note);
                    vals.push(Box::new(iso));
                    continue;
                }
                if *field == "notes" {
                    notes_slot = Some(vals.len());
                }
                if v.is_empty() {
                    // status is NOT NULL — fall back to the default rather than inserting NULL
                    if *field == "status" {
                        vals.push(Box::new("to_contact".to_string()));
                    } else {
                        vals.push(Box::new(Option::<String>::None));
                    }
                } else if *field == "status" {
                    vals.push(Box::new(normalize_status(&v).to_string()));
                } else {
                    vals.push(Box::new(v));
                }
            }
            // Original date text goes to the notes column (mapped or appended).
            let notes_cell = notes_slot
                .and_then(|_| fields.iter().find(|(f, _)| *f == "notes"))
                .and_then(|(_, c)| row.get(*c))
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            let merged = match (notes_cell, date_notes.is_empty()) {
                (n, true) => n,
                (Some(n), false) => Some(format!("{n}\n{}", date_notes.join("\n"))),
                (None, false) => Some(date_notes.join("\n")),
            };
            match notes_slot {
                Some(i) => vals[i] = Box::new(merged),
                None => vals.push(Box::new(merged)),
            }
            let refs: Vec<&dyn rusqlite::types::ToSql> = vals.iter().map(|b| b.as_ref()).collect();
            stmt.execute(refs.as_slice()).map_err(|e| e.to_string())?;
            inserted += 1;
        }
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(ImportResult { inserted, skipped, duplicates })
}
