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

/// Read the raw rows of a given sheet (or the whole CSV) into strings.
fn read_rows(path: &str, sheet: Option<&str>) -> Result<Vec<Vec<String>>, String> {
    let lower = path.to_lowercase();
    if lower.ends_with(".csv") || lower.ends_with(".tsv") {
        let delim = if lower.ends_with(".tsv") { b'\t' } else { b',' };
        let mut rdr = csv::ReaderBuilder::new()
            .has_headers(false)
            .flexible(true)
            .delimiter(delim)
            .from_path(path)
            .map_err(|e| e.to_string())?;
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

#[tauri::command]
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
}

#[derive(Serialize)]
pub struct ImportResult {
    pub inserted: usize,
    pub skipped: usize,
}

const ALLOWED_FIELDS: &[&str] = &[
    "name", "priority", "promoter", "venue", "type", "area", "scale", "date", "time", "format",
    "reason", "contact_channel", "email", "email_status", "status", "first_contact", "follow_up",
    "notes", "website", "tags",
];

#[tauri::command]
pub fn import_file(state: State<AppState>, req: ImportRequest) -> Result<ImportResult, String> {
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

    let col_names: Vec<String> = std::iter::once("category".to_string())
        .chain(fields.iter().map(|(f, _)| (*f).to_string()))
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
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let mut inserted = 0usize;
    let mut skipped = 0usize;
    {
        let mut stmt = tx.prepare(&sql).map_err(|e| e.to_string())?;
        for row in &data {
            // require a non-empty name
            let name_col = fields.iter().find(|(f, _)| *f == "name").map(|(_, c)| *c);
            let name_val = name_col.and_then(|c| row.get(c)).map(|s| s.trim());
            if name_val.map(|s| s.is_empty()).unwrap_or(true) {
                skipped += 1;
                continue;
            }
            let mut vals: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
            vals.push(Box::new(req.category.clone()));
            for (field, col) in &fields {
                let v = row.get(*col).cloned().unwrap_or_default();
                let v = v.trim().to_string();
                if v.is_empty() {
                    // status is NOT NULL — fall back to the default rather than inserting NULL
                    if *field == "status" {
                        vals.push(Box::new("to_contact".to_string()));
                    } else {
                        vals.push(Box::new(Option::<String>::None));
                    }
                } else {
                    vals.push(Box::new(v));
                }
            }
            let refs: Vec<&dyn rusqlite::types::ToSql> = vals.iter().map(|b| b.as_ref()).collect();
            stmt.execute(refs.as_slice()).map_err(|e| e.to_string())?;
            inserted += 1;
        }
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(ImportResult { inserted, skipped })
}
