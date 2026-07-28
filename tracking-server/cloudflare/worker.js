// PiersCRM, pixel de tracking d'ouverture, version Cloudflare Worker (gratuit).
// Endpoints (memes URLs que celles attendues par l'app) :
//   GET /o/<token>.gif  -> enregistre l'ouverture, renvoie un GIF 1x1 transparent
//   GET /opens.json     -> renvoie { token: { opened_at, count } }, avec CORS
//
// Necessite un namespace KV lie au Worker sous le nom OPENS (voir le guide).

const PIXEL = Uint8Array.from(
  atob("R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7"),
  (c) => c.charCodeAt(0)
);

export default {
  async fetch(request, env) {
    const url = new URL(request.url);

    // Pixel : /o/<token>.gif
    const m = url.pathname.match(/^\/o\/([A-Za-z0-9_-]+)\.gif$/);
    if (m) {
      const token = m[1];
      try {
        const prev = await env.OPENS.get(token, { type: "json" });
        const rec = prev
          ? { opened_at: prev.opened_at, count: (prev.count || 1) + 1 }
          : { opened_at: new Date().toISOString(), count: 1 };
        await env.OPENS.put(token, JSON.stringify(rec));
      } catch (e) {
        // on renvoie le pixel quoi qu'il arrive
      }
      return new Response(PIXEL, {
        headers: {
          "content-type": "image/gif",
          "cache-control": "no-store, no-cache, must-revalidate, private",
        },
      });
    }

    // Flux de synchronisation : /opens.json
    if (url.pathname === "/opens.json") {
      const out = {};
      let cursor;
      do {
        const list = await env.OPENS.list({ cursor });
        for (const k of list.keys) {
          out[k.name] = await env.OPENS.get(k.name, { type: "json" });
        }
        cursor = list.list_complete ? undefined : list.cursor;
      } while (cursor);
      return new Response(JSON.stringify(out), {
        headers: {
          "content-type": "application/json",
          "access-control-allow-origin": "*",
          "cache-control": "no-store",
        },
      });
    }

    return new Response("PiersCRM tracking", { status: 200 });
  },
};
