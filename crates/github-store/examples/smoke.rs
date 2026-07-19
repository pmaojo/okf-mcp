//! Smoke del GithubStore contra la API REAL de GitHub — el sandbox de
//! desarrollo no tiene salida a api.github.com, así que este binario
//! existe para ejecutarlo en una máquina con red:
//!
//! ```bash
//! GITHUB_TOKEN=ghp_xxx cargo run -p github-store --example smoke -- owner/repo main memoria
//! ```
//!
//! ADVERTENCIA: escribe de verdad en el repo indicado (crea, edita y
//! borra `memoria/smoke/prototipo.md` y deja commits). Usa un repo de
//! pruebas.

use github_store::GithubStore;
use memory_model::{Budget, ConceptId, Principal};
use store_core::{CommitRequest, MemoryRepository, StoreMaintenance};

fn main() {
    let mut args = std::env::args().skip(1);
    let repo_arg = args.next().expect("uso: smoke owner/repo [rama] [base_path]");
    let branch = args.next().unwrap_or_else(|| "main".to_string());
    let base_path = args.next().unwrap_or_else(|| "memoria".to_string());
    let (owner, repo) = repo_arg.split_once('/').expect("owner/repo");
    let token = std::env::var("GITHUB_TOKEN").ok();

    let mut store =
        GithubStore::new("https://api.github.com", owner, repo, branch, base_path, token);
    let id = ConceptId::parse("smoke/prototipo").unwrap();
    let actor = Principal::local_dev();
    let budget = Budget::default();
    let doc = |body: &str| format!("---\ntype: note\ntitle: Smoke\n---\n{body}\n");

    let t0 = std::time::Instant::now();
    let v1 = store
        .commit(
            CommitRequest {
                concept_id: id.clone(),
                expected: None,
                markdown: doc("primera versión"),
                reason: "smoke: crear".to_string(),
            },
            &actor,
            &budget,
        )
        .expect("crear");
    println!("crear: v{} {} ({:?})", v1.version, v1.content_id.to_hex(), t0.elapsed());

    let t = std::time::Instant::now();
    let view = store.get(&id).expect("get").expect("existe");
    println!("get: v{} {} bytes ({:?})", view.version, view.raw.len(), t.elapsed());

    let t = std::time::Instant::now();
    let v2 = store
        .commit(
            CommitRequest {
                concept_id: id.clone(),
                expected: Some(v1.content_id),
                markdown: doc("segunda versión"),
                reason: "smoke: editar".to_string(),
            },
            &actor,
            &budget,
        )
        .expect("editar");
    println!("editar: v{} ({:?})", v2.version, t.elapsed());

    // CAS: la base obsoleta debe rechazarse.
    let conflicto = store.commit(
        CommitRequest {
            concept_id: id.clone(),
            expected: Some(v1.content_id),
            markdown: doc("pisotón"),
            reason: "smoke: conflicto".to_string(),
        },
        &actor,
        &budget,
    );
    println!("conflicto esperado: {:?}", conflicto.err().map(|e| e.to_string()));

    let t = std::time::Instant::now();
    let historia = store.history(&id, 10, None).expect("historia");
    println!("historia: {} revisiones ({:?})", historia.len(), t.elapsed());
    for r in &historia {
        println!("  seq {} — {}", r.seq, r.reason);
    }

    let t = std::time::Instant::now();
    let status = store.status().expect("status");
    println!(
        "status: {} docs, {} borrados ({:?})",
        status.documents, status.deleted_documents, t.elapsed()
    );

    let t = std::time::Instant::now();
    store
        .delete(&id, v2.content_id, &actor, "smoke: limpiar".to_string())
        .expect("borrar");
    println!("borrar: ok ({:?})", t.elapsed());
    println!("total: {:?}", t0.elapsed());
}
