//! Strict identity rule for collectives found through a web search.
//!
//! The loose rule (enrich::core_matches_name) accepts a domain or handle that
//! contains any distinctive word of the name. Good enough for venues, whose
//! names are rarely dictionary words, it was a disaster on collectives:
//! "Sonic Flux" took e-flux.com, "RUSH" rush.com, "Planet 9" planet.com,
//! "Utopia Productions" @utopiamovies, and the crawl then stored the emails of
//! those unrelated companies.
//!
//! Here the domain label or handle must spell the whole name, once the usual
//! affixes are peeled off both sides ("the", "events", "records", the city,
//! a country code): "carcosamusic" is Carcosa, "unitysound.ldn" is Unity Sound
//! in London, "ldnentertainment" is not Beats LDN. A search result must also
//! read like music (music_context): a bare "rush" or "raindance" is someone
//! else until the page talks about DJs, parties or clubs.

use crate::db::normalise;

/// Peeled off either end of a name, a domain label or a handle.
const AFFIXES: &[&str] = &[
    "the", "official", "officiel", "events", "event", "collective", "collectif", "kollektiv",
    "collectiv", "productions", "production", "prods", "prod", "music", "musique", "records",
    "recordings", "recs", "soundsystem", "sounds", "sound", "crew", "parties", "party", "presents",
    "club", "live", "label", "djs", "dj", "agency", "promotions", "entertainment", "group", "team",
    "nights", "night", "family", "hq", "dnb", "techno", "ukg", "uk", "nl", "community",
    "promociones", "producciones", "hotel",
];

/// Company forms and stray short words dropped from a name ("OstGut GmbH",
/// "M7CLUB.ES", "Motel Campo (CH)").
const LEGAL: &[&str] = &["gmbh", "sl", "ltd", "bv", "sas", "sarl", "srl", "ev", "ug", "llc", "inc", "sa", "ag", "limited"];

/// Short forms of the cities a collective is listed in.
fn city_aliases(city: &str) -> Vec<String> {
    let c = normalise(city);
    let extra: &[&str] = match c.as_str() {
        "london" => &["ldn", "lon"],
        "amsterdam" => &["ams", "adam", "dam"],
        "barcelona" => &["bcn"],
        "berlin" => &["ber"],
        "manchester" => &["mcr", "manc"],
        "newyork" => &["nyc", "ny"],
        "rotterdam" => &["rdam", "rtm"],
        "paris" => &["prs"],
        "brussels" | "bruxelles" => &["bxl"],
        "glasgow" => &["gla"],
        "bristol" => &["bris", "brs"],
        "leeds" => &["lds"],
        _ => &[],
    };
    let mut out: Vec<String> = city
        .split(|ch: char| !ch.is_alphanumeric())
        .map(normalise)
        .filter(|w| w.len() >= 2)
        .collect();
    out.push(c);
    out.extend(extra.iter().map(|s| s.to_string()));
    out.retain(|w| !w.is_empty());
    out
}

/// Peel affixes off both ends until none applies, keeping at least 3 chars.
fn peel(core: &str, extra: &[String]) -> String {
    let mut s = core.to_string();
    loop {
        let before = s.clone();
        for a in AFFIXES.iter().map(|a| a.to_string()).chain(extra.iter().cloned()) {
            if s.len() > a.len() + 2 {
                if let Some(rest) = s.strip_prefix(a.as_str()) {
                    s = rest.to_string();
                }
            }
            if s.len() > a.len() + 2 {
                if let Some(rest) = s.strip_suffix(a.as_str()) {
                    s = rest.to_string();
                }
            }
        }
        if s == before {
            return s;
        }
    }
}

/// The name reduced the same way ("Unity Sound" in London -> "unitysound",
/// "Circular Amsterdam" -> "circular"): the full form, and the form without
/// company words and one or two letter words ("Le Botanique" -> "botanique").
fn name_cores(name: &str, city: Option<&str>) -> Vec<String> {
    let geo = city.map(city_aliases).unwrap_or_default();
    let words: Vec<String> = name.split(|c: char| !c.is_alphanumeric()).map(normalise).filter(|w| !w.is_empty()).collect();
    let own: Vec<&String> = words.iter().filter(|w| !geo.contains(w)).collect();
    let joined: String = if own.is_empty() { words.concat() } else { own.iter().map(|w| w.as_str()).collect() };
    let short: String = own.iter().filter(|w| !(w.len() <= 2 && w.chars().all(|c| c.is_alphabetic())) && !LEGAL.contains(&w.as_str())).map(|w| w.as_str()).collect();
    let mut out = vec![peel(&joined, &geo)];
    if !short.is_empty() {
        let p = peel(&short, &geo);
        if !out.contains(&p) {
            out.push(p);
        }
    }
    out.retain(|n| n.len() >= 3);
    out
}

fn same_stem(a: &str, b: &str) -> bool {
    a == b
        || a.strip_suffix('s') == Some(b)
        || b.strip_suffix('s') == Some(a)
        || a.strip_suffix('z') == Some(b)
        || b.strip_suffix('z') == Some(a)
}

/// Whether a reduced label (domain_core, or a handle with dots and
/// underscores folded) spells `name`.
pub(crate) fn spells_name(core: &str, name: &str, city: Option<&str>) -> bool {
    let geo = city.map(city_aliases).unwrap_or_default();
    let c = peel(&normalise(core), &geo);
    name_cores(name, city).iter().any(|n| same_stem(&c, n))
}

/// A short name ("RUSH", "LOAD BCN", "SOAK") spelled bare, without any
/// affix: "rush" proves little, "loadbcn" or "thesoaklondon" does.
pub(crate) fn is_bare_weak(core: &str, name: &str, city: Option<&str>) -> bool {
    let geo = city.map(city_aliases).unwrap_or_default();
    let own: String = name
        .split(|c: char| !c.is_alphanumeric())
        .map(normalise)
        .filter(|w| !w.is_empty() && !geo.contains(w) && !AFFIXES.contains(&w.as_str()))
        .collect();
    let c = normalise(core);
    own.len() < 5 && peel(&c, &geo) == c
}

/// Whether a result title or snippet reads like electronic music nightlife.
pub(crate) fn music_context(text: &str) -> bool {
    let low = text.to_lowercase();
    const PHRASES: &[&str] = &[
        "dj", "djs", "techno", "house", "party", "parties", "rave", "raves", "nightclub", "clubnight",
        "club", "clubbing", "dancefloor", "electronic", "disco", "lineup", "dnb", "jungle", "garage", "ukg",
        "minimal", "soundsystem", "afro", "breaks", "trance", "vinyl", "records", "promoter", "promoters",
        "soiree", "soirees", "residentadvisor", "boilerroom", "bass", "nightlife", "warehouse", "afterparty",
    ];
    let words: Vec<String> = low.split(|c: char| !c.is_alphanumeric()).map(normalise).filter(|w| !w.is_empty()).collect();
    if words.iter().any(|w| PHRASES.contains(&w.as_str())) {
        return true;
    }
    let squashed: String = words.concat();
    ["lineup", "residentadvisor", "racoevents", "drumandbass", "drumbass", "deephouse", "techhouse", "dancemusic", "boilerroom", "soundsystem"]
        .iter()
        .any(|p| squashed.contains(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_name_with_affixes() {
        assert!(spells_name("carcosamusic", "Carcosa", Some("London")));
        assert!(spells_name("unitysoundldn", "Unity Sound", Some("London")));
        assert!(spells_name("konflictrecords", "KONFLICT.", Some("London")));
        assert!(spells_name("circularamsterdam", "Circular Amsterdam", Some("Amsterdam")));
        assert!(spells_name("specialgroove", "Special Grooves", Some("London")));
        assert!(spells_name("engageaudiodnb", "Engage Audio", Some("London")));
        assert!(spells_name("mastiempolive", "Mas Tiempo", Some("London")));
        assert!(spells_name("femmmefraiche", "Fèmmme Fraîche", Some("London")));
    }

    #[test]
    fn a_shared_word_is_not_the_name() {
        assert!(!spells_name("eflux", "Sonic Flux", Some("Leeds")));
        assert!(!spells_name("utopiamovies", "Utopia Productions", Some("Leeds")));
        assert!(!spells_name("ldnentertainment", "Beats LDN", Some("London")));
        assert!(!spells_name("onecollective", "Type One Community", Some("London")));
        assert!(!spells_name("planet", "Planet 9", Some("London")));
        assert!(!spells_name("northernfoodandfarming", "Northern Groove Association 1", Some("Leeds")));
        assert!(!spells_name("theterracenyc", "The Terrace Party", Some("Leeds")));
        assert!(!spells_name("soundwavecollective", "Maat Sound", Some("Amsterdam")));
        assert!(!spells_name("justvibez", "Vibez", Some("London")));
    }

    #[test]
    fn weak_names_and_context() {
        assert!(is_bare_weak("rush", "RUSH", Some("Berlin")));
        assert!(!is_bare_weak("loadbcn", "LOAD BCN", Some("Barcelona")));
        assert!(!is_bare_weak("thesoaklondon", "SOAK", Some("London")));
        assert!(!is_bare_weak("nachtlicker", "Nachtlicker", Some("London")));
        assert!(spells_name("lasterclub", "Laster Club", Some("Leipzig")));
        assert!(spells_name("groovecontroldjcrew", "Groove Control", Some("Amsterdam")));
        assert!(spells_name("loadbcn", "LOAD BCN", Some("Barcelona")));
        assert!(spells_name("ostgut", "OstGut GmbH", Some("Berlin")));
        assert!(spells_name("m7club", "M7CLUB.ES", Some("Madrid")));
        assert!(spells_name("botanique", "Le Botanique", Some("Bruxelles")));
        assert!(spells_name("lesucre", "Le Sucre", Some("Lyon")));
        assert!(music_context("Techno parties in East London since 2015"));
        assert!(music_context("Line-up: Ben UFO, Joy Orbison"));
        assert!(!music_context("Rush is a premium flight booking service"));
        assert!(!music_context("Raindance Film Festival, independent film courses"));
    }
}
