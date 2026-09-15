//! Webhook de control por Telegram: un mensaje de texto llega, un
//! modelo gratuito de OpenRouter decide si responder directo o
//! invocar una tool de `okf-memory`, la tool se ejecuta de verdad
//! contra el mismo `MemoryTools<SupabaseStore>` que usa `api/mcp.rs`,
//! y la respuesta (natural, no JSON crudo) vuelve al chat.
//!
//! Configuración en Telegram: `setWebhook` con
//!   `url = https://<tu-dominio>/api/telegram`
//!   `secret_token = <el mismo valor que TELEGRAM_WEBHOOK_SECRET>`
//!
//! Variables de entorno:
//!   `TELEGRAM_BOT_TOKEN`         — token del bot (@BotFather).
//!   `TELEGRAM_WEBHOOK_SECRET`    — secreto compartido; sin él el
//!                                  webhook rechaza TODO (a diferencia
//!                                  de `JWKS_URL` en `api/mcp.rs`, no
//!                                  hay modo abierto: un webhook de
//!                                  control sin secreto es una
//!                                  invitación a que cualquiera
//!                                  escriba en el grafo).
//!   `TELEGRAM_ALLOWED_USER_IDS`  — opcional, lista separada por comas
//!                                  de `from.id` de Telegram
//!                                  autorizados a controlar el bot.
//!                                  Vacía = abierto a quien sea que
//!                                  hable con el bot (solo para
//!                                  pruebas).
//!   `OPENROUTER_API_KEY`,
//!   `OPENROUTER_MODEL`           — credenciales del modelo que decide
//!                                  qué tool invocar (ver
//!                                  `telegram_bridge::OpenRouterConfig`).
//!
//! **Historial**: cada chat de Telegram tiene su propio concepto
//! `sessions/telegram-<chat_id>` en el MISMO grafo OKF — nada de
//! estado aparte. Antes de decidir, se resuelve ese concepto (si
//! existe) y su markdown se inyecta en el prompt de sistema como
//! contexto; después de responder, el turno se añade al documento vía
//! `memory_commit` (CAS con el hash leído, igual que cualquier otro
//! escritor). El documento se acota a los últimos [`MAX_HISTORY_TURNS`]
//! turnos para que ni el prompt ni el documento crezcan sin límite.

#![forbid(unsafe_code)]

use vercel_entry::db;

use axum::body::Bytes;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Router;
use graph_core::NeighborSource;
use json_mini::Value;
use mcp_core::{ToolError, ToolHandler};
use memory_model::{Budget, Principal};
use memory_tools::MemoryTools;
use std::convert::Infallible;
use store_core::{MemoryRepository, StoreMaintenance};
use supabase_store::SupabaseStore;
use telegram_bridge::{Decision, IncomingMessage, OpenRouterConfig, TelegramConfig};
use vercel_runtime::axum::VercelLayer;
use vercel_runtime::{run, Error};

/// Cuántos turnos como máximo conserva el documento de sesión — tanto
/// para acotar el prompt (tokens de OpenRouter) como el tamaño del
/// documento en el grafo.
const MAX_HISTORY_TURNS: usize = 12;
/// Separador de turno: también actúa de "marca de posición" para
/// [`cap_turns`], así que si cambia aquí, `cap_turns` deja de
/// reconocer turnos escritos con el separador anterior (se tratarían
/// como preámbulo, no se perderían, pero no se contarían para el
/// límite hasta el siguiente turno).
const TURN_MARKER: &str = "\n### Turno ";

const SYSTEM_PROMPT: &str = "Eres el asistente de okf-memory por Telegram: decides si responder directo o invocar UNA tool del grafo de memoria para responder con datos reales en vez de inventarlos. Sé conciso — esto se lee en un chat de Telegram, no en un IDE. Si no hace falta ninguna tool (saludo, pregunta general), responde directo.";

/// Compara `from.id` del mensaje contra `TELEGRAM_ALLOWED_USER_IDS`.
/// Lista vacía o sin configurar = abierto (documentado como solo apto
/// para pruebas en el doc del módulo).
fn allowed_user(user_id: Option<i64>) -> bool {
    let allowed = std::env::var("TELEGRAM_ALLOWED_USER_IDS").unwrap_or_default();
    if allowed.trim().is_empty() {
        return true;
    }
    let Some(user_id) = user_id else { return false };
    allowed.split(',').filter_map(|s| s.trim().parse::<i64>().ok()).any(|id| id == user_id)
}

async fn webhook_handler(headers: HeaderMap, body: Bytes) -> Response {
    let Ok(secret) = std::env::var("TELEGRAM_WEBHOOK_SECRET") else {
        return (StatusCode::SERVICE_UNAVAILABLE, "TELEGRAM_WEBHOOK_SECRET no configurado")
            .into_response();
    };
    let header_secret = headers
        .get("x-telegram-bot-api-secret-token")
        .and_then(|v| v.to_str().ok());
    if header_secret != Some(secret.as_str()) {
        return (StatusCode::UNAUTHORIZED, "secreto inválido").into_response();
    }

    // Cualquier `Update` que no sea un mensaje de texto simple
    // (ediciones, adjuntos, callbacks de botones) se acepta con 200
    // sin trabajar — igual que el "ping"/eventos no-push de
    // `github-webhook.rs`: un 4xx aquí lleva a Telegram a reintentar o
    // eventualmente desactivar el webhook.
    let Some(message) = telegram_bridge::parse_update(&body) else {
        return (StatusCode::OK, "update ignorado").into_response();
    };

    if !allowed_user(message.from_user_id) {
        return (StatusCode::OK, "usuario no autorizado, ignorado").into_response();
    }

    let Some(telegram) = TelegramConfig::from_env() else {
        return (StatusCode::SERVICE_UNAVAILABLE, "TELEGRAM_BOT_TOKEN no configurado")
            .into_response();
    };
    let Some(openrouter) = OpenRouterConfig::from_env() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "OPENROUTER_API_KEY/OPENROUTER_MODEL no configurados",
        )
            .into_response();
    };
    let db_url = match std::env::var("POSTGRES_URL") {
        Ok(v) => v,
        Err(_) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "POSTGRES_URL no configurado")
                .into_response();
        }
    };

    let pool = db::get_db_pool(&db_url).await;
    let store = SupabaseStore::new(pool, gemini_embeddings::EmbeddingKeys::from_env());
    // El "quién" de cada escritura hecha desde Telegram queda visible
    // en `memory_history` como cualquier otro actor — no se camufla
    // como `local_dev`.
    let actor = Principal {
        subject: format!("telegram:{}", message.from_user_id.unwrap_or_default()),
        client_id: "telegram-bot".to_string(),
    };
    let mut tools = MemoryTools::new(store, actor, Budget::default());

    let client = reqwest::Client::new();
    let reply = handle_message(&client, &openrouter, &mut tools, &message).await;

    if let Err(e) = telegram_bridge::send_message(&client, &telegram, &message.chat_id, &reply).await {
        eprintln!("okf-telegram: fallo mandando respuesta a {}: {e}", message.chat_id);
    }

    (StatusCode::OK, "ok").into_response()
}

/// El flujo completo de un turno: cargar historial -> decidir ->
/// (ejecutar tool si toca) -> resumir -> persistir el turno. Devuelve
/// SIEMPRE un texto para mandar de vuelta, incluso en caso de fallo
/// (el operador ve el error en el chat, no solo en los logs de
/// Vercel).
async fn handle_message<R>(
    client: &reqwest::Client,
    openrouter: &OpenRouterConfig,
    tools: &mut MemoryTools<R>,
    message: &IncomingMessage,
) -> String
where
    R: MemoryRepository + StoreMaintenance + NeighborSource<Error = Infallible>,
    MemoryTools<R>: ToolHandler,
{
    let concept_id = format!("sessions/telegram-{}", message.chat_id);
    let (expected_hash, history_markdown) = load_history(tools, &concept_id);

    let system_prompt = if history_markdown.is_empty() {
        SYSTEM_PROMPT.to_string()
    } else {
        format!("{SYSTEM_PROMPT}\n\nHistorial reciente de esta conversación:\n{history_markdown}")
    };

    let tool_specs = tools.tools();
    let decision =
        match telegram_bridge::decide(client, openrouter, &system_prompt, &tool_specs, &message.text).await {
            Ok(d) => d,
            Err(e) => return format!("No pude hablar con el modelo: {e}"),
        };

    let (tool_name, reply) = match decision {
        Decision::Reply(text) => (None, text),
        Decision::ToolCall { call_id, name, arguments } => {
            let tool_result_json = match tools.call(&name, &arguments) {
                Ok(v) => json_mini::to_string(&v),
                Err(ToolError::Failed(msg)) => json_mini::to_string(&json_mini::obj([("error", json_mini::s(&msg))])),
                Err(_) => json_mini::to_string(&json_mini::obj([("error", json_mini::s("argumentos inválidos"))])),
            };
            let arguments_json = json_mini::to_string(&arguments);
            let summary = telegram_bridge::summarize_tool_result(
                client,
                openrouter,
                &system_prompt,
                &message.text,
                &call_id,
                &name,
                &arguments_json,
                &tool_result_json,
            )
            .await;
            match summary {
                Ok(text) => (Some(name), text),
                Err(e) => {
                    let msg = format!("Ejecuté {name}, pero fallé resumiendo el resultado: {e}");
                    (Some(name), msg)
                }
            }
        }
    };

    persist_turn(tools, &concept_id, expected_hash, &history_markdown, &message.text, tool_name.as_deref(), &reply);
    reply
}

/// Lee `document.hash`/`document.markdown` de un `memory_resolve`
/// sobre `concept_id`. `(None, "")` si el concepto no existe todavía
/// (primer mensaje de este chat) — CASO NORMAL, no un fallo: el
/// siguiente `memory_commit` lo crea.
fn load_history<H: ToolHandler>(tools: &mut H, concept_id: &str) -> (Option<String>, String) {
    let args = json_mini::obj([("concept_id", json_mini::s(concept_id))]);
    match tools.call("memory_resolve", &args) {
        Ok(result) => {
            let document = result.get("document");
            let hash = document
                .and_then(|d| d.get("hash"))
                .and_then(Value::as_str)
                .map(str::to_string);
            let markdown = document
                .and_then(|d| d.get("markdown"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            (hash, markdown)
        }
        Err(_) => (None, String::new()),
    }
}

/// Añade el turno actual al documento de sesión y lo commitea. Un
/// fallo aquí (p. ej. conflicto de CAS porque otro proceso escribió
/// la sesión a la vez) se registra pero NO cambia la respuesta que ya
/// se le mandó al usuario — perder un turno de historial es
/// degradación aceptable, perder la respuesta no lo es.
fn persist_turn<H: ToolHandler>(
    tools: &mut H,
    concept_id: &str,
    expected_hash: Option<String>,
    existing_markdown: &str,
    user_text: &str,
    tool_name: Option<&str>,
    reply: &str,
) {
    // Contador local sobre lo RETENIDO tras el último recorte, no un
    // id global de turno: tras el primer `cap_turns` puede repetir un
    // número ya usado antes de recortar. Es solo una etiqueta de
    // lectura dentro del documento, no una clave — no pasa nada.
    let turn_number = existing_markdown.matches(TURN_MARKER).count() + 1;
    let turn = render_turn(turn_number, user_text, tool_name, reply);

    let markdown = if existing_markdown.is_empty() {
        format!(
            "---\ntype: telegram-session\ntitle: \"Telegram {concept_id}\"\ntags:\n  - telegram\n  - session\n---\n{turn}"
        )
    } else {
        cap_turns(&format!("{existing_markdown}{turn}"), MAX_HISTORY_TURNS)
    };

    let mut fields: Vec<(String, Value)> = vec![
        ("concept_id".to_string(), json_mini::s(concept_id)),
        ("markdown".to_string(), json_mini::s(&markdown)),
        ("reason".to_string(), json_mini::s("turno de conversación de Telegram")),
    ];
    if let Some(hash) = expected_hash {
        fields.push(("expected_hash".to_string(), json_mini::s(&hash)));
    }
    let args = Value::Object(fields.into_iter().collect());

    if let Err(e) = tools.call("memory_commit", &args) {
        eprintln!("okf-telegram: fallo guardando historial de {concept_id}: {e:?}");
    }
}

fn render_turn(n: usize, user_text: &str, tool_name: Option<&str>, reply: &str) -> String {
    let tool_line = tool_name.map(|t| format!("**Tool:** {t}\n")).unwrap_or_default();
    format!("{TURN_MARKER}{n}\n**Usuario:** {user_text}\n{tool_line}**Respuesta:** {reply}\n")
}

/// Conserva el preámbulo (frontmatter + lo que sea que venga antes del
/// primer [`TURN_MARKER`]) y como mucho los últimos `max_turns`
/// bloques de turno.
fn cap_turns(markdown: &str, max_turns: usize) -> String {
    let mut parts: Vec<&str> = markdown.split(TURN_MARKER).collect();
    if parts.len() <= max_turns + 1 {
        return markdown.to_string();
    }
    let preamble = parts.remove(0);
    let kept = &parts[parts.len() - max_turns..];
    let mut out = preamble.to_string();
    for p in kept {
        out.push_str(TURN_MARKER);
        out.push_str(p);
    }
    out
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let router = Router::new().fallback(webhook_handler);
    let app = tower::ServiceBuilder::new().layer(VercelLayer::new()).service(router);
    run(app).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cap_turns_no_recorta_por_debajo_del_limite() {
        let md = "---\ntype: x\n---\n\n### Turno 1\na\n\n### Turno 2\nb\n";
        assert_eq!(cap_turns(md, 12), md);
    }

    #[test]
    fn cap_turns_conserva_preambulo_y_ultimos_n() {
        let mut md = "---\ntype: x\n---\n".to_string();
        for i in 1..=15 {
            md.push_str(&render_turn(i, "u", None, "r"));
        }
        let capped = cap_turns(&md, 12);
        assert!(capped.starts_with("---\ntype: x\n---\n"));
        assert!(!capped.contains("Turno 1\n"));
        assert!(capped.contains("Turno 15\n"));
        assert_eq!(capped.matches(TURN_MARKER).count(), 12);
    }

    #[test]
    fn render_turn_incluye_tool_solo_si_se_invoco_una() {
        assert!(!render_turn(1, "hola", None, "hi").contains("**Tool:**"));
        assert!(render_turn(1, "busca alice", Some("memory_search"), "3 resultados")
            .contains("**Tool:** memory_search"));
    }

    // Una sola prueba, no dos: `TELEGRAM_ALLOWED_USER_IDS` es estado
    // de PROCESO compartido — `cargo test` corre los tests de un mismo
    // binario en hilos distintos por defecto, así que dos tests
    // tocando la misma variable de entorno competirían entre sí.
    #[test]
    fn allowed_user_abierto_sin_lista_y_filtra_con_lista() {
        std::env::remove_var("TELEGRAM_ALLOWED_USER_IDS");
        assert!(allowed_user(Some(999)));
        assert!(allowed_user(None));

        std::env::set_var("TELEGRAM_ALLOWED_USER_IDS", "7, 42");
        assert!(allowed_user(Some(7)));
        assert!(allowed_user(Some(42)));
        assert!(!allowed_user(Some(999)));
        assert!(!allowed_user(None));
        std::env::remove_var("TELEGRAM_ALLOWED_USER_IDS");
    }
}
