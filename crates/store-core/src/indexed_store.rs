use crate::{
    Backlink, Budget, BulkOutcome, CommitOutcome, CommitRequest, DeleteOutcome, DocumentView,
    GraphStats, LinkHealth, MemoryRepository, SearchHit, SearchQuery, StoreError, StoreMaintenance,
    StoreStatus, ValidationReport, EmbedOutcome,
};
use memory_model::{ConceptId, ContentId, Principal, Revision};
use graph_core::NeighborSource;

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
        let outcome = self.github.commit(request, actor, budget)?;

        // 2. Sincronizar en el momento con Supabase (best effort)
        if let Err(e) = self.supabase.commit(req_for_supabase, actor, budget) {
            eprintln!(
                "IndexedStore: Best-effort commit to Supabase failed (reconciler will repair): {}",
                e
            );
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
        let outcome = self.github.delete(id, expected, actor, reason.clone())?;

        // 2. Borrar de Supabase (best effort)
        if let Err(e) = self.supabase.delete(id, expected, actor, reason) {
            eprintln!(
                "IndexedStore: Best-effort delete from Supabase failed (reconciler will repair): {}",
                e
            );
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
        let outcome = self.github.commit_bulk(requests, atomic, actor, budget)?;

        // 2. Sincronizar lote con Supabase si se aplicó
        if outcome.applied {
            if let Err(e) = self.supabase.commit_bulk(reqs_for_supabase, atomic, actor, budget) {
                eprintln!(
                    "IndexedStore: Best-effort commit_bulk to Supabase failed: {}",
                    e
                );
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
