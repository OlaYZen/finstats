#!/bin/sh
# Starts finstats as an unprivileged user that can write its data directory.
#
# When the bind-mounted data folder does not exist yet, Docker creates it owned by root, and an
# unprivileged process then cannot create the database in it. So the container starts as root for
# exactly one job: make the data directory belong to the user finstats will run as. Then it drops
# privileges for good and never runs finstats itself as root.
#
#   PUID / PGID   the user and group to run as (default 1000:1000); match the owner of your files
#   --user …      start the container as that user instead; then nothing is changed and nothing is dropped
set -e

DATA="${FINSTATS_DATA_DIR:-/data}"

if [ "$(id -u)" != 0 ]; then
    exec finstats "$@"
fi

PUID="${PUID:-1000}"
PGID="${PGID:-1000}"
case "$PUID$PGID" in *[!0-9]*) echo "finstats: PUID and PGID must be numbers (got '$PUID' and '$PGID')" >&2; exit 64 ;; esac

mkdir -p "$DATA"
# Only when something is actually owned by someone else: a recursive chown over a large poster
# cache on every start would be slow, and pointless.
if [ -n "$(find "$DATA" -maxdepth 2 \( ! -user "$PUID" -o ! -group "$PGID" \) -print -quit 2>/dev/null)" ]; then
    chown -R "$PUID:$PGID" "$DATA" 2>/dev/null \
        || echo "finstats: could not change the owner of $DATA (read-only or rootless mount?); continuing as $PUID:$PGID" >&2
fi

exec su-exec "$PUID:$PGID" finstats "$@"
