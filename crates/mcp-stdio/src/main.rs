//! Transporte stdio del servidor MCP de memoria.
//!
//! El transporte stdio de MCP es JSON delimitado por saltos de línea:
//! una petición por línea en stdin, una respuesta por línea en stdout.
//! stderr queda libre para logs (los clientes MCP lo ignoran).
//!
//! SOLID-S: este archivo SOLO hace E/S. Protocolo en `mcp-core`,
//! herramientas en `lib.rs`, datos en `memory-store`. Cambiar el
//! transporte a HTTP (hito 2) no toca nada más que este archivo.

#![forbid(unsafe_code)]

use mcp_stdio::MemoryTools;
use memory_model::{Budget, Principal};
use memory_store::InMemoryStore;
use std::io::{BufRead, Write};

fn main() {
    let budget = Budget::default();
    let tools = MemoryTools::new(InMemoryStore::new(), Principal::local_dev(), budget);
    let mut server = mcp_core::McpServer::new("okf-memory", env!("CARGO_PKG_VERSION"), tools);

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    // Leemos con un buffer propio para poder aplicar el presupuesto
    // ANTES de tener la línea entera en memoria: read_line ingenuo
    // asignaría sin límite ante una línea de 10 GB.
    let mut reader = stdin.lock();
    let mut line = String::new();

    loop {
        line.clear();
        match read_bounded_line(&mut reader, &mut line, budget.max_request_bytes) {
            Ok(0) => break, // EOF: el cliente cerró; salida limpia.
            Ok(_) => {}
            Err(ReadError::TooLong) => {
                // Respuesta de error JSON-RPC con id null y a seguir.
                let msg = format!(
                    r#"{{"jsonrpc":"2.0","id":null,"error":{{"code":-32600,"message":"petición mayor que {} bytes"}}}}"#,
                    budget.max_request_bytes
                );
                if writeln!(out, "{msg}").and_then(|_| out.flush()).is_err() {
                    break;
                }
                continue;
            }
            Err(ReadError::Io(e)) => {
                eprintln!("okf-memory: error de lectura: {e}");
                break;
            }
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(response) = server.handle_message(trimmed) {
            if writeln!(out, "{response}").and_then(|_| out.flush()).is_err() {
                break; // stdout cerrado: no hay a quién responder.
            }
        }
    }
}

enum ReadError {
    TooLong,
    Io(std::io::Error),
}

/// Lee una línea con tope de bytes. Si la línea supera `max`, consume
/// hasta el '\n' (para resincronizar) y devuelve `TooLong`.
///
/// Detalle importante: acumulamos BYTES y validamos UTF-8 una sola
/// vez al final. Validar trozo a trozo fallaría cuando un carácter
/// multibyte cae justo en el límite del buffer interno (8 KiB).
fn read_bounded_line<R: BufRead>(
    reader: &mut R,
    line: &mut String,
    max: usize,
) -> Result<usize, ReadError> {
    let mut bytes: Vec<u8> = Vec::new();
    let mut overflowed = false;
    loop {
        let available = match reader.fill_buf() {
            Ok(buf) => buf,
            Err(e) => return Err(ReadError::Io(e)),
        };
        let at_eof = available.is_empty();
        let newline = available.iter().position(|&b| b == b'\n');
        let take = newline.map(|p| p + 1).unwrap_or(available.len());

        if !overflowed {
            if bytes.len() + take > max {
                overflowed = true;
                bytes.clear();
            } else {
                bytes.extend_from_slice(&available[..take]);
            }
        }
        reader.consume(take);

        if newline.is_some() || at_eof {
            if overflowed {
                return Err(ReadError::TooLong);
            }
            let total = bytes.len();
            // UTF-8 roto es un error del cliente, no un pánico nuestro:
            // lo tratamos como línea inválida (demasiado rara).
            match String::from_utf8(std::mem::take(&mut bytes)) {
                Ok(text) => {
                    line.push_str(&text);
                    return Ok(total);
                }
                Err(_) => return Err(ReadError::TooLong),
            }
        }
    }
}
