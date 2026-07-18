//! Transporte real: parsea HTTP/1.1 sobre `TcpStream` y llama a
//! [`crate::route`]. Todo el análisis de bytes es acotado, siguiendo
//! el mismo patrón que `mcp-stdio` (capítulo 8 del tutorial): nunca
//! asignamos según lo que el cliente DICE que va a enviar, sino
//! según lo que el presupuesto permite.
//!
//! Deliberadamente MONOHILO: este servidor es el arnés local del
//! hito 2 para probar `route()` sobre sockets reales, no el modelo
//! de concurrencia de producción. En Vercel cada invocación HTTP la
//! atiende una instancia de función aislada — no hay un
//! `accept()` compartido que sincronizar. Cuando este servidor deba
//! atender clientes simultáneos en desarrollo, ver el ejercicio 2
//! del capítulo correspondiente del tutorial.

#![forbid(unsafe_code)]

use crate::{route, HttpRequest, HttpResponse};
use mcp_core::{McpServer, ToolHandler};
use memory_model::Budget;
use std::io::{BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};

/// Tope para la línea de petición y cada línea de cabecera.
const MAX_LINE_BYTES: usize = 8 * 1024;
/// Tope acumulado de TODAS las cabeceras de una petición.
const MAX_HEADER_BYTES: usize = 16 * 1024;

/// Acepta conexiones indefinidamente. Un fallo en una conexión se
/// registra en stderr; el servidor sigue con la siguiente — un
/// cliente roto no debe tumbar el proceso.
pub fn serve_forever<H: ToolHandler>(
    listener: TcpListener,
    budget: Budget,
    allowed_origins: Vec<String>,
    mut server: McpServer<H>,
) -> ! {
    loop {
        match listener.accept() {
            Ok((stream, _addr)) => {
                if let Err(e) = handle_connection(stream, &budget, &allowed_origins, &mut server) {
                    eprintln!("okf-memory-http: error en la conexión: {e}");
                }
            }
            Err(e) => eprintln!("okf-memory-http: error al aceptar: {e}"),
        }
    }
}

/// Procesa EXACTAMENTE una petición HTTP/1.1 sobre `stream` y escribe
/// la respuesta. Público para poder testear contra un `TcpStream`
/// real sin arrancar el bucle `accept()` infinito.
pub fn handle_connection<H: ToolHandler>(
    stream: TcpStream,
    budget: &Budget,
    allowed_origins: &[String],
    server: &mut McpServer<H>,
) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = stream;

    let response = match read_http_request(&mut reader, budget) {
        Ok(request) => route(&request, budget, allowed_origins, server),
        Err(ParseFailure::Io(e)) => return Err(e),
        Err(ParseFailure::Rejected(status, body)) => {
            HttpResponse { status, content_type: "text/plain", body }
        }
        Err(ParseFailure::BodyTooLarge(max)) => HttpResponse {
            status: 413,
            content_type: "text/plain",
            body: format!("el cuerpo declarado supera el máximo de {max} bytes"),
        },
    };

    write_response(&mut writer, &response)
}

enum ParseFailure {
    /// Petición mal formada o con una línea fuera de tamaño: 400.
    Rejected(u16, String),
    /// `Content-Length` supera el presupuesto: 413, SIN leer el
    /// cuerpo. Este es el caso que demuestra por qué comprobamos
    /// antes de asignar (ver capítulo 8, sección 5 del tutorial).
    BodyTooLarge(usize),
    Io(std::io::Error),
}

impl From<std::io::Error> for ParseFailure {
    fn from(e: std::io::Error) -> Self {
        ParseFailure::Io(e)
    }
}

fn rejected(msg: &str) -> ParseFailure {
    ParseFailure::Rejected(400, msg.to_string())
}

fn read_http_request<R: Read>(
    reader: &mut BufReader<R>,
    budget: &Budget,
) -> Result<HttpRequest, ParseFailure> {
    let request_line = read_bounded_line(reader, MAX_LINE_BYTES)
        .map_err(|_| rejected("línea de petición inválida o demasiado larga"))?;
    let mut parts = request_line.split(' ');
    let method = parts.next().ok_or_else(|| rejected("falta el método HTTP"))?.to_string();
    let raw_path = parts.next().ok_or_else(|| rejected("falta la ruta"))?;
    let path = raw_path.split('?').next().unwrap_or(raw_path).to_string();

    let mut content_type = None;
    let mut origin = None;
    let mut content_length: usize = 0;
    let mut header_bytes = 0usize;

    loop {
        let remaining = MAX_HEADER_BYTES.saturating_sub(header_bytes).min(MAX_LINE_BYTES);
        let line = read_bounded_line(reader, remaining)
            .map_err(|_| ParseFailure::Rejected(431, "cabeceras demasiado grandes o inválidas".to_string()))?;
        header_bytes += line.len() + 2; // +2 por el CRLF ya consumido
        if line.is_empty() {
            break; // línea en blanco: fin de cabeceras
        }
        if let Some((name, value)) = line.split_once(':') {
            match name.trim().to_ascii_lowercase().as_str() {
                "content-type" => content_type = Some(value.trim().to_string()),
                "origin" => origin = Some(value.trim().to_string()),
                "content-length" => {
                    content_length = value
                        .trim()
                        .parse()
                        .map_err(|_| rejected("Content-Length no es un entero válido"))?;
                }
                _ => {}
            }
        }
    }

    // El corte de presupuesto ocurre ANTES de asignar el buffer del
    // cuerpo: una cabecera que promete 4 GiB no nos hace reservar
    // 4 GiB para luego fallar. Fiel al invariante del capítulo 8.
    if content_length > budget.max_request_bytes {
        return Err(ParseFailure::BodyTooLarge(budget.max_request_bytes));
    }

    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body)?;

    Ok(HttpRequest { method, path, origin, content_type, body })
}

/// Lee una línea terminada en `\n` (tolerando `\r\n`), acotada a
/// `max` bytes. Falla si se supera el límite o si no es UTF-8.
fn read_bounded_line<R: Read>(reader: &mut BufReader<R>, max: usize) -> std::io::Result<String> {
    let mut bytes = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        reader.read_exact(&mut byte)?;
        if byte[0] == b'\n' {
            if bytes.last() == Some(&b'\r') {
                bytes.pop();
            }
            break;
        }
        bytes.push(byte[0]);
        if bytes.len() > max {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "línea demasiado larga"));
        }
    }
    String::from_utf8(bytes).map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "UTF-8 inválido"))
}

fn write_response(stream: &mut TcpStream, resp: &HttpResponse) -> std::io::Result<()> {
    let header = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        resp.status,
        reason_phrase(resp.status),
        resp.content_type,
        resp.body.len(),
    );
    stream.write_all(header.as_bytes())?;
    stream.write_all(resp.body.as_bytes())?;
    stream.flush()
}

fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        415 => "Unsupported Media Type",
        431 => "Request Header Fields Too Large",
        _ => "Error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use memory_tools::MemoryTools;
    use memory_model::Principal;
    use memory_store::InMemoryStore;
    use std::net::Shutdown;

    fn server() -> McpServer<MemoryTools<InMemoryStore>> {
        let tools = MemoryTools::new(InMemoryStore::new(), Principal::local_dev(), Budget::default());
        McpServer::new("test", "0", tools)
    }

    #[test]
    fn peticion_http_real_de_extremo_a_extremo() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let client = std::thread::spawn(move || {
            let mut stream = TcpStream::connect(addr).unwrap();
            let body = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
            let request = format!(
                "POST /mcp HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(request.as_bytes()).unwrap();
            stream.shutdown(Shutdown::Write).unwrap();
            let mut resp = String::new();
            stream.read_to_string(&mut resp).unwrap();
            resp
        });

        let (stream, _) = listener.accept().unwrap();
        let mut srv = server();
        handle_connection(stream, &Budget::default(), &[], &mut srv).unwrap();

        let resp = client.join().unwrap();
        assert!(resp.starts_with("HTTP/1.1 200"), "resp: {resp}");
        assert!(resp.contains("\"result\""));
    }

    #[test]
    fn content_length_que_excede_el_presupuesto_no_lee_el_cuerpo() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let client = std::thread::spawn(move || {
            let mut stream = TcpStream::connect(addr).unwrap();
            // Prometemos un cuerpo enorme pero NUNCA lo enviamos: si
            // el servidor intentara leerlo, este test se colgaría.
            let request = "POST /mcp HTTP/1.1\r\nContent-Type: application/json\r\nContent-Length: 999999999\r\n\r\n";
            stream.write_all(request.as_bytes()).unwrap();
            stream.shutdown(Shutdown::Write).unwrap();
            let mut resp = String::new();
            let _ = stream.read_to_string(&mut resp);
            resp
        });

        let (stream, _) = listener.accept().unwrap();
        let mut srv = server();
        let budget = Budget { max_request_bytes: 1024, ..Budget::default() };
        handle_connection(stream, &budget, &[], &mut srv).unwrap();

        let resp = client.join().unwrap();
        assert!(resp.starts_with("HTTP/1.1 413"), "resp: {resp}");
    }

    #[test]
    fn metodo_get_es_405_sobre_socket_real() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let client = std::thread::spawn(move || {
            let mut stream = TcpStream::connect(addr).unwrap();
            stream.write_all(b"GET /mcp HTTP/1.1\r\nHost: localhost\r\n\r\n").unwrap();
            stream.shutdown(Shutdown::Write).unwrap();
            let mut resp = String::new();
            stream.read_to_string(&mut resp).unwrap();
            resp
        });

        let (stream, _) = listener.accept().unwrap();
        let mut srv = server();
        handle_connection(stream, &Budget::default(), &[], &mut srv).unwrap();

        assert!(client.join().unwrap().starts_with("HTTP/1.1 405"));
    }
}
