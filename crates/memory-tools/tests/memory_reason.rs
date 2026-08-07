//! Tests de `memory_reason`: hechos asertados desde el vecindario
//! acotado (mismo recorrido que `memory_resolve`), razonamiento
//! `ontology-core` sobre `classes`/`properties`, y persistencia vía
//! `TripleStore` (capítulo 18 del tutorial).

use json_mini::Value;
use mcp_core::ToolHandler;
use memory_model::{Budget, ConceptId, Principal};
use memory_store::InMemoryStore;
use memory_tools::MemoryTools;
use store_core::TripleStore;

fn tools() -> MemoryTools<InMemoryStore> {
    MemoryTools::new(InMemoryStore::new(), Principal::local_dev(), Budget::default())
}

fn call(tools: &mut MemoryTools<InMemoryStore>, name: &str, args: &str) -> Value {
    let args = json_mini::parse(args).expect("argumentos JSON válidos");
    let out = tools.call(name, &args).expect("la llamada no debe fallar");
    json_mini::parse(&json_mini::to_string(&out)).unwrap()
}

fn commit(tools: &mut MemoryTools<InMemoryStore>, concept_id: &str, markdown: &str) {
    let args = format!(
        r#"{{"concept_id":{concept_id:?},"markdown":{markdown:?},"reason":"seed"}}"#,
    );
    call(tools, "memory_commit", &args);
}

#[test]
fn se_anuncia_con_ui() {
    let tools = tools();
    let spec = tools.tools().into_iter().find(|t| t.name == "memory_reason").unwrap();
    assert!(spec.ui_resource_uri.is_some());
}

#[test]
fn sin_ontologia_solo_devuelve_los_hechos_asertados() {
    let mut tools = tools();
    commit(&mut tools, "people/alice", "---\ntype: person\ntags:\n  - staff\n---\nVer [[reports_to:people/bob]].\n");

    let out = call(&mut tools, "memory_reason", r#"{"concept_id":"people/alice"}"#);

    assert_eq!(out.get("derived_count").unwrap(), &Value::Number(0.0));
    let triples = out.get("triples").unwrap().as_array().unwrap();
    assert!(triples.iter().all(|t| t.get("derived").unwrap() == &Value::Bool(false)));
    assert!(triples.iter().any(|t| t.get("predicate").unwrap().as_str() == Some("rdf:type")));
    assert!(triples.iter().any(|t| t.get("predicate").unwrap().as_str() == Some("reports_to")));
}

#[test]
fn subclass_of_deriva_el_tipo_general() {
    let mut tools = tools();
    commit(&mut tools, "people/alice", "---\ntype: student\n---\nnota\n");

    let out = call(
        &mut tools,
        "memory_reason",
        r#"{"concept_id":"people/alice","classes":[{"subclass":"student","superclass":"person"}]}"#,
    );

    let triples = out.get("triples").unwrap().as_array().unwrap();
    assert!(triples.iter().any(|t| {
        t.get("predicate").unwrap().as_str() == Some("rdf:type")
            && t.get("object").unwrap().get("value").unwrap().as_str() == Some("person")
            && t.get("derived").unwrap() == &Value::Bool(true)
    }));
    assert_eq!(out.get("derived_count").unwrap(), &Value::Number(1.0));
}

#[test]
fn transitive_property_encadena_a_traves_del_vecindario() {
    let mut tools = tools();
    commit(&mut tools, "tasks/a", "---\ntype: task\n---\nVer [[depends_on:tasks/b]].\n");
    commit(&mut tools, "tasks/b", "---\ntype: task\n---\nVer [[depends_on:tasks/c]].\n");
    commit(&mut tools, "tasks/c", "---\ntype: task\n---\nsin dependencias\n");

    let out = call(
        &mut tools,
        "memory_reason",
        r#"{"concept_id":"tasks/a","depth":3,"properties":[{"kind":"transitive","property":"depends_on"}]}"#,
    );

    let triples = out.get("triples").unwrap().as_array().unwrap();
    assert!(triples.iter().any(|t| {
        t.get("subject").unwrap().as_str() == Some("tasks/a")
            && t.get("predicate").unwrap().as_str() == Some("depends_on")
            && t.get("object").unwrap().get("value").unwrap().as_str() == Some("tasks/c")
            && t.get("derived").unwrap() == &Value::Bool(true)
    }));
}

#[test]
fn persist_por_defecto_guarda_en_el_triple_store() {
    let mut tools = tools();
    commit(&mut tools, "people/alice", "---\ntype: person\n---\nnota\n");

    call(&mut tools, "memory_reason", r#"{"concept_id":"people/alice","classes":[{"subclass":"person","superclass":"agent"}]}"#);

    let alice = ConceptId::parse("people/alice").unwrap();
    let stored = tools.repo().load_triples(&alice).unwrap();
    assert!(stored.iter().any(|t| t.predicate == "rdf:type"));
    assert_eq!(stored.len(), 2); // rdf:type person (asertado) + rdf:type agent (derivado)
}

#[test]
fn persist_false_no_persiste_pero_igual_devuelve_la_clausura() {
    let mut tools = tools();
    commit(&mut tools, "people/alice", "---\ntype: person\n---\nnota\n");

    let out = call(
        &mut tools,
        "memory_reason",
        r#"{"concept_id":"people/alice","classes":[{"subclass":"person","superclass":"agent"}],"persist":false}"#,
    );

    assert_eq!(out.get("persisted").unwrap(), &Value::Bool(false));
    assert_eq!(out.get("derived_count").unwrap(), &Value::Number(1.0));
}

#[test]
fn presupuesto_de_iteraciones_se_refleja_en_truncated() {
    let mut tools = tools();
    commit(&mut tools, "people/alice", "---\ntype: c0\n---\nnota\n");

    let classes = r#"[{"subclass":"c0","superclass":"c1"},{"subclass":"c1","superclass":"c2"},{"subclass":"c2","superclass":"c3"}]"#;
    let out = call(
        &mut tools,
        "memory_reason",
        &format!(r#"{{"concept_id":"people/alice","classes":{classes},"max_iterations":1}}"#),
    );

    let truncated = out.get("truncated").unwrap();
    assert_eq!(truncated.get("reasoning_by_iterations").unwrap(), &Value::Bool(true));
}

#[test]
fn ontology_id_carga_axiomas_del_documento_referenciado() {
    let mut tools = tools();
    commit(&mut tools, "ontologies/org", "---\ntype: ontology\n---\nsubclass_of: student -> person\n");
    commit(&mut tools, "people/alice", "---\ntype: student\n---\nnota\n");

    let out = call(
        &mut tools,
        "memory_reason",
        r#"{"concept_id":"people/alice","ontology_id":"ontologies/org"}"#,
    );

    assert_eq!(out.get("ontology_id").unwrap().as_str(), Some("ontologies/org"));
    let triples = out.get("triples").unwrap().as_array().unwrap();
    assert!(triples.iter().any(|t| {
        t.get("predicate").unwrap().as_str() == Some("rdf:type")
            && t.get("object").unwrap().get("value").unwrap().as_str() == Some("person")
            && t.get("derived").unwrap() == &Value::Bool(true)
    }));
}

#[test]
fn ontology_id_se_combina_con_axiomas_inline_no_los_reemplaza() {
    let mut tools = tools();
    commit(&mut tools, "ontologies/org", "---\ntype: ontology\n---\nsubclass_of: student -> person\n");
    commit(&mut tools, "people/alice", "---\ntype: student\n---\nVer [[depends_on:tasks/x]].\n");
    commit(&mut tools, "tasks/x", "---\ntype: task\n---\nVer [[depends_on:tasks/y]].\n");
    commit(&mut tools, "tasks/y", "---\ntype: task\n---\nsin dependencias\n");

    let out = call(
        &mut tools,
        "memory_reason",
        r#"{"concept_id":"people/alice","depth":3,"ontology_id":"ontologies/org","properties":[{"kind":"transitive","property":"depends_on"}]}"#,
    );

    let triples = out.get("triples").unwrap().as_array().unwrap();
    // Del documento referenciado (subclass_of):
    assert!(triples.iter().any(|t| t.get("object").unwrap().get("value").unwrap().as_str() == Some("person")));
    // Del axioma inline (transitive), aplicado sobre el MISMO conjunto de hechos:
    assert!(triples.iter().any(|t| {
        t.get("subject").unwrap().as_str() == Some("people/alice")
            && t.get("predicate").unwrap().as_str() == Some("depends_on")
            && t.get("object").unwrap().get("value").unwrap().as_str() == Some("tasks/y")
    }));
}

#[test]
fn ontology_id_inexistente_se_rechaza() {
    let mut tools = tools();
    commit(&mut tools, "people/alice", "---\ntype: person\n---\nnota\n");

    let args =
        json_mini::parse(r#"{"concept_id":"people/alice","ontology_id":"ontologies/no-existe"}"#)
            .unwrap();
    let err = tools.call("memory_reason", &args).unwrap_err();
    assert!(matches!(err, mcp_core::ToolError::Failed(ref msg) if msg.contains("not_found")));
}

#[test]
fn ontology_id_que_no_es_type_ontology_se_rechaza() {
    let mut tools = tools();
    commit(&mut tools, "people/bob", "---\ntype: person\n---\nno es una ontologia\n");
    commit(&mut tools, "people/alice", "---\ntype: person\n---\nnota\n");

    let args = json_mini::parse(r#"{"concept_id":"people/alice","ontology_id":"people/bob"}"#).unwrap();
    let err = tools.call("memory_reason", &args).unwrap_err();
    assert!(matches!(err, mcp_core::ToolError::InvalidArguments(_)));
}

#[test]
fn ontology_id_con_sintaxis_invalida_en_el_cuerpo_se_rechaza() {
    let mut tools = tools();
    commit(&mut tools, "ontologies/roto", "---\ntype: ontology\n---\nesto no es un axioma\n");
    commit(&mut tools, "people/alice", "---\ntype: person\n---\nnota\n");

    let args =
        json_mini::parse(r#"{"concept_id":"people/alice","ontology_id":"ontologies/roto"}"#).unwrap();
    let err = tools.call("memory_reason", &args).unwrap_err();
    assert!(matches!(err, mcp_core::ToolError::InvalidArguments(ref msg) if msg.contains("línea 1")));
}

#[test]
fn sin_ontology_id_el_campo_viene_null() {
    let mut tools = tools();
    commit(&mut tools, "people/alice", "---\ntype: person\n---\nnota\n");

    let out = call(&mut tools, "memory_reason", r#"{"concept_id":"people/alice"}"#);
    assert_eq!(out.get("ontology_id").unwrap(), &Value::Null);
}

#[test]
fn kind_de_axioma_desconocido_se_rechaza() {
    let mut tools = tools();
    commit(&mut tools, "people/alice", "---\ntype: person\n---\nnota\n");

    let args = json_mini::parse(
        r#"{"concept_id":"people/alice","properties":[{"kind":"bogus","property":"x"}]}"#,
    )
    .unwrap();
    let err = tools.call("memory_reason", &args).unwrap_err();
    assert!(matches!(err, mcp_core::ToolError::InvalidArguments(_)));
}
