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
#![warn(missing_docs)]

use json_mini::{arr, json, obj, s, Value};

/// Versión del protocolo que implementamos.
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// Códigos de error JSON-RPC 2.0.
pub mod code {
    /// El mensaje no era JSON válido.
    pub const PARSE_ERROR: f64 = -32700.0;
    /// JSON válido pero no es una petición JSON-RPC bien formada.
    pub const INVALID_REQUEST: f64 = -32600.0;
    /// El método no existe en este servidor.
    pub const METHOD_NOT_FOUND: f64 = -32601.0;
    /// Parámetros mal formados para un método que sí existe.
    pub const INVALID_PARAMS: f64 = -32602.0;
    /// Fallo interno del servidor (nunca de la herramienta: eso va
    /// como resultado con `isError: true`).
    pub const INTERNAL_ERROR: f64 = -32603.0;
}

/// Descripción de una herramienta para `tools/list`.
#[derive(Debug, Clone)]
pub struct ToolSpec {
    /// Nombre con el que el cliente invoca la herramienta.
    pub name: &'static str,
    /// Descripción orientada al MODELO: es quien decide usarla.
    pub description: &'static str,
    /// JSON Schema del parámetro `arguments`.
    pub input_schema: Value,
    /// URI `ui://` (ver [`UiResource`]) que renderiza el resultado de
    /// esta herramienta, si el cliente soporta la extensión MCP Apps
    /// (`io.modelcontextprotocol/ui`). `None` es la opción correcta
    /// para herramientas cuyo resultado no gana nada con una vista a
    /// medida — no todo tool necesita una.
    pub ui_resource_uri: Option<&'static str>,
    /// Nombre corto para HUMANOS (distinto de `name`, que es para el
    /// modelo/protocolo). Un host lo muestra en vez del `name` crudo
    /// cuando lista herramientas a la persona. `None` si `name` ya es
    /// suficientemente legible.
    pub title: Option<&'static str>,
    /// JSON Schema de `structuredContent` (spec 2025-06-18). Permite a
    /// un cliente validar o tipar la respuesta sin adivinar el shape
    /// desde `content[0].text`. `None` para herramientas sin resultado
    /// estructurado estable (ninguna hoy, pero el contrato lo admite).
    pub output_schema: Option<Value>,
    /// Pistas de comportamiento (spec 2025-06-18) que un host usa para
    /// decidir auto-aprobación o presentación — nunca para aplicar
    /// seguridad real, son declarativas y no verificadas por el
    /// protocolo. `None` dispersa las mismas herramientas de siempre
    /// (`EchoTools` en los tests de este crate) sin cambiar una línea.
    pub annotations: Option<ToolAnnotations>,
}

/// Pistas declarativas de comportamiento para una `ToolSpec`, según la
/// sección `annotations` de la spec MCP 2025-06-18. Cada campo es
/// `Option<bool>` porque "no se declaró" y "declarado false" son
/// cosas distintas: un cliente que no ve el campo aplica el default
/// de la spec (`readOnlyHint`/`idempotentHint`/`destructiveHint` no
/// tienen el mismo default), así que declaramos explícitamente cada
/// hint en vez de confiar en ese default implícito.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ToolAnnotations {
    /// La herramienta no modifica su entorno (no escribe nada).
    pub read_only_hint: Option<bool>,
    /// La herramienta puede realizar cambios destructivos o
    /// irreversibles (solo tiene sentido si `read_only_hint` es
    /// `false` o no está declarado).
    pub destructive_hint: Option<bool>,
    /// Llamar la herramienta repetidas veces con los MISMOS argumentos
    /// no tiene efecto adicional más allá de la primera vez.
    pub idempotent_hint: Option<bool>,
    /// La herramienta interactúa con un dominio abierto (el mundo
    /// exterior, p. ej. la web) en vez de un entorno cerrado y
    /// conocido (este grafo de memoria).
    pub open_world_hint: Option<bool>,
}

/// Un recurso `ui://`: HTML autocontenido (spec MCP Apps / ext-apps,
/// SEP-1724) que un cliente compatible renderiza en un iframe para
/// mostrar el resultado de una herramienta como algo más legible que
/// JSON crudo.
#[derive(Debug, Clone)]
pub struct UiResource {
    /// URI `ui://` con la que se anuncia y se lee el recurso.
    pub uri: &'static str,
    /// Nombre legible para `resources/list`.
    pub name: &'static str,
    /// Qué muestra esta vista y para qué herramienta.
    pub description: &'static str,
    /// HTML completo (`<!DOCTYPE html>...`), con CSS/JS inline. Debe
    /// ser autocontenido: el cliente lo sirve en un iframe aislado sin
    /// acceso a ningún build step ni CDN externo salvo que el propio
    /// HTML lo declare.
    pub html: &'static str,
}

const UI_APPS_MIME_TYPE: &str = "text/html;profile=mcp-app";

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
    /// Catálogo de herramientas, para `tools/list`.
    fn tools(&self) -> Vec<ToolSpec>;

    /// Ejecuta la herramienta `name` con `arguments`. La distinción
    /// entre variantes de [`ToolError`] importa: solo las dos
    /// primeras son errores de protocolo.
    fn call(&mut self, name: &str, arguments: &Value) -> Result<Value, ToolError>;

    /// Recursos `ui://` que este handler expone, para `resources/list`
    /// y `resources/read`. Cuerpo por defecto vacío: un handler que no
    /// sabe nada de MCP Apps (como `EchoTools` en los tests de este
    /// crate) no tiene que cambiar una sola línea — Abierto/Cerrado.
    fn ui_resources(&self) -> Vec<UiResource> {
        Vec::new()
    }

    /// Guía de USO — no de protocolo — que el cliente muestra al
    /// MODELO una vez, en `initialize` (campo `instructions` de
    /// `InitializeResult`), no en cada llamada. Es el sitio correcto
    /// para una convención que de otro modo el modelo tendría que
    /// adivinar o redescubrir cada sesión (p. ej. "busca un
    /// vocabulario ya declarado antes de inventar nombres nuevos"):
    /// pagar el coste de tokens UNA vez al arrancar es más barato y
    /// más determinista que repetirlo en la descripción de cada
    /// herramienta, y mucho más que dejar que el modelo alucine una
    /// convención plausible pero distinta cada vez. Cuerpo por
    /// defecto `None` — mismo Abierto/Cerrado que [`Self::ui_resources`]:
    /// un handler que no tiene nada que decir aquí no cambia una
    /// línea.
    fn instructions(&self) -> Option<&str> {
        None
    }
}

/// Estado del ciclo de vida MCP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lifecycle {
    AwaitingInitialize,
    Initializing,
    Ready,
}

/// Servidor MCP independiente del transporte.
///
/// # Ejemplo
///
/// Todo el protocolo es una función sobre strings — no hace falta
/// ningún socket para verlo funcionar:
///
/// ```
/// use json_mini::Value;
/// use mcp_core::{McpServer, ToolError, ToolHandler, ToolSpec};
///
/// struct Eco;
/// impl ToolHandler for Eco {
///     fn tools(&self) -> Vec<ToolSpec> { Vec::new() }
///     fn call(&mut self, _: &str, args: &Value) -> Result<Value, ToolError> {
///         Ok(args.clone())
///     }
/// }
///
/// let mut srv = McpServer::new("demo", "0.0.0", Eco);
///
/// // Una petición produce respuesta con el MISMO id…
/// let resp = srv
///     .handle_message(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#)
///     .unwrap();
/// assert!(resp.contains(r#""id":1"#));
///
/// // …y una notificación no produce nada, jamás.
/// assert!(srv
///     .handle_message(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
///     .is_none());
/// ```
pub struct McpServer<H: ToolHandler> {
    handler: H,
    server_name: &'static str,
    server_version: &'static str,
    lifecycle: Lifecycle,
}

impl<H: ToolHandler> McpServer<H> {
    /// Servidor recién nacido, a la espera de `initialize`.
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
            "ping" => ok_response(id, json!({})),
            "tools/list" => self.on_tools_list(id),
            "tools/call" => self.on_tools_call(id, params),
            "resources/list" => self.on_resources_list(id),
            "resources/read" => self.on_resources_read(id, params),
            _ => error_response(id, code::METHOD_NOT_FOUND, &format!("método desconocido: {method}")),
        }
    }

    fn on_initialize(&mut self, id: Value, params: &Value) -> Value {
        // Negociación de versión: si el cliente propone una que
        // conocemos, la aceptamos; si no, contestamos con la nuestra
        let requested_version = params
            .get("protocolVersion")
            .and_then(|v| v.as_str())
            .unwrap_or(PROTOCOL_VERSION);
        let version = match requested_version {
            "2024-11-05" | "2025-06-18" => requested_version,
            _ => PROTOCOL_VERSION,
        };
        self.lifecycle = Lifecycle::Initializing;

        let mut fields = vec![
            ("protocolVersion", s(version)),
            ("capabilities", obj([("tools", obj([])), ("resources", obj([]))])),
            (
                "serverInfo",
                obj([("name", s(self.server_name)), ("version", s(self.server_version))]),
            ),
        ];
        if let Some(instructions) = self.handler.instructions() {
            fields.push(("instructions", s(instructions)));
        }

        ok_response(id, Value::Object(fields.into_iter().map(|(k, v)| (k.to_string(), v)).collect()))
    }

    fn on_tools_list(&mut self, id: Value) -> Value {
        let tools: Vec<Value> = self
            .handler
            .tools()
            .into_iter()
            .map(|t| {
                let mut fields = vec![
                    ("name", s(t.name)),
                    ("description", s(t.description)),
                    ("inputSchema", t.input_schema),
                ];
                if let Some(title) = t.title {
                    fields.push(("title", s(title)));
                }
                if let Some(output_schema) = t.output_schema {
                    fields.push(("outputSchema", output_schema));
                }
                if let Some(a) = t.annotations {
                    let mut ann = Vec::new();
                    if let Some(v) = a.read_only_hint {
                        ann.push(("readOnlyHint".to_string(), Value::Bool(v)));
                    }
                    if let Some(v) = a.destructive_hint {
                        ann.push(("destructiveHint".to_string(), Value::Bool(v)));
                    }
                    if let Some(v) = a.idempotent_hint {
                        ann.push(("idempotentHint".to_string(), Value::Bool(v)));
                    }
                    if let Some(v) = a.open_world_hint {
                        ann.push(("openWorldHint".to_string(), Value::Bool(v)));
                    }
                    if !ann.is_empty() {
                        fields.push(("annotations", Value::Object(ann.into_iter().collect())));
                    }
                }
                if let Some(uri) = t.ui_resource_uri {
                    // `_meta` se manda SIEMPRE, sin negociar ninguna
                    // capability en `initialize`: un cliente MCP Apps
                    // real (Claude incluido) nunca declaró
                    // `capabilities.extensions["io.modelcontextprotocol/ui"]`,
                    // así que una gate ahí escondía la UI de todo el
                    // mundo. Un cliente que no entiende `_meta` está
                    // obligado por el spec a ignorarlo — no hace falta
                    // ocultarlo nosotros a mano.
                    //
                    // Doble anuncio a propósito: `ui.resourceUri` es el
                    // campo de la extensión MCP Apps (SEP-1724);
                    // `openai/outputTemplate` + `openai/widgetAccessible`
                    // son los que el host de Claude realmente lee hoy
                    // (Apps SDK). Mandar ambos cubre los dos sin tener
                    // que adivinar cuál interpretará el cliente.
                    fields.push((
                        "_meta",
                        obj([
                            (
                                "ui",
                                obj([
                                    ("resourceUri", s(uri)),
                                    ("visibility", arr(vec![s("model"), s("app")])),
                                ]),
                            ),
                            ("openai/outputTemplate", s(uri)),
                            ("openai/widgetAccessible", Value::Bool(true)),
                        ]),
                    ));
                }
                Value::Object(fields.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
            })
            .collect();
        ok_response(id, obj([("tools", arr(tools))]))
    }

    fn on_resources_list(&mut self, id: Value) -> Value {
        let resources: Vec<Value> = self
            .handler
            .ui_resources()
            .into_iter()
            .map(|r| {
                obj([
                    ("uri", s(r.uri)),
                    ("name", s(r.name)),
                    ("description", s(r.description)),
                    ("mimeType", s(UI_APPS_MIME_TYPE)),
                ])
            })
            .collect();
        ok_response(id, obj([("resources", arr(resources))]))
    }

    fn on_resources_read(&mut self, id: Value, params: &Value) -> Value {
        let Some(uri) = params.get("uri").and_then(|v| v.as_str()) else {
            return error_response(id, code::INVALID_PARAMS, "falta params.uri");
        };
        match self.handler.ui_resources().into_iter().find(|r| r.uri == uri) {
            Some(r) => ok_response(
                id,
                obj([(
                    "contents",
                    arr(vec![obj([
                        ("uri", s(r.uri)),
                        ("mimeType", s(UI_APPS_MIME_TYPE)),
                        ("text", s(r.html)),
                    ])]),
                )]),
            ),
            None => error_response(id, code::INVALID_PARAMS, &format!("recurso desconocido: {uri}")),
        }
    }

    fn on_tools_call(&mut self, id: Value, params: &Value) -> Value {
        let Some(name) = params.get("name").and_then(|v| v.as_str()) else {
            return error_response(id, code::INVALID_PARAMS, "falta params.name");
        };
        let default_args = obj([]);
        let arguments = params.get("arguments").unwrap_or(&default_args);

        match self.handler.call(name, arguments) {
            Ok(result) => {
                let text = json_mini::to_string(&result);
                // `structuredContent` solo tiene sentido para un
                // objeto JSON (así lo exige el esquema CallToolResult);
                // `content[0].text` se manda SIEMPRE igual, por
                // compatibilidad con clientes que no leen el campo
                // nuevo.
                let structured = matches!(result, Value::Object(_)).then_some(result);
                ok_response(id, tool_result(&text, structured, false))
            }
            Err(ToolError::UnknownTool) => {
                error_response(id, code::INVALID_PARAMS, &format!("herramienta desconocida: {name}"))
            }
            Err(ToolError::InvalidArguments(msg)) => {
                error_response(id, code::INVALID_PARAMS, &msg)
            }
            // Fallo de dominio: respuesta correcta de protocolo con
            // isError=true. Así el MODELO ve el conflicto CAS y puede
            // releer y reintentar.
            Err(ToolError::Failed(msg)) => ok_response(id, tool_result(&msg, None, true)),
        }
    }
}

/// Resultado de herramienta según el esquema MCP `CallToolResult`.
fn tool_result(text: &str, structured: Option<Value>, is_error: bool) -> Value {
    let mut fields = vec![
        (
            "content".to_string(),
            arr(vec![obj([("type", s("text")), ("text", s(text))])]),
        ),
        ("isError".to_string(), Value::Bool(is_error)),
    ];
    if let Some(structured) = structured {
        fields.push(("structuredContent".to_string(), structured));
    }
    Value::Object(fields.into_iter().collect())
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
                ui_resource_uri: None,
                title: None,
                output_schema: None,
                annotations: None,
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

    /// Handler de prueba con una herramienta vinculada a un recurso
    /// `ui://`, para probar `resources/list`, `resources/read` y la
    /// degradación elegante de `_meta.ui` en `tools/list`.
    struct EchoWithUi;

    impl ToolHandler for EchoWithUi {
        fn tools(&self) -> Vec<ToolSpec> {
            vec![ToolSpec {
                name: "echo",
                description: "devuelve lo recibido",
                input_schema: obj([("type", s("object"))]),
                ui_resource_uri: Some("ui://test/echo-view"),
                title: Some("Echo"),
                output_schema: Some(obj([("type", s("object"))])),
                annotations: Some(ToolAnnotations {
                    read_only_hint: Some(true),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(true),
                    open_world_hint: Some(false),
                }),
            }]
        }
        fn call(&mut self, _name: &str, arguments: &Value) -> Result<Value, ToolError> {
            Ok(arguments.clone())
        }
        fn ui_resources(&self) -> Vec<UiResource> {
            vec![UiResource {
                uri: "ui://test/echo-view",
                name: "Echo view",
                description: "vista de prueba",
                html: "<!DOCTYPE html><html><body>echo</body></html>",
            }]
        }
    }

    fn server_with_ui() -> McpServer<EchoWithUi> {
        McpServer::new("test", "0.0.0", EchoWithUi)
    }

    const INIT_SIN_UI: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}"#;
    const INIT_CON_UI: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{"extensions":{"io.modelcontextprotocol/ui":{"mimeTypes":["text/html;profile=mcp-app"]}}},"clientInfo":{"name":"t","version":"0"}}}"#;

    #[test]
    fn resources_list_y_read_devuelven_el_recurso_ui() {
        let mut srv = server_with_ui();
        srv.handle_message(INIT_SIN_UI).unwrap();

        let resp = srv
            .handle_message(r#"{"jsonrpc":"2.0","id":2,"method":"resources/list"}"#)
            .unwrap();
        assert!(resp.contains("\"ui://test/echo-view\""));
        assert!(resp.contains("\"mimeType\":\"text/html;profile=mcp-app\""));

        let resp = srv
            .handle_message(r#"{"jsonrpc":"2.0","id":3,"method":"resources/read","params":{"uri":"ui://test/echo-view"}}"#)
            .unwrap();
        assert!(resp.contains("<!DOCTYPE html>"));
    }

    #[test]
    fn resources_read_de_uri_desconocida_es_invalid_params() {
        let resp = server_with_ui()
            .handle_message(r#"{"jsonrpc":"2.0","id":1,"method":"resources/read","params":{"uri":"ui://no-existe"}}"#)
            .unwrap();
        assert!(resp.contains("-32602"));
    }

    #[test]
    fn tools_list_anuncia_meta_ui_siempre_declare_o_no_el_cliente_la_extension() {
        // Un cliente MCP Apps real (Claude incluido) no declara
        // `capabilities.extensions["io.modelcontextprotocol/ui"]` en
        // `initialize` — así que `_meta.ui` NO puede depender de esa
        // negociación, o nunca llegaría a un cliente real. Debe
        // aparecer igual con o sin la capability: un cliente que no
        // entiende `_meta` está obligado por el spec a ignorarlo.
        for init in [INIT_SIN_UI, INIT_CON_UI] {
            let mut srv = server_with_ui();
            srv.handle_message(init).unwrap();
            let resp = srv
                .handle_message(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#)
                .unwrap();
            assert!(resp.contains("\"resourceUri\":\"ui://test/echo-view\""));
            assert!(resp.contains("\"visibility\":[\"model\",\"app\"]"));
            // Doble anuncio a propósito: `ui.resourceUri` es el campo
            // de la extensión MCP Apps (SEP-1724); `openai/outputTemplate`
            // + `openai/widgetAccessible` son los que el host de Claude
            // realmente lee hoy (Apps SDK). Mandamos ambos porque no
            // hay forma de saber de antemano cuál interpretará el
            // cliente que conecte.
            assert!(resp.contains("\"openai/outputTemplate\":\"ui://test/echo-view\""));
            assert!(resp.contains("\"openai/widgetAccessible\":true"));
            assert!(resp.contains("\"title\":\"Echo\""));
            assert!(resp.contains("\"outputSchema\":{\"type\":\"object\"}"));
            assert!(resp.contains("\"readOnlyHint\":true"));
            assert!(resp.contains("\"destructiveHint\":false"));
            assert!(resp.contains("\"idempotentHint\":true"));
            assert!(resp.contains("\"openWorldHint\":false"));
        }
    }

    #[test]
    fn tools_call_incluye_structured_content_para_resultados_objeto() {
        let resp = server()
            .handle_message(r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"echo","arguments":{"x":1}}}"#)
            .unwrap();
        assert!(resp.contains("\"structuredContent\":{\"x\":1}"));
        // `content[0].text` se mantiene igual, por compatibilidad.
        assert!(resp.contains("{\\\"x\\\":1}"));
    }

    #[test]
    fn tools_call_fallido_no_lleva_structured_content() {
        let resp = server()
            .handle_message(r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"explota"}}"#)
            .unwrap();
        assert!(!resp.contains("structuredContent"));
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

    /// Handler mínimo que SÍ tiene algo que decir en `instructions`,
    /// para probar el otro lado del Abierto/Cerrado que `EchoTools`
    /// ya prueba por omisión (su `initialize` nunca lleva el campo).
    struct EchoWithInstructions;

    impl ToolHandler for EchoWithInstructions {
        fn tools(&self) -> Vec<ToolSpec> {
            Vec::new()
        }
        fn call(&mut self, _name: &str, _arguments: &Value) -> Result<Value, ToolError> {
            Err(ToolError::UnknownTool)
        }
        fn instructions(&self) -> Option<&str> {
            Some("busca convenciones ya declaradas antes de inventar nombres nuevos")
        }
    }

    #[test]
    fn initialize_omite_instructions_por_defecto() {
        let resp = server()
            .handle_message(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}"#)
            .unwrap();
        assert!(!resp.contains("instructions"));
    }

    #[test]
    fn initialize_incluye_instructions_cuando_el_handler_las_declara() {
        let mut srv = McpServer::new("test", "0.0.0", EchoWithInstructions);
        let resp = srv
            .handle_message(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}"#)
            .unwrap();
        assert!(resp.contains("busca convenciones ya declaradas"));
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
