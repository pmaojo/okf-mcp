//! Daemon persistente: bucle infinito sobre `outbox_worker::process_batch`.
//! Pensado para desplegarse FUERA de Vercel (Fly.io, Railway, un
//! contenedor en cualquier VPS) — un proceso de larga duración no
//! encaja en el modelo de Funciones serverless. El disparador
//! equivalente para Vercel (una invocación por Cron, sin loop) vive en
//! `crates/vercel-entry/api/outbox.rs`, reutilizando exactamente la
//! misma `process_batch`.

#![forbid(unsafe_code)]

use outbox_worker::process_batch;
use sqlx::PgPool;
use std::env;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Iniciando worker de Outbox...");

    let db_url = env::var("POSTGRES_URL").expect("POSTGRES_URL must be set");
    let (github_token, github_repo) = outbox_worker::github_sync_credentials();
    let embedding_keys = gemini_embeddings::EmbeddingKeys::from_env();

    if env::var("OKF_STORE").as_deref() == Ok("github") {
        println!("OKF_STORE=github: el paso A del batch (Contents API) se salta — IndexedStore ya escribió en GitHub de forma síncrona vía git-data API.");
    } else if github_token.is_none() || github_repo.is_none() {
        println!("ADVERTENCIA: GITHUB_TOKEN o GITHUB_REPO no configurados. Se saltará la sincronización con Git.");
    }
    if !embedding_keys.any_configured() {
        println!("ADVERTENCIA: ningún proveedor de embeddings configurado (GEMINI_API_KEY / MISTRAL_API_KEY / COHERE_API_KEY). Se saltará la generación de embeddings.");
    }

    let pool = PgPool::connect(&db_url).await?;

    // `schema.sql` trae varias sentencias separadas por `;`; `sqlx::query`
    // usa el protocolo extendido (prepared statement) y Postgres rechaza
    // varios comandos en una sola consulta preparada. `raw_sql` usa el
    // protocolo simple, que sí soporta scripts multi-sentencia (mismo
    // fallo y mismo fix que en vercel-entry/api/mcp.rs).
    sqlx::raw_sql(outbox_worker::schema_sql())
        .execute(&pool)
        .await?;

    let run_once = env::var("ONCE").is_ok();
    let client = reqwest::Client::new();

    let github_store = github_store::GithubStore::from_env();
    if github_store.is_err() {
        println!("ADVERTENCIA: No se pudo inicializar GithubStore. Se saltará la reconciliación GitHub -> Supabase.");
    }

    let mut ticks_since_reconcile = 0;

    loop {
        if ticks_since_reconcile == 0 {
            if let Ok(ref gh_store) = github_store {
                if let Err(e) = outbox_worker::reconcile_github_to_supabase(
                    &pool,
                    &client,
                    gh_store,
                    &embedding_keys,
                )
                .await
                {
                    eprintln!("Error en la reconciliación GitHub -> Supabase: {}", e);
                }
            }
        }

        let processed = process_batch(&pool, &client, github_token.as_deref(), github_repo.as_deref(), &embedding_keys).await?;

        if run_once {
            println!("Ejecución única (ONCE=1) completada.");
            break;
        }

        if processed == 0 {
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
        
        ticks_since_reconcile = (ticks_since_reconcile + 1) % 60;
    }

    Ok(())
}
