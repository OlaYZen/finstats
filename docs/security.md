# Security model

**Signing in.** finstats has no accounts of its own. A sign-in is checked against your Jellyfin
server every time, and the Jellyfin session that check creates is ended immediately. Passwords are
never stored or logged. finstats then issues its own session: a random 256-bit token, stored only
as a hash, sent as an `HttpOnly; SameSite=Lax` cookie (and `Secure` when a proxy reports HTTPS).
Attempts are rate limited to 10 per 5 minutes per address.

**Who sees what.** Jellyfin administrators see everything. Other users can only sign in when you
allow it — for everyone, or person by person — and then start with their own statistics only: no
other people's activity, no IP addresses, no device ids, no file paths, no server log, no settings.
An administrator can grant more under Settings → Access:

| Permission | Opens up |
|---|---|
| Sign in | Using finstats at all, when sign-in is not open to everyone. |
| See everyone's activity | Other people's statistics and history, the Users page, every live stream. |
| See network details | IP addresses, device ids, local versus remote. |
| See the server | The Server page, the server log, failed sign-ins, file paths. |
| Manage finstats | Settings, tasks, the Jellystat import, deleting plays. |

Grants for everyone and grants for one person add up; there are no "deny" rules to reason about.
All of it is enforced by the server on every request, not by hiding things in the interface, and
is read fresh each time, so taking a permission away works immediately — including locking
someone out. Only a Jellyfin administrator can change permissions: someone who may *manage*
finstats still cannot open sign-in to everyone, change the defaults or grant anything, so nobody
can promote themselves. The year recap is personal: you get your own. A Jellyfin administrator can open another
person's; no permission grants that to anyone else, and there is no whole-server recap.

**Talking to Jellyfin.** During setup finstats creates its own API key, named `finstats`, which
you can revoke at any time under Dashboard → API Keys. It only reads: it never modifies your
server and never starts a library scan. The key is stored in `data/finstats.db`, so protect the
data folder like you would protect Jellyfin's own.

**Setup window.** Until setup is completed, anyone who can reach the port can open the wizard —
but finishing it requires a Jellyfin administrator's credentials.

**The web interface** loads nothing from third parties: fonts and scripts are bundled, posters are
proxied from your own Jellyfin, and a strict Content-Security-Policy is sent with every response.
Requests that change anything are refused when their `Origin` does not match.

**The container** runs finstats as an unprivileged user (1000:1000, or `PUID`:`PGID`). It starts as root for one
step only: making the data directory belong to that user, because Docker creates a missing bind-mount folder as root.
It then drops privileges with `su-exec` and cannot get them back; finstats itself never runs as root. Start the
container with `--user` and even that step is skipped.

**Backups** (`data/backups`, and whatever you download from **Settings → Backups**) hold the full viewing history
with IP addresses, the permissions and the settings. They never contain the Jellyfin address or API key, nor any
sign-in session, so a leaked backup exposes history but grants no access. Only Jellyfin administrators can list,
download, delete or restore them, and a backup's file name is checked against the exact pattern finstats generates
before it touches the disk. Restoring validates the settings it brings back the same way the settings page does.

**No telemetry, and two outside requests: one you can switch off, one that is off until you switch it on.** finstats talks to your Jellyfin server and, by
default, to a public "what is my IP" service (`checkip.amazonaws.com`, falling back to Cloudflare's `cdn-cgi/trace`, by name and by `1.1.1.1`,
then `api.ipify.org` and `icanhazip.com`; several because ad-blocking DNS often blocks such services) every 15 minutes. It needs the answer to tell plays from your own household's public address apart
from remote ones. The request is a bare `GET` with `User-Agent: finstats` and `Accept: text/plain`: no version, no
identifiers, nothing about your server or users. What the service necessarily learns is that *something* at your
address asked. Turn off **Settings → Home network → Recognise my own public address** and finstats makes no
connections other than to Jellyfin; `FINSTATS_PUBLIC_IP_URL` points the lookup at a service of your own instead.

The second is the geolocation database behind the **Security** page. Looking an address up never leaves the machine: finstats
reads a city database file (`.mmdb`) in its data folder. Getting that file is the only part that can touch the network, and it
is off by default. With **Settings → Security → Keep the database up to date** on (or the *Download* button), finstats fetches
DB-IP's free "IP to City Lite" file from `download.db-ip.com`, once now and then monthly: a plain `GET` of a public file with
`User-Agent: finstats`, carrying no address of yours, no version and no identifiers. What DB-IP necessarily learns is that
something at your address downloaded its public file. Leave it off and put a file into `<data>/geoip/` yourself (DB-IP's, MaxMind's
GeoLite2-City, or any other in that format; `FINSTATS_GEOIP_DB` names one elsewhere) and finstats asks nobody. The map is drawn
from outlines bundled with finstats; no map tiles or map service are involved, so no coordinate ever leaves the browser.

**What the Security page is, and is not.** A place is the centre of a city or of an ISP's region, never a household, and it can be
hundreds of kilometres off; a VPN or a phone on mobile data looks like a trip. Alerts (*impossible travel*, *new country*) are a
reason to look, not proof. The page needs both *see network details* and *see everyone's activity*; resolving alerts needs *manage*.

**The services you connect.** Under **Settings → Connections** a Jellyfin administrator can point finstats at Sonarr, Radarr and Seerr. These are
requests to addresses *you* enter, normally on your own network; finstats contacts nothing on its own account, and the two outside requests above
stay the only ones. Your download client is not connected to finstats at all: Sonarr and Radarr already talk to it, and finstats reads their
queues instead.
- **Read-only.** finstats sends `GET` to these services and nothing else — there is no code in it that could approve a request, start a search,
  or add, pause or remove a download.
- **Keys and passwords** are stored in finstats' database next to the Jellyfin key, are never sent back to the browser (the settings page only
  learns that one is stored), never written to a log and never part of a backup. They travel in headers or request bodies, never in an address.
- **No redirect is followed.** A key would travel along a redirect to wherever it points, so finstats reports a redirect as an error and asks for
  the final address instead.
- **Certificates are verified.** A service with a self-signed certificate needs "Accept a self-signed certificate" switched on for that one
  connection; finstats then does not verify who answers there. Plain `http://` on your own network needs no switch.
