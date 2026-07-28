# Tracking d'ouverture, version Cloudflare Worker (gratuit, sans DNS)

Pourquoi cette option : ton domaine insrt.fr redirige vers Bandcamp et ne sert pas tes
fichiers. Cloudflare te donne une URL publique en HTTPS en quelques minutes, sans toucher a
ton domaine. C'est gratuit et suffisant pour ce besoin.

## Etapes (environ 10 minutes)

1. Cree un compte gratuit sur https://dash.cloudflare.com (ou connecte-toi).

2. Cree le stockage des ouvertures (KV) :
   - Menu de gauche : Storage & Databases, puis KV (ou Workers & Pages, KV).
   - Create a namespace, nomme-le par exemple OPENS, valide.

3. Cree le Worker :
   - Menu de gauche : Workers & Pages, Create, Create Worker.
   - Donne-lui un nom, par exemple pierscrm-track. Deploy (le code par defaut, on le
     remplacera juste apres).

4. Colle le code :
   - Ouvre le Worker, bouton Edit code.
   - Efface tout, colle le contenu du fichier worker.js fourni. Save and deploy.

5. Lie le stockage au Worker :
   - Dans le Worker, onglet Settings, section Bindings (ou Variables and Bindings).
   - Add binding, type KV namespace.
   - Variable name : OPENS (exactement ce mot).
   - KV namespace : choisis OPENS cree a l'etape 2. Save/Deploy.

6. Recupere l'URL du Worker :
   - En haut du Worker, tu as une URL du type
     https://pierscrm-track.TON-SOUS-DOMAINE.workers.dev

## Verifier (2 tests dans le navigateur)

1. Ouvre https://pierscrm-track.TON-SOUS-DOMAINE.workers.dev/o/test123.gif
   Rien ne s'affiche (pixel transparent), c'est normal.

2. Ouvre https://pierscrm-track.TON-SOUS-DOMAINE.workers.dev/opens.json
   Tu dois voir : {"test123":{"opened_at":"...","count":1}}

## Brancher l'app

Dans PiersCRM, Reglages, Tracking d'ouverture, colle l'URL du Worker sans slash final :

  https://pierscrm-track.TON-SOUS-DOMAINE.workers.dev

Enregistre. Ensuite, Emails, Historique, Synchroniser les ouvertures.

## Si tu preferes utiliser ton domaine insrt.fr

Il faudrait, cote hebergeur ou registrar :
- soit supprimer la redirection de insrt.fr vers Bandcamp et faire pointer le domaine vers
  l'espace cPanel ou tu as mis les fichiers PHP,
- soit creer un sous-domaine, par exemple track.insrt.fr, pointant vers cet espace cPanel.
Ensuite la version PHP (o.php, .htaccess, opens.json) marchera a l'adresse
https://track.insrt.fr. Dis-moi si tu veux qu'on parte sur cette voie, il me faut savoir
chez qui est gere le domaine et l'hebergement cPanel.
