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

**No telemetry, and one outside request you can switch off.** finstats talks to your Jellyfin server and, by
default, to a public "what is my IP" service (`checkip.amazonaws.com`, falling back to Cloudflare's `cdn-cgi/trace`, by name and by `1.1.1.1`,
then `api.ipify.org` and `icanhazip.com`; several because ad-blocking DNS often blocks such services) every 15 minutes. It needs the answer to tell plays from your own household's public address apart
from remote ones. The request is a bare `GET` with `User-Agent: finstats` and `Accept: text/plain`: no version, no
identifiers, nothing about your server or users. What the service necessarily learns is that *something* at your
address asked. Turn off **Settings → Home network → Recognise my own public address** and finstats makes no
connections other than to Jellyfin; `FINSTATS_PUBLIC_IP_URL` points the lookup at a service of your own instead.