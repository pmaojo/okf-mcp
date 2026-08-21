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

use conflict_core::{decide, CommitDecision, Conflict};
use graph_core::NeighborSource;
use hash_core::sha256;
use memory_model::{Budget, ConceptId, ContentId, Principal, Revision};
use okf_core::Link;
use store_core::{
    matches_prefix, Backlink, BulkItem, BulkOutcome, CommitOutcome, CommitRequest, DeleteOutcome,
    DocHint, DocumentView, EmbedOutcome, GraphStats, HeadHintSource, HintedRepository, LinkHealth,
    MemoryRepository, SearchHit, SearchQuery, StoreError, StoreMaintenance, StoreStatus,
    ValidationReport,
};
use std::cell::Cell;
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
    /// Borrado lógico: la cabeza queda enterrada (invisible para
    /// `get`/`search`) pero la historia y la versión sobreviven.
    pub deleted: bool,
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
#[derive(Debug, Clone, Default)]
pub struct InMemoryStore {
    blobs: HashMap<ContentId, Arc<str>>,
    heads: BTreeMap<ConceptId, Head>,
    revisions: Vec<Revision>,
    /// Índice derivado: se reconstruye en cada commit del documento
    /// y se vacía en su borrado (un borrado deja de enlazar).
    links: BTreeMap<ConceptId, Vec<Link>>,
    next_seq: u64,
    /// Triples que `memory_reason` guardó por sujeto — ver
    /// [`store_core::TripleStore`]. Vacío hasta la primera llamada;
    /// no se toca por `commit`/`delete`, así que un concepto
    /// reescrito conserva su última clausura razonada hasta que
    /// alguien vuelva a llamar a `memory_reason` sobre él.
    triples: BTreeMap<ConceptId, Vec<ontology_core::Triple>>,
    /// Contador para [`HeadHintSource::reserve_seq`]: solo existe para
    /// que `IndexedStore<InMemoryStore, InMemoryStore>` (el contrato
    /// de `IndexedStore` en los tests) tenga con qué implementar el
    /// trait — `InMemoryStore` no tiene un backend caro que optimizar,
    /// así que su `head_hint` siempre es `None` y este valor nunca
    /// llega a usarse para decidir un CAS real.
    hint_seq: Cell<u64>,
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
            tags: doc.tags,
            links: doc.links,
        })
    }
}

impl InMemoryStore {
    /// La cabeza VIVA de `id`: `None` si no existe o está borrada.
    fn live_head(&self, id: &ConceptId) -> Option<&Head> {
        self.heads.get(id).filter(|h| !h.deleted)
    }
}

impl MemoryRepository for InMemoryStore {
    fn get(&self, id: &ConceptId) -> Result<Option<DocumentView>, StoreError> {
        match self.live_head(id) {
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
            if head.deleted {
                continue;
            }
            if let Some(prefix) = &query.path_prefix {
                if !matches_prefix(id.as_str(), prefix) {
                    continue;
                }
            }
            let view = self.view(id, head, budget)?;
            if let Some(t) = &query.doc_type {
                if view.doc_type != *t {
                    continue;
                }
            }
            if let Some(t) = &query.exclude_type {
                if view.doc_type == *t {
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

        // 2. Identidad de contenido y decisión CAS pura. Una cabeza
        //    borrada lógicamente cuenta como inexistente para el CAS
        //    (recrear parte de expected = None), pero su versión
        //    sobrevive: la numeración nunca retrocede.
        let incoming = Self::content_id(&request.markdown);
        let head = self.heads.get(&request.concept_id);
        let live = head.filter(|h| !h.deleted);
        let decision = decide(live.map(|h| h.content_id), request.expected, incoming);

        match decision {
            CommitDecision::Conflict(c) => Err(StoreError::Conflict(c)),
            CommitDecision::NoChange => {
                let head = live.expect("NoChange implica cabeza viva");
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
                let base = live.map(|h| h.content_id);
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
                    Head { content_id: incoming, version, deleted: false },
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

    fn delete(
        &mut self,
        id: &ConceptId,
        expected: ContentId,
        actor: &Principal,
        reason: String,
    ) -> Result<DeleteOutcome, StoreError> {
        let head = match self.live_head(id) {
            None => return Err(StoreError::NotFound(id.clone())),
            Some(h) => h.clone(),
        };
        if head.content_id != expected {
            // El mismo contrato CAS que un commit: los hashes reales,
            // para releer y decidir. `incoming` no aplica a un
            // borrado; va el hash que el cliente declaró.
            return Err(StoreError::Conflict(Conflict {
                expected: Some(expected),
                current: Some(head.content_id),
                incoming: expected,
            }));
        }

        // Enterrar la cabeza, vaciar sus enlaces salientes y dejar
        // constancia en la historia — todo con &mut exclusivo, igual
        // de atómico que un commit.
        if let Some(h) = self.heads.get_mut(id) {
            h.deleted = true;
        }
        self.links.remove(id);

        let revision = Revision {
            seq: self.next_seq,
            concept_id: id.clone(),
            base: Some(head.content_id),
            result: head.content_id,
            actor: actor.clone(),
            reason,
        };
        self.next_seq += 1;
        self.revisions.push(revision.clone());

        Ok(DeleteOutcome { content_id: head.content_id, version: head.version, revision })
    }

    fn backlinks(&self, id: &ConceptId) -> Result<Vec<Backlink>, StoreError> {
        // Recorrido lineal del índice: en memoria el grafo cabe
        // entero; en Postgres esto es un índice sobre `target_id`.
        let mut out = Vec::new();
        for (source, links) in &self.links {
            let Some(head) = self.live_head(source) else { continue };
            let Some(link) = links.iter().find(|l| l.target == *id) else { continue };
            let view = self.view(source, head, &Budget::default())?;
            out.push(Backlink {
                source: SearchHit {
                    concept_id: view.concept_id,
                    content_id: view.content_id,
                    doc_type: view.doc_type,
                    title: view.title,
                    tags: view.tags,
                },
                rel: link.rel.clone(),
            });
        }
        Ok(out)
    }

    fn commit_bulk(
        &mut self,
        requests: Vec<CommitRequest>,
        atomic: bool,
        actor: &Principal,
        budget: &Budget,
    ) -> Result<BulkOutcome, StoreError> {
        if !atomic {
            let items = requests
                .into_iter()
                .map(|req| match self.commit(req, actor, budget) {
                    Ok(outcome) => BulkItem::Done(outcome),
                    Err(e) => BulkItem::Failed(e),
                })
                .collect();
            return Ok(BulkOutcome { applied: true, items });
        }

        // Atómico en memoria: aplicar sobre un CLON y quedárselo solo
        // si todo fue bien. El clon es la transacción — barato aquí
        // (los blobs son Arc compartidos), imposible de olvidar hacer
        // rollback.
        let mut speculative = self.clone();
        let mut items = Vec::with_capacity(requests.len());
        let mut failed_at: Option<usize> = None;
        for (idx, req) in requests.into_iter().enumerate() {
            if failed_at.is_some() {
                items.push(BulkItem::Skipped);
                continue;
            }
            match speculative.commit(req, actor, budget) {
                Ok(outcome) => items.push(BulkItem::Done(outcome)),
                Err(e) => {
                    failed_at = Some(idx);
                    items.push(BulkItem::Failed(e));
                }
            }
        }

        match failed_at {
            None => {
                *self = speculative;
                Ok(BulkOutcome { applied: true, items })
            }
            Some(idx) => {
                // Todo o nada: lo aplicado en el clon se descarta y
                // los items que habían ido bien pasan a Skipped.
                for (i, item) in items.iter_mut().enumerate() {
                    if i != idx {
                        *item = BulkItem::Skipped;
                    }
                }
                Ok(BulkOutcome { applied: false, items })
            }
        }
    }
}

/// `InMemoryStore` no tiene un backend caro que optimizar (ver
/// `GithubStore` para el caso real que motiva
/// [`HintedRepository`]/[`HeadHintSource`]): implementa ambos traits
/// de la forma más simple posible — sin pista nunca, delegando siempre
/// en su propio `commit`/`delete` — solo para que
/// `IndexedStore<InMemoryStore, InMemoryStore>` compile y el contrato
/// de `IndexedStore` pueda ejercitarse en los tests sin necesitar
/// GitHub ni Supabase de verdad.
impl HeadHintSource for InMemoryStore {
    fn head_hint(&self, _id: &ConceptId) -> Result<Option<DocHint>, StoreError> {
        Ok(None)
    }

    fn set_write_token(&self, _id: &ConceptId, _token: &str) -> Result<(), StoreError> {
        Ok(())
    }

    fn reserve_seq(&self) -> Result<u64, StoreError> {
        let next = self.hint_seq.get() + 1;
        self.hint_seq.set(next);
        Ok(next)
    }
}

impl HintedRepository for InMemoryStore {
    fn commit_hinted(
        &mut self,
        request: CommitRequest,
        actor: &Principal,
        budget: &Budget,
        _hint: Option<DocHint>,
        _next_seq: u64,
    ) -> Result<(CommitOutcome, Option<String>), StoreError> {
        Ok((self.commit(request, actor, budget)?, None))
    }

    fn delete_hinted(
        &mut self,
        id: &ConceptId,
        expected: ContentId,
        actor: &Principal,
        reason: String,
        _hint: Option<DocHint>,
        _next_seq: u64,
    ) -> Result<DeleteOutcome, StoreError> {
        self.delete(id, expected, actor, reason)
    }
}

impl StoreMaintenance for InMemoryStore {
    fn link_health(&self, id: &ConceptId) -> Result<LinkHealth, StoreError> {
        if self.live_head(id).is_none() {
            return Err(StoreError::NotFound(id.clone()));
        }
        let mut health = LinkHealth::default();
        for link in self.links.get(id).into_iter().flatten() {
            match self.heads.get(&link.target) {
                Some(h) if !h.deleted => health.ok.push(link.clone()),
                Some(_) => health.deleted.push(link.clone()),
                None => health.broken.push(link.clone()),
            }
        }
        Ok(health)
    }

    fn validate(
        &self,
        path_prefix: Option<&str>,
        budget: &Budget,
    ) -> Result<ValidationReport, StoreError> {
        let cap = budget.max_search_results;
        let mut report = ValidationReport::default();
        for (source, links) in &self.links {
            if self.live_head(source).is_none() {
                continue;
            }
            if let Some(prefix) = path_prefix {
                if !matches_prefix(source.as_str(), prefix) {
                    continue;
                }
            }
            for link in links {
                match self.heads.get(&link.target) {
                    Some(h) if !h.deleted => {}
                    Some(_) => {
                        report.deleted_referenced_total += 1;
                        if report.deleted_referenced.len() < cap {
                            report.deleted_referenced.push((source.clone(), link.target.clone()));
                        }
                    }
                    None => {
                        report.broken_links_total += 1;
                        if report.broken_links.len() < cap {
                            report.broken_links.push((source.clone(), link.target.clone()));
                        }
                    }
                }
            }
        }
        // Sin índice semántico no hay embeddings que deber: vacío es
        // la verdad, no un hueco sin implementar.
        Ok(report)
    }

    fn stats(&self, budget: &Budget) -> Result<GraphStats, StoreError> {
        let cap = budget.max_search_results;
        let mut stats = GraphStats::default();
        let mut by_type: BTreeMap<String, usize> = BTreeMap::new();
        let mut by_tag: BTreeMap<String, usize> = BTreeMap::new();
        let mut incoming: BTreeMap<ConceptId, usize> = BTreeMap::new();

        for (id, head) in &self.heads {
            if head.deleted {
                stats.deleted_documents += 1;
                continue;
            }
            stats.documents += 1;
            let view = self.view(id, head, budget)?;
            *by_type.entry(view.doc_type).or_default() += 1;
            for tag in view.tags {
                *by_tag.entry(tag).or_default() += 1;
            }
        }
        for (source, links) in &self.links {
            if self.live_head(source).is_none() {
                continue;
            }
            for link in links {
                *incoming.entry(link.target.clone()).or_default() += 1;
            }
        }

        stats.by_type = sorted_desc(by_type, cap);
        stats.by_tag = sorted_desc(by_tag, cap);
        let mut top: Vec<(ConceptId, usize)> = incoming.clone().into_iter().collect();
        top.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        top.truncate(cap);
        stats.top_linked = top;

        for (id, head) in &self.heads {
            if stats.orphans.len() >= cap {
                break;
            }
            let sin_salientes = self.links.get(id).is_none_or(|l| l.is_empty());
            if !head.deleted && sin_salientes && !incoming.contains_key(id) {
                stats.orphans.push(id.clone());
            }
        }
        Ok(stats)
    }

    fn status(&self) -> Result<StoreStatus, StoreError> {
        let budget = Budget::default();
        let report = self.validate(None, &budget)?;
        let mut status = StoreStatus::default();
        for head in self.heads.values() {
            if head.deleted {
                status.deleted_documents += 1;
            } else {
                status.documents += 1;
            }
        }
        status.broken_links = report.broken_links_total;
        status.deleted_referenced = report.deleted_referenced_total;
        // Embeddings y outbox no existen en este backend: cero es
        // el estado real, no un valor por rellenar.
        Ok(status)
    }

    fn embed_pending(
        &mut self,
        _path_prefix: Option<&str>,
        _max: usize,
    ) -> Result<EmbedOutcome, StoreError> {
        // La búsqueda en memoria es textual: no hay índice semántico
        // que reparar, así que nunca hay trabajo pendiente.
        Ok(EmbedOutcome::default())
    }
}

/// Ordena un recuento de mayor a menor (empates por clave) y corta.
fn sorted_desc(map: BTreeMap<String, usize>, cap: usize) -> Vec<(String, usize)> {
    let mut v: Vec<(String, usize)> = map.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v.truncate(cap);
    v
}

/// SOLID-I: el grafo solo necesita vecinos y tamaños; se los damos
/// sin exponer el resto del repositorio.
impl NeighborSource for InMemoryStore {
    type Error = Infallible;

    fn neighbors(&self, id: &ConceptId) -> Result<Vec<ConceptId>, Infallible> {
        Ok(self
            .links
            .get(id)
            .map(|links| links.iter().map(|l| l.target.clone()).collect())
            .unwrap_or_default())
    }

    fn document_size(&self, id: &ConceptId) -> Result<Option<usize>, Infallible> {
        Ok(self
            .live_head(id)
            .and_then(|h| self.blobs.get(&h.content_id))
            .map(|blob| blob.len()))
    }
}

impl store_core::TripleStore for InMemoryStore {
    fn save_triples(
        &mut self,
        subject: &ConceptId,
        triples: &[ontology_core::Triple],
    ) -> Result<(), StoreError> {
        self.triples.insert(subject.clone(), triples.to_vec());
        Ok(())
    }

    fn load_triples(&self, subject: &ConceptId) -> Result<Vec<ontology_core::Triple>, StoreError> {
        Ok(self.triples.get(subject).cloned().unwrap_or_default())
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
            exclude_type: None,
            tag: tag.map(String::from),
            path_prefix: None,
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
