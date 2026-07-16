//! La implementación en memoria pasa el contrato de MemoryRepository.
//! El adaptador de Supabase (hito 2) ejecutará EXACTAMENTE esta línea
//! contra Postgres.

#[test]
fn in_memory_cumple_el_contrato() {
    memory_store::contract::run_all(memory_store::InMemoryStore::new);
}
