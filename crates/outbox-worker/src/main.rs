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
