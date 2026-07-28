<?php
// PiersCRM, pixel de tracking d'ouverture, version PHP (hebergement mutualise).
// Enregistre une ouverture pour le token recu, puis renvoie un GIF 1x1 transparent.
// Le .htaccess reecrit /o/<token>.gif vers o.php?t=<token>, donc l'URL vue par
// l'app reste https://votre-domaine/tracking-server/o/<token>.gif

$token = isset($_GET['t']) ? preg_replace('/[^A-Za-z0-9_-]/', '', $_GET['t']) : '';
$file = __DIR__ . '/opens.json';

if ($token !== '') {
    $data = array();
    if (is_file($file)) {
        $raw = file_get_contents($file);
        $decoded = json_decode($raw, true);
        if (is_array($decoded)) { $data = $decoded; }
    }
    if (!isset($data[$token])) {
        $data[$token] = array('opened_at' => gmdate('Y-m-d\TH:i:s\Z'), 'count' => 1);
    } else {
        $data[$token]['count'] = (isset($data[$token]['count']) ? $data[$token]['count'] : 1) + 1;
    }
    file_put_contents($file, json_encode($data), LOCK_EX);
}

header('Content-Type: image/gif');
header('Cache-Control: no-store, no-cache, must-revalidate, private');
header('Pragma: no-cache');
header('Expires: 0');
echo base64_decode('R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7');
