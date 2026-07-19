//! La implementación en memoria pasa el contrato de MemoryRepository.
//! El adaptador de Supabase (hito 2) ejecutará EXACTAMENTE esta línea
//! contra Postgres.

#[test]
fn in_memory_cumple_el_contrato() {
    store_core::contract::run_all(memory_store::InMemoryStore::new);
}

#[test]
fn indexed_store_cumple_el_contrato() {
    store_core::contract::run_all(|| {
        store_core::IndexedStore::new(
            memory_store::InMemoryStore::new(),
            memory_store::InMemoryStore::new(),
        )
    });
}
