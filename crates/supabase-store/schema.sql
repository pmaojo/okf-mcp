-- Esquema para Supabase / PostgreSQL (okf-mcp)

CREATE TABLE IF NOT EXISTS blobs (
    content_id VARCHAR(64) PRIMARY KEY, -- SHA-256 en representación hexadecimal
    raw TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS heads (
    concept_id VARCHAR(255) PRIMARY KEY, -- Identificador lógico, p. ej. 'people/alice'
    content_id VARCHAR(64) NOT NULL REFERENCES blobs(content_id),
    version BIGINT NOT NULL,
    doc_type VARCHAR(100) NOT NULL,
    title VARCHAR(255),
    tags TEXT[] NOT NULL,
    deleted_at TIMESTAMP WITH TIME ZONE -- borrado lógico: NULL = vivo
);

CREATE TABLE IF NOT EXISTS revisions (
    seq BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    concept_id VARCHAR(255) NOT NULL,
    base VARCHAR(64) REFERENCES blobs(content_id),
    result VARCHAR(64) NOT NULL REFERENCES blobs(content_id),
    actor_subject VARCHAR(255) NOT NULL,
    actor_client_id VARCHAR(255) NOT NULL,
    reason TEXT NOT NULL,
    created_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS links (
    source_id VARCHAR(255) NOT NULL REFERENCES heads(concept_id) ON DELETE CASCADE,
    target_id VARCHAR(255) NOT NULL,
    rel VARCHAR(64), -- relación tipada de '[[rel:destino]]'; NULL = enlace genérico
    PRIMARY KEY (source_id, target_id)
);

-- Índices para optimizar las búsquedas y la paginación de historia
CREATE INDEX IF NOT EXISTS idx_revisions_concept_seq ON revisions(concept_id, seq DESC);
CREATE INDEX IF NOT EXISTS idx_links_target ON links(target_id);

CREATE TABLE IF NOT EXISTS outbox (
    seq BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    event_type VARCHAR(50) NOT NULL, -- p. ej. 'commit'
    concept_id VARCHAR(255) NOT NULL,
    content_id VARCHAR(64) NOT NULL,
    payload JSONB NOT NULL,
    status VARCHAR(20) NOT NULL DEFAULT 'pending', -- 'pending', 'processed', 'failed'
    attempts INT NOT NULL DEFAULT 0,
    created_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP,
    processed_at TIMESTAMP WITH TIME ZONE
);

CREATE INDEX IF NOT EXISTS idx_outbox_status_seq ON outbox(status, seq);

-- Habilitar extensión pgvector y crear tabla de embeddings
CREATE EXTENSION IF NOT EXISTS vector;

CREATE TABLE IF NOT EXISTS embeddings (
    concept_id VARCHAR(255) PRIMARY KEY REFERENCES heads(concept_id) ON DELETE CASCADE,
    embedding vector(768), -- gemini-embedding-001 truncado a 768 dims (ver gemini-embeddings::DIMENSIONS)
    content_id VARCHAR(64) -- de qué contenido es este vector; NULL = generado antes de rastrearlo
);

-- Migración en el arranque: este archivo se aplica con CREATE TABLE
-- IF NOT EXISTS en cada cold start, así que las tablas pueden venir
-- de una versión anterior sin las columnas nuevas. ADD COLUMN IF NOT
-- EXISTS es idempotente y no toca los datos existentes.
ALTER TABLE heads ADD COLUMN IF NOT EXISTS deleted_at TIMESTAMP WITH TIME ZONE;
ALTER TABLE links ADD COLUMN IF NOT EXISTS rel VARCHAR(64);
ALTER TABLE embeddings ADD COLUMN IF NOT EXISTS content_id VARCHAR(64);
