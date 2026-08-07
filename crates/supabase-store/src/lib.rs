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
mod triples;

use gemini_embeddings::{embed_document, Embedded, EmbeddingKeys};
use pgvector::Vector;
use store_core::{SearchQuery, StoreError};
use sqlx::postgres::{PgArguments, PgRow};
use sqlx::query::{Query, QueryScalar};
use sqlx::{FromRow, PgPool, Postgres};

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
    /// Con al menos un proveedor configurado, `search` embebe el texto
    /// de la consulta y ordena por similitud semántica (`pgvector`) en
    /// vez de solo la coincidencia de subcadena (`ILIKE`) — ver
    /// [`Self::search`].
    embedding_keys: EmbeddingKeys,
}

/// Genera el embedding de `markdown` (con el primer proveedor
/// disponible de `keys`) y lo deja indexado en `pgvector`, junto con
/// de QUÉ contenido (`content_id`) y de QUÉ modelo (`embedding_model`)
/// es el vector: `status`/`embed_pending` usan `content_id` para
/// distinguir un embedding al día de uno obsoleto, y `search_semantic`
/// usa `embedding_model` para no comparar nunca vectores de
/// proveedores distintos (ver el comentario de módulo de
/// `gemini_embeddings`).
///
/// Es la ÚNICA puerta de escritura al índice semántico: la usan el
/// camino inline (tras cada commit, best-effort) y `outbox-worker`
/// (la reparación asíncrona). Dos escritores, una función — que no
/// puedan divergir ni en qué proveedor llamaron ni en qué modelo
/// etiquetaron.
pub async fn index_embedding(
    pool: &PgPool,
    client: &reqwest::Client,
    keys: &EmbeddingKeys,
    concept_id: &str,
    content_id_hex: &str,
    markdown: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let Embedded { vector, model_id } = embed_document(client, keys, markdown).await?;
    pg_query(
        "INSERT INTO embeddings (concept_id, embedding, content_id, embedding_model)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (concept_id) DO UPDATE
             SET embedding = EXCLUDED.embedding,
                 content_id = EXCLUDED.content_id,
                 embedding_model = EXCLUDED.embedding_model",
    )
    .bind(concept_id)
    .bind(Vector::from(vector))
    .bind(content_id_hex)
    .bind(model_id)
    .execute(pool)
    .await?;
    Ok(())
}

impl SupabaseStore {
    /// Crea una nueva instancia de `SupabaseStore` a partir de las
    /// variables de entorno:
    /// - `POSTGRES_URL` (obligatoria)
    /// - `GEMINI_API_KEY`, `MISTRAL_API_KEY`, `COHERE_API_KEY` (todas
    ///   opcionales; con al menos una, la búsqueda semántica se activa
    ///   — ver [`EmbeddingKeys::from_env`])
    pub fn from_env() -> Result<Self, StoreError> {
        let db_url = std::env::var("POSTGRES_URL")
            .map_err(|_| StoreError::Backend("falta POSTGRES_URL".into()))?;
        let embedding_keys = EmbeddingKeys::from_env();

        let pool = block_on(async {
            PgPool::connect(&db_url).await
        }).map_err(|e| StoreError::Backend(format!("Error conectando a Postgres: {e}")))?;

        Ok(Self::new(pool, embedding_keys))
    }

    pub fn new(pool: PgPool, embedding_keys: EmbeddingKeys) -> Self {
        SupabaseStore { pool, embedding_keys }
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
    ///
    /// Filtra SIEMPRE por `embedding_model = embedding.model_id`: con
    /// la columna a dimensión variable (varios proveedores posibles),
    /// comparar `<=>` contra un vector de otro modelo no da un error,
    /// da un ranking sin significado — un documento indexado con el
    /// proveedor de respaldo queda fuera de este ranking (sigue
    /// encontrable por texto) hasta que se re-indexe con el proveedor
    /// activo.
    async fn search_semantic(
        &self,
        query: &SearchQuery,
        embedding: &Embedded,
        limit: i64,
    ) -> Result<Vec<PgRow>, StoreError> {
        let vector = Vector::from(embedding.vector.clone());
        pg_query(
            "SELECT h.concept_id, h.content_id, h.doc_type, h.title, h.tags
             FROM heads h
             JOIN blobs b ON h.content_id = b.content_id
             JOIN embeddings e ON e.concept_id = h.concept_id
             WHERE h.deleted_at IS NULL
               AND e.embedding_model = $1
               AND ($2::text IS NULL OR h.doc_type = $2)
               AND ($3::text IS NULL OR $3 = ANY(h.tags))
               AND ($4::text IS NULL OR h.concept_id = $4 OR h.concept_id LIKE $4 || '/%')
             ORDER BY e.embedding <=> $5
             LIMIT $6",
        )
        .bind(embedding.model_id)
        .bind(query.doc_type.as_deref())
        .bind(query.tag.as_deref())
        .bind(query.path_prefix.as_deref())
        .bind(vector)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| StoreError::Backend(e.to_string()))
    }

    /// Embedding inline tras un commit, best-effort: si ningún
    /// proveedor responde, el documento queda igualmente commiteado y
    /// visible para la búsqueda por texto; el evento del outbox lo
    /// reparará.
    fn embed_inline(&self, concept_id: &str, content_id_hex: &str, markdown: &str) {
        if !self.embedding_keys.any_configured() {
            return;
        }
        let client = reqwest::Client::new();
        let res = block_on(index_embedding(
            &self.pool,
            &client,
            &self.embedding_keys,
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
