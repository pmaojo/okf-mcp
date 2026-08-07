//! Tests de `memory_consolidate` sobre `InMemoryStore` sin adaptadores
//! de red: el modelo redacta título/resumen/entidades/decisiones, el
//! servidor solo valida y renderiza — nunca llama a un LLM propio.

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

#[test]
fn se_anuncia_sin_configuracion_extra_y_sin_ui() {
    let tools = tools();
    let spec = tools.tools().into_iter().find(|t| t.name == "memory_consolidate").unwrap();
    assert!(spec.ui_resource_uri.is_none());
}

#[test]
fn deriva_el_concept_id_del_titulo_cuando_se_omite() {
    let mut tools = tools();
    let out = call(
        &mut tools,
        "memory_consolidate",
        r#"{"title":"Sesion del 7 de agosto","summary":"Diseñamos memory_consolidate."}"#,
    );
    assert_eq!(out.get("concept_id").unwrap().as_str().unwrap(), "sessions/sesion-del-7-de-agosto");
    assert_eq!(out.get("created").unwrap(), &Value::Bool(true));
}

#[test]
fn renderiza_entidades_y_decisiones_como_enlaces_okf() {
    let mut tools = tools();
    call(&mut tools, "memory_commit", r#"{"concept_id":"projects/okf-mcp","markdown":"---\ntype: project\ntitle: okf-mcp\ntags:\n  - dev\n---\n","reason":"seed"}"#);

    let out = call(
        &mut tools,
        "memory_consolidate",
        r#"{
            "concept_id": "sessions/2026-08-07",
            "title": "Sesión con LifeOS",
            "summary": "Comparamos okf-mcp con LifeOS.",
            "entities": [{"concept_id": "projects/okf-mcp", "relation": "se amplió"}],
            "decisions": [{"text": "consolidar sin LLM en el servidor"}]
        }"#,
    );
    assert_eq!(out.get("concept_id").unwrap().as_str().unwrap(), "sessions/2026-08-07");

    let doc = call(&mut tools, "memory_resolve", r#"{"concept_id":"sessions/2026-08-07"}"#);
    let markdown = doc.get("document").unwrap().get("markdown").unwrap().as_str().unwrap();
    assert!(markdown.contains("type: session-summary"));
    assert!(markdown.contains("[[projects/okf-mcp]] (se amplió)"));
    assert!(markdown.contains("consolidar sin LLM en el servidor"));

    // El enlace es real: memory_backlinks lo ve desde el otro lado.
    let backlinks = call(&mut tools, "memory_backlinks", r#"{"concept_id":"projects/okf-mcp"}"#);
    let sources: Vec<&str> = backlinks
        .get("backlinks")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b.get("source").unwrap().get("concept_id").unwrap().as_str().unwrap())
        .collect();
    assert!(sources.contains(&"sessions/2026-08-07"));
}

#[test]
fn respeta_expected_hash_para_actualizar_una_sesion_existente() {
    let mut tools = tools();
    let first = call(
        &mut tools,
        "memory_consolidate",
        r#"{"concept_id":"sessions/2026-08-07","title":"Sesión","summary":"primer resumen"}"#,
    );
    let hash = first.get("hash").unwrap().as_str().unwrap().to_string();

    // Sin expected_hash correcto, choca con lo ya escrito (igual que memory_commit).
    let args = json_mini::parse(
        r#"{"concept_id":"sessions/2026-08-07","title":"Sesión","summary":"resumen sin el hash correcto"}"#,
    )
    .unwrap();
    assert!(tools.call("memory_consolidate", &args).is_err());

    let args = json_mini::parse(&format!(
        r#"{{"concept_id":"sessions/2026-08-07","title":"Sesión","summary":"resumen actualizado","expected_hash":"{hash}"}}"#
    ))
    .unwrap();
    let updated = tools.call("memory_consolidate", &args).unwrap();
    let updated: Value = json_mini::parse(&json_mini::to_string(&updated)).unwrap();
    assert_eq!(updated.get("created").unwrap(), &Value::Bool(false));
}

#[test]
fn titulo_o_resumen_vacios_se_rechazan() {
    let mut tools = tools();
    let args = json_mini::parse(r#"{"title":"  ","summary":"algo"}"#).unwrap();
    assert!(tools.call("memory_consolidate", &args).is_err());

    let args = json_mini::parse(r#"{"title":"algo","summary":"  "}"#).unwrap();
    assert!(tools.call("memory_consolidate", &args).is_err());
}

#[test]
fn concept_id_invalido_en_una_entidad_se_rechaza() {
    let mut tools = tools();
    let args = json_mini::parse(
        r#"{"title":"t","summary":"s","entities":[{"concept_id":"../etc/passwd"}]}"#,
    )
    .unwrap();
    assert!(tools.call("memory_consolidate", &args).is_err());
}
