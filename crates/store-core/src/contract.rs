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

use crate::{
    BulkItem, CommitRequest, MemoryRepository, SearchQuery, StoreError, StoreMaintenance,
};
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

fn request(concept: &str, expected: Option<ContentId>, markdown: &str) -> CommitRequest {
    CommitRequest {
        concept_id: id(concept),
        expected,
        markdown: markdown.to_string(),
        reason: "contrato".to_string(),
    }
}

/// Ejecuta el contrato completo. `mk` fabrica un repositorio VACÍO;
/// se llama una vez por propiedad para que los tests no se contaminen.
pub fn run_all<R: MemoryRepository + StoreMaintenance>(mut mk: impl FnMut() -> R) {
    crear_leer_y_versionar(mk());
    base_obsoleta_no_pisa(mk());
    commit_identico_es_idempotente(mk());
    documento_invalido_no_deja_rastro(mk());
    inexistente_es_none_y_notfound(mk());
    busqueda_respeta_filtros_y_limite(mk());
    busqueda_excluye_por_exclude_type(mk());
    historia_reciente_primero_y_paginada(mk());
    borrar_exige_cas_y_oculta_el_documento(mk());
    recrear_tras_borrar_continua_la_historia(mk());
    busqueda_por_prefijo_de_ruta(mk());
    backlinks_con_relacion_y_sin_borrados(mk());
    lote_no_atomico_aplica_lo_que_puede(mk());
    lote_atomico_es_todo_o_nada(mk());
    salud_de_enlaces_clasifica_destinos(mk());
    validate_reporta_rotos_y_borrados(mk());
    stats_cuenta_el_grafo(mk());
    status_cuenta_documentos_y_enlaces(mk());
    embed_pending_en_vacio_no_hace_nada(mk());
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
pub fn busqueda_respeta_filtros_y_limite<R: MemoryRepository>(mut repo: R) {
    commit(&mut repo, "people/ana", None,
        "---\ntype: person\ntitle: Ana\ntags:\n  - rust\n---\nIngeniera\n").unwrap();
    commit(&mut repo, "people/bo", None,
        "---\ntype: person\ntitle: Bo\n---\nDiseño\n").unwrap();
    commit(&mut repo, "notas/rust", None, &doc("Apuntes Rust", "lenguaje")).unwrap();

    let budget = Budget::default();
    let q = |text: Option<&str>, ty: Option<&str>, tag: Option<&str>, limit: Option<usize>| SearchQuery {
        text: text.map(String::from),
        doc_type: ty.map(String::from),
        exclude_type: None,
        tag: tag.map(String::from),
        path_prefix: None,
        limit,
    };
    assert_eq!(repo.search(&q(Some("rust"), None, None, None), &budget).unwrap().len(), 2);
    assert_eq!(repo.search(&q(Some("rust"), Some("person"), None, None), &budget).unwrap().len(), 1);
    assert_eq!(repo.search(&q(None, None, Some("rust"), None), &budget).unwrap().len(), 1);
    assert_eq!(repo.search(&q(None, None, None, Some(1)), &budget).unwrap().len(), 1, "limit corta");
    assert!(repo.search(&q(Some("nada-de-esto"), None, None, None), &budget).unwrap().is_empty());
}

/// `exclude_type` es lo contrario de `doc_type`: descarta esa clase en
/// vez de exigirla. Cubre la ausencia de "búsqueda por `not type:X`"
/// sin requerir post-filtrado del lado del agente.
pub fn busqueda_excluye_por_exclude_type<R: MemoryRepository>(mut repo: R) {
    commit(&mut repo, "people/ana", None,
        "---\ntype: person\ntitle: Ana\n---\nIngeniera\n").unwrap();
    commit(&mut repo, "tasks/uno", None,
        "---\ntype: task\ntitle: Uno\n---\nPendiente\n").unwrap();
    commit(&mut repo, "tasks/dos", None,
        "---\ntype: task\ntitle: Dos\n---\nPendiente\n").unwrap();

    let budget = Budget::default();
    let without_tasks = SearchQuery {
        exclude_type: Some("task".to_string()),
        ..SearchQuery::default()
    };
    let hits = repo.search(&without_tasks, &budget).unwrap();
    assert_eq!(hits.len(), 1, "solo queda el 'person'");
    assert_eq!(hits[0].concept_id, id("people/ana"));

    // Combinado con AND: exigir 'task' y excluir 'task' a la vez nunca
    // devuelve nada.
    let contradictory = SearchQuery {
        doc_type: Some("task".to_string()),
        exclude_type: Some("task".to_string()),
        ..SearchQuery::default()
    };
    assert!(repo.search(&contradictory, &budget).unwrap().is_empty());
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

/// Borrar exige la base leída (CAS) y, aceptado, el documento
/// desaparece de `get` y de `search` pero su historia queda.
pub fn borrar_exige_cas_y_oculta_el_documento<R: MemoryRepository>(mut repo: R) {
    let v1 = commit(&mut repo, "c/borrable", None, &doc("t", "v1")).unwrap();
    let v2 = commit(&mut repo, "c/borrable", Some(v1.content_id), &doc("t", "v2")).unwrap();

    // Base obsoleta: conflicto con los hashes reales, sin borrar nada.
    let err = repo
        .delete(&id("c/borrable"), v1.content_id, &Principal::local_dev(), "obsoleto".to_string())
        .unwrap_err();
    match err {
        StoreError::Conflict(c) => {
            assert_eq!(c.expected, Some(v1.content_id));
            assert_eq!(c.current, Some(v2.content_id));
        }
        other => panic!("base obsoleta al borrar debe ser Conflict, fue {other:?}"),
    }
    assert!(repo.get(&id("c/borrable")).unwrap().is_some(), "el conflicto no borra");

    // Base correcta: borrado aceptado.
    let out = repo
        .delete(&id("c/borrable"), v2.content_id, &Principal::local_dev(), "limpieza".to_string())
        .unwrap();
    assert_eq!(out.content_id, v2.content_id);
    assert_eq!(out.version, v2.version);

    assert!(repo.get(&id("c/borrable")).unwrap().is_none(), "get -> None tras borrar");
    let hits = repo.search(&SearchQuery::default(), &Budget::default()).unwrap();
    assert!(
        hits.iter().all(|h| h.concept_id != id("c/borrable")),
        "search no lista borrados"
    );

    // La historia sobrevive, con la revisión del borrado la primera.
    let h = repo.history(&id("c/borrable"), 10, None).unwrap();
    assert_eq!(h.len(), 3, "crear + editar + borrar");
    assert!(h[0].reason.contains("limpieza"));

    // Borrar lo ya borrado es NotFound, como cualquier inexistente.
    assert!(matches!(
        repo.delete(&id("c/borrable"), v2.content_id, &Principal::local_dev(), "otra vez".to_string()),
        Err(StoreError::NotFound(_))
    ));
}

/// Recrear un concepto borrado parte de `expected = None` (para el
/// cliente ES una creación) pero la numeración de versiones continúa:
/// la historia es una sola línea de tiempo.
pub fn recrear_tras_borrar_continua_la_historia<R: MemoryRepository>(mut repo: R) {
    let v1 = commit(&mut repo, "c/fenix", None, &doc("t", "primera vida")).unwrap();
    repo.delete(&id("c/fenix"), v1.content_id, &Principal::local_dev(), "muere".to_string())
        .unwrap();

    let v2 = commit(&mut repo, "c/fenix", None, &doc("t", "segunda vida")).unwrap();
    assert!(v2.created, "para el cliente es una creación");
    assert!(v2.version > v1.version, "la versión nunca retrocede");
    assert!(repo.get(&id("c/fenix")).unwrap().is_some());
    assert_eq!(repo.history(&id("c/fenix"), 10, None).unwrap().len(), 3);
}

/// `path_prefix` filtra por segmentos completos, no por subcadena.
pub fn busqueda_por_prefijo_de_ruta<R: MemoryRepository>(mut repo: R) {
    commit(&mut repo, "people/alice", None, &doc("Alice", "a")).unwrap();
    commit(&mut repo, "people/bob", None, &doc("Bob", "b")).unwrap();
    commit(&mut repo, "peoples/trampa", None, &doc("Trampa", "c")).unwrap();
    commit(&mut repo, "notas/rust", None, &doc("Rust", "d")).unwrap();

    let q = SearchQuery { path_prefix: Some("people".to_string()), ..SearchQuery::default() };
    let hits = repo.search(&q, &Budget::default()).unwrap();
    let ids: Vec<&str> = hits.iter().map(|h| h.concept_id.as_str()).collect();
    assert_eq!(ids, vec!["people/alice", "people/bob"], "segmentos completos, orden por id");
}

/// `backlinks` devuelve los orígenes vivos con su relación tipada,
/// ordenados por id; los orígenes borrados dejan de contar.
pub fn backlinks_con_relacion_y_sin_borrados<R: MemoryRepository>(mut repo: R) {
    let a = commit(&mut repo, "z/a", None,
        "---\ntype: note\n---\nusa [[uses:c/destino]]\n").unwrap();
    commit(&mut repo, "b/b", None, "---\ntype: note\n---\nver [[c/destino]]\n").unwrap();

    // El destino NI SIQUIERA existe: preguntar sigue siendo válido.
    let back = repo.backlinks(&id("c/destino")).unwrap();
    let resumen: Vec<(&str, Option<&str>)> = back
        .iter()
        .map(|b| (b.source.concept_id.as_str(), b.rel.as_deref()))
        .collect();
    assert_eq!(resumen, vec![("b/b", None), ("z/a", Some("uses"))]);

    repo.delete(&id("z/a"), a.content_id, &Principal::local_dev(), "fuera".to_string()).unwrap();
    let back = repo.backlinks(&id("c/destino")).unwrap();
    assert_eq!(back.len(), 1, "el origen borrado ya no enlaza");
    assert_eq!(back[0].source.concept_id.as_str(), "b/b");
}

/// En un lote NO atómico cada item corre su suerte y los posteriores
/// ven los efectos de los anteriores.
pub fn lote_no_atomico_aplica_lo_que_puede<R: MemoryRepository>(mut repo: R) {
    let out = repo
        .commit_bulk(
            vec![
                request("l/uno", None, &doc("Uno", "v1")),
                request("l/malo", None, "sin frontmatter"),
                request("l/dos", None, &doc("Dos", "v1")),
            ],
            false,
            &Principal::local_dev(),
            &Budget::default(),
        )
        .unwrap();

    assert!(out.applied);
    assert_eq!(out.items.len(), 3);
    assert!(matches!(out.items[0], BulkItem::Done(_)));
    assert!(matches!(out.items[1], BulkItem::Failed(StoreError::Okf(_))));
    assert!(matches!(out.items[2], BulkItem::Done(_)));
    assert!(repo.get(&id("l/uno")).unwrap().is_some());
    assert!(repo.get(&id("l/malo")).unwrap().is_none());
    assert!(repo.get(&id("l/dos")).unwrap().is_some());
}

/// En un lote atómico, un fallo revierte TODO: el culpable queda
/// `Failed`, el resto `Skipped`, y el almacén como estaba.
pub fn lote_atomico_es_todo_o_nada<R: MemoryRepository>(mut repo: R) {
    // Caso feliz: todo se aplica.
    let out = repo
        .commit_bulk(
            vec![
                request("l/a", None, &doc("A", "v1")),
                request("l/b", None, &doc("B", "v1")),
            ],
            true,
            &Principal::local_dev(),
            &Budget::default(),
        )
        .unwrap();
    assert!(out.applied);
    assert!(out.items.iter().all(|i| matches!(i, BulkItem::Done(_))));

    // Caso fallido: nada del lote queda escrito.
    let out = repo
        .commit_bulk(
            vec![
                request("l/c", None, &doc("C", "v1")),
                request("l/malo", None, "sin frontmatter"),
            ],
            true,
            &Principal::local_dev(),
            &Budget::default(),
        )
        .unwrap();
    assert!(!out.applied);
    assert!(matches!(out.items[0], BulkItem::Skipped));
    assert!(matches!(out.items[1], BulkItem::Failed(StoreError::Okf(_))));
    assert!(repo.get(&id("l/c")).unwrap().is_none(), "el lote atómico se revirtió entero");
    assert!(repo.get(&id("l/a")).unwrap().is_some(), "lo anterior al lote no se toca");
}

/// `link_health` clasifica cada enlace saliente por el estado real
/// de su destino.
pub fn salud_de_enlaces_clasifica_destinos<R: MemoryRepository + StoreMaintenance>(mut repo: R) {
    let muerto = commit(&mut repo, "s/muerto", None, &doc("Muerto", "x")).unwrap();
    commit(&mut repo, "s/vivo", None, &doc("Vivo", "x")).unwrap();
    commit(&mut repo, "s/origen", None,
        "---\ntype: note\n---\n[[s/vivo]] [[s/fantasma]] [[was:s/muerto]]\n").unwrap();
    repo.delete(&id("s/muerto"), muerto.content_id, &Principal::local_dev(), "rip".to_string())
        .unwrap();

    let health = repo.link_health(&id("s/origen")).unwrap();
    let solo = |v: &[okf_core::Link]| -> Vec<String> {
        v.iter().map(|l| l.target.as_str().to_string()).collect()
    };
    assert_eq!(solo(&health.ok), vec!["s/vivo"]);
    assert_eq!(solo(&health.broken), vec!["s/fantasma"]);
    assert_eq!(solo(&health.deleted), vec!["s/muerto"]);

    assert!(matches!(repo.link_health(&id("no/existe")), Err(StoreError::NotFound(_))));
}

/// `validate` recorre el grafo (o un subárbol) y reporta enlaces
/// rotos y referencias a borrados, con totales veraces.
pub fn validate_reporta_rotos_y_borrados<R: MemoryRepository + StoreMaintenance>(mut repo: R) {
    let muerto = commit(&mut repo, "v/muerto", None, &doc("Muerto", "x")).unwrap();
    commit(&mut repo, "v/origen", None,
        "---\ntype: note\n---\n[[v/fantasma]] [[v/muerto]]\n").unwrap();
    commit(&mut repo, "otra/rama", None, "---\ntype: note\n---\n[[otra/hueca]]\n").unwrap();
    repo.delete(&id("v/muerto"), muerto.content_id, &Principal::local_dev(), "rip".to_string())
        .unwrap();

    let report = repo.validate(None, &Budget::default()).unwrap();
    assert_eq!(report.broken_links_total, 2, "v/fantasma y otra/hueca");
    assert_eq!(report.deleted_referenced_total, 1, "v/muerto");
    assert!(report
        .deleted_referenced
        .contains(&(id("v/origen"), id("v/muerto"))));

    let report = repo.validate(Some("v"), &Budget::default()).unwrap();
    assert_eq!(report.broken_links_total, 1, "el subárbol no ve otra/hueca");
    assert_eq!(report.broken_links, vec![(id("v/origen"), id("v/fantasma"))]);
}

/// `stats` cuenta el grafo vivo: tipos, tags, hubs y huérfanos.
pub fn stats_cuenta_el_grafo<R: MemoryRepository + StoreMaintenance>(mut repo: R) {
    commit(&mut repo, "g/hub", None, &doc("Hub", "x")).unwrap();
    commit(&mut repo, "g/a", None,
        "---\ntype: person\ntags:\n  - rust\n---\n[[g/hub]]\n").unwrap();
    commit(&mut repo, "g/b", None,
        "---\ntype: person\ntags:\n  - rust\n  - mcp\n---\n[[g/hub]]\n").unwrap();
    commit(&mut repo, "g/isla", None, &doc("Isla", "sin enlaces")).unwrap();
    let extra = commit(&mut repo, "g/borrado", None, &doc("Borrado", "x")).unwrap();
    repo.delete(&id("g/borrado"), extra.content_id, &Principal::local_dev(), "rip".to_string())
        .unwrap();

    let stats = repo.stats(&Budget::default()).unwrap();
    assert_eq!(stats.documents, 4);
    assert_eq!(stats.deleted_documents, 1);
    assert!(stats.by_type.contains(&("person".to_string(), 2)));
    assert!(stats.by_type.contains(&("note".to_string(), 2)), "el borrado no cuenta");
    assert!(stats.by_tag.contains(&("rust".to_string(), 2)));
    assert_eq!(stats.top_linked[0], (id("g/hub"), 2));
    assert_eq!(stats.orphans, vec![id("g/isla")], "isla no enlaza ni es enlazada");
}

/// `status` responde "¿está sano?" con números que cuadran con el
/// grafo. Los campos de embeddings y outbox son backend-específicos
/// y aquí solo se exige que existan (un backend sin esas piezas
/// devuelve ceros).
pub fn status_cuenta_documentos_y_enlaces<R: MemoryRepository + StoreMaintenance>(mut repo: R) {
    let muerto = commit(&mut repo, "st/muerto", None, &doc("Muerto", "x")).unwrap();
    commit(&mut repo, "st/origen", None,
        "---\ntype: note\n---\n[[st/fantasma]] [[st/muerto]]\n").unwrap();
    repo.delete(&id("st/muerto"), muerto.content_id, &Principal::local_dev(), "rip".to_string())
        .unwrap();

    let status = repo.status().unwrap();
    assert_eq!(status.documents, 1);
    assert_eq!(status.deleted_documents, 1);
    assert_eq!(status.broken_links, 1);
    assert_eq!(status.deleted_referenced, 1);
}

/// Sobre un almacén vacío, `embed_pending` no tiene trabajo: el
/// resultado es vacío sea cual sea el backend (y sin exigir
/// credenciales de ningún proveedor).
pub fn embed_pending_en_vacio_no_hace_nada<R: MemoryRepository + StoreMaintenance>(mut repo: R) {
    let out = repo.embed_pending(None, 10).unwrap();
    assert!(out.embedded.is_empty());
    assert!(out.failed.is_empty());
    assert_eq!(out.remaining, 0);
}
