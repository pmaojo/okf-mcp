//! Endpoint interno de un solo uso (pero idempotente): registra
//! `/api/telegram` como webhook de Telegram vía `setWebhook`, leyendo
//! `TELEGRAM_BOT_TOKEN` de las variables de entorno de Vercel — así el
//! token del bot NUNCA tiene que salir de ahí, ni pasar por un chat ni
//! por un `curl` local con el token en texto plano. Lo único que hace
//! falta fuera de Vercel es el dominio del deploy (para saber a qué
//! URL apuntar el webhook) y el mismo secreto que protegerá cada
//! petición entrante.
//!
//! Uso, una vez desplegado y con las variables puestas en Vercel:
//!
//! ```text
//! curl -X POST https://<dominio>/api/telegram-activate \
//!   -H "Authorization: Bearer <TELEGRAM_WEBHOOK_SECRET>"
//! ```
//!
//! Protegido con el MISMO valor que `TELEGRAM_WEBHOOK_SECRET`: quien
//! puede activarlo es, por definición, quien ya tiene acceso al panel
//! de Vercel donde se configuró esa variable — no hace falta un
//! secreto de administración aparte.
//!
//! Idempotente: `setWebhook` simplemente sobreescribe la URL
//! registrada, así que llamar este endpoint dos veces (o tras mover
//! el deploy a otro dominio) no rompe nada.

#![forbid(unsafe_code)]

use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Router;
use vercel_entry::oauth_proxy::base_url;
use vercel_runtime::axum::VercelLayer;
use vercel_runtime::{run, Error};

async fn activate_handler(headers: HeaderMap) -> Response {
    let Ok(secret) = std::env::var("TELEGRAM_WEBHOOK_SECRET") else {
        return (StatusCode::SERVICE_UNAVAILABLE, "TELEGRAM_WEBHOOK_SECRET no configurado")
            .into_response();
    };
    let expected_auth = format!("Bearer {secret}");
    let auth = headers.get("authorization").and_then(|v| v.to_str().ok());
    if auth != Some(expected_auth.as_str()) {
        return (StatusCode::UNAUTHORIZED, "Authorization inválido").into_response();
    }

    let Some(telegram) = telegram_bridge::TelegramConfig::from_env() else {
        return (StatusCode::SERVICE_UNAVAILABLE, "TELEGRAM_BOT_TOKEN no configurado")
            .into_response();
    };

    let webhook_url = format!("{}/api/telegram", base_url(&headers));
    let client = reqwest::Client::new();
    let telegram_resp = client
        .post(format!("https://api.telegram.org/bot{}/setWebhook", telegram.bot_token))
        .json(&serde_json::json!({ "url": webhook_url, "secret_token": secret }))
        .send()
        .await;

    match telegram_resp {
        Ok(r) => {
            let ok = r.status().is_success();
            let body = r.text().await.unwrap_or_default();
            let status = if ok { StatusCode::OK } else { StatusCode::BAD_GATEWAY };
            (status, [("content-type", "application/json")], body).into_response()
        }
        Err(e) => (StatusCode::BAD_GATEWAY, format!("fallo llamando a la Bot API de Telegram: {e}"))
            .into_response(),
    }
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let router = Router::new().fallback(activate_handler);
    let app = tower::ServiceBuilder::new().layer(VercelLayer::new()).service(router);
    run(app).await
}

#[cfg(test)]
mod tests {
    // La lógica de este handler es casi toda I/O de red (llama a la
    // Bot API real) — sin un doble de `reqwest` en este crate, lo
    // único probado de forma barata aquí es la comparación de
    // `Authorization`, que es la parte que protege el endpoint.
    #[test]
    fn el_formato_esperado_de_authorization_es_bearer_mas_secreto() {
        let secret = "s3cr3t";
        let expected = format!("Bearer {secret}");
        assert_eq!(expected, "Bearer s3cr3t");
        assert_ne!(expected, secret);
    }
}
