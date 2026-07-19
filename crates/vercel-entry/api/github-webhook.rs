//! Webhook de GitHub: reacciona a un `push` sobre el repo de
//! conceptos reconciliando Supabase AL INSTANTE, en vez de esperar al
//! próximo tick del Cron de `api/outbox.rs` (que sigue existiendo
//! como red de seguridad si la entrega del webhook falla o el evento
//! se pierde).
//!
//! Configuración en GitHub: Settings -> Webhooks -> Add webhook
//!   Payload URL: https://<tu-dominio>/api/github-webhook
//!   Content type: application/json
//!   Secret: el mismo valor que `GITHUB_WEBHOOK_SECRET` en Vercel
//!   Events: "Just the push event"
//!
//! La firma `X-Hub-Signature-256` (HMAC-SHA256 del cuerpo crudo) se
//! verifica en tiempo constante ANTES de tocar la base de datos.
//! A diferencia de `CRON_SECRET` en `api/outbox.rs`, aquí NO hay modo
//! abierto sin secreto configurado: un webhook público sin firma
//! sería una invitación a disparar reconciliaciones (llamadas de pago
//! a la API de GitHub y a Gemini) para cualquiera que adivine la URL.

#![forbid(unsafe_code)]

use axum::body::Bytes;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Router;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use vercel_entry::db;
use vercel_runtime::axum::VercelLayer;
use vercel_runtime::{run, Error};

type HmacSha256 = Hmac<Sha256>;

/// Compara la firma `sha256=<hex>` que manda GitHub contra el HMAC
/// del cuerpo crudo calculado con nuestro secreto. `verify_slice` de
/// la crate `hmac` compara en tiempo constante — evita timing attacks
/// que un `==` sobre `String` no evitaría.
fn firma_valida(secret: &str, body: &[u8], header: Option<&str>) -> bool {
    let Some(header) = header else { return false };
    let Some(hex_sig) = header.strip_prefix("sha256=") else { return false };
    let Ok(sig_bytes) = hex::decode(hex_sig) else { return false };
    let Ok(mut mac) = HmacSha256::new_from_slice(secret.as_bytes()) else { return false };
    mac.update(body);
    mac.verify_slice(&sig_bytes).is_ok()
}

async fn webhook_handler(headers: HeaderMap, body: Bytes) -> Response {
    let Ok(secret) = std::env::var("GITHUB_WEBHOOK_SECRET") else {
        return (StatusCode::SERVICE_UNAVAILABLE, "GITHUB_WEBHOOK_SECRET no configurado")
            .into_response();
    };

    let signature = headers.get("x-hub-signature-256").and_then(|v| v.to_str().ok());
    if !firma_valida(&secret, &body, signature) {
        return (StatusCode::UNAUTHORIZED, "firma inválida").into_response();
    }

    // GitHub manda un "ping" al crear el webhook, y (si se configuró
    // mal en la UI) podría mandar otros eventos además de "push" — en
    // ambos casos respondemos 200 sin trabajar, para no acumular
    // fallos que lleven a GitHub a desactivar el webhook.
    let event = headers.get("x-github-event").and_then(|v| v.to_str().ok()).unwrap_or("");
    if event == "ping" {
        return (StatusCode::OK, "pong").into_response();
    }
    if event != "push" {
        return (StatusCode::OK, "evento ignorado").into_response();
    }

    let payload: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return (StatusCode::BAD_REQUEST, "payload inválido").into_response(),
    };

    // Ignorar pushes a ramas que no sea la que sirve GithubStore
    // (mismo criterio que `GITHUB_BRANCH` en github-store::from_env).
    let branch = std::env::var("GITHUB_BRANCH").unwrap_or_else(|_| "main".into());
    let target_ref = format!("refs/heads/{branch}");
    if payload["ref"].as_str() != Some(target_ref.as_str()) {
        return (StatusCode::OK, "rama distinta, ignorado").into_response();
    }

    let db_url = match std::env::var("POSTGRES_URL") {
        Ok(v) => v,
        Err(_) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "POSTGRES_URL no configurado")
                .into_response();
        }
    };
    let gemini_key = std::env::var("GEMINI_API_KEY").ok();

    let pool = db::get_db_pool(&db_url).await;
    let client = reqwest::Client::new();

    let github_store = match github_store::GithubStore::from_env() {
        Ok(store) => store,
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("GithubStore: {e}"))
                .into_response();
        }
    };

    match outbox_worker::reconcile_github_to_supabase(
        &pool,
        &client,
        &github_store,
        gemini_key.as_deref(),
    )
    .await
    {
        Ok(_) => (StatusCode::OK, "reconciliado").into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("error reconciliando: {e}"),
        )
            .into_response(),
    }
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let router = Router::new().fallback(webhook_handler);
    let app = tower::ServiceBuilder::new().layer(VercelLayer::new()).service(router);
    run(app).await
}
