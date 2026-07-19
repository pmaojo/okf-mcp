# Dockerfile para desplegar outbox-worker en Render (u otra plataforma con Docker).
# Compila el workspace de Cargo en release y ejecuta el binario del worker.

FROM rust:1.92-slim-bookworm AS builder

WORKDIR /app

# Instalar dependencias del sistema necesarias para compilar sqlx y openssl.
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Copiar el workspace completo y compilar el binario del outbox-worker.
COPY . .
RUN cargo build --release -p outbox-worker

# ---------------------------------------------------------------------------

FROM debian:bookworm-slim

WORKDIR /app

# Instalar certificados CA para que las peticiones HTTPS a GitHub/Gemini funcionen.
RUN apt-get update && apt-get install -y \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Copiar solo el binario compilado desde la etapa anterior.
COPY --from=builder /app/target/release/outbox-worker /usr/local/bin/outbox-worker

# El worker no expone puertos; es un proceso en segundo plano.
CMD ["outbox-worker"]
