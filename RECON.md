# RECON.md, module Venue Intelligence

Phase 0, reconnaissance du repo avant tout code. Aucune ligne du module n'est écrite tant
que tu n'as pas validé les décisions d'architecture en fin de fichier.

Convention d'écriture respectée ici et pour la suite : pas de tiret cadratin, uniquement
virgules, deux points ou parenthèses.

---

## Verdict en une ligne

Le repo est une application **desktop Tauri v2** (backend Rust, frontend React/TypeScript,
base **SQLite locale**). Ce n'est pas une app web, il n'y a ni serverless, ni Drizzle, ni
Postgres. Bonne nouvelle : la question que tu désignes comme la plus importante (où faire
tourner un moissonnage de plusieurs heures sans plafond serverless) ne se pose pas, le
backend Rust est un processus long qui peut drainer une file de tâches indéfiniment.

Le document suppose une stack Next.js + Drizzle + Postgres/Neon + serverless. Presque toutes
ces hypothèses sont fausses ici. Le détail des adaptations est en section « Décisions ».

---

## 1.1 Nature de l'application

**1. Web ou desktop, version.**
Desktop, **Tauri v2** (`tauri = "2"` dans `src-tauri/Cargo.toml`). Frontend React 18 +
Vite 6 + TypeScript, styling Tailwind. Backend Rust (edition 2021). Pas de Next, Remix ou
équivalent. Le HTML est servi depuis un bundle local embarqué, pas depuis un serveur.

**2. Runtime long ou serverless (la question clé).**
Processus **Rust natif, long**, avec l'async runtime de Tauri (tokio, via
`tauri::async_runtime`). Aucune limite de durée type 60 ou 300 secondes. Un worker de fond
peut traiter la file `vi_tasks` pendant des heures.
Nuance honnête : ce worker ne tourne **que lorsque l'application est ouverte**. Il n'y a pas
de serveur toujours actif. La reprise se fait par la file persistée en SQLite (une fermeture
ou un plantage ne perd rien), ce qui répond au critère J2, mais un moissonnage nocturne
sans app ouverte demanderait un binaire worker séparé, hors périmètre initial (noté en TODO).

**3. Auth et rôles.**
Aucune. Application **mono utilisateur**, données 100 % locales (fichier SQLite dans le
dossier de support de l'app). Pas de login, pas de rôles. La distribution à l'équipe se fait
par copie du binaire (DMG), chaque poste a sa propre base. Conséquence sur le module :
`vi_runs.created_by uuid` et le registre RGPD « multi utilisateur » n'ont pas de sens ici,
à adapter (voir RGPD en décisions).

---

## 1.2 Données

**4. ORM et base.**
Pas d'ORM. **rusqlite 0.32** (SQLite bundled) + pool **r2d2**. Base **SQLite fichier**, en
local : `~/Library/Application Support/fr.insrt.pierscrm/pierscrm.db`. SQL brut écrit à la
main dans les modules de commandes Rust. Donc ni Drizzle, ni Postgres, ni Neon.

**5. Emplacement des schémas et nommage.**
Schémas dans `src-tauri/src/db.rs` (instructions `CREATE TABLE`). Modèles serde dans
`src-tauri/src/models.rs`. Conventions : **snake_case** pour tables et colonnes, **clé
primaire INTEGER autoincrement** (pas d'uuid), timestamps en **TEXT ISO** (`datetime('now')`),
aucun préfixe de table aujourd'hui. Anglais majoritaire pour les noms techniques.

**6. Migrations.**
Pas de framework de migration, donc **pas de commande generate ni migrate**. `init_pool()`
dans `db.rs` exécute au démarrage un batch `CREATE TABLE IF NOT EXISTS`, puis un bloc
best effort de `ALTER TABLE ... ADD COLUMN` idempotent (les erreurs « colonne déjà là » sont
ignorées). Pour ajouter une table ou une colonne : on édite `db.rs`, l'app migre toute seule
au prochain lancement. Le workflow Drizzle du document ne s'applique pas.

**7. Entités métier existantes.**
Il n'existe **ni companies, ni organizations, ni leads, ni deals, ni pipelines séparés**.
Le pipeline de prospection **est la table `contacts`**. Tables présentes (`db.rs`) :
`artists`, `contacts`, `campaigns`, `emails`, `events`, `budget_items`, `tasks`, `kpis`,
`templates`, `settings`, `visa_countries`, `visa_dossiers`.

Colonnes des entités qui comptent pour l'intégration (module 6) :

- **contacts** : `id`, `artist_id`, `category` (venue, lineup, major, other), `priority`
  (P1/P2/P3 ou A/B/C), `name`, `promoter`, `venue`, `type`, `area`, `scale`, `date`, `time`,
  `format`, `reason`, `contact_channel`, `email`, `email_status`, `status` (pipeline à 9
  valeurs : to_contact, to_evaluate, low_priority, contacted, followed_up, in_discussion,
  confirmed, declined, no_answer), `first_contact`, `follow_up`, `notes`, `website`, `tags`,
  `created_at`, `updated_at`.
  C'est exactement une fiche « lieu à prospecter ». Un venue validé s'y promeut naturellement.
- **artists** : `id`, `name`, `real_name`, `tagline`, `bio`, `genres`, `city`, `country`,
  `avatar`, `email`, `phone`, `booking_email`, `website`, `instagram`, `soundcloud`,
  `spotify`, etc.
- **events** : `id`, `artist_id`, `contact_id`, `title`, `venue`, `city`, `date`,
  `start_time`, `end_time`, `status` (hold, confirmed, cancelled), `fee`, `notes`.
- **settings** : simple clé/valeur. C'est là que vivent déjà la config SMTP (clé `smtp`) et
  l'URL de tracking (`tracking_base_url`). Les clés d'API du module iront là.

---

## 1.3 Interface

**8. UI, composants, tokens.**
React + **TailwindCSS**, design system « Modernist » récent : tokens dans
`tailwind.config.js` + `src/styles.css` (variables CSS, accent **rouge #EC3013**, police
**Archivo**, zéro arrondi, règles 2px). Classes composants réutilisables : `.card`,
`.btn` (`-primary`, `-outline`, `-ghost`, `-danger`), `.input`, `.badge`, `.tbl`,
`.segmented`, `.kicker`, `.panel`. Icônes **lucide-react**. i18n **FR/EN** via
react-i18next. Thème clair/sombre.
**Correction au document** : la convention de style n'est **pas** l'inline. C'est Tailwind
plus ces classes. L'inline (`style={{}}`) ne sert que pour des couleurs dynamiques (barres
de pipeline, couleur par artiste). Je suivrai donc Tailwind et les classes existantes, pas
l'inline. Le repo gagne, comme demandé.

**9. Vues tableau existantes.**
Faites à la main : `<table className="tbl">` alimenté par react-query, filtrage par quelques
selects et une recherche transmise au SQL Rust (`list_contacts`). **Aucune librairie de
table utilisée** : `@tanstack/react-table` est bien dans les dépendances mais n'est importé
nulle part dans `src/`. **Aucune pagination ni virtualisation** aujourd'hui (les listes
actuelles sont petites, moins de 100 lignes).
Conséquence pour Venue Intelligence, qui peut produire des milliers de lieux : il faudra une
pagination côté SQL (LIMIT/OFFSET ou keyset) et sans doute `@tanstack/react-virtual` pour la
file de triage. À trancher en J1/J4 selon le volume attendu.

**10. Raccourcis clavier ou command palette.**
Aucun des deux. Le cockpit de triage introduira ses propres raccourcis (hook clavier maison).
À noter : il existe un **menu natif macOS** (`src-tauri/src/menu.rs`) qui émet des events vers
le frontend, réutilisable pour ajouter des entrées « Venue Intelligence ».

---

## 1.4 Infrastructure

**11. File de jobs ou tâches de fond.**
Aucune côté JS (ni BullMQ, ni Inngest, ni Trigger, ni cron). En revanche le backend Rust
peut lancer des tâches longues (`tauri::async_runtime::spawn`, ou un thread dédié). Le duo
`vi_runs` + `vi_tasks` persistés en SQLite, drainé par un worker Rust, est le bon design et
s'intègre nativement. Les **events Tauri** (`app.emit`) sont déjà utilisés dans le repo
(`menu-action`, `bulk-progress`) pour streamer de la progression vers l'UI, donc les stats
live d'un run passeront par le même canal.

**12. Appel à l'API Anthropic.**
Aucun aujourd'hui, aucune clé stockée, l'app est pensée hors ligne. Pour le module 4 (LLM) il
faudra : une clé API rangée dans `settings` (comme le SMTP), un client HTTP Rust
(**reqwest 0.13 est déjà présent en transitif** via le plugin updater, à passer en dépendance
directe). Réserve sur le modèle : le document cite `claude-sonnet-4-6`, qui a l'air daté. Les
identifiants courants côté Anthropic sont plutôt `claude-sonnet-5`, `claude-opus-4-8`,
`claude-haiku-4-5`. Je rendrai le modèle **configurable**, avec un défaut sur un modèle
courant, et je vérifierai l'id exact via la référence claude-api avant d'implémenter le J5.

**13. Variables d'environnement et fichier d'exemple.**
Aucune, pas de `.env` ni `.env.example`. C'est une app desktop : la configuration vit dans la
table `settings` du SQLite local. Les clés du module (Anthropic, éventuelle API de recherche
web) iront au même endroit. Réserve à signaler : `settings` est stocké en clair dans le
fichier SQLite local, acceptable pour un poste personnel, à garder en tête pour les clés.

---

## Deux points transverses déterminants

**Réseau et CORS (modules 2 et 4).** Le moissonnage RA (headers `User-Agent`, `Referer`,
`Origin` personnalisés) et le crawl des sites de clubs **doivent tourner côté Rust**
(reqwest). Depuis le webview, un `fetch` cross origin avec headers personnalisés est bloqué
par CORS, et `Origin`/`Referer` ne sont pas modifiables. Donc harvest, crawl et appel LLM
seront des **commandes Rust**, pas du code frontend. Cela conforte le choix de tout faire
tourner dans le backend.

**Webview embarquée (module 5, colonne centrale).** Tauri v2 sait afficher un vrai webview
enfant sur une URL (navigation de premier niveau, donc **non soumise à `X-Frame-Options`**
contrairement à un iframe). Mais positionner un webview de niveau OS à l'intérieur d'une
colonne d'un layout React est avancé (Tauri gère plusieurs webviews par fenêtre avec des
bounds à piloter). C'est faisable mais coûteux. Je ne ferai pas semblant qu'un iframe
classique fonctionnerait, il échoue sur la majorité des sites. Reco : commencer simple
(bouton « ouvrir la page », plus rendu du texte et des adresses extraites côté Rust dans la
colonne centrale), et n'ajouter le webview embarqué que si le triage le réclame vraiment.

---

## Décisions d'architecture proposées

**A. Où tourne le moissonnage long.**
Un **worker Rust in process** qui draine `vi_tasks`, démarré via `tauri::async_runtime::spawn`
au lancement d'un run. Persistance SQLite, donc **reprise native** après fermeture ou
plantage (critère J2 satisfait). Progression diffusée par un event Tauri type
`vi:run-progress`. Politesse réseau : pause de 1,5 s configurable, backoff exponentiel sur
429 et 5xx, plafond 3 tentatives puis tâche en `echec` sans bloquer le run. Aucun serveur
externe, aucun serverless. Limite assumée : n'avance que si l'app est ouverte.

**B. Stockage et types.**
rusqlite, tables préfixées `vi_`, **PK INTEGER autoincrement + colonne `slug` unique** (pas
d'uuid, cohérent avec le repo). Correspondance des types du document vers SQLite : uuid vers
INTEGER PK plus slug, enum vers TEXT (validation applicative, CHECK optionnel), jsonb vers
TEXT contenant du JSON, numeric vers REAL, timestamptz vers TEXT ISO, text[] vers TEXT JSON.
Dédoublonnage : `nom_normalise | ville`, avec priorité au `ra_venue_id` quand présent.
Fonction `normalise()` écrite en Rust (minuscules, NFKD, suppression des diacritiques et de
tout non alphanumérique). Fusion manuelle depuis l'UI via une action dédiée qui reporte
preuves et contacts sur le lieu conservé.

**C. Intégration CRM (module 6).**
Un `vi_venues` en statut `valide` se promeut en une ligne **`contacts`** :
`category = 'venue'`, `status = 'to_contact'`, `promoter` = promoteur RA principal, `email` =
email booking retenu, `website`, `area` = ville, `reason` = résumé des preuves récentes,
`tags` = pays, type, segment, saison. Lien bidirectionnel `vi_venues.crm_contact_id` vers
`contacts.id`. Détection de doublon avant création sur nom normalisé plus domaine du site, et
si le lieu existe déjà, proposer l'enrichissement de la fiche avec un diff champ par champ.
Il n'y a pas de company, lead ou deal à créer, ils n'existent pas : la fiche `contacts` **est**
le lead, elle entre dans le pipeline à `to_contact`. Les preuves restent dans `vi_evidence`,
seul un résumé est recopié dans `contacts.reason`/`notes`.

**D. LLM (module 4, étape 3).**
Clé Anthropic dans `settings`, client reqwest en Rust, modèle **configurable** (défaut sur un
Sonnet courant, id à confirmer via la référence claude-api). Sortie JSON stricte, parsée en
try/catch avec repli silencieux sur le regex. Comptage des tokens journalisé dans
`vi_runs.stats`.

**E. Résolution du site officiel (module 4, étape 1).**
Aucune API de recherche dans le repo. Trois options, à trancher avec toi : une API de
recherche (Brave Search, Serper ou Bing) avec clé dans `settings`, la moins fragile ;
un DuckDuckGo HTML léger ; ou un repli 100 % manuel où l'on colle le domaine dans le triage.
Reco : API plus repli manuel, jamais de scraping de Google.

**F. Interface.**
Réutiliser le design Modernist (`.tbl`, cartes, badges, kickers, rouge, Archivo). Nouveaux
écrans : « Venue Intelligence » (liste des runs, lancement, progression live), « Triage »
(cockpit 3 colonnes avec raccourcis), « Artistes de référence » (édition du seed), « Zones
RA ». Entrée ajoutée dans la sidebar (groupe Outreach, ou nouveau groupe « Intelligence »).
Pagination SQL pour les grandes listes, virtualisation pour la file de triage.

**G. Ce qui du document doit être adapté au repo.**
Drizzle et Postgres vers rusqlite et SQLite. Serverless et worker externe vers worker Rust
in process. companies, leads, deals vers la table `contacts`. Vercel Cron, BullMQ, Inngest
vers spawn Rust plus table `vi_tasks`. `.env` vers table `settings`. uuid vers INTEGER plus
slug. Styles inline vers Tailwind et classes existantes. iframe vers webview Tauri ou rendu
texte. `created_by uuid` et registre RGPD multi utilisateur vers un contexte mono
utilisateur local (on garde `source_url` et la date de collecte par contact, plus une liste
d'exclusion, mais sans notion d'utilisateur en base).

---

## Questions ouvertes avant J1

1. **Recherche web (point E)** : on part sur une API (Brave ou Serper) avec clé, ou sur un
   repli 100 % manuel du domaine saisi dans le triage pour commencer ?
2. **Modèle Anthropic** : je pars sur le Sonnet courant, configurable (le `sonnet-4-6` du
   document semble daté), ça te va ?
3. **Moissonnage** : un worker qui n'avance que quand l'app est ouverte te convient pour
   démarrer, ou tu veux dès le départ un binaire worker séparé pour tourner sans l'app ?
4. **Volume cible** : combien de lieux à terme (des centaines, des milliers) ? Cela décide
   pagination simple contre virtualisation lourde.
5. **RGPD mono utilisateur** : le registre des traitements devient un simple export CSV local
   (source_url plus date de collecte plus liste d'exclusion), sans utilisateur. OK ?

---

Stop. J'attends ta validation de ces décisions avant d'attaquer le J1 (schéma, migrations,
seed des artistes de référence et des zones).
