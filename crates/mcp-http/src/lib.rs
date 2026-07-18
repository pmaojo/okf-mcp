//! HTTP Streamable (MCP) sin estado, en `std` puro.
//!
//! Este crate es DELIBERADAMENTE dos cosas a la vez:
//!
//! 1. La especificación ejecutable de cómo debe comportarse el
//!    endpoint `/mcp` — código puro, sin sockets, testeado sin red.
//! 2. Un servidor real sobre `TcpListener` que sirve esa
//!    especificación localmente, para poder probar el hito 2 de
//!    principio a fin ANTES de añadir `tokio` + `vercel_runtime`
//!    (que solo entrarán en el crate adaptador `vercel-entry`).
//!
//! Cuando llegue ese adaptador, la única pieza que cambia es el
//! transporte (`server.rs`); [`route`] no se toca: es el contrato.
//!
//! SOLID en juego (ver `docs/09` del tutorial):
//! - **S:** `route` decide QUÉ responder; `server.rs` decide CÓMO
//!   leer bytes de un socket. Un archivo no sabe del otro.
//! - **D:** `route` es genérico sobre `ToolHandler`, igual que
//!   `mcp-core`. El endpoint HTTP no conoce `MemoryTools` ni
//!   `InMemoryStore`; los recibe inyectados desde `main.rs`.

#![forbid(unsafe_code)]

pub mod server;

use mcp_core::{McpServer, ToolHandler};
use memory_model::Budget;

/// Petición HTTP ya parseada (independiente del transporte real).
#[derive(Debug, Clone)]
pub struct HttpRequest {
    pub method: String,
    pub path: String,
    pub origin: Option<String>,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
}

/// Respuesta HTTP a serializar por el transporte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub content_type: &'static str,
    pub body: String,
}

impl HttpResponse {
    fn json(status: u16, body: String) -> Self {
        HttpResponse { status, content_type: "application/json", body }
    }
    fn empty(status: u16) -> Self {
        HttpResponse { status, content_type: "text/plain", body: String::new() }
    }
    fn text(status: u16, body: &str) -> Self {
        HttpResponse { status, content_type: "text/plain", body: body.to_string() }
    }
}

/// Decide y ejecuta la respuesta para una petición a `/mcp`.
///
/// Reglas, en el orden en que MCP Streamable HTTP y el plan de este
/// proyecto las exigen — cada una devuelve temprano, así que el
/// orden ES la prioridad:
///
/// 1. Solo existe la ruta `/mcp`. Cualquier otra → 404.
/// 2. Si el cliente envía `Origin` y no está en la lista permitida,
///    403. (Protección DNS-rebinding: los navegadores SIEMPRE
///    mandan `Origin`; los SDK de agentes normalmente no, así que
///    su ausencia no se penaliza.)
/// 3. `GET` y `DELETE` → 405 (no ofrecemos SSE servidor→cliente ni
///    sesiones que cerrar: el servidor es sin estado).
/// 4. Cualquier método que no sea `POST` → 405.
/// 5. El cuerpo debe caber en `budget.max_request_bytes` → si no, 413.
/// 6. `Content-Type` debe ser `application/json` → si no, 415.
/// 7. El cuerpo debe ser UTF-8 → si no, 400.
/// 8. Se delega en `mcp_core::McpServer::handle_message`. Una
///    notificación (sin id) no produce respuesta: 202 Accepted con
///    cuerpo vacío, tal como pide JSON-RPC sobre HTTP.
pub fn route<H: ToolHandler>(
    req: &HttpRequest,
    budget: &Budget,
    allowed_origins: &[String],
    server: &mut McpServer<H>,
) -> HttpResponse {
    if req.path != "/mcp" {
        return HttpResponse::text(404, "ruta desconocida: solo existe /mcp");
    }

    if let Some(origin) = &req.origin {
        if !allowed_origins.is_empty() && !allowed_origins.iter().any(|o| o == origin) {
            return HttpResponse::text(403, "origin no permitido");
        }
    }

    match req.method.as_str() {
        "GET" | "DELETE" => {
            return HttpResponse::text(
                405,
                "este servidor no ofrece stream servidor->cliente ni sesiones: usa POST",
            );
        }
        "POST" => {}
        _ => return HttpResponse::text(405, "método no soportado"),
    }

    if req.body.len() > budget.max_request_bytes {
        return HttpResponse::text(
            413,
            &format!("el cuerpo supera el máximo de {} bytes", budget.max_request_bytes),
        );
    }

    let is_json = req
        .content_type
        .as_deref()
        .map(|ct| ct.split(';').next().unwrap_or("").trim() == "application/json")
        .unwrap_or(false);
    if !is_json {
        return HttpResponse::text(415, "Content-Type debe ser application/json");
    }

    let text = match std::str::from_utf8(&req.body) {
        Ok(t) => t,
        Err(_) => return HttpResponse::text(400, "el cuerpo no es UTF-8 válido"),
    };

    match server.handle_message(text) {
        Some(response) => HttpResponse::json(200, response),
        // Notificación: JSON-RPC no espera respuesta. 202 es la
        // convención para "aceptado, sin contenido que devolver".
        None => HttpResponse::empty(202),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use memory_tools::MemoryTools;
    use memory_model::Principal;
    use memory_store::InMemoryStore;

    fn server() -> McpServer<MemoryTools<InMemoryStore>> {
        let tools = MemoryTools::new(InMemoryStore::new(), Principal::local_dev(), Budget::default());
        McpServer::new("okf-memory-http", "test", tools)
    }

    fn req(method: &str, path: &str, body: &str) -> HttpRequest {
        HttpRequest {
            method: method.to_string(),
            path: path.to_string(),
            origin: None,
            content_type: Some("application/json".to_string()),
            body: body.as_bytes().to_vec(),
        }
    }

    #[test]
    fn ruta_desconocida_es_404() {
        let mut s = server();
        let r = route(&req("POST", "/otra", "{}"), &Budget::default(), &[], &mut s);
        assert_eq!(r.status, 404);
    }

    #[test]
    fn get_y_delete_son_405() {
        let mut s = server();
        for m in ["GET", "DELETE"] {
            let r = route(&req(m, "/mcp", ""), &Budget::default(), &[], &mut s);
            assert_eq!(r.status, 405, "método {m}");
        }
    }

    #[test]
    fn origin_no_permitido_es_403() {
        let mut s = server();
        let mut r = req("POST", "/mcp", "{}");
        r.origin = Some("https://evil.example".to_string());
        let allowed = vec!["https://claude.ai".to_string()];
        let resp = route(&r, &Budget::default(), &allowed, &mut s);
        assert_eq!(resp.status, 403);
    }

    #[test]
    fn sin_origin_pasa_aunque_haya_lista_permitida() {
        let mut s = server();
        let r = req("POST", "/mcp", r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#);
        let allowed = vec!["https://claude.ai".to_string()];
        let resp = route(&r, &Budget::default(), &allowed, &mut s);
        assert_eq!(resp.status, 200, "clientes no-navegador no mandan Origin");
    }

    #[test]
    fn content_type_incorrecto_es_415() {
        let mut s = server();
        let mut r = req("POST", "/mcp", "{}");
        r.content_type = Some("text/plain".to_string());
        let resp = route(&r, &Budget::default(), &[], &mut s);
        assert_eq!(resp.status, 415);
    }

    #[test]
    fn cuerpo_demasiado_grande_es_413() {
        let mut s = server();
        let budget = Budget { max_request_bytes: 10, ..Budget::default() };
        let r = req("POST", "/mcp", "0123456789ABCDEF");
        let resp = route(&r, &budget, &[], &mut s);
        assert_eq!(resp.status, 413);
    }

    #[test]
    fn utf8_invalido_es_400() {
        let mut s = server();
        let mut r = req("POST", "/mcp", "");
        r.body = vec![0xff, 0xfe, 0xfd];
        let resp = route(&r, &Budget::default(), &[], &mut s);
        assert_eq!(resp.status, 400);
    }

    #[test]
    fn notificacion_devuelve_202_sin_cuerpo() {
        let mut s = server();
        let r = req("POST", "/mcp", r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
        let resp = route(&r, &Budget::default(), &[], &mut s);
        assert_eq!(resp.status, 202);
        assert!(resp.body.is_empty());
    }

    #[test]
    fn peticion_valida_ejecuta_una_herramienta() {
        let mut s = server();
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"memory_commit","arguments":{"concept_id":"n","reason":"x","markdown":"---\ntype: note\n---\nhola\n"}}}"#;
        let resp = route(&req("POST", "/mcp", body), &Budget::default(), &[], &mut s);
        assert_eq!(resp.status, 200);

        // El payload de la herramienta viaja como texto JSON DENTRO
        // del JSON-RPC (doblemente codificado); lo parseamos en vez
        // de buscar substrings escapados a mano.
        let outer = json_mini::parse(&resp.body).unwrap();
        let text = outer
            .get("result").unwrap()
            .get("content").unwrap()
            .as_array().unwrap()[0]
            .get("text").unwrap()
            .as_str().unwrap();
        let payload = json_mini::parse(text).unwrap();
        assert_eq!(payload.get("created"), Some(&json_mini::Value::Bool(true)));
    }
}
