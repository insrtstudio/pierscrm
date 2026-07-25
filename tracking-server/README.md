# PiersCRM — Email open-tracking server

A tiny, zero-dependency service that lets PiersCRM show whether a prospect **opened**
an email. It hosts an invisible tracking pixel and a sync feed the app reads.

## Why a separate server?

Open tracking works by embedding a 1×1 invisible image in the email. When the
recipient opens the message, their mail client loads that image from a URL — and
that request is the "open" signal. That URL **must be reachable from the public
internet** (the recipient is not on your machine), so this one small piece has to
live on a server on your domain. Everything else in PiersCRM stays local.

> Note: open tracking is best-effort. Some clients (or users) block remote images,
> so "not opened" can also mean "images were blocked". It's an indicator, not proof.

## Endpoints

| Route | Purpose |
|-------|---------|
| `GET /o/<token>.gif` | Records an open for `<token>`, returns a transparent pixel |
| `GET /opens.json` | Returns `{ token: { opened_at, count } }` — the app syncs from this |

## Run it

```bash
cd tracking-server
PORT=8080 node server.js
```

Opens are persisted to `opens.json` next to the script.

## Put it on your domain (HTTPS required)

Email clients only load remote images reliably over HTTPS. Two easy options:

**Caddy** (automatic HTTPS):
```
track.insrt.fr {
    reverse_proxy localhost:8080
}
```

**Cloudflare Tunnel / nginx + certbot** also work — anything that terminates TLS
and proxies to `localhost:8080`.

## Wire it into the app

1. Deploy this server, e.g. `https://track.insrt.fr`.
2. In PiersCRM → **Settings → Email open tracking**, paste that URL.
3. New emails are then sent as HTML with a pixel at
   `https://track.insrt.fr/o/<token>.gif`.
4. In **Emails → History**, click **Sync opens** to pull the latest opens and see
   the ✅ *Opened* / ⊘ *Not opened* status per email.

## Keep it running

Use a process manager so it survives reboots:
```bash
# systemd example — /etc/systemd/system/pierscrm-track.service
[Service]
ExecStart=/usr/bin/node /opt/pierscrm/tracking-server/server.js
Environment=PORT=8080
Restart=always
User=www-data
```
