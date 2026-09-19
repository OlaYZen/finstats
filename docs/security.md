# Security model

**Signing in.** finstats has no accounts of its own. A sign-in is checked against your Jellyfin
server every time, and the Jellyfin session that check creates is ended immediately. Passwords are
never stored or logged. finstats then issues its own session: a random 256-bit token, stored only
as a hash, sent as an `HttpOnly; SameSite=Lax` cookie (and `Secure` when a proxy reports HTTPS).
Attempts are rate limited to 10 per 5 minutes per address.

**Who sees what.** Administrators see everything. Other users can only sign in when you allow it,
and are then limited to their own statistics: no other people's activity, no IP addresses, no
device ids, no file paths, no server log, no settings. This is enforced by the server on every
request, not by hiding things in the interface. The year recap is personal for everyone,
administrators included.

**Talking to Jellyfin.** During setup finstats creates its own API key, named `finstats`, which
you can revoke at any time under Dashboard → API Keys. It only reads: it never modifies your
server and never starts a library scan. The key is stored in `data/finstats.db`, so protect the
data folder like you would protect Jellyfin's own.

**Setup window.** Until setup is completed, anyone who can reach the port can open the wizard —
but finishing it requires a Jellyfin administrator's credentials.

**The web interface** loads nothing from third parties: fonts and scripts are bundled, posters are
proxied from your own Jellyfin, and a strict Content-Security-Policy is sent with every response.
Requests that change anything are refused when their `Origin` does not match.

**No telemetry.** finstats makes no network connections other than to your Jellyfin server.
