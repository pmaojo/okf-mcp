//! Todo tool de escritura devuelve un campo `warnings` (array, vacío
//! en el caso normal) — ver `store_core::CommitOutcome::warnings` y
//! `MemoryTools::warnings_json`. Sobre `InMemoryStore` (sin backend
//! compuesto de por medio) siempre está vacío; lo que este test
//! garantiza es que el CAMPO existe con esa forma en cada respuesta,
//! para que un caller pueda mirarlo siempre sin comprobar antes si
//! está presente.

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

fn assert_empty_warnings(out: &Value) {
    assert_eq!(out.get("warnings").unwrap(), &Value::Array(Vec::new()));
}

#[test]
fn memory_commit_trae_warnings_vacio() {
    let mut tools = tools();
    let out = call(
        &mut tools,
        "memory_commit",
        r#"{"concept_id":"demo/a","markdown":"---\ntype: note\n---\nhola\n","reason":"seed"}"#,
    );
    assert_empty_warnings(&out);

    let out = call(
        &mut tools,
        "memory_commit",
        r#"{"concept_id":"demo/b","markdown":"---\ntype: note\n---\nhola\n","reason":"seed","dry_run":true}"#,
    );
    assert_empty_warnings(&out);
}

#[test]
fn memory_delete_trae_warnings_vacio() {
    let mut tools = tools();
    let commit = call(
        &mut tools,
        "memory_commit",
        r#"{"concept_id":"demo/c","markdown":"---\ntype: note\n---\nhola\n","reason":"seed"}"#,
    );
    let hash = commit.get("hash").unwrap().as_str().unwrap();
    let out = call(
        &mut tools,
        "memory_delete",
        &format!(r#"{{"concept_id":"demo/c","expected_hash":{hash:?},"reason":"limpieza"}}"#),
    );
    assert_empty_warnings(&out);
}

#[test]
fn memory_patch_trae_warnings_vacio() {
    let mut tools = tools();
    let commit = call(
        &mut tools,
        "memory_commit",
        r#"{"concept_id":"demo/d","markdown":"---\ntype: note\n---\nhola\n","reason":"seed"}"#,
    );
    let hash = commit.get("hash").unwrap().as_str().unwrap();
    let out = call(
        &mut tools,
        "memory_patch",
        &format!(
            r#"{{"concept_id":"demo/d","expected_hash":{hash:?},"reason":"tag","add_tags":["x"]}}"#
        ),
    );
    assert_empty_warnings(&out);
}

#[test]
fn memory_bulk_commit_trae_warnings_vacio_por_item() {
    let mut tools = tools();
    let out = call(
        &mut tools,
        "memory_bulk_commit",
        r#"{"requests":[{"concept_id":"demo/e","markdown":"---\ntype: note\n---\nhola\n","reason":"seed"}]}"#,
    );
    let items = out.get("items").unwrap().as_array().unwrap();
    assert_eq!(items[0].get("warnings").unwrap(), &Value::Array(Vec::new()));
}

#[test]
fn spec_propose_y_spec_tasks_traen_warnings_vacio() {
    let mut tools = tools();
    let out = call(
        &mut tools,
        "spec_propose",
        r#"{"concept_id":"specs/demo","title":"Demo","requirements":"req","design":"diseño"}"#,
    );
    assert_empty_warnings(&out);

    let out = call(
        &mut tools,
        "spec_tasks",
        r#"{"spec_id":"specs/demo","tasks":[{"title":"Uno"}]}"#,
    );
    assert_eq!(out.get("warnings").unwrap(), &Value::Array(Vec::new()));
}
