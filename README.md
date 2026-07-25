# PiersCRM

Application desktop native de **booking & management d'artistes** pour Insrt.Studio.
Tout-en-un, **100 % local** (aucune web-app, aucune donnée hébergée) — pensée pour être
distribuée aux employés.

Construite avec **Tauri v2** (Rust) + **React / TypeScript**, base **SQLite locale**.

## Modules

| Module | Rôle |
|--------|------|
| **Tableau de bord** | Vue d'ensemble : pipeline, budget, tâches, jours avant l'événement |
| **Artistes** | Profils complets, avatar, EPK/one-pager exportable en **PDF**, liens, KPIs & bookings par artiste |
| **Contacts** | CRM salles / line-ups / majors avec pipeline de statuts, filtres, recherche, envoi d'email |
| **Import** | Ingestion par lot **.xlsx / .xls / .csv / .tsv** avec mapping de colonnes et aperçu |
| **Emails** | Rédaction, templates à variables, envoi SMTP direct, historique + **tracking d'ouverture** |
| **Budget** | Dépenses / revenus, min-max-réel, net estimé |
| **Planning** | Rétro-planning et tâches par période |
| **KPIs** | Objectifs et résultats |
| **Visas** | Dossiers de demande (checklist de documents, statut) + base de règles de travail par pays |
| **Réglages** | Langue **FR/EN**, thème clair/sombre, SMTP, date cible, URL de tracking |

## Prérequis

- Node 18+ et **pnpm**
- **Rust** (stable) + les prérequis Tauri (Xcode CLT sur macOS)

## Développement

```bash
pnpm install
pnpm tauri dev      # lance l'app en mode dev (hot-reload frontend)
```

## Build de production

```bash
pnpm tauri build    # produit un .dmg / .app (macOS) dans src-tauri/target/release/bundle
```

## Données

La base SQLite vit dans le dossier de données de l'app :
`~/Library/Application Support/fr.insrt.pierscrm/pierscrm.db` (macOS).
Au premier lancement elle est créée, migrée et pré-remplie (templates d'emails, pays visa).
Les données ADE 2026 (salles, line-ups, budget, planning, KPIs) y ont été chargées depuis
le classeur Excel d'origine.

## Envoi d'emails

Renseignez le SMTP de votre boîte de domaine dans **Réglages → Compte email**.
Les emails partent directement depuis l'app (aucun intermédiaire). SMTP transite par
internet — c'est normal et inévitable ; seule l'app et ses données restent en local.

## Tracking d'ouverture des emails

Détecter l'ouverture d'un mail nécessite un **pixel invisible** hébergé sur une URL
publique (le client mail du destinataire doit pouvoir la charger). C'est la **seule**
brique qui doit vivre hors de la machine locale.

1. Déployez le mini-serveur fourni dans [`tracking-server/`](./tracking-server/) sur votre
   domaine (ex. `https://track.insrt.fr`).
2. Collez cette URL dans **Réglages → Tracking d'ouverture**.
3. Les nouveaux emails sont alors envoyés en HTML avec le pixel ; **Emails → Historique →
   Synchroniser les ouvertures** met à jour les statuts *Ouvert / Non ouvert*.

> Best-effort : certains clients bloquent les images distantes — « non ouvert » peut aussi
> signifier « images bloquées ». C'est un indicateur, pas une preuve.

## Visas — avertissement

Le module Visas fournit une **base d'orientation éditable**, pas un conseil juridique.
Chaque fiche pays doit être vérifiée auprès des sources officielles avant tout déplacement ;
les règles dépendent de la nationalité de l'artiste et du motif exact du voyage.

## Architecture

```
src/                 Frontend React/TS
  pages/             Une page par module
  components/        Layout, UI, modales, compose email
  lib/               api.ts (bindings Tauri), types, constantes
  i18n/              Traductions FR/EN
src-tauri/           Backend Rust
  src/db.rs          Schéma SQLite, migrations, seeds
  src/models.rs      Modèles (serde)
  src/commands/      Commandes Tauri (contacts, artists, import, email, visa, data)
tracking-server/     Serveur de pixel d'ouverture (Node, à déployer)
```
