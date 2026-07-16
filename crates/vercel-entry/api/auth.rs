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

#[derive(Debug, Deserialize, Clone)]
struct Jwk {
    kid: String,
    n: String,
    e: String,
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

async fn get_decoding_key(jwks_url: &str, kid: &str) -> Result<DecodingKey, AuthError> {
    let cache_lock = get_cache();

    // Comprobación inicial bajo bloqueo de lectura
    {
        let r = cache_lock.read().await;
        if let Some(cache) = &*r {
            if cache.expires_at > Instant::now() {
                if let Some(key) = cache.keys.iter().find(|k| k.kid == kid) {
                    return DecodingKey::from_rsa_components(&key.n, &key.e)
                        .map_err(|e| AuthError(format!("invalid key components: {e}")));
                }
            }
        }
    }

    // Recargar claves bajo bloqueo de escritura si expiró o no se encontró el kid
    let mut w = cache_lock.write().await;
    if let Some(cache) = &*w {
        if cache.expires_at > Instant::now() {
            if let Some(key) = cache.keys.iter().find(|k| k.kid == kid) {
                return DecodingKey::from_rsa_components(&key.n, &key.e)
                    .map_err(|e| AuthError(format!("invalid key components: {e}")));
            }
        }
    }

    let keys = fetch_jwks(jwks_url).await?;
    *w = Some(JwksCache {
        keys: keys.clone(),
        expires_at: Instant::now() + Duration::from_secs(3600 * 24), // Caché de 24 horas
    });

    if let Some(key) = keys.iter().find(|k| k.kid == kid) {
        DecodingKey::from_rsa_components(&key.n, &key.e)
            .map_err(|e| AuthError(format!("invalid key components: {e}")))
    } else {
        Err(AuthError(format!("key id {kid} not found in JWKS")))
    }
}

pub async fn validate_jwt(
    auth_header: Option<&str>,
    jwks_url: &str,
    expected_audience: Option<&str>,
) -> Result<memory_model::Principal, AuthError> {
    let header_val = auth_header.ok_or_else(|| AuthError("missing Authorization header".to_string()))?;
    if !header_val.starts_with("Bearer ") {
        return Err(AuthError("invalid token format, must be Bearer".to_string()));
    }
    let token = &header_val[7..];

    let header = decode_header(token).map_err(|e| AuthError(format!("invalid token header: {e}")))?;
    let kid = header.kid.ok_or_else(|| AuthError("missing kid in token header".to_string()))?;

    let decoding_key = get_decoding_key(jwks_url, &kid).await?;

    let mut validation = Validation::new(header.alg);
    if let Some(aud) = expected_audience {
        validation.set_audience(&[aud]);
    } else {
        validation.validate_aud = false; // Permitir cualquier audiencia si no se especifica
    }

    let token_data = decode::<Claims>(token, &decoding_key, &validation)
        .map_err(|e| AuthError(format!("token validation failed: {e}")))?;

    let claims = token_data.claims;
    let client_id = claims
        .client_id
        .or(claims.azp)
        .unwrap_or_else(|| "unknown".to_string());

    Ok(memory_model::Principal {
        subject: claims.sub,
        client_id,
    })
}
