/**
 * PiersCRM — minimal email open-tracking server.
 *
 * Two endpoints:
 *   GET /o/:token.gif   → logs an open for :token, returns a 1x1 transparent GIF
 *   GET /opens.json     → returns { token: { opened_at, count } } for the app to sync
 *
 * Zero dependencies (pure Node). Data is persisted to opens.json on disk.
 * Deploy on any small box on your own domain (e.g. https://track.insrt.fr),
 * then paste that URL in PiersCRM → Settings → Email open tracking.
 *
 *   PORT=8080 node server.js
 *
 * Put it behind HTTPS (Caddy / nginx / Cloudflare) — email clients require https
 * for remote images to load reliably.
 */
const http = require("http");
const fs = require("fs");
const path = require("path");

const PORT = process.env.PORT || 8080;
const DATA_FILE = path.join(__dirname, "opens.json");

// 1x1 transparent GIF
const PIXEL = Buffer.from(
  "R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7",
  "base64"
);

let opens = {};
try {
  opens = JSON.parse(fs.readFileSync(DATA_FILE, "utf8"));
} catch {
  opens = {};
}

let saveTimer = null;
function persist() {
  clearTimeout(saveTimer);
  saveTimer = setTimeout(() => {
    fs.writeFile(DATA_FILE, JSON.stringify(opens), () => {});
  }, 500);
}

const server = http.createServer((req, res) => {
  const url = new URL(req.url, `http://${req.headers.host}`);
  const pathname = url.pathname;

  // Pixel: /o/<token>.gif
  const m = pathname.match(/^\/o\/([A-Za-z0-9_-]+)\.gif$/);
  if (m) {
    const token = m[1];
    const now = new Date().toISOString();
    if (!opens[token]) opens[token] = { opened_at: now, count: 1 };
    else opens[token].count += 1;
    persist();
    res.writeHead(200, {
      "Content-Type": "image/gif",
      "Cache-Control": "no-store, no-cache, must-revalidate, private",
      Pragma: "no-cache",
      Expires: "0",
    });
    res.end(PIXEL);
    return;
  }

  // Sync feed: /opens.json  (CORS-open so the desktop app can fetch it)
  if (pathname === "/opens.json") {
    res.writeHead(200, {
      "Content-Type": "application/json",
      "Access-Control-Allow-Origin": "*",
      "Cache-Control": "no-store",
    });
    res.end(JSON.stringify(opens));
    return;
  }

  res.writeHead(404, { "Content-Type": "text/plain" });
  res.end("Not found");
});

server.listen(PORT, () => {
  console.log(`PiersCRM tracking server listening on :${PORT}`);
});
