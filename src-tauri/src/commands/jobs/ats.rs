//! Would a screening tool read this CV properly, and does it find what the
//! posting asks for?
//!
//! The check runs on the text extracted from the PDF itself (what an
//! applicant tracking system or an AI screener receives), not on our own
//! data. It is an estimate: no vendor publishes its rules. It measures the
//! things every parser depends on (contact found, sections named, reading
//! order, dates) and the coverage of the posting's key terms.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Check {
    pub key: String,
    pub ok: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Report {
    /// 0 to 100.
    pub score: i64,
    /// "pass", "borderline" or "risk".
    pub verdict: String,
    pub checks: Vec<Check>,
    pub keywords_found: Vec<String>,
    /// Asked by the posting, absent from the CV, but present in the profile:
    /// worth adding.
    pub keywords_addable: Vec<String>,
    /// Asked by the posting and not in the profile at all: a real gap.
    pub keywords_missing: Vec<String>,
    pub coverage: i64,
}

pub struct Expect<'a> {
    pub name: &'a str,
    pub email: &'a str,
    pub phone: &'a str,
    pub title: &'a str,
    /// Employers in the order printed, each with the lines that describe it.
    pub jobs: Vec<(&'a str, Vec<&'a str>)>,
    /// True for a CV this app printed: every employer must be found. False
    /// for a PDF from elsewhere, which may leave some jobs out.
    pub strict: bool,
    pub keywords: &'a [String],
    /// Everything the profile holds, lower case, to tell an addable term from a gap.
    pub profile_text: &'a str,
}

fn squash(s: &str) -> String {
    s.to_lowercase().split_whitespace().collect::<Vec<_>>().join(" ")
}

fn digits(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_digit()).collect()
}

/// A key term is found when its words are there, allowing plural and common
/// endings ("licensing" matches "licence" by its stem "licen").
pub fn has_term(text: &str, term: &str) -> bool {
    let t = squash(term);
    if t.is_empty() {
        return true;
    }
    if text.contains(&t) {
        return true;
    }
    t.split(' ').filter(|w| w.len() > 2 || t.len() <= 3).all(|w| {
        let n = w.chars().count();
        let stem: String = if n > 5 { w.chars().take((n * 3 / 5).max(4)).collect() } else { w.to_string() };
        text.contains(&stem)
    })
}

pub fn check(pdf_text: &str, e: &Expect) -> Report {
    let text = squash(pdf_text);
    let mut checks = Vec::new();
    let mut add = |key: &str, ok: bool, detail: String| checks.push(Check { key: key.to_string(), ok, detail });

    let head: String = text.chars().take(400).collect();
    let name_ok = !e.name.trim().is_empty() && head.contains(&squash(e.name));
    add("name", name_ok, if name_ok { "Nom lu en tête de document".into() } else { "Nom introuvable dans les premières lignes".into() });

    let email_ok = !e.email.trim().is_empty() && text.contains(&e.email.trim().to_lowercase());
    add("email", email_ok, if email_ok { "Adresse email lue en clair".into() } else { "Adresse email illisible ou absente".into() });

    let want = digits(e.phone);
    let phone_ok = want.len() < 6 || digits(pdf_text).contains(&want);
    add("phone", phone_ok, if phone_ok { "Téléphone lu".into() } else { "Téléphone illisible".into() });

    let sections = [("experience", "expérience"), ("skills", "compétences")];
    let missing: Vec<&str> = sections.iter().filter(|(en, fr)| !text.contains(en) && !text.contains(fr)).map(|(en, _)| *en).collect();
    add("sections", missing.is_empty(), if missing.is_empty() { "Rubriques standard reconnues (expérience, compétences)".into() } else { format!("Rubrique non reconnue : {}", missing.join(", ")) });

    // Reading order: employers must come out in the order printed, and the
    // lines that describe a job must come out with it, before the next one.
    let mut last = 0usize;
    let mut order_ok = true;
    let mut lost = Vec::new();
    let mut spots: Vec<(usize, &Vec<&str>)> = Vec::new();
    for (org, lines) in &e.jobs {
        let first: String = squash(org).chars().take(18).collect();
        match text.find(&first) {
            Some(i) => {
                if i < last {
                    order_ok = false;
                }
                last = i;
                spots.push((i, lines));
            }
            None => lost.push(*org),
        }
    }
    let order_good = order_ok && (lost.is_empty() || !e.strict) && !spots.is_empty();
    add("order", order_good, if order_good {
        "Ordre de lecture intact (les expériences sortent dans l'ordre)".into()
    } else if spots.is_empty() || (e.strict && !lost.is_empty()) {
        format!("Employeur non retrouvé dans le texte extrait : {}", lost.join(", "))
    } else {
        "Les expériences sortent dans le désordre (mise en page en colonnes ?)".into()
    });

    let mut sorted = spots.clone();
    sorted.sort_by_key(|(i, _)| *i);
    let (mut seen, mut detached) = (0, 0);
    for (n, (start, lines)) in sorted.iter().enumerate() {
        let end = sorted.get(n + 1).map(|(i, _)| *i).unwrap_or(usize::MAX);
        for line in lines.iter() {
            let frag: String = squash(line).chars().take(28).collect();
            if frag.chars().count() < 12 {
                continue;
            }
            if let Some(at) = text.find(&frag) {
                seen += 1;
                if at < *start || at >= end {
                    detached += 1;
                }
            }
        }
    }
    let attached = detached == 0;
    add("attached", attached, if seen == 0 {
        "Descriptions de poste non vérifiables".into()
    } else if attached {
        "Chaque description sort avec son poste".into()
    } else {
        format!("{detached} ligne(s) de description sortent loin de leur poste : un robot ne sait pas à quel emploi les rattacher")
    });

    let years = regex::Regex::new(r"(?:^|[^0-9])(19|20)[0-9]{2}").unwrap().find_iter(&text).count();
    let want_years = spots.len().max(1);
    add("dates", years >= want_years, if years >= want_years { "Dates lisibles".into() } else { "Dates peu lisibles".into() });

    let garbled = pdf_text.chars().filter(|c| *c == '\u{fffd}' || ('\u{e000}'..='\u{f8ff}').contains(c)).count();
    let ligatures = pdf_text.chars().filter(|c| ('\u{fb00}'..='\u{fb06}').contains(c)).count();
    add("glyphs", garbled == 0 && ligatures == 0, if garbled > 0 {
        format!("{garbled} caractères illisibles (police mal encodée)")
    } else if ligatures > 0 {
        format!("{ligatures} ligature(s) typographiques (fi, fl) : certains robots lisent mal ces mots")
    } else {
        "Aucun caractère illisible".into()
    });

    let spaced = regex::Regex::new(r"(?:\b[A-Za-zÀ-ÿ] ){5,}").unwrap().find_iter(pdf_text).count();
    add("spacing", spaced == 0, if spaced == 0 {
        "Mots entiers (pas de lettres espacées)".into()
    } else {
        format!("{spaced} titre(s) en lettres espacées : un robot lit des lettres isolées, pas le mot")
    });

    let title_words: Vec<String> = squash(e.title).split(' ').filter(|w| w.len() > 2).map(str::to_string).collect();
    let title_ok = !title_words.is_empty() && title_words.iter().all(|w| has_term(&head, w) || has_term(&text, w));
    add("title", title_ok, if title_ok { "L'intitulé du poste figure dans le CV".into() } else { "L'intitulé du poste ne figure pas dans le CV".into() });

    let profile = squash(e.profile_text);
    let (mut found, mut addable, mut gap) = (Vec::new(), Vec::new(), Vec::new());
    for k in e.keywords.iter().map(|k| k.trim()).filter(|k| !k.is_empty()) {
        if has_term(&text, k) {
            found.push(k.to_string());
        } else if has_term(&profile, k) {
            addable.push(k.to_string());
        } else {
            gap.push(k.to_string());
        }
    }
    let total = found.len() + addable.len() + gap.len();
    let coverage = if total == 0 { 100 } else { (found.len() * 100 / total) as i64 };

    // Structure counts for half, the posting's terms for half.
    let weights = [("name", 8), ("email", 10), ("phone", 3), ("sections", 6), ("order", 7), ("attached", 7), ("dates", 3), ("glyphs", 2), ("spacing", 2), ("title", 2)];
    let structure: i64 = weights.iter().map(|(k, w)| if checks.iter().any(|c| c.key == *k && c.ok) { *w } else { 0 }).sum();
    let score = structure + coverage / 2;
    let hard_fail = !email_ok || !name_ok || !order_good || !attached;
    let verdict = if hard_fail || score < 55 {
        "risk"
    } else if score < 75 {
        "borderline"
    } else {
        "pass"
    };
    Report { score, verdict: verdict.to_string(), checks, keywords_found: found, keywords_addable: addable, keywords_missing: gap, coverage }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expect<'a>(keywords: &'a [String]) -> Expect<'a> {
        Expect {
            name: "Alex Martin",
            email: "alex@example.test",
            phone: "+33 6 00 00 00 00",
            title: "Sync Manager",
            jobs: vec![("Acme Records, Lyon", vec!["Licensing and clearance for brands"]), ("Widgets Inc, Houston", vec!["Fifty accounts across the country"])],
            strict: true,
            keywords,
            profile_text: "sync clearance cue sheets negotiation metadata",
        }
    }

    const GOOD: &str = "ALEX MARTIN\nTarget role : Sync Manager\nContact : alex@example.test · +33 6 00 00 00 00\nEXPERIENCE\nFounder\nAcme Records, Lyon · 2017 to present\nLicensing and clearance for brands\nSales manager\nWidgets Inc, Houston · 2016 to 2018\nFifty accounts across the country\nSKILLS\nSync, cue sheets";

    #[test]
    fn clean_cv_passes_and_terms_are_sorted() {
        let kw: Vec<String> = ["Sync licensing", "Clearance", "Metadata", "Salesforce"].iter().map(|s| s.to_string()).collect();
        let r = check(GOOD, &expect(&kw));
        assert!(r.checks.iter().all(|c| c.ok), "{:?}", r.checks);
        assert_eq!(r.keywords_found, vec!["Sync licensing", "Clearance"]);
        assert_eq!(r.keywords_addable, vec!["Metadata"]);
        assert_eq!(r.keywords_missing, vec!["Salesforce"]);
        assert_eq!(r.coverage, 50);
        assert_eq!(r.score, 75);
        assert_eq!(r.verdict, "pass");
    }

    #[test]
    fn columns_read_out_of_order_are_a_risk() {
        let jumbled = "ALEX MARTIN\nalex@example.test +33 6 00 00 00 00\nEXPERIENCE\nSales manager Widgets Inc, Houston 2016\nFounder Acme Records, Lyon 2017\nSKILLS sync";
        let r = check(jumbled, &expect(&[]));
        assert!(!r.checks.iter().find(|c| c.key == "order").unwrap().ok);
        assert_eq!(r.verdict, "risk");
    }

    #[test]
    fn missing_email_is_a_risk_whatever_the_rest() {
        let r = check(&GOOD.replace("alex@example.test", ""), &expect(&[]));
        assert_eq!(r.verdict, "risk");
    }

    #[test]
    fn descriptions_far_from_their_job_and_spaced_titles_are_flagged() {
        let detached = GOOD.replace("Licensing and clearance for brands\n", "").replace("SKILLS", "SKILLS\nLicensing and clearance for brands");
        let r = check(&detached, &expect(&[]));
        assert!(!r.checks.iter().find(|c| c.key == "attached").unwrap().ok);
        assert_eq!(r.verdict, "risk");
        let spaced = GOOD.replace("EXPERIENCE", "E X P E R I E N C E").replace("Licensing", "Ef\u{fb01}cient licensing");
        let r = check(&spaced, &expect(&[]));
        assert!(!r.checks.iter().find(|c| c.key == "spacing").unwrap().ok);
        assert!(!r.checks.iter().find(|c| c.key == "sections").unwrap().ok);
        assert!(!r.checks.iter().find(|c| c.key == "glyphs").unwrap().ok);
    }

    #[test]
    fn term_matching_allows_endings() {
        assert!(has_term("music licence deals", "licensing"));
        assert!(has_term("pitching to supervisors", "Pitch"));
        assert!(!has_term("music licence deals", "touring"));
    }
}
