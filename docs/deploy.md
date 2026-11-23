# Setting up alwywatch-server

`alwywatch-server` is the optional server side of AlwyWatch: one small Docker container. It
can run anywhere that can reach your services over the network: on the same machine as
Jellyfin/Plex and Maintainerr, or on another one (a VPS, for instance), with their addresses
in its settings. Nothing else has to be installed next to it.

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
  create local Seerr users (an option in Server settings → Seerr).
* **Jellyfin, Plex, Tautulli**: read only.

It never stores Jellyfin/Plex passwords or tokens. AlwyWatch account passwords are stored as
Argon2id hashes.

## Quick start

Everything is set up in the web app, like the *arr apps: the compose file only says where the
data folder is. Media servers, API keys, Maintainerr, Tautulli, Seerr, email and who may sign
up are in **Settings → Admin → Server settings**, each with a **Test** button.

### Before you start

* Docker with Compose, on any machine that can reach your services.
* Jellyfin and/or Plex, reachable by your users over https.
* A hostname with TLS on your reverse proxy (for example `watch.example.org`). Passkeys and
  invitation links need https. Maintainerr, Tautulli and Seerr can stay LAN-only.
* Optional: **Maintainerr 3.x** (Leaving soon), Tautulli, Seerr, an SMTP account to email
  invitations (the one Vaultwarden or another service uses is fine).

### 1. Build the image

There is no prebuilt image yet. From the repository root:

```sh
docker build -f server/Dockerfile -t alwywatch-server .
```

### 2. Start it

```sh
mkdir -p ~/docker/alwywatch/data && cd ~/docker/alwywatch
cp <repo>/server/deploy/docker-compose.yml .
sudo chown -R 65534:65534 data      # the container runs as an unprivileged user
```

In `docker-compose.yml`, replace `CHANGE_ME_default` with the Docker network your Jellyfin,
Maintainerr and reverse proxy are on (`docker network ls`). On a machine of its own, delete the
`networks` parts and use addresses reachable from there.

```sh
docker compose up -d && docker logs alwywatch-server
```

The log shows a **setup code**: `[setup] No media server yet. Open this site and enter the
setup code 123456`. Only someone who can read the log can set the server up.

### 3. Reverse proxy

The proxy must pass `X-Forwarded-For` (Caddy, Traefik and the nginx lines below do) and be the
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

### 4. Set it up in the browser

Open `https://watch.example.org`. The setup page asks for the code, then your first media
server:

* **Jellyfin**: its address and a Jellyfin **administrator's** username and password. They are
  used once, to make AlwyWatch's own API key in Jellyfin; they are not kept.
* **Plex**: its address, then sign in with the **Plex account that owns the server**.

Within the same Docker network, also give the server's internal address under "Advanced"
(`http://jellyfin:8096`): the container reaches it there faster than through the internet.

Then sign in with that same Jellyfin administrator or Plex owner: that is what makes you an
admin of AlwyWatch (every Jellyfin administrator is one). Open **Settings → Admin → Server
settings** and add what you use:

| Section | What it gives | Where to find what it asks |
|---|---|---|
| This site's address | email links, passkeys, Pocket ID | `https://watch.example.org` |
| Maintainerr | Leaving soon and keep choices | its address (`http://maintainerr:6246`), the media server it manages, its API key if you set one |
| Seerr | requests for everyone, whatever Seerr is set up for | Seerr → Settings → General → API Key |
| Tautulli | "keep until watched" ends by itself for Plex users | Tautulli → Settings → Web Interface → API key |
| Email | invitations sent by email | your SMTP server: host, port, security, login, sender address |
| Sign-up | who may create an AlwyWatch account | invitation only (default), anyone, nobody |
| Pocket ID | single sign-on | an OIDC client in Pocket ID with the callback shown there |

"Keep until watched" ends by itself for Jellyfin users as soon as the Jellyfin server is added
(AlwyWatch has its own key). Saving restarts the server in a second with the new settings.

**Leaving soon, the first time**: turn on **Test mode** before adding Maintainerr. The server
then reads everything and writes nothing (the log says `[dry-run] would …` for each change);
choices made in test mode are kept apart and dropped when you turn it off. Read [Before
activating a Maintainerr collection](#before-activating-a-maintainerr-collection) before you
let Maintainerr delete anything.

### 5. Invite your people

**Settings → Admin → Invitations**: type a friend's email, keep "Also make a Jellyfin login"
checked and pick whose access they get (libraries, ratings). They receive an email from you
with a link: one username and password, and they have an AlwyWatch account and a Jellyfin
login at once. Without an email, "Create link" gives a link and a code to send yourself.

People who already have a Jellyfin or Plex login just sign in with it, on the web or in the
apps ("Sign in to AlwyWatch" → `watch.example.org`). A TV or another device: "Sign in with a
code" there, then Settings → This device → Sign in another device on a device already signed
in (or scan the QR code).

### Moving from config.toml and .env

A server set up with `data/config.toml` and `.env` (before settings moved into the app) imports
them into `data/settings.json` at its first start with this version, and says so in the log.
From then on the app is where settings change: the old variables are ignored (the log lists any
still set) and can be removed from the compose file and `.env`.

### Webhooks

Without them, "watched" is checked every 15 minutes. **Server settings → Webhooks** shows the
addresses to paste (they contain a secret made by the server):

* **Tautulli** → Settings → Notification Agents → Webhook: method POST, trigger "Watched".
* **Jellyfin** with the Webhook plugin: a Generic destination, notification type
  "Playback Stop".

A 404 answer means the address is wrong.

## Administration

An admin is a **Jellyfin administrator** or the **owner of the Plex server**, signed in with
that login. Everything is in **Settings → Admin** on every app: people's rights, the Leaving
soon countdown, kept forever, invitations, server status, and (in the web app) server
settings.

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
use Admin → **Leaving soon countdown** for that collection: it restarts each item's delay (the
collection's "delete after days"), all at once or spread over 14 or 30 days. Then activate the
collection in Maintainerr.

## Backup, upgrade, removal

* **Backup**: the database holds the keep choices, accounts (password hashes, passkeys),
  rights and synced settings. Stop the container, copy the whole `data/` folder (it holds `settings.json`
  with your API keys and email password: keep the copy private), start it again. Without stopping: `sqlite3 data/keep.sqlite ".backup backup.sqlite"` (a plain
  file copy of a running database can miss recent changes).
* **Restore**: put the files back, `chown -R 65534:65534 data`, start.
* **Upgrade**: back up, `git pull`, rebuild the image (step 1), `docker compose up -d`. The
  database is updated at start. Apps and server can be updated in any order.
* **Removal**: `docker compose down`. The global exclusions it added stay in Maintainerr
  (Rules → Exclusions); remove them there if you want those titles deleted after all.

## Troubleshooting

| Symptom | Look at |
|---|---|
| The setup page says the code is wrong | the newest `[setup]` line of `docker logs alwywatch-server` (a new code at each start) |
| `cannot open /data/keep.sqlite…` in the log | `data/` isn't owned by uid 65534 |
| Apps say "this server is not served here" | that media server isn't in Server settings |
| Jellyfin sign-in fails with "media server unreachable" | the server's address (or internal address) isn't reachable from the container: Test it |
| No "Leaving soon" for someone | they aren't signed in with a login on one of your servers |
| "Leaving soon" is empty or stale | Settings → Admin → Server status, then Test in Server settings → Maintainerr |
| No passkey button, invitations are bare codes | Server settings: this site's address is empty |
| "Keep until watched" never ends | Jellyfin: add the server again (it makes its key); Plex: Tautulli |
| An invitation email isn't sent | Server settings → Email → Test, with "send a test email to" |
| Everyone gets "too many attempts" together | the proxy doesn't send `X-Forwarded-For`, or there is a second proxy in front |
| The web app can't sign in to Jellyfin | the Jellyfin address must be https when the site is https |

The protocol between apps and server is in [keep-protocol.md](keep-protocol.md).
