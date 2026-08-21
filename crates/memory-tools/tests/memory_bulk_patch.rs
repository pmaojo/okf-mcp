//! Tests de `memory_bulk_patch`: patchear varios conceptos a la vez,
//! atómico por defecto — sobre `InMemoryStore` sin adaptadores de red.

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

fn status(v: &Value) -> &str {
    v.get("status").unwrap().as_str().unwrap()
}

#[test]
fn se_anuncia_con_ui() {
    let tools = tools();
    let names: Vec<&str> = tools.tools().iter().map(|t| t.name).collect();
    assert!(names.contains(&"memory_bulk_patch"));
    let spec = tools.tools().into_iter().find(|t| t.name == "memory_bulk_patch").unwrap();
    assert!(spec.ui_resource_uri.is_some());
}

#[test]
fn aplica_varios_patches_a_la_vez() {
    let mut tools = tools();
    let h1 = commit(&mut tools, "people/alice", "---\ntype: person\ntitle: Alice\n---\nIngeniera\n");
    let h2 = commit(&mut tools, "people/bob", "---\ntype: person\ntitle: Bob\n---\nDiseñador\n");

    let out = call(
        &mut tools,
        "memory_bulk_patch",
        &format!(
            r#"{{"patches":[
                {{"concept_id":"people/alice","expected_hash":"{h1}","reason":"promoción","add_tags":["lead"]}},
                {{"concept_id":"people/bob","expected_hash":"{h2}","reason":"promoción","add_tags":["lead"]}}
            ]}}"#
        ),
    );
    assert_eq!(out.get("applied").unwrap(), &Value::Bool(true));
    let items = out.get("items").unwrap().as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(status(&items[0]), "done");
    assert_eq!(status(&items[1]), "done");

    let alice = call(&mut tools, "memory_resolve", r#"{"concept_id":"people/alice"}"#);
    let tags: Vec<&str> = alice
        .get("document").unwrap()
        .get("tags").unwrap()
        .as_array().unwrap()
        .iter().map(|v| v.as_str().unwrap()).collect();
    assert!(tags.contains(&"lead"));
}

#[test]
fn atomico_no_persiste_nada_si_un_item_falla_pre_validacion() {
    let mut tools = tools();
    let h1 = commit(&mut tools, "people/alice", "---\ntype: person\ntitle: Alice\n---\nIngeniera\n");

    let out = call(
        &mut tools,
        "memory_bulk_patch",
        &format!(
            r#"{{"patches":[
                {{"concept_id":"people/alice","expected_hash":"{h1}","reason":"x","add_tags":["lead"]}},
                {{"concept_id":"people/no-existe","expected_hash":"{h1}","reason":"x","add_tags":["lead"]}}
            ]}}"#
        ),
    );
    assert_eq!(out.get("applied").unwrap(), &Value::Bool(false));
    let items = out.get("items").unwrap().as_array().unwrap();
    assert_eq!(status(&items[0]), "skipped", "atómico: el válido tampoco se aplica");
    assert_eq!(status(&items[1]), "failed");

    // Nada se persistió: alice sigue sin el tag.
    let alice = call(&mut tools, "memory_resolve", r#"{"concept_id":"people/alice"}"#);
    let tags: Vec<&str> = alice
        .get("document").unwrap()
        .get("tags").unwrap()
        .as_array().unwrap()
        .iter().map(|v| v.as_str().unwrap()).collect();
    assert!(!tags.contains(&"lead"));
}

#[test]
fn no_atomico_aplica_lo_que_puede() {
    let mut tools = tools();
    let h1 = commit(&mut tools, "people/alice", "---\ntype: person\ntitle: Alice\n---\nIngeniera\n");

    let out = call(
        &mut tools,
        "memory_bulk_patch",
        &format!(
            r#"{{"atomic":false,"patches":[
                {{"concept_id":"people/alice","expected_hash":"{h1}","reason":"x","add_tags":["lead"]}},
                {{"concept_id":"people/no-existe","expected_hash":"{h1}","reason":"x","add_tags":["lead"]}}
            ]}}"#
        ),
    );
    let items = out.get("items").unwrap().as_array().unwrap();
    assert_eq!(status(&items[0]), "done");
    assert_eq!(status(&items[1]), "failed");

    let alice = call(&mut tools, "memory_resolve", r#"{"concept_id":"people/alice"}"#);
    let tags: Vec<&str> = alice
        .get("document").unwrap()
        .get("tags").unwrap()
        .as_array().unwrap()
        .iter().map(|v| v.as_str().unwrap()).collect();
    assert!(tags.contains(&"lead"));
}

#[test]
fn respeta_el_gauntlet_de_evidencia_en_tasks() {
    let mut tools = tools();
    let h = commit(
        &mut tools,
        "tasks/uno",
        "---\ntype: task\ntitle: Uno\ntags:\n  - status-pending\n---\nPendiente\n",
    );

    let out = call(
        &mut tools,
        "memory_bulk_patch",
        &format!(
            r#"{{"patches":[
                {{"concept_id":"tasks/uno","expected_hash":"{h}","reason":"completar","remove_tags":["status-pending"],"add_tags":["status-done"]}}
            ]}}"#
        ),
    );
    assert_eq!(out.get("applied").unwrap(), &Value::Bool(false));
    let items = out.get("items").unwrap().as_array().unwrap();
    assert_eq!(status(&items[0]), "failed");
    let detail = items[0].get("error").unwrap().get("detail").unwrap().as_str().unwrap();
    assert!(detail.contains("Evidencia"));
}

#[test]
fn conflicto_de_cas_marca_solo_ese_item_en_modo_no_atomico() {
    let mut tools = tools();
    let h1 = commit(&mut tools, "people/alice", "---\ntype: person\ntitle: Alice\n---\nIngeniera\n");
    // Una escritura por debajo cambia el hash sin que el lote lo sepa.
    call(
        &mut tools,
        "memory_commit",
        &format!(r#"{{"concept_id":"people/alice","expected_hash":"{h1}","markdown":"---\ntype: person\ntitle: Alice\n---\nOtra cosa\n","reason":"editado en paralelo"}}"#),
    );

    let out = call(
        &mut tools,
        "memory_bulk_patch",
        &format!(
            r#"{{"atomic":false,"patches":[
                {{"concept_id":"people/alice","expected_hash":"{h1}","reason":"x","add_tags":["lead"]}}
            ]}}"#
        ),
    );
    let items = out.get("items").unwrap().as_array().unwrap();
    assert_eq!(status(&items[0]), "failed");
    assert_eq!(items[0].get("error").unwrap().get("kind").unwrap().as_str().unwrap(), "revision_conflict");
}
