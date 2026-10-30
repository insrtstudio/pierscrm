//! Booking documents (contract, rider / technical sheet, invoice) as PDF, in
//! French or English, built from the offer, the contact and the artist, with
//! Thibault (the email signature) as booker. Files go to
//! `<app data>/documents/offer-<id>/` and are attached to an email draft that
//! opens in the compose window: nothing is ever sent from here.
//!
//! Rendering (`render_*`) is pure (data in, bytes out) so it is tested
//! without a window; the commands only gather data and write the file.

use std::path::{Path, PathBuf};

use chrono::{Datelike, NaiveDate};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};

use super::advancing;
use super::money::{self, Invoice};
use super::pdf::{self, Doc, ACCENT, INK, PANEL, RULE, SUBTLE};
use super::{get, parse_day, today, Offer};
use crate::AppState;

// ---------------------------------------------------------------------------
// Data
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct Party {
    pub name: String,
    pub organisation: Option<String>,
    pub promoter: Option<String>,
    pub email: Option<String>,
    pub city: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ArtistInfo {
    pub name: String,
    pub real_name: Option<String>,
    pub tech_rider: Option<String>,
    pub mix_url: Option<String>,
    pub website: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Booker {
    pub name: String,
    pub company: String,
    pub email: Option<String>,
    pub phone: Option<String>,
}

/// Seller details for invoices (settings `invoice_issuer`, `invoice_bank`,
/// `invoice_vat_note`), free multi-line text typed once by the user.
#[derive(Debug, Clone, Default)]
pub struct Issuer {
    pub block: Option<String>,
    pub bank: Option<String>,
    pub vat_note: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct DocData {
    pub offer: Offer,
    pub contact: Party,
    pub artist: ArtistInfo,
    pub booker: Booker,
    pub issuer: Issuer,
}

fn setting(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM settings WHERE key=?1", params![key], |r| r.get::<_, String>(0))
        .optional()
        .ok()
        .flatten()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn nz(s: Option<String>) -> Option<String> {
    s.map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

#[derive(Deserialize, Default)]
struct Signature {
    #[serde(default)]
    name: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    phone: String,
    #[serde(default)]
    booking_email: String,
}

pub(crate) fn booker(conn: &Connection) -> Booker {
    let sig: Signature = setting(conn, "email_signature")
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let from_email = setting(conn, "smtp")
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get("from_email").and_then(|e| e.as_str()).map(str::to_string));
    Booker {
        name: nz(Some(sig.name)).unwrap_or_else(|| "Thibault".into()),
        company: nz(Some(sig.label)).unwrap_or_else(|| "Insrt.Studio".into()),
        email: nz(Some(sig.booking_email)).or(nz(from_email)),
        phone: nz(Some(sig.phone)),
    }
}

pub(crate) fn load(conn: &Connection, offer_id: i64) -> Result<DocData, String> {
    let offer = get(conn, offer_id)?;
    let contact = match offer.contact_id {
        Some(cid) => conn
            .query_row(
                "SELECT c.name, o.name, c.promoter, c.email, c.area FROM contacts c
                 LEFT JOIN organisations o ON o.id = c.organisation_id WHERE c.id=?1",
                params![cid],
                |r| {
                    Ok(Party {
                        name: r.get(0)?,
                        organisation: nz(r.get(1)?),
                        promoter: nz(r.get(2)?),
                        email: nz(r.get(3)?),
                        city: nz(r.get(4)?),
                    })
                },
            )
            .optional()
            .map_err(|e| e.to_string())?
            .unwrap_or_default(),
        None => Party::default(),
    };
    let artist = match offer.artist_id {
        Some(aid) => conn
            .query_row(
                "SELECT name, real_name, tech_rider, mix_url, website FROM artists WHERE id=?1",
                params![aid],
                |r| {
                    Ok(ArtistInfo {
                        name: r.get(0)?,
                        real_name: nz(r.get(1)?),
                        tech_rider: nz(r.get(2)?),
                        mix_url: nz(r.get(3)?),
                        website: nz(r.get(4)?),
                    })
                },
            )
            .optional()
            .map_err(|e| e.to_string())?
            .unwrap_or_default(),
        None => ArtistInfo::default(),
    };
    Ok(DocData {
        offer,
        contact,
        artist,
        booker: booker(conn),
        issuer: Issuer {
            block: setting(conn, "invoice_issuer"),
            bank: setting(conn, "invoice_bank"),
            vat_note: setting(conn, "invoice_vat_note"),
        },
    })
}

// ---------------------------------------------------------------------------
// Wording
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Fr,
    En,
}

impl Lang {
    pub fn of(s: &str) -> Lang {
        if s.starts_with("en") { Lang::En } else { Lang::Fr }
    }
    fn pick<'a>(self, fr: &'a str, en: &'a str) -> &'a str {
        match self {
            Lang::Fr => fr,
            Lang::En => en,
        }
    }
}

/// "1 200,00 €" / "€1,200.00"; other currencies by code.
pub fn money_fmt(v: f64, currency: &str, lang: Lang) -> String {
    let cents = (v.abs() * 100.0).round() as i64;
    let (int, dec) = (cents / 100, cents % 100);
    let digits = int.to_string();
    let sep = lang.pick(" ", ",");
    let mut grouped = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            grouped.push_str(sep);
        }
        grouped.push(c);
    }
    let sign = if v < 0.0 { "-" } else { "" };
    let cur = currency.trim().to_uppercase();
    match (lang, cur.as_str()) {
        (Lang::Fr, "EUR") => format!("{sign}{grouped},{dec:02} €"),
        (Lang::Fr, c) => format!("{sign}{grouped},{dec:02} {c}"),
        (Lang::En, "EUR") => format!("{sign}€{grouped}.{dec:02}"),
        (Lang::En, "GBP") => format!("{sign}£{grouped}.{dec:02}"),
        (Lang::En, c) => format!("{sign}{c} {grouped}.{dec:02}"),
    }
}

const MONTHS_FR: [&str; 12] = [
    "janvier", "février", "mars", "avril", "mai", "juin", "juillet", "août", "septembre", "octobre", "novembre", "décembre",
];
const MONTHS_EN: [&str; 12] = [
    "January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December",
];
const DAYS_FR: [&str; 7] = ["lundi", "mardi", "mercredi", "jeudi", "vendredi", "samedi", "dimanche"];
const DAYS_EN: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];

pub fn date_fmt(d: NaiveDate, lang: Lang, weekday: bool) -> String {
    let m = d.month0() as usize;
    let w = d.weekday().num_days_from_monday() as usize;
    match lang {
        Lang::Fr => {
            let day = if d.day() == 1 { "1er".to_string() } else { d.day().to_string() };
            let base = format!("{day} {} {}", MONTHS_FR[m], d.year());
            if weekday { format!("{} {base}", DAYS_FR[w]) } else { base }
        }
        Lang::En => {
            let base = format!("{} {} {}", d.day(), MONTHS_EN[m], d.year());
            if weekday { format!("{}, {base}", DAYS_EN[w]) } else { base }
        }
    }
}

fn day_str(s: &Option<String>, lang: Lang, weekday: bool) -> Option<String> {
    parse_day(s).map(|d| date_fmt(d, lang, weekday))
}

fn fee_line(o: &Offer, lang: Lang) -> String {
    let amount = o.fee_amount.filter(|v| *v > 0.0).map(|v| money_fmt(v, &o.currency, lang));
    let terms = nz(o.deal_terms.clone());
    match o.fee_type.as_str() {
        "versus" => {
            let g = amount.unwrap_or_else(|| lang.pick("à définir", "to be agreed").into());
            match terms {
                Some(t) => format!("{} {g} {} {t}", lang.pick("Garantie de", "Guarantee of"), lang.pick("versus", "versus")),
                None => format!("{} {g} (versus)", lang.pick("Garantie de", "Guarantee of")),
            }
        }
        "door" => {
            let t = terms.unwrap_or_else(|| lang.pick("recette de la billetterie, modalités à préciser", "door takings, terms to be agreed").into());
            match amount {
                Some(g) => format!("Door deal : {t} ({} {g})", lang.pick("minimum garanti", "guaranteed minimum")),
                None => format!("Door deal : {t}"),
            }
        }
        _ => match amount {
            Some(a) => format!("{} {a}", lang.pick("Cachet forfaitaire de", "Flat fee of")),
            None => lang.pick("Cachet à définir", "Fee to be agreed").into(),
        },
    }
}

fn travel_line(o: &Offer, lang: Lang) -> Option<String> {
    let who = match o.travel_by.as_deref() {
        Some("promoter") => Some(lang.pick("à la charge de l'organisateur", "covered by the promoter")),
        Some("artist") => Some(lang.pick("à la charge de l'artiste", "covered by the artist")),
        Some("split") => Some(lang.pick("partagé entre les parties", "shared between the parties")),
        _ => None,
    };
    let route = match (nz(o.travel_from.clone()), nz(o.travel_to.clone())) {
        (Some(f), Some(t)) => Some(format!("{f} > {t}")),
        (Some(f), None) => Some(f),
        (None, Some(t)) => Some(t),
        _ => None,
    };
    match (route, who) {
        (Some(r), Some(w)) => Some(format!("{r}, {w}")),
        (Some(r), None) => Some(r),
        (None, Some(w)) => Some(capitalize(w)),
        _ => None,
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn set_line(o: &Offer, lang: Lang) -> Option<String> {
    let len = o.set_length.filter(|l| *l > 0).map(|l| {
        let (h, m) = (l / 60, l % 60);
        match (h, m) {
            (0, m) => format!("{m} min"),
            (h, 0) => format!("{h} h"),
            (h, m) => format!("{h} h {m:02}"),
        }
    });
    let start = nz(o.set_time.clone());
    let end = super::set_end(&o.set_time, o.set_length);
    match (start, end, len) {
        (Some(s), Some(e), Some(l)) => Some(format!("{s} - {e} ({l})")),
        (Some(s), _, Some(l)) => Some(format!("{s} ({l})")),
        (Some(s), _, None) => Some(s),
        (None, _, Some(l)) => Some(format!("{} {l}", lang.pick("Set de", "Set of"))),
        _ => None,
    }
}

fn place_line(o: &Offer) -> Option<String> {
    let parts: Vec<String> = [o.venue.clone(), o.city.clone()].into_iter().flatten().filter(|s| !s.trim().is_empty()).collect();
    (!parts.is_empty()).then(|| parts.join(", "))
}

fn organiser_name(d: &DocData) -> String {
    d.contact.organisation.clone().unwrap_or_else(|| {
        if d.contact.name.trim().is_empty() { "-".into() } else { d.contact.name.clone() }
    })
}

fn artist_name(d: &DocData) -> String {
    if d.artist.name.trim().is_empty() { "Artiste".into() } else { d.artist.name.clone() }
}

fn footer(d: &DocData) -> String {
    let mut parts = vec![format!("{} · {}", d.booker.company, d.booker.name)];
    if let Some(e) = &d.booker.email {
        parts.push(e.clone());
    }
    if let Some(p) = &d.booker.phone {
        parts.push(p.clone());
    }
    parts.join(" · ")
}

// ---------------------------------------------------------------------------
// Layout helpers
// ---------------------------------------------------------------------------

fn header(doc: &mut Doc, kicker: &str, title: &str, subtitle: Option<&str>) {
    doc.rect(0.0, 0.0, pdf::PAGE_W, 6.0, ACCENT);
    let m = doc.margin;
    doc.y = 64.0;
    doc.text_at(m, doc.y, 8.0, true, ACCENT, &kicker.to_uppercase());
    doc.y += 26.0;
    doc.text_at(m, doc.y, 22.0, true, INK, title);
    if let Some(s) = subtitle {
        doc.y += 18.0;
        doc.text_at(m, doc.y, 10.0, false, SUBTLE, s);
    }
    doc.y += 14.0;
    doc.hline(m, pdf::PAGE_W - m, doc.y, INK, 1.2);
    doc.y += 6.0;
}

fn section(doc: &mut Doc, title: &str) {
    doc.ensure(60.0);
    doc.gap(16.0);
    let m = doc.margin;
    doc.text_at(m, doc.y, 8.5, true, ACCENT, &title.to_uppercase());
    doc.gap(5.0);
    doc.hline(m, pdf::PAGE_W - m, doc.y, RULE, 0.6);
    doc.gap(2.0);
}

const LABEL_W: f32 = 140.0;

fn kv(doc: &mut Doc, label: &str, value: Option<String>) {
    let Some(v) = value.filter(|v| !v.trim().is_empty()) else { return };
    let m = doc.margin;
    let w = doc.content_width() - LABEL_W;
    let lines = pdf::wrap(&v, 10.0, false, w);
    doc.ensure(14.5 * lines.len().min(4) as f32 + 4.0);
    let top = doc.y;
    doc.text_at(m, top + 14.0 - 4.0, 8.5, false, SUBTLE, label);
    doc.y = top;
    doc.paragraph(m + LABEL_W, w, 10.0, false, INK, &v);
    doc.gap(3.0);
}

fn para(doc: &mut Doc, text: &str) {
    let m = doc.margin;
    let w = doc.content_width();
    doc.paragraph(m, w, 9.5, false, INK, text);
    doc.gap(3.0);
}

fn numbered(doc: &mut Doc, n: usize, title: &str, text: &str) {
    doc.ensure(48.0);
    let m = doc.margin;
    let w = doc.content_width();
    doc.gap(4.0);
    doc.paragraph(m, w, 9.5, true, INK, &format!("{n}. {title}"));
    doc.paragraph(m, w, 9.5, false, INK, text);
}

// ---------------------------------------------------------------------------
// Documents
// ---------------------------------------------------------------------------

pub fn render_contract(d: &DocData, lang: Lang) -> Vec<u8> {
    let o = &d.offer;
    let artist = artist_name(d);
    let org = organiser_name(d);
    let date = day_str(&o.date, lang, true);
    let mut doc = Doc::new(&format!("{} {artist}", lang.pick("Contrat d'engagement", "Booking contract")));
    doc.footer = footer(d);
    let sub = [Some(artist.clone()), place_line(o), date.clone()].into_iter().flatten().collect::<Vec<_>>().join(" · ");
    header(
        &mut doc,
        &format!("{} · Booking", d.booker.company),
        lang.pick("Contrat d'engagement", "Booking contract"),
        Some(&sub),
    );

    section(&mut doc, lang.pick("Parties", "Parties"));
    let mut organiser = vec![org.clone()];
    if d.contact.organisation.is_some() && !d.contact.name.trim().is_empty() {
        organiser.push(d.contact.name.clone());
    }
    if let Some(p) = &d.contact.promoter {
        organiser.push(p.clone());
    }
    if let Some(e) = &d.contact.email {
        organiser.push(e.clone());
    }
    kv(&mut doc, lang.pick("L'Organisateur", "The Promoter"), Some(organiser.join("\n")));
    let mut artist_lines = vec![match &d.artist.real_name {
        Some(r) => format!("{artist} ({r})"),
        None => artist.clone(),
    }];
    artist_lines.push(format!(
        "{} {} ({}){}",
        lang.pick("représenté par", "represented by"),
        d.booker.name,
        d.booker.company,
        d.booker.email.as_ref().map(|e| format!(", {e}")).unwrap_or_default()
    ));
    kv(&mut doc, lang.pick("L'Artiste", "The Artist"), Some(artist_lines.join("\n")));

    section(&mut doc, lang.pick("Engagement", "Engagement"));
    kv(&mut doc, lang.pick("Date", "Date"), date.clone());
    kv(&mut doc, lang.pick("Lieu", "Venue"), place_line(o));
    kv(&mut doc, lang.pick("Horaire du set", "Set time"), set_line(o, lang));
    kv(&mut doc, lang.pick("Prestation", "Performance"), Some(format!("DJ set, {artist}")));

    section(&mut doc, lang.pick("Rémunération", "Fee"));
    kv(&mut doc, lang.pick("Cachet", "Fee"), Some(fee_line(o, lang)));
    if o.deposit_amount() > 0.0 {
        kv(
            &mut doc,
            lang.pick("Acompte", "Deposit"),
            Some(format!(
                "{} % : {}{}",
                o.deposit_pct.round(),
                money_fmt(o.deposit_amount(), &o.currency, lang),
                day_str(&o.deposit_due, lang, false)
                    .map(|v| format!(", {} {v}", lang.pick("au plus tard le", "due by")))
                    .unwrap_or_else(|| lang.pick(", à la signature", ", on signature").into())
            )),
        );
    }
    kv(
        &mut doc,
        lang.pick("Solde", "Balance"),
        Some(format!(
            "{}{}",
            money_fmt(o.balance_amount(), &o.currency, lang),
            day_str(&o.balance_due, lang, false)
                .or(date.clone())
                .map(|v| format!(", {} {v}", lang.pick("au plus tard le", "due by")))
                .unwrap_or_default()
        )),
    );
    kv(
        &mut doc,
        lang.pick("Règlement", "Payment"),
        Some(lang.pick("Par virement bancaire, sur facture.", "By bank transfer, on invoice.").into()),
    );

    section(&mut doc, lang.pick("Accueil", "Hospitality"));
    kv(&mut doc, lang.pick("Transport", "Travel"), travel_line(o, lang));
    kv(&mut doc, lang.pick("Hébergement", "Hotel"), nz(o.hotel.clone()));
    kv(&mut doc, lang.pick("Repas et boissons", "Meals and drinks"), nz(o.meals.clone()));
    kv(
        &mut doc,
        lang.pick("Liste d'invités", "Guest list"),
        o.guest_list.filter(|g| *g > 0).map(|g| format!("{g} {}", lang.pick("personnes", "people"))),
    );

    section(&mut doc, lang.pick("Conditions", "Terms"));
    let deposit_clause = if o.deposit_amount() > 0.0 {
        lang.pick(
            "Le présent contrat devient ferme à sa signature par les deux parties et à la réception de l'acompte.",
            "This contract becomes binding once signed by both parties and the deposit is received.",
        )
    } else {
        lang.pick(
            "Le présent contrat devient ferme à sa signature par les deux parties.",
            "This contract becomes binding once signed by both parties.",
        )
    };
    let clauses: [(&str, String); 5] = [
        (
            lang.pick("Technique", "Technical"),
            lang.pick(
                "L'Organisateur fournit le matériel décrit dans la fiche technique jointe, installé et en état de marche avant l'arrivée de l'Artiste, ainsi qu'un accès en coulisses sécurisé.",
                "The Promoter provides the equipment listed in the attached technical rider, set up and working before the Artist arrives, and secure backstage access.",
            )
            .into(),
        ),
        (
            lang.pick("Promotion", "Promotion"),
            lang.pick(
                "L'Organisateur utilise le nom, les photos et la biographie de l'Artiste fournis par le booker, pour la seule promotion de cet événement. Toute captation audio ou vidéo du set destinée à une diffusion demande l'accord écrit préalable de l'Artiste.",
                "The Promoter uses the Artist's name, photos and biography supplied by the booker for the promotion of this event only. Any audio or video recording of the set intended for release needs the Artist's prior written consent.",
            )
            .into(),
        ),
        (
            lang.pick("Annulation", "Cancellation"),
            lang.pick(
                "En cas d'annulation par l'Organisateur moins de 30 jours avant la date, l'acompte reste acquis à l'Artiste ; moins de 14 jours avant, le cachet est dû en totalité. En cas d'annulation par l'Artiste, l'acompte est remboursé. En cas de force majeure, aucune indemnité n'est due et les parties cherchent d'abord une nouvelle date.",
                "If the Promoter cancels less than 30 days before the date, the deposit is kept by the Artist; less than 14 days before, the full fee is due. If the Artist cancels, the deposit is refunded. In case of force majeure, no compensation is due and the parties first look for a new date.",
            )
            .into(),
        ),
        (
            lang.pick("Sécurité et assurance", "Safety and insurance"),
            lang.pick(
                "L'Organisateur est responsable de la sécurité du lieu, du public et de l'Artiste, et dispose des autorisations et assurances nécessaires à l'événement.",
                "The Promoter is responsible for the safety of the venue, the audience and the Artist, and holds the permits and insurance the event requires.",
            )
            .into(),
        ),
        (lang.pick("Validité", "Validity"), deposit_clause.into()),
    ];
    for (i, (t, c)) in clauses.iter().enumerate() {
        numbered(&mut doc, i + 1, t, c);
    }
    if let Some(n) = nz(o.notes.clone()) {
        section(&mut doc, lang.pick("Conditions particulières", "Special terms"));
        para(&mut doc, &n);
    }

    // Signatures
    doc.ensure(130.0);
    doc.gap(22.0);
    let m = doc.margin;
    let w = (doc.content_width() - 20.0) / 2.0;
    let top = doc.y;
    doc.text_at(m, top, 8.0, false, SUBTLE, lang.pick("Fait en deux exemplaires, le", "Made in two copies, on"));
    let top = top + 12.0;
    for (i, (who, name)) in [
        (lang.pick("Pour l'Organisateur", "For the Promoter"), org.clone()),
        (
            lang.pick("Pour l'Artiste", "For the Artist"),
            format!("{}, {}", d.booker.name, lang.pick("booker", "booker")),
        ),
    ]
    .iter()
    .enumerate()
    {
        let x = m + i as f32 * (w + 20.0);
        doc.rect(x, top, w, 86.0, PANEL);
        doc.text_at(x + 10.0, top + 16.0, 8.5, true, INK, who);
        doc.text_at(x + 10.0, top + 29.0, 8.5, false, SUBTLE, name);
        doc.text_at(x + 10.0, top + 78.0, 7.5, false, SUBTLE, lang.pick("Signature", "Signature"));
    }
    doc.y = top + 90.0;
    doc.finish()
}

pub fn render_rider(d: &DocData, lang: Lang) -> Vec<u8> {
    let o = &d.offer;
    let artist = artist_name(d);
    let mut doc = Doc::new(&format!("{} {artist}", lang.pick("Fiche technique", "Technical rider")));
    doc.footer = footer(d);
    let sub = [place_line(o), day_str(&o.date, lang, true)].into_iter().flatten().collect::<Vec<_>>().join(" · ");
    header(
        &mut doc,
        &format!("{artist} · {}", lang.pick("Fiche technique et accueil", "Technical and hospitality rider")),
        &artist,
        (!sub.is_empty()).then_some(sub.as_str()),
    );

    section(&mut doc, lang.pick("Set", "Set"));
    kv(&mut doc, lang.pick("Date", "Date"), day_str(&o.date, lang, true));
    kv(&mut doc, lang.pick("Lieu", "Venue"), place_line(o));
    kv(&mut doc, lang.pick("Horaire", "Set time"), set_line(o, lang));

    section(&mut doc, lang.pick("Technique", "Technical"));
    let tech = nz(o.tech_requirements.clone()).or(d.artist.tech_rider.clone());
    match tech {
        Some(t) => para(&mut doc, &t),
        None => para(
            &mut doc,
            lang.pick(
                "Matériel à confirmer avec le booker (aucune fiche technique renseignée pour l'artiste).",
                "Equipment to be confirmed with the booker (no technical rider on file for the artist).",
            ),
        ),
    }
    para(
        &mut doc,
        lang.pick(
            "Le matériel est installé, branché et testé avant l'arrivée de l'artiste. Merci de confirmer au booker tout changement au plus tard 14 jours avant la date.",
            "Equipment is set up, connected and tested before the artist arrives. Please confirm any change to the booker no later than 14 days before the date.",
        ),
    );

    section(&mut doc, lang.pick("Accueil", "Hospitality"));
    kv(&mut doc, lang.pick("Transport", "Travel"), travel_line(o, lang));
    kv(&mut doc, lang.pick("Hébergement", "Hotel"), nz(o.hotel.clone()));
    kv(&mut doc, lang.pick("Repas et boissons", "Meals and drinks"), nz(o.meals.clone()));
    kv(
        &mut doc,
        lang.pick("Liste d'invités", "Guest list"),
        o.guest_list.filter(|g| *g > 0).map(|g| {
            format!(
                "{g} {}",
                lang.pick("personnes (noms envoyés 7 jours avant la date)", "people (names sent 7 days before the date)")
            )
        }),
    );

    section(&mut doc, lang.pick("Contacts", "Contacts"));
    kv(
        &mut doc,
        lang.pick("Booker", "Booker"),
        Some(
            [Some(format!("{} ({})", d.booker.name, d.booker.company)), d.booker.email.clone(), d.booker.phone.clone()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join("\n"),
        ),
    );
    kv(
        &mut doc,
        lang.pick("Sur place", "On the night"),
        Some(lang.pick("Nom et téléphone à communiquer par l'organisateur", "Name and phone to be sent by the promoter").into()),
    );
    kv(&mut doc, lang.pick("Écouter", "Listen"), d.artist.mix_url.clone().or(d.artist.website.clone()));
    doc.finish()
}

pub fn render_invoice(d: &DocData, inv: &Invoice, lang: Lang, deposit_invoice: Option<&Invoice>) -> Vec<u8> {
    let o = &d.offer;
    let artist = artist_name(d);
    let mut doc = Doc::new(&format!("{} {}", lang.pick("Facture", "Invoice"), inv.number));
    doc.footer = footer(d);
    header(
        &mut doc,
        &format!("{} · {}", d.booker.company, lang.pick("Facture", "Invoice")),
        &format!("{} {}", lang.pick("Facture", "Invoice"), inv.number),
        None,
    );

    // Issuer | bill to
    let m = doc.margin;
    let col = (doc.content_width() - 24.0) / 2.0;
    let top = doc.y + 12.0;
    let issuer = d.issuer.block.clone().unwrap_or_else(|| {
        [Some(d.booker.company.clone()), Some(d.booker.name.clone()), d.booker.email.clone(), d.booker.phone.clone()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join("\n")
    });
    let mut bill = vec![organiser_name(d)];
    if d.contact.organisation.is_some() && !d.contact.name.trim().is_empty() {
        bill.push(d.contact.name.clone());
    }
    if let Some(p) = &d.contact.promoter {
        bill.push(p.clone());
    }
    if let Some(c) = &d.contact.city {
        bill.push(c.clone());
    }
    if let Some(e) = &d.contact.email {
        bill.push(e.clone());
    }
    let mut bottom = top;
    for (i, (label, text)) in [(lang.pick("Émetteur", "From"), issuer), (lang.pick("Facturé à", "Bill to"), bill.join("\n"))]
        .iter()
        .enumerate()
    {
        let x = m + i as f32 * (col + 24.0);
        doc.y = top;
        doc.text_at(x, doc.y, 8.0, true, ACCENT, &label.to_uppercase());
        doc.gap(4.0);
        doc.paragraph(x, col, 9.5, false, INK, text);
        bottom = bottom.max(doc.y);
    }
    doc.y = bottom + 8.0;

    section(&mut doc, lang.pick("Détails", "Details"));
    let issued = NaiveDate::parse_from_str(&inv.issued_on, "%Y-%m-%d").ok();
    kv(&mut doc, lang.pick("Date d'émission", "Issue date"), issued.map(|d| date_fmt(d, lang, false)));
    kv(&mut doc, lang.pick("Échéance", "Due date"), day_str(&inv.due_on, lang, false));
    kv(
        &mut doc,
        lang.pick("Prestation", "Performance"),
        Some(
            [Some(format!("DJ set, {artist}")), place_line(o), day_str(&o.date, lang, false)]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(", "),
        ),
    );

    // Lines
    doc.gap(18.0);
    let right = pdf::PAGE_W - m;
    doc.rect(m, doc.y, doc.content_width(), 22.0, PANEL);
    doc.text_at(m + 10.0, doc.y + 14.5, 8.5, true, INK, lang.pick("Description", "Description"));
    doc.text_right(right - 10.0, doc.y + 14.5, 8.5, true, INK, lang.pick("Montant", "Amount"));
    doc.gap(22.0);
    let desc = match inv.part.as_str() {
        "deposit" => format!(
            "{} {} % {} {artist}",
            lang.pick("Acompte de", "Deposit of"),
            o.deposit_pct.round(),
            lang.pick("sur le cachet artistique de", "on the performance fee of")
        ),
        _ if o.deposit_amount() > 0.0 => format!("{} {artist}", lang.pick("Solde du cachet artistique de", "Balance of the performance fee of")),
        _ => format!("{} {artist}", lang.pick("Cachet artistique de", "Performance fee of")),
    };
    let lines = pdf::wrap(&desc, 10.0, false, doc.content_width() - 140.0);
    let row_top = doc.y;
    for (i, l) in lines.iter().enumerate() {
        doc.text_at(m + 10.0, row_top + 16.0 + i as f32 * 14.0, 10.0, false, INK, l);
    }
    doc.text_right(right - 10.0, row_top + 16.0, 10.0, false, INK, &money_fmt(inv.amount, &inv.currency, lang));
    doc.y = row_top + 16.0 + (lines.len() as f32 - 1.0) * 14.0 + 10.0;
    if inv.part == "balance" && o.deposit_amount() > 0.0 {
        let note = format!(
            "{} {} : {}{}",
            lang.pick("Cachet total", "Total fee"),
            money_fmt(o.fee_amount.unwrap_or(0.0), &o.currency, lang),
            lang.pick("acompte de ", "deposit of "),
            money_fmt(o.deposit_amount(), &o.currency, lang)
        ) + &deposit_invoice
            .map(|di| format!(" ({} {})", lang.pick("facture", "invoice"), di.number))
            .unwrap_or_default()
            + lang.pick(" déduit", " deducted");
        doc.paragraph(m + 10.0, doc.content_width() - 140.0, 8.5, false, SUBTLE, &note);
        doc.gap(4.0);
    }
    doc.hline(m, right, doc.y, RULE, 0.6);
    doc.gap(22.0);
    doc.text_right(right - 120.0, doc.y, 10.0, true, INK, lang.pick("Total à payer", "Total due"));
    doc.text_right(right - 10.0, doc.y, 12.0, true, ACCENT, &money_fmt(inv.amount, &inv.currency, lang));
    doc.gap(10.0);
    if let Some(v) = &d.issuer.vat_note {
        doc.gap(4.0);
        doc.text_right(right - 10.0, doc.y + 8.0, 8.0, false, SUBTLE, v);
        doc.gap(10.0);
    }

    section(&mut doc, lang.pick("Paiement", "Payment"));
    para(
        &mut doc,
        &format!(
            "{}{}",
            lang.pick("Par virement bancaire en indiquant le numéro de facture", "By bank transfer, quoting the invoice number"),
            day_str(&inv.due_on, lang, false)
                .map(|v| format!(", {} {v}.", lang.pick("au plus tard le", "no later than")))
                .unwrap_or_else(|| ".".into())
        ),
    );
    if let Some(b) = &d.issuer.bank {
        kv(&mut doc, lang.pick("Coordonnées bancaires", "Bank details"), Some(b.clone()));
    }
    doc.finish()
}

// ---------------------------------------------------------------------------
// Files
// ---------------------------------------------------------------------------

pub(crate) fn docs_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?.join("documents");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

/// A path the UI sends back (to attach or open) must be one of our generated
/// documents: inside the documents folder, an existing file.
pub(crate) fn checked_path(app: &AppHandle, path: &str) -> Result<PathBuf, String> {
    let root = docs_dir(app)?.canonicalize().map_err(|e| e.to_string())?;
    let p = Path::new(path).canonicalize().map_err(|_| format!("file not found: {path}"))?;
    if !p.starts_with(&root) || !p.is_file() {
        return Err("Only documents generated by PiersCRM can be attached.".into());
    }
    Ok(p)
}

/// (file name, bytes) of an attachment, for email.rs.
pub(crate) fn load_attachment(app: &AppHandle, path: &str) -> Result<(String, Vec<u8>), String> {
    let p = checked_path(app, path)?;
    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("document.pdf").to_string();
    let bytes = std::fs::read(&p).map_err(|e| e.to_string())?;
    Ok((name, bytes))
}

fn slug(s: &str) -> String {
    let mut out = String::new();
    for word in s.split(|c: char| !c.is_alphanumeric()) {
        let w = crate::db::normalise(word);
        if w.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push('-');
        }
        out.push_str(&w);
    }
    out.chars().take(60).collect()
}

#[derive(Debug, Clone, Serialize)]
pub struct GeneratedDoc {
    /// contract | rider | invoice_deposit | invoice_balance
    pub kind: String,
    pub path: String,
    pub file_name: String,
    pub invoice_number: Option<String>,
}

/// Render one document of an offer to bytes (+ file name), issuing the
/// invoice number when needed. Pure apart from the invoice row.
pub(crate) fn build(conn: &Connection, offer_id: i64, kind: &str, today: NaiveDate) -> Result<(String, Vec<u8>, Option<Invoice>), String> {
    let d = load(conn, offer_id)?;
    let lang = Lang::of(&d.offer.language);
    let base = slug(&format!(
        "{} {} {}",
        artist_name(&d),
        d.offer.venue.clone().unwrap_or_else(|| organiser_name(&d)),
        d.offer.date.clone().unwrap_or_default()
    ));
    match kind {
        "contract" => Ok((format!("{}-{base}.pdf", lang.pick("contrat", "contract")), render_contract(&d, lang), None)),
        "rider" => Ok((format!("{}-{base}.pdf", lang.pick("fiche-technique", "rider")), render_rider(&d, lang), None)),
        "invoice_deposit" | "invoice_balance" => {
            let part = kind.trim_start_matches("invoice_");
            let inv = money::issue_invoice(conn, offer_id, part, today)?;
            let dep = (part == "balance").then(|| money::find_invoice(conn, offer_id, "deposit")).flatten();
            let bytes = render_invoice(&d, &inv, lang, dep.as_ref());
            Ok((format!("{}-{}.pdf", lang.pick("facture", "invoice"), slug(&inv.number)), bytes, Some(inv)))
        }
        _ => Err(format!("unknown document: {kind}")),
    }
}

fn generate(app: &AppHandle, conn: &Connection, offer_id: i64, kind: &str) -> Result<GeneratedDoc, String> {
    let (name, bytes, inv) = build(conn, offer_id, kind, today())?;
    let dir = docs_dir(app)?.join(format!("offer-{offer_id}"));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(&name);
    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
    let path_s = path.to_string_lossy().to_string();
    if let Some(i) = &inv {
        let _ = conn.execute("UPDATE invoices SET file_path=?2 WHERE id=?1", params![i.id, path_s]);
    }
    Ok(GeneratedDoc { kind: kind.into(), path: path_s, file_name: name, invoice_number: inv.map(|i| i.number) })
}

#[tauri::command(async)]
pub fn generate_offer_document(
    app: AppHandle,
    state: State<'_, AppState>,
    offer_id: i64,
    kind: String,
) -> Result<GeneratedDoc, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    generate(&app, &conn, offer_id, &kind)
}

/// Documents already generated for an offer (newest first).
#[tauri::command(async)]
pub fn list_offer_documents(app: AppHandle, offer_id: i64) -> Result<Vec<GeneratedDoc>, String> {
    let dir = docs_dir(&app)?.join(format!("offer-{offer_id}"));
    let Ok(rd) = std::fs::read_dir(&dir) else { return Ok(Vec::new()) };
    let mut out: Vec<(std::time::SystemTime, GeneratedDoc)> = Vec::new();
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("pdf") {
            continue;
        }
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
        let kind = if name.starts_with("contrat") || name.starts_with("contract") {
            "contract"
        } else if name.starts_with("fiche") || name.starts_with("rider") {
            "rider"
        } else {
            "invoice"
        };
        let at = e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
        out.push((at, GeneratedDoc { kind: kind.into(), path: p.to_string_lossy().to_string(), file_name: name, invoice_number: None }));
    }
    out.sort_by(|a, b| b.0.cmp(&a.0));
    Ok(out.into_iter().map(|(_, d)| d).collect())
}

/// Open a generated document in the default PDF viewer.
#[tauri::command(async)]
pub fn open_offer_document(app: AppHandle, path: String) -> Result<(), String> {
    let p = checked_path(&app, &path)?;
    tauri_plugin_opener::open_path(p, None::<&str>).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Email drafts (opened in the compose window, sent by the user)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct EmailDraft {
    pub contact_id: Option<i64>,
    pub to: String,
    pub subject: String,
    pub body: String,
    pub attachments: Vec<GeneratedDoc>,
}

fn greeting(d: &DocData, lang: Lang) -> String {
    let who = d.contact.promoter.clone().unwrap_or_else(|| d.contact.name.clone());
    let first = who.split_whitespace().next().unwrap_or("").to_string();
    match lang {
        Lang::Fr => if first.is_empty() { "Bonjour,".into() } else { format!("Bonjour {first},") },
        Lang::En => if first.is_empty() { "Hi,".into() } else { format!("Hi {first},") },
    }
}

fn summary_lines(d: &DocData, lang: Lang) -> String {
    let o = &d.offer;
    let mut l: Vec<String> = Vec::new();
    let mut push = |label: &str, v: Option<String>| {
        if let Some(v) = v.filter(|v| !v.trim().is_empty()) {
            l.push(format!("- {label} : {v}"));
        }
    };
    push(lang.pick("Date", "Date"), day_str(&o.date, lang, true));
    push(lang.pick("Lieu", "Venue"), place_line(o));
    push(lang.pick("Set", "Set"), set_line(o, lang));
    push(lang.pick("Cachet", "Fee"), Some(fee_line(o, lang)));
    if o.deposit_amount() > 0.0 {
        push(
            lang.pick("Acompte", "Deposit"),
            Some(format!("{} % ({})", o.deposit_pct.round(), money_fmt(o.deposit_amount(), &o.currency, lang))),
        );
    }
    push(lang.pick("Transport", "Travel"), travel_line(o, lang));
    push(lang.pick("Hébergement", "Hotel"), nz(o.hotel.clone()));
    push(lang.pick("Repas et boissons", "Meals and drinks"), nz(o.meals.clone()));
    push(
        lang.pick("Liste d'invités", "Guest list"),
        o.guest_list.filter(|g| *g > 0).map(|g| g.to_string()),
    );
    l.join("\n")
}

pub(crate) fn draft_text(d: &DocData, purpose: &str, inv: Option<&Invoice>) -> (String, String) {
    let lang = Lang::of(&d.offer.language);
    let o = &d.offer;
    let artist = artist_name(d);
    let when = day_str(&o.date, lang, false).unwrap_or_default();
    let place = o.venue.clone().unwrap_or_else(|| organiser_name(d));
    let hi = greeting(d, lang);
    let sign = format!("{}\n{}", lang.pick("Belle journée,", "Best,"), d.booker.name);
    match purpose {
        "offer" => (
            format!("{} {artist} @ {place}, {when}", lang.pick("Proposition", "Offer")),
            format!(
                "{hi}\n\n{}\n\n{}\n\n{}\n\n{sign}",
                lang.pick(
                    &format!("Comme convenu, voici notre proposition pour {artist} :"),
                    &format!("As discussed, here is our offer for {artist}:"),
                ),
                summary_lines(d, lang),
                match day_str(&o.answer_by, lang, false) {
                    Some(dl) => format!(
                        "{} {dl}. {}",
                        lang.pick("Merci de me faire un retour avant le", "Could you get back to me by"),
                        lang.pick("Dès ton accord, je t'envoie le contrat et la fiche technique.", "Once agreed, I'll send the contract and the rider.")
                    ),
                    None => lang
                        .pick("Dès ton accord, je t'envoie le contrat et la fiche technique.", "Once agreed, I'll send the contract and the rider.")
                        .into(),
                }
            ),
        ),
        "contract" => (
            format!("{} {artist} @ {place}, {when}", lang.pick("Contrat", "Contract")),
            format!(
                "{hi}\n\n{}\n\n{}\n\n{}\n\n{sign}",
                lang.pick(
                    "Merci pour la confirmation ! Tu trouveras en pièces jointes le contrat et la fiche technique.",
                    "Thanks for confirming! Please find attached the contract and the technical rider.",
                ),
                summary_lines(d, lang),
                lang.pick(
                    "Peux-tu me renvoyer le contrat signé ? Je t'envoie ensuite la facture d'acompte.",
                    "Could you send the contract back signed? I'll then send the deposit invoice.",
                )
            ),
        ),
        _ => {
            let number = inv.map(|i| i.number.clone()).unwrap_or_default();
            let amount = inv.map(|i| money_fmt(i.amount, &i.currency, lang)).unwrap_or_default();
            let due = inv.and_then(|i| day_str(&i.due_on, lang, false));
            let what = if purpose == "invoice_deposit" { lang.pick("d'acompte", "deposit") } else { lang.pick("de solde", "balance") };
            (
                match lang {
                    Lang::Fr => format!("Facture {number} {what}, {artist} @ {place}"),
                    Lang::En => format!("Invoice {number} ({what}), {artist} @ {place}"),
                },
                format!(
                    "{hi}\n\n{}\n\n{sign}",
                    match lang {
                        Lang::Fr => format!(
                            "Tu trouveras en pièce jointe la facture {what} {number} ({amount}) pour la date du {when}{}.",
                            due.map(|d| format!(", à régler avant le {d}")).unwrap_or_default()
                        ),
                        Lang::En => format!(
                            "Please find attached the {what} invoice {number} ({amount}) for the show on {when}{}.",
                            due.map(|d| format!(", due by {d}")).unwrap_or_default()
                        ),
                    }
                ),
            )
        }
    }
}

/// Draft for an offer: "offer" (terms in the body), "contract" (contract +
/// rider attached), "invoice_deposit" / "invoice_balance" (invoice attached).
#[tauri::command(async)]
pub fn offer_email_draft(
    app: AppHandle,
    state: State<'_, AppState>,
    offer_id: i64,
    purpose: String,
) -> Result<EmailDraft, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let kinds: &[&str] = match purpose.as_str() {
        "offer" => &[],
        "contract" => &["contract", "rider"],
        "invoice_deposit" => &["invoice_deposit"],
        "invoice_balance" => &["invoice_balance"],
        _ => return Err(format!("unknown draft: {purpose}")),
    };
    let mut attachments = Vec::new();
    for k in kinds {
        attachments.push(generate(&app, &conn, offer_id, k)?);
    }
    let d = load(&conn, offer_id)?;
    let inv = purpose
        .strip_prefix("invoice_")
        .and_then(|p| money::find_invoice(&conn, offer_id, p));
    let (subject, body) = draft_text(&d, &purpose, inv.as_ref());
    Ok(EmailDraft {
        contact_id: d.offer.contact_id,
        to: d.contact.email.clone().unwrap_or_default(),
        subject,
        body,
        attachments,
    })
}

pub(crate) fn advancing_text(d: &DocData, g: &advancing::GigAdvancing) -> (String, String) {
    let lang = Lang::of(&d.offer.language);
    let artist = artist_name(d);
    let when = parse_day(&Some(g.date.clone())).map(|x| date_fmt(x, lang, true)).unwrap_or_default();
    let place = g.venue.clone().unwrap_or_else(|| g.title.clone());
    let open = |k: &str| g.items.iter().any(|i| i.key == k && i.state.is_none());
    let mut asks: Vec<&str> = Vec::new();
    if open("set_time") {
        asks.push(lang.pick("la confirmation de l'horaire du set (et l'ordre du line-up)", "confirmation of the set time (and the running order)"));
    }
    if open("tech_rider") {
        asks.push(lang.pick("la confirmation que la fiche technique (jointe) est bien prise en compte", "confirmation that the attached technical rider is covered"));
    }
    if open("promo") {
        asks.push(lang.pick("ce dont tu as besoin côté promo (photos, bio, visuels)", "what you need for promo (photos, bio, artwork)"));
    }
    if open("travel") && d.offer.travel_by.as_deref() != Some("artist") {
        asks.push(lang.pick("les détails du transport", "the travel details"));
    }
    if open("hotel") && nz(d.offer.hotel.clone()).is_some() {
        asks.push(lang.pick("l'adresse de l'hôtel et la réservation", "the hotel address and booking"));
    }
    if open("guest_list") {
        asks.push(lang.pick("à qui envoyer la liste d'invités (je l'envoie 7 jours avant)", "where to send the guest list (I'll send it 7 days before)"));
    }
    if open("night_contact") {
        asks.push(lang.pick("le nom et le numéro du contact sur place le soir même", "the name and phone number of the contact on the night"));
    }
    asks.push(lang.pick("l'adresse exacte et l'accès artistes (heure d'arrivée, parking)", "the exact address and artist access (arrival time, parking)"));
    let list = asks.iter().map(|a| format!("- {a}")).collect::<Vec<_>>().join("\n");
    let hi = greeting(d, lang);
    let sign = format!("{}\n{}", lang.pick("Merci et à très vite,", "Thanks and see you soon,"), d.booker.name);
    (
        format!("Advancing {artist} @ {place}, {}", when),
        format!(
            "{hi}\n\n{}\n\n{list}\n\n{}\n\n{sign}",
            match lang {
                Lang::Fr => format!("On approche de la date du {when} ! Pour préparer au mieux la venue de {artist}, pourrais-tu me confirmer :"),
                Lang::En => format!("The show on {when} is getting close! To prepare {artist}'s visit, could you confirm:"),
            },
            lang.pick("La fiche technique est en pièce jointe.", "The technical rider is attached.")
        ),
    )
}

/// The J-14 advancing email to the venue: what is still open on the
/// checklist, with the rider attached (generated fresh from the offer).
#[tauri::command(async)]
pub fn advancing_email_draft(app: AppHandle, state: State<'_, AppState>, event_id: i64) -> Result<EmailDraft, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let g = advancing::for_event(&conn, event_id).ok_or("event not found")?;
    let (d, attachments) = match g.offer_id {
        Some(oid) => (load(&conn, oid)?, vec![generate(&app, &conn, oid, "rider")?]),
        None => {
            // A gig entered in the Agenda without an offer: the contact and
            // artist of the event, no rider attachment.
            let (artist_id, contact_id): (Option<i64>, Option<i64>) = conn
                .query_row("SELECT artist_id, contact_id FROM events WHERE id=?1", params![event_id], |r| Ok((r.get(0)?, r.get(1)?)))
                .map_err(|e| e.to_string())?;
            let mut o = match contact_id {
                Some(c) => super::prefill(&conn, c)?,
                None => Offer { language: "fr".into(), ..Default::default() },
            };
            o.artist_id = artist_id.or(o.artist_id);
            o.venue = g.venue.clone();
            o.date = Some(g.date.clone());
            let mut d = DocData { offer: o, booker: booker(&conn), ..Default::default() };
            if let Some(c) = contact_id {
                d.contact = conn
                    .query_row("SELECT name, promoter, email, area FROM contacts WHERE id=?1", params![c], |r| {
                        Ok(Party { name: r.get(0)?, organisation: None, promoter: nz(r.get(1)?), email: nz(r.get(2)?), city: nz(r.get(3)?) })
                    })
                    .unwrap_or_default();
            }
            if let Some(a) = d.offer.artist_id {
                d.artist.name = conn.query_row("SELECT name FROM artists WHERE id=?1", params![a], |r| r.get(0)).unwrap_or_default();
            }
            (d, Vec::new())
        }
    };
    let (subject, mut body) = advancing_text(&d, &g);
    if attachments.is_empty() {
        body = body
            .replace("La fiche technique est en pièce jointe.\n\n", "")
            .replace("The technical rider is attached.\n\n", "");
    }
    Ok(EmailDraft {
        contact_id: d.offer.contact_id.or(g.contact_id),
        to: d.contact.email.clone().unwrap_or_default(),
        subject,
        body,
        attachments,
    })
}

#[cfg(test)]
mod tests {
    use super::super::save;
    use super::super::tests::{day, db, offer};
    use super::*;

    fn accepted(c: &Connection) -> i64 {
        let mut o = offer(c);
        o.status = "accepted".into();
        o.hotel = Some("1 nuit, chambre double".into());
        o.meals = Some("Dîner pour 2, boissons au bar".into());
        o.guest_list = Some(4);
        o.travel_from = Some("Paris".into());
        o.travel_to = Some("Amsterdam".into());
        save(c, &o, day("2026-11-01")).unwrap().0
    }

    #[test]
    fn formats() {
        assert_eq!(money_fmt(1200.0, "EUR", Lang::Fr), "1 200,00 €");
        assert_eq!(money_fmt(1200.5, "EUR", Lang::En), "€1,200.50");
        assert_eq!(money_fmt(950.0, "GBP", Lang::En), "£950.00");
        assert_eq!(money_fmt(1234567.0, "CHF", Lang::Fr), "1 234 567,00 CHF");
        assert_eq!(date_fmt(day("2026-12-12"), Lang::Fr, true), "samedi 12 décembre 2026");
        assert_eq!(date_fmt(day("2026-12-01"), Lang::Fr, false), "1er décembre 2026");
        assert_eq!(date_fmt(day("2026-12-12"), Lang::En, true), "Saturday, 12 December 2026");
        assert_eq!(slug("Piers @ Paradiso 2026-12-12"), "piers-paradiso-2026-12-12");
    }

    #[test]
    fn pdf_smoke_all_documents_both_languages() {
        let c = db();
        let id = accepted(&c);
        let t = day("2026-11-02");
        for lang in ["fr", "en"] {
            c.execute("UPDATE offers SET language=?1 WHERE id=?2", params![lang, id]).unwrap();
            for kind in ["contract", "rider", "invoice_deposit", "invoice_balance"] {
                let (name, bytes, inv) = build(&c, id, kind, t).unwrap();
                assert!(name.ends_with(".pdf"), "{name}");
                assert!(bytes.starts_with(b"%PDF-1.4"), "{kind} {lang}");
                assert!(bytes.len() > 1500, "{kind} {lang}: {} bytes", bytes.len());
                assert_eq!(inv.is_some(), kind.starts_with("invoice"));
            }
        }
        // Both languages reuse the same two invoice numbers.
        let n: i64 = c.query_row("SELECT COUNT(*) FROM invoices", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 2);
        assert!(build(&c, id, "poster", t).is_err());
    }

    #[test]
    fn drafts_never_use_the_em_dash_in_french() {
        let c = db();
        let id = accepted(&c);
        let d = load(&c, id).unwrap();
        for p in ["offer", "contract", "invoice_deposit"] {
            let (s, b) = draft_text(&d, p, None);
            assert!(!s.contains('—') && !b.contains('—'), "{p}");
        }
        let (s, b) = draft_text(&d, "offer", None);
        assert!(s.contains("Piers @ Paradiso"));
        assert!(b.contains("1 200,00 €"));
        let ev = d.offer.event_id.unwrap();
        let g = advancing::for_event(&c, ev).unwrap();
        let (s, b) = advancing_text(&d, &g);
        assert!(s.starts_with("Advancing Piers @ Paradiso"));
        assert!(b.contains("contact sur place"));
        assert!(b.contains("hôtel"));
        assert!(!b.contains('—'));
    }
}
