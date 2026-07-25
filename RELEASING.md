# Distribution & mises à jour (push)

PiersCRM se distribue en **DMG** et se met à jour **tout seul** (plugin updater de Tauri).
Tu ne réinstalles jamais : tu publies une version, chaque app installée la propose au
démarrage + dans Réglages → Mises à jour.

L'app est configurée pour lire son manifeste sur **GitHub Releases** :
`src-tauri/tauri.conf.json → plugins.updater.endpoints`.

---

## Étape 0 — Créer le dépôt (une seule fois)

```bash
cd /Users/thibaultpierens/Projets/pierscrm
git init && git add . && git commit -m "PiersCRM initial"

# crée le repo PUBLIC (requis : voir note plus bas) et pousse
gh repo create insrt-studio/pierscrm --public --source=. --push
```

Puis **remplace `OWNER/REPO`** dans `src-tauri/tauri.conf.json` par ton slug réel
(ex. `insrt-studio/pierscrm`) :

```json
"endpoints": ["https://github.com/insrt-studio/pierscrm/releases/latest/download/latest.json"]
```

Commit ce changement.

> ⚠️ **Le dépôt (ou au moins les releases) doit être public.** Les assets d'un repo privé
> exigent une authentification que l'updater n'envoie pas. Pour rester privé il faut un
> proxy avec token — demande-le-moi si besoin.

### Secrets de signature (une seule fois)

Le workflow signe les mises à jour avec ta clé privée. Ajoute 2 secrets au repo :

```bash
gh secret set TAURI_SIGNING_PRIVATE_KEY < src-tauri/pierscrm-updater.key
gh secret set TAURI_SIGNING_PRIVATE_KEY_PASSWORD --body ""
```

> La clé privée `src-tauri/pierscrm-updater.key` **n'est pas commitée** (`.gitignore`).
> Sauvegarde-la ailleurs : sans elle, plus aucune mise à jour signable.

---

## Publier une version — automatique 🚀

Le workflow `.github/workflows/release.yml` build (macOS **universel** Intel + Apple
Silicon), signe, crée la release et attache le DMG + `latest.json`.

```bash
# 1) bump la version
#    src-tauri/tauri.conf.json  → "version": "0.2.0"
#    package.json               → "version": "0.2.0"

git commit -am "v0.2.0"
git tag v0.2.0
git push origin main --tags
```

GitHub s'occupe du reste. Les apps installées détectent la release, téléchargent,
vérifient la signature et s'installent au clic. **Aucun build local.**

Suis l'avancement dans l'onglet **Actions** du repo.

---

## Publier à la main (secours, sans Actions)

```bash
export TAURI_SIGNING_PRIVATE_KEY="$(cat src-tauri/pierscrm-updater.key)"
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD=""
pnpm tauri build

UPDATE_BASE_URL="https://github.com/OWNER/REPO/releases/latest/download" \
  pnpm release:manifest --notes "Ce qui change"

gh release create v0.2.0 \
  src-tauri/target/release/bundle/dmg/PiersCRM_0.2.0_x64.dmg \
  src-tauri/target/release/bundle/macos/PiersCRM.app.tar.gz \
  src-tauri/target/release/bundle/latest.json \
  --title "PiersCRM 0.2.0" --notes "Ce qui change"
```

---

## Héberger sur ton propre domaine plutôt que GitHub

Mets l'endpoint sur `https://updates.insrt.fr/pierscrm/latest.json` dans la config, puis
uploade `latest.json` + `PiersCRM.app.tar.gz` sur ce chemin (nginx/Caddy/S3). Le script
`pnpm release:manifest` (défaut `UPDATE_BASE_URL=https://updates.insrt.fr/pierscrm`) génère
le manifeste correspondant.

---

## Gatekeeper (app non notarisée Apple)

Le DMG est signé **ad-hoc** mais pas notarisé (pas de compte Apple Developer). Au 1er
lancement : **clic droit sur l'app → Ouvrir** (une fois par machine). Pour supprimer
l'avertissement (distribution large), il faut un **Apple Developer ID** (99 $/an) —
renseigne `bundle.macOS.signingIdentity` + les secrets `APPLE_*` dans le workflow pour
la notarisation. La chaîne updater ne change pas.
