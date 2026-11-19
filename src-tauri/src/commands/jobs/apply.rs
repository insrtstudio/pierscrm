//! From a posting to an application: the adapted CV as a PDF, its screening
//! check, the email, and after the user's click the send and the follow-up.
//! Nothing here sends on its own: `send` and `send_followup` are only called
//! from a button.

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use tauri::{AppHandle, Manager, State};

use super::ats;
use super::cv::{self, Posting, Profile};
use crate::commands::email::{self, SendResult, ThreadRef};
use crate::commands::residency::claude::{self, extract_json, Llm, LlmRequest, SystemBlock};
use crate::AppState;

/// Model for what a recruiter reads (CV wording, emails).
pub fn writing_model(conn: &Connection) -> String {
    super::setting(conn, "jobs_model").filter(|m| m.starts_with("claude-")).unwrap_or_else(|| "claude-sonnet-5-5".to_string())
}

pub fn jobs_dir(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app.path().app_data_dir().map_err(|e| e.to_string())?.join("jobs"))
}

struct Post {
    title: String,
    company: String,
    location: String,
    description: String,
    lang: String,
    keywords: Vec<String>,
    fit: Value,
    company_brief: Option<String>,
    contact_name: Option<String>,
}

fn load_post(conn: &Connection, id: i64) -> Result<Post, String> {
    conn.query_row(
        "SELECT title, company, location, description, COALESCE(lang, ''), fit, company_brief, contact_name FROM job_posts WHERE id = ?1",
        params![id],
        |r| {
            let fit: Value = r.get::<_, Option<String>>(5)?.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null);
            let keywords = fit["keywords"].as_array().map(|a| a.iter().filter_map(|k| k.as_str().map(str::to_string)).collect()).unwrap_or_default();
            Ok(Post {
                title: r.get(0)?,
                company: r.get(1)?,
                location: r.get(2)?,
                description: r.get(3)?,
                lang: r.get(4)?,
                keywords,
                fit,
                company_brief: r.get(6)?,
                contact_name: r.get(7)?,
            })
        },
    )
    .map_err(|e| e.to_string())
}

// ---------------- Email ----------------

fn mail_schema() -> Value {
    let s = json!({ "type": "string" });
    json!({ "type": "object", "additionalProperties": false, "required": ["subject", "body", "followup"],
        "properties": { "subject": s, "body": s, "followup": s } })
}

fn mail_request(p: &Profile, post: &Post, lang: &str, model: &str, role: &str) -> LlmRequest {
    let rules = "You write a job application email for the candidate below, to be sent with the CV attached, and a short \
follow-up to send a week later if there is no answer. The email is the cover letter: there is no separate letter.\n\
Voice: a real person writing to another, relaxed and direct, sure of the work done without selling it. Humble, never a \
beginner. No corporate formulas ('I am writing to express my interest', 'I am confident that', 'passionate'), no list of \
adjectives, no flattery of the company beyond one concrete, true reason to write to them.\n\
Content of the email, 110 to 170 words: which role, then the two or three facts of the profile that answer what this \
posting asks for (concrete: names, numbers, only from the profile), then one honest line on any practical point the \
recruiter will wonder about (right to work, relocation) when the notes mention one, then a simple close offering a call. \
Mention that the CV is attached. Sign with the first and last name, then the phone number on its own line.\n\
Follow-up, 40 to 70 words: polite, light, no pressure, recalls the role and the date is not needed, offers again to talk.\n\
Form: plain text, short paragraphs separated by a blank line. Never use a colon. Never use an em dash or a semicolon. \
No bullet points. Greet the named contact by first name when one is given, otherwise a neutral greeting in the \
language. Subject: role and name, plain, at most 70 characters, no colon.\n\
Never state a fact that is not in the profile. Write in the language asked. Answer with JSON only.";
    LlmRequest {
        model: model.to_string(),
        max_tokens: 1200,
        system: vec![
            SystemBlock { text: rules.to_string(), cache: false },
            SystemBlock { text: format!("CANDIDATE\nName: {}\nPhone: {}\n{}", p.identity.name, p.identity.phone, super::brief::profile_digest(p)), cache: true },
        ],
        user: format!(
            "Language: {}\nRole: {}\nCompany: {}\nPlace: {}\nContact named in the posting: {}\nAbout the company: {}\nWhere the profile answers the posting: {}\nWhat the posting asks that the profile does not show (do not claim these): {}\nPractical note: {}\n\nPOSTING\n{}",
            if lang == "fr" { "French" } else { "English" },
            role,
            post.company,
            post.location,
            post.contact_name.as_deref().unwrap_or("none"),
            post.company_brief.as_deref().unwrap_or("unknown"),
            post.fit["strengths"],
            post.fit["gaps"],
            post.fit["work_right"].as_str().unwrap_or("none"),
            post.description.chars().take(5000).collect::<String>()
        ),
        schema: Some(mail_schema()),
    }
}

/// The email voice bans colons and long dashes; the model mostly complies,
/// this makes it certain.
pub fn voice(s: &str) -> String {
    let s = claude::no_dashes(s).replace(" : ", ", ").replace(": ", ", ").replace(" ;", ",").replace(';', ",");
    s.lines().map(str::trim_end).collect::<Vec<_>>().join("\n").trim().to_string()
}

/// Email used when no AI is available: short, true, and plainly a draft.
pub fn template_mail(p: &Profile, title: &str, lang: &str) -> (String, String, String) {
    let name = p.identity.name.trim();
    if lang == "fr" {
        (
            format!("Candidature {title}, {name}"),
            format!("Bonjour,\n\nJe vous écris au sujet du poste de {title}. Mon CV est en pièce jointe.\n\nJe serais ravi d'en parler de vive voix si mon profil vous intéresse.\n\n{name}\n{}", p.identity.phone),
            format!("Bonjour,\n\nJe me permets de revenir vers vous au sujet du poste de {title}. Je reste disponible pour en parler quand vous voulez.\n\n{name}"),
        )
    } else {
        (
            format!("Application for {title}, {name}"),
            format!("Hello,\n\nI am writing about the {title} role. My CV is attached.\n\nI would be glad to talk it through if my profile is of interest.\n\n{name}\n{}", p.identity.phone),
            format!("Hello,\n\nI wanted to follow up on my application for the {title} role. Happy to talk whenever suits you.\n\n{name}"),
        )
    }
}

// ---------------- Prepare ----------------

fn profile_text(p: &Profile) -> String {
    serde_json::to_string(p).unwrap_or_default().to_lowercase()
}

/// Screening check of a PDF against the profile and, when given, a posting.
/// `doc` is the CV this app printed; None for a PDF from elsewhere, checked
/// against the profile's own jobs and lines.
pub fn check_pdf(p: &Profile, path: &Path, title: &str, keywords: &[String], doc: Option<&cv::CvDoc>) -> Result<ats::Report, String> {
    let text = cv::pdf_text(path).ok_or("Texte du PDF illisible : un robot ne lirait rien non plus (PDF image ou protégé).")?;
    let ptext = profile_text(p);
    let en: Vec<Vec<&str>> = p.experience.iter().map(|e| e.bullets.iter().flat_map(|b| [b.en.as_str(), b.fr.as_str()]).collect()).collect();
    let jobs: Vec<(&str, Vec<&str>)> = match doc {
        Some(d) => d.experience.iter().map(|e| (e.org.as_str(), e.bullets.iter().map(String::as_str).collect())).collect(),
        None => p.experience.iter().zip(en).map(|(e, lines)| (e.org.as_str(), lines)).collect(),
    };
    let title = &doc.map(|d| d.target.clone()).filter(|t| !t.is_empty()).unwrap_or_else(|| cv::headline(title, "", ""));
    Ok(ats::check(&text, &ats::Expect { name: &p.identity.name, email: &p.identity.email, phone: &p.identity.phone, title, jobs, strict: doc.is_some(), keywords, profile_text: &ptext }))
}

/// Build the application of one posting: CV, check, email. Replaces a draft,
/// never touches an application already sent.
pub fn prepare(conn: &Connection, dir: &Path, llm: Option<&dyn Llm>, id: i64) -> Result<(), String> {
    let p = cv::load_profile(conn).ok_or("Profil non renseigné (onglet Emploi, Réglages).")?;
    let state: Option<String> = conn
        .query_row("SELECT state FROM job_applications WHERE post_id = ?1", params![id], |r| r.get(0))
        .optional()
        .map_err(|e| e.to_string())?;
    if matches!(state.as_deref(), Some("sent") | Some("manual")) {
        return Err("Candidature déjà partie.".into());
    }
    let post = load_post(conn, id)?;
    let lang = if post.lang == "fr" { "fr" } else { "en" };
    let model = writing_model(conn);
    let posting = Posting { title: &post.title, company: &post.company, location: &post.location, description: &post.description, lang, keywords: &post.keywords };
    let mut log = |m: &str, u: &claude::Usage| claude::log_usage(conn, "jobs_cv", None, m, u);
    let (doc, mut warnings) = cv::tailor(&p, &posting, llm, &model, &mut log);

    let (cv_path, report) = match cv::print_pdf(&doc, &dir.join(id.to_string())) {
        Ok(path) => {
            let report = match check_pdf(&p, &path, &post.title, &post.keywords, Some(&doc)) {
                Ok(r) => Some(r),
                Err(e) => {
                    warnings.push(e);
                    None
                }
            };
            (Some(path.display().to_string()), report)
        }
        Err(e) => {
            warnings.push(e);
            (None, None)
        }
    };

    let role = if doc.target.is_empty() { post.title.as_str() } else { doc.target.as_str() };
    let (subject, body, followup) = match llm.map(|l| l.complete(&mail_request(&p, &post, lang, &model, role))) {
        Some(Ok(resp)) => {
            claude::log_usage(conn, "jobs_mail", None, &resp.model, &resp.usage);
            match extract_json(&resp.text) {
                Some(v) if v["body"].as_str().map(|b| b.len() > 80).unwrap_or(false) => (
                    voice(v["subject"].as_str().unwrap_or("")).replace('\n', " "),
                    voice(v["body"].as_str().unwrap_or("")),
                    voice(v["followup"].as_str().unwrap_or("")),
                ),
                _ => template_mail(&p, role, lang),
            }
        }
        Some(Err(e)) => {
            warnings.push(format!("Email non rédigé par l'IA : {e}"));
            template_mail(&p, role, lang)
        }
        None => template_mail(&p, role, lang),
    };

    conn.execute(
        "INSERT INTO job_applications (post_id, cv_json, cv_path, cv_warnings, ats, subject, body, followup_body, state)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'draft')
         ON CONFLICT(post_id) DO UPDATE SET cv_json = excluded.cv_json, cv_path = excluded.cv_path, cv_warnings = excluded.cv_warnings,
            ats = excluded.ats, subject = excluded.subject, body = excluded.body, followup_body = excluded.followup_body",
        params![id, serde_json::to_string(&doc).ok(), cv_path, serde_json::to_string(&warnings).ok(),
            report.and_then(|r| serde_json::to_string(&r).ok()), subject, body, followup],
    )
    .map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE job_posts SET status = CASE WHEN status = 'new' THEN 'shortlisted' ELSE status END,
            next_action = 'Relire et envoyer la candidature', updated_at = datetime('now') WHERE id = ?1",
        params![id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

// ---------------- Send ----------------

fn due_date(conn: &Connection) -> String {
    (chrono::Local::now() + chrono::Duration::days(super::followup_days(conn))).format("%Y-%m-%d").to_string()
}

fn mark_applied(conn: &Connection, id: i64, due: &str) {
    let _ = conn.execute(
        "UPDATE job_posts SET status = CASE WHEN status IN ('new', 'shortlisted') THEN 'applied' ELSE status END,
            next_action = 'Relancer si pas de réponse', next_action_at = ?2, updated_at = datetime('now') WHERE id = ?1",
        params![id, due],
    );
}

pub fn send(pool: &crate::db::DbPool, id: i64, to: &str, subject: &str, body: &str) -> Result<SendResult, String> {
    let (state, cv_path): (String, Option<String>) = {
        let conn = pool.get().map_err(|e| e.to_string())?;
        conn.query_row("SELECT state, cv_path FROM job_applications WHERE post_id = ?1", params![id], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()
            .map_err(|e| e.to_string())?
            .ok_or("Candidature non préparée.")?
    };
    if state != "draft" {
        return Err("Candidature déjà partie.".into());
    }
    if subject.trim().is_empty() || body.trim().is_empty() {
        return Err("Objet ou message vide.".into());
    }
    let path = cv_path.ok_or("CV non généré : impossible d'envoyer sans pièce jointe.")?;
    let bytes = std::fs::read(&path).map_err(|e| format!("CV introuvable ({path}) : {e}"))?;
    let name = Path::new(&path).file_name().and_then(|n| n.to_str()).unwrap_or("CV.pdf").to_string();
    let (res, email_id, message_id) = email::send_personal(pool, to, subject.trim(), body.trim(), None, &[(name, bytes)])?;
    if res.ok {
        let conn = pool.get().map_err(|e| e.to_string())?;
        let due = due_date(&conn);
        conn.execute(
            "UPDATE job_applications SET state = 'sent', subject = ?2, body = ?3, email_id = ?4, message_id = ?5,
                sent_at = datetime('now'), followup_due = ?6 WHERE post_id = ?1",
            params![id, subject.trim(), body.trim(), email_id, message_id, due],
        )
        .map_err(|e| e.to_string())?;
        let _ = conn.execute("UPDATE job_posts SET apply_email = COALESCE(apply_email, ?2), apply_kind = 'email' WHERE id = ?1", params![id, to.trim().to_lowercase()]);
        mark_applied(&conn, id, &due);
    }
    Ok(res)
}

/// The application was filed by hand on a form: follow it like a sent one.
pub fn mark_manual(conn: &Connection, id: i64) -> Result<(), String> {
    let due = due_date(conn);
    conn.execute(
        "INSERT INTO job_applications (post_id, state, sent_at, followup_due) VALUES (?1, 'manual', datetime('now'), ?2)
         ON CONFLICT(post_id) DO UPDATE SET state = 'manual', sent_at = datetime('now'), followup_due = ?2
         WHERE job_applications.state = 'draft'",
        params![id, due],
    )
    .map_err(|e| e.to_string())?;
    mark_applied(conn, id, &due);
    Ok(())
}

pub fn re_subject(s: &str) -> String {
    let t = s.trim();
    if t.to_lowercase().starts_with("re:") || t.to_lowercase().starts_with("re ") { t.to_string() } else { format!("Re: {t}") }
}

pub fn send_followup(pool: &crate::db::DbPool, id: i64, to: &str, body: &str) -> Result<SendResult, String> {
    let (state, subject, message_id, done): (String, String, Option<String>, Option<String>) = {
        let conn = pool.get().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT state, subject, message_id, followup_sent_at FROM job_applications WHERE post_id = ?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .ok_or("Aucune candidature à relancer.")?
    };
    if !matches!(state.as_str(), "sent" | "manual") {
        return Err("La candidature n'est pas encore partie.".into());
    }
    if done.is_some() {
        return Err("Relance déjà envoyée.".into());
    }
    if body.trim().is_empty() {
        return Err("Message vide.".into());
    }
    let subject = if subject.trim().is_empty() {
        let conn = pool.get().map_err(|e| e.to_string())?;
        conn.query_row("SELECT title FROM job_posts WHERE id = ?1", params![id], |r| r.get::<_, String>(0)).map_err(|e| e.to_string())?
    } else {
        re_subject(&subject)
    };
    let thread = message_id.map(|m| ThreadRef { in_reply_to: Some(m.clone()), references: vec![m] });
    let (res, email_id, _) = email::send_personal(pool, to, &subject, body.trim(), thread.as_ref(), &[])?;
    if res.ok {
        let conn = pool.get().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE job_applications SET followup_email_id = ?2, followup_sent_at = datetime('now'), followup_body = ?3 WHERE post_id = ?1",
            params![id, email_id, body.trim()],
        )
        .map_err(|e| e.to_string())?;
        let _ = conn.execute(
            "UPDATE job_posts SET next_action = 'Relance envoyée, attendre la réponse', next_action_at = NULL, updated_at = datetime('now') WHERE id = ?1",
            params![id],
        );
    }
    Ok(res)
}

// ---------------- Commands ----------------

#[tauri::command]
pub async fn job_prepare(app: AppHandle, state: State<'_, AppState>, id: i64) -> Result<(), String> {
    let pool = state.pool.clone();
    let dir = jobs_dir(&app)?;
    tauri::async_runtime::spawn_blocking(move || {
        let conn = pool.get().map_err(|e| e.to_string())?;
        super::daily::complete_one(&conn, id);
        let llm = claude::client(&conn);
        prepare(&conn, &dir, llm.as_deref(), id)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(async)]
pub fn job_save_draft(state: State<'_, AppState>, id: i64, subject: String, body: String, followup: String) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE job_applications SET subject = CASE WHEN state = 'draft' THEN ?2 ELSE subject END,
            body = CASE WHEN state = 'draft' THEN ?3 ELSE body END, followup_body = ?4 WHERE post_id = ?1",
        params![id, subject.trim(), body.trim(), followup.trim()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn job_send(state: State<'_, AppState>, id: i64, to: String, subject: String, body: String) -> Result<SendResult, String> {
    let pool = state.pool.clone();
    tauri::async_runtime::spawn_blocking(move || send(&pool, id, &to, &subject, &body)).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn job_send_followup(state: State<'_, AppState>, id: i64, to: String, body: String) -> Result<SendResult, String> {
    let pool = state.pool.clone();
    tauri::async_runtime::spawn_blocking(move || send_followup(&pool, id, &to, &body)).await.map_err(|e| e.to_string())?
}

#[tauri::command(async)]
pub fn job_mark_applied(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    mark_manual(&conn, id)
}

/// Open the generated CV (or show it in the Finder, to drag it into a form).
#[tauri::command(async)]
pub fn job_open_cv(state: State<'_, AppState>, id: i64, reveal: bool) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let path: Option<String> = conn
        .query_row("SELECT cv_path FROM job_applications WHERE post_id = ?1", params![id], |r| r.get(0))
        .optional()
        .map_err(|e| e.to_string())?
        .flatten();
    let path = path.filter(|p| Path::new(p).exists()).ok_or("CV non généré.")?;
    let mut cmd = std::process::Command::new("open");
    if reveal {
        cmd.arg("-R");
    }
    cmd.arg(&path).spawn().map_err(|e| e.to_string())?;
    Ok(())
}

/// Screening check of any PDF (an existing CV), against a posting when given.
#[tauri::command(async)]
pub fn jobs_check_pdf(state: State<'_, AppState>, path: String, post_id: Option<i64>) -> Result<ats::Report, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let p = cv::load_profile(&conn).ok_or("Profil non renseigné (onglet Emploi, Réglages).")?;
    let (title, keywords) = match post_id {
        Some(id) => {
            let post = load_post(&conn, id)?;
            (post.title, post.keywords)
        }
        None => (String::new(), Vec::new()),
    };
    check_pdf(&p, Path::new(&path), &title, &keywords, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_has_no_colon_no_long_dash() {
        let v = voice("Hello Dana,\n\nOne thing: I ran two labels — and a library; happy to talk.  \n");
        assert!(!v.contains(':') && !v.contains('—') && !v.contains(';'), "{v}");
        assert!(v.starts_with("Hello Dana,\n\nOne thing, I ran"));
    }

    #[test]
    fn manual_filing_is_followed_like_a_send() {
        let conn = crate::commands::jobs::test_db();
        conn.execute("INSERT INTO job_posts (source_key, title) VALUES ('k', 'Sync Manager')", []).unwrap();
        mark_manual(&conn, 1).unwrap();
        let j = crate::commands::jobs::one(&conn, 1).unwrap();
        assert_eq!((j.status.as_str(), j.app_state.as_deref()), ("applied", Some("manual")));
        assert!(j.followup_due.is_some() && j.next_action_at == j.followup_due);
    }

    #[test]
    fn template_mail_follows_the_language() {
        let p = cv::sample_profile();
        let (s, b, f) = template_mail(&p, "Sync Manager", "fr");
        assert!(s.starts_with("Candidature Sync Manager") && b.contains("pièce jointe") && f.contains("revenir vers vous"));
        assert!(!b.contains(':'));
        assert_eq!(re_subject("Application"), "Re: Application");
        assert_eq!(re_subject("Re: Application"), "Re: Application");
    }
}

#[cfg(test)]
mod real_pdf {
    use super::*;

    /// Prints a real PDF from a profile file and checks it, to see the chain
    /// work on this machine: `JOBS_PROFILE=/path/profile.json JOBS_OUT=/dir
    /// cargo test --lib real_pdf -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn print_and_check() {
        let p: Profile = serde_json::from_str(&std::fs::read_to_string(std::env::var("JOBS_PROFILE").unwrap()).unwrap()).unwrap();
        let out = PathBuf::from(std::env::var("JOBS_OUT").unwrap());
        let lang = std::env::var("JOBS_LANG").unwrap_or_else(|_| "en".into());
        let kw: Vec<String> = ["sync licensing", "clearance", "cue sheets", "music supervisors", "Salesforce", "metadata"].iter().map(|s| s.to_string()).collect();
        // JOBS_CHOICES: a JSON file shaped like the AI's answer, to see a tailored CV without calling the AI.
        let doc = match std::env::var("JOBS_CHOICES") {
            Ok(f) => {
                let choices: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(f).unwrap()).unwrap();
                let post = Posting { title: "Sync Manager", company: "", location: "London", description: "", lang: &lang, keywords: &kw };
                let (doc, warnings) = cv::apply_choices(&p, &post, &choices);
                println!("warnings: {warnings:#?}");
                cv::ordered(doc)
            }
            Err(_) => cv::ordered(cv::base_doc(&p, &lang, "Sync Manager", "London")),
        };
        let path = cv::print_pdf(&doc, &out).unwrap();
        let r = check_pdf(&p, &path, "Sync Manager", &kw, Some(&doc)).unwrap();
        println!("{}\n{}", path.display(), serde_json::to_string_pretty(&r).unwrap());
        if let Ok(other) = std::env::var("JOBS_OTHER") {
            let r = check_pdf(&p, Path::new(&other), "Sync Manager", &kw, None).unwrap();
            println!("OTHER {}\n{}", other, serde_json::to_string_pretty(&r).unwrap());
        }
    }
}
#[cfg(test)]
mod dump {
    #[test]
    #[ignore]
    fn dump_text() {
        let t = super::cv::pdf_text(std::path::Path::new(&std::env::var("JOBS_PDF").unwrap())).unwrap();
        println!("{}", t.chars().take(2600).collect::<String>());
    }
}
