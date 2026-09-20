# Security policy

finstats holds things worth protecting: who watched what and when, the IP addresses they watched from, and
an API key for your Jellyfin server. Reports are taken seriously and handled privately.

## Supported versions

Fixes go into the newest release. Please check that the problem still exists on the latest version
(`ghcr.io/olayzen/finstats:latest`) before reporting.

| Version | Supported |
|---|---|
| 1.x (latest) | yes |
| older | no: update first |

## Reporting a vulnerability

**Do not open a public issue, discussion or pull request for a vulnerability.**

Use GitHub's private reporting: open the repository's
[**Security** tab → **Report a vulnerability**](https://github.com/OlaYZen/finstats/security/advisories/new).
Only the maintainers can see the report.

If that button is not available to you, open a regular issue that says only *"I would like to report a
security problem privately"*, with no details, and a maintainer will set up a private channel.

A useful report says:

- the finstats version and how it is run (image tag, reverse proxy, anything unusual);
- what an attacker needs (no account, a signed-in user without permissions, a manager, network position…);
- what they get (other people's history, IP addresses, the API key, a write, a crash…);
- steps to reproduce, ideally against a fresh install with invented data.

Please do not include real viewing data, a database, a backup or a Jellystat export.

## What to expect

You will get an answer, normally within a week. If the report is confirmed, a fix is released as soon as it
is ready, the advisory is published with credit to you unless you prefer otherwise, and the patch notes say
plainly what was wrong. Please give a fix a reasonable chance to reach users before publishing details.

## What is in scope

Anything that lets someone see, change or destroy what they should not: reading another user's plays, IP
addresses or file paths without the matching permission; opening someone else's recap; raising one's own
permissions; getting at the Jellyfin API key or a session; reaching files outside the web UI; cross-site
request forgery or script injection; a backup that contains a secret; the container running finstats as
root.

Out of scope: problems that need an administrator account on the same Jellyfin server (an administrator can
already see everything), a finstats exposed to the internet without HTTPS, and vulnerabilities in Jellyfin
itself (report those to Jellyfin).

How finstats is meant to protect these things is described in [`docs/security.md`](docs/security.md).
