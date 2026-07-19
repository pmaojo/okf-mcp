#![forbid(unsafe_code)]

/// Ejecuta un Future de forma síncrona, tolerando si ya nos encontramos
/// dentro de un runtime de Tokio (como en Axum/Vercel) o fuera de él (tests).
///
/// Este macro evita tener que escribir `block_on(async move { ... })`
/// de forma repetitiva en los adaptadores síncronos del repositorio.
#[macro_export]
macro_rules! block_on {
    ($($body:tt)*) => {
        $crate::block_on(async move { $($body)* })
    };
}

mod repository;
mod maintenance;
mod neighbors;

use conflict_core::{decide, CommitDecision, Conflict};
use gemini_embeddings::embed;
use pgvector::Vector;
use graph_core::NeighborSource;
use hash_core::sha256;
use memory_model::{Budget, ConceptId, ContentId, Principal, Revision};
use store_core::{
    Backlink, BulkItem, BulkOutcome, CommitOutcome, CommitRequest, DeleteOutcome, DocumentView,
    EmbedOutcome, GraphStats, LinkHealth, MemoryRepository, SearchHit, SearchQuery, StoreError,
    StoreMaintenance, StoreStatus, ValidationReport,
};
use okf_core::Link;
use sqlx::postgres::{PgArguments, PgRow};
use sqlx::query::{Query, QueryScalar};
use sqlx::{FromRow, PgPool, Postgres, Row, Transaction};
use std::convert::Infallible;
use std::sync::Arc;

/// Todas las consultas de este adaptador se construyen con
/// `persistent(false)`: en producción `POSTGRES_URL` apunta al pooler
/// de Supabase en modo transacción, que puede entregar la misma
/// conexión física a otra sesión lógica entre transacciones. Un
/// prepared statement CON nombre (`sqlx_s_N`) sobrevive en el backend
/// y colisiona con el homónimo de otra sesión ("prepared statement
/// \"sqlx_s_N\" already exists"). `persistent(false)` hace que sqlx
/// use el statement SIN nombre del protocolo extendido, que se
/// re-prepara en cada uso y no puede colisionar. Verificado sobre el
/// código de sqlx 0.8: `statement_cache_capacity(0)` (el pool de
/// `vercel-entry`) solo evita CACHEAR el statement, no que reciba
/// nombre — por eso la protección vive aquí, consulta a consulta.
pub(crate) fn pg_query(sql: &str) -> Query<'_, Postgres, PgArguments> {
    sqlx::query(sql).persistent(false)
}

/// Igual que [`pg_query`], para consultas de una sola columna.
pub(crate) fn pg_query_scalar<'q, O>(sql: &'q str) -> QueryScalar<'q, Postgres, O, PgArguments>
where
    (O,): for<'r> FromRow<'r, PgRow>,
{
    sqlx::query_scalar(sql).persistent(false)
}

/// Helper para ejecutar de forma síncrona tareas asíncronas de base de datos.
/// Expuesto para uso interno del macro [`block_on!`].
pub fn block_on<F: std::future::Future>(fut: F) -> F::Output {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => tokio::task::block_in_place(|| handle.block_on(fut)),
        Err(_) => tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(fut),
    }
}

/// Adaptador de repositorio que persiste los datos en una base de datos Supabase / PostgreSQL.
#[derive(Debug, Clone)]
pub struct SupabaseStore {
    pool: PgPool,
    /// Si está configurada, `search` embebe el texto de la consulta
    /// con Gemini y ordena por similitud semántica (`pgvector`) en vez
    /// de la coincidencia de subcadena (`ILIKE`) — ver [`Self::search`].
    gemini_api_key: Option<String>,
}

/// Genera el embedding de `markdown` con Gemini y lo deja indexado en
/// `pgvector`, registrando de QUÉ contenido es el vector
/// (`content_id`): así `status`/`embed_pending` distinguen un
/// embedding al día de uno obsoleto.
///
/// Es la ÚNICA puerta de escritura al índice semántico: la usan el
/// camino inline (tras cada commit, best-effort) y `outbox-worker`
/// (la reparación asíncrona). Dos escritores, una función — que no
/// puedan divergir ni en modelo ni en dimensionalidad.
pub async fn index_embedding(
    pool: &PgPool,
    client: &reqwest::Client,
    gemini_key: &str,
    concept_id: &str,
    content_id_hex: &str,
    markdown: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let values = embed(client, gemini_key, markdown).await?;
    pg_query(
        "INSERT INTO embeddings (concept_id, embedding, content_id)
         VALUES ($1, $2, $3)
         ON CONFLICT (concept_id) DO UPDATE
             SET embedding = EXCLUDED.embedding,
                 content_id = EXCLUDED.content_id",
    )
    .bind(concept_id)
    .bind(Vector::from(values))
    .bind(content_id_hex)
    .execute(pool)
    .await?;
    Ok(())
}

impl SupabaseStore {
    /// Crea una nueva instancia de `SupabaseStore` a partir de las variables de entorno:
    /// - `POSTGRES_URL` (obligatoria)
    /// - `GEMINI_API_KEY` (opcional)
    pub fn from_env() -> Result<Self, StoreError> {
        let db_url = std::env::var("POSTGRES_URL")
            .map_err(|_| StoreError::Backend("falta POSTGRES_URL".into()))?;
        let gemini_api_key = std::env::var("GEMINI_API_KEY").ok();
        
        let pool = block_on(async {
            PgPool::connect(&db_url).await
        }).map_err(|e| StoreError::Backend(format!("Error conectando a Postgres: {e}")))?;

        Ok(Self::new(pool, gemini_api_key))
    }

    pub fn new(pool: PgPool, gemini_api_key: Option<String>) -> Self {
        SupabaseStore { pool, gemini_api_key }
    }

    async fn search_keyword(&self, query: &SearchQuery, limit: i64) -> Result<Vec<PgRow>, StoreError> {
        pg_query(
            "SELECT h.concept_id, h.content_id, h.doc_type, h.title, h.tags
             FROM heads h
             JOIN blobs b ON h.content_id = b.content_id
             WHERE h.deleted_at IS NULL
               AND ($1::text IS NULL OR h.doc_type = $1)
               AND ($2::text IS NULL OR $2 = ANY(h.tags))
               AND ($3::text IS NULL OR h.concept_id = $3 OR h.concept_id LIKE $3 || '/%')
               AND ($4::text IS NULL OR (
                   h.concept_id ILIKE $5 OR
                   h.title ILIKE $5 OR
                   b.raw ILIKE $5 OR
                   EXISTS (SELECT 1 FROM unnest(h.tags) t WHERE t ILIKE $5)
               ))
             ORDER BY h.concept_id
             LIMIT $6",
        )
        .bind(query.doc_type.as_deref())
        .bind(query.tag.as_deref())
        .bind(query.path_prefix.as_deref())
        .bind(query.text.as_deref())
        .bind(query.text.as_ref().map(|t| format!("%{}%", t)).as_deref())
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| StoreError::Backend(e.to_string()))
    }

    /// Ranking por similitud semántica: `<=>` es la distancia de
    /// coseno de pgvector (menor = más parecido), así que ordenar
    /// ascendente ya da el orden de relevancia correcto. Solo entran
    /// en el ranking los conceptos con embedding calculado — por eso
    /// este camino nunca va solo: `search` antepone SIEMPRE las
    /// coincidencias exactas de texto, que no dependen del índice.
    async fn search_semantic(
        &self,
        query: &SearchQuery,
        embedding: &[f32],
        limit: i64,
    ) -> Result<Vec<PgRow>, StoreError> {
        let vector = Vector::from(embedding.to_vec());
        pg_query(
            "SELECT h.concept_id, h.content_id, h.doc_type, h.title, h.tags
             FROM heads h
             JOIN blobs b ON h.content_id = b.content_id
             JOIN embeddings e ON e.concept_id = h.concept_id
             WHERE h.deleted_at IS NULL
               AND ($1::text IS NULL OR h.doc_type = $1)
               AND ($2::text IS NULL OR $2 = ANY(h.tags))
               AND ($3::text IS NULL OR h.concept_id = $3 OR h.concept_id LIKE $3 || '/%')
             ORDER BY e.embedding <=> $4
             LIMIT $5",
        )
        .bind(query.doc_type.as_deref())
        .bind(query.tag.as_deref())
        .bind(query.path_prefix.as_deref())
        .bind(vector)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| StoreError::Backend(e.to_string()))
    }

    /// Embedding inline tras un commit, best-effort: si Gemini no
    /// responde, el documento queda igualmente commiteado y visible
    /// para la búsqueda por texto; el evento del outbox lo reparará.
    fn embed_inline(&self, concept_id: &str, content_id_hex: &str, markdown: &str) {
        let Some(key) = self.gemini_api_key.as_deref() else { return };
        let client = reqwest::Client::new();
        let res = block_on(index_embedding(
            &self.pool,
            &client,
            key,
            concept_id,
            content_id_hex,
            markdown,
        ));
        if let Err(e) = res {
            eprintln!(
                "embed inline falló para {concept_id} ({e}); el outbox lo reparará en su próximo ciclo"
            );
        }
    }
}
