//! PROTOTIPO: GitHub como fuente de verdad de la memoria.
//!
//! `GithubStore` implementa [`MemoryRepository`] + [`StoreMaintenance`]
//! sobre la API REST de GitHub:
//!
//! - **Documentos** = archivos `{base_path}/{concept_id}.md` en una
//!   rama. Los bytes del archivo son la verdad.
//! - **CAS** = el parámetro `sha` de la API de contents: un `PUT` con
//!   el blob sha leído falla si el archivo cambió debajo — la misma
//!   garantía de concurrencia optimista que `expected_hash`, cerrada
//!   por el propio git.
//! - **Historia** = commits. Cada revisión viaja como una línea
//!   `Memory-Rev:` estructurada en el mensaje de commit (seq global,
//!   op, concepto, versión, hashes SHA-256, actor, motivo), de modo
//!   que `memory_history` se reconstruye sin releer blobs antiguos.
//! - **Lote atómico** = API de git data: blobs → tree → commit →
//!   actualización de ref sin force. Si la rama avanzó, el update
//!   falla y el lote entero se descarta: todo-o-nada real.
//!
//! Estrategia de lectura: las operaciones de grafo del contrato
//! (search con subcadena, backlinks, validate, stats) necesitan el
//! grafo completo, así que el adaptador materializa un *snapshot* en
//! memoria (árbol + contenidos + trailers de commits) cacheado por el
//! sha de HEAD: si la rama no se movió, ninguna operación de lectura
//! vuelve a la red. Tras una escritura propia el snapshot se
//! actualiza en sitio. Esto es honesto con el coste real: sin un
//! índice derivado, buscar en GitHub ES leerse el repo.
//!
//! Limitaciones conocidas del prototipo (hallazgos, no descuidos):
//! - los saltos de línea del `reason` se aplanan a espacios (viven en
//!   una línea del mensaje de commit);
//! - un archivo editado a mano en GitHub sin trailer `Memory-Rev:`
//!   aparece como versión 1 sin historia — la reconciliación de
//!   ediciones humanas necesitaría sintetizar revisiones desde los
//!   commits ajenos;
//! - la búsqueda es textual: sin índice semántico, `embed_pending`
//!   devuelve vacío igual que `InMemoryStore`.

#![forbid(unsafe_code)]

use base64::Engine as _;
use conflict_core::{decide, CommitDecision, Conflict};
use memory_model::{Budget, ConceptId, ContentId, Principal, Revision};
use okf_core::Link;
use serde::Deserialize;
use std::sync::Mutex;
use std::collections::BTreeMap;
use std::convert::Infallible;
use std::sync::Arc;
use store_core::{
    matches_prefix, Backlink, BulkItem, BulkOutcome, CommitOutcome, CommitRequest, DeleteOutcome,
    DocumentView, EmbedOutcome, GraphStats, LinkHealth, MemoryRepository, SearchHit, SearchQuery,
    StoreError, StoreMaintenance, StoreStatus, ValidationReport,
};

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

fn b64(s: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(s.as_bytes())
}

fn from_b64(s: &str) -> Result<String, StoreError> {
    let compact: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(compact)
        .map_err(|e| StoreError::Backend(format!("base64 inválido de la API: {e}")))?;
    String::from_utf8(bytes).map_err(|e| StoreError::Backend(format!("contenido no UTF-8: {e}")))
}

// -------------------------------------------------------------------
// El trailer Memory-Rev: una revisión por línea de mensaje de commit
// -------------------------------------------------------------------

const REV_TRAILER: &str = "Memory-Rev: ";

#[derive(Debug, Clone, PartialEq, Eq)]
enum RevOp {
    Commit,
    Delete,
}

fn encode_rev(op: &RevOp, rev: &Revision, version: u64) -> String {
    let op = match op {
        RevOp::Commit => "commit",
        RevOp::Delete => "delete",
    };
    let base = rev.base.map(|b| b.to_hex()).unwrap_or_else(|| "-".to_string());
    // El reason vive en una línea: los saltos de línea se aplanan.
    let reason = rev.reason.replace(['\n', '\r'], " ");
    format!(
        "{REV_TRAILER}{}|{}|{}|{}|{}|{}|{}|{}|{}",
        rev.seq,
        op,
        rev.concept_id,
        version,
        base,
        rev.result.to_hex(),
        rev.actor.subject,
        rev.actor.client_id,
        reason
    )
}

fn decode_rev(line: &str) -> Option<(RevOp, Revision, u64)> {
    let raw = line.strip_prefix(REV_TRAILER)?;
    let mut parts = raw.splitn(9, '|');
    let seq: u64 = parts.next()?.parse().ok()?;
    let op = match parts.next()? {
        "commit" => RevOp::Commit,
        "delete" => RevOp::Delete,
        _ => return None,
    };
    let concept_id = ConceptId::parse(parts.next()?).ok()?;
    let version: u64 = parts.next()?.parse().ok()?;
    let base = match parts.next()? {
        "-" => None,
        hex => Some(ContentId::from_hex(hex)?),
    };
    let result = ContentId::from_hex(parts.next()?)?;
    let subject = parts.next()?.to_string();
    let client_id = parts.next()?.to_string();
    let reason = parts.next()?.to_string();
    Some((
        op,
        Revision {
            seq,
            concept_id,
            base,
            result,
            actor: Principal { subject, client_id },
            reason,
        },
        version,
    ))
}

// -------------------------------------------------------------------
// Snapshot: el estado del repo materializado en memoria
// -------------------------------------------------------------------

#[derive(Debug, Clone)]
struct LiveDoc {
    raw: Arc<str>,
    content_id: ContentId,
    version: u64,
    doc_type: String,
    title: Option<String>,
    tags: Vec<String>,
    links: Vec<Link>,
    /// Blob sha de git del archivo: el token CAS para PUT/DELETE.
    file_sha: String,
}

#[derive(Debug, Clone)]
struct GoneDoc {
    version: u64,
}

#[derive(Debug, Clone)]
struct Snapshot {
    head_sha: String,
    live: BTreeMap<ConceptId, LiveDoc>,
    gone: BTreeMap<ConceptId, GoneDoc>,
    /// Todas las revisiones, orden ascendente por `seq`.
    revisions: Vec<Revision>,
    next_seq: u64,
}

impl Snapshot {
    fn hit(&self, id: &ConceptId, doc: &LiveDoc) -> SearchHit {
        SearchHit {
            concept_id: id.clone(),
            content_id: doc.content_id,
            doc_type: doc.doc_type.clone(),
            title: doc.title.clone(),
            tags: doc.tags.clone(),
        }
    }
}

// -------------------------------------------------------------------
// Tipos de la API de GitHub
// -------------------------------------------------------------------

#[derive(Deserialize)]
struct RefResponse {
    object: RefObject,
}

#[derive(Deserialize)]
struct RefObject {
    sha: String,
}

#[derive(Deserialize)]
struct TreeResponse {
    tree: Vec<TreeEntry>,
}

#[derive(Deserialize)]
struct TreeEntry {
    path: String,
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Deserialize)]
struct ContentsResponse {
    content: String,
    sha: String,
}

#[derive(Deserialize)]
struct CommitListEntry {
    commit: CommitData,
}

#[derive(Deserialize)]
struct CommitData {
    message: String,
}

#[derive(Deserialize)]
struct WriteResponse {
    content: Option<WriteContent>,
    commit: WriteCommit,
}

#[derive(Deserialize)]
struct WriteContent {
    sha: String,
}

#[derive(Deserialize)]
struct WriteCommit {
    sha: String,
}

#[derive(Deserialize)]
struct GitCommitResponse {
    tree: Option<TreeRef>,
}

#[derive(Deserialize)]
struct TreeRef {
    sha: String,
}

#[derive(Deserialize)]
struct ShaOnly {
    sha: String,
}

// -------------------------------------------------------------------
// GithubStore
// -------------------------------------------------------------------

/// [`MemoryRepository`] sobre un repositorio de GitHub.
pub struct GithubStore {
    client: reqwest::Client,
    /// Base de la API (`https://api.github.com` en producción; en los
    /// tests de contrato, el servidor falso local).
    api_base: String,
    owner: String,
    repo: String,
    branch: String,
    /// Prefijo dentro del repo bajo el que viven los documentos
    /// (p. ej. `memoria`). Vacío = raíz del repo.
    base_path: String,
    token: Option<String>,
    cache: Mutex<Option<Snapshot>>,
}

impl GithubStore {
    /// Crea el adaptador. La rama debe existir (con al menos un
    /// commit); el prototipo no la crea.
    pub fn new(
        api_base: impl Into<String>,
        owner: impl Into<String>,
        repo: impl Into<String>,
        branch: impl Into<String>,
        base_path: impl Into<String>,
        token: Option<String>,
    ) -> Self {
        GithubStore {
            client: reqwest::Client::new(),
            api_base: api_base.into(),
            owner: owner.into(),
            repo: repo.into(),
            branch: branch.into(),
            base_path: base_path.into().trim_matches('/').to_string(),
            token,
            cache: Mutex::new(None),
        }
    }

    /// Crea el adaptador leyendo las variables de entorno:
    /// - `GITHUB_REPO` (obligatorio, formato `owner/repo`)
    /// - `GITHUB_TOKEN` (obligatorio)
    /// - `GITHUB_BRANCH` (defecto: `main`)
    /// - `GITHUB_PATH` (defecto: `memoria`)
    pub fn from_env() -> Result<Self, StoreError> {
        let full_repo = std::env::var("GITHUB_REPO")
            .map_err(|_| StoreError::Backend("falta GITHUB_REPO (formato owner/repo)".into()))?;
        let (owner, repo) = full_repo.split_once('/').ok_or_else(|| {
            StoreError::Backend(format!(
                "GITHUB_REPO debe tener formato owner/repo, recibido: {full_repo}"
            ))
        })?;
        let token = std::env::var("GITHUB_TOKEN")
            .map_err(|_| StoreError::Backend("falta GITHUB_TOKEN".into()))?;
        let branch = std::env::var("GITHUB_BRANCH").unwrap_or_else(|_| "main".into());
        let base_path = std::env::var("GITHUB_PATH").unwrap_or_else(|_| "memoria".into());
        Ok(Self::new(
            "https://api.github.com",
            owner, repo, branch, base_path,
            Some(token),
        ))
    }

    fn url(&self, rest: &str) -> String {
        format!("{}/repos/{}/{}/{rest}", self.api_base, self.owner, self.repo)
    }

    fn doc_path(&self, id: &ConceptId) -> String {
        if self.base_path.is_empty() {
            format!("{id}.md")
        } else {
            format!("{}/{id}.md", self.base_path)
        }
    }

    fn path_to_concept(&self, path: &str) -> Option<ConceptId> {
        let rel = if self.base_path.is_empty() {
            path
        } else {
            path.strip_prefix(&self.base_path)?.strip_prefix('/')?
        };
        ConceptId::parse(rel.strip_suffix(".md")?).ok()
    }

    fn request(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let req = req
            .header("user-agent", "okf-mcp-github-store")
            .header("x-github-api-version", "2022-11-28");
        match &self.token {
            Some(t) => req.header("authorization", format!("Bearer {t}")),
            None => req,
        }
    }

    async fn get_json<T: serde::de::DeserializeOwned>(&self, url: &str) -> Result<T, StoreError> {
        let resp = self
            .request(self.client.get(url))
            .send()
            .await
            .map_err(|e| StoreError::Backend(format!("GET {url}: {e}")))?;
        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| StoreError::Backend(format!("GET {url}: {e}")))?;
        if !status.is_success() {
            let corto: String = body.chars().take(200).collect();
            return Err(StoreError::Backend(format!("GET {url}: HTTP {status}: {corto}")));
        }
        serde_json::from_str(&body)
            .map_err(|e| StoreError::Backend(format!("GET {url}: respuesta no deserializable: {e}")))
    }

    // ---- snapshot -----------------------------------------------------

    /// Sha de HEAD de la rama, una llamada barata que decide si el
    /// snapshot cacheado sigue siendo válido.
    async fn head_sha(&self) -> Result<String, StoreError> {
        let r: RefResponse =
            self.get_json(&self.url(&format!("git/refs/heads/{}", self.branch))).await?;
        Ok(r.object.sha)
    }

    async fn build_snapshot(&self, head_sha: String) -> Result<Snapshot, StoreError> {
        // 1. Árbol completo: qué documentos están vivos.
        let tree: TreeResponse = self
            .get_json(&self.url(&format!("git/trees/{head_sha}?recursive=1")))
            .await?;

        // 2. Contenido de cada documento vivo.
        let mut live: BTreeMap<ConceptId, LiveDoc> = BTreeMap::new();
        let budget = Budget::default();
        for entry in tree.tree.iter().filter(|e| e.kind == "blob") {
            let Some(id) = self.path_to_concept(&entry.path) else { continue };
            let contents: ContentsResponse = self
                .get_json(&self.url(&format!(
                    "contents/{}?ref={head_sha}",
                    self.doc_path(&id)
                )))
                .await?;
            let raw = from_b64(&contents.content)?;
            let content_id = ContentId(hash_core::sha256(raw.as_bytes()));
            // Un archivo que no parsea como OKF no entra al grafo: el
            // documento se ignora (edición humana rota, no un 500).
            let Ok(doc) = okf_core::parse_document(&raw, &budget) else { continue };
            live.insert(
                id,
                LiveDoc {
                    raw: Arc::from(raw.as_str()),
                    content_id,
                    version: 1, // se refina con las revisiones
                    doc_type: doc.doc_type,
                    title: doc.title,
                    tags: doc.tags,
                    links: doc.links,
                    file_sha: contents.sha,
                },
            );
        }

        // 3. La historia: trailers Memory-Rev de todos los commits.
        let mut revisions: Vec<Revision> = Vec::new();
        let mut ops: BTreeMap<u64, (RevOp, ConceptId, u64)> = BTreeMap::new();
        let mut page = 1usize;
        loop {
            let commits: Vec<CommitListEntry> = self
                .get_json(&self.url(&format!(
                    "commits?sha={head_sha}&per_page=100&page={page}"
                )))
                .await?;
            let n = commits.len();
            for c in commits {
                for line in c.commit.message.lines() {
                    if let Some((op, rev, version)) = decode_rev(line) {
                        ops.insert(rev.seq, (op, rev.concept_id.clone(), version));
                        revisions.push(rev);
                    }
                }
            }
            if n < 100 {
                break;
            }
            page += 1;
        }
        revisions.sort_by_key(|r| r.seq);
        let next_seq = revisions.last().map(|r| r.seq + 1).unwrap_or(1);

        // 4. Versiones y borrados, replegando la historia en orden.
        let mut versions: BTreeMap<ConceptId, u64> = BTreeMap::new();
        let mut deleted: BTreeMap<ConceptId, ()> = BTreeMap::new();
        for rev in &revisions {
            let (op, _, version) = &ops[&rev.seq];
            match op {
                RevOp::Commit => {
                    versions.insert(rev.concept_id.clone(), *version);
                    deleted.remove(&rev.concept_id);
                }
                RevOp::Delete => {
                    deleted.insert(rev.concept_id.clone(), ());
                }
            }
        }
        for (id, doc) in live.iter_mut() {
            if let Some(v) = versions.get(id) {
                doc.version = *v;
            }
        }
        let mut gone: BTreeMap<ConceptId, GoneDoc> = BTreeMap::new();
        for id in deleted.into_keys() {
            if live.contains_key(&id) {
                continue; // el archivo vivo manda (recreado o editado a mano)
            }
            let version = versions.get(&id).copied().unwrap_or(0);
            gone.insert(id, GoneDoc { version });
        }

        Ok(Snapshot { head_sha, live, gone, revisions, next_seq })
    }

    /// El snapshot vigente: reusa la caché si HEAD no se movió.
    fn snapshot(&self) -> Result<Snapshot, StoreError> {
        block_on(async {
            let head = self.head_sha().await?;
            if let Some(snap) = self.cache.lock().unwrap().as_ref() {
                if snap.head_sha == head {
                    return Ok(snap.clone());
                }
            }
            let snap = self.build_snapshot(head).await?;
            *self.cache.lock().unwrap() = Some(snap.clone());
            Ok(snap)
        })
    }

    fn store_cache(&self, snap: Snapshot) {
        *self.cache.lock().unwrap() = Some(snap);
    }

    // ---- escrituras ---------------------------------------------------

    async fn put_file(
        &self,
        path: &str,
        content: &str,
        message: &str,
        file_sha: Option<&str>,
    ) -> Result<WriteResponse, StoreError> {
        let mut body = serde_json::json!({
            "message": message,
            "content": b64(content),
            "branch": self.branch,
        });
        if let Some(sha) = file_sha {
            body["sha"] = serde_json::Value::String(sha.to_string());
        }
        let url = self.url(&format!("contents/{path}"));
        let resp = self
            .request(self.client.put(&url))
            .json(&body)
            .send()
            .await
            .map_err(|e| StoreError::Backend(format!("PUT {url}: {e}")))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| StoreError::Backend(format!("PUT {url}: {e}")))?;
        if !status.is_success() {
            // 409: la rama o el archivo avanzaron debajo — el CAS de
            // git rechazó la escritura. Invalidamos la caché para que
            // el siguiente intento vea la cabeza real.
            *self.cache.lock().unwrap() = None;
            return Err(StoreError::Backend(format!("PUT {url}: HTTP {status}: {text}")));
        }
        serde_json::from_str(&text)
            .map_err(|e| StoreError::Backend(format!("PUT {url}: respuesta no deserializable: {e}")))
    }

    async fn delete_file(
        &self,
        path: &str,
        message: &str,
        file_sha: &str,
    ) -> Result<WriteResponse, StoreError> {
        let body = serde_json::json!({
            "message": message,
            "sha": file_sha,
            "branch": self.branch,
        });
        let url = self.url(&format!("contents/{path}"));
        let resp = self
            .request(self.client.delete(&url))
            .json(&body)
            .send()
            .await
            .map_err(|e| StoreError::Backend(format!("DELETE {url}: {e}")))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| StoreError::Backend(format!("DELETE {url}: {e}")))?;
        if !status.is_success() {
            *self.cache.lock().unwrap() = None;
            return Err(StoreError::Backend(format!("DELETE {url}: HTTP {status}: {text}")));
        }
        serde_json::from_str(&text).map_err(|e| {
            StoreError::Backend(format!("DELETE {url}: respuesta no deserializable: {e}"))
        })
    }

    fn view_of(&self, id: &ConceptId, doc: &LiveDoc) -> DocumentView {
        DocumentView {
            concept_id: id.clone(),
            content_id: doc.content_id,
            version: doc.version,
            raw: doc.raw.clone(),
            doc_type: doc.doc_type.clone(),
            title: doc.title.clone(),
            tags: doc.tags.clone(),
            links: doc.links.clone(),
        }
    }

    /// Decide un commit contra el snapshot SIN tocar la red: la misma
    /// lógica pura que `InMemoryStore`, compartida entre el commit
    /// individual y la simulación del lote atómico.
    fn decide_commit(
        snap: &Snapshot,
        request: &CommitRequest,
        actor: &Principal,
        budget: &Budget,
    ) -> Result<DecidedCommit, StoreError> {
        let doc = okf_core::parse_document(&request.markdown, budget)?;
        let incoming = ContentId(hash_core::sha256(request.markdown.as_bytes()));
        let live = snap.live.get(&request.concept_id);
        let decision = decide(live.map(|d| d.content_id), request.expected, incoming);
        match decision {
            CommitDecision::Conflict(c) => Err(StoreError::Conflict(c)),
            CommitDecision::NoChange => {
                let live = live.expect("NoChange implica cabeza viva");
                Ok(DecidedCommit::NoChange {
                    outcome: CommitOutcome {
                        revision: None,
                        content_id: live.content_id,
                        version: live.version,
                        created: false,
                        no_change: true,
                    },
                })
            }
            CommitDecision::Create | CommitDecision::Update => {
                let created = matches!(decision, CommitDecision::Create);
                let base = live.map(|d| d.content_id);
                let prev_version = live
                    .map(|d| d.version)
                    .or_else(|| snap.gone.get(&request.concept_id).map(|g| g.version))
                    .unwrap_or(0);
                let version = prev_version + 1;
                let revision = Revision {
                    seq: snap.next_seq,
                    concept_id: request.concept_id.clone(),
                    base,
                    result: incoming,
                    actor: actor.clone(),
                    reason: request.reason.clone(),
                };
                Ok(DecidedCommit::Write {
                    created,
                    version,
                    revision,
                    doc_type: doc.doc_type,
                    title: doc.title,
                    tags: doc.tags,
                    links: doc.links,
                })
            }
        }
    }

    /// Aplica al snapshot en memoria el efecto de una escritura ya
    /// confirmada por la API.
    #[allow(clippy::too_many_arguments)]
    fn apply_write(
        snap: &mut Snapshot,
        id: &ConceptId,
        markdown: &str,
        decided: &DecidedCommit,
        file_sha: String,
    ) {
        let DecidedCommit::Write { version, revision, doc_type, title, tags, links, .. } = decided
        else {
            return;
        };
        snap.gone.remove(id);
        snap.live.insert(
            id.clone(),
            LiveDoc {
                raw: Arc::from(markdown),
                content_id: revision.result,
                version: *version,
                doc_type: doc_type.clone(),
                title: title.clone(),
                tags: tags.clone(),
                links: links.clone(),
                file_sha,
            },
        );
        snap.revisions.push(revision.clone());
        snap.next_seq = revision.seq + 1;
    }
}

/// Resultado de decidir un commit contra el snapshot.
enum DecidedCommit {
    NoChange {
        outcome: CommitOutcome,
    },
    Write {
        created: bool,
        version: u64,
        revision: Revision,
        doc_type: String,
        title: Option<String>,
        tags: Vec<String>,
        links: Vec<Link>,
    },
}

impl MemoryRepository for GithubStore {
    fn get(&self, id: &ConceptId) -> Result<Option<DocumentView>, StoreError> {
        let snap = self.snapshot()?;
        Ok(snap.live.get(id).map(|doc| self.view_of(id, doc)))
    }

    fn search(&self, query: &SearchQuery, budget: &Budget) -> Result<Vec<SearchHit>, StoreError> {
        let snap = self.snapshot()?;
        let limit = query
            .limit
            .unwrap_or(budget.max_search_results)
            .min(budget.max_search_results);
        let needle = query.text.as_ref().map(|t| t.to_lowercase());
        let mut hits = Vec::new();
        for (id, doc) in &snap.live {
            if hits.len() >= limit {
                break;
            }
            if let Some(prefix) = &query.path_prefix {
                if !matches_prefix(id.as_str(), prefix) {
                    continue;
                }
            }
            if let Some(t) = &query.doc_type {
                if doc.doc_type != *t {
                    continue;
                }
            }
            if let Some(tag) = &query.tag {
                if !doc.tags.iter().any(|x| x == tag) {
                    continue;
                }
            }
            if let Some(needle) = &needle {
                let in_id = id.as_str().contains(needle.as_str());
                let in_title = doc
                    .title
                    .as_deref()
                    .is_some_and(|t| t.to_lowercase().contains(needle.as_str()));
                let in_tags = doc.tags.iter().any(|t| t.to_lowercase().contains(needle.as_str()));
                let in_body = doc.raw.to_lowercase().contains(needle.as_str());
                if !(in_id || in_title || in_tags || in_body) {
                    continue;
                }
            }
            hits.push(snap.hit(id, doc));
        }
        Ok(hits)
    }

    fn commit(
        &mut self,
        request: CommitRequest,
        actor: &Principal,
        budget: &Budget,
    ) -> Result<CommitOutcome, StoreError> {
        let mut snap = self.snapshot()?;
        let decided = Self::decide_commit(&snap, &request, actor, budget)?;
        match &decided {
            DecidedCommit::NoChange { outcome } => Ok(outcome.clone()),
            DecidedCommit::Write { created, version, revision, .. } => {
                let path = self.doc_path(&request.concept_id);
                let message = format!(
                    "{}\n\n{}",
                    revision.reason.replace(['\n', '\r'], " "),
                    encode_rev(&RevOp::Commit, revision, *version)
                );
                let file_sha = snap.live.get(&request.concept_id).map(|d| d.file_sha.clone());
                let resp = block_on(self.put_file(
                    &path,
                    &request.markdown,
                    &message,
                    file_sha.as_deref(),
                ))?;
                let outcome = CommitOutcome {
                    revision: Some(revision.clone()),
                    content_id: revision.result,
                    version: *version,
                    created: *created,
                    no_change: false,
                };
                let new_file_sha = resp.content.map(|c| c.sha).unwrap_or_default();
                Self::apply_write(&mut snap, &request.concept_id, &request.markdown, &decided, new_file_sha);
                snap.head_sha = resp.commit.sha;
                self.store_cache(snap);
                Ok(outcome)
            }
        }
    }

    fn history(
        &self,
        id: &ConceptId,
        limit: usize,
        before_seq: Option<u64>,
    ) -> Result<Vec<Revision>, StoreError> {
        let snap = self.snapshot()?;
        let conocido = snap.live.contains_key(id)
            || snap.gone.contains_key(id)
            || snap.revisions.iter().any(|r| r.concept_id == *id);
        if !conocido {
            return Err(StoreError::NotFound(id.clone()));
        }
        let cut = before_seq.unwrap_or(u64::MAX);
        Ok(snap
            .revisions
            .iter()
            .rev()
            .filter(|r| r.concept_id == *id && r.seq < cut)
            .take(limit)
            .cloned()
            .collect())
    }

    fn delete(
        &mut self,
        id: &ConceptId,
        expected: ContentId,
        actor: &Principal,
        reason: String,
    ) -> Result<DeleteOutcome, StoreError> {
        let mut snap = self.snapshot()?;
        let Some(doc) = snap.live.get(id).cloned() else {
            return Err(StoreError::NotFound(id.clone()));
        };
        if doc.content_id != expected {
            return Err(StoreError::Conflict(Conflict {
                expected: Some(expected),
                current: Some(doc.content_id),
                incoming: expected,
            }));
        }
        let revision = Revision {
            seq: snap.next_seq,
            concept_id: id.clone(),
            base: Some(doc.content_id),
            result: doc.content_id,
            actor: actor.clone(),
            reason,
        };
        let message = format!(
            "{}\n\n{}",
            revision.reason.replace(['\n', '\r'], " "),
            encode_rev(&RevOp::Delete, &revision, doc.version)
        );
        let resp = block_on(self.delete_file(&self.doc_path(id), &message, &doc.file_sha))?;

        snap.live.remove(id);
        snap.gone.insert(id.clone(), GoneDoc { version: doc.version });
        snap.revisions.push(revision.clone());
        snap.next_seq = revision.seq + 1;
        snap.head_sha = resp.commit.sha;
        self.store_cache(snap);

        Ok(DeleteOutcome { content_id: doc.content_id, version: doc.version, revision })
    }

    fn backlinks(&self, id: &ConceptId) -> Result<Vec<Backlink>, StoreError> {
        let snap = self.snapshot()?;
        let mut out = Vec::new();
        for (source, doc) in &snap.live {
            let Some(link) = doc.links.iter().find(|l| l.target == *id) else { continue };
            out.push(Backlink { source: snap.hit(source, doc), rel: link.rel.clone() });
        }
        Ok(out)
    }

    fn commit_bulk(
        &mut self,
        requests: Vec<CommitRequest>,
        atomic: bool,
        actor: &Principal,
        budget: &Budget,
    ) -> Result<BulkOutcome, StoreError> {
        if !atomic {
            let items = requests
                .into_iter()
                .map(|req| match self.commit(req, actor, budget) {
                    Ok(outcome) => BulkItem::Done(outcome),
                    Err(e) => BulkItem::Failed(e),
                })
                .collect();
            return Ok(BulkOutcome { applied: true, items });
        }

        // Atómico: simular TODO el lote contra un clon del snapshot;
        // solo si cada item pasa se materializa en UN único commit de
        // git (blobs+tree+commit+ref sin force). Si algo falla, no se
        // ha tocado la red: rollback gratis.
        let snap = self.snapshot()?;
        let mut speculative = snap.clone();
        let mut items: Vec<BulkItem> = Vec::with_capacity(requests.len());
        let mut writes: Vec<(String, String, String)> = Vec::new(); // (path, contenido, trailer)
        let mut failed_at: Option<usize> = None;
        for (idx, req) in requests.iter().enumerate() {
            if failed_at.is_some() {
                items.push(BulkItem::Skipped);
                continue;
            }
            match Self::decide_commit(&speculative, req, actor, budget) {
                Err(e) => {
                    failed_at = Some(idx);
                    items.push(BulkItem::Failed(e));
                }
                Ok(DecidedCommit::NoChange { outcome }) => {
                    items.push(BulkItem::Done(outcome));
                }
                Ok(decided @ DecidedCommit::Write { .. }) => {
                    let DecidedCommit::Write { created, version, revision, .. } = &decided else {
                        unreachable!()
                    };
                    writes.push((
                        self.doc_path(&req.concept_id),
                        req.markdown.clone(),
                        encode_rev(&RevOp::Commit, revision, *version),
                    ));
                    items.push(BulkItem::Done(CommitOutcome {
                        revision: Some(revision.clone()),
                        content_id: revision.result,
                        version: *version,
                        created: *created,
                        no_change: false,
                    }));
                    // El siguiente item del lote ve este efecto.
                    Self::apply_write(
                        &mut speculative,
                        &req.concept_id,
                        &req.markdown,
                        &decided,
                        String::new(),
                    );
                }
            }
        }

        if let Some(idx) = failed_at {
            for (i, item) in items.iter_mut().enumerate() {
                if i != idx {
                    *item = BulkItem::Skipped;
                }
            }
            return Ok(BulkOutcome { applied: false, items });
        }
        if writes.is_empty() {
            return Ok(BulkOutcome { applied: true, items });
        }

        // Materializar: un commit con todos los archivos y todos los
        // trailers, y un update de ref que exige fast-forward.
        let new_head = block_on(async {
            let parent: GitCommitResponse = self
                .get_json(&self.url(&format!("git/commits/{}", snap.head_sha)))
                .await?;
            let base_tree = parent
                .tree
                .map(|t| t.sha)
                .ok_or_else(|| StoreError::Backend("commit sin tree".to_string()))?;

            let entries: Vec<serde_json::Value> = writes
                .iter()
                .map(|(path, content, _)| {
                    serde_json::json!({
                        "path": path, "mode": "100644", "type": "blob", "content": content,
                    })
                })
                .collect();
            let tree: ShaOnly = self
                .post_json(
                    &self.url("git/trees"),
                    &serde_json::json!({ "base_tree": base_tree, "tree": entries }),
                )
                .await?;

            let trailers: Vec<&str> = writes.iter().map(|(_, _, t)| t.as_str()).collect();
            let message = format!("lote atómico ({} cambios)\n\n{}", writes.len(), trailers.join("\n"));
            let commit: ShaOnly = self
                .post_json(
                    &self.url("git/commits"),
                    &serde_json::json!({
                        "message": message, "tree": tree.sha, "parents": [snap.head_sha],
                    }),
                )
                .await?;

            let url = self.url(&format!("git/refs/heads/{}", self.branch));
            let resp = self
                .request(self.client.patch(&url))
                .json(&serde_json::json!({ "sha": commit.sha, "force": false }))
                .send()
                .await
                .map_err(|e| StoreError::Backend(format!("PATCH {url}: {e}")))?;
            if !resp.status().is_success() {
                let status = resp.status();
                return Err(StoreError::Backend(format!(
                    "PATCH {url}: HTTP {status}: la rama avanzó durante el lote"
                )));
            }
            Ok::<String, StoreError>(commit.sha)
        });
        if let Err(e) = new_head {
            // La rama se movió (u otro fallo de red): el lote no
            // quedó escrito y la caché ya no es de fiar.
            *self.cache.lock().unwrap() = None;
            return Err(e);
        }

        // Los file_sha del clon especulativo son placeholders: se
        // invalida la caché para releerlos del árbol nuevo; el estado
        // lógico del lote ya quedó confirmado en git.
        *self.cache.lock().unwrap() = None;
        Ok(BulkOutcome { applied: true, items })
    }
}

impl GithubStore {
    async fn post_json<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        body: &serde_json::Value,
    ) -> Result<T, StoreError> {
        let resp = self
            .request(self.client.post(url))
            .json(body)
            .send()
            .await
            .map_err(|e| StoreError::Backend(format!("POST {url}: {e}")))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| StoreError::Backend(format!("POST {url}: {e}")))?;
        if !status.is_success() {
            let corto: String = text.chars().take(200).collect();
            return Err(StoreError::Backend(format!("POST {url}: HTTP {status}: {corto}")));
        }
        serde_json::from_str(&text).map_err(|e| {
            StoreError::Backend(format!("POST {url}: respuesta no deserializable: {e}"))
        })
    }
}

impl StoreMaintenance for GithubStore {
    fn link_health(&self, id: &ConceptId) -> Result<LinkHealth, StoreError> {
        let snap = self.snapshot()?;
        let Some(doc) = snap.live.get(id) else {
            return Err(StoreError::NotFound(id.clone()));
        };
        let mut health = LinkHealth::default();
        for link in &doc.links {
            if snap.live.contains_key(&link.target) {
                health.ok.push(link.clone());
            } else if snap.gone.contains_key(&link.target) {
                health.deleted.push(link.clone());
            } else {
                health.broken.push(link.clone());
            }
        }
        Ok(health)
    }

    fn validate(
        &self,
        path_prefix: Option<&str>,
        budget: &Budget,
    ) -> Result<ValidationReport, StoreError> {
        let snap = self.snapshot()?;
        let cap = budget.max_search_results;
        let mut report = ValidationReport::default();
        for (source, doc) in &snap.live {
            if let Some(prefix) = path_prefix {
                if !matches_prefix(source.as_str(), prefix) {
                    continue;
                }
            }
            for link in &doc.links {
                if snap.live.contains_key(&link.target) {
                    continue;
                }
                if snap.gone.contains_key(&link.target) {
                    report.deleted_referenced_total += 1;
                    if report.deleted_referenced.len() < cap {
                        report.deleted_referenced.push((source.clone(), link.target.clone()));
                    }
                } else {
                    report.broken_links_total += 1;
                    if report.broken_links.len() < cap {
                        report.broken_links.push((source.clone(), link.target.clone()));
                    }
                }
            }
        }
        Ok(report)
    }

    fn stats(&self, budget: &Budget) -> Result<GraphStats, StoreError> {
        let snap = self.snapshot()?;
        let cap = budget.max_search_results;
        let mut stats = GraphStats {
            documents: snap.live.len(),
            deleted_documents: snap.gone.len(),
            ..GraphStats::default()
        };

        let mut by_type: BTreeMap<String, usize> = BTreeMap::new();
        let mut by_tag: BTreeMap<String, usize> = BTreeMap::new();
        let mut incoming: BTreeMap<ConceptId, usize> = BTreeMap::new();
        for doc in snap.live.values() {
            *by_type.entry(doc.doc_type.clone()).or_default() += 1;
            for tag in &doc.tags {
                *by_tag.entry(tag.clone()).or_default() += 1;
            }
        }
        for doc in snap.live.values() {
            for link in &doc.links {
                *incoming.entry(link.target.clone()).or_default() += 1;
            }
        }
        stats.by_type = sorted_desc(by_type, cap);
        stats.by_tag = sorted_desc(by_tag, cap);
        let mut top: Vec<(ConceptId, usize)> = incoming.clone().into_iter().collect();
        top.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        top.truncate(cap);
        stats.top_linked = top;
        for (id, doc) in &snap.live {
            if stats.orphans.len() >= cap {
                break;
            }
            if doc.links.is_empty() && !incoming.contains_key(id) {
                stats.orphans.push(id.clone());
            }
        }
        Ok(stats)
    }

    fn status(&self) -> Result<StoreStatus, StoreError> {
        let report = self.validate(None, &Budget::default())?;
        let snap = self.snapshot()?;
        Ok(StoreStatus {
            documents: snap.live.len(),
            deleted_documents: snap.gone.len(),
            broken_links: report.broken_links_total,
            deleted_referenced: report.deleted_referenced_total,
            ..StoreStatus::default()
        })
    }

    fn embed_pending(
        &mut self,
        _path_prefix: Option<&str>,
        _max: usize,
    ) -> Result<EmbedOutcome, StoreError> {
        // Git no indexa embeddings: la búsqueda de este backend es
        // textual y nunca hay trabajo semántico pendiente. Vacío es
        // la verdad (igual que InMemoryStore).
        Ok(EmbedOutcome::default())
    }
}

fn sorted_desc(map: BTreeMap<String, usize>, cap: usize) -> Vec<(String, usize)> {
    let mut v: Vec<(String, usize)> = map.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v.truncate(cap);
    v
}

impl graph_core::NeighborSource for GithubStore {
    type Error = Infallible;

    fn neighbors(&self, id: &ConceptId) -> Result<Vec<ConceptId>, Infallible> {
        Ok(self
            .snapshot()
            .ok()
            .and_then(|s| s.live.get(id).map(|d| d.links.iter().map(|l| l.target.clone()).collect()))
            .unwrap_or_default())
    }

    fn document_size(&self, id: &ConceptId) -> Result<Option<usize>, Infallible> {
        Ok(self.snapshot().ok().and_then(|s| s.live.get(id).map(|d| d.raw.len())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn el_trailer_sobrevive_ida_y_vuelta() {
        let rev = Revision {
            seq: 7,
            concept_id: ConceptId::parse("people/alice").unwrap(),
            base: Some(ContentId(hash_core::sha256(b"antes"))),
            result: ContentId(hash_core::sha256(b"despues")),
            actor: Principal::local_dev(),
            reason: "motivo con | barras y espacios".to_string(),
        };
        let line = encode_rev(&RevOp::Commit, &rev, 3);
        let (op, back, version) = decode_rev(&line).expect("decodifica");
        assert_eq!(op, RevOp::Commit);
        assert_eq!(version, 3);
        assert_eq!(back, rev);
    }

    #[test]
    fn lineas_ajenas_no_confunden_al_decodificador() {
        assert!(decode_rev("un mensaje de commit humano").is_none());
        assert!(decode_rev("Memory-Rev: rota").is_none());
    }
}
