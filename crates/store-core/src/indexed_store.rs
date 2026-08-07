use crate::{
    Backlink, Budget, BulkItem, BulkOutcome, CommitOutcome, CommitRequest, DeleteOutcome,
    DocumentView, GraphStats, LinkHealth, MemoryRepository, SearchHit, SearchQuery, StoreError,
    StoreMaintenance, StoreStatus, ValidationReport, EmbedOutcome,
};
use memory_model::{ConceptId, ContentId, Principal, Revision};
use graph_core::NeighborSource;

/// Un mensaje uniforme para los tres puntos de sincronización
/// best-effort de este módulo — el texto es lo que termina en
/// `CommitOutcome::warnings`/`DeleteOutcome::warnings`, así que lo
/// lee un MODELO, no solo un humano mirando logs: dice qué falló Y
/// qué implica para lecturas inmediatas.
fn sync_warning(op: &str, err: &StoreError) -> String {
    format!(
        "el índice de lectura (Supabase) no se sincronizó tras {op} en GitHub (ni siquiera \
         reintentando una vez): {err} — memory_resolve/memory_search pueden seguir mostrando \
         el estado anterior hasta que se repare (reconciliación diaria)"
    )
}

/// Espera fija y corta antes del único reintento — no backoff
/// exponencial, esto no es una cola de reintentos indefinidos. Un
/// blip de red típico (un timeout de conexión, un 5xx pasajero) se
/// resuelve en milisegundos; si el segundo intento TAMBIÉN falla, es
/// una señal de que el problema no es transitorio y seguir
/// reintentando en el camino caliente de la llamada solo la haría
/// más lenta sin arreglar nada — para eso está `outcome.warnings` (el
/// caller se entera YA) y la reconciliación diaria (lo repara
/// después).
const RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(150);

/// Ejecuta `attempt` una vez; si falla, espera [`RETRY_DELAY`] y lo
/// intenta una segunda y última vez. El camino de éxito (la inmensa
/// mayoría de las llamadas) no paga NADA extra: ni el `sleep` ni el
/// segundo intento se ejecutan si el primero ya funcionó.
fn once_with_one_retry<T>(mut attempt: impl FnMut() -> Result<T, StoreError>) -> Result<T, StoreError> {
    match attempt() {
        Ok(v) => Ok(v),
        Err(_first_err) => {
            std::thread::sleep(RETRY_DELAY);
            attempt()
        }
    }
}

/// Adaptador compuesto que delega las escrituras a un almacén de verdad (GitHub)
/// y las lecturas rápidas e índices a un almacén de lectura (Supabase).
///
/// Implementa una arquitectura CQRS donde `G` es el comando principal y `S` es
/// la proyección o índice de lectura.
pub struct IndexedStore<G, S> {
    github: G,
    supabase: S,
}

impl<G, S> IndexedStore<G, S> {
    /// Crea una nueva instancia de `IndexedStore`.
    pub fn new(github: G, supabase: S) -> Self {
        Self { github, supabase }
    }

    /// Obtiene una referencia al almacén de verdad (GitHub).
    pub fn github(&self) -> &G {
        &self.github
    }

    /// Obtiene una referencia al almacén de lectura (Supabase).
    pub fn supabase(&self) -> &S {
        &self.supabase
    }
}

impl<G, S> MemoryRepository for IndexedStore<G, S>
where
    G: MemoryRepository,
    S: MemoryRepository,
{
    fn get(&self, id: &ConceptId) -> Result<Option<DocumentView>, StoreError> {
        // Las lecturas puntuales van a Supabase por rendimiento e indexación
        self.supabase.get(id)
    }

    fn search(&self, query: &SearchQuery, budget: &Budget) -> Result<Vec<SearchHit>, StoreError> {
        // La búsqueda semántica y textual va a Supabase
        self.supabase.search(query, budget)
    }

    fn history(
        &self,
        id: &ConceptId,
        limit: usize,
        before_seq: Option<u64>,
    ) -> Result<Vec<Revision>, StoreError> {
        // Leemos la historia desde GitHub para garantizar la fidelidad absoluta de auditoría
        self.github.history(id, limit, before_seq)
    }

    fn backlinks(&self, id: &ConceptId) -> Result<Vec<Backlink>, StoreError> {
        // Los backlinks se consultan en Supabase
        self.supabase.backlinks(id)
    }

    fn commit(
        &mut self,
        request: CommitRequest,
        actor: &Principal,
        budget: &Budget,
    ) -> Result<CommitOutcome, StoreError> {
        let req_for_supabase = request.clone();

        // 1. Escribir en la fuente de verdad (GitHub)
        let mut outcome = self.github.commit(request, actor, budget)?;

        // 2. Sincronizar en el momento con Supabase (best effort, con
        //    UN reintento — ver `once_with_one_retry`): un fallo aquí
        //    NO deshace el commit (ya está en GitHub, la fuente de
        //    verdad), pero SÍ dice que memory_resolve/memory_search
        //    (que leen del índice) pueden seguir mostrando el estado
        //    anterior. Antes esto solo iba a stderr del servidor ("el
        //    reconciliador lo arreglará") — con una reconciliación
        //    diaria, un caller que confía en el `Ok` de esta llamada
        //    puede pasar hasta 24h sin saber que su escritura no es
        //    visible todavía. Ahora también viaja en
        //    `outcome.warnings`, visible para quien llamó.
        let supabase = &mut self.supabase;
        if let Err(e) =
            once_with_one_retry(|| supabase.commit(req_for_supabase.clone(), actor, budget))
        {
            let warning = sync_warning("commit", &e);
            eprintln!("IndexedStore: {warning}");
            outcome.warnings.push(warning);
        }

        Ok(outcome)
    }

    fn delete(
        &mut self,
        id: &ConceptId,
        expected: ContentId,
        actor: &Principal,
        reason: String,
    ) -> Result<DeleteOutcome, StoreError> {
        // 1. Borrar de la fuente de verdad (GitHub)
        let mut outcome = self.github.delete(id, expected, actor, reason.clone())?;

        // 2. Borrar de Supabase (best effort, con un reintento) — ver
        //    el comentario de `commit` arriba: el fallo se registra en
        //    `outcome.warnings` además de en stderr, para que quien
        //    llamó a `memory_delete` sepa YA que el documento puede
        //    seguir apareciendo en lecturas hasta que se repare, en
        //    vez de asumir que "success" significa "invisible en
        //    todas partes".
        let supabase = &mut self.supabase;
        if let Err(e) =
            once_with_one_retry(|| supabase.delete(id, expected, actor, reason.clone()))
        {
            let warning = sync_warning("delete", &e);
            eprintln!("IndexedStore: {warning}");
            outcome.warnings.push(warning);
        }

        Ok(outcome)
    }

    fn commit_bulk(
        &mut self,
        requests: Vec<CommitRequest>,
        atomic: bool,
        actor: &Principal,
        budget: &Budget,
    ) -> Result<BulkOutcome, StoreError> {
        let reqs_for_supabase = requests.clone();

        // 1. Aplicar lote a GitHub
        let mut outcome = self.github.commit_bulk(requests, atomic, actor, budget)?;

        // 2. Sincronizar lote con Supabase si se aplicó. El fallo es
        //    de TODO el lote (una sola llamada a commit_bulk contra
        //    Supabase), así que se anota en cada item que sí se
        //    aplicó — no hay un lugar a nivel de `BulkOutcome` para
        //    un aviso que no sea "sobre alguno de los items".
        if outcome.applied {
            let supabase = &mut self.supabase;
            if let Err(e) = once_with_one_retry(|| {
                supabase.commit_bulk(reqs_for_supabase.clone(), atomic, actor, budget)
            }) {
                let warning = sync_warning("commit_bulk", &e);
                eprintln!("IndexedStore: {warning}");
                for item in &mut outcome.items {
                    if let BulkItem::Done(done) = item {
                        done.warnings.push(warning.clone());
                    }
                }
            }
        }

        Ok(outcome)
    }
}

impl<G, S> StoreMaintenance for IndexedStore<G, S>
where
    G: StoreMaintenance,
    S: StoreMaintenance,
{
    fn link_health(&self, id: &ConceptId) -> Result<LinkHealth, StoreError> {
        self.supabase.link_health(id)
    }

    fn validate(
        &self,
        path_prefix: Option<&str>,
        budget: &Budget,
    ) -> Result<ValidationReport, StoreError> {
        self.supabase.validate(path_prefix, budget)
    }

    fn stats(&self, budget: &Budget) -> Result<GraphStats, StoreError> {
        self.supabase.stats(budget)
    }

    fn status(&self) -> Result<StoreStatus, StoreError> {
        // Combinamos estados de observabilidad
        let mut status = self.supabase.status()?;
        
        // Si hay discrepancias del outbox en Supabase, las mostramos.
        // Pero en IndexedStore, el outbox_pending de GitHub es 0 (no tiene).
        // Queremos reflejar que los documentos provienen de GitHub:
        if let Ok(github_status) = self.github.status() {
            status.documents = github_status.documents;
            status.deleted_documents = github_status.deleted_documents;
        }
        
        Ok(status)
    }

    fn embed_pending(
        &mut self,
        path_prefix: Option<&str>,
        max: usize,
    ) -> Result<EmbedOutcome, StoreError> {
        // Los embeddings pendientes se procesan en Supabase
        self.supabase.embed_pending(path_prefix, max)
    }
}

impl<G, S> NeighborSource for IndexedStore<G, S>
where
    S: NeighborSource,
{
    type Error = S::Error;

    fn neighbors(&self, id: &ConceptId) -> Result<Vec<ConceptId>, Self::Error> {
        self.supabase.neighbors(id)
    }

    fn document_size(&self, id: &ConceptId) -> Result<Option<usize>, Self::Error> {
        self.supabase.document_size(id)
    }
}

/// Igual que `NeighborSource` arriba: los triples son un índice
/// derivado, no la fuente de verdad — delegan en Supabase, nunca en
/// GitHub.
impl<G, S> crate::TripleStore for IndexedStore<G, S>
where
    S: crate::TripleStore,
{
    fn save_triples(
        &mut self,
        subject: &ConceptId,
        triples: &[ontology_core::Triple],
    ) -> Result<(), StoreError> {
        self.supabase.save_triples(subject, triples)
    }

    fn load_triples(&self, subject: &ConceptId) -> Result<Vec<ontology_core::Triple>, StoreError> {
        self.supabase.load_triples(subject)
    }
}
