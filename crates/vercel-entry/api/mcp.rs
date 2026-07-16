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

use axum::body::Bytes;
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Router;
use mcp_core::McpServer;
use mcp_http::{route, HttpRequest};
use mcp_stdio::MemoryTools;
use memory_model::{Budget, Principal};
use memory_store::InMemoryStore;
use std::sync::{Mutex, OnceLock};
use tower::ServiceBuilder;
use vercel_runtime::axum::VercelLayer;
use vercel_runtime::{run, Error};

// ADVERTENCIA DE ARQUITECTURA, no un descuido: las funciones de
// Vercel son efímeras (una instancia de proceso puede morir entre
// invocaciones). Este `OnceLock` es un caché "mejor esfuerzo" para
// instancias CALIENTES — jamás una fuente de verdad de sesión (ver
// el plan del proyecto: "no guardes sesiones en una LRU en
// memoria"). El almacén REAL en producción vive en Supabase; hasta
// que exista ese adaptador, cada instancia FRÍA empieza con memoria
// vacía. Documentado, no oculto: no hay persistencia real todavía.
fn shared_server() -> &'static Mutex<McpServer<MemoryTools<InMemoryStore>>> {
    static SERVER: OnceLock<Mutex<McpServer<MemoryTools<InMemoryStore>>>> = OnceLock::new();
    SERVER.get_or_init(|| {
        let budget = Budget::default();
        let tools = MemoryTools::new(InMemoryStore::new(), Principal::local_dev(), budget);
        Mutex::new(McpServer::new("okf-memory-vercel", env!("CARGO_PKG_VERSION"), tools))
    })
}

/// Lista separada por comas en la variable de entorno
/// `ALLOWED_ORIGINS` del proyecto Vercel. Vacía por defecto — igual
/// que `mcp-http` local, pero aquí SÍ importa fijarla antes de un
/// despliegue real (ver ejercicio 2 del capítulo 10 del tutorial).
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
/// defecto) — este archivo ES semánticamente el endpoint `/mcp`, así
/// que sintetizamos esa ruta al construir la petición interna en vez
/// de depender de cómo Vercel exponga la ruta real dentro de la
/// función (detalle de la plataforma que no queremos acoplar aquí).
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

    let mut server = shared_server()
        .lock()
        .expect("route() nunca hace panic: el mutex no puede envenenarse");
    let resp = route(&http_req, &budget, &origins, &mut server);

    let status = StatusCode::from_u16(resp.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, [("content-type", resp.content_type)], resp.body).into_response()
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    // Sin rutas registradas: TODA petición (cualquier método, ANY
    // path que Vercel le entregue a esta función) cae en el
    // fallback. `route()` es quien decide 405/403/404 desde ahí —
    // el router de axum no duplica esa lógica.
    let router = Router::new().fallback(mcp_handler);
    let app = ServiceBuilder::new().layer(VercelLayer::new()).service(router);
    run(app).await
}
