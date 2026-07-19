//! Tests del flujo spec-driven (`spec_propose` → `spec_tasks` →
//! `spec_status`), sobre `InMemoryStore` sin adaptadores de red.

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

fn as_f64(v: &Value) -> f64 {
    v.as_f64().expect("esperaba número")
}

#[test]
fn se_anuncian_siempre_sin_configuracion_extra() {
    let tools = tools();
    let names: Vec<&str> = tools.tools().iter().map(|t| t.name).collect();
    assert!(names.contains(&"spec_propose"));
    assert!(names.contains(&"spec_tasks"));
    assert!(names.contains(&"spec_status"));
}

#[test]
fn spec_propose_crea_el_documento_con_requisitos_y_diseno() {
    let mut tools = tools();
    let out = call(
        &mut tools,
        "spec_propose",
        r#"{"concept_id":"specs/mejor-busqueda","title":"Mejor búsqueda","requirements":"Debe soportar filtros por fecha.","design":"Añadir un índice B-tree sobre created_at.","reason":"propuesta inicial"}"#,
    );
    assert_eq!(out.get("concept_id").unwrap().as_str().unwrap(), "specs/mejor-busqueda");
    assert_eq!(out.get("created").unwrap(), &Value::Bool(true));

    let doc = call(&mut tools, "memory_resolve", r#"{"concept_id":"specs/mejor-busqueda"}"#);
    let markdown =
        doc.get("document").unwrap().get("markdown").unwrap().as_str().unwrap();
    assert!(markdown.contains("type: spec"));
    assert!(markdown.contains("status-proposed"));
    assert!(markdown.contains("## Requisitos"));
    assert!(markdown.contains("filtros por fecha"));
    assert!(markdown.contains("## Diseño"));
    assert!(markdown.contains("índice B-tree"));
}

#[test]
fn spec_tasks_crea_tareas_enlazadas_al_spec() {
    let mut tools = tools();
    call(
        &mut tools,
        "spec_propose",
        r#"{"concept_id":"specs/mejor-busqueda","title":"Mejor búsqueda","requirements":"r","design":"d"}"#,
    );

    let out = call(
        &mut tools,
        "spec_tasks",
        r#"{"spec_id":"specs/mejor-busqueda","tasks":[
            {"title":"Agregar columna nueva"},
            {"title":"Crear indice", "description":"B-tree sobre la columna nueva"}
        ]}"#,
    );
    assert_eq!(as_f64(out.get("created").unwrap()), 2.0);
    let task_ids: Vec<&str> = out
        .get("task_ids")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(
        task_ids,
        [
            "specs/mejor-busqueda/tasks/01-agregar-columna-nueva",
            "specs/mejor-busqueda/tasks/02-crear-indice",
        ]
    );

    let doc = call(
        &mut tools,
        "memory_resolve",
        r#"{"concept_id":"specs/mejor-busqueda/tasks/02-crear-indice"}"#,
    );
    let markdown =
        doc.get("document").unwrap().get("markdown").unwrap().as_str().unwrap();
    assert!(markdown.contains("status-pending"));
    assert!(markdown.contains("[[implements:specs/mejor-busqueda]]"));
    assert!(markdown.contains("B-tree sobre la columna nueva"));

    // El enlace se ve también desde el spec, vía backlinks.
    let backlinks =
        call(&mut tools, "memory_backlinks", r#"{"concept_id":"specs/mejor-busqueda"}"#);
    assert_eq!(as_f64(backlinks.get("count").unwrap()), 2.0);
}

#[test]
fn spec_tasks_falla_si_el_spec_no_existe() {
    let mut tools = tools();
    let args = json_mini::parse(
        r#"{"spec_id":"specs/no-existe","tasks":[{"title":"x"}]}"#,
    )
    .unwrap();
    assert!(tools.call("spec_tasks", &args).is_err());
}

#[test]
fn spec_status_agrega_el_progreso_de_las_tareas() {
    let mut tools = tools();
    call(
        &mut tools,
        "spec_propose",
        r#"{"concept_id":"specs/mejor-busqueda","title":"Mejor búsqueda","requirements":"r","design":"d"}"#,
    );
    call(
        &mut tools,
        "spec_tasks",
        r#"{"spec_id":"specs/mejor-busqueda","tasks":[
            {"title":"Tarea A"},
            {"title":"Tarea B"},
            {"title":"Tarea C"}
        ]}"#,
    );

    // Avanza una tarea con memory_patch: mismo mecanismo que cualquier
    // otro cambio de metadatos, sin tool nuevo para esto.
    let doc = call(
        &mut tools,
        "memory_resolve",
        r#"{"concept_id":"specs/mejor-busqueda/tasks/01-tarea-a"}"#,
    );
    let hash = doc.get("document").unwrap().get("hash").unwrap().as_str().unwrap().to_string();
    call(
        &mut tools,
        "memory_patch",
        &format!(
            r#"{{"concept_id":"specs/mejor-busqueda/tasks/01-tarea-a","expected_hash":"{hash}","remove_tags":["status-pending"],"add_tags":["status-done"],"reason":"completada"}}"#
        ),
    );

    let status = call(&mut tools, "spec_status", r#"{"spec_id":"specs/mejor-busqueda"}"#);
    assert_eq!(status.get("spec_status").unwrap().as_str().unwrap(), "proposed");
    assert_eq!(as_f64(status.get("tasks_total").unwrap()), 3.0);
    let by_status = status.get("by_status").unwrap();
    assert_eq!(as_f64(by_status.get("done").unwrap()), 1.0);
    assert_eq!(as_f64(by_status.get("pending").unwrap()), 2.0);
    assert_eq!(as_f64(status.get("progress").unwrap()), 1.0 / 3.0);
    let next: Vec<&str> = status
        .get("next_pending")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(
        next,
        [
            "specs/mejor-busqueda/tasks/02-tarea-b",
            "specs/mejor-busqueda/tasks/03-tarea-c",
        ]
    );
}

#[test]
fn spec_status_sin_tareas_da_progreso_cero() {
    let mut tools = tools();
    call(
        &mut tools,
        "spec_propose",
        r#"{"concept_id":"specs/vacio","title":"Vacío","requirements":"r","design":"d"}"#,
    );
    let status = call(&mut tools, "spec_status", r#"{"spec_id":"specs/vacio"}"#);
    assert_eq!(as_f64(status.get("tasks_total").unwrap()), 0.0);
    assert_eq!(as_f64(status.get("progress").unwrap()), 0.0);
}

fn set_status(tools: &mut MemoryTools<InMemoryStore>, concept_id: &str, new_status: &str) {
    let doc = call(tools, "memory_resolve", &format!(r#"{{"concept_id":"{concept_id}"}}"#));
    let hash = doc.get("document").unwrap().get("hash").unwrap().as_str().unwrap().to_string();
    call(
        tools,
        "memory_patch",
        &format!(
            r#"{{"concept_id":"{concept_id}","expected_hash":"{hash}","remove_tags":["status-pending","status-in_progress"],"add_tags":["status-{new_status}"],"reason":"avance"}}"#
        ),
    );
}

#[test]
fn depends_on_por_titulo_dentro_del_mismo_lote() {
    let mut tools = tools();
    call(
        &mut tools,
        "spec_propose",
        r#"{"concept_id":"specs/migracion","title":"Migración","requirements":"r","design":"d"}"#,
    );
    call(
        &mut tools,
        "spec_tasks",
        r#"{"spec_id":"specs/migracion","tasks":[
            {"title":"Crear tabla"},
            {"title":"Migrar datos", "depends_on":["Crear tabla"]}
        ]}"#,
    );

    let doc = call(
        &mut tools,
        "memory_resolve",
        r#"{"concept_id":"specs/migracion/tasks/02-migrar-datos"}"#,
    );
    let markdown = doc.get("document").unwrap().get("markdown").unwrap().as_str().unwrap();
    assert!(markdown.contains("[[depends_on:specs/migracion/tasks/01-crear-tabla]]"));

    // Con la dependencia sin terminar, "Migrar datos" no es "próxima".
    let status = call(&mut tools, "spec_status", r#"{"spec_id":"specs/migracion"}"#);
    let next: Vec<&str> = status
        .get("next_pending")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(next, ["specs/migracion/tasks/01-crear-tabla"]);
    assert_eq!(as_f64(status.get("waiting_on_dependencies").unwrap()), 1.0);

    // Al completar "Crear tabla", "Migrar datos" pasa a ser próxima.
    set_status(&mut tools, "specs/migracion/tasks/01-crear-tabla", "done");
    let status2 = call(&mut tools, "spec_status", r#"{"spec_id":"specs/migracion"}"#);
    let next2: Vec<&str> = status2
        .get("next_pending")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(next2, ["specs/migracion/tasks/02-migrar-datos"]);
    assert_eq!(as_f64(status2.get("waiting_on_dependencies").unwrap()), 0.0);
}

#[test]
fn depends_on_referencia_invalida_se_rechaza() {
    let mut tools = tools();
    call(
        &mut tools,
        "spec_propose",
        r#"{"concept_id":"specs/migracion","title":"Migración","requirements":"r","design":"d"}"#,
    );
    let args = json_mini::parse(
        r#"{"spec_id":"specs/migracion","tasks":[{"title":"X","depends_on":["no existe ni es id valido !!"]}]}"#,
    )
    .unwrap();
    assert!(tools.call("spec_tasks", &args).is_err());
}
