# Venue Intelligence, journal de session

Journal de continuité entre sessions Claude Code. Le plus récent en haut.

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
