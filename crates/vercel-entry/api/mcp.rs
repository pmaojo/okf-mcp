//! Adaptador Vercel: el bridge de la función serverless a
//! `mcp_http::route`, la MISMA lógica pura que sirve el transporte
//! local (`crates/mcp-http`, probado con `TcpListener` de verdad en
//! el tutorial, capítulo 10). Esta es la ÚNICA pieza nueva de este
//! archivo: traducir entre el `axum::Request` que entrega
//! `vercel_runtime` y `mcp_http::{HttpRequest, HttpResponse}`.
//!
//! Todo lo demás — parseo JSON-RPC, las cuatro herramientas, el
//! presupuesto, la comprobación de `Origin` — es exactamente el
//! código que ya corre con `cargo run -p mcp-http`.

#![forbid(unsafe_code)]

use vercel_entry::auth;

use axum::body::Bytes;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use mcp_core::McpServer;
use mcp_http::{route, HttpRequest};
use memory_tools::MemoryTools;
use memory_model::{Budget, Principal};
use supabase_store::SupabaseStore;
use std::sync::OnceLock;
use tower::ServiceBuilder;
use tower_http::cors::{AllowHeaders, AllowOrigin, CorsLayer};
use vercel_runtime::axum::VercelLayer;
use vercel_runtime::{run, Error};

static DB_POOL: OnceLock<sqlx::PgPool> = OnceLock::new();

/// Obtiene o inicializa el pool de conexiones de base de datos de manera thread-safe.
///
/// En el primer cold start de cada instancia serverless, aplica
/// `schema.sql` contra `POSTGRES_URL` (mismo patrón que
/// `outbox-worker`, ver `crates/outbox-worker/src/main.rs`). Es
/// idempotente (`CREATE TABLE IF NOT EXISTS`), así que repetirlo en
/// cada deploy/instancia nueva es seguro.
async fn get_db_pool(db_url: &str) -> sqlx::PgPool {
    if let Some(pool) = DB_POOL.get() {
        return pool.clone();
    }
    let pool = sqlx::PgPool::connect(db_url)
        .await
        .expect("Failed to connect to Supabase PostgreSQL database");
    // `schema.sql` trae varias sentencias separadas por `;`. `sqlx::query`
    // usa el protocolo extendido (prepared statement), que Postgres
    // rechaza si el texto trae más de un comando ("cannot insert
    // multiple commands into a prepared statement"). `raw_sql` usa el
    // protocolo simple, que sí soporta scripts multi-sentencia.
    sqlx::raw_sql(include_str!("../../supabase-store/schema.sql"))
        .execute(&pool)
        .await
        .expect("Failed to apply database schema");
    let _ = DB_POOL.set(pool.clone());
    pool
}

/// Lista separada por comas en la variable de entorno
/// `ALLOWED_ORIGINS` del proyecto Vercel.
fn allowed_origins() -> Vec<String> {
    std::env::var("ALLOWED_ORIGINS")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Cabeceras CORS (incluye la respuesta al preflight `OPTIONS`), sin
/// las cuales un cliente MCP que llame desde el navegador (Claude.ai
/// hace `fetch` directo al servidor MCP, no vía su backend) nunca
/// llega a ver la respuesta: el navegador corta la petición en el
/// preflight antes de que exista Authorization que validar. Reutiliza
/// la misma lista de `ALLOWED_ORIGINS` que ya usa `mcp_http::route`
/// para la comprobación anti DNS-rebinding — misma semántica: lista
/// vacía es permisivo, lista no vacía restringe.
fn cors_layer() -> CorsLayer {
    let origins = allowed_origins();
    CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(move |origin: &HeaderValue, _| {
            origins.is_empty()
                || origin
                    .to_str()
                    .map(|o| origins.iter().any(|allowed| allowed == o))
                    .unwrap_or(false)
        }))
        .allow_methods([Method::GET, Method::POST, Method::DELETE])
        .allow_headers(AllowHeaders::mirror_request())
}

/// Reconstruye el origen público (esquema + host) a partir de las
/// cabeceras de la petición, para anunciar URLs absolutas en la
/// metadata de descubrimiento OAuth (RFC 9728) sin hardcodear el
/// dominio de despliegue (útil también en preview deployments).
fn base_url(headers: &HeaderMap) -> String {
    let host = headers.get("host").and_then(|v| v.to_str().ok()).unwrap_or("localhost");
    let scheme = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("https");
    format!("{scheme}://{host}")
}

/// `GET /.well-known/oauth-protected-resource` (RFC 9728): le dice a
/// un cliente MCP OAuth-aware (p. ej. Claude) contra qué
/// Authorization Server debe autenticarse antes de llamar a `/mcp`.
/// El AS mismo (aquí, Supabase Auth) expone su propio descubrimiento
/// OIDC en `<issuer>/.well-known/openid-configuration`.
async fn protected_resource_metadata_handler(headers: HeaderMap) -> Response {
    let issuer = match std::env::var("OAUTH_ISSUER") {
        Ok(v) => v,
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
    };
    let body = format!(
        r#"{{"resource":"{}/mcp","authorization_servers":["{issuer}"],"bearer_methods_supported":["header"]}}"#,
        base_url(&headers)
    );
    (StatusCode::OK, [("content-type", "application/json")], body).into_response()
}

/// `GET /authorize`: proxy transparente hacia el `authorization_endpoint`
/// real de Supabase.
///
/// Existe por una limitación observada en el modo "Client ID manual"
/// de Claude para conectores MCP: en vez de seguir el
/// `authorization_servers` que anunciamos en
/// `/.well-known/oauth-protected-resource` (RFC 9728), asume que el
/// propio servidor MCP aloja el Authorization Server en su mismo
/// dominio. En vez de pelearnos con eso, se lo damos: este endpoint
/// reenvía la query string tal cual al `authorization_endpoint` real,
/// sin tocar ni un parámetro (ni siquiera el `code_challenge` —
/// cualquier re-serialización podría cambiar el encoding y romper la
/// verificación PKCE en el otro extremo).
async fn authorize_proxy_handler(uri: Uri) -> Response {
    let issuer = match std::env::var("OAUTH_ISSUER") {
        Ok(v) => v,
        Err(_) => return (StatusCode::NOT_FOUND, "OAUTH_ISSUER no configurado").into_response(),
    };
    let query = uri.query().unwrap_or("");
    let location = format!("{issuer}/oauth/authorize?{query}");
    (StatusCode::FOUND, [("location", location)]).into_response()
}

/// `POST /token`: reenvía tal cual al `token_endpoint` real de
/// Supabase y devuelve su respuesta sin modificar.
///
/// Reenvía también `Authorization` si viene presente: si la OAuth App
/// en Supabase está registrada para `client_secret_basic` (no
/// `none`), el `client_secret` viaja ahí, en Basic Auth — no en el
/// cuerpo. El proxy nunca LEE ni construye esa cabecera, solo la
/// deja pasar: sigue sin custodiar ningún secreto propio.
async fn token_proxy_handler(headers: HeaderMap, body: Bytes) -> Response {
    let issuer = match std::env::var("OAUTH_ISSUER") {
        Ok(v) => v,
        Err(_) => return (StatusCode::NOT_FOUND, "OAUTH_ISSUER no configurado").into_response(),
    };
    let content_type = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/x-www-form-urlencoded")
        .to_string();

    let client = reqwest::Client::new();
    let mut req = client
        .post(format!("{issuer}/oauth/token"))
        .header("content-type", content_type);
    if let Some(auth) = headers.get("authorization") {
        req = req.header("authorization", auth.clone());
    }
    let upstream = match req.body(body.to_vec()).send().await {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                format!("no se pudo contactar con el Authorization Server: {e}"),
            )
                .into_response();
        }
    };

    let status = StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let resp_content_type = upstream
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/json")
        .to_string();
    let bytes = match upstream.bytes().await {
        Ok(b) => b,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                format!("no se pudo leer la respuesta del Authorization Server: {e}"),
            )
                .into_response();
        }
    };

    (status, [("content-type", resp_content_type)], bytes).into_response()
}

/// Plantilla de la UI de consentimiento del OAuth Server de Supabase
/// Auth. Supabase NO aloja una pantalla de consentimiento propia — la
/// documentación es explícita en que construir este frontend es
/// responsabilidad del desarrollador (guía "Build a Custom OAuth
/// Server"). Las tres llamadas REST que usa (`GET
/// /oauth/authorizations/{id}`, `POST
/// /oauth/authorizations/{id}/consent` con `{"action":"approve"}` o
/// `{"action":"deny"}`, y `POST /token?grant_type=password` para el
/// login) no están documentadas como REST crudo en la guía — se
/// verificaron leyendo `_getAuthorizationDetails` /
/// `_approveAuthorization` / `_denyAuthorization` en el código fuente
/// de `@supabase/auth-js` (`GoTrueClient.ts`), que es lo que
/// `supabase.auth.oauth.*` envuelve.
///
/// La sesión de usuario (login) se guarda en `sessionStorage` del
/// navegador, nunca llega a este servidor: todas las llamadas a
/// Supabase Auth las hace el JS de la página directamente contra
/// `OAUTH_ISSUER`, con la anon key pública (`SUPABASE_ANON_KEY`, no es
/// secreta — es la misma que llevaría cualquier cliente `supabase-js`
/// en el navegador).
const CONSENT_PAGE_TEMPLATE: &str = r#"<!doctype html>
<html lang="es">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Autorizar acceso — okf-mcp</title>
<style>
  :root { color-scheme: light dark; }
  body { font-family: -apple-system, system-ui, sans-serif; max-width: 420px; margin: 10vh auto; padding: 0 1.5rem; }
  h1 { font-size: 1.25rem; }
  .status { margin: 1rem 0; padding: .75rem 1rem; border-radius: 8px; background: #eef; }
  .status.error { background: #fee; color: #900; }
  form, #consent-view { display: none; }
  label { display: block; margin-top: .75rem; font-size: .9rem; }
  input { width: 100%; padding: .5rem; margin-top: .25rem; box-sizing: border-box; }
  button { margin-top: 1rem; padding: .6rem 1rem; cursor: pointer; }
  #approve-btn { background: #16a34a; color: white; border: none; border-radius: 6px; }
  #deny-btn { background: transparent; border: 1px solid #999; border-radius: 6px; margin-left: .5rem; }
  ul#scope-list { padding-left: 1.2rem; }
  .error-text { color: #900; font-size: .85rem; min-height: 1.2em; }
</style>
</head>
<body>
  <h1>Autorizar acceso a okf-mcp</h1>
  <div id="status" class="status" style="display:none"></div>

  <form id="login-form">
    <p>Inicia sesión para continuar.</p>
    <label for="email">Email</label>
    <input id="email" type="email" required autocomplete="email">
    <label for="password">Contraseña</label>
    <input id="password" type="password" required autocomplete="current-password">
    <div id="login-error" class="error-text"></div>
    <button type="submit">Iniciar sesión</button>
  </form>

  <div id="consent-view">
    <p><strong id="client-name"></strong> solicita acceso a tu cuenta con los siguientes permisos:</p>
    <ul id="scope-list"></ul>
    <button id="approve-btn" type="button">Permitir</button>
    <button id="deny-btn" type="button">Denegar</button>
  </div>

<script>
const ISSUER = __OAUTH_ISSUER_JSON__;
const ANON_KEY = __SUPABASE_ANON_KEY_JSON__;
const SESSION_KEY = "okf_consent_session";

const params = new URLSearchParams(window.location.search);
const authorizationId = params.get("authorization_id");

const els = {
  status: document.getElementById("status"),
  loginForm: document.getElementById("login-form"),
  loginError: document.getElementById("login-error"),
  consentView: document.getElementById("consent-view"),
  clientName: document.getElementById("client-name"),
  scopeList: document.getElementById("scope-list"),
  approveBtn: document.getElementById("approve-btn"),
  denyBtn: document.getElementById("deny-btn"),
};

function showStatus(msg, isError) {
  els.status.textContent = msg;
  els.status.style.display = msg ? "block" : "none";
  els.status.className = isError ? "status error" : "status";
}

function getSession() {
  try {
    const raw = sessionStorage.getItem(SESSION_KEY);
    if (!raw) return null;
    const session = JSON.parse(raw);
    if (!session.access_token || !session.expires_at) return null;
    if (Date.now() / 1000 >= session.expires_at - 30) return null;
    return session;
  } catch (e) {
    return null;
  }
}

function saveSession(tokenResponse) {
  const session = {
    access_token: tokenResponse.access_token,
    expires_at: Math.floor(Date.now() / 1000) + (tokenResponse.expires_in || 3600),
  };
  sessionStorage.setItem(SESSION_KEY, JSON.stringify(session));
  return session;
}

async function gotrueFetch(path, accessToken, init) {
  const headers = Object.assign(
    {
      apikey: ANON_KEY,
      Authorization: "Bearer " + (accessToken || ANON_KEY),
      "Content-Type": "application/json",
    },
    (init && init.headers) || {}
  );
  const resp = await fetch(ISSUER + path, Object.assign({}, init, { headers }));
  const body = await resp.json().catch(() => ({}));
  if (!resp.ok) {
    const message = body.error_description || body.msg || body.error || ("HTTP " + resp.status);
    throw new Error(message);
  }
  return body;
}

async function login(email, password) {
  const tokenResponse = await gotrueFetch("/token?grant_type=password", null, {
    method: "POST",
    body: JSON.stringify({ email: email, password: password }),
  });
  return saveSession(tokenResponse);
}

async function loadAuthorization(session) {
  const details = await gotrueFetch(
    "/oauth/authorizations/" + encodeURIComponent(authorizationId),
    session.access_token,
    { method: "GET" }
  );
  // La API devuelve solo {redirect_url} cuando el usuario ya había
  // consentido antes: no hay nada que mostrar, se redirige directo.
  if (details.redirect_url && Object.keys(details).length === 1) {
    window.location.href = details.redirect_url;
    return null;
  }
  return details;
}

async function decide(session, action) {
  const result = await gotrueFetch(
    "/oauth/authorizations/" + encodeURIComponent(authorizationId) + "/consent",
    session.access_token,
    { method: "POST", body: JSON.stringify({ action: action }) }
  );
  if (result.redirect_url) {
    window.location.href = result.redirect_url;
  } else {
    showStatus("Respuesta inesperada del servidor de autorización.", true);
  }
}

function renderConsent(details) {
  els.loginForm.style.display = "none";
  els.consentView.style.display = "block";
  els.clientName.textContent = details.client_name || details.client_id || "Aplicación desconocida";
  els.scopeList.innerHTML = "";
  const scopes = (details.scope || "").split(" ").filter(Boolean);
  if (scopes.length === 0) {
    const li = document.createElement("li");
    li.textContent = "(sin scopes declarados)";
    els.scopeList.appendChild(li);
  } else {
    scopes.forEach(function (scope) {
      const li = document.createElement("li");
      li.textContent = scope;
      els.scopeList.appendChild(li);
    });
  }
}

async function boot() {
  if (!authorizationId) {
    showStatus("Falta el parámetro authorization_id en la URL.", true);
    return;
  }
  const session = getSession();
  if (!session) {
    els.loginForm.style.display = "block";
    return;
  }
  showStatus("Cargando detalles de la autorización…", false);
  try {
    const details = await loadAuthorization(session);
    if (!details) return; // ya redirigido
    showStatus("", false);
    renderConsent(details);
  } catch (err) {
    sessionStorage.removeItem(SESSION_KEY);
    els.loginForm.style.display = "block";
    showStatus("", false);
    els.loginError.textContent = "Tu sesión expiró, vuelve a iniciar sesión: " + err.message;
  }
}

els.loginForm.addEventListener("submit", async function (e) {
  e.preventDefault();
  els.loginError.textContent = "";
  const email = document.getElementById("email").value;
  const password = document.getElementById("password").value;
  try {
    const session = await login(email, password);
    els.loginForm.style.display = "none";
    showStatus("Cargando detalles de la autorización…", false);
    const details = await loadAuthorization(session);
    if (!details) return;
    showStatus("", false);
    renderConsent(details);
  } catch (err) {
    els.loginError.textContent = err.message;
  }
});

els.approveBtn.addEventListener("click", async function () {
  const session = getSession();
  if (!session) { location.reload(); return; }
  els.approveBtn.disabled = true;
  els.denyBtn.disabled = true;
  try {
    await decide(session, "approve");
  } catch (err) {
    showStatus(err.message, true);
    els.approveBtn.disabled = false;
    els.denyBtn.disabled = false;
  }
});

els.denyBtn.addEventListener("click", async function () {
  const session = getSession();
  if (!session) { location.reload(); return; }
  els.approveBtn.disabled = true;
  els.denyBtn.disabled = true;
  try {
    await decide(session, "deny");
  } catch (err) {
    showStatus(err.message, true);
    els.approveBtn.disabled = false;
    els.denyBtn.disabled = false;
  }
});

boot();
</script>
</body>
</html>
"#;

/// `GET /oauth/consent`: la UI de autorización del OAuth Server de
/// Supabase (configurada como Authorization Path en Authentication >
/// OAuth Server, combinada con el Site URL — ver
/// `CONSENT_PAGE_TEMPLATE`). Reutiliza `OAUTH_ISSUER` (ya usado por
/// `authorize_proxy_handler`/`token_proxy_handler`) y necesita además
/// `SUPABASE_ANON_KEY`, la clave anon pública del proyecto.
async fn consent_page_handler() -> Response {
    let issuer = match std::env::var("OAUTH_ISSUER") {
        Ok(v) => v,
        Err(_) => return (StatusCode::NOT_FOUND, "OAUTH_ISSUER no configurado").into_response(),
    };
    let anon_key = match std::env::var("SUPABASE_ANON_KEY") {
        Ok(v) => v,
        Err(_) => return (StatusCode::NOT_FOUND, "SUPABASE_ANON_KEY no configurado").into_response(),
    };
    let html = CONSENT_PAGE_TEMPLATE
        .replace(
            "__OAUTH_ISSUER_JSON__",
            &serde_json::to_string(&issuer).unwrap_or_else(|_| "\"\"".to_string()),
        )
        .replace(
            "__SUPABASE_ANON_KEY_JSON__",
            &serde_json::to_string(&anon_key).unwrap_or_else(|_| "\"\"".to_string()),
        );
    (StatusCode::OK, [("content-type", "text/html; charset=utf-8")], html).into_response()
}

/// Handler único: da igual la ruta pública que el cliente use
/// (`/mcp` vía el rewrite de `vercel.json`, o `/api/mcp` por
/// defecto) — este archivo ES semánticamente el endpoint `/mcp`.
async fn mcp_handler(method: Method, headers: HeaderMap, body: Bytes) -> Response {
    let origin = headers.get("origin").and_then(|v| v.to_str().ok()).map(str::to_string);
    let content_type = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    let http_req = HttpRequest {
        method: method.to_string(),
        path: "/mcp".to_string(),
        origin,
        content_type,
        body: body.to_vec(),
    };

    let budget = Budget::default();
    let origins = allowed_origins();

    // 1. Obtener y validar el Principal (OAuth 2.1)
    let auth_header = headers.get("authorization").and_then(|v| v.to_str().ok());
    let actor = match std::env::var("JWKS_URL") {
        Ok(jwks_url) => {
            // La especificación de autorización de MCP exige validar que
            // el token fue emitido para ESTE cliente OAuth. Supabase Auth
            // firma todo JWT con `aud: "authenticated"` (es el rol de
            // Postgres, no identifica al cliente) — lo que identifica al
            // cliente es el claim `client_id`, que es lo que
            // `validate_jwt` compara contra JWT_AUDIENCE si está presente.
            //
            // Con el registro dinámico de clientes (DCR) activado en el
            // OAuth Server de Supabase, Claude puede auto-registrarse con
            // un `client_id` nuevo en cualquier momento — fijar
            // JWT_AUDIENCE a un único cliente pre-registrado rompería la
            // conexión cada vez que eso pasa. Por eso JWT_AUDIENCE es
            // opcional: si está configurada, restringe a ese client_id
            // (útil en despliegues sin DCR, con un cliente fijo); si no,
            // se acepta cualquier token válido firmado por este Supabase
            // Auth — la seguridad real la da la sesión de usuario (login)
            // más la aprobación explícita en la pantalla de consentimiento
            // de `/oauth/consent`, no el client_id.
            let audience = std::env::var("JWT_AUDIENCE")
                .ok()
                .filter(|v| !v.is_empty());
            match auth::validate_jwt(auth_header, &jwks_url, audience.as_deref()).await {
                Ok(principal) => principal,
                Err(err) => {
                    // Cabecera exigida por la especificación de autorización
                    // de MCP: le dice al cliente dónde encontrar la metadata
                    // del recurso protegido (RFC 9728) para iniciar el flujo
                    // OAuth en lugar de fallar en silencio.
                    let metadata_url = format!(
                        "{}/.well-known/oauth-protected-resource",
                        base_url(&headers)
                    );
                    return (
                        StatusCode::UNAUTHORIZED,
                        [
                            ("content-type", "text/plain".to_string()),
                            (
                                "www-authenticate",
                                format!(r#"Bearer resource_metadata="{metadata_url}""#),
                            ),
                        ],
                        format!("Unauthorized: {}", err.0),
                    )
                        .into_response();
                }
            }
        }
        Err(_) => {
            // Si no está configurado JWKS_URL, corremos en desarrollo local/abierto sin validar
            Principal::local_dev()
        }
    };

    // 2. Obtener pool de base de datos e instanciar servicios de forma efímera
    let db_url = std::env::var("POSTGRES_URL").expect("POSTGRES_URL must be set");
    let pool = get_db_pool(&db_url).await;
    let store = SupabaseStore::new(pool);
    let tools = MemoryTools::new(store, actor, budget);
    let mut server = McpServer::new("okf-memory-vercel", env!("CARGO_PKG_VERSION"), tools);

    let resp = route(&http_req, &budget, &origins, &mut server);

    let status = StatusCode::from_u16(resp.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, [("content-type", resp.content_type)], resp.body).into_response()
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    // Rutas explícitas: la metadata de descubrimiento OAuth (RFC 9728)
    // y el proxy transparente de `/authorize` + `/token` hacia
    // Supabase. Todo lo demás cae en el fallback de siempre.
    let router = Router::new()
        .route(
            "/.well-known/oauth-protected-resource",
            get(protected_resource_metadata_handler),
        )
        .route("/authorize", get(authorize_proxy_handler))
        .route("/token", post(token_proxy_handler))
        .route("/oauth/consent", get(consent_page_handler))
        .fallback(mcp_handler)
        .layer(cors_layer());
    let app = ServiceBuilder::new().layer(VercelLayer::new()).service(router);
    run(app).await
}
