# Deploying alwywatch-server

`alwywatch-server` is the optional backend for the "keep" feature: it lists what Maintainerr will
delete, stores per-user keep requests and applies them to Maintainerr. It also bridges Seerr,
hosts watch together rooms and device transfers, and serves the [web app](web.md) at `/`. It is
one static binary in a `scratch` image with a SQLite database in `/data` and the web app in
`/web` (`ALWYWATCH_WEB`).

Nothing here is applied automatically. These are the steps for the owner of the server.

## 1. What it needs

| Need | Why | How |
|---|---|---|
| Network access to Maintainerr | read collections, write exclusions/postpones | join `vod_default` |
| Network access to Jellyfin | validate Jellyfin users' tokens, map items, "watched" checks | join `jellyfin_default` |
| Network access to Tautulli | "watched" checks for Plex users (no Plex Pass needed) | join `tautulli_default` |
| Outbound HTTPS to plex.tv | validate Plex users' tokens | default |
| `JELLYFIN_API_KEY` | read any user's played state + provider ids (users' tokens are never stored) | Jellyfin → Dashboard → API Keys |
| `TAUTULLI_API_KEY` | read Plex users' watch history | Tautulli → Settings → Web Interface → API |
| `WEBHOOK_SECRET` | optional: lets Tautulli/Jellyfin wake the "watched" check immediately | any long random string |
| `MAINTAINERR_API_KEY` | only if Maintainerr's API key is turned on | Maintainerr → Settings |
| `SEERR_API_KEY` | optional Seerr bridge: Seerr for users of the other media server type | Seerr → Settings → General → API Key |

Other settings: `ALWYWATCH_CONFIG` (default `/data/config.toml`), `ALWYWATCH_DATA` (`/data`),
`ALWYWATCH_LISTEN` (`0.0.0.0:8080`), `ALWYWATCH_WEB` (`/web`), `JELLYFIN_API_KEY_<server id>` for
one key per Jellyfin server, and `ALWYWATCH_DRY_RUN=1` (or `dry_run = true`): reads everything,
writes nothing to Maintainerr or Seerr, votes go to `keep-dryrun.sqlite`.

Without the Jellyfin/Tautulli keys everything works except automatic release of "keep until I
watched it" (those then behave like "keep forever" until the user removes them). Without
`SEERR_API_KEY` (or `[seerr]` in config.toml) the Seerr bridge is off and users connect to Seerr
directly, which only works for the media server type Seerr runs on.

### Seerr bridge

Seerr serves one media server type at a time. With `[seerr]` and `SEERR_API_KEY` set, users of the
other type (today: Jellyfin users, since Seerr is in Plex mode) use Seerr through alwywatch-server:
* a user whose Seerr account already exists (same Plex id / Jellyfin user id) is linked to it;
* otherwise a **local Seerr user** is created once (email `jellyfin-<id>@alwywatch.local`, random
  password nobody knows, Seerr's *default permissions*). It shows up in Seerr → Users like anyone
  else: set quotas, auto-approve or remove it there. `create_users = false` disables creation;
  such users then get "no Seerr account, ask the administrator";
* calls are made with the API key acting as that user (`X-API-User`), so Seerr enforces their
  permissions. Only search, details, requesting, listing and cancelling requests are forwarded;
  settings, users and approval endpoints are refused by the bridge.

## 2. Files

```
~/docker/alwywatch/
├── docker-compose.yml      # server/deploy/docker-compose.yml
├── .env                    # TAUTULLI_API_KEY=… JELLYFIN_API_KEY=… WEBHOOK_SECRET=…   (chmod 600)
└── data/
    └── config.toml         # server/deploy/config.example.toml, filled in
```

Fill `config.toml` with the real ids:
* Plex machine identifier: `curl -sk https://plex:32400/identity` inside the Docker network.
* Jellyfin server id: `curl -s http://jellyfin:8096/System/Info/Public` (`Id`).
* `[maintainerr] server` = the Plex id today (Maintainerr runs in Plex mode). When Maintainerr is
  switched to Jellyfin, change it to the Jellyfin id; nothing else changes.

`data/` must be writable by uid 65534 (`sudo chown -R 65534:65534 data`).

## 3. Build and start

```sh
# on the build machine (repository root)
docker build -f server/Dockerfile -t alwywatch-server .
docker save alwywatch-server | ssh <server> docker load
# on the server
cd ~/docker/alwywatch && docker compose up -d && docker logs -f alwywatch-server
```
The log shows `Maintainerr 3.32.0` and `listening on 0.0.0.0:8080`. The first sync reads every
collection item and its title (about one minute for ~1000 items). The server refuses to write
to a Maintainerr major version it was not built for.

## 4. Caddy

Public (remote users and the web app must reach it), no `localonly`. Long polls (watch together)
need ~40 s requests; Caddy's defaults allow that.

```caddy
# alwywatch keep service
watch.example.org {
	import common
	import block
	reverse_proxy alwywatch-server:8080
}
```
Caddy must share a network with the container: add `alwywatch_default` to Caddy's networks (or
add `caddy`'s network to the compose file). Then `docker exec caddy caddy reload --config /etc/caddy/Caddyfile`.

## 5. Optional webhooks (instant "watched" release)

* **Tautulli** → Settings → Notification Agents → Webhook:
  URL `http://alwywatch-server:8080/v1/hooks/tautulli/<WEBHOOK_SECRET>`, method POST, trigger
  "Watched". The body does not matter: the hook wakes the check, which asks Tautulli itself.
* **Jellyfin** (if the Webhook plugin is installed later): Generic destination to
  `http://alwywatch-server:8080/v1/hooks/jellyfin/<WEBHOOK_SECRET>`, notification type
  "Playback Stop".

Without webhooks the check runs every 15 minutes.

## 6. Before activating a Maintainerr collection

Every item in "Deletion" and "Series out" is already past its nominal deletion date. Activating
a collection as-is would delete everything in it at the next rule run. Use the admin section in
the app → "Start countdown" (optionally spread over N days) first: it resets each item's
countdown so people get the full 30 days to vote. alwywatch-server never activates collections.

## 7. Backup and removal

Back up `data/keep.sqlite` (the votes). Removing the service: `docker compose down`; then, in
Maintainerr → Rules → Exclusions, remove the global exclusions it added (they are listed in the
admin log), or keep them.
