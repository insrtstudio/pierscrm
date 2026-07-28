# Tracking d'ouverture, version PHP (hebergement mutualise type cPanel)

Ton hebergement insrt.fr sert du PHP, pas du Node. Le fichier server.js ne s'execute donc
jamais. Utilise plutot ces 3 fichiers PHP. Ils font exactement la meme chose et marchent
avec l'app sans rien changer.

## Ce qu'il faut deposer

Dans le gestionnaire de fichiers, va dans le dossier deja cree :
insrt.fr / public / www / tracking-server

Supprime server.js et README.md (ils ne servent pas ici), puis charge ces 3 fichiers dans
ce meme dossier tracking-server :

1. o.php          le pixel, il enregistre les ouvertures
2. .htaccess      reecrit l'URL du pixel et ajoute l'en-tete CORS
3. opens.json     le fichier ou sont stockees les ouvertures (contient juste {} au depart)

Astuce : le fichier .htaccess commence par un point, il peut etre masque. Coche
"Afficher les fichiers caches" en haut du gestionnaire si tu ne le vois pas apres l'upload.

## Verifier que ca marche (2 tests dans ton navigateur)

1. Ouvre : https://insrt.fr/tracking-server/o/test123.gif
   Tu ne verras rien (c'est un pixel transparent), c'est normal. Aucune erreur = bon signe.

2. Ouvre : https://insrt.fr/tracking-server/opens.json
   Tu dois voir apparaitre quelque chose comme :
   {"test123":{"opened_at":"2026-07-28T20:00:00Z","count":1}}
   Si tu vois cette ligne, le tracking fonctionne.

Si le test 2 affiche toujours {} (vide), le dossier n'est pas accessible en ecriture :
dans le gestionnaire, fais un clic droit sur opens.json, Permissions, et mets 666
(ou sur le dossier tracking-server, mets 755 ou 777 si besoin), puis refais le test 1.

## Brancher l'app

Dans PiersCRM, Reglages, Tracking d'ouverture des emails, colle exactement :

  https://insrt.fr/tracking-server

(sans slash a la fin). Enregistre. C'est tout.

Ensuite, tes prochains emails partent avec le pixel. Dans Emails, Historique, clique sur
"Synchroniser les ouvertures" pour voir qui a ouvert.

## Si l'URL n'est pas la bonne

Selon la config de l'hebergement, la racine du site peut differer. Si le test 1 renvoie une
erreur 404, essaie ces variantes dans le navigateur pour trouver la bonne, puis mets la meme
base (sans /o/...) dans l'app :

  https://insrt.fr/tracking-server/o/test123.gif
  https://www.insrt.fr/tracking-server/o/test123.gif

Envoie-moi le resultat si aucune ne marche, je t'aide a trouver le bon chemin.
