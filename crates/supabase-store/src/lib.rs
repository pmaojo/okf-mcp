#![forbid(unsafe_code)]

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
fn pg_query(sql: &str) -> Query<'_, Postgres, PgArguments> {
    sqlx::query(sql).persistent(false)
}

/// Igual que [`pg_query`], para consultas de una sola columna.
fn pg_query_scalar<'q, O>(sql: &'q str) -> QueryScalar<'q, Postgres, O, PgArguments>
where
    (O,): for<'r> FromRow<'r, PgRow>,
{
    sqlx::query_scalar(sql).persistent(false)
}

/// Ejecuta un Future de forma síncrona, tolerando si ya nos encontramos
/// dentro de un runtime de Tokio (como en Axum/Vercel) o fuera de él (tests).
fn block_on<F: std::future::Future>(fut: F) -> F::Output {
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

/// Misma forma de fila para `search_keyword` y `search_semantic`:
/// ambas seleccionan exactamente las mismas columnas de `heads`.
fn row_to_search_hit(row: PgRow) -> SearchHit {
    let content_id_hex: String = row.get("content_id");
    let content_id = ContentId::from_hex(&content_id_hex).expect("hash de db válido");
    let concept_id_str: String = row.get("concept_id");
    let concept_id = ConceptId::parse(&concept_id_str).expect("concept_id de db válido");
    let doc_type: String = row.get("doc_type");
    let title: Option<String> = row.get("title");
    let tags: Vec<String> = row.get("tags");

    SearchHit {
        concept_id,
        content_id,
        doc_type,
        title,
        tags,
    }
}

/// Combina resultados de texto y semánticos: las coincidencias
/// exactas (ya presentes) van primero; detrás, el ranking semántico
/// añade lo que el texto no encontró, sin duplicar conceptos. Corta
/// en `limit`.
fn merge_hybrid(keyword: Vec<PgRow>, semantic: Vec<PgRow>, limit: usize) -> Vec<PgRow> {
    let mut seen: std::collections::BTreeSet<String> =
        keyword.iter().map(|r| r.get::<String, _>("concept_id")).collect();
    let mut out = keyword;
    for row in semantic {
        if out.len() >= limit {
            break;
        }
        let cid: String = row.get("concept_id");
        if seen.insert(cid) {
            out.push(row);
        }
    }
    out.truncate(limit);
    out
}

/// El cuerpo real de `commit`, sobre una transacción ya abierta y SIN
/// decidir su destino (eso lo hace [`finish_tx`]). Extraído para que
/// [`SupabaseStore::commit_bulk`] en modo atómico pueda encadenar
/// varios commits en la MISMA transacción.
async fn commit_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    request: CommitRequest,
    actor: &Principal,
    budget: &Budget,
) -> Result<CommitOutcome, StoreError> {
    // 1. Validar el formato OKF del documento antes de tocar la base.
    let doc = okf_core::parse_document(&request.markdown, budget)?;

    let incoming_hash = sha256(request.markdown.as_bytes());
    let incoming = ContentId(incoming_hash);
    let incoming_hex = incoming.to_hex();

    // Bloquear la fila (exista o no, viva o borrada) para evitar
    // escrituras concurrentes; `is_deleted` se calcula en SQL para no
    // depender de ningún tipo de fecha en el lado de Rust.
    let current_head = pg_query(
        "SELECT content_id, version, (deleted_at IS NOT NULL) AS is_deleted
         FROM heads WHERE concept_id = $1 FOR UPDATE",
    )
    .bind(request.concept_id.as_str())
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| StoreError::Backend(e.to_string()))?;

    let row_exists = current_head.is_some();
    let is_deleted = current_head.as_ref().is_some_and(|h| h.get::<bool, _>("is_deleted"));
    let current_version: i64 = current_head.as_ref().map(|h| h.get("version")).unwrap_or(0);
    // La cabeza VIVA es la que cuenta para el CAS: una borrada es
    // "no existe" para quien escribe (recrear parte de expected=None),
    // pero la fila física y su versión siguen ahí.
    let live_content_id = if is_deleted {
        None
    } else {
        current_head.as_ref().map(|h| {
            let hex: String = h.get("content_id");
            ContentId::from_hex(&hex).expect("hash de db válido")
        })
    };

    let decision = decide(live_content_id, request.expected, incoming);

    match decision {
        CommitDecision::Conflict(c) => Err(StoreError::Conflict(c)),
        CommitDecision::NoChange => Ok(CommitOutcome {
            revision: None,
            content_id: live_content_id.expect("NoChange implica cabeza viva"),
            version: current_version as u64,
            created: false,
            no_change: true,
        }),
        CommitDecision::Create | CommitDecision::Update => {
            let created = matches!(decision, CommitDecision::Create);
            let new_version = current_version + 1;

            pg_query(
                "INSERT INTO blobs (content_id, raw) VALUES ($1, $2) ON CONFLICT (content_id) DO NOTHING",
            )
            .bind(&incoming_hex)
            .bind(&request.markdown)
            .execute(&mut **tx)
            .await
            .map_err(|e| StoreError::Backend(e.to_string()))?;

            // La fila FÍSICA decide INSERT vs UPDATE (recrear un
            // borrado es un UPDATE que limpia `deleted_at`); `created`
            // (para el cliente) es una cosa lógica distinta.
            if row_exists {
                pg_query(
                    "UPDATE heads
                     SET content_id = $1, version = $2, doc_type = $3, title = $4, tags = $5,
                         deleted_at = NULL
                     WHERE concept_id = $6",
                )
                .bind(&incoming_hex)
                .bind(new_version)
                .bind(&doc.doc_type)
                .bind(&doc.title)
                .bind(&doc.tags)
                .bind(request.concept_id.as_str())
                .execute(&mut **tx)
                .await
                .map_err(|e| StoreError::Backend(e.to_string()))?;
            } else {
                pg_query(
                    "INSERT INTO heads (concept_id, content_id, version, doc_type, title, tags)
                     VALUES ($1, $2, $3, $4, $5, $6)",
                )
                .bind(request.concept_id.as_str())
                .bind(&incoming_hex)
                .bind(new_version)
                .bind(&doc.doc_type)
                .bind(&doc.title)
                .bind(&doc.tags)
                .execute(&mut **tx)
                .await
                .map_err(|e| StoreError::Backend(e.to_string()))?;
            }

            let seq = pg_query_scalar::<i64>(
                "INSERT INTO revisions (concept_id, base, result, actor_subject, actor_client_id, reason)
                 VALUES ($1, $2, $3, $4, $5, $6)
                 RETURNING seq",
            )
            .bind(request.concept_id.as_str())
            .bind(live_content_id.map(|c| c.to_hex()))
            .bind(&incoming_hex)
            .bind(&actor.subject)
            .bind(&actor.client_id)
            .bind(&request.reason)
            .fetch_one(&mut **tx)
            .await
            .map_err(|e| StoreError::Backend(e.to_string()))?;

            pg_query("DELETE FROM links WHERE source_id = $1")
                .bind(request.concept_id.as_str())
                .execute(&mut **tx)
                .await
                .map_err(|e| StoreError::Backend(e.to_string()))?;

            for link in &doc.links {
                pg_query(
                    "INSERT INTO links (source_id, target_id, rel) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
                )
                .bind(request.concept_id.as_str())
                .bind(link.target.as_str())
                .bind(link.rel.as_deref())
                .execute(&mut **tx)
                .await
                .map_err(|e| StoreError::Backend(e.to_string()))?;
            }

            let payload = serde_json::json!({
                "concept_id": request.concept_id.as_str(),
                "content_id": incoming_hex,
                "markdown": request.markdown,
                "reason": request.reason,
                "actor": {
                    "subject": actor.subject,
                    "client_id": actor.client_id
                }
            });

            pg_query(
                "INSERT INTO outbox (event_type, concept_id, content_id, payload)
                 VALUES ('commit', $1, $2, $3)",
            )
            .bind(request.concept_id.as_str())
            .bind(&incoming_hex)
            .bind(payload)
            .execute(&mut **tx)
            .await
            .map_err(|e| StoreError::Backend(e.to_string()))?;

            let revision = Revision {
                seq: seq as u64,
                concept_id: request.concept_id.clone(),
                base: live_content_id,
                result: incoming,
                actor: actor.clone(),
                reason: request.reason,
            };

            Ok(CommitOutcome {
                revision: Some(revision),
                content_id: incoming,
                version: new_version as u64,
                created,
                no_change: false,
            })
        }
    }
}

/// Borra lógicamente sobre una transacción ya abierta, con el mismo
/// contrato CAS que un commit: `expected` obligatorio.
async fn delete_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    id: &ConceptId,
    expected: ContentId,
    actor: &Principal,
    reason: String,
) -> Result<DeleteOutcome, StoreError> {
    let head = pg_query(
        "SELECT content_id, version FROM heads
         WHERE concept_id = $1 AND deleted_at IS NULL FOR UPDATE",
    )
    .bind(id.as_str())
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| StoreError::Backend(e.to_string()))?;

    let Some(head) = head else {
        return Err(StoreError::NotFound(id.clone()));
    };
    let content_hex: String = head.get("content_id");
    let content_id = ContentId::from_hex(&content_hex).expect("hash de db válido");
    let version: i64 = head.get("version");

    if content_id != expected {
        return Err(StoreError::Conflict(Conflict {
            expected: Some(expected),
            current: Some(content_id),
            incoming: expected,
        }));
    }

    pg_query("UPDATE heads SET deleted_at = CURRENT_TIMESTAMP WHERE concept_id = $1")
        .bind(id.as_str())
        .execute(&mut **tx)
        .await
        .map_err(|e| StoreError::Backend(e.to_string()))?;

    // Un borrado deja de enlazar: mismo motivo que en memoria.
    pg_query("DELETE FROM links WHERE source_id = $1")
        .bind(id.as_str())
        .execute(&mut **tx)
        .await
        .map_err(|e| StoreError::Backend(e.to_string()))?;

    let seq = pg_query_scalar::<i64>(
        "INSERT INTO revisions (concept_id, base, result, actor_subject, actor_client_id, reason)
         VALUES ($1, $2, $3, $4, $5, $6)
         RETURNING seq",
    )
    .bind(id.as_str())
    .bind(&content_hex)
    .bind(&content_hex)
    .bind(&actor.subject)
    .bind(&actor.client_id)
    .bind(&reason)
    .fetch_one(&mut **tx)
    .await
    .map_err(|e| StoreError::Backend(e.to_string()))?;

    let payload = serde_json::json!({
        "concept_id": id.as_str(),
        "reason": reason,
        "actor": {
            "subject": actor.subject,
            "client_id": actor.client_id
        }
    });

    pg_query(
        "INSERT INTO outbox (event_type, concept_id, content_id, payload)
         VALUES ('delete', $1, $2, $3)",
    )
    .bind(id.as_str())
    .bind(&content_hex)
    .bind(payload)
    .execute(&mut **tx)
    .await
    .map_err(|e| StoreError::Backend(e.to_string()))?;

    let revision = Revision {
        seq: seq as u64,
        concept_id: id.clone(),
        base: Some(content_id),
        result: content_id,
        actor: actor.clone(),
        reason,
    };

    Ok(DeleteOutcome { content_id, version: version as u64, revision })
}

/// Cierra una transacción según el resultado de lo que se hizo dentro:
/// `commit` en éxito, `rollback` en fallo. El rollback es best-effort
/// (si falla, manda el error original: la conexión se corta igual al
/// dropear `tx`).
async fn finish_tx<T>(
    tx: Transaction<'_, Postgres>,
    res: Result<T, StoreError>,
) -> Result<T, StoreError> {
    match res {
        Ok(v) => {
            tx.commit().await.map_err(|e| StoreError::Backend(e.to_string()))?;
            Ok(v)
        }
        Err(e) => {
            let _ = tx.rollback().await;
            Err(e)
        }
    }
}

impl MemoryRepository for SupabaseStore {
    fn get(&self, id: &ConceptId) -> Result<Option<DocumentView>, StoreError> {
        let res = block_on(async {
            pg_query(
                "SELECT h.content_id, h.version, b.raw, h.doc_type, h.title, h.tags
                 FROM heads h
                 JOIN blobs b ON h.content_id = b.content_id
                 WHERE h.concept_id = $1 AND h.deleted_at IS NULL",
            )
            .bind(id.as_str())
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| StoreError::Backend(e.to_string()))
        })?;

        match res {
            None => Ok(None),
            Some(row) => {
                let content_id_hex: String = row.get("content_id");
                let content_id = ContentId::from_hex(&content_id_hex)
                    .ok_or_else(|| StoreError::Backend("hash de contenido corrupto en base de datos".to_string()))?;
                
                let raw: String = row.get("raw");
                // Analizar el documento para extraer enlaces (no persistidos directamente en la tabla heads/blobs)
                let doc = okf_core::parse_document(&raw, &Budget::default())?;

                let version: i64 = row.get("version");
                let doc_type: String = row.get("doc_type");
                let title: Option<String> = row.get("title");
                let tags: Vec<String> = row.get("tags");

                Ok(Some(DocumentView {
                    concept_id: id.clone(),
                    content_id,
                    version: version as u64,
                    raw: Arc::from(raw),
                    doc_type,
                    title,
                    tags,
                    links: doc.links,
                }))
            }
        }
    }

    /// Búsqueda HÍBRIDA. Las coincidencias exactas de texto (`ILIKE`
    /// sobre id, título, tags y cuerpo) van SIEMPRE primero: no
    /// dependen del índice semántico, así que un documento recién
    /// commiteado es encontrable en el mismo segundo. Detrás, si hay
    /// `GEMINI_API_KEY`, el ranking semántico de pgvector añade los
    /// documentos próximos en significado que el texto literal no
    /// capturó. Si Gemini falla, la parte exacta sobrevive: un
    /// problema transitorio del proveedor degrada la búsqueda, no la
    /// tumba.
    fn search(&self, query: &SearchQuery, budget: &Budget) -> Result<Vec<SearchHit>, StoreError> {
        let limit = query
            .limit
            .unwrap_or(budget.max_search_results)
            .min(budget.max_search_results) as i64;

        let rows = block_on(async {
            let keyword = self.search_keyword(query, limit).await?;

            if let (Some(text), Some(gemini_key)) =
                (query.text.as_deref(), self.gemini_api_key.as_deref())
            {
                let client = reqwest::Client::new();
                match embed(&client, gemini_key, text).await {
                    Ok(embedding) => {
                        let semantic = self.search_semantic(query, &embedding, limit).await?;
                        return Ok::<_, StoreError>(merge_hybrid(keyword, semantic, limit as usize));
                    }
                    Err(e) => eprintln!(
                        "ranking semántico falló ({e}); la búsqueda sigue solo con coincidencia de texto"
                    ),
                }
            }
            Ok(keyword)
        })?;

        Ok(rows.into_iter().map(row_to_search_hit).collect())
    }

    fn commit(
        &mut self,
        request: CommitRequest,
        actor: &Principal,
        budget: &Budget,
    ) -> Result<CommitOutcome, StoreError> {
        // Datos para el embed inline ANTES de ceder request al helper.
        let concept = request.concept_id.as_str().to_string();
        let markdown = request.markdown.clone();

        let outcome = block_on(async {
            let mut tx =
                self.pool.begin().await.map_err(|e| StoreError::Backend(e.to_string()))?;
            let res = commit_in_tx(&mut tx, request, actor, budget).await;
            finish_tx(tx, res).await
        })?;

        // Indexación semántica inline, best-effort y FUERA de la
        // transacción: no retiene el FOR UPDATE ni una conexión del
        // pooler durante una llamada HTTP, y su fallo no deshace el
        // commit — el evento del outbox ya quedó registrado y el cron
        // lo reparará.
        if outcome.revision.is_some() {
            self.embed_inline(&concept, &outcome.content_id.to_hex(), &markdown);
        }
        Ok(outcome)
    }

    fn history(
        &self,
        id: &ConceptId,
        limit: usize,
        before_seq: Option<u64>,
    ) -> Result<Vec<Revision>, StoreError> {
        let before = before_seq.unwrap_or(i64::MAX as u64) as i64;
        let limit = limit as i64;

        let res = block_on(async {
            // Verificar si el concepto existe en heads o revisiones
            let exists = pg_query_scalar::<bool>(
                "SELECT EXISTS(SELECT 1 FROM heads WHERE concept_id = $1)
                 OR EXISTS(SELECT 1 FROM revisions WHERE concept_id = $1)",
            )
            .bind(id.as_str())
            .fetch_one(&self.pool)
            .await
            .map_err(|e| StoreError::Backend(e.to_string()))?;

            if !exists {
                return Err(StoreError::NotFound(id.clone()));
            }

            let rows = pg_query(
                "SELECT seq, base, result, actor_subject, actor_client_id, reason
                 FROM revisions
                 WHERE concept_id = $1 AND seq < $2
                 ORDER BY seq DESC
                 LIMIT $3",
            )
            .bind(id.as_str())
            .bind(before)
            .bind(limit)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| StoreError::Backend(e.to_string()))?;

            let mut revisions = Vec::new();
            for r in rows {
                let seq: i64 = r.get("seq");
                let base_hex: Option<String> = r.get("base");
                let base = base_hex.map(|b| ContentId::from_hex(&b).expect("hash de db válido"));
                let result_hex: String = r.get("result");
                let result = ContentId::from_hex(&result_hex).expect("hash de db válido");
                let actor_subject: String = r.get("actor_subject");
                let actor_client_id: String = r.get("actor_client_id");
                let reason: String = r.get("reason");

                revisions.push(Revision {
                    seq: seq as u64,
                    concept_id: id.clone(),
                    base,
                    result,
                    actor: Principal {
                        subject: actor_subject,
                        client_id: actor_client_id,
                    },
                    reason,
                });
            }
            Ok(revisions)
        })?;

        Ok(res)
    }

    fn delete(
        &mut self,
        id: &ConceptId,
        expected: ContentId,
        actor: &Principal,
        reason: String,
    ) -> Result<DeleteOutcome, StoreError> {
        block_on(async {
            let mut tx = self.pool.begin().await.map_err(|e| StoreError::Backend(e.to_string()))?;
            let res = delete_in_tx(&mut tx, id, expected, actor, reason).await;
            finish_tx(tx, res).await
        })
    }

    fn backlinks(&self, id: &ConceptId) -> Result<Vec<Backlink>, StoreError> {
        let res = block_on(async {
            let rows = pg_query(
                "SELECT h.concept_id, h.content_id, h.doc_type, h.title, h.tags, l.rel
                 FROM links l
                 JOIN heads h ON l.source_id = h.concept_id
                 WHERE l.target_id = $1 AND h.deleted_at IS NULL
                 ORDER BY h.concept_id",
            )
            .bind(id.as_str())
            .fetch_all(&self.pool)
            .await
            .map_err(|e| StoreError::Backend(e.to_string()))?;

            let mut backlinks = Vec::new();
            for row in rows {
                let rel: Option<String> = row.get("rel");
                let source = row_to_search_hit(row);
                backlinks.push(Backlink { source, rel });
            }
            Ok::<_, StoreError>(backlinks)
        })?;
        Ok(res)
    }

    fn commit_bulk(
        &mut self,
        requests: Vec<CommitRequest>,
        atomic: bool,
        actor: &Principal,
        budget: &Budget,
    ) -> Result<BulkOutcome, StoreError> {
        if !atomic {
            let mut items = Vec::with_capacity(requests.len());
            for req in requests {
                match self.commit(req, actor, budget) {
                    Ok(outcome) => items.push(BulkItem::Done(outcome)),
                    Err(e) => items.push(BulkItem::Failed(e)),
                }
            }
            return Ok(BulkOutcome { applied: true, items });
        }

        let res = block_on(async {
            let mut tx = self.pool.begin().await.map_err(|e| StoreError::Backend(e.to_string()))?;
            let mut items = Vec::with_capacity(requests.len());
            let mut failed_at = None;

            for (idx, req) in requests.clone().into_iter().enumerate() {
                match commit_in_tx(&mut tx, req, actor, budget).await {
                    Ok(outcome) => items.push(BulkItem::Done(outcome)),
                    Err(e) => {
                        failed_at = Some(idx);
                        items.push(BulkItem::Failed(e));
                        break;
                    }
                }
            }

            if failed_at.is_some() {
                let _ = tx.rollback().await;
                for item in items.iter_mut() {
                    if !matches!(item, BulkItem::Failed(_)) {
                        *item = BulkItem::Skipped;
                    }
                }
                while items.len() < requests.len() {
                    items.push(BulkItem::Skipped);
                }
                Ok::<_, StoreError>(BulkOutcome { applied: false, items })
            } else {
                tx.commit().await.map_err(|e| StoreError::Backend(e.to_string()))?;
                for (req, item) in requests.iter().zip(items.iter()) {
                    if let BulkItem::Done(outcome) = item {
                        if outcome.revision.is_some() {
                            self.embed_inline(req.concept_id.as_str(), &outcome.content_id.to_hex(), &req.markdown);
                        }
                    }
                }
                Ok::<_, StoreError>(BulkOutcome { applied: true, items })
            }
        })?;

        Ok(res)
    }
}

impl StoreMaintenance for SupabaseStore {
    fn link_health(&self, id: &ConceptId) -> Result<LinkHealth, StoreError> {
        let exists = block_on(async {
            pg_query_scalar::<bool>(
                "SELECT EXISTS(SELECT 1 FROM heads WHERE concept_id = $1 AND deleted_at IS NULL)",
            )
            .bind(id.as_str())
            .fetch_one(&self.pool)
            .await
            .map_err(|e| StoreError::Backend(e.to_string()))
        })?;

        if !exists {
            return Err(StoreError::NotFound(id.clone()));
        }

        let rows = block_on(async {
            pg_query(
                "SELECT l.target_id, l.rel, (h.concept_id IS NOT NULL) as exists, (h.deleted_at IS NOT NULL) as is_deleted
                 FROM links l
                 LEFT JOIN heads h ON l.target_id = h.concept_id
                 WHERE l.source_id = $1
                 ORDER BY l.target_id",
            )
            .bind(id.as_str())
            .fetch_all(&self.pool)
            .await
            .map_err(|e| StoreError::Backend(e.to_string()))
        })?;

        let mut health = LinkHealth::default();
        for row in rows {
            let target_str: String = row.get("target_id");
            let target = ConceptId::parse(&target_str).map_err(|e| StoreError::Backend(e.to_string()))?;
            let rel: Option<String> = row.get("rel");
            let link = Link { target, rel };
            
            let exists: bool = row.get("exists");
            let is_deleted: bool = row.get("is_deleted");
            if !exists {
                health.broken.push(link);
            } else if is_deleted {
                health.deleted.push(link);
            } else {
                health.ok.push(link);
            }
        }
        Ok(health)
    }

    fn validate(
        &self,
        path_prefix: Option<&str>,
        budget: &Budget,
    ) -> Result<ValidationReport, StoreError> {
        let cap = budget.max_search_results;

        let rows = block_on(async {
            pg_query(
                "SELECT l.source_id, l.target_id, l.rel, (h.concept_id IS NOT NULL) as exists, (h.deleted_at IS NOT NULL) as is_deleted
                 FROM links l
                 JOIN heads sh ON l.source_id = sh.concept_id
                 LEFT JOIN heads h ON l.target_id = h.concept_id
                 WHERE sh.deleted_at IS NULL
                   AND ($1::text IS NULL OR l.source_id = $1 OR l.source_id LIKE $1 || '/%')
                 ORDER BY l.source_id, l.target_id",
            )
            .bind(path_prefix)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| StoreError::Backend(e.to_string()))
        })?;

        let mut report = ValidationReport::default();
        for row in rows {
            let source_str: String = row.get("source_id");
            let target_str: String = row.get("target_id");
            let source = ConceptId::parse(&source_str).map_err(|e| StoreError::Backend(e.to_string()))?;
            let target = ConceptId::parse(&target_str).map_err(|e| StoreError::Backend(e.to_string()))?;

            let exists: bool = row.get("exists");
            let is_deleted: bool = row.get("is_deleted");

            if !exists {
                report.broken_links_total += 1;
                if report.broken_links.len() < cap {
                    report.broken_links.push((source.clone(), target.clone()));
                }
            } else if is_deleted {
                report.deleted_referenced_total += 1;
                if report.deleted_referenced.len() < cap {
                    report.deleted_referenced.push((source.clone(), target.clone()));
                }
            }
        }

        if self.gemini_api_key.is_some() {
            let emb_rows = block_on(async {
                pg_query(
                    "SELECT h.concept_id
                     FROM heads h
                     LEFT JOIN embeddings e ON h.concept_id = e.concept_id
                     WHERE h.deleted_at IS NULL
                       AND ($1::text IS NULL OR h.concept_id = $1 OR h.concept_id LIKE $1 || '/%')
                       AND (e.concept_id IS NULL OR e.content_id IS NULL OR e.content_id <> h.content_id)
                     ORDER BY h.concept_id",
                )
                .bind(path_prefix)
                .fetch_all(&self.pool)
                .await
                .map_err(|e| StoreError::Backend(e.to_string()))
            })?;

            report.missing_embeddings_total = emb_rows.len();
            for row in emb_rows {
                if report.missing_embeddings.len() >= cap {
                    break;
                }
                let concept_str: String = row.get("concept_id");
                let concept = ConceptId::parse(&concept_str).map_err(|e| StoreError::Backend(e.to_string()))?;
                report.missing_embeddings.push(concept);
            }
        }

        Ok(report)
    }

    fn stats(&self, budget: &Budget) -> Result<GraphStats, StoreError> {
        let cap = budget.max_search_results;

        let (docs, del_docs) = block_on(async {
            let docs = pg_query_scalar::<i64>("SELECT COUNT(*) FROM heads WHERE deleted_at IS NULL")
                .fetch_one(&self.pool)
                .await;
            let del_docs = pg_query_scalar::<i64>("SELECT COUNT(*) FROM heads WHERE deleted_at IS NOT NULL")
                .fetch_one(&self.pool)
                .await;
            let docs = docs.map_err(|e| StoreError::Backend(e.to_string()))?;
            let del_docs = del_docs.map_err(|e| StoreError::Backend(e.to_string()))?;
            Ok::<_, StoreError>((docs, del_docs))
        })?;

        let type_rows = block_on(async {
            pg_query(
                "SELECT doc_type, COUNT(*) as cnt
                 FROM heads
                 WHERE deleted_at IS NULL
                 GROUP BY doc_type
                 ORDER BY cnt DESC, doc_type
                 LIMIT $1",
            )
            .bind(cap as i64)
            .fetch_all(&self.pool)
            .await
        }).map_err(|e| StoreError::Backend(e.to_string()))?;

        let tag_rows = block_on(async {
            pg_query(
                "SELECT unnest(tags) as tag, COUNT(*) as cnt
                 FROM heads
                 WHERE deleted_at IS NULL
                 GROUP BY tag
                 ORDER BY cnt DESC, tag
                 LIMIT $1",
            )
            .bind(cap as i64)
            .fetch_all(&self.pool)
            .await
        }).map_err(|e| StoreError::Backend(e.to_string()))?;

        let top_linked_rows = block_on(async {
            pg_query(
                "SELECT target_id, COUNT(*) as cnt
                 FROM links l
                 JOIN heads h ON l.source_id = h.concept_id
                 WHERE h.deleted_at IS NULL
                 GROUP BY target_id
                 ORDER BY cnt DESC, target_id
                 LIMIT $1",
            )
            .bind(cap as i64)
            .fetch_all(&self.pool)
            .await
        }).map_err(|e| StoreError::Backend(e.to_string()))?;

        let orphan_rows = block_on(async {
            pg_query(
                "SELECT concept_id FROM heads h
                 WHERE deleted_at IS NULL
                   AND NOT EXISTS (SELECT 1 FROM links WHERE source_id = h.concept_id)
                   AND NOT EXISTS (SELECT 1 FROM links WHERE target_id = h.concept_id)
                 ORDER BY concept_id
                 LIMIT $1",
            )
            .bind(cap as i64)
            .fetch_all(&self.pool)
            .await
        }).map_err(|e| StoreError::Backend(e.to_string()))?;

        let by_type = type_rows.into_iter().map(|r| (r.get("doc_type"), r.get::<i64, _>("cnt") as usize)).collect();
        let by_tag = tag_rows.into_iter().map(|r| (r.get("tag"), r.get::<i64, _>("cnt") as usize)).collect();
        let top_linked = top_linked_rows.into_iter().filter_map(|r| {
            let target_str: String = r.get("target_id");
            let target = ConceptId::parse(&target_str).ok()?;
            Some((target, r.get::<i64, _>("cnt") as usize))
        }).collect();
        let orphans = orphan_rows.into_iter().filter_map(|r| {
            let concept_str: String = r.get("concept_id");
            ConceptId::parse(&concept_str).ok()
        }).collect();

        Ok(GraphStats {
            documents: docs as usize,
            deleted_documents: del_docs as usize,
            by_type,
            by_tag,
            top_linked,
            orphans,
        })
    }

    fn status(&self) -> Result<StoreStatus, StoreError> {
        let row = block_on(async {
            pg_query(
                "SELECT
                   (SELECT COUNT(*) FROM heads WHERE deleted_at IS NULL) as docs,
                   (SELECT COUNT(*) FROM heads WHERE deleted_at IS NOT NULL) as deleted_docs,
                   (SELECT COUNT(*) FROM heads h LEFT JOIN embeddings e ON h.concept_id = e.concept_id WHERE h.deleted_at IS NULL AND (e.concept_id IS NULL OR e.content_id IS NULL OR e.content_id <> h.content_id)) as missing_embs,
                   (SELECT COUNT(*) FROM links l JOIN heads sh ON l.source_id = sh.concept_id LEFT JOIN heads h ON l.target_id = h.concept_id WHERE sh.deleted_at IS NULL AND h.concept_id IS NULL) as broken,
                   (SELECT COUNT(*) FROM links l JOIN heads sh ON l.source_id = sh.concept_id JOIN heads h ON l.target_id = h.concept_id WHERE sh.deleted_at IS NULL AND h.deleted_at IS NOT NULL) as del_ref,
                   (SELECT COUNT(*) FROM outbox WHERE status = 'pending') as pending_outbox,
                   (SELECT COUNT(*) FROM outbox WHERE status = 'failed') as failed_outbox"
            )
            .fetch_one(&self.pool)
            .await
        }).map_err(|e| StoreError::Backend(e.to_string()))?;

        Ok(StoreStatus {
            documents: row.get::<i64, _>("docs") as usize,
            deleted_documents: row.get::<i64, _>("deleted_docs") as usize,
            missing_embeddings: row.get::<i64, _>("missing_embs") as usize,
            broken_links: row.get::<i64, _>("broken") as usize,
            deleted_referenced: row.get::<i64, _>("del_ref") as usize,
            outbox_pending: row.get::<i64, _>("pending_outbox") as usize,
            outbox_failed: row.get::<i64, _>("failed_outbox") as usize,
        })
    }

    fn embed_pending(
        &mut self,
        path_prefix: Option<&str>,
        max: usize,
    ) -> Result<EmbedOutcome, StoreError> {
        let gemini_key = match &self.gemini_api_key {
            Some(k) => k.clone(),
            None => return Ok(EmbedOutcome::default()),
        };

        let rows = block_on(async {
            pg_query(
                "SELECT h.concept_id, b.raw, h.content_id
                 FROM heads h
                 JOIN blobs b ON h.content_id = b.content_id
                 LEFT JOIN embeddings e ON h.concept_id = e.concept_id
                 WHERE h.deleted_at IS NULL
                   AND ($1::text IS NULL OR h.concept_id = $1 OR h.concept_id LIKE $1 || '/%')
                   AND (e.concept_id IS NULL OR e.content_id IS NULL OR e.content_id <> h.content_id)
                 ORDER BY h.concept_id
                 LIMIT $2",
            )
            .bind(path_prefix)
            .bind(max as i64)
            .fetch_all(&self.pool)
            .await
        }).map_err(|e| StoreError::Backend(e.to_string()))?;

        let mut outcome = EmbedOutcome::default();
        let client = reqwest::Client::new();

        for row in rows {
            let concept_str: String = row.get("concept_id");
            let concept = ConceptId::parse(&concept_str).map_err(|e| StoreError::Backend(e.to_string()))?;
            let raw: String = row.get("raw");
            let content_hex: String = row.get("content_id");

            let res = block_on(index_embedding(&self.pool, &client, &gemini_key, &concept_str, &content_hex, &raw));
            match res {
                Ok(_) => outcome.embedded.push(concept),
                Err(e) => outcome.failed.push((concept, e.to_string())),
            }
        }

        let remaining = block_on(async {
            pg_query_scalar::<i64>(
                "SELECT COUNT(*)
                 FROM heads h
                 LEFT JOIN embeddings e ON h.concept_id = e.concept_id
                 WHERE h.deleted_at IS NULL
                   AND ($1::text IS NULL OR h.concept_id = $1 OR h.concept_id LIKE $1 || '/%')
                   AND (e.concept_id IS NULL OR e.content_id IS NULL OR e.content_id <> h.content_id)",
            )
            .bind(path_prefix)
            .fetch_one(&self.pool)
            .await
        }).map_err(|e| StoreError::Backend(e.to_string()))?;

        outcome.remaining = remaining as usize;
        Ok(outcome)
    }
}

impl NeighborSource for SupabaseStore {
    type Error = Infallible;

    fn neighbors(&self, id: &ConceptId) -> Result<Vec<ConceptId>, Self::Error> {
        let res = block_on(async {
            pg_query_scalar::<String>(
                "SELECT target_id FROM links WHERE source_id = $1 ORDER BY target_id",
            )
            .bind(id.as_str())
            .fetch_all(&self.pool)
            .await
        });

        match res {
            Ok(rows) => {
                let parsed = rows
                    .into_iter()
                    .filter_map(|r| ConceptId::parse(&r).ok())
                    .collect();
                Ok(parsed)
            }
            Err(e) => {
                eprintln!("Error cargando vecinos del concepto {id} desde la base de datos: {e}");
                Ok(Vec::new())
            }
        }
    }

    fn document_size(&self, id: &ConceptId) -> Result<Option<usize>, Self::Error> {
        let res = block_on(async {
            pg_query_scalar::<String>(
                "SELECT b.raw
                 FROM heads h
                 JOIN blobs b ON h.content_id = b.content_id
                 WHERE h.concept_id = $1",
            )
            .bind(id.as_str())
            .fetch_optional(&self.pool)
            .await
        });

        match res {
            Ok(Some(raw)) => Ok(Some(raw.len())),
            Ok(None) => Ok(None),
            Err(e) => {
                eprintln!("Error cargando tamaño del documento {id} desde la base de datos: {e}");
                Ok(None)
            }
        }
    }
}
