# syntax=docker/dockerfile:1

FROM rust:1-alpine AS build
RUN apk add --no-cache musl-dev
WORKDIR /src

# Dependencies first, so editing finstats itself doesn't rebuild the world. The two cache mounts make
# even that incremental between builds on one machine: the crate registry and the target directory
# survive, so a changed source file recompiles finstats and relinks instead of building everything
# again — minutes off every local rebuild and every run of the QA image check. They are BuildKit
# caches and change nothing about the image: the binary is copied out of the cache onto a real layer,
# and a builder without them (a cold CI runner) simply builds as before.
COPY Cargo.toml Cargo.lock ./
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=finstats-registry \
    --mount=type=cache,target=/src/target,id=finstats-target \
    mkdir -p src web && echo 'fn main() {}' > src/main.rs \
    && cargo build --release --locked \
    && rm -rf src target/release/deps/finstats-* target/release/finstats

COPY src ./src
COPY web ./web
# Compiled into the binary: the in-app patch notes.
COPY CHANGELOG.md ./
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=finstats-registry \
    --mount=type=cache,target=/src/target,id=finstats-target \
    cargo build --release --locked && cp target/release/finstats /finstats

FROM alpine:3.22
# Ties the published image to its source; the release workflow adds version, revision and date.
LABEL org.opencontainers.image.title="finstats" \
      org.opencontainers.image.description="Lightweight playback statistics for Jellyfin" \
      org.opencontainers.image.source="https://github.com/OlaYZen/finstats" \
      org.opencontainers.image.licenses="GPL-3.0-only"
# tzdata: "plays per day" and the hour-of-day heatmap follow the TZ variable.
# su-exec: the entrypoint drops from root to the finstats user with it.
RUN apk add --no-cache tzdata su-exec \
    && addgroup -g 1000 finstats && adduser -D -u 1000 -G finstats finstats \
    && mkdir /data && chown finstats:finstats /data
COPY --from=build /finstats /usr/local/bin/finstats
COPY docker-entrypoint.sh /usr/local/bin/docker-entrypoint.sh

# No USER line on purpose: the entrypoint starts as root only to make /data writable (Docker creates a
# missing bind-mount folder as root), then runs finstats as PUID:PGID, 1000:1000 unless told otherwise.
ENV FINSTATS_DATA_DIR=/data \
    FINSTATS_BIND=0.0.0.0:8080
VOLUME /data
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s \
    CMD wget -qO /dev/null http://127.0.0.1:8080/api/status || exit 1
ENTRYPOINT ["docker-entrypoint.sh"]
