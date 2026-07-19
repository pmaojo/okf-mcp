//! Adaptadores HTTP de los puertos de `ingest-core`.
//!
//! [`GithubFetcher`] implementa `SourceFetcher`: acepta las mismas
//! fuentes que `npx skills add` (el atajo `owner/repo` y URLs de
//! repo, subcarpeta o archivo de GitHub), descarga los archivos de
//! texto vía la API de trees + `raw.githubusercontent.com` con topes
//! de cantidad y tamaño, y expone `license_spdx_id` (campo
//! `license.spdx_id` de la API de repos de GitHub) para que
//! `ingest-core` grabe la licencia detectada como metadato — sin
//! ningún modelo de por medio, nunca bloqueante.
//!
//! Este crate NO llama a ningún LLM: la síntesis con IA se evaluó y se
//! descartó para `skill_ingest` (reescribir con un modelo no resuelve
//! nada de licencia — sigue siendo obra derivada — y además cuesta
//! tokens/cuota por cada skill). Gemini (y su respaldo Mistral/Cohere)
//! solo se usan en este proyecto para embeddings, en `gemini-embeddings`.
//!
//! El puerto de `ingest-core` es síncrono (el núcleo no conoce
//! `tokio`); aquí se puentea con el mismo `block_on` tolerante a
//! runtime que usa `supabase-store`.

#![forbid(unsafe_code)]

use ingest_core::{IngestError, SourceFetcher, SourceFile};
use serde::Deserialize;

/// Archivos máximos que se descargan de una fuente.
pub const MAX_FILES: usize = 40;
/// Tamaño máximo de un archivo individual, en bytes.
pub const MAX_FILE_BYTES: u64 = 128 * 1024;

/// Extensiones consideradas texto ingerible. Todo lo demás (binarios,
/// imágenes, lockfiles gigantes) se ignora en el listado.
const TEXT_EXTENSIONS: &[&str] = &[
    "md", "markdown", "mdx", "txt", "rs", "py", "ts", "tsx", "js", "jsx", "json", "toml", "yaml",
    "yml", "sh", "css", "html", "sql", "go", "rb", "java", "c", "h", "cpp", "hpp",
];

/// Ejecuta un Future de forma síncrona, tolerando si ya nos encontramos
/// dentro de un runtime de Tokio (como en Axum/Vercel) o fuera de él (tests).
fn block_on<F: std::future::Future>(fut: F) -> F::Output {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => tokio::task::block_in_place(|| handle.block_on(fut)),
        Err(_) => tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(fut),
    }
}

// -------------------------------------------------------------------
// Interpretación de la fuente
// -------------------------------------------------------------------

/// A qué apunta la fuente pedida por el cliente.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Source {
    /// Un repo de GitHub, opcionalmente una ref y una subcarpeta.
    GithubTree { owner: String, repo: String, reference: Option<String>, path: String },
    /// Un archivo individual accesible por URL directa.
    SingleFile { url: String, name: String },
}

/// Interpreta la fuente. Formas aceptadas (las mismas que resuelve
/// `npx skills add` más la URL directa):
/// - `owner/repo` (atajo sin esquema)
/// - `https://github.com/owner/repo`
/// - `https://github.com/owner/repo/tree/<ref>/<sub/carpeta>`
/// - `https://github.com/owner/repo/blob/<ref>/<archivo>`
/// - `https://raw.githubusercontent.com/owner/repo/<ref>/<archivo>`
/// - cualquier otra URL `https://` → archivo individual
fn parse_source(source: &str) -> Result<Source, IngestError> {
    let s = source.trim().trim_end_matches('/');
    let invalid = |msg: &str| IngestError::InvalidSource(format!("{msg}: {source}"));

    if !s.contains("://") {
        let mut parts = s.split('/');
        let (owner, repo) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
        if owner.is_empty() || repo.is_empty() || parts.next().is_some() {
            return Err(invalid("esperaba 'owner/repo' o una URL https"));
        }
        return Ok(Source::GithubTree {
            owner: owner.to_string(),
            repo: repo.to_string(),
            reference: None,
            path: String::new(),
        });
    }

    let sin_esquema = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"))
        .ok_or_else(|| invalid("solo se aceptan URLs http(s)"))?;
    let mut segs = sin_esquema.split('/');
    let host = segs.next().unwrap_or("");
    let resto: Vec<&str> = segs.collect();

    match host {
        "github.com" | "www.github.com" => {
            if resto.len() < 2 {
                return Err(invalid("URL de GitHub sin owner/repo"));
            }
            let owner = resto[0].to_string();
            let repo = resto[1].trim_end_matches(".git").to_string();
            match resto.get(2) {
                None => Ok(Source::GithubTree { owner, repo, reference: None, path: String::new() }),
                Some(&"tree") | Some(&"blob") if resto.len() >= 4 => {
                    let reference = Some(resto[3].to_string());
                    let path = resto[4..].join("/");
                    if resto[2] == "blob" && !path.is_empty() {
                        let name = path.rsplit('/').next().unwrap_or(&path).to_string();
                        let url = format!(
                            "https://raw.githubusercontent.com/{owner}/{repo}/{}/{path}",
                            resto[3]
                        );
                        Ok(Source::SingleFile { url, name })
                    } else {
                        Ok(Source::GithubTree { owner, repo, reference, path })
                    }
                }
                Some(_) => Err(invalid("URL de GitHub no reconocida (esperaba /tree/ o /blob/)")),
            }
        }
        "raw.githubusercontent.com" => {
            if resto.len() < 4 {
                return Err(invalid("URL raw de GitHub incompleta"));
            }
            let name = resto.last().unwrap_or(&"archivo").to_string();
            Ok(Source::SingleFile { url: s.to_string(), name })
        }
        _ => {
            let name = resto.last().filter(|n| !n.is_empty()).unwrap_or(&host).to_string();
            Ok(Source::SingleFile { url: s.to_string(), name })
        }
    }
}

fn is_text_path(path: &str) -> bool {
    match path.rsplit_once('.') {
        Some((_, ext)) => TEXT_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()),
        None => false,
    }
}

// -------------------------------------------------------------------
// GithubFetcher
// -------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct RepoInfo {
    default_branch: String,
    license: Option<RepoLicense>,
}

#[derive(Debug, Deserialize)]
struct RepoLicense {
    spdx_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TreeEntry {
    path: String,
    #[serde(rename = "type")]
    kind: String,
    size: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct TreeResponse {
    tree: Vec<TreeEntry>,
}

/// `SourceFetcher` sobre la API pública de GitHub. Con `GITHUB_TOKEN`
/// (opcional) sube el límite de peticiones y permite repos privados.
pub struct GithubFetcher {
    client: reqwest::Client,
    token: Option<String>,
    max_files: usize,
    max_file_bytes: u64,
}

impl GithubFetcher {
    /// Fetcher con topes por defecto y el token de `GITHUB_TOKEN` si
    /// está en el entorno.
    pub fn from_env() -> Self {
        GithubFetcher {
            client: reqwest::Client::new(),
            token: std::env::var("GITHUB_TOKEN").ok().filter(|t| !t.is_empty()),
            max_files: MAX_FILES,
            max_file_bytes: MAX_FILE_BYTES,
        }
    }

    fn request(&self, url: &str) -> reqwest::RequestBuilder {
        let req = self
            .client
            .get(url)
            .header("user-agent", "okf-mcp-skill-ingest")
            .header("x-github-api-version", "2022-11-28");
        match &self.token {
            Some(t) => req.header("authorization", format!("Bearer {t}")),
            None => req,
        }
    }

    async fn get_text(&self, url: &str) -> Result<String, IngestError> {
        let resp = self
            .request(url)
            .send()
            .await
            .map_err(|e| IngestError::Fetch(format!("{url}: {e}")))?;
        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| IngestError::Fetch(format!("{url}: {e}")))?;
        if !status.is_success() {
            let corto: String = body.chars().take(300).collect();
            return Err(IngestError::Fetch(format!("{url}: HTTP {status}: {corto}")));
        }
        Ok(body)
    }

    async fn fetch_async(&self, source: &str) -> Result<Vec<SourceFile>, IngestError> {
        match parse_source(source)? {
            Source::SingleFile { url, name } => {
                let content = self.get_text(&url).await?;
                Ok(vec![SourceFile { path: name, content }])
            }
            Source::GithubTree { owner, repo, reference, path } => {
                let reference = match reference {
                    Some(r) => r,
                    None => {
                        let info = self
                            .get_text(&format!("https://api.github.com/repos/{owner}/{repo}"))
                            .await?;
                        serde_json::from_str::<RepoInfo>(&info)
                            .map_err(|e| IngestError::Fetch(format!("metadata del repo: {e}")))?
                            .default_branch
                    }
                };
                let tree_raw = self
                    .get_text(&format!(
                        "https://api.github.com/repos/{owner}/{repo}/git/trees/{reference}?recursive=1"
                    ))
                    .await?;
                let tree: TreeResponse = serde_json::from_str(&tree_raw)
                    .map_err(|e| IngestError::Fetch(format!("árbol del repo: {e}")))?;

                let prefix = if path.is_empty() { String::new() } else { format!("{path}/") };
                let mut candidatos: Vec<&TreeEntry> = tree
                    .tree
                    .iter()
                    .filter(|e| e.kind == "blob")
                    .filter(|e| prefix.is_empty() || e.path.starts_with(&prefix))
                    .filter(|e| is_text_path(&e.path))
                    .filter(|e| e.size.unwrap_or(0) <= self.max_file_bytes)
                    .collect();
                // Prioridad: SKILL.md, luego el resto de markdown,
                // luego el resto — así el tope de archivos nunca deja
                // fuera la definición de una skill por culpa de sus
                // auxiliares.
                candidatos.sort_by_key(|e| {
                    let p = &e.path;
                    let rango = if p.ends_with("SKILL.md") {
                        0
                    } else if p.ends_with(".md") || p.ends_with(".mdx") || p.ends_with(".markdown")
                    {
                        1
                    } else {
                        2
                    };
                    (rango, p.clone())
                });
                candidatos.truncate(self.max_files);

                let mut files = Vec::with_capacity(candidatos.len());
                for entry in candidatos {
                    let raw_url = format!(
                        "https://raw.githubusercontent.com/{owner}/{repo}/{reference}/{}",
                        entry.path
                    );
                    let content = self.get_text(&raw_url).await?;
                    let relative =
                        entry.path.strip_prefix(&prefix).unwrap_or(&entry.path).to_string();
                    files.push(SourceFile { path: relative, content });
                }
                Ok(files)
            }
        }
    }

    /// SPDX id de la licencia del repo (campo `license.spdx_id` de la
    /// API de repos de GitHub — la misma llamada que ya hacemos para
    /// `default_branch`, GitHub ya la detecta por su cuenta). `None`
    /// para una fuente que no es un repo (`Source::SingleFile`), sin
    /// SPDX reconocido, o cuando GitHub reporta `"NOASSERTION"` (vio
    /// un archivo de licencia pero no pudo clasificarlo).
    async fn license_async(&self, source: &str) -> Result<Option<String>, IngestError> {
        let Source::GithubTree { owner, repo, .. } = parse_source(source)? else {
            return Ok(None);
        };
        let info = self.get_text(&format!("https://api.github.com/repos/{owner}/{repo}")).await?;
        let parsed: RepoInfo = serde_json::from_str(&info)
            .map_err(|e| IngestError::Fetch(format!("metadata del repo: {e}")))?;
        Ok(parsed.license.and_then(|l| l.spdx_id).filter(|id| id != "NOASSERTION"))
    }
}

impl SourceFetcher for GithubFetcher {
    fn fetch(&self, source: &str) -> Result<Vec<SourceFile>, IngestError> {
        block_on(self.fetch_async(source))
    }

    fn license_spdx_id(&self, source: &str) -> Result<Option<String>, IngestError> {
        block_on(self.license_async(source))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atajo_owner_repo() {
        assert_eq!(
            parse_source("udapy/rust-agentic-skills").unwrap(),
            Source::GithubTree {
                owner: "udapy".into(),
                repo: "rust-agentic-skills".into(),
                reference: None,
                path: String::new(),
            }
        );
    }

    #[test]
    fn url_de_repo_y_subcarpeta() {
        assert_eq!(
            parse_source("https://github.com/udapy/rust-agentic-skills/tree/main/skills").unwrap(),
            Source::GithubTree {
                owner: "udapy".into(),
                repo: "rust-agentic-skills".into(),
                reference: Some("main".into()),
                path: "skills".into(),
            }
        );
    }

    #[test]
    fn url_blob_se_vuelve_raw() {
        assert_eq!(
            parse_source("https://github.com/o/r/blob/main/skills/a/SKILL.md").unwrap(),
            Source::SingleFile {
                url: "https://raw.githubusercontent.com/o/r/main/skills/a/SKILL.md".into(),
                name: "SKILL.md".into(),
            }
        );
    }

    #[test]
    fn url_cualquiera_es_archivo_individual() {
        assert_eq!(
            parse_source("https://example.com/docs/guia.md").unwrap(),
            Source::SingleFile { url: "https://example.com/docs/guia.md".into(), name: "guia.md".into() }
        );
    }

    #[test]
    fn fuentes_invalidas_se_rechazan() {
        assert!(parse_source("solo-un-segmento").is_err());
        assert!(parse_source("a/b/c").is_err());
        assert!(parse_source("ftp://x/y").is_err());
        assert!(parse_source("https://github.com/solo-owner").is_err());
    }

    #[test]
    fn filtro_de_texto() {
        assert!(is_text_path("skills/a/SKILL.md"));
        assert!(is_text_path("x.tsx"));
        assert!(!is_text_path("logo.png"));
        assert!(!is_text_path("LICENSE"));
    }
}
