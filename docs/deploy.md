# Setting up alwywatch-server

`alwywatch-server` is the optional server side of AlwyWatch: one small Docker container. It
can run anywhere that can reach your services over the network: on the same machine as
Jellyfin/Plex and Maintainerr, or on another one (a VPS, for instance), with their addresses
in its config. Nothing else has to be installed next to it.

It also works on its own for the web app, accounts, watch together and device sign-in: the
only thing it always needs is at least one Jellyfin or Plex server for people to sign in to.
Maintainerr (Leaving soon), Tautulli and Seerr are optional additions.

**Do you need it?** Without it, the apps are plain Jellyfin/Plex clients with downloads and
(directly) Seerr requests. With it you also get:

* **Leaving soon**: everyone sees what Maintainerr is about to delete and can ask to keep it
  (N days, until they have watched it, or forever);
* **Watch together** rooms;
* the **web app**, served at the site's address;
* settings that follow a person across devices, and signing a new device in with a code or QR;
* optional **AlwyWatch accounts** (password, passkeys, Pocket ID);
* Seerr requests for users of both Jellyfin and Plex, whatever your Seerr is set up for.

## What it changes on your server

* **Maintainerr**: it adds *global exclusions* for titles people keep, removes them when the
  keep ends, and can restart deletion countdowns when an admin asks. It never activates or
  deletes a collection, and never deletes media.
* **Seerr** (only with the Seerr bridge): it creates requests on behalf of users, and may
  create local Seerr users (see `[seerr]` in the config example).
* **Jellyfin, Plex, Tautulli**: read only.

It never stores Jellyfin/Plex passwords or tokens. AlwyWatch account passwords are stored as
Argon2id hashes.

## Quick start

### Before you start

* Docker with Compose, on any machine that can reach the services below.
* For Leaving soon: **Maintainerr 3.x**, already set up with at least one collection. The
  server refuses to start against another major version (it will not write to an API it
  wasn't built for). Without Maintainerr, delete the `[maintainerr]` block from the config.
* Jellyfin and/or Plex, reachable by your users over https.
* A hostname with TLS on your reverse proxy (for example `watch.example.org`). The apps and the
  web app talk to it from outside; passkeys need https. Maintainerr, Tautulli and Seerr can
  stay LAN-only.

### 1. Build the image

There is no prebuilt image yet. On the server (needs only Docker; the first build compiles
everything and takes several minutes):

```sh
git clone https://github.com/arlexTech/alwywatch && cd alwywatch
docker build -f server/Dockerfile -t alwywatch-server .
```

You can also build on another machine and send it over:
`docker save alwywatch-server | ssh <server> docker load`.

### 2. Create its folder

```sh
mkdir -p ~/alwywatch/data && cd ~/alwywatch          # any directory
cp <repo>/server/deploy/docker-compose.yml .
cp <repo>/server/deploy/.env.example .env && chmod 600 .env
cp <repo>/server/deploy/config.example.toml data/config.toml
sudo chown -R 65534:65534 data                       # the container runs as this user
```

### 3. Fill in `data/config.toml`

The minimum is one `[[servers]]` entry; add `[maintainerr]` for Leaving soon:

* **Server id**: open `https://<your jellyfin>/System/Info/Public` in a browser and copy `Id`.
  For Plex, open `https://<your plex>/identity` and copy `machineIdentifier`. A wrong id shows
  up later as "this server is not served here" when signing in.
* **`public_url`**: the address your users' devices use for that media server.
* **`internal_url`** (Jellyfin only, optional): how the container reaches Jellyfin. Plex users
  are checked at plex.tv, so Plex needs nothing here.
* **`[maintainerr] url`**: how the container reaches Maintainerr; **`server`**: the id of the
  media server Maintainerr is set up for.

Leave `.env` as it is for now: every line in it is optional.

### 4. Put it on the right Docker network

In `docker-compose.yml`, replace `CHANGE_ME_default` with the network your Maintainerr and
Jellyfin containers are on (`docker network ls`; usually `<folder>_default`). Your reverse
proxy must be on that network too, or add its network the same way.

On a machine of its own, there is no network to share: delete the `networks` parts of the
compose file and use addresses reachable from there in `config.toml` (public https addresses,
or a VPN). Maintainerr is made for a private network: reach it over a VPN rather than
exposing it.

### 5. First start, changing nothing

```sh
echo ALWYWATCH_DRY_RUN=1 >> .env
docker compose up -d && docker logs -f alwywatch-server
```

You should see a line listing what is on and off, then `Maintainerr 3.x.y` and
`listening on 0.0.0.0:8080`. In dry run the server reads everything and writes nothing; the
log says `[dry-run] would …` for each change it would have made. The first reading of
Maintainerr takes about a minute per thousand items.

If it stops instead, the last log line says why and what to change. The common ones:

| Log | Cause |
|---|---|
| `/data/config.toml: No such file…` | step 2 or 3 was skipped, or `data/` isn't mounted |
| `cannot open /data/keep.sqlite…` | `data/` isn't owned by uid 65534 |
| `[maintainerr] server = … must be the id of one [[servers]] entry` | the two ids differ |
| `Maintainerr not reachable yet` (keeps running) | wrong `[maintainerr] url`, or not on the same network |
| `Maintainerr … is not supported` | Maintainerr isn't 3.x |

### 6. Reverse proxy

The proxy must pass `X-Forwarded-For` (Caddy, Traefik and the nginx line below do) and be the
only way to reach port 8080: the sign-in rate limit counts per client address. Requests can
last 40 seconds (watch together), which default timeouts allow.

```caddy
watch.example.org {
	reverse_proxy alwywatch-server:8080
}
```

```nginx
location / {
    proxy_pass http://alwywatch-server:8080;
    proxy_set_header Host $host;
    proxy_set_header X-Forwarded-For $remote_addr;
    proxy_read_timeout 60s;
}
```

Behind a second proxy or CDN (Cloudflare…), every visitor shares one rate limit, because the
server only sees the CDN's address.

Check: `curl https://watch.example.org/healthz` answers `ok`, and
`https://watch.example.org/v1/servers` lists your media servers.

### 7. Sign in: you are the admin

Open `https://watch.example.org` and sign in with your **Jellyfin administrator** account (or
the **Plex account that owns the server**). That is what makes you an admin; there is no
separate setting. "Leaving soon" and "Admin" appear in the menu.

### 8. Go live

Remove the `ALWYWATCH_DRY_RUN` line from `.env`, then `docker compose up -d`. Choices made
during the dry run were kept in a separate database and are not carried over. Read
[Before activating a Maintainerr collection](#before-activating-a-maintainerr-collection)
before you let Maintainerr delete anything.

### 9. Give it to your people

* **Web**: `https://watch.example.org`, signing in with their Jellyfin or Plex login.
* **Apps**: at first launch, "I was given an AlwyWatch address" → `watch.example.org` → their
  server → their Jellyfin or Plex login.
* **A TV or a second device**: "Sign in with a code" on the new device, then scan its QR code
  with a phone, or type the code in Settings → AlwyWatch & requests → Add a device on a device
  that is already signed in.

## Optional extras

Each one is a line in `.env` (and sometimes a block in `config.toml`), then
`docker compose up -d`. The startup log line shows which are on.

| Setting | What it adds |
|---|---|
| `ALWYWATCH_PUBLIC_URL=https://watch.example.org` | Passkeys, invitation links instead of bare codes; required for Pocket ID. Scheme and host only, no path |
| `JELLYFIN_API_KEY` | "Keep until I have watched it" ends by itself for Jellyfin users. Without it, those keeps stay until the person removes them. `JELLYFIN_API_KEY_<server id>` gives one key per Jellyfin server (the id as in `config.toml`) |
| `[tautulli]` + `TAUTULLI_API_KEY` | The same for Plex users (no Plex Pass needed) |
| `WEBHOOK_SECRET` + [webhooks](#webhooks) | That release happens at once instead of within 15 minutes |
| `[seerr]` + `SEERR_API_KEY` | Requests in the web app, and for users of the media server type your Seerr isn't set up for. Both are needed; with only one the bridge is off |
| `MAINTAINERR_API_KEY` | Only if you turned on an API key in Maintainerr's settings |
| `ALWYWATCH_REGISTRATION` | Who may create an AlwyWatch account without a Jellyfin/Plex login: `invite` (default), `open`, `closed` |
| `ALWYWATCH_POCKETID_*` | Single sign-on, see below |
| `sync_minutes` in `config.toml` | How often Maintainerr is read again (default 10) |

Other environment variables: `ALWYWATCH_CONFIG` (default `/data/config.toml`),
`ALWYWATCH_DATA` (`/data`), `ALWYWATCH_LISTEN` (`0.0.0.0:8080`), `ALWYWATCH_WEB` (`/web`, where
the image keeps the web app). The environment wins over `config.toml`; the image sets the
listen address and data directory, so change those through the environment.

### AlwyWatch accounts

Accounts are optional: a Jellyfin or Plex login is enough for everything. An account adds a
way to sign in that doesn't depend on the media server (password, passkey, Pocket ID), and lets
someone with no Jellyfin/Plex login have a profile. An account alone can't vote on Leaving soon
or request; that needs a media-server login linked to it.

* **A user with a Jellyfin/Plex login** gives themselves a username and password on the web
  app's Account page.
* **Someone without one** needs an invitation (unless registration is `open`): as an admin,
  signed in with your Jellyfin/Plex admin login, open Account → Invitations → New invitation.
  It works once, for 7 days.
* **Deleting an account**: the person does it on the Account page ("Delete account").
* **Forgotten password**: there is no reset by email. The person signs in with their linked
  Jellyfin/Plex login or a passkey and sets a new password on the Account page.
* There is no list of accounts or invitations in the admin pages yet.

**Pocket ID**: in Pocket ID, create an OIDC client whose callback is
`https://watch.example.org/v1/account/oidc/callback`, then set `ALWYWATCH_POCKETID_ISSUER`
(Pocket ID's address, e.g. `https://id.example.org`), `ALWYWATCH_POCKETID_CLIENT_ID` and
`ALWYWATCH_POCKETID_CLIENT_SECRET`. Anyone who can sign in to your Pocket ID gets an account on
first sign-in, whatever `ALWYWATCH_REGISTRATION` says.

### Webhooks

Without them, "watched" is checked every 15 minutes. With `WEBHOOK_SECRET` set (generate one
with `openssl rand -hex 32`):

* **Tautulli** → Settings → Notification Agents → Webhook: URL
  `http://alwywatch-server:8080/v1/hooks/tautulli/<WEBHOOK_SECRET>`, method POST, trigger
  "Watched". The body doesn't matter. Tautulli must be on the same Docker network (or use the
  public address).
* **Jellyfin** with the Webhook plugin: a Generic destination to
  `http://alwywatch-server:8080/v1/hooks/jellyfin/<WEBHOOK_SECRET>`, notification type
  "Playback Stop".

A 404 answer means the secret is unset or wrong.

## Administration

An admin is a **Jellyfin administrator** or the **owner of the Plex server**, signed in with
that login. The admin pages are in the web app's menu ("Admin", "Kept forever") and in the
apps under Leaving soon → Admin.

* **Rights per person**: whether they may keep forever, how many titles forever, and the
  longest "keep N days". By default everyone may keep forever, without limits.
* **Requests as**: which Seerr user a person's requests are made as (Seerr bridge only).
* **Start countdown**: see below.
* **Kept forever**: who keeps what forever.

What the server wrote to Maintainerr is recorded in the database:
`sqlite3 data/keep.sqlite 'select * from log'`.

### Before activating a Maintainerr collection

If a Maintainerr collection is inactive and its items are already past their delete date,
activating it deletes them at the next rule run, before anyone could ask to keep them. First
use Admin → **Start countdown** for that collection: it restarts each item's delay (the
collection's "delete after days"), all at once or spread over 14 or 30 days. Then activate the
collection in Maintainerr.

## Backup, upgrade, removal

* **Backup**: the database holds the keep choices, accounts (password hashes, passkeys),
  rights and synced settings. Stop the container, copy the whole `data/` folder and `.env`,
  start it again. Without stopping: `sqlite3 data/keep.sqlite ".backup backup.sqlite"` (a plain
  file copy of a running database can miss recent changes).
* **Restore**: put the files back, `chown -R 65534:65534 data`, start.
* **Upgrade**: back up, `git pull`, rebuild the image (step 1), `docker compose up -d`. The
  database is updated at start. Apps and server can be updated in any order.
* **Removal**: `docker compose down`. The global exclusions it added stay in Maintainerr
  (Rules → Exclusions); remove them there if you want those titles deleted after all.

## Troubleshooting

| Symptom | Look at |
|---|---|
| Apps say "this server is not served here" | the `id` in `[[servers]]` doesn't match the media server |
| Jellyfin sign-in fails with "media server unreachable" | `internal_url` (or `public_url`) isn't reachable from the container |
| No "Leaving soon" for someone | they aren't signed in with a login on one of the `[[servers]]` |
| "Leaving soon" is empty or stale | log: `Maintainerr not reachable yet` or `sync failed` (URL, network, API key) |
| No passkey button, invitations are bare codes | `ALWYWATCH_PUBLIC_URL` is unset |
| "Keep until watched" never ends | `JELLYFIN_API_KEY` / `[tautulli]` + `TAUTULLI_API_KEY` missing |
| Everyone gets "too many attempts" together | the proxy doesn't send `X-Forwarded-For`, or there is a second proxy in front |
| The web app can't sign in to Jellyfin | Jellyfin's `public_url` must be https when the site is https |

The protocol between apps and server is in [keep-protocol.md](keep-protocol.md).
