# syntax=docker/dockerfile:1

FROM rust:1-alpine AS build
RUN apk add --no-cache musl-dev
WORKDIR /src

# Dependencies first, so editing finstats itself doesn't rebuild the world.
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src web && echo 'fn main() {}' > src/main.rs \
    && cargo build --release --locked \
    && rm -rf src target/release/deps/finstats-* target/release/finstats

COPY src ./src
COPY web ./web
# Compiled into the binary: the in-app patch notes.
COPY CHANGELOG.md ./
RUN cargo build --release --locked

FROM alpine:3.22
# Ties the published image to its source; the release workflow adds version, revision and date.
LABEL org.opencontainers.image.title="finstats" \
      org.opencontainers.image.description="Lightweight playback statistics for Jellyfin" \
      org.opencontainers.image.source="https://github.com/OlaYZen/finstats" \
      org.opencontainers.image.licenses="GPL-3.0-only"
# tzdata: "plays per day" and the hour-of-day heatmap follow the TZ variable.
RUN apk add --no-cache tzdata \
    && addgroup -g 1000 finstats && adduser -D -u 1000 -G finstats finstats \
    && mkdir /data && chown finstats:finstats /data
COPY --from=build /src/target/release/finstats /usr/local/bin/finstats

USER finstats
ENV FINSTATS_DATA_DIR=/data \
    FINSTATS_BIND=0.0.0.0:8080
VOLUME /data
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s \
    CMD wget -qO /dev/null http://127.0.0.1:8080/api/status || exit 1
ENTRYPOINT ["finstats"]
