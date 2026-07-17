#![forbid(unsafe_code)]

use jsonwebtoken::{decode, decode_header, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

#[derive(Debug, Clone)]
pub struct AuthError(pub String);

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    sub: String,
    iss: Option<String>,
    aud: Option<String>,
    exp: usize,
    client_id: Option<String>,
    azp: Option<String>,
}

/// Una llave JWK puede ser RSA (`n`, `e`) o EC (`crv`, `x`, `y`) — el
/// tipo lo indica `kty`. Supabase Auth firma con ES256 (EC P-256)
/// por defecto, así que ambas formas deben soportarse: un proveedor
/// distinto (o una futura rotación de Supabase) podría usar RSA.
#[derive(Debug, Deserialize, Clone)]
struct Jwk {
    kid: String,
    kty: String,
    #[serde(default)]
    n: Option<String>,
    #[serde(default)]
    e: Option<String>,
    #[serde(default)]
    crv: Option<String>,
    #[serde(default)]
    x: Option<String>,
    #[serde(default)]
    y: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Jwks {
    keys: Vec<Jwk>,
}

struct JwksCache {
    keys: Vec<Jwk>,
    expires_at: Instant,
}

static JWKS_CACHE: OnceLock<Arc<RwLock<Option<JwksCache>>>> = OnceLock::new();

fn get_cache() -> &'static Arc<RwLock<Option<JwksCache>>> {
    JWKS_CACHE.get_or_init(|| Arc::new(RwLock::new(None)))
}

async fn fetch_jwks(jwks_url: &str) -> Result<Vec<Jwk>, AuthError> {
    let client = reqwest::Client::new();
    let resp = client
        .get(jwks_url)
        .send()
        .await
        .map_err(|e| AuthError(format!("failed to fetch JWKS: {e}")))?;

    let jwks = resp
        .json::<Jwks>()
        .await
        .map_err(|e| AuthError(format!("failed to parse JWKS: {e}")))?;

    Ok(jwks.keys)
}

/// Fabrica la llave de decodificación según el tipo de llave (`kty`).
/// Supabase Auth usa EC (P-256 / ES256); RSA se soporta también por
/// si el proyecto o un proveedor futuro firma con RS256.
fn decoding_key_from_jwk(jwk: &Jwk) -> Result<DecodingKey, AuthError> {
    match jwk.kty.as_str() {
        "RSA" => {
            let (n, e) = jwk
                .n
                .as_deref()
                .zip(jwk.e.as_deref())
                .ok_or_else(|| AuthError("RSA JWK missing n/e components".to_string()))?;
            DecodingKey::from_rsa_components(n, e)
                .map_err(|e| AuthError(format!("invalid RSA key components: {e}")))
        }
        "EC" => {
            // `jsonwebtoken::DecodingKey::from_ec_components` solo entiende
            // P-256 (ES256); rechazamos otras curvas explícitamente en vez
            // de dejar que la librería infiera algo incorrecto.
            if jwk.crv.as_deref() != Some("P-256") {
                return Err(AuthError(format!(
                    "unsupported EC curve: {:?} (only P-256/ES256 is supported)",
                    jwk.crv
                )));
            }
            let (x, y) = jwk
                .x
                .as_deref()
                .zip(jwk.y.as_deref())
                .ok_or_else(|| AuthError("EC JWK missing x/y components".to_string()))?;
            DecodingKey::from_ec_components(x, y)
                .map_err(|e| AuthError(format!("invalid EC key components: {e}")))
        }
        other => Err(AuthError(format!("unsupported key type: {other}"))),
    }
}

async fn get_decoding_key(jwks_url: &str, kid: &str) -> Result<DecodingKey, AuthError> {
    let cache_lock = get_cache();

    // Comprobación inicial bajo bloqueo de lectura
    {
        let r = cache_lock.read().await;
        if let Some(cache) = &*r {
            if cache.expires_at > Instant::now() {
                if let Some(key) = cache.keys.iter().find(|k| k.kid == kid) {
                    return decoding_key_from_jwk(key);
                }
            }
        }
    }

    // Recargar claves bajo bloqueo de escritura si expiró o no se encontró el kid
    let mut w = cache_lock.write().await;
    if let Some(cache) = &*w {
        if cache.expires_at > Instant::now() {
            if let Some(key) = cache.keys.iter().find(|k| k.kid == kid) {
                return decoding_key_from_jwk(key);
            }
        }
    }

    let keys = fetch_jwks(jwks_url).await?;
    *w = Some(JwksCache {
        keys: keys.clone(),
        expires_at: Instant::now() + Duration::from_secs(3600 * 24), // Caché de 24 horas
    });

    if let Some(key) = keys.iter().find(|k| k.kid == kid) {
        decoding_key_from_jwk(key)
    } else {
        Err(AuthError(format!("key id {kid} not found in JWKS")))
    }
}

/// `expected_client_id`: si se pasa, restringe la aceptación a tokens
/// cuyo claim `client_id` (quién los pidió) coincida exactamente —
/// NO es una audiencia JWT (`aud`) en el sentido estándar: Supabase
/// Auth siempre firma `aud: "authenticated"` para todos sus tokens.
pub async fn validate_jwt(
    auth_header: Option<&str>,
    jwks_url: &str,
    expected_client_id: Option<&str>,
) -> Result<memory_model::Principal, AuthError> {
    let header_val = auth_header.ok_or_else(|| AuthError("missing Authorization header".to_string()))?;
    if !header_val.starts_with("Bearer ") {
        return Err(AuthError("invalid token format, must be Bearer".to_string()));
    }
    let token = &header_val[7..];

    let header = decode_header(token).map_err(|e| AuthError(format!("invalid token header: {e}")))?;
    let kid = header.kid.ok_or_else(|| AuthError("missing kid in token header".to_string()))?;

    let decoding_key = get_decoding_key(jwks_url, &kid).await?;

    // Supabase Auth firma TODOS sus JWTs con `aud: "authenticated"` —
    // es el rol de Postgres al que se autentica PostgREST, no tiene
    // nada que ver con qué cliente OAuth pidió el token (confirmado
    // contra la guía "Token Security & RLS" de Supabase: la
    // audiencia siempre es `authenticated`; el cliente que emitió el
    // token va en un claim aparte, `client_id`). Validar `aud` contra
    // el client_id esperado — como hacía esta función antes —
    // rechaza SIEMPRE, porque esa comparación nunca puede coincidir.
    let mut validation = Validation::new(header.alg);
    validation.validate_aud = false;

    let token_data = decode::<Claims>(token, &decoding_key, &validation)
        .map_err(|e| AuthError(format!("token validation failed: {e}")))?;

    let claims = token_data.claims;
    let client_id = claims
        .client_id
        .or(claims.azp)
        .unwrap_or_else(|| "unknown".to_string());

    // La restricción real de "solo tokens emitidos para ESTA app" —
    // que exige la especificación de autorización de MCP — se hace
    // aquí, comparando el claim `client_id` real del token.
    if let Some(expected_client_id) = expected_client_id {
        if client_id != expected_client_id {
            return Err(AuthError(format!(
                "token issued for a different OAuth client (got {client_id}, expected {expected_client_id})"
            )));
        }
    }

    Ok(memory_model::Principal {
        subject: claims.sub,
        client_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Respuesta real de `https://<proyecto>.supabase.co/auth/v1/.well-known/jwks.json`.
    /// Supabase Auth firma con ES256 (EC P-256), no RSA — este test
    /// habría fallado con `missing field n` antes de soportar `kty: "EC"`.
    #[test]
    fn parsea_jwks_ec_real_de_supabase() {
        let body = r#"{"keys":[{"alg":"ES256","crv":"P-256","ext":true,"key_ops":["verify"],"kid":"2f8b0ae7-873d-4eb1-b3f9-2a2e3cb1756a","kty":"EC","use":"sig","x":"49FWTTSm0IZInGtUGeoNCCfnYfKNtq3nI9zFVSxHmR8","y":"HFDKkCL1Tsp96ezQbwDXiPoFcD8ebgxUAvgUdQTlpHk"}]}"#;
        let jwks: Jwks = serde_json::from_str(body).expect("debe parsear JWKS EC sin error");
        let key = &jwks.keys[0];
        assert_eq!(key.kty, "EC");
        decoding_key_from_jwk(key).expect("debe fabricar una DecodingKey EC válida");
    }

    #[test]
    fn rechaza_curva_ec_no_soportada() {
        let mut jwk = Jwk {
            kid: "x".to_string(),
            kty: "EC".to_string(),
            n: None,
            e: None,
            crv: Some("P-384".to_string()),
            x: Some("AA".to_string()),
            y: Some("AA".to_string()),
        };
        assert!(decoding_key_from_jwk(&jwk).is_err());
        jwk.crv = None;
        assert!(decoding_key_from_jwk(&jwk).is_err());
    }
}
