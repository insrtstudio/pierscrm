// PiersCRM, pixel de tracking, version Val Town (https://val.town).
// Cree un "HTTP val", colle ce code, tu obtiens une URL HTTPS publique du type
//   https://TON-USER-main.web.val.run
// Memes chemins que ceux attendus par l'app : /o/<token>.gif et /opens.json
// Persistance via le blob storage integre (aucune config).

import { blob } from "https://esm.town/v/std/blob";

const KEY = "pierscrm_opens";
const PIXEL = Uint8Array.from(
  atob("R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7"),
  (c) => c.charCodeAt(0),
);

export default async function (req: Request): Promise<Response> {
  const url = new URL(req.url);

  // Pixel : /o/<token>.gif
  const m = url.pathname.match(/^\/o\/([A-Za-z0-9_-]+)\.gif$/);
  if (m) {
    const token = m[1];
    let data: Record<string, { opened_at: string; count: number }> = {};
    try {
      data = (await blob.getJSON(KEY)) ?? {};
    } catch (_) {
      data = {};
    }
    data[token] = data[token]
      ? { opened_at: data[token].opened_at, count: (data[token].count || 1) + 1 }
      : { opened_at: new Date().toISOString(), count: 1 };
    await blob.setJSON(KEY, data);
    return new Response(PIXEL, {
      headers: { "content-type": "image/gif", "cache-control": "no-store" },
    });
  }

  // Flux de synchronisation : /opens.json
  if (url.pathname === "/opens.json") {
    let data = {};
    try {
      data = (await blob.getJSON(KEY)) ?? {};
    } catch (_) {
      data = {};
    }
    return new Response(JSON.stringify(data), {
      headers: {
        "content-type": "application/json",
        "access-control-allow-origin": "*",
        "cache-control": "no-store",
      },
    });
  }

  return new Response("PiersCRM tracking", { status: 200 });
}
