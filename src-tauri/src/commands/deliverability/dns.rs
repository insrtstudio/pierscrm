//! Domain authentication check for the sending address: SPF, DKIM, DMARC, MX.
//!
//! The network sits behind [`Resolver`] (DNS-over-HTTPS in the app, a fixture
//! map in tests). Everything else (provider detection, TXT parsing, verdicts,
//! the exact record to add) is pure and tested on fixtures.

use serde::{Deserialize, Serialize};

// ---------------- Resolver ----------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RType {
    Txt,
    Mx,
}

/// One DNS question. `Ok(vec![])` means "no such record" (NXDOMAIN or no
/// data); `Err` means the lookup itself failed (offline, resolver down).
pub trait Resolver {
    fn lookup(&self, name: &str, rtype: RType) -> Result<Vec<String>, String>;
}

/// DNS-over-HTTPS through Cloudflare's JSON API, Google's as a fallback.
/// TXT answers come back joined (`"a" "b"` → `ab`), MX as `"10 host."`.
pub struct DohResolver {
    http: reqwest::blocking::Client,
}

impl DohResolver {
    pub fn new() -> Result<Self, String> {
        let http = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(8))
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self { http })
    }

    fn query(&self, base: &str, name: &str, rtype: RType) -> Result<Vec<String>, String> {
        let t = match rtype {
            RType::Txt => "TXT",
            RType::Mx => "MX",
        };
        let url = reqwest::Url::parse_with_params(base, &[("name", name), ("type", t)]).map_err(|e| e.to_string())?;
        let body: serde_json::Value = self
            .http
            .get(url)
            .header("accept", "application/dns-json")
            .send()
            .map_err(|e| e.to_string())?
            .error_for_status()
            .map_err(|e| e.to_string())?
            .json()
            .map_err(|e| e.to_string())?;
        parse_doh_json(&body, rtype)
    }
}

impl Resolver for DohResolver {
    fn lookup(&self, name: &str, rtype: RType) -> Result<Vec<String>, String> {
        self.query("https://cloudflare-dns.com/dns-query", name, rtype)
            .or_else(|_| self.query("https://dns.google/resolve", name, rtype))
    }
}

/// Answers of a DoH JSON response, filtered on the asked type (a CNAME in the
/// chain, as for Microsoft's DKIM selectors, comes back as an extra answer).
pub fn parse_doh_json(v: &serde_json::Value, rtype: RType) -> Result<Vec<String>, String> {
    let status = v.get("Status").and_then(|s| s.as_i64()).unwrap_or(2);
    match status {
        0 => {}
        3 => return Ok(Vec::new()), // NXDOMAIN
        s => return Err(format!("DNS status {}", s)),
    }
    let want = match rtype {
        RType::Txt => 16,
        RType::Mx => 15,
    };
    Ok(v.get("Answer")
        .and_then(|a| a.as_array())
        .map(|arr| {
            arr.iter()
                .filter(|a| a.get("type").and_then(|t| t.as_i64()) == Some(want))
                .filter_map(|a| a.get("data").and_then(|d| d.as_str()))
                .map(|d| match rtype {
                    RType::Txt => join_txt(d),
                    RType::Mx => d.trim().to_string(),
                })
                .collect()
        })
        .unwrap_or_default())
}

/// `"v=spf1 include:a" " ~all"` → `v=spf1 include:a ~all`. An unquoted value
/// is kept as is.
pub fn join_txt(data: &str) -> String {
    let d = data.trim();
    if !d.starts_with('"') {
        return d.to_string();
    }
    let mut out = String::new();
    let mut in_q = false;
    let mut esc = false;
    for c in d.chars() {
        if esc {
            out.push(c);
            esc = false;
        } else if c == '\\' && in_q {
            esc = true;
        } else if c == '"' {
            in_q = !in_q;
        } else if in_q {
            out.push(c);
        }
    }
    out
}

// ---------------- Providers ----------------

pub struct Provider {
    pub key: &'static str,
    pub name: &'static str,
    /// Substrings of the SMTP host that identify the provider.
    smtp: &'static [&'static str],
    /// Substrings of an MX host.
    mx: &'static [&'static str],
    /// SPF include(s); the first is the one to add.
    pub spf: &'static [&'static str],
    /// DKIM selectors this provider uses.
    pub selectors: &'static [&'static str],
    /// Where DKIM is switched on, in plain French.
    pub dkim_howto: &'static str,
}

pub const PROVIDERS: &[Provider] = &[
    Provider {
        key: "google",
        name: "Google Workspace",
        smtp: &["gmail.com", "google.com", "googlemail.com"],
        mx: &["google.com", "googlemail.com"],
        spf: &["_spf.google.com"],
        selectors: &["google"],
        dkim_howto: "Console d'administration Google : Applications, Google Workspace, Gmail, Authentifier les e-mails. Générez la clé, ajoutez le TXT indiqué (nom google._domainkey) chez votre hébergeur DNS, puis cliquez sur « Démarrer l'authentification ».",
    },
    Provider {
        key: "microsoft",
        name: "Microsoft 365",
        smtp: &["office365.com", "outlook.com", "office.com"],
        mx: &["protection.outlook.com", "outlook.com"],
        spf: &["spf.protection.outlook.com"],
        selectors: &["selector1", "selector2"],
        dkim_howto: "Portail Microsoft Defender : Stratégies, Authentification des e-mails, DKIM. Ajoutez les deux CNAME selector1._domainkey et selector2._domainkey indiqués, puis activez la signature.",
    },
    Provider {
        key: "ovh",
        name: "OVHcloud",
        smtp: &["ovh.net", "ovh.com", "ovhcloud"],
        mx: &["ovh.net", "ovh.com"],
        spf: &["mx.ovh.com"],
        selectors: &["ovhmo1", "ovhmo-selector-1", "ovhex", "ovh"],
        dkim_howto: "Espace client OVHcloud : Web Cloud, Noms de domaine, votre domaine, onglet DKIM (ou Emails, Sécurité). Activez DKIM, OVH crée lui-même les enregistrements si la zone DNS est chez OVH.",
    },
    Provider {
        key: "ionos",
        name: "IONOS",
        smtp: &["ionos.", "1and1", "1und1"],
        mx: &["ionos.", "1and1", "kundenserver.de"],
        spf: &["_spf-eu.ionos.com"],
        selectors: &["s1-ionos", "s2-ionos", "s42582890"],
        dkim_howto: "Espace client IONOS : Domaines & SSL, votre domaine, DNS. Activez la configuration e-mail automatique (ajoute SPF et DKIM).",
    },
    Provider {
        key: "gandi",
        name: "Gandi",
        smtp: &["gandi.net"],
        mx: &["gandi.net"],
        spf: &["_mailcust.gandi.net"],
        selectors: &["gm1", "gm2", "gm3"],
        dkim_howto: "Interface Gandi : Noms de domaine, votre domaine, Enregistrements DNS. Les CNAME gm1, gm2 et gm3._domainkey sont ajoutés en activant la boîte Gandi Mail.",
    },
    Provider {
        key: "infomaniak",
        name: "Infomaniak",
        smtp: &["infomaniak"],
        mx: &["infomaniak"],
        spf: &["spf.infomaniak.ch"],
        selectors: &["20240101", "default"],
        dkim_howto: "Manager Infomaniak : Service Mail, Configuration, DKIM. Activez-le puis copiez l'enregistrement proposé dans la zone DNS.",
    },
    Provider {
        key: "zoho",
        name: "Zoho Mail",
        smtp: &["zoho."],
        mx: &["zoho."],
        spf: &["zoho.eu", "zoho.com"],
        selectors: &["zmail", "zoho"],
        dkim_howto: "Console Zoho Mail : Domaines, votre domaine, Authentification e-mail, DKIM. Ajoutez un sélecteur puis copiez le TXT proposé.",
    },
    Provider {
        key: "brevo",
        name: "Brevo",
        smtp: &["brevo.com", "sendinblue.com"],
        mx: &[],
        spf: &["spf.brevo.com", "spf.sendinblue.com"],
        selectors: &["brevo1", "brevo2", "mail"],
        dkim_howto: "Brevo : Paramètres, Expéditeurs, domaines et IP dédiées, Domaines. Authentifiez le domaine et ajoutez les enregistrements affichés.",
    },
    Provider {
        key: "mailgun",
        name: "Mailgun",
        smtp: &["mailgun.org"],
        mx: &["mailgun.org"],
        spf: &["mailgun.org"],
        selectors: &["mx", "k1", "smtp", "pic"],
        dkim_howto: "Mailgun : Sending, Domains, votre domaine, DNS records. Ajoutez le TXT DKIM indiqué.",
    },
    Provider {
        key: "sendgrid",
        name: "SendGrid",
        smtp: &["sendgrid.net"],
        mx: &[],
        spf: &["sendgrid.net"],
        selectors: &["s1", "s2"],
        dkim_howto: "SendGrid : Settings, Sender Authentication, Authenticate Your Domain. Ajoutez les CNAME s1 et s2._domainkey.",
    },
    Provider {
        key: "ses",
        name: "Amazon SES",
        smtp: &["amazonaws.com"],
        mx: &["amazonaws.com"],
        spf: &["amazonses.com"],
        selectors: &[],
        dkim_howto: "Console Amazon SES : Identities, votre domaine, Easy DKIM. Ajoutez les trois CNAME indiqués.",
    },
    Provider {
        key: "icloud",
        name: "iCloud Mail",
        smtp: &["mail.me.com", "icloud.com"],
        mx: &["icloud.com", "mail.me.com"],
        spf: &["icloud.com"],
        selectors: &["sig1"],
        dkim_howto: "Réglages iCloud+ : Domaine e-mail personnalisé. Apple affiche le CNAME sig1._domainkey à ajouter.",
    },
    Provider {
        key: "fastmail",
        name: "Fastmail",
        smtp: &["fastmail.com", "messagingengine.com"],
        mx: &["messagingengine.com"],
        spf: &["spf.messagingengine.com"],
        selectors: &["fm1", "fm2", "fm3"],
        dkim_howto: "Fastmail : Réglages, Domaines, votre domaine. Ajoutez les CNAME fm1, fm2 et fm3._domainkey.",
    },
    Provider {
        key: "proton",
        name: "Proton Mail",
        smtp: &["protonmail.ch", "proton.me"],
        mx: &["protonmail.ch"],
        spf: &["_spf.protonmail.ch"],
        selectors: &["protonmail", "protonmail2", "protonmail3"],
        dkim_howto: "Proton : Paramètres, Domaines, votre domaine, DKIM. Ajoutez les trois CNAME indiqués.",
    },
];

/// Selectors tried for every domain, after the provider's own.
pub const COMMON_SELECTORS: &[&str] =
    &["default", "dkim", "mail", "k1", "s1", "s2", "selector1", "selector2", "google", "key1", "smtp"];

/// The sending provider, from the SMTP host first (it is what signs the mail),
/// else from the MX hosts.
pub fn detect_provider(smtp_host: &str, mx_hosts: &[String]) -> Option<&'static Provider> {
    let h = smtp_host.trim().to_ascii_lowercase();
    if !h.is_empty() {
        if let Some(p) = PROVIDERS.iter().find(|p| p.smtp.iter().any(|s| h.contains(s))) {
            return Some(p);
        }
    }
    PROVIDERS.iter().find(|p| {
        mx_hosts
            .iter()
            .any(|m| p.mx.iter().any(|s| m.to_ascii_lowercase().contains(s)))
    })
}

/// Selectors to try, provider's first, without duplicates.
pub fn selectors_for(p: Option<&Provider>) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for s in p.map(|p| p.selectors).unwrap_or(&[]).iter().chain(COMMON_SELECTORS) {
        if !out.contains(s) {
            out.push(s);
        }
    }
    out
}

/// Domain of an address (`Piers <booking@insrt.fr>` → `insrt.fr`).
pub fn domain_of(addr: &str) -> Option<String> {
    let a = addr.trim().trim_end_matches('>');
    let (_, d) = a.rsplit_once('@')?;
    let d = d.trim().trim_end_matches('.').to_ascii_lowercase();
    (d.contains('.') && d.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')).then_some(d)
}

// ---------------- Verdicts ----------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Ok,
    Warn,
    Fail,
    /// Lookup failed: no verdict.
    Unknown,
}

/// A DNS record to add or replace, ready to copy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fix {
    /// TXT, CNAME...
    pub rtype: String,
    /// Host name as typed at the DNS provider ("@" for the domain itself).
    pub host: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Check {
    /// spf | dkim | dmarc | mx
    pub key: String,
    pub status: Status,
    /// One line, plain French.
    pub summary: String,
    /// What is wrong and what to do, plain French.
    pub detail: Option<String>,
    /// Record found (for display).
    pub found: Option<String>,
    pub fix: Option<Fix>,
}

impl Check {
    fn new(key: &str, status: Status, summary: impl Into<String>) -> Self {
        Check { key: key.into(), status, summary: summary.into(), detail: None, found: None, fix: None }
    }
    fn detail(mut self, d: impl Into<String>) -> Self {
        self.detail = Some(d.into());
        self
    }
    fn found(mut self, f: impl Into<String>) -> Self {
        self.found = Some(f.into());
        self
    }
    fn fix(mut self, rtype: &str, host: &str, value: impl Into<String>) -> Self {
        self.fix = Some(Fix { rtype: rtype.into(), host: host.into(), value: value.into() });
        self
    }
}

fn is_spf(t: &str) -> bool {
    let l = t.trim().to_ascii_lowercase();
    l == "v=spf1" || l.starts_with("v=spf1 ")
}

/// SPF verdict. `includes` are the provider's SPF includes (empty when the
/// provider is unknown: then only the record's shape is judged).
pub fn spf_check(txts: &[String], includes: &[&str]) -> Check {
    let recs: Vec<&String> = txts.iter().filter(|t| is_spf(t)).collect();
    let wanted = includes.first().map(|i| format!("include:{} ", i)).unwrap_or_default();
    match recs.len() {
        0 => {
            return Check::new("spf", Status::Fail, "Aucun enregistrement SPF")
                .detail("Sans SPF, Gmail et Outlook ne peuvent pas vérifier que ce serveur a le droit d'envoyer pour votre domaine : vos e-mails partent en spam. Ajoutez ce TXT sur le domaine.")
                .fix("TXT", "@", format!("v=spf1 {}~all", wanted));
        }
        1 => {}
        n => {
            let merged = merge_spf(&recs.iter().map(|s| s.as_str()).collect::<Vec<_>>(), includes);
            return Check::new("spf", Status::Fail, format!("{} enregistrements SPF (un seul autorisé)", n))
                .detail("Plusieurs TXT « v=spf1 » rendent SPF invalide. Supprimez-les et remplacez-les par un seul enregistrement qui les fusionne.")
                .found(recs.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n"))
                .fix("TXT", "@", merged);
        }
    }
    let rec = recs[0].trim();
    let terms: Vec<String> = rec.split_whitespace().map(|t| t.to_ascii_lowercase()).collect();
    let has_include = |inc: &str| {
        let inc = inc.to_ascii_lowercase();
        terms.iter().any(|t| t.trim_start_matches(['+', '~', '?']) == format!("include:{}", inc))
    };
    let all = terms.iter().rev().find(|t| t.ends_with("all") && t.len() <= 4).cloned();
    if !includes.is_empty() && !includes.iter().any(|i| has_include(i)) {
        return Check::new("spf", Status::Fail, "SPF présent mais n'autorise pas votre serveur d'envoi")
            .detail(format!(
                "L'enregistrement SPF ne contient pas « include:{} ». Remplacez-le par la version ci-dessous (vos autorisations actuelles sont conservées).",
                includes[0]
            ))
            .found(rec)
            .fix("TXT", "@", merge_spf(&[rec], includes));
    }
    match all.as_deref() {
        Some("+all") | Some("all") => Check::new("spf", Status::Fail, "SPF autorise tout le monde (+all)")
            .detail("« +all » permet à n'importe quel serveur d'envoyer en votre nom : les filtres le traitent comme une absence de SPF. Terminez par « ~all ».")
            .found(rec)
            .fix("TXT", "@", replace_all_mech(rec, "~all")),
        Some("?all") => Check::new("spf", Status::Warn, "SPF neutre (?all)")
            .detail("« ?all » ne protège pas le domaine. Terminez plutôt par « ~all ».")
            .found(rec)
            .fix("TXT", "@", replace_all_mech(rec, "~all")),
        None => Check::new("spf", Status::Warn, "SPF sans règle finale")
            .detail("Ajoutez « ~all » à la fin de l'enregistrement.")
            .found(rec)
            .fix("TXT", "@", format!("{} ~all", rec)),
        _ if includes.is_empty() => Check::new("spf", Status::Ok, "SPF présent")
            .detail("Fournisseur d'envoi non reconnu : vérifiez que son serveur figure bien dans cet enregistrement.")
            .found(rec),
        _ => Check::new("spf", Status::Ok, "SPF correct").found(rec),
    }
}

fn replace_all_mech(rec: &str, with: &str) -> String {
    rec.split_whitespace()
        .map(|t| {
            let l = t.to_ascii_lowercase();
            if l.ends_with("all") && l.len() <= 4 {
                with.to_string()
            } else {
                t.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// One SPF record carrying every mechanism of `recs` plus the provider's
/// include, ending with the strictest sane "all" found (default ~all).
pub fn merge_spf(recs: &[&str], includes: &[&str]) -> String {
    let mut mechs: Vec<String> = Vec::new();
    let mut all = "~all".to_string();
    for r in recs {
        for t in r.split_whitespace().skip(1) {
            let l = t.to_ascii_lowercase();
            if l.ends_with("all") && l.len() <= 4 {
                if l == "-all" {
                    all = "-all".into();
                }
                continue;
            }
            if !mechs.iter().any(|m| m.eq_ignore_ascii_case(t)) {
                mechs.push(t.to_string());
            }
        }
    }
    let has_any = includes
        .iter()
        .any(|i| mechs.iter().any(|m| m.to_ascii_lowercase() == format!("include:{}", i.to_ascii_lowercase())));
    if let (false, Some(i)) = (has_any, includes.first()) {
        mechs.push(format!("include:{}", i));
    }
    let mut out = vec!["v=spf1".to_string()];
    out.extend(mechs);
    out.push(all);
    out.join(" ")
}

/// A DKIM key record: `v=DKIM1` (or a bare `p=`) with a non-empty key.
pub fn dkim_key_state(txt: &str) -> Option<bool> {
    let t = txt.trim();
    let tags: Vec<(String, String)> = t
        .split(';')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    let is_dkim = tags.iter().any(|(k, v)| k == "v" && v.eq_ignore_ascii_case("DKIM1"))
        || tags.iter().any(|(k, _)| k == "p");
    if !is_dkim {
        return None;
    }
    Some(tags.iter().any(|(k, v)| k == "p" && !v.is_empty()))
}

/// DKIM verdict from the selectors that answered. `found`: (selector, TXT).
pub fn dkim_check(found: &[(String, String)], provider: Option<&Provider>, domain: &str) -> Check {
    let valid: Vec<&(String, String)> = found.iter().filter(|(_, t)| dkim_key_state(t) == Some(true)).collect();
    if let Some((sel, _)) = valid.first() {
        let sels = valid.iter().map(|(s, _)| s.as_str()).collect::<Vec<_>>().join(", ");
        return Check::new("dkim", Status::Ok, format!("DKIM actif (sélecteur {})", sels))
            .found(format!("{}._domainkey.{}", sel, domain));
    }
    let revoked = found.iter().any(|(_, t)| dkim_key_state(t) == Some(false));
    let howto = provider.map(|p| p.dkim_howto).unwrap_or(
        "Dans l'interface de votre messagerie, cherchez « DKIM » ou « Authentification des e-mails », générez une clé, puis ajoutez l'enregistrement fourni dans la zone DNS du domaine.",
    );
    let summary = if revoked {
        "Clé DKIM révoquée (p= vide)"
    } else {
        "Aucune clé DKIM trouvée"
    };
    let mut c = Check::new("dkim", Status::Fail, summary).detail(format!(
        "Sans signature DKIM, les grandes messageries (Gmail, Outlook, Yahoo) classent vite les envois en spam. {}",
        howto
    ));
    if let Some(sel) = provider.and_then(|p| p.selectors.first()) {
        c.fix = Some(Fix {
            rtype: if provider.is_some_and(|p| ["microsoft", "gandi", "sendgrid", "fastmail", "proton", "icloud"].contains(&p.key)) {
                "CNAME".into()
            } else {
                "TXT".into()
            },
            host: format!("{}._domainkey", sel),
            value: "valeur fournie par votre messagerie (clé publique)".into(),
        });
    }
    c
}

/// DMARC policy: (p, has rua).
pub fn dmarc_policy(txt: &str) -> Option<(String, bool)> {
    let t = txt.trim();
    if !t.to_ascii_lowercase().starts_with("v=dmarc1") {
        return None;
    }
    let mut p = String::new();
    let mut rua = false;
    for kv in t.split(';') {
        if let Some((k, v)) = kv.split_once('=') {
            match k.trim().to_ascii_lowercase().as_str() {
                "p" => p = v.trim().to_ascii_lowercase(),
                "rua" => rua = !v.trim().is_empty(),
                _ => {}
            }
        }
    }
    Some((p, rua))
}

pub fn dmarc_check(txts: &[String], domain: &str) -> Check {
    let recs: Vec<(&String, (String, bool))> =
        txts.iter().filter_map(|t| dmarc_policy(t).map(|p| (t, p))).collect();
    let suggested = format!("v=DMARC1; p=none; rua=mailto:dmarc@{}; adkim=r; aspf=r", domain);
    match recs.len() {
        0 => Check::new("dmarc", Status::Fail, "Aucun enregistrement DMARC")
            .detail("Gmail et Yahoo exigent un DMARC pour les envois en volume. Commencez par une politique « none » (aucun impact sur la remise, vous recevez des rapports), puis passez à « quarantine » une fois SPF et DKIM au vert.")
            .fix("TXT", "_dmarc", suggested),
        1 => {
            let (rec, (p, rua)) = &recs[0];
            match p.as_str() {
                "quarantine" | "reject" => Check::new("dmarc", Status::Ok, format!("DMARC actif (p={})", p)).found(rec.as_str()),
                "none" => {
                    let mut c = Check::new("dmarc", Status::Ok, "DMARC présent (p=none, surveillance)").found(rec.as_str());
                    if !rua {
                        c = c.detail("Ajoutez « rua=mailto:dmarc@... » pour recevoir les rapports.");
                    } else {
                        c = c.detail("Quand SPF et DKIM sont au vert depuis quelques semaines, passez à « p=quarantine ».");
                    }
                    c
                }
                _ => Check::new("dmarc", Status::Warn, "DMARC sans politique valide")
                    .detail("La balise « p= » est absente ou invalide : l'enregistrement est ignoré.")
                    .found(rec.as_str())
                    .fix("TXT", "_dmarc", suggested),
            }
        }
        _ => Check::new("dmarc", Status::Fail, "Plusieurs enregistrements DMARC")
            .detail("Un seul TXT « v=DMARC1 » est autorisé sur _dmarc : gardez-en un.")
            .found(recs.iter().map(|(r, _)| r.as_str()).collect::<Vec<_>>().join("\n"))
            .fix("TXT", "_dmarc", suggested),
    }
}

/// MX hosts, by priority (`"10 mx1.x."` → `mx1.x`).
pub fn mx_hosts(answers: &[String]) -> Vec<String> {
    let mut v: Vec<(u32, String)> = answers
        .iter()
        .filter_map(|a| {
            let mut it = a.split_whitespace();
            let first = it.next()?;
            match it.next() {
                Some(h) => Some((first.parse().unwrap_or(u32::MAX), h.trim_end_matches('.').to_ascii_lowercase())),
                None => Some((u32::MAX, first.trim_end_matches('.').to_ascii_lowercase())),
            }
        })
        .filter(|(_, h)| !h.is_empty())
        .collect();
    v.sort();
    v.into_iter().map(|(_, h)| h).collect()
}

pub fn mx_check(hosts: &[String]) -> Check {
    if hosts.is_empty() {
        Check::new("mx", Status::Fail, "Aucun serveur de réception (MX)")
            .detail("Sans MX, les réponses des salles ne peuvent pas arriver et beaucoup de filtres rejettent l'expéditeur. Ajoutez les MX de votre messagerie.")
    } else {
        Check::new("mx", Status::Ok, format!("Réception configurée ({})", hosts[0])).found(hosts.join(", "))
    }
}

// ---------------- Report ----------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DnsReport {
    pub domain: String,
    pub provider: Option<String>,
    pub checks: Vec<Check>,
    pub checked_at: String,
    /// SPF and DKIM both pass: the autopilot may run.
    pub ready: bool,
}

impl DnsReport {
    pub fn status_of(&self, key: &str) -> Status {
        self.checks.iter().find(|c| c.key == key).map(|c| c.status).unwrap_or(Status::Unknown)
    }
}

fn unknown(key: &str, e: &str) -> Check {
    Check::new(key, Status::Unknown, "Vérification impossible").detail(format!("Requête DNS en échec ({}). Réessayez plus tard.", e))
}

/// Run every lookup through `r` and judge them.
pub fn check_domain(r: &dyn Resolver, from_email: &str, smtp_host: &str, now: &str) -> Result<DnsReport, String> {
    let domain = domain_of(from_email).ok_or("Adresse d'envoi invalide (Réglages, Email).")?;
    let mx_res = r.lookup(&domain, RType::Mx);
    let mx = mx_res.as_ref().map(|a| mx_hosts(a)).unwrap_or_default();
    let provider = detect_provider(smtp_host, &mx);

    let spf = match r.lookup(&domain, RType::Txt) {
        Ok(t) => spf_check(&t, provider.map(|p| p.spf).unwrap_or(&[])),
        Err(e) => unknown("spf", &e),
    };

    let mut found: Vec<(String, String)> = Vec::new();
    let mut errors = 0;
    let sels = selectors_for(provider);
    for sel in &sels {
        match r.lookup(&format!("{}._domainkey.{}", sel, domain), RType::Txt) {
            Ok(txts) => {
                for t in txts {
                    if dkim_key_state(&t).is_some() {
                        found.push((sel.to_string(), t));
                    }
                }
            }
            Err(_) => errors += 1,
        }
        // The provider's own selector answered with a valid key: enough.
        if found.iter().any(|(_, t)| dkim_key_state(t) == Some(true)) {
            break;
        }
    }
    let dkim = if found.is_empty() && errors == sels.len() {
        unknown("dkim", "aucune réponse")
    } else {
        dkim_check(&found, provider, &domain)
    };

    let dmarc = match r.lookup(&format!("_dmarc.{}", domain), RType::Txt) {
        Ok(t) => dmarc_check(&t, &domain),
        Err(e) => unknown("dmarc", &e),
    };
    let mx_c = match &mx_res {
        Ok(_) => mx_check(&mx),
        Err(e) => unknown("mx", e),
    };

    let ready = spf.status != Status::Fail && dkim.status != Status::Fail;
    Ok(DnsReport {
        domain,
        provider: provider.map(|p| p.name.to_string()),
        checks: vec![spf, dkim, dmarc, mx_c],
        checked_at: now.to_string(),
        ready,
    })
}

/// Why the autopilot must not be switched on, from the last report for the
/// current sending domain. A report that could not resolve does not block.
pub fn autopilot_block(report: Option<&DnsReport>, from_email: &str) -> Option<String> {
    let r = report?;
    if domain_of(from_email).as_deref() != Some(r.domain.as_str()) {
        return None;
    }
    let mut bad = Vec::new();
    if r.status_of("spf") == Status::Fail {
        bad.push("SPF");
    }
    if r.status_of("dkim") == Status::Fail {
        bad.push("DKIM");
    }
    (!bad.is_empty()).then(|| {
        format!(
            "Authentification du domaine {} incomplète ({} à corriger) : sans elle les envois automatiques partiraient en spam. Voir Réglages, Délivrabilité.",
            r.domain,
            bad.join(" et ")
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct Fixture(HashMap<(String, bool), Result<Vec<String>, String>>);
    impl Fixture {
        fn new() -> Self {
            Fixture(HashMap::new())
        }
        fn txt(mut self, n: &str, v: &[&str]) -> Self {
            self.0.insert((n.into(), true), Ok(v.iter().map(|s| s.to_string()).collect()));
            self
        }
        fn mx(mut self, n: &str, v: &[&str]) -> Self {
            self.0.insert((n.into(), false), Ok(v.iter().map(|s| s.to_string()).collect()));
            self
        }
    }
    impl Resolver for Fixture {
        fn lookup(&self, name: &str, rtype: RType) -> Result<Vec<String>, String> {
            self.0.get(&(name.to_string(), rtype == RType::Txt)).cloned().unwrap_or(Ok(vec![]))
        }
    }
    struct Offline;
    impl Resolver for Offline {
        fn lookup(&self, _: &str, _: RType) -> Result<Vec<String>, String> {
            Err("offline".into())
        }
    }

    #[test]
    fn doh_json_parsing() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"Status":0,"Answer":[
                {"name":"selector1._domainkey.x.fr","type":5,"data":"selector1-x._domainkey.x.onmicrosoft.com."},
                {"name":"selector1-x","type":16,"data":"\"v=DKIM1; k=rsa; \" \"p=MIGf\""}]}"#,
        )
        .unwrap();
        assert_eq!(parse_doh_json(&v, RType::Txt).unwrap(), vec!["v=DKIM1; k=rsa; p=MIGf".to_string()]);
        let nx: serde_json::Value = serde_json::from_str(r#"{"Status":3}"#).unwrap();
        assert!(parse_doh_json(&nx, RType::Txt).unwrap().is_empty());
        let fail: serde_json::Value = serde_json::from_str(r#"{"Status":2}"#).unwrap();
        assert!(parse_doh_json(&fail, RType::Txt).is_err());
        assert_eq!(join_txt(r#""a\"b" "c""#), "a\"bc");
        assert_eq!(mx_hosts(&["20 b.x.".into(), "10 a.x.".into()]), vec!["a.x", "b.x"]);
    }

    #[test]
    fn provider_detection() {
        assert_eq!(detect_provider("smtp.gmail.com", &[]).unwrap().key, "google");
        assert_eq!(detect_provider("smtp.office365.com", &[]).unwrap().key, "microsoft");
        assert_eq!(detect_provider("ssl0.ovh.net", &[]).unwrap().key, "ovh");
        assert_eq!(detect_provider("mail.example.fr", &["insrt-fr.mail.protection.outlook.com".into()]).unwrap().key, "microsoft");
        assert!(detect_provider("mail.example.fr", &["mx.example.fr".into()]).is_none());
        let s = selectors_for(detect_provider("smtp.office365.com", &[]));
        assert_eq!(&s[..2], &["selector1", "selector2"]);
        assert_eq!(s.iter().filter(|x| **x == "selector1").count(), 1);
        assert_eq!(domain_of("Piers <Booking@Insrt.FR>").as_deref(), Some("insrt.fr"));
        assert_eq!(domain_of("nope"), None);
    }

    #[test]
    fn spf_verdicts() {
        let g = &["_spf.google.com"][..];
        let c = spf_check(&["google-site-verification=x".into()], g);
        assert_eq!(c.status, Status::Fail);
        assert_eq!(c.fix.unwrap().value, "v=spf1 include:_spf.google.com ~all");
        let ok = spf_check(&["v=spf1 include:_spf.google.com ~all".into()], g);
        assert_eq!(ok.status, Status::Ok);
        // Present but the provider is missing: merged fix keeps the existing mechanisms.
        let miss = spf_check(&["v=spf1 include:mx.ovh.com -all".into()], g);
        assert_eq!(miss.status, Status::Fail);
        assert_eq!(miss.fix.unwrap().value, "v=spf1 include:mx.ovh.com include:_spf.google.com -all");
        // Two records: invalid, merged.
        let two = spf_check(&["v=spf1 include:mx.ovh.com ~all".into(), "v=spf1 ip4:1.2.3.4 ~all".into()], g);
        assert_eq!(two.status, Status::Fail);
        assert_eq!(two.fix.unwrap().value, "v=spf1 include:mx.ovh.com ip4:1.2.3.4 include:_spf.google.com ~all");
        assert_eq!(spf_check(&["v=spf1 include:_spf.google.com +all".into()], g).status, Status::Fail);
        assert_eq!(spf_check(&["v=spf1 include:_spf.google.com ?all".into()], g).status, Status::Warn);
        assert_eq!(spf_check(&["v=spf1 include:_spf.google.com".into()], g).status, Status::Warn);
        // Unknown provider: shape only.
        assert_eq!(spf_check(&["v=spf1 a mx ~all".into()], &[]).status, Status::Ok);
    }

    #[test]
    fn dkim_and_dmarc_verdicts() {
        assert_eq!(dkim_key_state("v=DKIM1; k=rsa; p=MIIB"), Some(true));
        assert_eq!(dkim_key_state("v=DKIM1; p="), Some(false));
        assert_eq!(dkim_key_state("v=spf1 ~all"), None);
        let ms = detect_provider("smtp.office365.com", &[]);
        let none = dkim_check(&[], ms, "x.fr");
        assert_eq!(none.status, Status::Fail);
        let fix = none.fix.unwrap();
        assert_eq!((fix.rtype.as_str(), fix.host.as_str()), ("CNAME", "selector1._domainkey"));
        assert_eq!(dkim_check(&[("selector1".into(), "v=DKIM1; p=abc".into())], ms, "x.fr").status, Status::Ok);
        assert!(dkim_check(&[("s1".into(), "v=DKIM1; p=".into())], None, "x.fr").summary.contains("révoquée"));

        let d = dmarc_check(&[], "insrt.fr");
        assert_eq!(d.status, Status::Fail);
        assert_eq!(d.fix.unwrap().host, "_dmarc");
        assert_eq!(dmarc_check(&["v=DMARC1; p=quarantine; rua=mailto:a@b".into()], "x").status, Status::Ok);
        assert_eq!(dmarc_check(&["v=DMARC1; p=none".into()], "x").status, Status::Ok);
        assert_eq!(dmarc_check(&["v=DMARC1; p=bogus".into()], "x").status, Status::Warn);
        assert_eq!(dmarc_check(&["v=DMARC1; p=none".into(), "v=DMARC1; p=reject".into()], "x").status, Status::Fail);
    }

    #[test]
    fn full_report_and_autopilot_gate() {
        let good = Fixture::new()
            .mx("insrt.fr", &["1 aspmx.l.google.com."])
            .txt("insrt.fr", &["v=spf1 include:_spf.google.com ~all"])
            .txt("google._domainkey.insrt.fr", &["v=DKIM1; k=rsa; p=MIIB"])
            .txt("_dmarc.insrt.fr", &["v=DMARC1; p=none; rua=mailto:d@insrt.fr"]);
        let r = check_domain(&good, "booking@insrt.fr", "smtp.gmail.com", "now").unwrap();
        assert_eq!(r.provider.as_deref(), Some("Google Workspace"));
        assert!(r.ready);
        assert!(r.checks.iter().all(|c| c.status == Status::Ok), "{:?}", r.checks);
        assert_eq!(autopilot_block(Some(&r), "booking@insrt.fr"), None);

        let bad = Fixture::new().mx("insrt.fr", &["10 mx1.mail.ovh.net."]).txt("insrt.fr", &["v=spf1 include:mx.ovh.com ~all"]);
        let r = check_domain(&bad, "booking@insrt.fr", "ssl0.ovh.net", "now").unwrap();
        assert_eq!(r.status_of("spf"), Status::Ok);
        assert_eq!(r.status_of("dkim"), Status::Fail);
        assert_eq!(r.status_of("dmarc"), Status::Fail);
        assert!(!r.ready);
        assert!(autopilot_block(Some(&r), "booking@insrt.fr").unwrap().contains("DKIM"));
        // Another sending domain since the check: the old report does not block.
        assert_eq!(autopilot_block(Some(&r), "booking@other.fr"), None);

        // Offline: unknown, never a false "fail", never a block.
        let r = check_domain(&Offline, "booking@insrt.fr", "smtp.gmail.com", "now").unwrap();
        assert!(r.checks.iter().all(|c| c.status == Status::Unknown));
        assert!(r.ready);
    }
}
