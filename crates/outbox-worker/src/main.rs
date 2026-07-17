#![forbid(unsafe_code)]

use base64::prelude::*;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, USER_AGENT};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{PgPool, Row};
use std::env;
use std::time::Duration;

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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Iniciando worker de Outbox...");

    let db_url = env::var("POSTGRES_URL").expect("POSTGRES_URL must be set");
    let github_token = env::var("GITHUB_TOKEN").ok();
    let github_repo = env::var("GITHUB_REPO").ok(); // formato: "usuario/repositorio"
    let gemini_key = env::var("GEMINI_API_KEY").ok();

    if github_token.is_none() || github_repo.is_none() {
        println!("ADVERTENCIA: GITHUB_TOKEN o GITHUB_REPO no configurados. Se saltará la sincronización con Git.");
    }
    if gemini_key.is_none() {
        println!("ADVERTENCIA: GEMINI_API_KEY no configurada. Se saltará la generación de embeddings.");
    }

    let pool = PgPool::connect(&db_url).await?;

    // Ejecutar migraciones o verificar la existencia de las tablas.
    // `schema.sql` trae varias sentencias separadas por `;`; `sqlx::query`
    // usa el protocolo extendido (prepared statement) y Postgres rechaza
    // varios comandos en una sola consulta preparada. `raw_sql` usa el
    // protocolo simple, que sí soporta scripts multi-sentencia (mismo
    // fallo y mismo fix que en vercel-entry/api/mcp.rs).
    sqlx::raw_sql(include_str!("../../../crates/supabase-store/schema.sql"))
        .execute(&pool)
        .await?;

    let run_once = env::var("ONCE").is_ok();
    let client = reqwest::Client::new();

    loop {
        let processed = process_batch(&pool, &client, github_token.as_deref(), github_repo.as_deref(), gemini_key.as_deref()).await?;
        
        if run_once {
            println!("Ejecución única (ONCE=1) completada.");
            break;
        }

        if processed == 0 {
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }

    Ok(())
}

async fn process_batch(
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

    // 1. Comprobar si el archivo ya existe para obtener su SHA
    let get_resp = client.get(&url)
        .headers(headers.clone())
        .send()
        .await?;

    let sha = if get_resp.status().is_success() {
        let content: GithubContentResponse = get_resp.json().await?;
        Some(content.sha)
    } else {
        None
    };

    // 2. Base64 encoding del markdown
    let content_b64 = BASE64_STANDARD.encode(markdown.as_bytes());

    // 3. Crear o actualizar el archivo
    let put_req = GithubPutRequest {
        message: reason.to_string(),
        content: content_b64,
        sha,
    };

    let put_resp = client.put(&url)
        .headers(headers)
        .json(&put_req)
        .send()
        .await?;

    if !put_resp.status().is_success() {
        let err_text = put_resp.text().await?;
        return Err(format!("GitHub API returned error: {}", err_text).into());
    }

    println!("Concepto {} sincronizado con GitHub exitosamente.", concept_id);
    Ok(())
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
