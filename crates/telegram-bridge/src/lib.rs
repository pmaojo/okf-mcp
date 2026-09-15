//! Puente entre Telegram, OpenRouter y las `ToolSpec` de `mcp-core`.
//!
//! Dos usos, ambos en `crates/vercel-entry`:
//!
//! 1. **Notificación** (`send_message`): avisar a un chat de Telegram
//!    cuando pasa algo en el grafo (un commit, un borrado) sin que
//!    nadie lo pidiera desde Telegram — lo dispara `api/mcp.rs` en
//!    caliente tras una escritura, fire-and-forget.
//! 2. **Control** (`decide` + `summarize_tool_result`): interpretar un
//!    mensaje QUE LLEGA de Telegram y decidir qué `ToolSpec` invocar,
//!    vía un modelo de OpenRouter con tool-calling estilo OpenAI —
//!    `api/telegram.rs` hace de webhook, ejecuta la tool elegida contra
//!    el `ToolHandler` real y usa `summarize_tool_result` para
//!    devolver una respuesta en lenguaje natural.
//!
//! Este crate NO ejecuta tools ni conoce `MemoryTools`: solo sabe
//! hablar HTTP con Telegram/OpenRouter y traducir `ToolSpec` al
//! formato `tools` de la Chat Completions API. Ejecutar la tool
//! elegida es responsabilidad del llamador (`vercel-entry`), que sí
//! tiene un `ToolHandler` a mano — mismo principio de separación que
//! `mcp_http::route` frente a `mcp-core`.

use mcp_core::ToolSpec;
use serde_json::{json, Value as JsonValue};

/// Falló hablar con la Bot API de Telegram.
#[derive(Debug, thiserror::Error)]
pub enum TelegramError {
    /// Error de transporte (DNS, TLS, timeout, ...).
    #[error("fallo de red hacia Telegram: {0}")]
    Http(#[from] reqwest::Error),
    /// Telegram respondió, pero con un error de la Bot API.
    #[error("Telegram respondió {status}: {body}")]
    Api { status: u16, body: String },
}

/// Credenciales para hablar con la Bot API de Telegram. `chat_id` es
/// el destino de las notificaciones PUSH (no del webhook de control,
/// que recibe el `chat_id` de cada mensaje entrante) — típicamente el
/// chat con el propio operador o un canal.
#[derive(Debug, Clone)]
pub struct TelegramConfig {
    /// Token del bot, tal como lo da @BotFather.
    pub bot_token: String,
    /// `chat_id` por defecto para notificaciones push.
    pub chat_id: Option<String>,
}

impl TelegramConfig {
    /// Lee `TELEGRAM_BOT_TOKEN` (obligatoria) y `TELEGRAM_CHAT_ID`
    /// (opcional, solo hace falta para notificar; el webhook de
    /// control no la necesita porque el `chat_id` viaja en cada
    /// `Update`). `None` si falta el token: notificar es opcional, no
    /// hace falta abortar el arranque del servidor MCP por esto.
    pub fn from_env() -> Option<Self> {
        let bot_token = std::env::var("TELEGRAM_BOT_TOKEN").ok().filter(|v| !v.is_empty())?;
        let chat_id = std::env::var("TELEGRAM_CHAT_ID").ok().filter(|v| !v.is_empty());
        Some(TelegramConfig { bot_token, chat_id })
    }
}

/// Manda `text` al chat `chat_id` vía `sendMessage`, con `parse_mode:
/// "Markdown"` (el modo LEGACY de Telegram, no MarkdownV2: acepta
/// `*negrita*`/`_cursiva*`/`` `code` `` sin exigir escapar cada
/// `.`/`-`/`(`/`)` del texto, a diferencia de MarkdownV2). Un
/// `concept_id` con `_` o un `*` suelto en el texto de un modelo
/// puede dejar una entidad sin cerrar y que Telegram devuelva 400
/// ("can't parse entities") — en ese caso reintenta UNA vez sin
/// `parse_mode`, texto plano siempre es válido: perder el formato es
/// aceptable, perder el mensaje no.
pub async fn send_message(
    client: &reqwest::Client,
    config: &TelegramConfig,
    chat_id: &str,
    text: &str,
) -> Result<(), TelegramError> {
    let url = format!("https://api.telegram.org/bot{}/sendMessage", config.bot_token);
    let resp = client
        .post(&url)
        .json(&json!({ "chat_id": chat_id, "text": text, "parse_mode": "Markdown" }))
        .send()
        .await?;
    if resp.status().is_success() {
        return Ok(());
    }
    if resp.status().as_u16() != 400 {
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        return Err(TelegramError::Api { status, body });
    }

    let plain = client
        .post(&url)
        .json(&json!({ "chat_id": chat_id, "text": text }))
        .send()
        .await?;
    if !plain.status().is_success() {
        let status = plain.status().as_u16();
        let body = plain.text().await.unwrap_or_default();
        return Err(TelegramError::Api { status, body });
    }
    Ok(())
}

/// Un mensaje de texto entrante, ya extraído de un `Update` de
/// Telegram. Ignoramos deliberadamente todo lo que no sea
/// `message.text` (fotos, stickers, ediciones, mensajes de canal): el
/// webhook de control solo entiende lenguaje natural dirigido a una
/// tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingMessage {
    /// Chat del que vino el mensaje — también el destino de la respuesta.
    pub chat_id: String,
    /// ID numérico de Telegram del emisor, para filtrar por
    /// `TELEGRAM_ALLOWED_USER_IDS` en el llamador.
    pub from_user_id: Option<i64>,
    /// Texto del mensaje.
    pub text: String,
}

/// Extrae `message.chat.id` + `message.text` de un `Update` crudo
/// (el body que Telegram POSTea al webhook). `None` para cualquier
/// `Update` que no sea un mensaje de texto simple (ediciones,
/// mensajes de canal, callbacks de botones, adjuntos sin texto, ...).
pub fn parse_update(body: &[u8]) -> Option<IncomingMessage> {
    let value: JsonValue = serde_json::from_slice(body).ok()?;
    let message = value.get("message")?;
    let chat_id = message.get("chat")?.get("id")?;
    let chat_id = if let Some(n) = chat_id.as_i64() { n.to_string() } else { chat_id.as_str()?.to_string() };
    let text = message.get("text")?.as_str()?.to_string();
    let from_user_id = message.get("from").and_then(|f| f.get("id")).and_then(JsonValue::as_i64);
    Some(IncomingMessage { chat_id, from_user_id, text })
}

/// Falló hablar con OpenRouter, o la respuesta no tenía la forma
/// esperada de una Chat Completions API estilo OpenAI.
#[derive(Debug, thiserror::Error)]
pub enum OpenRouterError {
    /// Error de transporte.
    #[error("fallo de red hacia OpenRouter: {0}")]
    Http(#[from] reqwest::Error),
    /// OpenRouter respondió, pero con un error HTTP.
    #[error("OpenRouter respondió {status}: {body}")]
    Api { status: u16, body: String },
    /// Respuesta 2xx pero sin `choices[0].message`.
    #[error("respuesta de OpenRouter sin choices[0].message")]
    EmptyResponse,
    /// El `arguments` de un `tool_call` no es JSON válido — pasa con
    /// modelos pequeños/gratuitos que no siguen el schema al pie de la
    /// letra; se trata como fallo de dominio, no de protocolo, para
    /// que el llamador pueda avisar por Telegram en vez de tumbar la
    /// función.
    #[error("argumentos de tool_call no son JSON válido: {0}")]
    InvalidToolArguments(String),
}

/// Credenciales y modelo para OpenRouter (API compatible con OpenAI
/// Chat Completions). `model` es explícito y sin default hardcodeado a
/// un ID concreto: el catálogo de modelos ":free" de OpenRouter cambia
/// con el tiempo (se retiran y aparecen otros) — ver
/// <https://openrouter.ai/models?max_price=0> para el vigente hoy.
#[derive(Debug, Clone)]
pub struct OpenRouterConfig {
    /// Clave de API de OpenRouter (`sk-or-...`).
    pub api_key: String,
    /// ID de modelo, p. ej. `"meta-llama/llama-3.1-8b-instruct:free"`.
    pub model: String,
}

impl OpenRouterConfig {
    /// Lee `OPENROUTER_API_KEY` y `OPENROUTER_MODEL`, ambas
    /// obligatorias. `None` si falta cualquiera — el control por
    /// Telegram simplemente no se ofrece sin las dos.
    pub fn from_env() -> Option<Self> {
        let api_key = std::env::var("OPENROUTER_API_KEY").ok().filter(|v| !v.is_empty())?;
        let model = std::env::var("OPENROUTER_MODEL").ok().filter(|v| !v.is_empty())?;
        Some(OpenRouterConfig { api_key, model })
    }
}

/// Qué decidió el modelo hacer con el mensaje del usuario.
#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    /// Responder directamente, sin tocar ninguna tool.
    Reply(String),
    /// Invocar una tool. `call_id` hay que devolverlo intacto en
    /// [`summarize_tool_result`] — es lo que enlaza el resultado con
    /// esta llamada concreta en la siguiente vuelta de la
    /// conversación.
    ToolCall { call_id: String, name: String, arguments: json_mini::Value },
}

/// Traduce las `ToolSpec` de un `ToolHandler` al array `tools` de la
/// Chat Completions API (estilo OpenAI: `{"type":"function","function":{...}}`).
/// El `inputSchema` de cada tool viaja tal cual como `parameters` —
/// mismo JSON Schema que ya validamos en el protocolo MCP, no hace
/// falta traducirlo a otra cosa.
pub fn tool_defs(tools: &[ToolSpec]) -> JsonValue {
    let defs: Vec<JsonValue> = tools
        .iter()
        .map(|t| {
            // Puente `json_mini::Value` -> `serde_json::Value` por
            // texto: son dos crates de JSON independientes a
            // propósito (`json_mini` es std-only por diseño, ver su
            // doc de módulo) — ida y vuelta por string es más simple y
            // más corto que un conversor recursivo variante a
            // variante, y el volumen aquí (un `inputSchema` por tool,
            // unos pocos KB) no lo hace un problema de rendimiento.
            let schema_text = json_mini::to_string(&t.input_schema);
            let parameters: JsonValue = serde_json::from_str(&schema_text)
                .unwrap_or_else(|_| json!({"type": "object", "properties": {}}));
            json!({
                "type": "function",
                "function": {
                    "name": t.name,
                    "description": t.description,
                    "parameters": parameters,
                }
            })
        })
        .collect();
    JsonValue::Array(defs)
}

/// Primera vuelta: manda el mensaje del usuario + el catálogo de tools
/// a OpenRouter con `tool_choice: "auto"` y devuelve qué decidió el
/// modelo — responder directo, o invocar una tool concreta.
pub async fn decide(
    client: &reqwest::Client,
    config: &OpenRouterConfig,
    system_prompt: &str,
    tools: &[ToolSpec],
    user_text: &str,
) -> Result<Decision, OpenRouterError> {
    let body = json!({
        "model": config.model,
        "messages": [
            {"role": "system", "content": system_prompt},
            {"role": "user", "content": user_text},
        ],
        "tools": tool_defs(tools),
        "tool_choice": "auto",
    });
    let message = chat_completion(client, config, &body).await?;
    parse_decision(&message)
}

/// Segunda vuelta: le devuelve al modelo el resultado de la tool que
/// eligió en [`decide`] (mismo `call_id`) y pide un resumen en
/// lenguaje natural para mandar por Telegram — sin volver a declarar
/// `tools`, así el modelo no puede encadenar otra llamada en vez de
/// responder.
#[allow(clippy::too_many_arguments)]
pub async fn summarize_tool_result(
    client: &reqwest::Client,
    config: &OpenRouterConfig,
    system_prompt: &str,
    user_text: &str,
    call_id: &str,
    tool_name: &str,
    tool_arguments_json: &str,
    tool_result_json: &str,
) -> Result<String, OpenRouterError> {
    let body = json!({
        "model": config.model,
        "messages": [
            {"role": "system", "content": system_prompt},
            {"role": "user", "content": user_text},
            {
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": call_id,
                    "type": "function",
                    "function": {"name": tool_name, "arguments": tool_arguments_json},
                }],
            },
            {"role": "tool", "tool_call_id": call_id, "content": tool_result_json},
        ],
    });
    let message = chat_completion(client, config, &body).await?;
    Ok(message.get("content").and_then(JsonValue::as_str).unwrap_or_default().to_string())
}

/// POST compartido por [`decide`] y [`summarize_tool_result`]: manda
/// `body` y devuelve `choices[0].message` crudo — cada llamador lo
/// interpreta a su manera (decisión vs. texto final).
async fn chat_completion(
    client: &reqwest::Client,
    config: &OpenRouterConfig,
    body: &JsonValue,
) -> Result<JsonValue, OpenRouterError> {
    let resp = client
        .post("https://openrouter.ai/api/v1/chat/completions")
        .bearer_auth(&config.api_key)
        .json(body)
        .send()
        .await?;
    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        return Err(OpenRouterError::Api { status, body });
    }
    let parsed: JsonValue = resp.json().await?;
    parsed
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .cloned()
        .ok_or(OpenRouterError::EmptyResponse)
}

fn parse_decision(message: &JsonValue) -> Result<Decision, OpenRouterError> {
    let tool_calls = message.get("tool_calls").and_then(JsonValue::as_array);
    let Some(first) = tool_calls.and_then(|calls| calls.first()) else {
        let text = message.get("content").and_then(JsonValue::as_str).unwrap_or_default();
        return Ok(Decision::Reply(text.to_string()));
    };
    let call_id = first.get("id").and_then(JsonValue::as_str).unwrap_or_default().to_string();
    let function = first.get("function").cloned().unwrap_or_default();
    let name = function.get("name").and_then(JsonValue::as_str).unwrap_or_default().to_string();
    let arguments_text = function.get("arguments").and_then(JsonValue::as_str).unwrap_or("{}");
    let arguments = json_mini::parse(arguments_text)
        .map_err(|e| OpenRouterError::InvalidToolArguments(e.to_string()))?;
    Ok(Decision::ToolCall { call_id, name, arguments })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_update_extrae_chat_id_y_texto() {
        let body = br#"{"update_id":1,"message":{"message_id":1,"chat":{"id":42},"from":{"id":7},"text":"busca alice"}}"#;
        let msg = parse_update(body).unwrap();
        assert_eq!(msg.chat_id, "42");
        assert_eq!(msg.from_user_id, Some(7));
        assert_eq!(msg.text, "busca alice");
    }

    #[test]
    fn parse_update_ignora_updates_sin_texto() {
        let body = br#"{"update_id":1,"message":{"message_id":1,"chat":{"id":42},"sticker":{}}}"#;
        assert!(parse_update(body).is_none());
    }

    #[test]
    fn parse_update_ignora_json_sin_message() {
        assert!(parse_update(br#"{"update_id":1}"#).is_none());
    }

    #[test]
    fn tool_defs_traduce_name_description_y_schema() {
        let tools = vec![ToolSpec {
            name: "echo",
            description: "devuelve lo recibido",
            input_schema: json_mini::obj([("type", json_mini::s("object"))]),
            ui_resource_uri: None,
            title: None,
            output_schema: None,
            annotations: None,
        }];
        let defs = tool_defs(&tools);
        let arr = defs.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["type"], "function");
        assert_eq!(arr[0]["function"]["name"], "echo");
        assert_eq!(arr[0]["function"]["parameters"]["type"], "object");
    }

    #[test]
    fn parse_decision_sin_tool_calls_es_reply() {
        let message = json!({"role": "assistant", "content": "hola"});
        let decision = parse_decision(&message).unwrap();
        assert_eq!(decision, Decision::Reply("hola".to_string()));
    }

    #[test]
    fn parse_decision_con_tool_call_extrae_nombre_y_argumentos() {
        let message = json!({
            "role": "assistant",
            "tool_calls": [{
                "id": "call_1",
                "type": "function",
                "function": {"name": "memory_search", "arguments": "{\"query\":\"alice\"}"},
            }],
        });
        let decision = parse_decision(&message).unwrap();
        match decision {
            Decision::ToolCall { call_id, name, arguments } => {
                assert_eq!(call_id, "call_1");
                assert_eq!(name, "memory_search");
                assert_eq!(arguments.get("query").and_then(|v| v.as_str()), Some("alice"));
            }
            other => panic!("esperaba ToolCall, llegó {other:?}"),
        }
    }

    #[test]
    fn parse_decision_con_argumentos_invalidos_es_error_de_dominio() {
        let message = json!({
            "tool_calls": [{
                "id": "call_1",
                "function": {"name": "memory_search", "arguments": "no es json"},
            }],
        });
        assert!(matches!(parse_decision(&message), Err(OpenRouterError::InvalidToolArguments(_))));
    }
}
