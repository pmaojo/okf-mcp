//! `IndexedStore` escribe primero en GitHub (la fuente de verdad) y
//! sincroniza con Supabase (el índice de lectura) best-effort: un
//! fallo ahí no deshace el commit/delete, pero SÍ deja
//! `memory_resolve`/`memory_search` sirviendo el estado anterior
//! hasta que se repare. Antes de este test, ese fallo solo iba a
//! stderr — invisible para quien llamó — y solo se corregía con la
//! reconciliación diaria (`vercel.json`, cron `0 3 * * *`). Estos
//! tests prueban que ahora también viaja en `outcome.warnings`.
//!
//! Los dos backends de `IndexedStore` son `InMemoryStore` — no hace
//! falta GitHub ni Postgres reales para forzar el desacuerdo entre
//! "fuente de verdad" y "índice": basta con escribir directamente en
//! uno de los dos ANTES de envolverlos, para que la sincronización
//! del otro choque con lo que ya había.

use memory_model::{Budget, ConceptId, ContentId, Principal, Revision};
use memory_store::InMemoryStore;
use std::cell::Cell;
use store_core::{
    Backlink, BulkOutcome, CommitOutcome, CommitRequest, DeleteOutcome, DocumentView, IndexedStore,
    MemoryRepository, SearchHit, SearchQuery, StoreError,
};

/// Envoltorio sobre `InMemoryStore` que falla la PRIMERA vez que se
/// llama a `commit`/`delete` y delega normalmente el resto — simula
/// un blip transitorio (un timeout, un 5xx pasajero) sin necesitar
/// una red real. `Cell` porque `commit`/`delete` toman `&mut self` de
/// todos modos, pero así el propio campo no necesita ser `mut` para
/// leerlo/ponerlo desde un método que solo pide `&self` en teoría —
/// simplemente más simple que pelear con el borrow checker en un
/// wrapper de test.
struct FlakyOnce {
    inner: InMemoryStore,
    fail_next: Cell<bool>,
}

impl FlakyOnce {
    fn new(inner: InMemoryStore) -> Self {
        Self { inner, fail_next: Cell::new(true) }
    }
}

impl MemoryRepository for FlakyOnce {
    fn get(&self, id: &ConceptId) -> Result<Option<DocumentView>, StoreError> {
        self.inner.get(id)
    }
    fn search(&self, query: &SearchQuery, budget: &Budget) -> Result<Vec<SearchHit>, StoreError> {
        self.inner.search(query, budget)
    }
    fn history(
        &self,
        id: &ConceptId,
        limit: usize,
        before_seq: Option<u64>,
    ) -> Result<Vec<Revision>, StoreError> {
        self.inner.history(id, limit, before_seq)
    }
    fn backlinks(&self, id: &ConceptId) -> Result<Vec<Backlink>, StoreError> {
        self.inner.backlinks(id)
    }
    fn commit(
        &mut self,
        request: CommitRequest,
        actor: &Principal,
        budget: &Budget,
    ) -> Result<CommitOutcome, StoreError> {
        if self.fail_next.replace(false) {
            return Err(StoreError::Backend("blip transitorio de red".to_string()));
        }
        self.inner.commit(request, actor, budget)
    }
    fn delete(
        &mut self,
        id: &ConceptId,
        expected: ContentId,
        actor: &Principal,
        reason: String,
    ) -> Result<DeleteOutcome, StoreError> {
        if self.fail_next.replace(false) {
            return Err(StoreError::Backend("blip transitorio de red".to_string()));
        }
        self.inner.delete(id, expected, actor, reason)
    }
    fn commit_bulk(
        &mut self,
        requests: Vec<CommitRequest>,
        atomic: bool,
        actor: &Principal,
        budget: &Budget,
    ) -> Result<BulkOutcome, StoreError> {
        self.inner.commit_bulk(requests, atomic, actor, budget)
    }
}

fn id(s: &str) -> ConceptId {
    ConceptId::parse(s).unwrap()
}

fn doc(body: &str) -> String {
    format!("---\ntype: note\n---\n{body}\n")
}

#[test]
fn commit_avisa_si_el_sync_a_supabase_choca_con_un_conflicto() {
    let concept = id("demo/x");

    // Supabase YA tiene el concepto (con otro contenido) antes de que
    // IndexedStore intente sincronizar un `expected: None` — el commit
    // de la fuente de verdad (GitHub) va a "crear"; el intento de la
    // misma request contra Supabase choca con lo que ya había.
    let mut supabase = InMemoryStore::new();
    supabase
        .commit(
            CommitRequest {
                concept_id: concept.clone(),
                expected: None,
                markdown: doc("ya estaba aquí"),
                reason: "seed de supabase".to_string(),
            },
            &Principal::local_dev(),
            &Budget::default(),
        )
        .unwrap();

    let github = InMemoryStore::new();
    let mut store = IndexedStore::new(github, supabase);

    let outcome = store
        .commit(
            CommitRequest {
                concept_id: concept.clone(),
                expected: None,
                markdown: doc("contenido real"),
                reason: "commit real".to_string(),
            },
            &Principal::local_dev(),
            &Budget::default(),
        )
        .expect("GitHub (la fuente de verdad) acepta el commit igual");

    assert!(
        !outcome.warnings.is_empty(),
        "un fallo de sync a Supabase debe quedar en outcome.warnings, no solo en stderr"
    );
    assert!(outcome.warnings[0].contains("Supabase"));

    // Y el índice de lectura, en efecto, sigue sirviendo el contenido
    // viejo — exactamente el síntoma que motivó este fix.
    let stale = store.get(&concept).unwrap().unwrap();
    assert!(stale.raw.contains("ya estaba aquí"));
}

#[test]
fn delete_avisa_si_supabase_nunca_tuvo_el_concepto() {
    let concept = id("demo/solo-en-github");

    let mut github = InMemoryStore::new();
    let outcome = github
        .commit(
            CommitRequest {
                concept_id: concept.clone(),
                expected: None,
                markdown: doc("contenido"),
                reason: "seed".to_string(),
            },
            &Principal::local_dev(),
            &Budget::default(),
        )
        .unwrap();
    let content_id: ContentId = outcome.content_id;

    // Supabase nunca vio este concepto — como si su sync de creación
    // ya hubiera fallado antes.
    let supabase = InMemoryStore::new();
    let mut store = IndexedStore::new(github, supabase);

    let outcome = store
        .delete(&concept, content_id, &Principal::local_dev(), "borrado".to_string())
        .expect("GitHub sí lo tiene y acepta el borrado");

    assert!(
        !outcome.warnings.is_empty(),
        "borrar algo que Supabase nunca tuvo debe avisar, no fallar en silencio"
    );
}

#[test]
fn commit_sin_desacuerdo_no_genera_avisos() {
    let concept = id("demo/limpio");
    let store = IndexedStore::new(InMemoryStore::new(), InMemoryStore::new());
    let mut store = store;

    let outcome = store
        .commit(
            CommitRequest {
                concept_id: concept,
                expected: None,
                markdown: doc("todo en orden"),
                reason: "commit limpio".to_string(),
            },
            &Principal::local_dev(),
            &Budget::default(),
        )
        .unwrap();

    assert!(outcome.warnings.is_empty());
}

#[test]
fn commit_se_recupera_solo_de_un_blip_transitorio_sin_generar_aviso() {
    let concept = id("demo/flaky-commit");
    let github = InMemoryStore::new();
    let supabase = FlakyOnce::new(InMemoryStore::new());
    let mut store = IndexedStore::new(github, supabase);

    let outcome = store
        .commit(
            CommitRequest {
                concept_id: concept.clone(),
                expected: None,
                markdown: doc("contenido"),
                reason: "seed".to_string(),
            },
            &Principal::local_dev(),
            &Budget::default(),
        )
        .unwrap();

    assert!(
        outcome.warnings.is_empty(),
        "un blip que se resuelve en el reintento no debe dejar aviso — se resolvió solo"
    );
    // Y el índice SÍ tiene el dato — no es solo que no avisó, es que
    // el segundo intento realmente escribió.
    assert!(store.get(&concept).unwrap().is_some());
}

#[test]
fn delete_se_recupera_solo_de_un_blip_transitorio_sin_generar_aviso() {
    let concept = id("demo/flaky-delete");
    let mut github = InMemoryStore::new();
    let outcome = github
        .commit(
            CommitRequest {
                concept_id: concept.clone(),
                expected: None,
                markdown: doc("contenido"),
                reason: "seed".to_string(),
            },
            &Principal::local_dev(),
            &Budget::default(),
        )
        .unwrap();
    let content_id = outcome.content_id;

    let mut supabase_inner = InMemoryStore::new();
    supabase_inner
        .commit(
            CommitRequest {
                concept_id: concept.clone(),
                expected: None,
                markdown: doc("contenido"),
                reason: "seed en supabase".to_string(),
            },
            &Principal::local_dev(),
            &Budget::default(),
        )
        .unwrap();
    let supabase = FlakyOnce::new(supabase_inner);
    let mut store = IndexedStore::new(github, supabase);

    let outcome = store
        .delete(&concept, content_id, &Principal::local_dev(), "borrado".to_string())
        .unwrap();

    assert!(outcome.warnings.is_empty());
    assert!(store.get(&concept).unwrap().is_none(), "el segundo intento sí debe haber borrado");
}
