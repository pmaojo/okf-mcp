//! Una API de GitHub falsa, en memoria, con la semántica que
//! documenta GitHub para el subconjunto que usa `GithubStore`:
//! contents con CAS por blob sha, trees recursivos, commits paginados
//! y la API de git data con update de ref sin force (fast-forward).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use base64::Engine as _;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

type Files = BTreeMap<String, String>;

#[derive(Clone)]
struct Commit {
    sha: String,
    message: String,
    parent: Option<String>,
    files: Files,
}

struct Fake {
    /// Historia lineal de la rama `main`, la más antigua primero.
    commits: Vec<Commit>,
    /// Árboles creados vía la API de git data, por sha.
    trees: HashMap<String, Files>,
    /// Commits creados vía git data aún no apuntados por la ref.
    pending: HashMap<String, Commit>,
    counter: u64,
}

impl Fake {
    fn new() -> Self {
        let mut fake = Fake {
            commits: Vec::new(),
            trees: HashMap::new(),
            pending: HashMap::new(),
            counter: 0,
        };
        let sha = fake.next_sha();
        fake.commits.push(Commit {
            sha,
            message: "init".to_string(),
            parent: None,
            files: Files::new(),
        });
        fake
    }

    fn next_sha(&mut self) -> String {
        self.counter += 1;
        format!("{:040x}", self.counter)
    }

    fn head(&self) -> &Commit {
        self.commits.last().expect("la rama siempre tiene un commit")
    }

    fn append(&mut self, message: String, files: Files) -> String {
        let parent = Some(self.head().sha.clone());
        let sha = self.next_sha();
        self.commits.push(Commit { sha: sha.clone(), message, parent, files });
        sha
    }
}

fn blob_sha(content: &str) -> String {
    let h = hash_core::sha256(content.as_bytes());
    h.iter().take(20).map(|b| format!("{b:02x}")).collect()
}

fn b64(s: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(s.as_bytes())
}

fn from_b64(s: &str) -> String {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(s)
        .expect("el adaptador siempre manda base64 válido");
    String::from_utf8(bytes).expect("utf-8")
}

type Shared = Arc<Mutex<Fake>>;

fn not_found(msg: &str) -> Response {
    (StatusCode::NOT_FOUND, Json(json!({ "message": msg }))).into_response()
}

fn conflict(msg: &str) -> Response {
    (StatusCode::CONFLICT, Json(json!({ "message": msg }))).into_response()
}

async fn get_ref(State(state): State<Shared>, Path((_, _, branch)): Path<(String, String, String)>) -> Response {
    if branch != "main" {
        return not_found("rama desconocida");
    }
    let fake = state.lock().unwrap();
    Json(json!({ "object": { "sha": fake.head().sha } })).into_response()
}

async fn patch_ref(
    State(state): State<Shared>,
    Path((_, _, branch)): Path<(String, String, String)>,
    Json(body): Json<Value>,
) -> Response {
    if branch != "main" {
        return not_found("rama desconocida");
    }
    let sha = body["sha"].as_str().unwrap_or_default().to_string();
    let mut fake = state.lock().unwrap();
    let Some(commit) = fake.pending.remove(&sha) else {
        return not_found("commit desconocido");
    };
    // Sin force: solo fast-forward desde la cabeza actual.
    if commit.parent.as_deref() != Some(fake.head().sha.as_str()) {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({ "message": "Update is not a fast forward" })),
        )
            .into_response();
    }
    fake.commits.push(commit);
    Json(json!({ "object": { "sha": sha } })).into_response()
}

async fn get_tree(
    State(state): State<Shared>,
    Path((_, _, sha)): Path<(String, String, String)>,
) -> Response {
    let fake = state.lock().unwrap();
    let Some(commit) = fake.commits.iter().find(|c| c.sha == sha) else {
        return not_found("árbol desconocido");
    };
    let tree: Vec<Value> = commit
        .files
        .iter()
        .map(|(path, content)| {
            json!({
                "path": path,
                "type": "blob",
                "sha": blob_sha(content),
                "size": content.len(),
            })
        })
        .collect();
    Json(json!({ "sha": format!("tree-{sha}"), "tree": tree, "truncated": false })).into_response()
}

async fn get_git_commit(
    State(state): State<Shared>,
    Path((_, _, sha)): Path<(String, String, String)>,
) -> Response {
    let fake = state.lock().unwrap();
    let Some(commit) = fake.commits.iter().find(|c| c.sha == sha) else {
        return not_found("commit desconocido");
    };
    Json(json!({ "sha": commit.sha, "tree": { "sha": format!("tree-{sha}") } })).into_response()
}

async fn post_tree(State(state): State<Shared>, Json(body): Json<Value>) -> Response {
    let mut fake = state.lock().unwrap();
    let mut files = match body["base_tree"].as_str() {
        Some(base) => {
            let by_commit = base
                .strip_prefix("tree-")
                .and_then(|c| fake.commits.iter().find(|x| x.sha == c).map(|x| x.files.clone()));
            match by_commit.or_else(|| fake.trees.get(base).cloned()) {
                Some(f) => f,
                None => return not_found("base_tree desconocido"),
            }
        }
        None => Files::new(),
    };
    for entry in body["tree"].as_array().cloned().unwrap_or_default() {
        let path = entry["path"].as_str().unwrap_or_default().to_string();
        match entry["content"].as_str() {
            Some(content) => {
                files.insert(path, content.to_string());
            }
            None => {
                files.remove(&path);
            }
        }
    }
    let sha = fake.next_sha();
    fake.trees.insert(sha.clone(), files);
    Json(json!({ "sha": sha })).into_response()
}

async fn post_commit(State(state): State<Shared>, Json(body): Json<Value>) -> Response {
    let mut fake = state.lock().unwrap();
    let tree_sha = body["tree"].as_str().unwrap_or_default().to_string();
    let Some(files) = fake.trees.get(&tree_sha).cloned() else {
        return not_found("tree desconocido");
    };
    let parent = body["parents"]
        .as_array()
        .and_then(|p| p.first())
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let message = body["message"].as_str().unwrap_or_default().to_string();
    let sha = fake.next_sha();
    fake.pending.insert(sha.clone(), Commit { sha: sha.clone(), message, parent, files });
    Json(json!({ "sha": sha })).into_response()
}

async fn list_commits(State(state): State<Shared>, Query(q): Query<HashMap<String, String>>) -> Response {
    let fake = state.lock().unwrap();
    let from = q.get("sha").cloned().unwrap_or_else(|| fake.head().sha.clone());
    let per_page: usize = q.get("per_page").and_then(|v| v.parse().ok()).unwrap_or(30);
    let page: usize = q.get("page").and_then(|v| v.parse().ok()).unwrap_or(1);
    let Some(idx) = fake.commits.iter().position(|c| c.sha == from) else {
        return not_found("sha desconocido");
    };
    let newest_first: Vec<Value> = fake.commits[..=idx]
        .iter()
        .rev()
        .skip((page - 1) * per_page)
        .take(per_page)
        .map(|c| json!({ "sha": c.sha, "commit": { "message": c.message } }))
        .collect();
    Json(Value::Array(newest_first)).into_response()
}

async fn contents(
    State(state): State<Shared>,
    Path((_, _, path)): Path<(String, String, String)>,
    method: axum::http::Method,
    Query(q): Query<HashMap<String, String>>,
    body: Option<Json<Value>>,
) -> Response {
    let mut fake = state.lock().unwrap();
    match method.as_str() {
        "GET" => {
            let at = q.get("ref").cloned().unwrap_or_else(|| fake.head().sha.clone());
            let commit = fake
                .commits
                .iter()
                .find(|c| c.sha == at || at == "main")
                .or_else(|| (at == "main").then(|| fake.head()));
            let Some(commit) = commit else { return not_found("ref desconocida") };
            match commit.files.get(&path) {
                None => not_found("no existe el archivo"),
                Some(content) => Json(json!({
                    "path": path,
                    "sha": blob_sha(content),
                    "encoding": "base64",
                    "content": b64(content),
                }))
                .into_response(),
            }
        }
        "PUT" => {
            let Some(Json(body)) = body else { return conflict("falta el cuerpo") };
            let message = body["message"].as_str().unwrap_or_default().to_string();
            let content = from_b64(body["content"].as_str().unwrap_or_default());
            let declared = body["sha"].as_str();
            let current = fake.head().files.get(&path).cloned();
            // El CAS de la API de contents: crear exige que no exista;
            // actualizar exige el blob sha vigente.
            match (&current, declared) {
                (Some(actual), Some(sha)) if blob_sha(actual) != sha => {
                    return conflict("sha desactualizado")
                }
                (Some(_), None) => return conflict("falta sha para actualizar"),
                (None, Some(_)) => return conflict("el archivo no existe"),
                _ => {}
            }
            let mut files = fake.head().files.clone();
            files.insert(path.clone(), content.clone());
            let commit_sha = fake.append(message, files);
            Json(json!({
                "content": { "sha": blob_sha(&content) },
                "commit": { "sha": commit_sha },
            }))
            .into_response()
        }
        "DELETE" => {
            let Some(Json(body)) = body else { return conflict("falta el cuerpo") };
            let message = body["message"].as_str().unwrap_or_default().to_string();
            let declared = body["sha"].as_str().unwrap_or_default();
            let Some(actual) = fake.head().files.get(&path).cloned() else {
                return not_found("no existe el archivo");
            };
            if blob_sha(&actual) != declared {
                return conflict("sha desactualizado");
            }
            let mut files = fake.head().files.clone();
            files.remove(&path);
            let commit_sha = fake.append(message, files);
            Json(json!({ "content": null, "commit": { "sha": commit_sha } })).into_response()
        }
        _ => not_found("método no soportado"),
    }
}

/// Handle del servidor falso: URL base y reset directo del estado.
pub struct FakeServer {
    base_url: String,
    state: Shared,
}

impl FakeServer {
    pub fn base_url(&self) -> String {
        self.base_url.clone()
    }

    /// Vuelve al estado inicial (un commit raíz sin archivos): cada
    /// propiedad del contrato parte de un repositorio vacío.
    pub fn reset(&self) {
        *self.state.lock().unwrap() = Fake::new();
    }
}

/// Levanta el servidor en un puerto libre, en su propio runtime.
pub fn spawn() -> FakeServer {
    let state: Shared = Arc::new(Mutex::new(Fake::new()));
    let router = Router::new()
        .route("/repos/{o}/{r}/git/refs/heads/{branch}", get(get_ref))
        .route("/repos/{o}/{r}/git/refs/heads/{branch}", patch(patch_ref))
        .route("/repos/{o}/{r}/git/trees/{sha}", get(get_tree))
        .route("/repos/{o}/{r}/git/trees", post(post_tree))
        .route("/repos/{o}/{r}/git/commits/{sha}", get(get_git_commit))
        .route("/repos/{o}/{r}/git/commits", post(post_commit))
        .route("/repos/{o}/{r}/commits", get(list_commits))
        .route(
            "/repos/{o}/{r}/contents/{*path}",
            get(contents).put(contents).delete(contents),
        )
        .with_state(state.clone());

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("puerto libre");
    let addr = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async move {
            let listener = tokio::net::TcpListener::from_std(listener).unwrap();
            axum::serve(listener, router).await.unwrap();
        });
    });

    FakeServer { base_url: format!("http://{addr}"), state }
}
