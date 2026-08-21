use crate::{
    Backlink, Budget, BulkOutcome, CommitOutcome, CommitRequest, DeleteOutcome, DocumentView,
    GraphStats, HeadHintSource, HintedRepository, LinkHealth, MemoryRepository, SearchHit,
    SearchQuery, StoreError, StoreMaintenance, StoreStatus, ValidationReport, EmbedOutcome,
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
    G: MemoryRepository + HintedRepository,
    S: MemoryRepository + HeadHintSource,
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
        let concept_id = request.concept_id.clone();
        let req_for_supabase = request.clone();

        // 1. Escribir en la fuente de verdad (GitHub). El camino
        // barato (HintedRepository) exige DOS cosas del índice de
        // lectura antes de intentarlo:
        //   - la cabeza conocida (`head_hint`), para decidir el CAS
        //     sin recorrerse el repo entero;
        //   - un `next_seq` reservado ATÓMICAMENTE (`reserve_seq`),
        //     para que dos escrituras concurrentes por este camino no
        //     deriven el mismo número por separado (el camino barato,
        //     a propósito, no relee el historial para coordinarse).
        // Sin una reserva (Supabase caído) no hay forma segura de
        // coordinar esa numeración: se cae al `commit` autosuficiente
        // de GitHub, que se calcula su propio `seq` recorriendo su
        // historial.
        let (outcome, new_token) = match self.supabase.reserve_seq() {
            Ok(next_seq) => {
                let hint = self.supabase.head_hint(&concept_id).unwrap_or(None);
                self.github.commit_hinted(request, actor, budget, hint, next_seq)?
            }
            Err(_) => (self.github.commit(request, actor, budget)?, None),
        };

        // 2. Sincronizar en el momento con Supabase (best effort)
        match self.supabase.commit(req_for_supabase, actor, budget) {
            Err(e) => eprintln!(
                "IndexedStore: Best-effort commit to Supabase failed (reconciler will repair): {}",
                e
            ),
            Ok(_) => {
                if let Some(token) = new_token {
                    if let Err(e) = self.supabase.set_write_token(&concept_id, &token) {
                        eprintln!(
                            "IndexedStore: no se pudo guardar el token CAS en Supabase (el reconciler lo reparará): {}",
                            e
                        );
                    }
                }
            }
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
        // 1. Borrar de la fuente de verdad (GitHub) — misma lógica de
        // pista + reserva que en `commit`.
        let outcome = match self.supabase.reserve_seq() {
            Ok(next_seq) => {
                let hint = self.supabase.head_hint(id).unwrap_or(None);
                self.github.delete_hinted(id, expected, actor, reason.clone(), hint, next_seq)?
            }
            Err(_) => self.github.delete(id, expected, actor, reason.clone())?,
        };

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
        // Supabase es el índice que IndexedStore mantiene sincronizado
        // con cada escritura: sus recuentos de documentos ya reflejan
        // el estado de GitHub sin tener que preguntarle (eso costaría
        // recorrerse el repo entero por cada ping — ver GithubStore::snapshot).
        self.supabase.status()
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
