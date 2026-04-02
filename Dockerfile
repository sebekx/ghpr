# ── Build stage ──────────────────────────────────────────────
FROM rust:latest AS builder

RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY Cargo.toml Cargo.lock* ./
COPY src/ src/

RUN cargo build --release

# ── Runtime stage ────────────────────────────────────────────
FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y ca-certificates libssl3 && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/ghpr /usr/local/bin/ghpr

ENTRYPOINT ["ghpr"]

# ── Export stage: copy binary out ────────────────────────────
# Usage:
#   docker build --target export --output type=local,dest=./dist .
FROM scratch AS export
COPY --from=builder /app/target/release/ghpr /ghpr
