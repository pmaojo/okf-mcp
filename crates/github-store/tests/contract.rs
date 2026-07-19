//! GithubStore pasa el MISMO contrato que InMemoryStore y
//! SupabaseStore — Liskov ejecutable contra una API de GitHub falsa.
//!
//! El servidor falso implementa, en memoria, el subconjunto de la API
//! REST que usa el adaptador (refs, trees, contents con CAS por blob
//! sha, commits paginados y la API de git data para el lote atómico),
//! con la misma semántica que documenta GitHub. Lo que NO puede
//! verificar es a la API real (este sandbox no tiene salida a
//! api.github.com): ese smoke vive en `examples/smoke.rs` para
//! ejecutarlo fuera.

mod fake_github;

use github_store::GithubStore;

#[test]
fn github_store_cumple_el_contrato() {
    let server = fake_github::spawn();
    store_core::contract::run_all(|| {
        server.reset();
        GithubStore::new(server.base_url(), "owner", "repo", "main", "memoria", None)
    });
}
