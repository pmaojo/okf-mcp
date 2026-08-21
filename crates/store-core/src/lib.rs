//! Contrato y tipos base de persistencia de documentos okf-mcp.
//!
//! SOLID en juego:
//! - **L (sustitución de Liskov):** [`MemoryRepository`] es el contrato abstracto.
//!   Tanto `InMemoryStore` como `SupabaseStore` deben comportarse de forma idéntica
//!   para cualquier consumidor, validado por los tests en `contract`.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod contract;
/// Adaptador compuesto para CQRS / Almacenamiento indexado.
pub mod indexed_store;

pub use indexed_store::IndexedStore;

use conflict_core::Conflict;
use memory_model::{Budget, ConceptId, ContentId, Principal, Revision};
use okf_core::{Link, OkfError};
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
    /// Enlaces `[[...]]` salientes (con relación tipada opcional),
    /// ya validados.
    pub links: Vec<Link>,
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
    /// Texto libre. El CONTRATO solo exige que las coincidencias de
    /// subcadena (sin distinción de mayúsculas, sobre id, título,
    /// tags y cuerpo) aparezcan en el resultado; un backend con
    /// índice semántico puede además devolver documentos
    /// semánticamente próximos, detrás de las coincidencias exactas.
    pub text: Option<String>,
    /// Igualdad exacta sobre el campo `type` del frontmatter.
    pub doc_type: Option<String>,
    /// Excluye los documentos cuyo `type` sea EXACTAMENTE este valor
    /// (lo contrario de `doc_type`). Compatible con `not type:X` sin
    /// necesitar post-filtrado del lado del agente — p. ej. buscar
    /// todo menos `type: task`. Se combina con AND igual que el
    /// resto: `doc_type` y `exclude_type` a la vez son válidos (y, si
    /// coinciden, el resultado siempre está vacío).
    pub exclude_type: Option<String>,
    /// Pertenencia exacta en la lista de tags.
    pub tag: Option<String>,
    /// Prefijo de ruta lógica, por segmentos completos: `people`
    /// acepta `people/alice` pero no `peoples/x` — ver
    /// [`matches_prefix`].
    pub path_prefix: Option<String>,
    /// Tope de resultados; siempre acotado además por
    /// `budget.max_search_results`.
    pub limit: Option<usize>,
}

/// ¿`id` cae bajo `prefix`, contando por segmentos completos?
///
/// ```
/// use store_core::matches_prefix;
/// assert!(matches_prefix("people/alice", "people"));
/// assert!(matches_prefix("people", "people"));
/// assert!(!matches_prefix("peoples/alice", "people"));
/// ```
pub fn matches_prefix(id: &str, prefix: &str) -> bool {
    id == prefix || (id.starts_with(prefix) && id.as_bytes().get(prefix.len()) == Some(&b'/'))
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

/// Resultado de un borrado lógico aceptado.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteOutcome {
    /// Hash del contenido que quedó enterrado (el que declaró el
    /// cliente como `expected`).
    pub content_id: ContentId,
    /// Versión de la cabeza en el momento del borrado.
    pub version: u64,
    /// La revisión que deja constancia del borrado en la historia.
    pub revision: Revision,
}

/// Un enlace entrante: quién apunta a un concepto y con qué relación.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backlink {
    /// El documento origen del enlace, en forma compacta.
    pub source: SearchHit,
    /// Relación tipada del enlace (`[[rel:destino]]`), si la hay.
    pub rel: Option<String>,
}

/// Resultado de un item dentro de [`MemoryRepository::commit_bulk`].
#[derive(Debug, Clone)]
pub enum BulkItem {
    /// El commit se aplicó (o era `no_change`).
    Done(CommitOutcome),
    /// El commit falló por su propia causa (CAS, validación…).
    Failed(StoreError),
    /// No se aplicó: en modo atómico, otro item del lote falló.
    Skipped,
}

/// Resultado de un lote de commits.
#[derive(Debug, Clone)]
pub struct BulkOutcome {
    /// `true` si el lote (o parte de él, en modo no atómico) quedó
    /// persistido; `false` si en modo atómico se revirtió todo.
    pub applied: bool,
    /// Un resultado por petición, en el mismo orden de entrada.
    pub items: Vec<BulkItem>,
}

/// Salud de los enlaces salientes de UN documento.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LinkHealth {
    /// Enlaces cuyo destino existe y está vivo.
    pub ok: Vec<Link>,
    /// Enlaces cuyo destino no existe (ni existió).
    pub broken: Vec<Link>,
    /// Enlaces cuyo destino existe pero está borrado lógicamente.
    pub deleted: Vec<Link>,
}

/// Informe de validación del grafo (o de un subárbol).
///
/// Las listas están acotadas por `budget.max_search_results`; los
/// contadores `*_total` dicen cuántos problemas hay EN REALIDAD,
/// para que un informe truncado nunca parezca un informe limpio.
#[derive(Debug, Clone, Default)]
pub struct ValidationReport {
    /// Pares (origen, destino) con destino inexistente.
    pub broken_links: Vec<(ConceptId, ConceptId)>,
    /// Total real de enlaces rotos (puede superar a la lista).
    pub broken_links_total: usize,
    /// Pares (origen, destino) con destino borrado lógicamente.
    pub deleted_referenced: Vec<(ConceptId, ConceptId)>,
    /// Total real de referencias a borrados.
    pub deleted_referenced_total: usize,
    /// Documentos vivos que el índice semántico del backend aún no
    /// cubre. Backend-específico: un almacén sin índice semántico
    /// (p. ej. en memoria) devuelve siempre vacío, honestamente.
    pub missing_embeddings: Vec<ConceptId>,
    /// Total real de documentos sin embedding al día.
    pub missing_embeddings_total: usize,
}

/// Métricas del grafo de conocimiento.
#[derive(Debug, Clone, Default)]
pub struct GraphStats {
    /// Documentos vivos (excluye borrados lógicos).
    pub documents: usize,
    /// Documentos borrados lógicamente que conservan historia.
    pub deleted_documents: usize,
    /// Recuento por campo `type`, de mayor a menor.
    pub by_type: Vec<(String, usize)>,
    /// Recuento por tag, de mayor a menor.
    pub by_tag: Vec<(String, usize)>,
    /// Conceptos con más enlaces entrantes (hubs), de mayor a menor.
    pub top_linked: Vec<(ConceptId, usize)>,
    /// Documentos vivos sin enlaces entrantes ni salientes.
    pub orphans: Vec<ConceptId>,
}

/// Estado de salud operativo del almacén, para observabilidad.
///
/// Los campos de embeddings y outbox son backend-específicos: un
/// almacén sin índice semántico ni outbox (en memoria) devuelve
/// ceros, y eso también es información veraz.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StoreStatus {
    /// Documentos vivos.
    pub documents: usize,
    /// Borrados lógicos que conservan historia.
    pub deleted_documents: usize,
    /// Documentos vivos cuyo embedding falta o no corresponde al
    /// contenido actual.
    pub missing_embeddings: usize,
    /// Enlaces cuyo destino no existe.
    pub broken_links: usize,
    /// Enlaces cuyo destino está borrado lógicamente.
    pub deleted_referenced: usize,
    /// Eventos del outbox aún pendientes de procesar.
    pub outbox_pending: usize,
    /// Eventos del outbox agotados tras varios reintentos.
    pub outbox_failed: usize,
}

/// Resultado de forzar la indexación semántica ([`StoreMaintenance::embed_pending`]).
#[derive(Debug, Clone, Default)]
pub struct EmbedOutcome {
    /// Conceptos indexados en esta llamada.
    pub embedded: Vec<ConceptId>,
    /// Conceptos que fallaron, con el motivo.
    pub failed: Vec<(ConceptId, String)>,
    /// Cuántos siguen pendientes tras esta llamada (el lote está
    /// acotado; repite la llamada para continuar).
    pub remaining: usize,
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
    /// nunca más de `budget.max_search_results`. Los documentos
    /// borrados lógicamente (ver [`MemoryRepository::delete`]) quedan
    /// SIEMPRE excluidos, sin que el llamador tenga que pedirlo — es
    /// responsabilidad del backend, no del agente ni del cliente MCP.
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
    /// Un concepto borrado lógicamente CONSERVA su historia.
    fn history(
        &self,
        id: &ConceptId,
        limit: usize,
        before_seq: Option<u64>,
    ) -> Result<Vec<Revision>, StoreError>;

    /// Borrado lógico con compare-and-swap: `expected` es
    /// OBLIGATORIO — borrar exige haber leído lo que se borra.
    ///
    /// Tras un borrado aceptado:
    /// - `get` devuelve `None` y `search` deja de listarlo;
    /// - sus enlaces salientes desaparecen del grafo;
    /// - su historia sigue disponible, con una revisión que registra
    ///   el borrado;
    /// - un `commit` posterior con `expected = None` lo RECREA,
    ///   continuando la numeración de versiones (la historia es una).
    ///
    /// Un concepto inexistente (o ya borrado) es
    /// [`StoreError::NotFound`]; una base obsoleta es
    /// [`StoreError::Conflict`] con los hashes para releer.
    fn delete(
        &mut self,
        id: &ConceptId,
        expected: ContentId,
        actor: &Principal,
        reason: String,
    ) -> Result<DeleteOutcome, StoreError>;

    /// Vecindario ENTRANTE: qué documentos vivos enlazan a `id`, con
    /// su relación tipada si la declararon. `id` no tiene que
    /// existir — preguntar "¿quién apunta aquí?" antes de crear (o
    /// después de borrar) es legítimo. Orden: por `concept_id` del
    /// origen.
    fn backlinks(&self, id: &ConceptId) -> Result<Vec<Backlink>, StoreError>;

    /// Lote de commits en orden. Con `atomic = false` cada item se
    /// aplica (o falla) por su cuenta y los posteriores ven los
    /// efectos de los anteriores. Con `atomic = true` o se aplican
    /// TODOS o ninguno: al primer fallo se revierte el lote entero,
    /// el item culpable queda [`BulkItem::Failed`] y el resto
    /// [`BulkItem::Skipped`].
    ///
    /// `Err` global solo por fallos del backend; los fallos por item
    /// van dentro de [`BulkOutcome::items`].
    fn commit_bulk(
        &mut self,
        requests: Vec<CommitRequest>,
        atomic: bool,
        actor: &Principal,
        budget: &Budget,
    ) -> Result<BulkOutcome, StoreError>;
}

/// Estado mínimo de un documento tal y como lo ve el índice de
/// lectura (Supabase en `IndexedStore`), para que el backend de
/// escritura pueda decidir un commit/delete sin releerse el repo
/// entero. Cualquier campo que el índice no pueda garantizar al día
/// se traduce en `None` desde [`HeadHintSource::head_hint`] — nunca en
/// un valor adivinado.
#[derive(Debug, Clone)]
pub struct DocHint {
    /// Hash de la cabeza viva; `None` si el concepto no existe o está
    /// borrado lógicamente.
    pub live_content_id: Option<ContentId>,
    /// Última versión conocida (0 si el concepto nunca existió).
    pub version: u64,
    /// Token CAS de la escritura subyacente (p. ej. el blob sha de
    /// git); `None` si no hay cabeza viva, o si el índice nunca llegó
    /// a capturarlo.
    pub file_sha: Option<String>,
}

/// Camino barato de escritura para un backend cuyo `commit`/`delete`
/// normales (los de [`MemoryRepository`]) tienen un coste de red que
/// crece con el tamaño del repositorio entero (el caso de
/// `GithubStore`, que sin esto tendría que recorrerse todo el árbol y
/// el historial de commits en cada escritura). Con la pista de un
/// índice de lectura ya sincronizado ([`DocHint`]), el backend decide
/// el compare-and-swap leyendo solo la cabeza — y recurre al camino
/// completo de [`MemoryRepository`] cuando `hint` es `None`, cuando la
/// pista resulta obsoleta (la rama avanzó por debajo) o cuando no
/// puede derivar de forma barata lo que le falta.
///
/// `next_seq` (reservado por el llamador con
/// [`HeadHintSource::reserve_seq`], NUNCA calculado aquí) es lo que
/// hace seguro saltarse la relectura del historial: dos escrituras
/// concurrentes por este camino no leen nada que las coordine entre
/// sí, así que sin un contador compartido y atómico ambas podrían
/// derivar el mismo número de forma independiente y corromper el
/// orden que asume la reconstrucción de versiones a partir de los
/// trailers `Memory-Rev:`. Un backend que además tiene que recurrir a
/// su historial (porque la pista falta o resultó obsoleta) trata
/// `next_seq` como un SUELO, nunca como el valor final: nunca escribe
/// una revisión por debajo de lo que su propio historial ya muestra,
/// así que sigue siendo correcto incluso con una reserva
/// desactualizada.
pub trait HintedRepository {
    /// Igual que [`MemoryRepository::commit`], pero además de la
    /// salida habitual devuelve el nuevo token CAS de la escritura
    /// (p. ej. el blob sha) cuando el commit cambió el archivo —
    /// `None` si no hubo escritura o si se recurrió al camino
    /// completo. El llamador ([`indexed_store::IndexedStore`]) lo
    /// persiste en el índice para que el PRÓXIMO commit pueda volver a
    /// tomar este camino barato.
    fn commit_hinted(
        &mut self,
        request: CommitRequest,
        actor: &Principal,
        budget: &Budget,
        hint: Option<DocHint>,
        next_seq: u64,
    ) -> Result<(CommitOutcome, Option<String>), StoreError>;

    /// Igual que [`MemoryRepository::delete`], con la misma pista, el
    /// mismo `next_seq` reservado y el mismo criterio de cuándo
    /// recurrir al camino completo.
    fn delete_hinted(
        &mut self,
        id: &ConceptId,
        expected: ContentId,
        actor: &Principal,
        reason: String,
        hint: Option<DocHint>,
        next_seq: u64,
    ) -> Result<DeleteOutcome, StoreError>;
}

/// Fuente de pistas para [`HintedRepository`]: el índice de lectura
/// que `IndexedStore` ya mantiene sincronizado con cada escritura
/// (best-effort) y que por tanto puede responder por la cabeza de un
/// concepto, y coordinar la numeración de revisiones, sin tocar el
/// backend de escritura.
pub trait HeadHintSource {
    /// La pista vigente para `id`, o `None` si el índice no tiene fila
    /// para ese concepto todavía (primera escritura, o el índice nunca
    /// llegó a sincronizarla) — la señal para que el backend de
    /// escritura recurra al camino completo.
    fn head_hint(&self, id: &ConceptId) -> Result<Option<DocHint>, StoreError>;

    /// Persiste el nuevo token CAS de una escritura ya confirmada por
    /// el backend de escritura, para que el PRÓXIMO `head_hint` pueda
    /// ofrecerlo.
    fn set_write_token(&self, id: &ConceptId, token: &str) -> Result<(), StoreError>;

    /// Reserva ATÓMICAMENTE el próximo número de una secuencia global
    /// compartida por todos los escritores que pasan por este índice
    /// (p. ej. una `SEQUENCE` de Postgres) — ver [`HintedRepository`]
    /// para por qué esto, y no un cálculo local, es lo que hace seguro
    /// el camino barato bajo concurrencia. Los huecos (números
    /// reservados que terminan sin usarse, p. ej. por un commit que
    /// resultó `NoChange`) son inofensivos: lo único que importa es
    /// que dos llamadas nunca devuelvan el mismo número.
    fn reserve_seq(&self) -> Result<u64, StoreError>;
}

/// Mantenimiento y observabilidad del almacén: validación del grafo,
/// métricas, estado de salud e indexación semántica bajo demanda.
///
/// Trait SEPARADO de [`MemoryRepository`] a conciencia (SOLID-I): el
/// ciclo de vida de los documentos y el diagnóstico del sistema son
/// clientes distintos. Un consumidor que solo lee y escribe no
/// debería arrastrar la superficie de mantenimiento, ni al revés.
pub trait StoreMaintenance {
    /// Clasifica los enlaces salientes de `id` según la salud de su
    /// destino: vivo, inexistente o borrado lógicamente.
    /// [`StoreError::NotFound`] si `id` no existe o está borrado.
    fn link_health(&self, id: &ConceptId) -> Result<LinkHealth, StoreError>;

    /// Valida el grafo completo (o el subárbol bajo `path_prefix`,
    /// por segmentos — ver [`matches_prefix`]). Las listas del
    /// informe se acotan con `budget.max_search_results`; los
    /// totales reales van aparte para que el truncado sea visible.
    fn validate(
        &self,
        path_prefix: Option<&str>,
        budget: &Budget,
    ) -> Result<ValidationReport, StoreError>;

    /// Métricas del grafo: recuentos por tipo y tag, hubs y
    /// huérfanos. Las listas se acotan con
    /// `budget.max_search_results`.
    fn stats(&self, budget: &Budget) -> Result<GraphStats, StoreError>;

    /// Estado operativo en tiempo real: documentos, embeddings
    /// pendientes, enlaces rotos, outbox. Pensado para responder
    /// "¿está sano el sistema?" en una sola llamada barata.
    fn status(&self) -> Result<StoreStatus, StoreError>;

    /// Indexa AHORA los documentos (bajo `path_prefix`, o todos)
    /// cuyo embedding falta o quedó obsoleto, hasta `max` por
    /// llamada. Un backend sin índice semántico devuelve el
    /// resultado vacío: no hay nada que esa búsqueda no cubra ya.
    fn embed_pending(
        &mut self,
        path_prefix: Option<&str>,
        max: usize,
    ) -> Result<EmbedOutcome, StoreError>;
}

/// Persistencia de los triples que `ontology-core` deriva
/// (`crates/ontology-core`, capítulo 18 del tutorial) — asertados
/// (extraídos del frontmatter/enlaces) y derivados (por
/// `ontology_core::materialize`) por igual.
///
/// Trait SEPARADO de [`MemoryRepository`] a conciencia, igual que
/// [`StoreMaintenance`] (SOLID-I): razonar sobre un concepto es un
/// cliente distinto de leerlo o escribirlo, y un consumidor que solo
/// hace CRUD de documentos no debería arrastrar esta superficie.
///
/// `save_triples` REEMPLAZA todos los triples conocidos de `subject`
/// — no los acumula — porque cada llamada a `memory_reason`
/// materializa la clausura completa para ese sujeto; guardar la unión
/// con la ejecución anterior resucitaría hechos que una ontología más
/// reciente ya no implica.
pub trait TripleStore {
    /// Reemplaza los triples almacenados de `subject` por `triples`.
    fn save_triples(
        &mut self,
        subject: &ConceptId,
        triples: &[ontology_core::Triple],
    ) -> Result<(), StoreError>;

    /// Los triples conocidos cuyo sujeto es `subject`, o vacío si
    /// nunca se razonó sobre él (no es un error: es normal que la
    /// mayoría de conceptos nunca hayan pasado por `memory_reason`).
    fn load_triples(&self, subject: &ConceptId) -> Result<Vec<ontology_core::Triple>, StoreError>;
}
