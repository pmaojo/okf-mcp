//! Adaptador Vercel: el bridge de la función serverless a
//! `mcp_http::route`, la MISMA lógica pura que sirve el transporte
//! local (`crates/mcp-http`, probado con `TcpListener` de verdad en
//! el tutorial, capítulo 10). Esta es la ÚNICA pieza nueva de este
//! archivo: traducir entre el `axum::Request` que entrega
//! `vercel_runtime` y `mcp_http::{HttpRequest, HttpResponse}`.
//!
//! Todo lo demás — parseo JSON-RPC, las cuatro herramientas, el
//! presupuesto, la comprobación de `Origin` — es exactamente el
//! código que ya corre con `cargo run -p mcp-http`.

#![forbid(unsafe_code)]

mod auth;

use axum::body::Bytes;
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Router;
use mcp_core::McpServer;
use mcp_http::{route, HttpRequest};
use mcp_stdio::MemoryTools;
use memory_model::{Budget, Principal};
use supabase_store::SupabaseStore;
use std::sync::OnceLock;
use tower::ServiceBuilder;
use vercel_runtime::axum::VercelLayer;
use vercel_runtime::{run, Error};

static DB_POOL: OnceLock<sqlx::PgPool> = OnceLock::new();

/// Obtiene o inicializa el pool de conexiones de base de datos de manera thread-safe.
async fn get_db_pool(db_url: &str) -> sqlx::PgPool {
    if let Some(pool) = DB_POOL.get() {
        return pool.clone();
    }
    let pool = sqlx::PgPool::connect(db_url)
        .await
        .expect("Failed to connect to Supabase PostgreSQL database");
    let _ = DB_POOL.set(pool.clone());
    pool
}

/// Lista separada por comas en la variable de entorno
/// `ALLOWED_ORIGINS` del proyecto Vercel.
fn allowed_origins() -> Vec<String> {
    std::env::var("ALLOWED_ORIGINS")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Handler único: da igual la ruta pública que el cliente use
/// (`/mcp` vía el rewrite de `vercel.json`, o `/api/mcp` por
/// defecto) — este archivo ES semánticamente el endpoint `/mcp`.
async fn mcp_handler(method: Method, headers: HeaderMap, body: Bytes) -> Response {
    let origin = headers.get("origin").and_then(|v| v.to_str().ok()).map(str::to_string);
    let content_type = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    let http_req = HttpRequest {
        method: method.to_string(),
        path: "/mcp".to_string(),
        origin,
        content_type,
        body: body.to_vec(),
    };

    let budget = Budget::default();
    let origins = allowed_origins();

    // 1. Obtener y validar el Principal (OAuth 2.1)
    let auth_header = headers.get("authorization").and_then(|v| v.to_str().ok());
    let actor = match std::env::var("JWKS_URL") {
        Ok(jwks_url) => {
            let audience = std::env::var("JWT_AUDIENCE").ok();
            match auth::validate_jwt(auth_header, &jwks_url, audience.as_deref()).await {
                Ok(principal) => principal,
                Err(err) => {
                    return (
                        StatusCode::UNAUTHORIZED,
                        [("content-type", "text/plain")],
                        format!("Unauthorized: {}", err.0),
                    )
                        .into_response();
                }
            }
        }
        Err(_) => {
            // Si no está configurado JWKS_URL, corremos en desarrollo local/abierto sin validar
            Principal::local_dev()
        }
    };

    // 2. Obtener pool de base de datos e instanciar servicios de forma efímera
    let db_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let pool = get_db_pool(&db_url).await;
    let store = SupabaseStore::new(pool);
    let tools = MemoryTools::new(store, actor, budget);
    let mut server = McpServer::new("okf-memory-vercel", env!("CARGO_PKG_VERSION"), tools);

    let resp = route(&http_req, &budget, &origins, &mut server);

    let status = StatusCode::from_u16(resp.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, [("content-type", resp.content_type)], resp.body).into_response()
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    // Sin rutas registradas: TODA petición cae en el fallback.
    let router = Router::new().fallback(mcp_handler);
    let app = ServiceBuilder::new().layer(VercelLayer::new()).service(router);
    run(app).await
}
