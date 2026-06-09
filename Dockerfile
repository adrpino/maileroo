# Stage 1: Cargo-chef base
FROM rust:slim-bookworm AS chef
RUN cargo install cargo-chef --locked
WORKDIR /app

# Stage 2: Recipe planner
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# Stage 3: Dependency builder
FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json

# Install compilation build dependencies (needed for crypto crates like aws-lc-rs)
RUN apt-get update && apt-get install -y --no-install-recommends \
    cmake \
    build-essential \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

RUN cargo chef cook --release --recipe-path recipe.json

# Build the main binary
COPY . .
RUN cargo build --release --bin maileroo

# Stage 4: Minimal runtime container
FROM debian:bookworm-slim AS runtime
WORKDIR /app

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/maileroo /usr/local/bin/maileroo

# Add secure non-root user
RUN groupadd -g 10001 maileroo && \
    useradd -u 10001 -g maileroo -m -s /bin/sh maileroo
RUN mkdir -p /app/storage && chown -R maileroo:maileroo /app/storage
VOLUME /app/storage
USER maileroo

EXPOSE 3000 2525

# Standard environment defaults
ENV STORAGE_DIR=/app/storage
ENV ACME_CACHE_DIR=/app/storage/certs/acme
ENV DATABASE_URL=sqlite:///app/storage/maileroo.db?mode=rwc

# Labels to automatically link this image to your GitHub repo
LABEL org.opencontainers.image.source="https://github.com/adrpino/maileroo"
LABEL org.opencontainers.image.description="Self-contained open source secure email server and relay"
LABEL org.opencontainers.image.licenses="MIT"

ENTRYPOINT ["/usr/local/bin/maileroo"]
