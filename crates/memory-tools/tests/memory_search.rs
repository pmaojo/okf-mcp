//! Tests de `memory_search` a nivel de herramienta: que el argumento
//! `not_type` llegue hasta `SearchQuery::exclude_type` y que los
//! documentos borrados nunca aparezcan en los resultados. El resto del
//! comportamiento de filtrado vive en el contrato de
//! `store_core::contract`, compartido por los tres backends.

use json_mini::Value;
use mcp_core::ToolHandler;
use memory_model::{Budget, Principal};
use memory_store::InMemoryStore;
use memory_tools::MemoryTools;

fn tools() -> MemoryTools<InMemoryStore> {
    MemoryTools::new(InMemoryStore::new(), Principal::local_dev(), Budget::default())
}

fn call(tools: &mut MemoryTools<InMemoryStore>, name: &str, args: &str) -> Value {
    let args = json_mini::parse(args).expect("argumentos JSON válidos");
    let out = tools.call(name, &args).expect("la llamada no debe fallar");
    json_mini::parse(&json_mini::to_string(&out)).unwrap()
}

fn commit(tools: &mut MemoryTools<InMemoryStore>, concept_id: &str, markdown: &str) -> String {
    let out = call(
        tools,
        "memory_commit",
        &format!(
            r#"{{"concept_id":"{concept_id}","markdown":{},"reason":"seed"}}"#,
            json_mini::to_string(&Value::String(markdown.to_string()))
        ),
    );
    out.get("hash").unwrap().as_str().unwrap().to_string()
}

fn concept_ids(out: &Value) -> Vec<&str> {
    out.get("results").unwrap().as_array().unwrap().iter()
        .map(|h| h.get("concept_id").unwrap().as_str().unwrap())
        .collect()
}

#[test]
fn not_type_excluye_ese_type() {
    let mut tools = tools();
    commit(&mut tools, "people/alice", "---\ntype: person\ntitle: Alice\n---\nIngeniera\n");
    commit(&mut tools, "tasks/uno", "---\ntype: task\ntitle: Uno\n---\nPendiente\n");
    commit(&mut tools, "tasks/dos", "---\ntype: task\ntitle: Dos\n---\nPendiente\n");

    let out = call(&mut tools, "memory_search", r#"{"not_type":"task"}"#);
    assert_eq!(concept_ids(&out), vec!["people/alice"]);
}

#[test]
fn busqueda_nunca_lista_documentos_borrados() {
    let mut tools = tools();
    let h = commit(&mut tools, "people/alice", "---\ntype: person\ntitle: Alice\n---\nIngeniera\n");
    call(
        &mut tools,
        "memory_delete",
        &format!(r#"{{"concept_id":"people/alice","expected_hash":"{h}","reason":"baja"}}"#),
    );

    let out = call(&mut tools, "memory_search", r#"{"query":"Alice"}"#);
    assert!(concept_ids(&out).is_empty(), "search no debe listar documentos borrados");

    let out_all = call(&mut tools, "memory_search", r#"{}"#);
    assert!(concept_ids(&out_all).is_empty());
}
