//! Lógica compartida del worker de Outbox: procesar un lote de eventos
//! pendientes contra GitHub (vía el submódulo `github_sync`) y Gemini (generando embeddings),
//! además de ejecutar el bucle de reconciliación inversa periódica de GitHub -> Supabase.
//!
//! Sin el bucle `loop` — vive fuera, en cada binario que llama a
//! [`process_batch`]: `main.rs` (daemon persistente, para desplegar
//! fuera de Vercel) y `crates/vercel-entry/api/outbox.rs` (una
//! invocación por disparo de Vercel Cron, ya que las Funciones
//! serverless no soportan un proceso de larga duración). Mismo patrón
//! que `mcp_http::route` / `crates/vercel-entry/api/mcp.rs`: la lógica
//! pura vive en un crate compartido, cada transporte solo la envuelve.

#![forbid(unsafe_code)]

mod github_sync;
mod reconciliation;

pub use reconciliation::reconcile_github_to_supabase;

use serde_json::Value;
use sqlx::{PgPool, Row};

/// Consultas con `persistent(false)`: `POSTGRES_URL` suele apuntar al
/// pooler de Supabase en modo transacción, donde un prepared statement
/// CON nombre (`sqlx_s_N`) puede colisionar con el de otra sesión
/// lógica sobre la misma conexión física reciclada. El statement SIN
/// nombre del protocolo se re-prepara en cada uso y no colisiona —
/// misma regla que en `supabase-store` (ver su `pg_query`).
pub(crate) fn pg_query(sql: &str) -> sqlx::query::Query<'_, sqlx::Postgres, sqlx::postgres::PgArguments> {
    sqlx::query(sql).persistent(false)
}

/// `schema.sql` compartido con `supabase-store`: mismo contenido que
/// aplica `crates/vercel-entry/api/mcp.rs` en su propio cold start.
/// `CREATE TABLE IF NOT EXISTS` lo hace seguro de repetir en cada
/// binario que arranca contra la misma base de datos.
pub fn schema_sql() -> &'static str {
    include_str!("../../../crates/supabase-store/schema.sql")
}

/// Papel de GitHub en el despliegue. Por defecto NINGUNO: Supabase es
/// el único almacén (datos, historia, enlaces, embeddings) y GitHub
/// no se lee ni se escribe. Un límite de la API de GitHub nunca puede
/// tumbar la memoria si GitHub no está en el camino.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GithubMode {
    /// Sin GitHub (por defecto): ni espejo ni reconciliación.
    Off,
    /// Espejo de SALIDA opt-in (`OKF_GITHUB_MIRROR=1`): cada
    /// commit/delete de Supabase se copia a GitHub vía Contents API.
    /// Supabase sigue siendo la fuente de verdad: NUNCA se reconcilia
    /// GitHub → Supabase en este modo.
    Mirror,
    /// GitHub es la fuente de verdad (`OKF_STORE=github`, el modo
    /// antiguo `IndexedStore`): se reconcilia GitHub → Supabase.
    Primary,
}

impl GithubMode {
    /// Decide el modo a partir de los valores de `OKF_STORE` y
    /// `OKF_GITHUB_MIRROR` (función pura: testeable sin tocar el
    /// entorno del proceso).
    ///
    /// ```
    /// use outbox_worker::GithubMode;
    /// assert_eq!(GithubMode::from_values(None, None), GithubMode::Off);
    /// assert_eq!(GithubMode::from_values(Some("supabase"), None), GithubMode::Off);
    /// assert_eq!(GithubMode::from_values(None, Some("1")), GithubMode::Mirror);
    /// assert_eq!(GithubMode::from_values(Some("github"), Some("1")), GithubMode::Primary);
    /// ```
    pub fn from_values(okf_store: Option<&str>, mirror: Option<&str>) -> GithubMode {
        if okf_store == Some("github") {
            GithubMode::Primary
        } else if matches!(mirror, Some("1") | Some("true") | Some("yes")) {
            GithubMode::Mirror
        } else {
            GithubMode::Off
        }
    }

    /// Lee el modo del entorno del proceso.
    pub fn from_env() -> GithubMode {
        GithubMode::from_values(
            std::env::var("OKF_STORE").ok().as_deref(),
            std::env::var("OKF_GITHUB_MIRROR").ok().as_deref(),
        )
    }
}

/// Credenciales de GitHub para el paso A de [`process_batch`]: solo
/// en [`GithubMode::Mirror`]. En `Off` no hay paso A; en `Primary`
/// (`OKF_STORE=github`) se omite porque `IndexedStore` ya escribió.
///
/// En modo `IndexedStore` (`OKF_STORE=github`), GitHub ya es la
/// fuente de verdad: cada commit/delete se escribe ahí de forma
/// síncrona (git-data API) ANTES de que `IndexedStore` intente el
/// espejo best-effort a Supabase — y ese espejo es justamente quien
/// encola el evento del outbox que `process_batch` procesaría aquí.
/// Sin este corte, el paso A repetiría esa misma escritura vía la API
/// de Contents, duplicando cada commit/delete en el repo. El resto
/// del batch (paso B: embeddings) sigue funcionando igual — sirve de
/// red de reintento si el embedding inline del commit falló.
pub fn github_sync_credentials() -> (Option<String>, Option<String>) {
    if GithubMode::from_env() != GithubMode::Mirror {
        return (None, None);
    }
    (std::env::var("GITHUB_TOKEN").ok(), std::env::var("GITHUB_REPO").ok())
}

/// Procesa un lote de hasta 10 eventos `pending` del outbox. Devuelve
/// cuántos tomó (no necesariamente cuántos tuvieron éxito: los
/// fallidos vuelven a `pending`, o a `failed` a partir del quinto
/// intento).
pub async fn process_batch(
    pool: &PgPool,
    client: &reqwest::Client,
    github_token: Option<&str>,
    github_repo: Option<&str>,
    embedding_keys: &gemini_embeddings::EmbeddingKeys,
) -> Result<usize, Box<dyn std::error::Error>> {
    // 1. Obtener un lote de eventos pendientes usando FOR UPDATE SKIP LOCKED
    let mut tx = pool.begin().await?;
    let rows = pg_query(
        "SELECT seq, event_type, concept_id, content_id, payload
         FROM outbox
         WHERE status = 'pending'
         ORDER BY seq
         LIMIT 10
         FOR UPDATE SKIP LOCKED"
    )
    .fetch_all(&mut *tx)
    .await?;

    let count = rows.len();
    if count > 0 {
        println!("Procesando lote de {} eventos...", count);
    }

    for row in rows {
        let seq: i64 = row.get("seq");
        let event_type: String = row.get("event_type");
        let concept_id: String = row.get("concept_id");
        let content_id: String = row.get("content_id");
        let payload: Value = row.get("payload");

        println!("Procesando evento seq={} tipo={} concepto={}", seq, event_type, concept_id);

        let mut success = true;
        let mut error_msg = String::new();

        if event_type == "commit" {
            let markdown = payload["markdown"].as_str().unwrap_or("");
            let reason = payload["reason"].as_str().unwrap_or("okf-mcp commit");

            // A. Sincronización con GitHub
            if let (Some(token), Some(repo)) = (github_token, github_repo) {
                if let Err(e) = github_sync::sync_to_github(client, token, repo, &concept_id, markdown, reason).await {
                    success = false;
                    error_msg = format!("GitHub sync failed: {e}");
                }
            }

            // B. Generación de embeddings e indexación vectorial
            if success && embedding_keys.any_configured() {
                match supabase_store::index_embedding(pool, client, embedding_keys, &concept_id, &content_id, markdown).await {
                    Ok(_) => {}
                    Err(e) => {
                        success = false;
                        error_msg = format!("Embedding generation failed: {e}");
                    }
                }
            }
        } else if event_type == "delete" {
            let reason = payload["reason"].as_str().unwrap_or("okf-mcp delete");

            // A. Borrado de GitHub
            if let (Some(token), Some(repo)) = (github_token, github_repo) {
                if let Err(e) = github_sync::delete_from_github(client, token, repo, &concept_id, reason).await {
                    success = false;
                    error_msg = format!("GitHub delete failed: {e}");
                }
            }

            // B. Borrar embedding en la DB
            if success {
                let res = pg_query("DELETE FROM embeddings WHERE concept_id = $1")
                    .bind(&concept_id)
                    .execute(pool)
                    .await;
                if let Err(e) = res {
                    success = false;
                    error_msg = format!("Embedding deletion failed: {e}");
                }
            }
        }

        // C. Actualizar estado en el outbox
        if success {
            pg_query(
                "UPDATE outbox
                 SET status = 'processed', processed_at = CURRENT_TIMESTAMP
                 WHERE seq = $1"
            )
            .bind(seq)
            .execute(&mut *tx)
            .await?;
            println!("Evento seq={} procesado con éxito.", seq);
        } else {
            eprintln!("Error procesando evento seq={}: {}", seq, error_msg);
            pg_query(
                "UPDATE outbox
                 SET attempts = attempts + 1,
                     status = CASE WHEN attempts >= 5 THEN 'failed' ELSE 'pending' END
                 WHERE seq = $1"
            )
            .bind(seq)
            .execute(&mut *tx)
            .await?;
        }
    }

    tx.commit().await?;
    Ok(count)
}
