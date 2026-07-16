//! Binario del transporte HTTP: sirve `POST /mcp` sobre `TcpListener`.
//!
//! Uso local:
//! ```text
//! PORT=8787 ALLOWED_ORIGINS=https://claude.ai cargo run -p mcp-http
//! ```
//!
//! Este binario es el arnés del hito 2 para probar `mcp_http::route`
//! con sockets reales, en tu máquina, SIN Vercel ni Supabase. El
//! adaptador `vercel-entry` (más adelante, con `tokio` +
//! `vercel_runtime`) reutilizará esa misma función sin cambiar una
//! línea de la lógica de protocolo.

#![forbid(unsafe_code)]

use memory_model::{Budget, Principal};
use memory_store::InMemoryStore;
use mcp_stdio::MemoryTools;
use std::net::TcpListener;

fn main() {
    let port: u16 = std::env::var("PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(8787);
    let allowed_origins: Vec<String> = std::env::var("ALLOWED_ORIGINS")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();

    if allowed_origins.is_empty() {
        eprintln!(
            "okf-memory-http: ALLOWED_ORIGINS no está configurado; se aceptará cualquier \
             Origin. Vale para desarrollo local; en producción DEBES fijarlo."
        );
    }

    let budget = Budget::default();
    let tools = MemoryTools::new(InMemoryStore::new(), Principal::local_dev(), budget);
    let mcp_server = mcp_core::McpServer::new("okf-memory-http", env!("CARGO_PKG_VERSION"), tools);

    let addr = format!("127.0.0.1:{port}");
    let listener = TcpListener::bind(&addr).unwrap_or_else(|e| {
        eprintln!("okf-memory-http: no se pudo escuchar en {addr}: {e}");
        std::process::exit(1);
    });
    eprintln!("okf-memory-http: escuchando en http://{addr}/mcp");

    mcp_http::server::serve_forever(listener, budget, allowed_origins, mcp_server);
}
