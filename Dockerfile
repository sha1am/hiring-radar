# syntax=docker/dockerfile:1

# ---------- frontend ----------
# Node builds the React bundle; the runtime image never sees Node. Its own stage
# so that editing a .tsx file doesn't invalidate the Rust layers, and vice
# versa — the two halves of this project change at very different rates.
FROM node:22-slim AS ui

WORKDIR /ui

# Dependency layer: package.json alone, so `npm ci` is cached across every
# source edit. package-lock.json is copied with it when present.
COPY frontend/package*.json ./
RUN npm ci --no-audit --no-fund

COPY frontend/ ./
RUN npm run build

# ---------- builder ----------
# Deps are built in their own layer so day-to-day source edits rebuild in
# seconds instead of re-compiling tokio/axum/sqlx every time.
FROM rust:1-slim-bookworm AS builder

WORKDIR /app

# blake3 and the bundled libsqlite3-sys both need a C toolchain. TLS is rustls
# throughout (see Cargo.toml), so there is deliberately no OpenSSL here.
RUN apt-get update \
 && apt-get install -y --no-install-recommends pkg-config \
 && rm -rf /var/lib/apt/lists/*

# Dependency layer: a stub main.rs lets cargo resolve and build every crate
# without the real sources being present.
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src \
 && echo 'fn main() {}' > src/main.rs \
 && cargo build --release \
 && rm -rf src

COPY src ./src
RUN touch src/main.rs \
 && cargo build --release \
 && (strip target/release/hiring-radar || true)

# ---------- runtime ----------
FROM debian:bookworm-slim

# ca-certificates for outbound TLS (Greenhouse, Workday, LinkedIn, ntfy, SMTP);
# curl only so HEALTHCHECK has something to call.
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --create-home --uid 10001 radar \
 && mkdir -p /data \
 && chown radar:radar /data

COPY --from=builder /app/target/release/hiring-radar /usr/local/bin/hiring-radar

# The built dashboard. Static files served by the Rust process — no nginx, no
# second container, and the SPA is same-origin with its own API, so there is no
# CORS to configure.
COPY --from=ui /ui/dist /app/ui

# The starter company lists, baked in so a fresh `docker compose up` crawls
# something. Compose bind-mounts ./companies over this, so editing the files on
# the host takes effect on the next crawl — no rebuild, no restart. Baking them
# in as well means the image still works when run without compose.
COPY companies /app/companies

USER radar
WORKDIR /app

# config.toml is mounted read-only at runtime; state lives on the /data volume.
ENV RADAR_CONFIG=/app/config.toml \
    RADAR_COMPANIES_DIR=/app/companies \
    RADAR_UI_DIR=/app/ui \
    RADAR_DB=/data/hiring.db \
    RADAR_BIND=0.0.0.0:8080 \
    RUST_LOG=hiring_radar=info,tower_http=warn

VOLUME ["/data"]
EXPOSE 8080

# /api/status, not /: the SPA's index.html is a static file that would come back
# 200 even with the database gone. The API answering means the process is
# actually working.
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
  CMD curl -fsS http://127.0.0.1:8080/api/status > /dev/null || exit 1

CMD ["hiring-radar"]
