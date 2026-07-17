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

use vercel_entry::auth;

use axum::body::Bytes;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use mcp_core::McpServer;
use mcp_http::{route, HttpRequest};
use memory_tools::MemoryTools;
use memory_model::{Budget, Principal};
use supabase_store::SupabaseStore;
use std::sync::OnceLock;
use tower::ServiceBuilder;
use vercel_runtime::axum::VercelLayer;
use vercel_runtime::{run, Error};

static DB_POOL: OnceLock<sqlx::PgPool> = OnceLock::new();

/// Obtiene o inicializa el pool de conexiones de base de datos de manera thread-safe.
///
/// En el primer cold start de cada instancia serverless, aplica
/// `schema.sql` contra `POSTGRES_URL` (mismo patrón que
/// `outbox-worker`, ver `crates/outbox-worker/src/main.rs`). Es
/// idempotente (`CREATE TABLE IF NOT EXISTS`), así que repetirlo en
/// cada deploy/instancia nueva es seguro.
async fn get_db_pool(db_url: &str) -> sqlx::PgPool {
    if let Some(pool) = DB_POOL.get() {
        return pool.clone();
    }
    let pool = sqlx::PgPool::connect(db_url)
        .await
        .expect("Failed to connect to Supabase PostgreSQL database");
    sqlx::query(include_str!("../../supabase-store/schema.sql"))
        .execute(&pool)
        .await
        .expect("Failed to apply database schema");
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

/// Reconstruye el origen público (esquema + host) a partir de las
/// cabeceras de la petición, para anunciar URLs absolutas en la
/// metadata de descubrimiento OAuth (RFC 9728) sin hardcodear el
/// dominio de despliegue (útil también en preview deployments).
fn base_url(headers: &HeaderMap) -> String {
    let host = headers.get("host").and_then(|v| v.to_str().ok()).unwrap_or("localhost");
    let scheme = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("https");
    format!("{scheme}://{host}")
}

/// `GET /.well-known/oauth-protected-resource` (RFC 9728): le dice a
/// un cliente MCP OAuth-aware (p. ej. Claude) contra qué
/// Authorization Server debe autenticarse antes de llamar a `/mcp`.
/// El AS mismo (aquí, Supabase Auth) expone su propio descubrimiento
/// OIDC en `<issuer>/.well-known/openid-configuration`.
async fn protected_resource_metadata_handler(headers: HeaderMap) -> Response {
    let issuer = match std::env::var("OAUTH_ISSUER") {
        Ok(v) => v,
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
    };
    let body = format!(
        r#"{{"resource":"{}/mcp","authorization_servers":["{issuer}"],"bearer_methods_supported":["header"]}}"#,
        base_url(&headers)
    );
    (StatusCode::OK, [("content-type", "application/json")], body).into_response()
}

/// `GET /authorize`: proxy transparente hacia el `authorization_endpoint`
/// real de Supabase.
///
/// Existe por una limitación observada en el modo "Client ID manual"
/// de Claude para conectores MCP: en vez de seguir el
/// `authorization_servers` que anunciamos en
/// `/.well-known/oauth-protected-resource` (RFC 9728), asume que el
/// propio servidor MCP aloja el Authorization Server en su mismo
/// dominio. En vez de pelearnos con eso, se lo damos: este endpoint
/// reenvía la query string tal cual al `authorization_endpoint` real,
/// sin tocar ni un parámetro (ni siquiera el `code_challenge` —
/// cualquier re-serialización podría cambiar el encoding y romper la
/// verificación PKCE en el otro extremo).
async fn authorize_proxy_handler(uri: Uri) -> Response {
    let issuer = match std::env::var("OAUTH_ISSUER") {
        Ok(v) => v,
        Err(_) => return (StatusCode::NOT_FOUND, "OAUTH_ISSUER no configurado").into_response(),
    };
    let query = uri.query().unwrap_or("");
    let location = format!("{issuer}/oauth/authorize?{query}");
    (StatusCode::FOUND, [("location", location)]).into_response()
}

/// `POST /token`: reenvía tal cual al `token_endpoint` real de
/// Supabase y devuelve su respuesta sin modificar.
///
/// No custodia ningún secreto de cliente: Supabase acepta clientes
/// públicos autenticados solo por PKCE
/// (`token_endpoint_auth_methods_supported` incluye `"none"`), así
/// que este proxy es un simple reenvío de bytes, no un participante
/// de la negociación.
async fn token_proxy_handler(headers: HeaderMap, body: Bytes) -> Response {
    let issuer = match std::env::var("OAUTH_ISSUER") {
        Ok(v) => v,
        Err(_) => return (StatusCode::NOT_FOUND, "OAUTH_ISSUER no configurado").into_response(),
    };
    let content_type = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/x-www-form-urlencoded")
        .to_string();

    let client = reqwest::Client::new();
    let upstream = match client
        .post(format!("{issuer}/oauth/token"))
        .header("content-type", content_type)
        .body(body.to_vec())
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                format!("no se pudo contactar con el Authorization Server: {e}"),
            )
                .into_response();
        }
    };

    let status = StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let resp_content_type = upstream
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/json")
        .to_string();
    let bytes = match upstream.bytes().await {
        Ok(b) => b,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                format!("no se pudo leer la respuesta del Authorization Server: {e}"),
            )
                .into_response();
        }
    };

    (status, [("content-type", resp_content_type)], bytes).into_response()
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
            // La especificación de autorización de MCP exige validar que
            // el token fue emitido para ESTE recurso (claim `aud`). Con
            // JWKS_URL activo ya no estamos en modo abierto de desarrollo,
            // así que exigimos también JWT_AUDIENCE: aceptar cualquier
            // audiencia dejaría pasar tokens emitidos para otra app bajo
            // el mismo Authorization Server (p. ej. otro cliente OAuth de
            // Supabase que no es este servidor MCP).
            let audience = match std::env::var("JWT_AUDIENCE") {
                Ok(v) => v,
                Err(_) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        [("content-type", "text/plain")],
                        "Server misconfigured: JWKS_URL is set but JWT_AUDIENCE is missing".to_string(),
                    )
                        .into_response();
                }
            };
            match auth::validate_jwt(auth_header, &jwks_url, Some(&audience)).await {
                Ok(principal) => principal,
                Err(err) => {
                    // Cabecera exigida por la especificación de autorización
                    // de MCP: le dice al cliente dónde encontrar la metadata
                    // del recurso protegido (RFC 9728) para iniciar el flujo
                    // OAuth en lugar de fallar en silencio.
                    let metadata_url = format!(
                        "{}/.well-known/oauth-protected-resource",
                        base_url(&headers)
                    );
                    return (
                        StatusCode::UNAUTHORIZED,
                        [
                            ("content-type", "text/plain".to_string()),
                            (
                                "www-authenticate",
                                format!(r#"Bearer resource_metadata="{metadata_url}""#),
                            ),
                        ],
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
    let db_url = std::env::var("POSTGRES_URL").expect("POSTGRES_URL must be set");
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
    // Rutas explícitas: la metadata de descubrimiento OAuth (RFC 9728)
    // y el proxy transparente de `/authorize` + `/token` hacia
    // Supabase. Todo lo demás cae en el fallback de siempre.
    let router = Router::new()
        .route(
            "/.well-known/oauth-protected-resource",
            get(protected_resource_metadata_handler),
        )
        .route("/authorize", get(authorize_proxy_handler))
        .route("/token", post(token_proxy_handler))
        .fallback(mcp_handler);
    let app = ServiceBuilder::new().layer(VercelLayer::new()).service(router);
    run(app).await
}
