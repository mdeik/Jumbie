# ── Build Arguments ─────────────────────────────────────────────────────
# These can be passed at build time via CI/CD (e.g., docker build --build-arg VERSION=0.1.0).
# VERSION defaults to the Cargo.toml version. BUILD_DATE defaults to now.
# GIT_COMMIT defaults to "unknown" if not set.
ARG VERSION=0.1.0
ARG BUILD_DATE
ARG GIT_COMMIT=unknown
# Trunk wasm bundler — keep in sync with TRUNK_VERSION in .github/workflows/rust.yml.
ARG TRUNK_VERSION=v0.21.14
# Rust base image — keep in sync with the channel in rust-toolchain.toml.
ARG RUST_VERSION=1.98.0

# --- Chef Stage ---
# Base image with cargo-chef installed for dependency caching.
FROM rust:${RUST_VERSION}-slim-trixie AS chef

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    ca-certificates \
    build-essential \
    lld \
    m4 \
    && rm -rf /var/lib/apt/lists/*

RUN cargo install cargo-chef --locked

# Link native binaries with lld (much faster than bfd). The wasm frontend
# build is unaffected — its target doesn't match this table.
RUN mkdir -p /app/.cargo && printf '[target.x86_64-unknown-linux-gnu]\nrustflags = ["-C", "link-arg=-fuse-ld=lld"]\n' > /app/.cargo/config.toml

WORKDIR /app

# --- Planner Stage ---
# Scans the workspace and produces a recipe.json listing all dependencies.
# This recipe is used by `cargo chef cook` to build & cache deps separately
# from the project source, so dependency layers are reused across builds.
FROM chef AS planner

COPY Cargo.toml Cargo.lock ./
COPY backend/Cargo.toml backend/
COPY crates/shared/Cargo.toml crates/shared/
COPY crates/plugin-sdk/Cargo.toml crates/plugin-sdk/
COPY frontend/Cargo.toml frontend/

# Touch dummy entry-points so cargo metadata can validate the workspace
RUN mkdir -p backend/src crates/shared/src crates/plugin-sdk/src frontend/src && \
    touch \
    backend/src/main.rs \
    crates/shared/src/lib.rs \
    crates/plugin-sdk/src/lib.rs \
    frontend/src/lib.rs

RUN cargo chef prepare --recipe-path recipe.json

# --- Frontend Builder ---
FROM chef AS frontend-builder

ARG TRUNK_VERSION

RUN apt-get update && apt-get install -y --no-install-recommends \
    curl \
    wget \
    && rm -rf /var/lib/apt/lists/*

RUN rustup target add wasm32-unknown-unknown

# Install Trunk (WASM bundler)
RUN wget -qO- https://github.com/trunk-rs/trunk/releases/download/${TRUNK_VERSION}/trunk-x86_64-unknown-linux-gnu.tar.gz | tar -xzf- && \
    mv trunk /usr/local/bin/

# Copy workspace scaffolding needed for trunk (cargo metadata resolves all members)
COPY Cargo.toml Cargo.lock ./
COPY backend/Cargo.toml backend/
COPY crates/shared/Cargo.toml crates/shared/
COPY crates/plugin-sdk/Cargo.toml crates/plugin-sdk/
COPY frontend/Cargo.toml frontend/

# Dummy entry-point so cargo metadata can validate the workspace
RUN mkdir -p backend/src && \
    touch backend/src/main.rs

# Copy the full source for workspace crates the frontend depends on
COPY crates/shared/ crates/shared/
COPY crates/plugin-sdk/ crates/plugin-sdk/

# Copy & build the frontend
COPY frontend/ frontend/
WORKDIR /app/frontend
RUN trunk build --release
WORKDIR /app

# --- Backend Builder ---
FROM chef AS backend-builder

# Inject version info for build.rs to embed into the binary
ARG VERSION
ARG GIT_COMMIT
ARG BUILD_DATE
ENV SOURCE_VERSION=${GIT_COMMIT}
ENV SOURCE_DATE_EPOCH=${BUILD_DATE}

# 1. Copy the dependency recipe and cook (caches all crate dependencies)
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json --no-default-features --features docker-defaults --bin jumbie

# 2. Copy the built frontend dist so build.rs finds real content for rust-embed
COPY --from=frontend-builder /app/frontend/dist frontend/dist

# 3. Copy workspace scaffolding
COPY Cargo.toml Cargo.lock ./
COPY backend/Cargo.toml backend/
COPY crates/ crates/

# 4. Copy the actual source (only what changed, after deps are cached)
COPY backend/ backend/

# 5. Build the release binary, then strip debug symbols to save ~30% size
RUN cargo build --release --no-default-features --features docker-defaults --bin jumbie && \
    strip /app/target/release/jumbie

# --- FFmpeg Downloader ---
# Static build of ffmpeg/ffprobe to avoid Debian's massive shared library
# dependency chain (~370MB of GPU/display libraries). These are compiled
# with all common codecs and have zero shared library dependencies.
FROM debian:trixie-slim AS ffmpeg-downloader

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    xz-utils \
    && rm -rf /var/lib/apt/lists/*

ADD https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-linux64-gpl.tar.xz /tmp/ffmpeg.tar.xz
RUN tar -xf /tmp/ffmpeg.tar.xz -C /tmp \
    && cp /tmp/ffmpeg-*/bin/ffmpeg /usr/local/bin/ \
    && cp /tmp/ffmpeg-*/bin/ffprobe /usr/local/bin/ \
    && rm -rf /tmp/ffmpeg.tar.xz /tmp/ffmpeg-*

# --- Runtime ---
FROM debian:trixie-slim

ARG VERSION
ARG BUILD_DATE
ARG GIT_COMMIT

# ── OCI Image Labels ────────────────────────────────────────────────────
# These labels follow the OCI Image Spec conventions for provenance.
# They can be inspected with: docker inspect <image>
LABEL org.opencontainers.image.title="Jumbie"
LABEL org.opencontainers.image.description="Series organizer and media manager"
LABEL org.opencontainers.image.version="${VERSION}"
LABEL org.opencontainers.image.created="${BUILD_DATE}"
LABEL org.opencontainers.image.revision="${GIT_COMMIT}"
LABEL org.opencontainers.image.url="https://github.com/mdeik/Jumbie"
LABEL org.opencontainers.image.source="https://github.com/mdeik/Jumbie"
LABEL org.opencontainers.image.licenses="MIT"

WORKDIR /app

# Install runtime dependencies only (not build tools)
RUN apt-get update && apt-get install -y --no-install-recommends \
    libssl3 \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Static ffmpeg/ffprobe (no Debian package dependency chain)
COPY --from=ffmpeg-downloader /usr/local/bin/ffmpeg /usr/local/bin/
COPY --from=ffmpeg-downloader /usr/local/bin/ffprobe /usr/local/bin/

# Copy the stripped release binary
COPY --from=backend-builder /app/target/release/jumbie /usr/local/bin/

RUN mkdir -p /config /data /plugins /logs /tmp/jumbie

# ── Runtime Environment Variables ──────────────────────────────────────────────
# These env vars override the corresponding fields in config.toml at runtime.
# They affect in-memory values only — config.toml is never written to.
#
#   JUMBIE_CONFIG      Path to config.toml (default: /config/config.toml)
#   JUMBIE_DATABASE    Path to the SQLite database file
#   JUMBIE_PLUGINS_DIR Directory containing external plugins
#   JUMBIE_TMP_DIR     Temporary directory for unknown/unmatched files
#   JUMBIE_LOGS_DIR    Directory for log files (default: /logs in container)
#   JUMBIE_LOG_LEVEL   Logging level: trace, debug, info, warn, error
#                      (default: info, use "debug" for verbose output)
#
# The frontend is embedded in the binary (rust-embed) — no separate
# frontend directory configuration is needed.
ENV JUMBIE_DATABASE=/data/jumbie.db

EXPOSE 3000

CMD ["jumbie"]
