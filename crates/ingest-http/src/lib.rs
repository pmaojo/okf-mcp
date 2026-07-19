//! Adaptadores HTTP de los puertos de `ingest-core`.
//!
//! - [`GithubFetcher`] implementa `SourceFetcher`: acepta las mismas
//!   fuentes que `npx skills add` (el atajo `owner/repo` y URLs de
//!   repo, subcarpeta o archivo de GitHub) y descarga los archivos de
//!   texto vía la API de trees + `raw.githubusercontent.com`, con
//!   topes de cantidad y tamaño.
//! - [`GeminiSynthesizer`] implementa `Synthesizer` sobre el endpoint
//!   `generateContent` de Gemini (el mismo proveedor que ya genera
//!   los embeddings del proyecto, pero con cliente propio:
//!   `gemini-embeddings` guarda el invariante de los embeddings —
//!   mismo modelo y dimensionalidad en lectura y escritura — y la
//!   generación de texto no forma parte de ese invariante).
//! - [`ChatCompletionSynthesizer`] implementa `Synthesizer` sobre
//!   cualquier proveedor con endpoint de chat compatible con la API
//!   de OpenAI: Groq, OpenRouter, Cerebras, Mistral y la API de
//!   compatibilidad de Cohere. No hay adaptador dedicado por
//!   proveedor porque los cinco comparten el mismo contrato HTTP —
//!   solo cambian URL, modelo por defecto y clave.
//! - [`FallbackSynthesizer`] compone varios `Synthesizer` (p. ej.
//!   Gemini + Groq + OpenRouter) y prueba el siguiente si el anterior
//!   falla — así una cuota agotada (429) en un proveedor no bloquea
//!   `synthesize: true` mientras quede otro configurado.
//!
//! Los puertos de `ingest-core` son síncronos (el núcleo no conoce
//! `tokio`); aquí se puentea con el mismo `block_on` tolerante a
//! runtime que usa `supabase-store`.

#![forbid(unsafe_code)]

use ingest_core::{IngestError, SourceFetcher, SourceFile, Synthesizer};
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
}

impl SourceFetcher for GithubFetcher {
    fn fetch(&self, source: &str) -> Result<Vec<SourceFile>, IngestError> {
        block_on(self.fetch_async(source))
    }
}

// -------------------------------------------------------------------
// GeminiSynthesizer
// -------------------------------------------------------------------

/// Modelo de generación de texto para la síntesis de `skill_ingest`
/// (estable GA a julio de 2026). Sobreescribible con la variable de
/// entorno `GEMINI_SYNTHESIS_MODEL` — este proyecto ya vivió la
/// retirada de `text-embedding-004`: mejor poder cambiar de modelo
/// sin redesplegar código.
pub const SYNTHESIS_MODEL: &str = "gemini-3.5-flash";

fn synthesis_model() -> String {
    std::env::var("GEMINI_SYNTHESIS_MODEL")
        .ok()
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| SYNTHESIS_MODEL.to_string())
}

#[derive(serde::Serialize)]
struct GenPart {
    text: String,
}

#[derive(serde::Serialize)]
struct GenContent {
    parts: Vec<GenPart>,
}

#[derive(serde::Serialize)]
struct GenRequest {
    contents: Vec<GenContent>,
}

#[derive(Deserialize)]
struct GenRespPart {
    text: Option<String>,
}

#[derive(Deserialize)]
struct GenRespContent {
    parts: Option<Vec<GenRespPart>>,
}

#[derive(Deserialize)]
struct GenCandidate {
    content: Option<GenRespContent>,
}

#[derive(Deserialize)]
struct GenResponse {
    candidates: Option<Vec<GenCandidate>>,
}

/// `Synthesizer` sobre el endpoint `generateContent` de Gemini
/// (modelo [`SYNTHESIS_MODEL`]).
pub struct GeminiSynthesizer {
    client: reqwest::Client,
    api_key: String,
}

impl GeminiSynthesizer {
    /// Sintetizador con la clave de API dada.
    pub fn new(api_key: String) -> Self {
        GeminiSynthesizer { client: reqwest::Client::new(), api_key }
    }

    async fn generate(&self, prompt: &str) -> Result<String, IngestError> {
        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
            synthesis_model(),
            self.api_key
        );
        let body = GenRequest {
            contents: vec![GenContent { parts: vec![GenPart { text: prompt.to_string() }] }],
        };
        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| IngestError::Synthesis(format!("gemini: {e}")))?;
        let status = resp.status();
        let raw = resp.text().await.map_err(|e| IngestError::Synthesis(format!("gemini: {e}")))?;
        if !status.is_success() {
            let corto: String = raw.chars().take(300).collect();
            return Err(IngestError::Synthesis(format!("gemini: HTTP {status}: {corto}")));
        }
        let parsed: GenResponse = serde_json::from_str(&raw)
            .map_err(|e| IngestError::Synthesis(format!("gemini: respuesta no deserializable: {e}")))?;
        let text: String = parsed
            .candidates
            .unwrap_or_default()
            .into_iter()
            .filter_map(|c| c.content)
            .filter_map(|c| c.parts)
            .flatten()
            .filter_map(|p| p.text)
            .collect();
        if text.is_empty() {
            return Err(IngestError::Synthesis(
                "gemini: la respuesta de generateContent no trae texto".to_string(),
            ));
        }
        Ok(text)
    }
}

impl Synthesizer for GeminiSynthesizer {
    fn synthesize(&self, prompt: &str) -> Result<String, IngestError> {
        block_on(self.generate(prompt))
    }
}

// -------------------------------------------------------------------
// ChatCompletionSynthesizer: proveedores OpenAI-compatibles
// -------------------------------------------------------------------

#[derive(serde::Serialize)]
struct ChatMessage {
    role: &'static str,
    content: String,
}

#[derive(serde::Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
}

#[derive(Deserialize)]
struct ChatChoiceMessage {
    content: Option<String>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: Option<ChatChoiceMessage>,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Option<Vec<ChatChoice>>,
}

/// `Synthesizer` sobre cualquier proveedor con endpoint de chat
/// compatible con la API de `/chat/completions` de OpenAI. Un solo
/// cliente cubre Groq, OpenRouter, Cerebras, Mistral y la API de
/// compatibilidad de Cohere — solo cambian endpoint, modelo y clave.
pub struct ChatCompletionSynthesizer {
    client: reqwest::Client,
    provider: &'static str,
    endpoint: String,
    api_key: String,
    model: String,
}

impl ChatCompletionSynthesizer {
    fn new(
        provider: &'static str,
        endpoint: &str,
        api_key: String,
        default_model: &str,
        env_model_var: &str,
    ) -> Self {
        let model = std::env::var(env_model_var)
            .ok()
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| default_model.to_string());
        ChatCompletionSynthesizer {
            client: reqwest::Client::new(),
            provider,
            endpoint: endpoint.to_string(),
            api_key,
            model,
        }
    }

    /// Groq (`api.groq.com`): free tier alto, muy rápido. Modelo
    /// sobreescribible con `GROQ_SYNTHESIS_MODEL`.
    pub fn groq(api_key: String) -> Self {
        Self::new(
            "groq",
            "https://api.groq.com/openai/v1/chat/completions",
            api_key,
            "llama-3.3-70b-versatile",
            "GROQ_SYNTHESIS_MODEL",
        )
    }

    /// OpenRouter (`openrouter.ai`): agrega varios modelos con
    /// etiqueta gratis y ya hace fallback interno entre proveedores.
    /// Modelo sobreescribible con `OPENROUTER_SYNTHESIS_MODEL`.
    pub fn openrouter(api_key: String) -> Self {
        Self::new(
            "openrouter",
            "https://openrouter.ai/api/v1/chat/completions",
            api_key,
            "meta-llama/llama-3.3-70b-instruct:free",
            "OPENROUTER_SYNTHESIS_MODEL",
        )
    }

    /// Cerebras (`api.cerebras.ai`): velocidad similar a Groq, con
    /// free tier. Modelo sobreescribible con `CEREBRAS_SYNTHESIS_MODEL`.
    pub fn cerebras(api_key: String) -> Self {
        Self::new(
            "cerebras",
            "https://api.cerebras.ai/v1/chat/completions",
            api_key,
            "llama-3.3-70b",
            "CEREBRAS_SYNTHESIS_MODEL",
        )
    }

    /// Mistral (`api.mistral.ai`): opción secundaria, límites de free
    /// tier más bajos. Modelo sobreescribible con
    /// `MISTRAL_SYNTHESIS_MODEL`.
    pub fn mistral(api_key: String) -> Self {
        Self::new(
            "mistral",
            "https://api.mistral.ai/v1/chat/completions",
            api_key,
            "mistral-small-latest",
            "MISTRAL_SYNTHESIS_MODEL",
        )
    }

    /// Cohere, vía su API de compatibilidad con OpenAI: opción
    /// secundaria, límites de free tier más bajos. Modelo
    /// sobreescribible con `COHERE_SYNTHESIS_MODEL`.
    pub fn cohere(api_key: String) -> Self {
        Self::new(
            "cohere",
            "https://api.cohere.com/compatibility/v1/chat/completions",
            api_key,
            "command-r7b-12-2024",
            "COHERE_SYNTHESIS_MODEL",
        )
    }

    async fn generate(&self, prompt: &str) -> Result<String, IngestError> {
        let body = ChatRequest {
            model: self.model.clone(),
            messages: vec![ChatMessage { role: "user", content: prompt.to_string() }],
        };
        let err = |e: String| IngestError::Synthesis(format!("{}: {e}", self.provider));
        let resp = self
            .client
            .post(&self.endpoint)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| err(e.to_string()))?;
        let status = resp.status();
        let raw = resp.text().await.map_err(|e| err(e.to_string()))?;
        if !status.is_success() {
            let corto: String = raw.chars().take(300).collect();
            return Err(err(format!("HTTP {status}: {corto}")));
        }
        let parsed: ChatResponse =
            serde_json::from_str(&raw).map_err(|e| err(format!("respuesta no deserializable: {e}")))?;
        let text = parsed
            .choices
            .unwrap_or_default()
            .into_iter()
            .find_map(|c| c.message.and_then(|m| m.content))
            .unwrap_or_default();
        if text.is_empty() {
            return Err(err("la respuesta no trae texto".to_string()));
        }
        Ok(text)
    }
}

impl Synthesizer for ChatCompletionSynthesizer {
    fn synthesize(&self, prompt: &str) -> Result<String, IngestError> {
        block_on(self.generate(prompt))
    }
}

// -------------------------------------------------------------------
// FallbackSynthesizer: encadena proveedores
// -------------------------------------------------------------------

/// `Synthesizer` que encadena varios proveedores: prueba cada uno en
/// el orden dado y pasa al siguiente si el anterior falla (el caso que
/// motivó esto: un 429 por cuota agotada en Gemini bloqueaba
/// `synthesize: true` hasta que se restablecía la cuota). Si todos
/// fallan, el error agrega el motivo de cada proveedor.
pub struct FallbackSynthesizer {
    providers: Vec<Box<dyn Synthesizer>>,
}

impl FallbackSynthesizer {
    /// Encadena `providers` en el orden dado: el primero es el
    /// preferido, el resto son respaldo automático. Construir con una
    /// lista vacía es un error del llamador: siempre fallará con "no
    /// hay proveedores configurados".
    pub fn new(providers: Vec<Box<dyn Synthesizer>>) -> Self {
        FallbackSynthesizer { providers }
    }
}

impl Synthesizer for FallbackSynthesizer {
    fn synthesize(&self, prompt: &str) -> Result<String, IngestError> {
        let mut errors = Vec::new();
        for provider in &self.providers {
            match provider.synthesize(prompt) {
                Ok(text) => return Ok(text),
                Err(IngestError::Synthesis(msg)) => errors.push(msg),
                Err(other) => errors.push(other.to_string()),
            }
        }
        if errors.is_empty() {
            return Err(IngestError::Synthesis(
                "no hay proveedores de síntesis configurados".to_string(),
            ));
        }
        Err(IngestError::Synthesis(format!(
            "todos los proveedores de síntesis fallaron: {}",
            errors.join(" | ")
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    /// `Synthesizer` de prueba: falla o acierta según se le pida, y
    /// cuenta cuántas veces se le llamó (en un `Rc` compartido con el
    /// test) para comprobar que el fallback no invoca proveedores de
    /// más tras un éxito.
    struct FakeProvider {
        result: Result<&'static str, &'static str>,
        calls: Rc<Cell<u32>>,
    }

    impl FakeProvider {
        fn ok(text: &'static str) -> Self {
            FakeProvider { result: Ok(text), calls: Rc::new(Cell::new(0)) }
        }
        fn fail(msg: &'static str) -> Self {
            FakeProvider { result: Err(msg), calls: Rc::new(Cell::new(0)) }
        }
        fn calls_handle(&self) -> Rc<Cell<u32>> {
            self.calls.clone()
        }
    }

    impl Synthesizer for FakeProvider {
        fn synthesize(&self, _prompt: &str) -> Result<String, IngestError> {
            self.calls.set(self.calls.get() + 1);
            self.result.map(str::to_string).map_err(|m| IngestError::Synthesis(m.to_string()))
        }
    }

    #[test]
    fn usa_el_primer_proveedor_que_funciona() {
        let fallback =
            FallbackSynthesizer::new(vec![Box::new(FakeProvider::ok("primero"))]);
        assert_eq!(fallback.synthesize("x").unwrap(), "primero");
    }

    #[test]
    fn cae_al_siguiente_proveedor_si_el_primero_falla() {
        let fallback = FallbackSynthesizer::new(vec![
            Box::new(FakeProvider::fail("gemini: HTTP 429: cuota agotada")),
            Box::new(FakeProvider::ok("respaldo")),
        ]);
        assert_eq!(fallback.synthesize("x").unwrap(), "respaldo");
    }

    #[test]
    fn no_llama_al_segundo_si_el_primero_acierta() {
        let segundo = FakeProvider::ok("no debería usarse");
        let calls_segundo = segundo.calls_handle();
        let fallback = FallbackSynthesizer::new(vec![
            Box::new(FakeProvider::ok("primero")),
            Box::new(segundo),
        ]);
        assert_eq!(fallback.synthesize("x").unwrap(), "primero");
        assert_eq!(calls_segundo.get(), 0);
    }

    #[test]
    fn agrega_los_errores_si_todos_fallan() {
        let fallback = FallbackSynthesizer::new(vec![
            Box::new(FakeProvider::fail("gemini: HTTP 429")),
            Box::new(FakeProvider::fail("groq: HTTP 429")),
        ]);
        let err = fallback.synthesize("x").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("gemini: HTTP 429"), "mensaje: {msg}");
        assert!(msg.contains("groq: HTTP 429"), "mensaje: {msg}");
    }

    #[test]
    fn sin_proveedores_falla_claro() {
        let fallback = FallbackSynthesizer::new(vec![]);
        let err = fallback.synthesize("x").unwrap_err();
        assert!(err.to_string().contains("no hay proveedores"));
    }

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
