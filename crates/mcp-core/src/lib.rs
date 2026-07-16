//! Núcleo del protocolo MCP: JSON-RPC 2.0 + ciclo de vida + despacho
//! de herramientas.
//!
//! SOLID en juego:
//! - **D:** el servidor no conoce ninguna herramienta concreta.
//!   Depende del trait [`ToolHandler`]; las herramientas de memoria
//!   viven en otro crate y se inyectan.
//! - **O:** añadir una herramienta = implementar una entrada más en
//!   el handler. Este crate no se toca.
//! - **S:** aquí no hay E/S. `handle_message(&str) -> Option<String>`
//!   es una función sobre strings; el transporte (stdio hoy, HTTP en
//!   el hito 2) vive fuera. Por eso el MISMO crate servirá para
//!   Vercel sin cambios.
//!
//! Este crate es agnóstico del transporte pero no del formato: usa
//! `json-mini` (interno, solo std). En el hito 2 el endpoint público
//! usará `serde_json` en el adaptador `json-wire`.

#![forbid(unsafe_code)]

use json_mini::{arr, obj, s, Value};

/// Versión del protocolo que implementamos.
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// Códigos de error JSON-RPC 2.0.
pub mod code {
    pub const PARSE_ERROR: f64 = -32700.0;
    pub const INVALID_REQUEST: f64 = -32600.0;
    pub const METHOD_NOT_FOUND: f64 = -32601.0;
    pub const INVALID_PARAMS: f64 = -32602.0;
    pub const INTERNAL_ERROR: f64 = -32603.0;
}

/// Descripción de una herramienta para `tools/list`.
#[derive(Debug, Clone)]
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    /// JSON Schema del parámetro `arguments`.
    pub input_schema: Value,
}

/// Fallos al invocar una herramienta.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolError {
    /// No existe la herramienta (error de protocolo → -32602).
    UnknownTool,
    /// Argumentos mal formados (error de protocolo → -32602).
    InvalidArguments(String),
    /// La herramienta se ejecutó y falló (NO es error de protocolo:
    /// se devuelve como resultado con `isError: true`, para que el
    /// modelo pueda leer el fallo y reaccionar).
    Failed(String),
}

/// El contrato entre el protocolo y las herramientas.
pub trait ToolHandler {
    fn tools(&self) -> Vec<ToolSpec>;
    fn call(&mut self, name: &str, arguments: &Value) -> Result<Value, ToolError>;
}

/// Estado del ciclo de vida MCP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lifecycle {
    AwaitingInitialize,
    Initializing,
    Ready,
}

/// Servidor MCP independiente del transporte.
pub struct McpServer<H: ToolHandler> {
    handler: H,
    server_name: &'static str,
    server_version: &'static str,
    lifecycle: Lifecycle,
}

impl<H: ToolHandler> McpServer<H> {
    pub fn new(server_name: &'static str, server_version: &'static str, handler: H) -> Self {
        McpServer {
            handler,
            server_name,
            server_version,
            lifecycle: Lifecycle::AwaitingInitialize,
        }
    }

    /// Procesa un mensaje JSON-RPC en crudo. Devuelve la respuesta
    /// serializada, o `None` si era una notificación (las
    /// notificaciones jamás se responden, ni siquiera con errores).
    pub fn handle_message(&mut self, raw: &str) -> Option<String> {
        let value = match json_mini::parse(raw) {
            Ok(v) => v,
            Err(e) => {
                return Some(json_mini::to_string(&error_response(
                    Value::Null,
                    code::PARSE_ERROR,
                    &format!("JSON inválido: {e}"),
                )));
            }
        };

        let id = value.get("id").cloned();
        let method = value.get("method").and_then(|m| m.as_str()).map(str::to_string);

        match (id, method) {
            // Notificación: método sin id.
            (None, Some(method)) => {
                self.handle_notification(&method);
                None
            }
            // Petición: método con id.
            (Some(id), Some(method)) => {
                if !matches!(id, Value::String(_) | Value::Number(_)) {
                    return Some(json_mini::to_string(&error_response(
                        Value::Null,
                        code::INVALID_REQUEST,
                        "id debe ser string o número",
                    )));
                }
                let params = value.get("params").cloned().unwrap_or(Value::Null);
                let response = self.handle_request(id, &method, &params);
                Some(json_mini::to_string(&response))
            }
            // Respuesta de cliente u objeto sin método.
            (id, None) => {
                let id = id.unwrap_or(Value::Null);
                Some(json_mini::to_string(&error_response(
                    id,
                    code::INVALID_REQUEST,
                    "falta el campo 'method'",
                )))
            }
        }
    }

    fn handle_notification(&mut self, method: &str) {
        if method == "notifications/initialized" && self.lifecycle == Lifecycle::Initializing {
            self.lifecycle = Lifecycle::Ready;
        }
        // El resto de notificaciones se ignoran deliberadamente.
    }

    fn handle_request(&mut self, id: Value, method: &str, params: &Value) -> Value {
        match method {
            "initialize" => self.on_initialize(id, params),
            "ping" => ok_response(id, obj([])),
            "tools/list" => self.on_tools_list(id),
            "tools/call" => self.on_tools_call(id, params),
            _ => error_response(id, code::METHOD_NOT_FOUND, &format!("método desconocido: {method}")),
        }
    }

    fn on_initialize(&mut self, id: Value, params: &Value) -> Value {
        // Negociación de versión: si el cliente propone una que
        // conocemos, la aceptamos; si no, contestamos con la nuestra
        // (el cliente decide entonces si puede seguir).
        let requested = params.get("protocolVersion").and_then(|v| v.as_str());
        let version = match requested {
            Some(v) if v == PROTOCOL_VERSION || v == "2025-03-26" || v == "2024-11-05" => v,
            _ => PROTOCOL_VERSION,
        };
        self.lifecycle = Lifecycle::Initializing;
        ok_response(
            id,
            obj([
                ("protocolVersion", s(version)),
                ("capabilities", obj([("tools", obj([]))])),
                (
                    "serverInfo",
                    obj([
                        ("name", s(self.server_name)),
                        ("version", s(self.server_version)),
                    ]),
                ),
            ]),
        )
    }

    fn on_tools_list(&mut self, id: Value) -> Value {
        let tools: Vec<Value> = self
            .handler
            .tools()
            .into_iter()
            .map(|t| {
                obj([
                    ("name", s(t.name)),
                    ("description", s(t.description)),
                    ("inputSchema", t.input_schema),
                ])
            })
            .collect();
        ok_response(id, obj([("tools", arr(tools))]))
    }

    fn on_tools_call(&mut self, id: Value, params: &Value) -> Value {
        let Some(name) = params.get("name").and_then(|v| v.as_str()) else {
            return error_response(id, code::INVALID_PARAMS, "falta params.name");
        };
        let default_args = obj([]);
        let arguments = params.get("arguments").unwrap_or(&default_args);

        match self.handler.call(name, arguments) {
            Ok(result) => ok_response(id, tool_result(&json_mini::to_string(&result), false)),
            Err(ToolError::UnknownTool) => {
                error_response(id, code::INVALID_PARAMS, &format!("herramienta desconocida: {name}"))
            }
            Err(ToolError::InvalidArguments(msg)) => {
                error_response(id, code::INVALID_PARAMS, &msg)
            }
            // Fallo de dominio: respuesta correcta de protocolo con
            // isError=true. Así el MODELO ve el conflicto CAS y puede
            // releer y reintentar.
            Err(ToolError::Failed(msg)) => ok_response(id, tool_result(&msg, true)),
        }
    }
}

/// Resultado de herramienta según el esquema MCP `CallToolResult`.
fn tool_result(text: &str, is_error: bool) -> Value {
    obj([
        (
            "content",
            arr(vec![obj([("type", s("text")), ("text", s(text))])]),
        ),
        ("isError", Value::Bool(is_error)),
    ])
}

fn ok_response(id: Value, result: Value) -> Value {
    obj([("jsonrpc", s("2.0")), ("id", id), ("result", result)])
}

fn error_response(id: Value, code: f64, message: &str) -> Value {
    obj([
        ("jsonrpc", s("2.0")),
        ("id", id),
        (
            "error",
            obj([("code", Value::Number(code)), ("message", s(message))]),
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    struct EchoTools;

    impl ToolHandler for EchoTools {
        fn tools(&self) -> Vec<ToolSpec> {
            vec![ToolSpec {
                name: "echo",
                description: "devuelve lo recibido",
                input_schema: obj([("type", s("object"))]),
            }]
        }
        fn call(&mut self, name: &str, arguments: &Value) -> Result<Value, ToolError> {
            match name {
                "echo" => Ok(arguments.clone()),
                "explota" => Err(ToolError::Failed("bum controlado".to_string())),
                _ => Err(ToolError::UnknownTool),
            }
        }
    }

    fn server() -> McpServer<EchoTools> {
        McpServer::new("test", "0.0.0", EchoTools)
    }

    #[test]
    fn ciclo_de_vida_completo() {
        let mut srv = server();
        let resp = srv
            .handle_message(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}"#)
            .unwrap();
        assert!(resp.contains("\"protocolVersion\":\"2025-06-18\""));
        assert!(resp.contains("\"serverInfo\""));

        // La notificación no produce respuesta.
        assert!(srv
            .handle_message(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
            .is_none());

        let resp = srv
            .handle_message(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#)
            .unwrap();
        assert!(resp.contains("\"echo\""));

        let resp = srv
            .handle_message(r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"echo","arguments":{"x":1}}}"#)
            .unwrap();
        assert!(resp.contains("isError\":false"));
        assert!(resp.contains("{\\\"x\\\":1}"));
    }

    #[test]
    fn json_invalido_devuelve_parse_error() {
        let resp = server().handle_message("{rotísimo").unwrap();
        assert!(resp.contains("-32700"));
    }

    #[test]
    fn metodo_desconocido() {
        let resp = server()
            .handle_message(r#"{"jsonrpc":"2.0","id":1,"method":"no/existe"}"#)
            .unwrap();
        assert!(resp.contains("-32601"));
    }

    #[test]
    fn herramienta_desconocida_es_invalid_params() {
        let resp = server()
            .handle_message(r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"nada"}}"#)
            .unwrap();
        assert!(resp.contains("-32602"));
    }

    #[test]
    fn fallo_de_dominio_no_es_error_de_protocolo() {
        let resp = server()
            .handle_message(r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"explota"}}"#)
            .unwrap();
        assert!(resp.contains("\"result\""));
        assert!(resp.contains("isError\":true"));
        assert!(resp.contains("bum controlado"));
    }

    #[test]
    fn el_id_vuelve_intacto() {
        let resp = server()
            .handle_message(r#"{"jsonrpc":"2.0","id":"abc-7","method":"ping"}"#)
            .unwrap();
        assert!(resp.contains("\"id\":\"abc-7\""));
        let resp = server()
            .handle_message(r#"{"jsonrpc":"2.0","id":42,"method":"ping"}"#)
            .unwrap();
        assert!(resp.contains("\"id\":42"));
    }
}
