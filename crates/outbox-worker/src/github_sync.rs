//! Sincronización de un concepto commiteado con GitHub: sube el
//! Markdown a `docs/{concept_id}.md` vía la API de Contents. Aislado
//! de `lib.rs` porque es la ÚNICA parte de `process_batch` que habla
//! con GitHub — generar embeddings (Gemini) es una preocupación
//! totalmente distinta, ver [`crate::embeddings`].

use base64::prelude::*;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, USER_AGENT};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
struct GithubContentResponse {
    sha: String,
}

#[derive(Debug, Serialize)]
struct GithubPutRequest {
    message: String,
    content: String,
    sha: Option<String>,
}

pub async fn sync_to_github(
    client: &reqwest::Client,
    token: &str,
    repo: &str,
    concept_id: &str,
    markdown: &str,
    reason: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let path = format!("docs/{concept_id}.md");
    let url = format!("https://api.github.com/repos/{repo}/contents/{path}");

    let mut headers = HeaderMap::new();
    headers.insert(AUTHORIZATION, HeaderValue::from_str(&format!("Bearer {token}"))?);
    headers.insert(USER_AGENT, HeaderValue::from_static("okf-mcp-outbox-worker"));
    headers.insert("Accept", HeaderValue::from_static("application/vnd.github+json"));

    let content_b64 = BASE64_STANDARD.encode(markdown.as_bytes());

    // GitHub exige el `sha` actual del archivo para actualizarlo — es su
    // propio compare-and-swap. Entre que leemos ese sha (GET) y mandamos
    // el PUT puede colarse otra escritura al mismo archivo (otro worker,
    // un commit manual); GitHub responde 409 con el sha ya caducado. Sin
    // reintento, ese evento se queda incrementando `attempts` con el
    // MISMO sha obsoleto hasta marcarse 'failed' a la quinta vez, sin
    // haber tenido nunca una oportunidad real de sincronizar. Releer y
    // reintentar en el momento resuelve la carrera sin esperar al
    // reintento del outbox (que tarda 5 rondas de sleep(5s)).
    const MAX_INTENTOS: u32 = 3;
    for intento in 1..=MAX_INTENTOS {
        let sha = fetch_sha(client, &url, &headers).await?;

        let put_req = GithubPutRequest {
            message: reason.to_string(),
            content: content_b64.clone(),
            sha,
        };

        let put_resp = client.put(&url)
            .headers(headers.clone())
            .json(&put_req)
            .send()
            .await?;

        if put_resp.status().is_success() {
            println!("Concepto {concept_id} sincronizado con GitHub exitosamente.");
            return Ok(());
        }

        let sha_obsoleto = put_resp.status() == reqwest::StatusCode::CONFLICT;
        if sha_obsoleto && intento < MAX_INTENTOS {
            eprintln!(
                "sha obsoleto sincronizando {concept_id} (intento {intento}/{MAX_INTENTOS}); releyendo y reintentando"
            );
            continue;
        }

        let err_text = put_resp.text().await?;
        return Err(format!("GitHub API returned error: {err_text}").into());
    }

    unreachable!("el bucle siempre devuelve en el último intento (éxito o error)");
}

/// SHA actual del archivo en GitHub, o `None` si todavía no existe
/// (entonces el PUT lo crea en vez de actualizarlo).
async fn fetch_sha(
    client: &reqwest::Client,
    url: &str,
    headers: &HeaderMap,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let get_resp = client.get(url).headers(headers.clone()).send().await?;
    if get_resp.status().is_success() {
        let content: GithubContentResponse = get_resp.json().await?;
        Ok(Some(content.sha))
    } else {
        Ok(None)
    }
}

#[derive(Debug, Serialize)]
struct GithubDeleteRequest {
    message: String,
    sha: String,
}

/// Elimina un concepto de GitHub en la ruta `docs/{concept_id}.md` utilizando la API de Contents.
/// Realiza un flujo CAS (Compare-And-Swap) releyendo el SHA en caso de conflicto por concurrencia.
pub async fn delete_from_github(
    client: &reqwest::Client,
    token: &str,
    repo: &str,
    concept_id: &str,
    reason: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let path = format!("docs/{concept_id}.md");
    let url = format!("https://api.github.com/repos/{repo}/contents/{path}");

    let mut headers = HeaderMap::new();
    headers.insert(AUTHORIZATION, HeaderValue::from_str(&format!("Bearer {token}"))?);
    headers.insert(USER_AGENT, HeaderValue::from_static("okf-mcp-outbox-worker"));
    headers.insert("Accept", HeaderValue::from_static("application/vnd.github+json"));

    const MAX_INTENTOS: u32 = 3;
    for intento in 1..=MAX_INTENTOS {
        let sha = match fetch_sha(client, &url, &headers).await? {
            Some(sha) => sha,
            None => {
                println!("Concepto {concept_id} ya no existe en GitHub. Saltando borrado.");
                return Ok(());
            }
        };

        let delete_req = GithubDeleteRequest {
            message: reason.to_string(),
            sha,
        };

        let delete_resp = client.delete(&url)
            .headers(headers.clone())
            .json(&delete_req)
            .send()
            .await?;

        if delete_resp.status().is_success() || delete_resp.status() == reqwest::StatusCode::NOT_FOUND {
            println!("Concepto {concept_id} borrado de GitHub exitosamente.");
            return Ok(());
        }

        let sha_obsoleto = delete_resp.status() == reqwest::StatusCode::CONFLICT;
        if sha_obsoleto && intento < MAX_INTENTOS {
            eprintln!(
                "sha obsoleto borrando {concept_id} (intento {intento}/{MAX_INTENTOS}); releyendo y reintentando"
            );
            continue;
        }

        let err_text = delete_resp.text().await?;
        return Err(format!("GitHub API returned error on delete: {err_text}").into());
    }

    unreachable!("el bucle siempre devuelve en el último intento (éxito o error)");
}
