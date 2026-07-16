//! Contrato y tipos base de persistencia de documentos okf-mcp.
//!
//! SOLID en juego:
//! - **L (sustitución de Liskov):** [`MemoryRepository`] es el contrato abstracto.
//!   Tanto `InMemoryStore` como `SupabaseStore` deben comportarse de forma idéntica
//!   para cualquier consumidor, validado por los tests en `contract`.

#![forbid(unsafe_code)]

pub mod contract;

use conflict_core::Conflict;
use memory_model::{Budget, ConceptId, ContentId, Principal, Revision};
use okf_core::OkfError;
use std::fmt;
use std::sync::Arc;

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
