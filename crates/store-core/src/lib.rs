//! Contrato y tipos base de persistencia de documentos okf-mcp.
//!
//! SOLID en juego:
//! - **L (sustitución de Liskov):** [`MemoryRepository`] es el contrato abstracto.
//!   Tanto `InMemoryStore` como `SupabaseStore` deben comportarse de forma idéntica
//!   para cualquier consumidor, validado por los tests en `contract`.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

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
    /// Ruta lógica del documento.
    pub concept_id: ConceptId,
    /// Hash del contenido actual: la base para el próximo commit.
    pub content_id: ContentId,
    /// Cuántas veces avanzó la cabeza (1 = recién creado).
    pub version: u64,
    /// Los bytes exactos del Markdown, compartidos sin copiar.
    pub raw: Arc<str>,
    /// Campo `type` del frontmatter.
    pub doc_type: String,
    /// Campo `title` del frontmatter, si existe.
    pub title: Option<String>,
    /// Campo `tags` del frontmatter.
    pub tags: Vec<String>,
    /// Enlaces `[[...]]` salientes, ya validados.
    pub links: Vec<ConceptId>,
}

/// Petición de escritura. `expected` = hash que el cliente leyó
/// (`None` si cree estar creando el documento).
#[derive(Debug, Clone)]
pub struct CommitRequest {
    /// Ruta lógica del documento a escribir.
    pub concept_id: ConceptId,
    /// Hash que el cliente leyó (`None` = "estoy creando").
    pub expected: Option<ContentId>,
    /// El documento completo, frontmatter incluido.
    pub markdown: String,
    /// Motivo declarado por el agente, para la historia.
    pub reason: String,
}

/// Resultado de un commit aceptado.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitOutcome {
    /// La revisión creada; `None` si no hubo nada que escribir.
    pub revision: Option<Revision>,
    /// Hash de la cabeza tras la operación.
    pub content_id: ContentId,
    /// Versión de la cabeza tras la operación.
    pub version: u64,
    /// `true` si el documento no existía y se creó.
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
    /// Igualdad exacta sobre el campo `type` del frontmatter.
    pub doc_type: Option<String>,
    /// Pertenencia exacta en la lista de tags.
    pub tag: Option<String>,
    /// Tope de resultados; siempre acotado además por
    /// `budget.max_search_results`.
    pub limit: Option<usize>,
}

/// Un candidato compacto: lo justo para decidir si merece un `get`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    /// Ruta lógica del documento encontrado.
    pub concept_id: ConceptId,
    /// Hash de su contenido actual.
    pub content_id: ContentId,
    /// Campo `type` del frontmatter.
    pub doc_type: String,
    /// Campo `title` del frontmatter, si existe.
    pub title: Option<String>,
    /// Campo `tags` del frontmatter.
    pub tags: Vec<String>,
}

/// Todo lo que puede salir mal al leer o escribir.
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
///
/// Las garantías de comportamiento no viven solo en esta página:
/// están escritas como tests en el módulo [`contract`], y toda
/// implementación debe pasarlos. Eso es Liskov hecho ejecutable.
pub trait MemoryRepository {
    /// Cabeza actual de `id`, o `None` si el concepto no existe.
    fn get(&self, id: &ConceptId) -> Result<Option<DocumentView>, StoreError>;

    /// Candidatos que cumplen TODOS los criterios de `query`,
    /// nunca más de `budget.max_search_results`.
    fn search(&self, query: &SearchQuery, budget: &Budget) -> Result<Vec<SearchHit>, StoreError>;

    /// Escritura con compare-and-swap: valida el documento, compara
    /// `request.expected` con la cabeza real y solo entonces escribe.
    /// Un rechazo llega como [`StoreError::Conflict`] con los hashes
    /// para releer y reintentar.
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
