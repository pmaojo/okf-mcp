//! El contrato de [`MemoryRepository`], como tests ejecutables.
//!
//! Liskov ejecutable: estas funciones prueban COMPORTAMIENTO usando
//! solo el trait. `InMemoryStore` (hito 1) y `SupabaseStore` (hito 2)
//! deben ser indistinguibles para quien use el trait: mismos
//! errores, mismas garantías CAS. Si ambas implementaciones
//! pasan esta suite, son sustituibles; si el contrato necesita un
//! test que el trait no permite escribir, eso es un detalle de
//! implementación y NO forma parte del contrato.
//!
//! Uso desde los tests de cualquier implementación:
//!
//! ```rust,ignore
//! use store_core::contract;
//! use memory_store::InMemoryStore;
//! contract::run_all(InMemoryStore::new);
//! ```
//!
//! (El ejemplo lleva `ignore` a conciencia: `memory-store` DEPENDE
//! de este crate, no al revés, así que un doctest de aquí no puede
//! importarlo sin crear un ciclo. La versión ejecutable vive en los
//! tests de cada implementación — véase
//! `crates/memory-store/tests/contract.rs`.)

use crate::{CommitRequest, MemoryRepository, SearchQuery, StoreError, TagsMode};
use memory_model::{Budget, ConceptId, ContentId, Principal};

fn id(s: &str) -> ConceptId {
    ConceptId::parse(s).expect("id de test válido")
}

fn doc(title: &str, body: &str) -> String {
    format!("---\ntype: note\ntitle: {title}\n---\n{body}\n")
}

fn commit<R: MemoryRepository>(
    repo: &mut R,
    concept: &str,
    expected: Option<ContentId>,
    markdown: &str,
) -> Result<crate::CommitOutcome, StoreError> {
    repo.commit(
        CommitRequest {
            concept_id: id(concept),
            expected,
            markdown: markdown.to_string(),
            reason: "contrato".to_string(),
        },
        &Principal::local_dev(),
        &Budget::default(),
    )
}

/// Ejecuta el contrato completo. `mk` fabrica un repositorio VACÍO;
/// se llama una vez por propiedad para que los tests no se contaminen.
pub fn run_all<R: MemoryRepository>(mut mk: impl FnMut() -> R) {
    crear_leer_y_versionar(mk());
    base_obsoleta_no_pisa(mk());
    commit_identico_es_idempotente(mk());
    documento_invalido_no_deja_rastro(mk());
    inexistente_es_none_y_notfound(mk());
    busqueda_respeta_filtros_y_limite(mk());
    historia_reciente_primero_y_paginada(mk());
}

/// Crear parte en versión 1; `get` devuelve los bytes escritos;
/// actualizar sobre la base correcta incrementa la versión.
pub fn crear_leer_y_versionar<R: MemoryRepository>(mut repo: R) {
    let v1 = commit(&mut repo, "c/uno", None, &doc("Uno", "v1")).expect("crear");
    assert!(v1.created && v1.version == 1, "la creación es versión 1");

    let view = repo.get(&id("c/uno")).expect("get").expect("existe");
    assert_eq!(view.content_id, v1.content_id, "get devuelve la cabeza");
    assert!(view.raw.contains("v1"), "los bytes son los escritos");

    let v2 = commit(&mut repo, "c/uno", Some(v1.content_id), &doc("Uno", "v2")).expect("update");
    assert!(!v2.created && v2.version == 2, "la actualización incrementa la versión");
}

/// Una base obsoleta produce [`StoreError::Conflict`] con los hashes
/// correctos y NO altera la escritura más nueva.
pub fn base_obsoleta_no_pisa<R: MemoryRepository>(mut repo: R) {
    let v1 = commit(&mut repo, "c", None, &doc("t", "v1")).unwrap();
    let v2 = commit(&mut repo, "c", Some(v1.content_id), &doc("t", "v2")).unwrap();

    let err = commit(&mut repo, "c", Some(v1.content_id), &doc("t", "pisotón")).unwrap_err();
    match err {
        StoreError::Conflict(c) => {
            assert_eq!(c.expected, Some(v1.content_id), "expected = lo que declaró el cliente");
            assert_eq!(c.current, Some(v2.content_id), "current = la cabeza real");
        }
        other => panic!("una base obsoleta debe ser Conflict, fue {other:?}"),
    }
    let view = repo.get(&id("c")).unwrap().unwrap();
    assert_eq!(view.content_id, v2.content_id, "la escritura buena sigue intacta");
}

/// Reescribir contenido idéntico es `no_change`: ni versión nueva ni
/// revisión nueva.
pub fn commit_identico_es_idempotente<R: MemoryRepository>(mut repo: R) {
    let texto = doc("t", "igual");
    let v1 = commit(&mut repo, "c", None, &texto).unwrap();
    let v2 = commit(&mut repo, "c", Some(v1.content_id), &texto).unwrap();
    assert!(v2.no_change, "contenido idéntico es no_change");
    assert_eq!(v2.version, v1.version, "no_change no avanza la versión");
    let h = repo.history(&id("c"), 10, None).unwrap();
    assert_eq!(h.len(), 1, "no_change no crea revisión");
}

/// Un documento que no valida se rechaza ANTES de tocar el almacén.
pub fn documento_invalido_no_deja_rastro<R: MemoryRepository>(mut repo: R) {
    let err = commit(&mut repo, "c", None, "sin frontmatter").unwrap_err();
    assert!(matches!(err, StoreError::Okf(_)), "validación antes que nada");
    assert!(repo.get(&id("c")).unwrap().is_none(), "nada quedó escrito");
}

/// Lo inexistente es `None` en `get` y `NotFound` en `history`.
pub fn inexistente_es_none_y_notfound<R: MemoryRepository>(repo: R) {
    assert!(repo.get(&id("no/existe")).unwrap().is_none(), "get -> None");
    assert!(
        matches!(repo.history(&id("no/existe"), 5, None), Err(StoreError::NotFound(_))),
        "history -> NotFound"
    );
}

/// Los criterios de búsqueda se combinan con AND y `limit` corta.
/// Los filtros estructurados (`doc_type`, `status`, `path_prefix`,
/// `tags`) son LITERALES: si nada los cumple, el resultado es vacío
/// — nunca se degrada a candidatos que no los cumplan.
pub fn busqueda_respeta_filtros_y_limite<R: MemoryRepository>(mut repo: R) {
    commit(&mut repo, "people/ana", None,
        "---\ntype: person\ntitle: Ana\nstatus: active\ntags:\n  - rust\n---\nIngeniera\n").unwrap();
    commit(&mut repo, "people/bo", None,
        "---\ntype: person\ntitle: Bo\nstatus: archived\n---\nDiseño\n").unwrap();
    commit(&mut repo, "notas/rust", None, &doc("Apuntes Rust", "lenguaje")).unwrap();
    commit(&mut repo, "skills/prog/mcp", None,
        "---\ntype: skill\ntitle: MCP\ntags:\n  - programming\n  - mcp\n---\nServidores MCP\n").unwrap();
    commit(&mut repo, "skills/prog/testing", None,
        "---\ntype: skill\ntitle: Testing\ntags:\n  - programming\n  - testing\n---\nPruebas\n").unwrap();

    let budget = Budget::default();
    let q = |text: Option<&str>, ty: Option<&str>, tags: &[&str], mode: TagsMode| SearchQuery {
        text: text.map(String::from),
        doc_type: ty.map(String::from),
        tags: tags.iter().map(|t| t.to_string()).collect(),
        tags_mode: mode,
        ..SearchQuery::default()
    };

    // text + type en AND.
    assert_eq!(repo.search(&q(Some("rust"), None, &[], TagsMode::Any), &budget).unwrap().len(), 2);
    assert_eq!(repo.search(&q(Some("rust"), Some("person"), &[], TagsMode::Any), &budget).unwrap().len(), 1);
    assert!(repo.search(&q(Some("nada-de-esto"), None, &[], TagsMode::Any), &budget).unwrap().is_empty());

    // tags es pertenencia LITERAL, con modo any/all explícito.
    assert_eq!(repo.search(&q(None, None, &["rust"], TagsMode::Any), &budget).unwrap().len(), 1);
    assert_eq!(repo.search(&q(None, None, &["programming"], TagsMode::Any), &budget).unwrap().len(), 2);
    assert_eq!(repo.search(&q(None, None, &["mcp", "testing"], TagsMode::Any), &budget).unwrap().len(), 2);
    assert_eq!(repo.search(&q(None, None, &["programming", "mcp"], TagsMode::All), &budget).unwrap().len(), 1);
    assert!(
        repo.search(&q(None, None, &["mcp", "testing"], TagsMode::All), &budget).unwrap().is_empty(),
        "all exige TODOS los tags en el mismo documento"
    );
    assert!(
        repo.search(&q(None, None, &["no-existe"], TagsMode::Any), &budget).unwrap().is_empty(),
        "un tag sin coincidencias devuelve vacío, no candidatos sin el tag"
    );

    // tags + type + text en AND, todos a la vez.
    assert_eq!(
        repo.search(&q(Some("pruebas"), Some("skill"), &["programming"], TagsMode::Any), &budget)
            .unwrap()
            .len(),
        1
    );

    // path_prefix lista una "carpeta" lógica completa.
    let by_prefix = |prefix: &str| SearchQuery {
        path_prefix: Some(prefix.to_string()),
        ..SearchQuery::default()
    };
    assert_eq!(repo.search(&by_prefix("skills/"), &budget).unwrap().len(), 2);
    assert_eq!(repo.search(&by_prefix("skills/prog/"), &budget).unwrap().len(), 2);
    assert_eq!(repo.search(&by_prefix("people/"), &budget).unwrap().len(), 2);
    assert!(repo.search(&by_prefix("vacio/"), &budget).unwrap().is_empty());

    // status filtra el ciclo de vida, separado de type.
    let by_status = |status: &str| SearchQuery {
        status: Some(status.to_string()),
        ..SearchQuery::default()
    };
    let activos = repo.search(&by_status("active"), &budget).unwrap();
    assert_eq!(activos.len(), 1);
    assert_eq!(activos[0].status.as_deref(), Some("active"));
    assert_eq!(repo.search(&by_status("archived"), &budget).unwrap().len(), 1);
    assert!(
        repo.search(&by_status("deleted"), &budget).unwrap().is_empty(),
        "sin status no hay coincidencia: los documentos sin el campo no cuentan"
    );

    // limit corta.
    let limited = SearchQuery { limit: Some(1), ..SearchQuery::default() };
    assert_eq!(repo.search(&limited, &budget).unwrap().len(), 1, "limit corta");
}

/// La historia sale de más reciente a más antigua y `before_seq`
/// pagina sin solapar.
pub fn historia_reciente_primero_y_paginada<R: MemoryRepository>(mut repo: R) {
    let mut last = None;
    for i in 0..5 {
        let out = commit(&mut repo, "c", last, &doc("t", &format!("v{i}"))).unwrap();
        last = Some(out.content_id);
    }
    let page1 = repo.history(&id("c"), 2, None).unwrap();
    assert_eq!(page1.len(), 2);
    assert!(page1[0].seq > page1[1].seq, "más reciente primero");
    let page2 = repo.history(&id("c"), 2, Some(page1[1].seq)).unwrap();
    assert!(page2[0].seq < page1[1].seq, "before_seq pagina sin solapar");
}
