# Venue Intelligence, TODO

Suivi du module intégré dans l'app CRM desktop (Tauri, Rust, SQLite). Voir `../RECON.md`
pour les décisions d'architecture.

## Jalons

- [x] **J0** RECON.md, décisions d'architecture validées (2026-07-28)
- [x] **J1** Schéma vi_*, migrations dans db.rs, seed artistes de référence + zones RA
- [x] **J2** Moissonnage RA, file de tâches, worker Rust reprenable, écran Venues (+ qualification de base foldée du J3 pour rendre la build testable ce soir)
- [ ] **J3** Qualification, preuves (vi_evidence), score, promoteurs
- [ ] **J4** Cockpit de triage (3 colonnes, raccourcis clavier), sans enrichissement auto
- [ ] **J5** Enrichissement contacts (résolution site, crawl, LLM Anthropic)
- [ ] **J6** Intégration CRM (promotion venue vers table contacts, dédoublonnage)
- [ ] **J7** Fraîcheur (re-vérification 180 j), export CSV, garde-fous RGPD

## Décisions figées (validées avec l'utilisateur)

- Volume cible : centaines de lieux, donc pagination SQL simple, pas de virtualisation lourde.
- LLM : Sonnet courant par défaut, configurable, avec bascule Opus par run pour les lots
  ambigus. Id de modèle à confirmer via la référence claude-api au J5.
- Recherche web (résolution du site, J5) : repli manuel du domaine dans le triage d'abord,
  provider API configurable ensuite (Brave ou Serper), jamais de scraping Google.
- Worker de moissonnage : tourne uniquement quand l'app est ouverte pour démarrer. Reprise
  garantie par la file persistée en SQLite.
- RGPD : contexte mono utilisateur. Traçabilité par source_url + date de collecte, liste
  d'exclusion vi_exclusions, registre = export CSV local. Pas de notion d'utilisateur en base.

## Pistes de sourcing à ajouter (demande utilisateur, 2026-07-28)

- **Bandsintown comme seconde source.** Utile pour référencer des clubs et trouver plus de
  lieux liés à des artistes clés (page artiste Bandsintown, ex. Cloonee). À traiter comme un
  second moissonneur alimentant les mêmes tables vi_ (venues, evidence source_type='autre'
  avec l'URL Bandsintown). À vérifier : API ou HTML, robustesse, dédoublonnage venue avec RA
  (par nom_normalise + ville, puisque pas de ra_venue_id côté Bandsintown).
- Enrichir la liste de référence dans la veine house / tech house : Franky Rizardo,
  Mason Collective, etc. (Cloonee déjà en tier 1). Ajout possible dès maintenant via l'onglet
  "Artistes de référence" de l'app, ou en seed.

## Dette et points à traiter plus tard

- Worker sans app ouverte : évaluer un binaire worker séparé ou un cron OS (hors périmètre
  initial).
- Colonne centrale du triage (J4) : décider webview Tauri embarqué contre rendu texte serveur.
  L'iframe classique est exclu (X-Frame-Options).
- Clés d'API stockées en clair dans settings (SQLite local). Acceptable poste perso, à revoir
  si multi poste.
- Pagination des grandes listes (vi_venues) à ajouter au J4.
- vi_reference_artists : champ genres non seedé, à compléter depuis l'UI si utile au filtrage.

## Hors périmètre (anti objectifs du brief)

- Aucun envoi d'email depuis le module (reste dans le pipeline CRM existant).
- Aucune adresse email inventée ou déduite d'un motif.
- Pas de fuzzy matching sur les noms d'artistes.
- Pas de validation automatique qui remplace le triage humain.
- Pas de refactor du CRM existant au passage.
