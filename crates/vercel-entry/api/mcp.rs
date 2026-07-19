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
//!
//! El pool de base de datos vive en [`vercel_entry::db`] (compartido
//! con `api/outbox.rs`) y el proxy del Authorization Server OAuth
//! (metadata RFC 9728, `/authorize`, `/token`, pantalla de
//! consentimiento) vive en [`vercel_entry::oauth_proxy`] — ninguno de
//! los dos es parte del bridge MCP, solo comparten despliegue con él.

#![forbid(unsafe_code)]

use vercel_entry::{auth, db, oauth_proxy};

use axum::body::Bytes;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use graph_core::NeighborSource;
use mcp_core::{McpServer, ToolHandler};
use mcp_http::{route, HttpRequest};
use memory_tools::MemoryTools;
use memory_model::{Budget, Principal};
use std::convert::Infallible;
use store_core::{MemoryRepository, StoreMaintenance};
use supabase_store::SupabaseStore;
use tower::ServiceBuilder;
use tower_http::cors::{AllowHeaders, AllowOrigin, CorsLayer};
use vercel_runtime::axum::VercelLayer;
use vercel_runtime::{run, Error};

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

/// Cabeceras CORS (incluye la respuesta al preflight `OPTIONS`), sin
/// las cuales un cliente MCP que llame desde el navegador (Claude.ai
/// hace `fetch` directo al servidor MCP, no vía su backend) nunca
/// llega a ver la respuesta: el navegador corta la petición en el
/// preflight antes de que exista Authorization que validar. Reutiliza
/// la misma lista de `ALLOWED_ORIGINS` que ya usa `mcp_http::route`
/// para la comprobación anti DNS-rebinding — misma semántica: lista
/// vacía es permisivo, lista no vacía restringe.
fn cors_layer() -> CorsLayer {
    let origins = allowed_origins();
    CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(move |origin: &HeaderValue, _| {
            origins.is_empty()
                || origin
                    .to_str()
                    .map(|o| origins.iter().any(|allowed| allowed == o))
                    .unwrap_or(false)
        }))
        .allow_methods([Method::GET, Method::POST, Method::DELETE])
        .allow_headers(AllowHeaders::mirror_request())
}

/// Owners de fuentes de `skill_ingest` cuyo escrutinio de contenido
/// sospechoso se omite en la respuesta (repos ya revisados por su
/// cuenta, p. ej. los oficiales de Anthropic). Lista separada por
/// comas en `SKILL_INGEST_TRUSTED_OWNERS`; por defecto solo
/// `anthropics`.
fn trusted_owners() -> Vec<String> {
    std::env::var("SKILL_INGEST_TRUSTED_OWNERS")
        .ok()
        .filter(|v| !v.is_empty())
        .map(|v| v.split(',').map(|o| o.trim().to_string()).filter(|o| !o.is_empty()).collect())
        .unwrap_or_else(|| vec!["anthropics".to_string()])
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
            // el token fue emitido para ESTE cliente OAuth. Supabase Auth
            // firma todo JWT con `aud: "authenticated"` (es el rol de
            // Postgres, no identifica al cliente) — lo que identifica al
            // cliente es el claim `client_id`, que es lo que
            // `validate_jwt` compara contra JWT_AUDIENCE si está presente.
            //
            // Con el registro dinámico de clientes (DCR) activado en el
            // OAuth Server de Supabase, Claude puede auto-registrarse con
            // un `client_id` nuevo en cualquier momento — fijar
            // JWT_AUDIENCE a un único cliente pre-registrado rompería la
            // conexión cada vez que eso pasa. Por eso JWT_AUDIENCE es
            // opcional: si está configurada, restringe a ese client_id
            // (útil en despliegues sin DCR, con un cliente fijo); si no,
            // se acepta cualquier token válido firmado por este Supabase
            // Auth — la seguridad real la da la sesión de usuario (login)
            // más la aprobación explícita en la pantalla de consentimiento
            // de `/oauth/consent`, no el client_id.
            let audience = std::env::var("JWT_AUDIENCE")
                .ok()
                .filter(|v| !v.is_empty());
            match auth::validate_jwt(auth_header, &jwks_url, audience.as_deref()).await {
                Ok(principal) => principal,
                Err(err) => {
                    // Cabecera exigida por la especificación de autorización
                    // de MCP: le dice al cliente dónde encontrar la metadata
                    // del recurso protegido (RFC 9728) para iniciar el flujo
                    // OAuth en lugar de fallar en silencio.
                    let metadata_url = format!(
                        "{}/.well-known/oauth-protected-resource",
                        oauth_proxy::base_url(&headers)
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

    // 2. Seleccionar backend e instanciar servicios
    let store_kind = std::env::var("OKF_STORE").unwrap_or_else(|_| "supabase".into());
    match store_kind.as_str() {
        "github" => {
            let gh_store = match github_store::GithubStore::from_env() {
                Ok(s) => s,
                Err(e) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        [("content-type", "text/plain")],
                        format!("github-store init error: {e}"),
                    )
                        .into_response();
                }
            };
            if std::env::var("POSTGRES_URL").is_ok() {
                let db_url = std::env::var("POSTGRES_URL").unwrap();
                let pool = db::get_db_pool(&db_url).await;
                let db_store = SupabaseStore::new(pool, gemini_embeddings::EmbeddingKeys::from_env());
                let store = store_core::IndexedStore::new(gh_store, db_store);

                let tools = MemoryTools::new(store, actor, budget)
                    .with_ingest(Box::new(ingest_http::GithubFetcher::from_env()), trusted_owners());
                handle_mcp(&http_req, &budget, &origins, tools)
            } else {
                let tools = MemoryTools::new(gh_store, actor, budget);
                handle_mcp(&http_req, &budget, &origins, tools)
            }
        }
        _ => {
            let db_url = std::env::var("POSTGRES_URL").expect("POSTGRES_URL must be set");
            let pool = db::get_db_pool(&db_url).await;
            let store = SupabaseStore::new(pool, gemini_embeddings::EmbeddingKeys::from_env());
            // `skill_ingest`: descarga server-side desde GitHub y siempre
            // conserva el contenido original íntegro bajo cabecera OKF
            // generada — sin síntesis con LLM (ver `ingest_core`/`ingest_http`).
            let tools = MemoryTools::new(store, actor, budget)
                .with_ingest(Box::new(ingest_http::GithubFetcher::from_env()), trusted_owners());
            handle_mcp(&http_req, &budget, &origins, tools)
        }
    }
}

/// Ejecuta la petición MCP contra un `MemoryTools<R>` genérico.
fn handle_mcp<R>(
    http_req: &HttpRequest,
    budget: &Budget,
    origins: &[String],
    tools: MemoryTools<R>,
) -> Response
where
    R: MemoryRepository + StoreMaintenance + NeighborSource<Error = Infallible>,
    MemoryTools<R>: ToolHandler,
{
    let mut server = McpServer::new("okf-memory-vercel", env!("CARGO_PKG_VERSION"), tools);
    let resp = route(http_req, budget, origins, &mut server);
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
            get(oauth_proxy::protected_resource_metadata_handler),
        )
        .route("/authorize", get(oauth_proxy::authorize_proxy_handler))
        .route("/token", post(oauth_proxy::token_proxy_handler))
        .route("/oauth/consent", get(oauth_proxy::consent_page_handler))
        .fallback(mcp_handler)
        .layer(cors_layer());
    let app = ServiceBuilder::new().layer(VercelLayer::new()).service(router);
    run(app).await
}
