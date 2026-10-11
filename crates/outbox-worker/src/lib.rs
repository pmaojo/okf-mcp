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
pub mod inbound;
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

/// Papel de GitHub en el despliegue. Supabase es SIEMPRE el almacén
/// principal salvo con `OKF_STORE=github`; esto decide qué se copia a
/// GitHub y qué se lee de él.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GithubMode {
    /// Sin GitHub: ni espejo ni reconciliación (no hay credenciales
    /// o el espejo está apagado a propósito con `OKF_GITHUB_MIRROR=0`).
    Off,
    /// Espejo de SALIDA: cada commit/delete de Supabase se copia a
    /// GitHub vía Contents API desde el outbox (el cron ya existente
    /// o el worker). Es el modo por defecto en cuanto hay
    /// `GITHUB_TOKEN` + `GITHUB_REPO`, como siempre. Supabase manda:
    /// NUNCA se reconcilia GitHub → Supabase en este modo (esa
    /// reconciliación trata a GitHub como verdad absoluta y borraría
    /// lo que solo existe en Supabase). Lo que sí admite es la
    /// importación incremental de pushes: ver [`inbound`].
    Mirror,
    /// GitHub es la fuente de verdad (`OKF_STORE=github`, el modo
    /// antiguo `IndexedStore`): se reconcilia GitHub → Supabase.
    Primary,
}

impl GithubMode {
    /// Decide el modo a partir de `OKF_STORE`, `OKF_GITHUB_MIRROR` y
    /// de si hay credenciales (`GITHUB_TOKEN` y `GITHUB_REPO`).
    /// Función pura: testeable sin tocar el entorno del proceso.
    ///
    /// ```
    /// use outbox_worker::GithubMode;
    /// // Sin credenciales no hay GitHub.
    /// assert_eq!(GithubMode::from_values(None, None, false), GithubMode::Off);
    /// // Con credenciales, espejo de salida (el comportamiento de siempre).
    /// assert_eq!(GithubMode::from_values(None, None, true), GithubMode::Mirror);
    /// assert_eq!(GithubMode::from_values(Some("supabase"), None, true), GithubMode::Mirror);
    /// // Se puede apagar a propósito.
    /// assert_eq!(GithubMode::from_values(None, Some("0"), true), GithubMode::Off);
    /// // El modo antiguo manda sobre todo lo demás.
    /// assert_eq!(GithubMode::from_values(Some("github"), Some("0"), false), GithubMode::Primary);
    /// ```
    pub fn from_values(okf_store: Option<&str>, mirror: Option<&str>, has_credentials: bool) -> GithubMode {
        if okf_store == Some("github") {
            GithubMode::Primary
        } else if has_credentials && !matches!(mirror, Some("0") | Some("false") | Some("no") | Some("off")) {
            GithubMode::Mirror
        } else {
            GithubMode::Off
        }
    }

    /// Lee el modo del entorno del proceso.
    pub fn from_env() -> GithubMode {
        let has_credentials =
            std::env::var("GITHUB_TOKEN").is_ok() && std::env::var("GITHUB_REPO").is_ok();
        GithubMode::from_values(
            std::env::var("OKF_STORE").ok().as_deref(),
            std::env::var("OKF_GITHUB_MIRROR").ok().as_deref(),
            has_credentials,
        )
    }
}

/// ¿Está activa la importación incremental de pushes de GitHub
/// (`OKF_GITHUB_INBOUND=1`)? Apagada por defecto.
///
/// ```
/// use outbox_worker::inbound_enabled_from;
/// assert!(!inbound_enabled_from(None));
/// assert!(inbound_enabled_from(Some("1")));
/// assert!(!inbound_enabled_from(Some("0")));
/// ```
pub fn inbound_enabled_from(value: Option<&str>) -> bool {
    matches!(value, Some("1") | Some("true") | Some("yes"))
}

/// Lee `OKF_GITHUB_INBOUND` del entorno.
pub fn inbound_enabled() -> bool {
    inbound_enabled_from(std::env::var("OKF_GITHUB_INBOUND").ok().as_deref())
}

/// Credenciales de GitHub para el paso A de [`process_batch`]: solo
/// en [`GithubMode::Mirror`]. En `Off` no hay paso A; en `Primary`
/// (`OKF_STORE=github`) se omite porque `IndexedStore` ya escribió
/// de forma síncrona y repetirlo duplicaría cada commit.
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
