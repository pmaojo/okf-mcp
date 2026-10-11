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

    let mode = outbox_worker::GithubMode::from_env();
    match mode {
        outbox_worker::GithubMode::Off => {
            println!("Modo Supabase-only: GitHub no se lee ni se escribe (para un espejo de salida, OKF_GITHUB_MIRROR=1).");
        }
        outbox_worker::GithubMode::Mirror => {
            if github_token.is_none() || github_repo.is_none() {
                println!("ADVERTENCIA: OKF_GITHUB_MIRROR=1 pero faltan GITHUB_TOKEN o GITHUB_REPO. Se saltará el espejo.");
            }
        }
        outbox_worker::GithubMode::Primary => {
            println!("OKF_STORE=github: el paso A del batch (Contents API) se salta — IndexedStore ya escribió en GitHub de forma síncrona vía git-data API.");
        }
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

    // Reconciliar GitHub -> Supabase solo si GitHub es la fuente de
    // verdad (OKF_STORE=github). En Supabase-only y en espejo de
    // salida, Supabase manda y GitHub no se lee.
    let github_store = if mode == outbox_worker::GithubMode::Primary {
        let gs = github_store::GithubStore::from_env().ok();
        if gs.is_none() {
            println!("ADVERTENCIA: No se pudo inicializar GithubStore. Se saltará la reconciliación GitHub -> Supabase.");
        }
        gs
    } else {
        None
    };

    let mut ticks_since_reconcile = 0;

    loop {
        if ticks_since_reconcile == 0 {
            if let Some(ref gh_store) = github_store {
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
