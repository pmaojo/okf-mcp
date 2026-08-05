# syntax=docker/dockerfile:1
#
# Imagen del daemon de `outbox-worker` (ver README, sección "Outbox,
# GitHub y embeddings"): el proceso de larga duración pensado para
# correr fuera de Vercel — Fly.io, Railway, un VPS con `docker compose`.
# El Cron de Vercel (`/api/outbox`) sigue siendo la red de seguridad de
# una vez al día; esta imagen es el camino rápido (loop cada pocos
# segundos, ver `crates/outbox-worker/src/main.rs`).
#
# El contexto de build DEBE ser la raíz del repo (no este directorio):
# es un workspace de Cargo con dependencias `path = "../otro-crate"`,
# así que el build necesita ver el workspace entero.
#
#   docker build -f crates/outbox-worker/Dockerfile -t okf-outbox-worker .
#
# (o usa el docker-compose.yml de la raíz, que ya pasa este -f y este
# contexto por ti).

FROM rust:1-slim-bookworm AS builder

# `reqwest`/`sqlx` de este crate compilan contra OpenSSL (no rustls) —
# ver el Cargo.lock: aparecen `native-tls`/`openssl-sys`. pkg-config +
# libssl-dev son el par mínimo para que `openssl-sys` los encuentre.
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY . .

# Cache mounts de BuildKit en vez del truco clásico de "copiar solo
# Cargo.toml primero": con un workspace de 19 crates y dependencias
# `path = ...` entre ellos, ese truco exige generar un stub por cada
# miembro y es frágil ante cambios de estructura. El cache mount
# persiste el registry descargado y el `target/` compilado entre
# builds en el MISMO host — justo el caso de uso real aquí (una VPS
# donde reconstruyes esta imagen de vez en cuando), no el de un
# runner de CI efímero.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    cargo build --release -p outbox-worker \
    && cp target/release/outbox-worker /usr/local/bin/outbox-worker

FROM debian:bookworm-slim AS runtime

# ca-certificates: outbox-worker habla HTTPS con la API de GitHub, de
# Gemini/Mistral/Cohere y con el pooler de Supabase — sin esto, cada
# conexión TLS falla por no poder validar el certificado del servidor.
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates libssl3 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --no-create-home --shell /usr/sbin/nologin outbox

COPY --from=builder /usr/local/bin/outbox-worker /usr/local/bin/outbox-worker

USER outbox
ENTRYPOINT ["/usr/local/bin/outbox-worker"]
