//! Repositorio de memoria: blobs inmutables + cabezas móviles.
//!
//! Modelo de datos (el mismo que tendrá Postgres en el hito 2):
//!
//! ```text
//! blobs      : ContentId -> Arc<str>      (inmutable, deduplicado)
//! heads      : ConceptId -> Head          (puntero mutable + versión)
//! revisions  : Vec<Revision>              (append-only, seq global)
//! links      : ConceptId -> Vec<ConceptId>(índice DERIVADO de heads)
//! ```
//!
//! SOLID en juego:
//! - **L (sustitución de Liskov):** [`MemoryRepository`] es el
//!   contrato. `InMemoryStore` (hito 1) y `SupabaseStore` (hito 2)
//!   deben ser indistinguibles para quien use el trait: mismos
//!   errores, mismas garantías CAS. Los tests del trait se escriben
//!   una vez y se ejecutan contra ambas implementaciones.
//! - **D:** `mcp-core` y las herramientas dependen del trait, nunca
//!   de `InMemoryStore` directamente.
//! - **S:** validar OKF es de `okf-core`; decidir el CAS es de
//!   `conflict-core`; aquí solo se orquesta y se almacena.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use conflict_core::{decide, CommitDecision};
use graph_core::NeighborSource;
use hash_core::sha256;
use memory_model::{Budget, ConceptId, ContentId, Principal, Revision};
use store_core::{CommitOutcome, CommitRequest, DocumentView, MemoryRepository, SearchHit, SearchQuery, StoreError, TagsMode};
use std::collections::{BTreeMap, HashMap};
use std::convert::Infallible;
use std::sync::Arc;

/// Cabeza de un documento: a qué blob apunta y cuántas veces avanzó.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Head {
    /// Blob al que apunta la cabeza ahora mismo.
    pub content_id: ContentId,
    /// Cuántas veces avanzó (1 = recién creado).
    pub version: u64,
}

/// Implementación en memoria del hito 1.
///
/// # Ejemplo
///
/// El ciclo completo crear → leer → editar, con el CAS protegiendo
/// la escritura concurrente:
///
/// ```
/// use memory_model::{Budget, ConceptId, Principal};
/// use memory_store::InMemoryStore;
/// use store_core::{CommitRequest, MemoryRepository, StoreError};
///
/// let mut store = InMemoryStore::new();
/// let id = ConceptId::parse("notas/rust").unwrap();
/// let req = |expected, markdown: &str| CommitRequest {
///     concept_id: id.clone(),
///     expected,
///     markdown: markdown.to_string(),
///     reason: "ejemplo rustdoc".to_string(),
/// };
/// let actor = Principal::local_dev();
/// let budget = Budget::default();
///
/// // Crear (expected = None):
/// let v1 = store
///     .commit(req(None, "---\ntype: note\n---\nhola\n"), &actor, &budget)
///     .unwrap();
/// assert!(v1.created);
///
/// // Editar declarando la base leída: el CAS lo acepta.
/// let v2 = store
///     .commit(req(Some(v1.content_id), "---\ntype: note\n---\nadiós\n"), &actor, &budget)
///     .unwrap();
/// assert_eq!(v2.version, 2);
///
/// // Reescribir sobre la base VIEJA: conflicto, nunca pérdida.
/// let err = store
///     .commit(req(Some(v1.content_id), "---\ntype: note\n---\npisotón\n"), &actor, &budget)
///     .unwrap_err();
/// assert!(matches!(err, StoreError::Conflict(_)));
/// ```
#[derive(Debug, Default)]
pub struct InMemoryStore {
    blobs: HashMap<ContentId, Arc<str>>,
    heads: BTreeMap<ConceptId, Head>,
    revisions: Vec<Revision>,
    /// Índice derivado: se reconstruye en cada commit del documento.
    links: BTreeMap<ConceptId, Vec<ConceptId>>,
    next_seq: u64,
}

impl InMemoryStore {
    /// Almacén vacío con la secuencia de revisiones en 1.
    pub fn new() -> Self {
        InMemoryStore { next_seq: 1, ..Default::default() }
    }

    fn content_id(markdown: &str) -> ContentId {
        ContentId(sha256(markdown.as_bytes()))
    }

    fn view(&self, id: &ConceptId, head: &Head, budget: &Budget) -> Result<DocumentView, StoreError> {
        let raw = self
            .blobs
            .get(&head.content_id)
            .cloned()
            .ok_or_else(|| StoreError::Backend(format!("blob perdido para {id}")))?;
        // El blob ya se validó al escribir; re-analizamos para no
        // guardar metadatos duplicados en memoria. En Postgres esto
        // serán columnas derivadas.
        let doc = okf_core::parse_document(&raw, budget)?;
        Ok(DocumentView {
            concept_id: id.clone(),
            content_id: head.content_id,
            version: head.version,
            raw,
            doc_type: doc.doc_type,
            title: doc.title,
            status: doc.status,
            tags: doc.tags,
            links: doc.links,
        })
    }
}

impl MemoryRepository for InMemoryStore {
    fn get(&self, id: &ConceptId) -> Result<Option<DocumentView>, StoreError> {
        match self.heads.get(id) {
            None => Ok(None),
            Some(head) => Ok(Some(self.view(id, head, &Budget::default())?)),
        }
    }

    fn search(&self, query: &SearchQuery, budget: &Budget) -> Result<Vec<SearchHit>, StoreError> {
        let limit = query
            .limit
            .unwrap_or(budget.max_search_results)
            .min(budget.max_search_results);
        let needle = query.text.as_ref().map(|t| t.to_lowercase());
        let mut hits = Vec::new();

        for (id, head) in &self.heads {
            if hits.len() >= limit {
                break;
            }
            // El prefijo se comprueba ANTES de construir la vista:
            // descartar por id no requiere re-analizar el documento.
            if let Some(prefix) = &query.path_prefix {
                if !id.as_str().starts_with(prefix.as_str()) {
                    continue;
                }
            }
            let view = self.view(id, head, budget)?;
            if let Some(t) = &query.doc_type {
                if view.doc_type != *t {
                    continue;
                }
            }
            if let Some(st) = &query.status {
                if view.status.as_deref() != Some(st.as_str()) {
                    continue;
                }
            }
            if !query.tags.is_empty() {
                let has = |tag: &String| view.tags.iter().any(|x| x == tag);
                let ok = match query.tags_mode {
                    TagsMode::Any => query.tags.iter().any(has),
                    TagsMode::All => query.tags.iter().all(has),
                };
                if !ok {
                    continue;
                }
            }
            if let Some(needle) = &needle {
                let in_id = view.concept_id.as_str().contains(needle.as_str());
                let in_title = view
                    .title
                    .as_deref()
                    .is_some_and(|t| t.to_lowercase().contains(needle.as_str()));
                let in_tags = view.tags.iter().any(|t| t.to_lowercase().contains(needle.as_str()));
                let in_body = view.raw.to_lowercase().contains(needle.as_str());
                if !(in_id || in_title || in_tags || in_body) {
                    continue;
                }
            }
            hits.push(SearchHit {
                concept_id: view.concept_id,
                content_id: view.content_id,
                doc_type: view.doc_type,
                title: view.title,
                status: view.status,
                tags: view.tags,
            });
        }
        Ok(hits)
    }

    fn commit(
        &mut self,
        request: CommitRequest,
        actor: &Principal,
        budget: &Budget,
    ) -> Result<CommitOutcome, StoreError> {
        // 1. Validar SIEMPRE antes de mirar el CAS: un documento
        //    inválido no debe ni llegar a producir un conflicto.
        let doc = okf_core::parse_document(&request.markdown, budget)?;

        // 2. Identidad de contenido y decisión CAS pura.
        let incoming = Self::content_id(&request.markdown);
        let head = self.heads.get(&request.concept_id);
        let decision = decide(head.map(|h| h.content_id), request.expected, incoming);

        match decision {
            CommitDecision::Conflict(c) => Err(StoreError::Conflict(c)),
            CommitDecision::NoChange => {
                let head = self.heads.get(&request.concept_id).expect("NoChange implica cabeza");
                Ok(CommitOutcome {
                    revision: None,
                    content_id: head.content_id,
                    version: head.version,
                    created: false,
                    no_change: true,
                })
            }
            CommitDecision::Create | CommitDecision::Update => {
                let created = matches!(decision, CommitDecision::Create);
                let base = head.map(|h| h.content_id);
                let version = head.map(|h| h.version + 1).unwrap_or(1);

                // 3. Escribir: blob inmutable, cabeza, revisión e
                //    índice de enlaces. En memoria esto es atómico
                //    porque tenemos &mut exclusivo; en Postgres será
                //    una transacción.
                self.blobs
                    .entry(incoming)
                    .or_insert_with(|| Arc::from(request.markdown.as_str()));
                self.heads.insert(
                    request.concept_id.clone(),
                    Head { content_id: incoming, version },
                );
                self.links.insert(request.concept_id.clone(), doc.links.clone());

                let revision = Revision {
                    seq: self.next_seq,
                    concept_id: request.concept_id.clone(),
                    base,
                    result: incoming,
                    actor: actor.clone(),
                    reason: request.reason,
                };
                self.next_seq += 1;
                self.revisions.push(revision.clone());

                Ok(CommitOutcome {
                    revision: Some(revision),
                    content_id: incoming,
                    version,
                    created,
                    no_change: false,
                })
            }
        }
    }

    fn history(
        &self,
        id: &ConceptId,
        limit: usize,
        before_seq: Option<u64>,
    ) -> Result<Vec<Revision>, StoreError> {
        if !self.heads.contains_key(id) && !self.revisions.iter().any(|r| r.concept_id == *id) {
            return Err(StoreError::NotFound(id.clone()));
        }
        let cut = before_seq.unwrap_or(u64::MAX);
        let out: Vec<Revision> = self
            .revisions
            .iter()
            .rev()
            .filter(|r| r.concept_id == *id && r.seq < cut)
            .take(limit)
            .cloned()
            .collect();
        Ok(out)
    }
}

/// SOLID-I: el grafo solo necesita vecinos y tamaños; se los damos
/// sin exponer el resto del repositorio.
impl NeighborSource for InMemoryStore {
    type Error = Infallible;

    fn neighbors(&self, id: &ConceptId) -> Result<Vec<ConceptId>, Infallible> {
        Ok(self.links.get(id).cloned().unwrap_or_default())
    }

    fn document_size(&self, id: &ConceptId) -> Result<Option<usize>, Infallible> {
        Ok(self
            .heads
            .get(id)
            .and_then(|h| self.blobs.get(&h.content_id))
            .map(|blob| blob.len()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> ConceptId {
        ConceptId::parse(s).unwrap()
    }

    fn doc(title: &str, body: &str) -> String {
        format!("---\ntype: note\ntitle: {title}\n---\n{body}\n")
    }

    fn commit(
        store: &mut InMemoryStore,
        concept: &str,
        expected: Option<ContentId>,
        markdown: &str,
    ) -> Result<CommitOutcome, StoreError> {
        store.commit(
            CommitRequest {
                concept_id: id(concept),
                expected,
                markdown: markdown.to_string(),
                reason: "test".to_string(),
            },
            &Principal::local_dev(),
            &Budget::default(),
        )
    }

    #[test]
    fn crear_leer_actualizar() {
        let mut s = InMemoryStore::new();
        let v1 = commit(&mut s, "notas/rust", None, &doc("Rust", "hola")).unwrap();
        assert!(v1.created);
        assert_eq!(v1.version, 1);

        let view = s.get(&id("notas/rust")).unwrap().unwrap();
        assert_eq!(view.title.as_deref(), Some("Rust"));
        assert_eq!(view.content_id, v1.content_id);

        let v2 = commit(&mut s, "notas/rust", Some(v1.content_id), &doc("Rust", "adiós")).unwrap();
        assert_eq!(v2.version, 2);
        assert!(!v2.created);
    }

    #[test]
    fn una_base_obsoleta_nunca_pisa_una_escritura_mas_nueva() {
        let mut s = InMemoryStore::new();
        let v1 = commit(&mut s, "n", None, &doc("t", "v1")).unwrap();
        let v2 = commit(&mut s, "n", Some(v1.content_id), &doc("t", "v2")).unwrap();

        // Otro agente que leyó v1 intenta escribir sin ver v2.
        let err = commit(&mut s, "n", Some(v1.content_id), &doc("t", "pisotón")).unwrap_err();
        match err {
            StoreError::Conflict(c) => {
                assert_eq!(c.expected, Some(v1.content_id));
                assert_eq!(c.current, Some(v2.content_id));
            }
            other => panic!("se esperaba conflicto, hubo {other:?}"),
        }
        // El contenido de v2 sigue intacto.
        let view = s.get(&id("n")).unwrap().unwrap();
        assert_eq!(view.content_id, v2.content_id);
    }

    #[test]
    fn commit_idempotente() {
        let mut s = InMemoryStore::new();
        let texto = doc("t", "igual");
        let v1 = commit(&mut s, "n", None, &texto).unwrap();
        let v2 = commit(&mut s, "n", Some(v1.content_id), &texto).unwrap();
        assert!(v2.no_change);
        assert_eq!(v2.version, 1);
        assert_eq!(s.history(&id("n"), 10, None).unwrap().len(), 1);
    }

    #[test]
    fn documento_invalido_no_toca_el_almacen() {
        let mut s = InMemoryStore::new();
        let err = commit(&mut s, "n", None, "sin frontmatter").unwrap_err();
        assert!(matches!(err, StoreError::Okf(_)));
        assert!(s.get(&id("n")).unwrap().is_none());
    }

    #[test]
    fn busqueda_con_filtros() {
        let mut s = InMemoryStore::new();
        commit(&mut s, "people/alice", None,
            "---\ntype: person\ntitle: Alice\ntags:\n  - rust\n---\nIngeniera\n").unwrap();
        commit(&mut s, "people/bob", None,
            "---\ntype: person\ntitle: Bob\n---\nDiseñador\n").unwrap();
        commit(&mut s, "notas/rust", None, &doc("Apuntes Rust", "lenguaje")).unwrap();

        let budget = Budget::default();
        let q = |text: Option<&str>, doc_type: Option<&str>, tags: &[&str]| SearchQuery {
            text: text.map(String::from),
            doc_type: doc_type.map(String::from),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            ..SearchQuery::default()
        };

        assert_eq!(s.search(&q(Some("rust"), None, &[]), &budget).unwrap().len(), 2);
        assert_eq!(s.search(&q(Some("rust"), Some("person"), &[]), &budget).unwrap().len(), 1);
        assert_eq!(s.search(&q(None, None, &["rust"]), &budget).unwrap().len(), 1);
        assert_eq!(s.search(&q(Some("ingeniera"), None, &[]), &budget).unwrap().len(), 1);
        assert_eq!(s.search(&q(Some("nada-de-esto"), None, &[]), &budget).unwrap().len(), 0);

        let prefijo = SearchQuery {
            path_prefix: Some("people/".to_string()),
            ..SearchQuery::default()
        };
        assert_eq!(s.search(&prefijo, &budget).unwrap().len(), 2);
    }

    #[test]
    fn historia_paginada() {
        let mut s = InMemoryStore::new();
        let mut last = None;
        for i in 0..5 {
            let out = commit(&mut s, "n", last, &doc("t", &format!("v{i}"))).unwrap();
            last = Some(out.content_id);
        }
        let page1 = s.history(&id("n"), 2, None).unwrap();
        assert_eq!(page1.len(), 2);
        assert_eq!(page1[0].seq, 5);
        let page2 = s.history(&id("n"), 2, Some(page1[1].seq)).unwrap();
        assert_eq!(page2[0].seq, 3);
        assert!(matches!(
            s.history(&id("no-existe"), 2, None),
            Err(StoreError::NotFound(_))
        ));
    }

    #[test]
    fn los_enlaces_alimentan_el_grafo() {
        let mut s = InMemoryStore::new();
        commit(&mut s, "a", None, "---\ntype: nota\n---\nver [[b]] y [[c]]\n").unwrap();
        commit(&mut s, "b", None, "---\ntype: nota\n---\nver [[c]]\n").unwrap();
        commit(&mut s, "c", None, "---\ntype: nota\n---\nfin\n").unwrap();

        let t = graph_core::bounded_bfs(&s, &id("a"), &Budget::default()).unwrap();
        let ids: Vec<&str> = t.visited.iter().map(|v| v.id.as_str()).collect();
        assert_eq!(ids, vec!["a", "b", "c"]);
    }

    #[test]
    fn blobs_deduplicados() {
        let mut s = InMemoryStore::new();
        let texto = doc("t", "compartido");
        commit(&mut s, "uno", None, &texto).unwrap();
        commit(&mut s, "dos", None, &texto).unwrap();
        assert_eq!(s.blobs.len(), 1);
        assert_eq!(s.heads.len(), 2);
    }
}
