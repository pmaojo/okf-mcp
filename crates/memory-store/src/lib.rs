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

pub mod contract;

use conflict_core::{decide, CommitDecision, Conflict};
use graph_core::NeighborSource;
use hash_core::sha256;
use memory_model::{Budget, ConceptId, ContentId, Principal, Revision};
use okf_core::OkfError;
use std::collections::{BTreeMap, HashMap};
use std::convert::Infallible;
use std::fmt;
use std::sync::Arc;

/// Cabeza de un documento: a qué blob apunta y cuántas veces avanzó.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Head {
    pub content_id: ContentId,
    pub version: u64,
}

/// Vista de lectura de un documento. `raw` es un `Arc<str>` para
/// compartir el blob sin copiarlo (los blobs son inmutables, así que
/// compartir es seguro por construcción).
#[derive(Debug, Clone)]
pub struct DocumentView {
    pub concept_id: ConceptId,
    pub content_id: ContentId,
    pub version: u64,
    pub raw: Arc<str>,
    pub doc_type: String,
    pub title: Option<String>,
    pub tags: Vec<String>,
    pub links: Vec<ConceptId>,
}

/// Petición de escritura. `expected` = hash que el cliente leyó
/// (`None` si cree estar creando el documento).
#[derive(Debug, Clone)]
pub struct CommitRequest {
    pub concept_id: ConceptId,
    pub expected: Option<ContentId>,
    pub markdown: String,
    pub reason: String,
}

/// Resultado de un commit aceptado.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitOutcome {
    pub revision: Option<Revision>,
    pub content_id: ContentId,
    pub version: u64,
    pub created: bool,
    /// `true` si el contenido ya era idéntico (idempotencia).
    pub no_change: bool,
}

/// Consulta de búsqueda. Todos los criterios son opcionales y se
/// combinan con AND.
#[derive(Debug, Clone, Default)]
pub struct SearchQuery {
    /// Subcadena, sin distinción de mayúsculas, sobre id, título,
    /// tags y cuerpo.
    pub text: Option<String>,
    pub doc_type: Option<String>,
    pub tag: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub concept_id: ConceptId,
    pub content_id: ContentId,
    pub doc_type: String,
    pub title: Option<String>,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    /// El documento no pasa la validación OKF.
    Okf(OkfError),
    /// CAS rechazado: datos para releer y reintentar.
    Conflict(Conflict),
    /// El concepto no existe (para operaciones que lo exigen).
    NotFound(ConceptId),
    /// Fallo del backend (en memoria no ocurre; sobre red sí).
    Backend(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::Okf(e) => write!(f, "documento OKF inválido: {e}"),
            StoreError::Conflict(c) => write!(
                f,
                "conflicto de revisión: se esperaba {:?}, hay {:?}",
                c.expected.map(|h| h.to_hex()),
                c.current.map(|h| h.to_hex()),
            ),
            StoreError::NotFound(id) => write!(f, "no existe el concepto {id}"),
            StoreError::Backend(msg) => write!(f, "error del backend: {msg}"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<OkfError> for StoreError {
    fn from(e: OkfError) -> Self {
        StoreError::Okf(e)
    }
}

/// El contrato que cualquier backend de memoria debe cumplir.
pub trait MemoryRepository {
    fn get(&self, id: &ConceptId) -> Result<Option<DocumentView>, StoreError>;

    fn search(&self, query: &SearchQuery, budget: &Budget) -> Result<Vec<SearchHit>, StoreError>;

    fn commit(
        &mut self,
        request: CommitRequest,
        actor: &Principal,
        budget: &Budget,
    ) -> Result<CommitOutcome, StoreError>;

    /// Historia de un concepto, de más reciente a más antigua.
    /// `before_seq` pagina: solo revisiones con `seq < before_seq`.
    fn history(
        &self,
        id: &ConceptId,
        limit: usize,
        before_seq: Option<u64>,
    ) -> Result<Vec<Revision>, StoreError>;
}

/// Implementación en memoria del hito 1.
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
            let view = self.view(id, head, budget)?;
            if let Some(t) = &query.doc_type {
                if view.doc_type != *t {
                    continue;
                }
            }
            if let Some(tag) = &query.tag {
                if !view.tags.iter().any(|x| x == tag) {
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
        let q = |text: Option<&str>, doc_type: Option<&str>, tag: Option<&str>| SearchQuery {
            text: text.map(String::from),
            doc_type: doc_type.map(String::from),
            tag: tag.map(String::from),
            limit: None,
        };

        assert_eq!(s.search(&q(Some("rust"), None, None), &budget).unwrap().len(), 2);
        assert_eq!(s.search(&q(Some("rust"), Some("person"), None), &budget).unwrap().len(), 1);
        assert_eq!(s.search(&q(None, None, Some("rust")), &budget).unwrap().len(), 1);
        assert_eq!(s.search(&q(Some("ingeniera"), None, None), &budget).unwrap().len(), 1);
        assert_eq!(s.search(&q(Some("nada-de-esto"), None, None), &budget).unwrap().len(), 0);
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
