#![forbid(unsafe_code)]

use conflict_core::{decide, CommitDecision};
use gemini_embeddings::embed;
use pgvector::Vector;
use graph_core::NeighborSource;
use hash_core::sha256;
use memory_model::{Budget, ConceptId, ContentId, Principal, Revision};
use store_core::{
    CommitOutcome, CommitRequest, DocumentView, MemoryRepository, SearchHit, SearchQuery, StoreError,
    TagsMode,
};
use sqlx::postgres::{PgArguments, PgRow};
use sqlx::query::{Query, QueryScalar};
use sqlx::{FromRow, PgPool, Postgres, Row};
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

impl SupabaseStore {
    pub fn new(pool: PgPool, gemini_api_key: Option<String>) -> Self {
        SupabaseStore { pool, gemini_api_key }
    }

    /// Los filtros estructurados (`doc_type`, `status`, `path_prefix`,
    /// `tags`) viven en el WHERE de AMBAS variantes de búsqueda: son
    /// literales y se aplican siempre; si nada los cumple, la
    /// respuesta es vacía — nunca se degrada a candidatos sin filtro.
    /// Para `tags`, `&&` es solapamiento (modo any) y `@>` es
    /// contención (modo all); con lista vacía el guard de
    /// `cardinality` desactiva el filtro.
    async fn search_keyword(&self, query: &SearchQuery, limit: i64) -> Result<Vec<PgRow>, StoreError> {
        pg_query(
            "SELECT h.concept_id, h.content_id, h.doc_type, h.title, h.status, h.tags
             FROM heads h
             JOIN blobs b ON h.content_id = b.content_id
             WHERE ($1::text IS NULL OR h.doc_type = $1)
               AND ($2::text IS NULL OR h.status = $2)
               AND ($3::text IS NULL OR starts_with(h.concept_id, $3))
               AND (cardinality($4::text[]) = 0 OR
                    (CASE WHEN $5 THEN h.tags @> $4::text[] ELSE h.tags && $4::text[] END))
               AND ($6::text IS NULL OR (
                   h.concept_id ILIKE $7 OR
                   h.title ILIKE $7 OR
                   b.raw ILIKE $7 OR
                   EXISTS (SELECT 1 FROM unnest(h.tags) t WHERE t ILIKE $7)
               ))
             ORDER BY h.concept_id
             LIMIT $8",
        )
        .bind(query.doc_type.as_deref())
        .bind(query.status.as_deref())
        .bind(query.path_prefix.as_deref())
        .bind(&query.tags)
        .bind(query.tags_mode == TagsMode::All)
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
    /// en el ranking los conceptos que YA tienen embedding calculado
    /// (`JOIN embeddings`, no `LEFT JOIN`) — `outbox-worker` lo genera
    /// de forma asíncrona tras cada commit, así que un concepto recién
    /// escrito puede tardar hasta el próximo ciclo del outbox en
    /// aparecer en una búsqueda semántica (sí aparece de inmediato en
    /// la búsqueda por palabra clave, que no depende del outbox).
    async fn search_semantic(
        &self,
        query: &SearchQuery,
        embedding: &[f32],
        limit: i64,
    ) -> Result<Vec<PgRow>, StoreError> {
        let vector = Vector::from(embedding.to_vec());
        pg_query(
            "SELECT h.concept_id, h.content_id, h.doc_type, h.title, h.status, h.tags
             FROM heads h
             JOIN blobs b ON h.content_id = b.content_id
             JOIN embeddings e ON e.concept_id = h.concept_id
             WHERE ($1::text IS NULL OR h.doc_type = $1)
               AND ($2::text IS NULL OR h.status = $2)
               AND ($3::text IS NULL OR starts_with(h.concept_id, $3))
               AND (cardinality($4::text[]) = 0 OR
                    (CASE WHEN $5 THEN h.tags @> $4::text[] ELSE h.tags && $4::text[] END))
             ORDER BY e.embedding <=> $6
             LIMIT $7",
        )
        .bind(query.doc_type.as_deref())
        .bind(query.status.as_deref())
        .bind(query.path_prefix.as_deref())
        .bind(&query.tags)
        .bind(query.tags_mode == TagsMode::All)
        .bind(vector)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| StoreError::Backend(e.to_string()))
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
    let status: Option<String> = row.get("status");
    let tags: Vec<String> = row.get("tags");

    SearchHit {
        concept_id,
        content_id,
        doc_type,
        title,
        status,
        tags,
    }
}

impl MemoryRepository for SupabaseStore {
    fn get(&self, id: &ConceptId) -> Result<Option<DocumentView>, StoreError> {
        let res = block_on(async {
            pg_query(
                "SELECT h.content_id, h.version, b.raw, h.doc_type, h.title, h.status, h.tags
                 FROM heads h
                 JOIN blobs b ON h.content_id = b.content_id
                 WHERE h.concept_id = $1",
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
                let status: Option<String> = row.get("status");
                let tags: Vec<String> = row.get("tags");

                Ok(Some(DocumentView {
                    concept_id: id.clone(),
                    content_id,
                    version: version as u64,
                    raw: Arc::from(raw),
                    doc_type,
                    title,
                    status,
                    tags,
                    links: doc.links,
                }))
            }
        }
    }

    /// Búsqueda semántica (pgvector, cuando hay `GEMINI_API_KEY`
    /// configurada) con reintento automático a coincidencia de
    /// subcadena (`ILIKE`) si no hay clave, o si la llamada a Gemini
    /// falla — un problema transitorio del proveedor de embeddings no
    /// debe tumbar la búsqueda por completo, solo degradarla.
    fn search(&self, query: &SearchQuery, budget: &Budget) -> Result<Vec<SearchHit>, StoreError> {
        let limit = query
            .limit
            .unwrap_or(budget.max_search_results)
            .min(budget.max_search_results) as i64;

        let rows = block_on(async {
            if let (Some(text), Some(gemini_key)) =
                (query.text.as_deref(), self.gemini_api_key.as_deref())
            {
                let client = reqwest::Client::new();
                match embed(&client, gemini_key, text).await {
                    Ok(embedding) => return self.search_semantic(query, &embedding, limit).await,
                    Err(e) => eprintln!(
                        "búsqueda semántica falló ({e}); usando coincidencia de texto (ILIKE) como respaldo"
                    ),
                }
            }
            self.search_keyword(query, limit).await
        })?;

        Ok(rows.into_iter().map(row_to_search_hit).collect())
    }

    fn commit(
        &mut self,
        request: CommitRequest,
        actor: &Principal,
        budget: &Budget,
    ) -> Result<CommitOutcome, StoreError> {
        // 1. Validar el formato OKF del documento antes de realizar transacciones
        let doc = okf_core::parse_document(&request.markdown, budget)?;

        let incoming_hash = sha256(request.markdown.as_bytes());
        let incoming = ContentId(incoming_hash);
        let incoming_hex = incoming.to_hex();

        let res = block_on(async {
            let mut tx = self.pool.begin().await.map_err(|e| StoreError::Backend(e.to_string()))?;

            // Bloquear la fila de la cabeza actual para evitar escrituras concurrentes
            let current_head = pg_query(
                "SELECT content_id, version FROM heads WHERE concept_id = $1 FOR UPDATE",
            )
            .bind(request.concept_id.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| StoreError::Backend(e.to_string()))?;

            let current_content_id = current_head
                .as_ref()
                .map(|h| {
                    let hex: String = h.get("content_id");
                    ContentId::from_hex(&hex).expect("hash de db válido")
                });

            let decision = decide(current_content_id, request.expected, incoming);

            match decision {
                CommitDecision::Conflict(c) => {
                    tx.rollback().await.map_err(|e| StoreError::Backend(e.to_string()))?;
                    Err(StoreError::Conflict(c))
                }
                CommitDecision::NoChange => {
                    let h = current_head.unwrap();
                    let version: i64 = h.get("version");
                    tx.rollback().await.map_err(|e| StoreError::Backend(e.to_string()))?;
                    Ok(CommitOutcome {
                        revision: None,
                        content_id: incoming,
                        version: version as u64,
                        created: false,
                        no_change: true,
                    })
                }
                CommitDecision::Create | CommitDecision::Update => {
                    let created = matches!(decision, CommitDecision::Create);
                    let base_hex = current_content_id.map(|h| h.to_hex());
                    let new_version = current_head.as_ref().map(|h| h.get::<i64, _>("version") + 1).unwrap_or(1);

                    // Insertar blob si no existe
                    pg_query(
                        "INSERT INTO blobs (content_id, raw) VALUES ($1, $2) ON CONFLICT (content_id) DO NOTHING",
                    )
                    .bind(&incoming_hex)
                    .bind(&request.markdown)
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| StoreError::Backend(e.to_string()))?;

                    // Actualizar o crear la cabeza del documento
                    if created {
                        pg_query(
                            "INSERT INTO heads (concept_id, content_id, version, doc_type, title, status, tags)
                             VALUES ($1, $2, $3, $4, $5, $6, $7)",
                        )
                        .bind(request.concept_id.as_str())
                        .bind(&incoming_hex)
                        .bind(new_version)
                        .bind(&doc.doc_type)
                        .bind(&doc.title)
                        .bind(&doc.status)
                        .bind(&doc.tags)
                        .execute(&mut *tx)
                        .await
                        .map_err(|e| StoreError::Backend(e.to_string()))?;
                    } else {
                        pg_query(
                            "UPDATE heads SET content_id = $1, version = $2, doc_type = $3, title = $4, status = $5, tags = $6
                             WHERE concept_id = $7",
                        )
                        .bind(&incoming_hex)
                        .bind(new_version)
                        .bind(&doc.doc_type)
                        .bind(&doc.title)
                        .bind(&doc.status)
                        .bind(&doc.tags)
                        .bind(request.concept_id.as_str())
                        .execute(&mut *tx)
                        .await
                        .map_err(|e| StoreError::Backend(e.to_string()))?;
                    }

                    // Insertar la revisión correspondiente
                    let seq = pg_query_scalar::<i64>(
                        "INSERT INTO revisions (concept_id, base, result, actor_subject, actor_client_id, reason)
                         VALUES ($1, $2, $3, $4, $5, $6)
                         RETURNING seq",
                    )
                    .bind(request.concept_id.as_str())
                    .bind(base_hex.as_deref())
                    .bind(&incoming_hex)
                    .bind(&actor.subject)
                    .bind(&actor.client_id)
                    .bind(&request.reason)
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(|e| StoreError::Backend(e.to_string()))?;

                    // Actualizar los enlaces salientes (derived metadata)
                    pg_query(
                        "DELETE FROM links WHERE source_id = $1",
                    )
                    .bind(request.concept_id.as_str())
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| StoreError::Backend(e.to_string()))?;

                    for link in &doc.links {
                        pg_query(
                            "INSERT INTO links (source_id, target_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
                        )
                        .bind(request.concept_id.as_str())
                        .bind(link.as_str())
                        .execute(&mut *tx)
                        .await
                        .map_err(|e| StoreError::Backend(e.to_string()))?;
                    }

                    // Registrar evento en el Outbox transaccional
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
                         VALUES ('commit', $1, $2, $3)"
                    )
                    .bind(request.concept_id.as_str())
                    .bind(&incoming_hex)
                    .bind(payload)
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| StoreError::Backend(e.to_string()))?;

                    tx.commit().await.map_err(|e| StoreError::Backend(e.to_string()))?;

                    let revision = Revision {
                        seq: seq as u64,
                        concept_id: request.concept_id.clone(),
                        base: current_content_id,
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
        })?;

        Ok(res)
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
