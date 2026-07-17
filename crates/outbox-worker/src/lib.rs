//! Lógica compartida del worker de Outbox: procesar un lote de eventos
//! pendientes contra GitHub (sincronización) y Gemini (embeddings).
//!
//! Sin el bucle `loop` — vive fuera, en cada binario que llama a
//! [`process_batch`]: `main.rs` (daemon persistente, para desplegar
//! fuera de Vercel) y `crates/vercel-entry/api/outbox.rs` (una
//! invocación por disparo de Vercel Cron, ya que las Funciones
//! serverless no soportan un proceso de larga duración). Mismo patrón
//! que `mcp_http::route` / `crates/vercel-entry/api/mcp.rs`: la lógica
//! pura vive en un crate compartido, cada transporte solo la envuelve.

#![forbid(unsafe_code)]

use base64::prelude::*;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, USER_AGENT};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{PgPool, Row};

#[derive(Debug, Deserialize)]
struct GithubContentResponse {
    sha: String,
}

#[derive(Debug, Serialize)]
struct GithubPutRequest {
    message: String,
    content: String,
    sha: Option<String>,
}

#[derive(Debug, Serialize)]
struct GeminiEmbeddingRequestPart {
    text: String,
}

#[derive(Debug, Serialize)]
struct GeminiEmbeddingRequestContent {
    parts: Vec<GeminiEmbeddingRequestPart>,
}

#[derive(Debug, Serialize)]
struct GeminiEmbeddingRequest {
    model: String,
    content: GeminiEmbeddingRequestContent,
}

#[derive(Debug, Deserialize)]
struct GeminiEmbeddingResponseValue {
    values: Vec<f32>,
}

#[derive(Debug, Deserialize)]
struct GeminiEmbeddingResponse {
    embedding: GeminiEmbeddingResponseValue,
}

/// `schema.sql` compartido con `supabase-store`: mismo contenido que
/// aplica `crates/vercel-entry/api/mcp.rs` en su propio cold start.
/// `CREATE TABLE IF NOT EXISTS` lo hace seguro de repetir en cada
/// binario que arranca contra la misma base de datos.
pub fn schema_sql() -> &'static str {
    include_str!("../../../crates/supabase-store/schema.sql")
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
    gemini_key: Option<&str>,
) -> Result<usize, Box<dyn std::error::Error>> {
    // 1. Obtener un lote de eventos pendientes usando FOR UPDATE SKIP LOCKED
    let mut tx = pool.begin().await?;
    let rows = sqlx::query(
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
        let _content_id: String = row.get("content_id");
        let payload: Value = row.get("payload");

        println!("Procesando evento seq={} tipo={} concepto={}", seq, event_type, concept_id);

        let mut success = true;
        let mut error_msg = String::new();

        if event_type == "commit" {
            let markdown = payload["markdown"].as_str().unwrap_or("");
            let reason = payload["reason"].as_str().unwrap_or("okf-mcp commit");

            // A. Sincronización con GitHub
            if let (Some(token), Some(repo)) = (github_token, github_repo) {
                if let Err(e) = sync_to_github(client, token, repo, &concept_id, markdown, reason).await {
                    success = false;
                    error_msg = format!("GitHub sync failed: {e}");
                }
            }

            // B. Generación de embeddings e indexación vectorial
            if success {
                if let Some(key) = gemini_key {
                    match generate_and_save_embedding(pool, client, key, &concept_id, markdown).await {
                        Ok(_) => {}
                        Err(e) => {
                            success = false;
                            error_msg = format!("Embedding generation failed: {e}");
                        }
                    }
                }
            }
        }

        // C. Actualizar estado en el outbox
        if success {
            sqlx::query(
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
            sqlx::query(
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

async fn sync_to_github(
    client: &reqwest::Client,
    token: &str,
    repo: &str,
    concept_id: &str,
    markdown: &str,
    reason: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let path = format!("docs/{concept_id}.md");
    let url = format!("https://api.github.com/repos/{repo}/contents/{path}");

    let mut headers = HeaderMap::new();
    headers.insert(AUTHORIZATION, HeaderValue::from_str(&format!("Bearer {token}"))?);
    headers.insert(USER_AGENT, HeaderValue::from_static("okf-mcp-outbox-worker"));
    headers.insert("Accept", HeaderValue::from_static("application/vnd.github+json"));

    let content_b64 = BASE64_STANDARD.encode(markdown.as_bytes());

    // GitHub exige el `sha` actual del archivo para actualizarlo — es su
    // propio compare-and-swap. Entre que leemos ese sha (GET) y mandamos
    // el PUT puede colarse otra escritura al mismo archivo (otro worker,
    // un commit manual); GitHub responde 409 con el sha ya caducado. Sin
    // reintento, ese evento se queda incrementando `attempts` con el
    // MISMO sha obsoleto hasta marcarse 'failed' a la quinta vez, sin
    // haber tenido nunca una oportunidad real de sincronizar. Releer y
    // reintentar en el momento resuelve la carrera sin esperar al
    // reintento del outbox (que tarda 5 rondas de sleep(5s)).
    const MAX_INTENTOS: u32 = 3;
    for intento in 1..=MAX_INTENTOS {
        let sha = fetch_sha(client, &url, &headers).await?;

        let put_req = GithubPutRequest {
            message: reason.to_string(),
            content: content_b64.clone(),
            sha,
        };

        let put_resp = client.put(&url)
            .headers(headers.clone())
            .json(&put_req)
            .send()
            .await?;

        if put_resp.status().is_success() {
            println!("Concepto {concept_id} sincronizado con GitHub exitosamente.");
            return Ok(());
        }

        let sha_obsoleto = put_resp.status() == reqwest::StatusCode::CONFLICT;
        if sha_obsoleto && intento < MAX_INTENTOS {
            eprintln!(
                "sha obsoleto sincronizando {concept_id} (intento {intento}/{MAX_INTENTOS}); releyendo y reintentando"
            );
            continue;
        }

        let err_text = put_resp.text().await?;
        return Err(format!("GitHub API returned error: {err_text}").into());
    }

    unreachable!("el bucle siempre devuelve en el último intento (éxito o error)");
}

/// SHA actual del archivo en GitHub, o `None` si todavía no existe
/// (entonces el PUT lo crea en vez de actualizarlo).
async fn fetch_sha(
    client: &reqwest::Client,
    url: &str,
    headers: &HeaderMap,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let get_resp = client.get(url).headers(headers.clone()).send().await?;
    if get_resp.status().is_success() {
        let content: GithubContentResponse = get_resp.json().await?;
        Ok(Some(content.sha))
    } else {
        Ok(None)
    }
}

async fn generate_and_save_embedding(
    pool: &PgPool,
    client: &reqwest::Client,
    gemini_key: &str,
    concept_id: &str,
    markdown: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/text-embedding-004:embedContent?key={}",
        gemini_key
    );

    let req_body = GeminiEmbeddingRequest {
        model: "models/text-embedding-004".to_string(),
        content: GeminiEmbeddingRequestContent {
            parts: vec![GeminiEmbeddingRequestPart {
                text: markdown.to_string(),
            }],
        },
    };

    let resp = client.post(&url)
        .json(&req_body)
        .send()
        .await?;

    if !resp.status().is_success() {
        let err_text = resp.text().await?;
        return Err(format!("Gemini API returned error: {}", err_text).into());
    }

    let embed_resp: GeminiEmbeddingResponse = resp.json().await?;
    let values = embed_resp.embedding.values;

    // Formatear vector como string: "[0.1,0.2,...]"
    let vector_str = format!(
        "[{}]",
        values
            .iter()
            .map(|v| v.to_string())
            .collect::<Vec<_>>()
            .join(",")
    );

    // Guardar o actualizar en pgvector
    sqlx::query(
        "INSERT INTO embeddings (concept_id, embedding)
         VALUES ($1, $2::vector)
         ON CONFLICT (concept_id) DO UPDATE SET embedding = EXCLUDED.embedding"
    )
    .bind(concept_id)
    .bind(vector_str)
    .execute(pool)
    .await?;

    println!("Embedding para {} generado e indexado exitosamente.", concept_id);
    Ok(())
}
