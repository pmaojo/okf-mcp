//! Adaptador Vercel del worker de Outbox: dispara
//! `outbox_worker::process_batch` UNA vez por invocación, en vez del
//! `loop` infinito de `crates/outbox-worker/src/main.rs` — las
//! Funciones serverless de Vercel no soportan un proceso de larga
//! duración, así que aquí el "loop" lo hace Vercel Cron llamando a
//! este endpoint periódicamente (ver `crons` en `vercel.json`).
//!
//! Protegido con `CRON_SECRET`: cuando configuras esa variable de
//! entorno en el proyecto de Vercel, Vercel añade automáticamente
//! `Authorization: Bearer <CRON_SECRET>` a las peticiones con las que
//! DISPARA sus propios Cron Jobs. Comprobarlo evita que cualquiera que
//! adivine la URL pública dispare sincronizaciones por su cuenta
//! (llamadas de pago/con cuota a la API de GitHub y a la de Gemini).
//! Sin `CRON_SECRET` configurado, el endpoint queda abierto — igual
//! que `api/mcp.rs` sin `JWKS_URL`: aceptable en desarrollo, no en
//! producción.

#![forbid(unsafe_code)]

use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Router;
use sqlx::postgres::PgConnectOptions;
use std::str::FromStr;
use std::sync::OnceLock;
use vercel_runtime::axum::VercelLayer;
use vercel_runtime::{run, Error};

static DB_POOL: OnceLock<sqlx::PgPool> = OnceLock::new();

/// Mismo patrón que `api/mcp.rs::get_db_pool`: pool cacheado por
/// instancia serverless, con el caché de prepared statements
/// desactivado porque el pooler de Supabase (modo transacción) puede
/// reciclar la conexión física entre invocaciones distintas.
async fn get_db_pool(db_url: &str) -> sqlx::PgPool {
    if let Some(pool) = DB_POOL.get() {
        return pool.clone();
    }
    let connect_options = PgConnectOptions::from_str(db_url)
        .expect("Invalid POSTGRES_URL")
        .statement_cache_capacity(0);
    let pool = sqlx::PgPool::connect_with(connect_options)
        .await
        .expect("Failed to connect to Supabase PostgreSQL database");
    sqlx::raw_sql(outbox_worker::schema_sql())
        .execute(&pool)
        .await
        .expect("Failed to apply database schema");
    let _ = DB_POOL.set(pool.clone());
    pool
}

/// `true` si la petición trae el `CRON_SECRET` esperado — o si no hay
/// ningún `CRON_SECRET` configurado.
fn cron_autorizado(headers: &HeaderMap) -> bool {
    let Ok(secret) = std::env::var("CRON_SECRET") else {
        return true;
    };
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|v| v == format!("Bearer {secret}"))
        .unwrap_or(false)
}

async fn outbox_handler(headers: HeaderMap) -> Response {
    if !cron_autorizado(&headers) {
        return (StatusCode::UNAUTHORIZED, "CRON_SECRET inválido o ausente").into_response();
    }

    let db_url = match std::env::var("POSTGRES_URL") {
        Ok(v) => v,
        Err(_) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "POSTGRES_URL no configurado")
                .into_response();
        }
    };
    let github_token = std::env::var("GITHUB_TOKEN").ok();
    let github_repo = std::env::var("GITHUB_REPO").ok();
    let gemini_key = std::env::var("GEMINI_API_KEY").ok();

    let pool = get_db_pool(&db_url).await;
    let client = reqwest::Client::new();

    match outbox_worker::process_batch(
        &pool,
        &client,
        github_token.as_deref(),
        github_repo.as_deref(),
        gemini_key.as_deref(),
    )
    .await
    {
        Ok(processed) => (
            StatusCode::OK,
            [("content-type", "application/json")],
            format!(r#"{{"processed":{processed}}}"#),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("error procesando el lote: {e}"),
        )
            .into_response(),
    }
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let router = Router::new().fallback(outbox_handler);
    let app = tower::ServiceBuilder::new().layer(VercelLayer::new()).service(router);
    run(app).await
}
