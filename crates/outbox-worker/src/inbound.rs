//! Importación INCREMENTAL de pushes de GitHub a Supabase.
//!
//! Por qué no se reutiliza `reconcile_github_to_supabase`: esa función
//! compara el estado completo y trata a GitHub como verdad absoluta
//! (sobrescribe todo lo que difiere y marca como borrado todo lo que
//! solo existe en Supabase). Con Supabase como almacén principal eso
//! destruiría datos. Aquí solo se tocan los archivos que el push
//! menciona:
//!
//! - archivo presente con el MISMO contenido que Supabase: no se hace
//!   nada (así el espejo de salida no provoca un bucle: su propio
//!   commit llega de vuelta y se reconoce idéntico);
//! - presente con contenido distinto: commit normal (validación OKF,
//!   enlaces, embeddings, outbox) con `expected` = cabeza actual, de
//!   modo que la versión anterior queda en `revisions`; GitHub gana
//!   como revisión nueva, no se pierde nada;
//! - ausente en el commit final del push: borrado lógico de ese
//!   concepto, nunca de otros.
//!
//! La lógica de decisión es genérica sobre [`MemoryRepository`] y se
//! prueba sin red ni base de datos; la parte de red
//! ([`fetch_remote`]) es una sola llamada a la API de Contents por
//! archivo.

use memory_model::{Budget, ConceptId, Principal};
use serde_json::Value;
use std::collections::BTreeSet;
use store_core::{CommitRequest, MemoryRepository};

/// Archivos que como máximo se importan por push. El resto se cuenta
/// en [`InboundReport::skipped`]: cada archivo cuesta una llamada a la
/// API de GitHub y el webhook tiene que responder a tiempo.
pub const MAX_FILES_PER_PUSH: usize = 100;

/// Estado final de un archivo en el commit `after` del push.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Remote {
    /// El archivo existe con estos bytes.
    Present(String),
    /// El archivo ya no existe.
    Absent,
}

/// Qué pasó con cada archivo del push.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InboundReport {
    /// Conceptos creados o actualizados desde GitHub.
    pub imported: usize,
    /// Archivos que ya coincidían con Supabase (incluye ecos del
    /// espejo y borrados de algo que ya no existía).
    pub unchanged: usize,
    /// Conceptos borrados lógicamente porque el push borró el archivo.
    pub deleted: usize,
    /// Archivos ignorados: fuera de `base_path`, no `.md`, id
    /// inválido o por encima de [`MAX_FILES_PER_PUSH`].
    pub skipped: usize,
    /// Archivos que Supabase rechazó (p. ej. OKF inválido), con el
    /// motivo. Un fallo no aborta el resto.
    pub failed: Vec<String>,
    /// El payload venía truncado (GitHub lista como mucho 20 commits):
    /// puede haber archivos del push sin importar.
    pub incomplete: bool,
}

/// Concepto que corresponde a una ruta del repo: `{base_path}/{id}.md`
/// (misma convención que usa el espejo de salida y `GithubStore`).
///
/// ```
/// use outbox_worker::inbound::concept_for_path;
/// assert_eq!(concept_for_path("memoria", "memoria/people/alice.md").unwrap().as_str(), "people/alice");
/// assert_eq!(concept_for_path("", "people/alice.md").unwrap().as_str(), "people/alice");
/// assert!(concept_for_path("memoria", "otra/people/alice.md").is_none());
/// assert!(concept_for_path("memoria", "memoria/README.txt").is_none());
/// assert!(concept_for_path("memoria", "memoria/../x.md").is_none());
/// ```
pub fn concept_for_path(base_path: &str, path: &str) -> Option<ConceptId> {
    let base = base_path.trim_matches('/');
    let rel = if base.is_empty() {
        path
    } else {
        path.strip_prefix(base)?.strip_prefix('/')?
    };
    let id = rel.strip_suffix(".md")?;
    ConceptId::parse(id).ok()
}

/// Rutas que menciona un payload de `push` (`added`, `modified` y
/// `removed` de todos sus commits). Devuelve también si el payload
/// puede estar truncado.
pub fn changed_paths(payload: &Value) -> (BTreeSet<String>, bool) {
    let mut out = BTreeSet::new();
    let commits = payload["commits"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    for commit in commits {
        for key in ["added", "modified", "removed"] {
            if let Some(list) = commit[key].as_array() {
                out.extend(list.iter().filter_map(|p| p.as_str().map(String::from)));
            }
        }
    }
    // GitHub incluye como máximo 20 commits en el payload.
    (out, commits.len() >= 20)
}

/// Aplica a `repo` el estado remoto de UN archivo. No lanza: devuelve
/// el resultado para que el llamador lo cuente.
pub fn apply_remote<R: MemoryRepository>(
    repo: &mut R,
    id: &ConceptId,
    remote: Remote,
    actor: &Principal,
    budget: &Budget,
) -> Result<Applied, String> {
    let existing = repo.get(id).map_err(|e| e.to_string())?;
    match remote {
        Remote::Present(raw) => {
            if existing.as_ref().is_some_and(|v| &*v.raw == raw.as_str()) {
                return Ok(Applied::Unchanged);
            }
            repo.commit(
                CommitRequest {
                    concept_id: id.clone(),
                    expected: existing.map(|v| v.content_id),
                    markdown: raw,
                    reason: "importado desde GitHub (push)".to_string(),
                },
                actor,
                budget,
            )
            .map(|_| Applied::Imported)
            .map_err(|e| e.to_string())
        }
        Remote::Absent => match existing {
            None => Ok(Applied::Unchanged),
            Some(v) => repo
                .delete(id, v.content_id, actor, "borrado en GitHub (push)".to_string())
                .map(|_| Applied::Deleted)
                .map_err(|e| e.to_string()),
        },
    }
}

/// Resultado de [`apply_remote`] para un archivo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Applied {
    /// Creado o actualizado.
    Imported,
    /// Ya coincidía.
    Unchanged,
    /// Borrado lógico.
    Deleted,
}

/// El actor con el que queda registrada la importación en
/// `revisions`.
pub fn github_actor() -> Principal {
    Principal { subject: "github/webhook".to_string(), client_id: "github".to_string() }
}

/// Aplica el estado remoto de varios archivos y resume el resultado.
/// `files` = pares (ruta del repo, estado remoto); las rutas que no
/// cuelgan de `base_path` o no son `.md` válidos cuentan como
/// ignoradas.
pub fn apply_all<R: MemoryRepository>(
    repo: &mut R,
    base_path: &str,
    files: Vec<(String, Remote)>,
    incomplete: bool,
    budget: &Budget,
) -> InboundReport {
    let actor = github_actor();
    let mut report = InboundReport { incomplete, ..InboundReport::default() };
    for (path, remote) in files {
        let Some(id) = concept_for_path(base_path, &path) else {
            report.skipped += 1;
            continue;
        };
        match apply_remote(repo, &id, remote, &actor, budget) {
            Ok(Applied::Imported) => report.imported += 1,
            Ok(Applied::Unchanged) => report.unchanged += 1,
            Ok(Applied::Deleted) => report.deleted += 1,
            Err(e) => report.failed.push(format!("{path}: {e}")),
        }
    }
    report
}

/// Lee de GitHub el estado de `path` en el commit `git_ref` (una
/// llamada a la API de Contents con `Accept: raw`). 404 = el archivo
/// no existe en ese commit.
pub async fn fetch_remote(
    client: &reqwest::Client,
    token: &str,
    repo: &str,
    path: &str,
    git_ref: &str,
) -> Result<Remote, Box<dyn std::error::Error>> {
    let url = format!("https://api.github.com/repos/{repo}/contents/{path}");
    let resp = client
        .get(&url)
        .query(&[("ref", git_ref)])
        .header("Authorization", format!("Bearer {token}"))
        .header("User-Agent", "okf-mcp-inbound")
        .header("Accept", "application/vnd.github.raw+json")
        .send()
        .await?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(Remote::Absent);
    }
    if !resp.status().is_success() {
        return Err(format!("GitHub {} al leer {path}: {}", resp.status(), resp.text().await?).into());
    }
    Ok(Remote::Present(resp.text().await?))
}

/// Importa un push completo: lee de GitHub los archivos mencionados
/// (hasta [`MAX_FILES_PER_PUSH`]) y los aplica a `repo`.
///
/// La lectura de red es asíncrona y la aplicación a `repo` es
/// síncrona (el puerto `MemoryRepository` lo es), por eso se leen
/// TODOS primero y luego se aplican.
pub async fn import_push<R: MemoryRepository>(
    repo: &mut R,
    client: &reqwest::Client,
    token: &str,
    github_repo: &str,
    base_path: &str,
    payload: &Value,
    budget: &Budget,
) -> InboundReport {
    let (paths, incomplete) = changed_paths(payload);
    let after = payload["after"].as_str().unwrap_or("");
    let mut skipped_over_cap = 0usize;
    let mut files: Vec<(String, Remote)> = Vec::new();
    let mut fetch_failed: Vec<String> = Vec::new();
    let mut relevant = 0usize;
    for path in paths {
        if concept_for_path(base_path, &path).is_none() {
            files.push((path, Remote::Absent)); // se contará como ignorado
            continue;
        }
        relevant += 1;
        if relevant > MAX_FILES_PER_PUSH {
            skipped_over_cap += 1;
            continue;
        }
        match fetch_remote(client, token, github_repo, &path, after).await {
            Ok(remote) => files.push((path, remote)),
            Err(e) => fetch_failed.push(format!("{path}: {e}")),
        }
    }
    let mut report = apply_all(repo, base_path, files, incomplete, budget);
    report.skipped += skipped_over_cap;
    report.failed.extend(fetch_failed);
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use memory_store::InMemoryStore;

    fn doc(title: &str, body: &str) -> String {
        format!("---\ntype: note\ntitle: {title}\n---\n{body}\n")
    }

    fn put(store: &mut InMemoryStore, id: &str, md: &str) {
        store
            .commit(
                CommitRequest {
                    concept_id: ConceptId::parse(id).unwrap(),
                    expected: None,
                    markdown: md.to_string(),
                    reason: "seed".to_string(),
                },
                &Principal::local_dev(),
                &Budget::default(),
            )
            .unwrap();
    }

    fn files(items: &[(&str, Remote)]) -> Vec<(String, Remote)> {
        items.iter().map(|(p, r)| (p.to_string(), r.clone())).collect()
    }

    #[test]
    fn crea_un_concepto_nuevo_desde_github() {
        let mut s = InMemoryStore::new();
        let r = apply_all(
            &mut s,
            "memoria",
            files(&[("memoria/notas/nueva.md", Remote::Present(doc("Nueva", "hola")))]),
            false,
            &Budget::default(),
        );
        assert_eq!((r.imported, r.unchanged, r.deleted), (1, 0, 0));
        assert!(s.get(&ConceptId::parse("notas/nueva").unwrap()).unwrap().is_some());
    }

    /// El eco del espejo de salida (mismo contenido) no escribe nada:
    /// es lo que evita el bucle Supabase -> GitHub -> Supabase.
    #[test]
    fn contenido_identico_no_escribe_nada() {
        let mut s = InMemoryStore::new();
        let md = doc("Igual", "x");
        put(&mut s, "notas/igual", &md);
        let r = apply_all(
            &mut s,
            "memoria",
            files(&[("memoria/notas/igual.md", Remote::Present(md))]),
            false,
            &Budget::default(),
        );
        assert_eq!((r.imported, r.unchanged), (0, 1));
        let v = s.get(&ConceptId::parse("notas/igual").unwrap()).unwrap().unwrap();
        assert_eq!(v.version, 1, "no se creó revisión");
    }

    /// Editar en GitHub gana, pero como revisión nueva: la anterior
    /// sigue en la historia.
    #[test]
    fn edicion_en_github_es_una_revision_nueva() {
        let mut s = InMemoryStore::new();
        put(&mut s, "notas/a", &doc("A", "v1"));
        let r = apply_all(
            &mut s,
            "memoria",
            files(&[("memoria/notas/a.md", Remote::Present(doc("A", "v2")))]),
            false,
            &Budget::default(),
        );
        assert_eq!(r.imported, 1);
        let id = ConceptId::parse("notas/a").unwrap();
        let v = s.get(&id).unwrap().unwrap();
        assert_eq!(v.version, 2);
        assert!(v.raw.contains("v2"));
        assert_eq!(s.history(&id, 10, None).unwrap().len(), 2, "la v1 sigue en la historia");
    }

    #[test]
    fn borrar_el_archivo_borra_solo_ese_concepto() {
        let mut s = InMemoryStore::new();
        put(&mut s, "notas/a", &doc("A", "a"));
        put(&mut s, "notas/b", &doc("B", "b"));
        let r = apply_all(
            &mut s,
            "memoria",
            files(&[("memoria/notas/a.md", Remote::Absent)]),
            false,
            &Budget::default(),
        );
        assert_eq!(r.deleted, 1);
        assert!(s.get(&ConceptId::parse("notas/a").unwrap()).unwrap().is_none());
        assert!(s.get(&ConceptId::parse("notas/b").unwrap()).unwrap().is_some(), "lo que el push no menciona no se toca");
    }

    /// La razón de ser del módulo: lo que solo existe en Supabase
    /// sobrevive a cualquier push.
    #[test]
    fn nunca_borra_lo_que_solo_existe_en_supabase() {
        let mut s = InMemoryStore::new();
        put(&mut s, "facts/skill_success_rate/g/c/default", &doc("hecho", "solo en supabase"));
        let r = apply_all(
            &mut s,
            "memoria",
            files(&[("memoria/notas/otra.md", Remote::Present(doc("Otra", "x")))]),
            false,
            &Budget::default(),
        );
        assert_eq!(r.imported, 1);
        assert!(s
            .get(&ConceptId::parse("facts/skill_success_rate/g/c/default").unwrap())
            .unwrap()
            .is_some());
    }

    #[test]
    fn borrar_algo_que_ya_no_existe_es_un_no_op() {
        let mut s = InMemoryStore::new();
        let r = apply_all(
            &mut s,
            "memoria",
            files(&[("memoria/notas/fantasma.md", Remote::Absent)]),
            false,
            &Budget::default(),
        );
        assert_eq!((r.deleted, r.unchanged), (0, 1));
    }

    #[test]
    fn un_okf_invalido_se_reporta_y_no_aborta_el_resto() {
        let mut s = InMemoryStore::new();
        let r = apply_all(
            &mut s,
            "memoria",
            files(&[
                ("memoria/notas/mal.md", Remote::Present("sin frontmatter".to_string())),
                ("memoria/notas/bien.md", Remote::Present(doc("Bien", "x"))),
            ]),
            false,
            &Budget::default(),
        );
        assert_eq!(r.imported, 1);
        assert_eq!(r.failed.len(), 1);
        assert!(r.failed[0].contains("notas/mal.md"));
    }

    #[test]
    fn rutas_ajenas_se_ignoran() {
        let mut s = InMemoryStore::new();
        let r = apply_all(
            &mut s,
            "memoria",
            files(&[
                ("README.md", Remote::Present(doc("R", "x"))),
                ("memoria/notas/dato.txt", Remote::Present(doc("T", "x"))),
                ("otra/notas/a.md", Remote::Present(doc("O", "x"))),
            ]),
            false,
            &Budget::default(),
        );
        assert_eq!((r.imported, r.skipped), (0, 3));
    }

    #[test]
    fn changed_paths_une_added_modified_y_removed() {
        let payload = serde_json::json!({
            "commits": [
                {"added": ["memoria/a.md"], "modified": ["memoria/b.md"], "removed": []},
                {"added": [], "modified": ["memoria/b.md"], "removed": ["memoria/c.md"]},
            ]
        });
        let (paths, incomplete) = changed_paths(&payload);
        assert_eq!(paths.into_iter().collect::<Vec<_>>(), ["memoria/a.md", "memoria/b.md", "memoria/c.md"]);
        assert!(!incomplete);
    }

    #[test]
    fn veinte_commits_se_marcan_como_posible_truncado() {
        let commits: Vec<Value> = (0..20)
            .map(|i| serde_json::json!({"added": [format!("memoria/n{i}.md")], "modified": [], "removed": []}))
            .collect();
        let (_, incomplete) = changed_paths(&serde_json::json!({ "commits": commits }));
        assert!(incomplete);
    }
}
