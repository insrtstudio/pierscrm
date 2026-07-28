# Venue Intelligence, journal de session

Journal de continuité entre sessions Claude Code. Le plus récent en haut.

## 2026-07-29, étude booking + v0.7.3 pack de templates pro

Contexte : l'utilisateur veut aligner l'app sur les standards des meilleurs bookers, pour des
artistes EN DÉVELOPPEMENT (petits), avec un professionnalisme maximal pour débloquer des dates.

- **Étude booking** produite par un sous-agent de recherche (29 outils, 25 sources : RA, Attack
  Magazine, DJ TechTools, Sonicbids, gigmit, ZIPDJ). Publiée en Artifact lisible (Archivo-like,
  accent rouge Insrt, thèmes clair/sombre) :
  https://claude.ai/code/artifact/485d055c-f09b-42be-88dd-af4a24177359
  Points clés pour émergents : fit musical avant le draw, humilité + chiffres honnêtes (ou
  masqués si faibles), mail formule 4 lignes / 1 lien / téléphone / perso, le showcase no-fee
  comme levier n°1 pour une première date, cadence 1-2 relances espacées avec du neuf, EPK carré,
  ciblage par ville d'audience.
- L'utilisateur a validé les 4 chantiers d'alignement : (1) pack templates, (2) système de
  relance, (3) fiche EPK aux normes, (4) ciblage par ville. FAIT ce tour : #1.
- **v0.7.3, pack de templates pro** (db.rs `seed_defaults` réécrit) : insertion idempotente PAR
  NOM (n'écrase pas les éditions, arrive aussi sur installs existantes). 3 templates aux normes,
  MULTI-ARTISTES via {{artist}} : "Prise de contact" (4 lignes, fit-first, humble), "Showcase
  (sans cachet)" (levier émergent), "Relance" (courte, avec {{news}} = du neuf). Rappel :
  render_template laisse {{var}} en littéral si non résolue ; {{artist}} vient de la campagne,
  {{news}} est un input au cas par cas.

RESTE (chantiers validés) : #2 relances (file à relancer J+7, max 2), #3 EPK aux normes (mix un
clic, bio, dates, chiffres honnêtes, presse, rider, fee range, contact haut+bas, export PDF), #4
ciblage par ville (villes d'audience par artiste -> tri des salles Venue Intelligence).

## 2026-07-29, v0.7.2, signature email + correction label Insrt

- DKIM résolu côté utilisateur : le off/on Amen a régénéré la clé avec un NOUVEAU sélecteur
  `key-va2kejvjo6` (l'ancien `key_53r23h5unk` est caduc), publié et valide. mail-tester passe
  de 4,3 à 8,6/10, DKIM/SPF/DMARC tous verts. Restent seulement HTML_IMAGE_ONLY (email court +
  pixel, se règle en écrivant un peu plus de texte) et le rDNS mismatch (infra Amen, non
  corrigeable). List-Unsubscribe : l'utilisateur ne le veut pas (démarchage nominatif, pas une
  newsletter), donc non ajouté.
- **Formulaire de signature** (Réglages > Signature email) : champs nom, rôle, label, tel,
  email booking, site, Instagram, SoundCloud, stockés en JSON via set_setting("email_signature"),
  avec aperçu live. Backend `SignatureData` + `render_signature` (plain + HTML), ajoutée
  automatiquement en bas de chaque email (send_email et send_bulk via message_body). Gère
  handles ou URLs pour les réseaux. Le texte de signature aide aussi à réduire HTML_IMAGE_ONLY.
- **Correction label** : "Insrt.Studio" -> "Insrt" partout (placeholder from_name Settings.tsx,
  deux templates seed db.rs). Les deux templates seed réécrits en versions étoffées, naturelles,
  sans tiret cadratin, sans ligne de signature en dur (la signature s'ajoute toute seule).
  Insrt.Studio = maison de musique d'illustration séparée, ne pas confondre.

## 2026-07-29, v0.7.1, deliverability email (multipart)

Test mail-tester réel de l'utilisateur (score 4.3/10) : DKIM=fail "key not found in DNS"
(Amen signe avec le sélecteur `key_53r23h5unk` mais la clé publique n'est PAS publiée dans le
DNS insrt.fr -> à corriger côté Amen, action utilisateur), et surtout des pénalités de format
que le code produisait : HTML_MIME_NO_HTML_TAG (0.635, pas de balise <html>), MIME_HTML_ONLY
(pas de partie text/plain), HTML_IMAGE_ONLY_08 (1.781). Correctif : `message_body()` construit
un **multipart/alternative** = partie text/plain (le corps brut) + partie text/html enveloppée
dans un vrai document `<!doctype html><html>...`. Le pixel de tracking reste uniquement dans la
partie HTML. Appliqué aux deux chemins (send_email et send_bulk via build_message). SPF/DMARC
déjà pass. rDNS mismatch = infra Amen, non corrigeable. List-Unsubscribe : noté, à ajouter (pas
un malus de score, utile surtout au vrai volume). cargo check OK.

## 2026-07-29, J5, enrichissement des contacts (fiche par lieu)

**Contexte** L'utilisateur : le moissonnage rend juste des noms, il veut une fiche par salle
avec site web, téléphone et email. Puis : "utilise toutes les ressources (Shotgun, RA,
Bandsintown, Instagram)". Décision : livrer d'abord la voie fiable RA + crawl du site (site,
tel, email), les autres sources (IG, Bandsintown, Shotgun) en couches suivantes.

**Découverte clé** L'API RA expose déjà par lieu, via `venue(id)` GraphQL : `website`,
`phone`, `address`, `capacity`, `blurb`. Donc site + tel + adresse + capacité viennent
directement de RA (near-100% pour site/adresse/capacité, phone souvent vide). RA n'a PAS
d'email. L'email s'obtient en crawlant le site du lieu.

**Livré**
- `src-tauri/src/commands/ra.rs` : `VENUE_QUERY` + `fetch_venue_detail(id)` (isolé comme le
  reste du client RA).
- `src-tauri/src/commands/enrich.rs` (nouveau) : `process_enrich_task` = fiche RA puis crawl
  du site. Crawl robuste : variantes www/apex, choix de la base dont l'accueil répond,
  découverte des liens internes (contact/mentions/impressum/about) + chemins devinés, max 12
  fetches, politesse 900 ms. Extraction : mailto d'abord (toujours réel) puis regex, filtre
  des faux positifs (sentry, wixpress, images, noreply), scoring par préfixe (booking=100,
  management=60, info=30, reservation=5, +10 si mailto) et rôle deviné. Aucune adresse
  inventée, chaque contact garde son source_url. Tel du site en repli si RA n'en a pas.
  Commandes `vi_start_enrich(force)` (file de tâches enrich_venue sur les lieux qualifiés/
  validés) et `vi_venue_detail(id)` (fiche complète : contacts + preuves + promoteurs).
- `src-tauri/src/commands/harvest.rs` : worker rendu générique, aiguille par type de tâche
  (harvest_area_window vs enrich_venue) ; `spawn_worker` exposé ; `vi_list_venues` renvoie
  telephone, best_email, nb_emails, enriched.
- `src-tauri/src/db.rs` : colonnes `telephone` et `enriched_at` sur vi_venues (migration ALTER).
- Dépendance `regex` ajoutée. 3 tests unitaires (extraction/scoring, variantes de base, liens
  internes) verts.
- `src/pages/Venues.tsx` : bouton "Enrichir les fiches", colonnes contact (email/tel/site +
  badge "à enrichir"), rafraîchissement auto pendant l'enrichissement, modal fiche lieu
  (coordonnées, emails scorés avec copie, preuves, promoteurs). i18n FR/EN sans tiret cadratin.

**Aussi (question déliverabilité de l'utilisateur)**
- `send_bulk` : confirmé, un `To:` par destinataire (jamais de CC/BCC, personne ne voit les
  autres). Ajout d'un throttle configurable `bulk_delay_ms` entre envois (défaut 1200 ms, 0
  désactive, plafond 60 s) : le rythme est le premier levier anti-spam et anti rate-limit.

**Vérifié**
- cargo check + cargo test (enrich) OK, npx tsc --noEmit OK, npm run build OK.
- Test live RA venue(182004) : website/address/capacity remontent bien. Constat réaliste : les
  URLs RA sont parfois périmées (404) ou les sites bloquent les bots (403) ; le crawl a donc un
  taux de réussite partiel sur l'email, ce qui est attendu et géré (fiche partielle + badge).

**Reste (J5)** extraction LLM Sonnet pour cas ambigus, sources IG/Bandsintown/Shotgun,
résolution de site via search API quand RA n'a pas de website.

## 2026-07-28, J2, moissonnage RA (+ qualification de base)

**Livré**
- Client RA isolé, `src-tauri/src/commands/ra.rs` : requêtes GraphQL `area` et
  `eventListings` dans un seul fichier, headers navigateur, gestion d'erreur explicite qui
  pointe vers ce fichier si le schéma bouge. Découverte clé : la résolution des area ids se
  fait via la requête GraphQL `area(countryUrlCode, areaUrlName)`, pas via __NEXT_DATA__ (la
  page HTML est derrière Cloudflare). Introspection RA active, endpoint public sans auth.
- Moteur de moissonnage, `src-tauri/src/commands/harvest.rs` : run (vi_runs) découpé en
  tâches (vi_tasks) une par zone x fenêtre de 90 jours, worker unique sur l'async runtime
  Tauri qui draine la file. Politesse 1,5 s, backoff sur 429/5xx, 3 tentatives puis echec
  sans bloquer le run. Reprise via vi_resume_run (récupère les tâches en_cours bloquées).
  Upsert des lieux (dedup par ra_venue_id), preuves par match exact normalisé contre la liste
  de référence active, promoteurs agrégés. Progression live via event Tauri vi:run-progress.
- Qualification foldée (J3 de base) en fin de run : score_qualif (tier1=10, tier2=6, tier3=4,
  cap 100, +10 si >= 3 artistes distincts), statut qualifie si preuve ou nb_events >= 12,
  sinon rejete. But : rendre la build testable de bout en bout ce soir.
- Commandes : vi_resolve_area, vi_resolve_all_areas, vi_start_harvest, vi_resume_run,
  vi_list_runs, vi_list_venues. Dépendances ajoutées : reqwest (TLS système macOS) + tokio
  (time).
- UI, `src/pages/Venues.tsx` : page à 4 onglets (Lieux, Moissonnage, Zones RA, Artistes de
  référence), progression live, reprise, résolution des zones, édition de la liste de
  référence. Entrée sidebar Venues, route lazy /venues, i18n FR/EN sans tiret cadratin.

**Vérifié**
- cargo check, cargo build, pnpm build OK. Boot OK, schéma et seed intacts.
- Test live réel contre RA (`cargo test live_ra_smoke`) : ibiza area id = 25, juin 2024 =
  520 events, parsing serde OK, exemple Chinois Ibiza avec Dan Ghenacia et Raresh (tier 2),
  donc lieu qualifié avec preuves. La chaîne complète fonctionne.

**Limites connues (notées pour plus tard)**
- Worker actif seulement app ouverte (reprise persistée sinon).
- nb_events_periode s'incrémente par event moissonné, peut sur-compter sur des re-runs qui
  se chevauchent. Signal principal = les preuves (dédoublonnées, exactes), le seuil
  nb_events reste secondaire.
- Quelques slugs de zones (cotedazur, riminiravenna, sardinia...) inconnus de RA, à corriger
  ou saisir l'area id à la main dans l'onglet Zones.
- Pas encore de triage humain (J4) ni d'enrichissement contacts (J5).

## 2026-07-28, J1, schéma et seed

**Livré**
- Schéma des 9 tables `vi_*` ajouté au batch de migration existant, dans
  `src-tauri/src/db.rs` (fonction `migrate`) :
  `vi_venues`, `vi_evidence`, `vi_contacts`, `vi_promoters`, `vi_runs`, `vi_tasks`,
  `vi_ra_areas`, `vi_reference_artists`, `vi_exclusions` (RGPD), plus les index.
  Convention retenue : PK INTEGER autoincrement plus slug unique (pas d'uuid), enums en
  TEXT, JSON en TEXT, timestamps en TEXT ISO. Noms de colonnes en français (snake_case,
  cohérent avec la convention snake_case du repo). Lien CRM prévu via
  `vi_venues.crm_contact_id` vers `contacts(id)`.
- Fonction `normalise()` publique dans `db.rs` : minuscules, repli des diacritiques latins
  vers l'ASCII (table manuelle, pas de crate ajouté), suppression du non alphanumérique.
- Seed sur premier lancement uniquement (`seed_venue_intelligence` dans `db.rs`) :
  118 artistes de référence (61 tier 1, 35 tier 2, 22 tier 3) et 34 zones RA
  (`ra_area_id` NULL, à résoudre au J2).
- Modèles serde `ViReferenceArtist`, `ViArea`, `ViOverview` dans `src-tauri/src/models.rs`.
- Commandes Tauri dans `src-tauri/src/commands/vi.rs`, enregistrées dans `lib.rs` :
  `vi_list_reference_artists`, `vi_save_reference_artist`, `vi_delete_reference_artist`,
  `vi_list_areas`, `vi_save_area`, `vi_overview`. La liste d'artistes et les zones sont
  donc éditables, ce qui prépare le réglage de précision et la saisie manuelle des area ids.

**Vérifié**
- `cargo check` et `cargo build` OK.
- Migration appliquée au lancement, 9 tables `vi_*` présentes.
- Comptes en base : 118 artistes (61 / 35 / 22), 34 zones, 0 zone résolue.
- `normalise()` : Flügel vers romanflugel, Amémé vers ameme, D'Julz vers djulz, Rossi. vers
  rossi, &ME vers me, Chloé vers chloe.

**Fichiers touchés**
- `src-tauri/src/db.rs` (tables, index, normalise, seed)
- `src-tauri/src/models.rs` (modèles vi_)
- `src-tauri/src/commands/vi.rs` (nouveau)
- `src-tauri/src/commands/mod.rs`, `src-tauri/src/lib.rs` (enregistrement)

**Pas fait volontairement (respect de l'ordre des jalons)**
- Aucune UI (les écrans Venue Intelligence viennent à partir du J2 pour les Runs, J4 pour le
  triage). Les commandes de lecture existent seulement pour vérifier le seed.
- Aucun code de moissonnage, qualification ou enrichissement.

**Prochaine étape, J2**
Moissonnage RA : requête GraphQL isolée dans un seul fichier, découpage en tâches
`harvest_area_window` (fenêtre 90 jours), worker Rust in process qui draine `vi_tasks`,
reprise après fermeture, écran Runs avec progression live via event Tauri. Critère : run sur
2 zones interrompu puis repris sans retraitement.

## 2026-07-28, J0, reconnaissance
- `RECON.md` produit à la racine. Repo = Tauri desktop, pas web. Adaptations majeures notées.
- Décisions validées par l'utilisateur, réponses aux 5 questions consignées dans TODO.md.
