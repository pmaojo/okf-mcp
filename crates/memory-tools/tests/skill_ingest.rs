//! Tests de la herramienta `skill_ingest` con puertos falsos:
//! el fetcher devuelve un repo de skills en memoria y el
//! sintetizador un cuerpo fijo — nada de red, solo el contrato.

use ingest_core::{IngestError, SourceFetcher, SourceFile, Synthesizer};
use json_mini::Value;
use memory_model::{Budget, Principal};
use memory_store::InMemoryStore;
use memory_tools::MemoryTools;
use mcp_core::ToolHandler;

struct FakeFetcher(Vec<SourceFile>);

impl SourceFetcher for FakeFetcher {
    fn fetch(&self, _source: &str) -> Result<Vec<SourceFile>, IngestError> {
        Ok(self.0.clone())
    }
}

struct FakeSynthesizer;

impl Synthesizer for FakeSynthesizer {
    fn synthesize(&self, _prompt: &str) -> Result<String, IngestError> {
        Ok("Resumen original de la skill: pasos y advertencias.".to_string())
    }
}

fn skills_repo() -> Vec<SourceFile> {
    let skill = |name: &str| {
        format!("---\nname: {name}\ndescription: hace cosas\n---\n\n# {name}\n\nInstrucciones.\n")
    };
    vec![
        SourceFile {
            path: "skills/rust-kernel/SKILL.md".to_string(),
            content: skill("Rust Kernel"),
        },
        SourceFile {
            path: "skills/rust-kernel/scripts/run.sh".to_string(),
            content: "echo hola".to_string(),
        },
        SourceFile {
            path: "skills/lint-hunter/SKILL.md".to_string(),
            content: skill("Lint Hunter"),
        },
    ]
}

fn tools_with(
    fetcher: FakeFetcher,
    synthesizer: Option<Box<dyn Synthesizer>>,
) -> MemoryTools<InMemoryStore> {
    MemoryTools::new(InMemoryStore::new(), Principal::local_dev(), Budget::default())
        .with_ingest(Box::new(fetcher), synthesizer)
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
fn sin_fetcher_no_se_anuncia_ni_funciona() {
    let mut tools =
        MemoryTools::new(InMemoryStore::new(), Principal::local_dev(), Budget::default());
    assert!(tools.tools().iter().all(|t| t.name != "skill_ingest"));
    let args = json_mini::parse(r#"{"source":"o/r","path_prefix":"skills"}"#).unwrap();
    let err = tools.call("skill_ingest", &args).unwrap_err();
    let msg = format!("{err:?}");
    assert!(msg.contains("ingest_unavailable"), "error inesperado: {msg}");
}

#[test]
fn con_fetcher_se_anuncia() {
    let tools = tools_with(FakeFetcher(skills_repo()), Some(Box::new(FakeSynthesizer)));
    assert!(tools.tools().iter().any(|t| t.name == "skill_ingest"));
}

#[test]
fn dry_run_devuelve_el_plan_sin_escribir() {
    let mut tools = tools_with(FakeFetcher(skills_repo()), Some(Box::new(FakeSynthesizer)));
    let out = call(
        &mut tools,
        "skill_ingest",
        r#"{"source":"udapy/rust-agentic-skills","path_prefix":"skills/programming","dry_run":true}"#,
    );
    assert_eq!(out.get("format").unwrap().as_str().unwrap(), "agentic-skills");
    assert_eq!(out.get("dry_run").unwrap(), &Value::Bool(true));
    let units = out.get("units").unwrap().as_array().unwrap();
    assert_eq!(units.len(), 2);
    // Sin `synthesize: true`, el plan anuncia conversión determinista.
    assert!(units
        .iter()
        .all(|u| u.get("action").unwrap().as_str().unwrap() == "convert-verbatim"));

    // Nada persistido.
    let list = call(&mut tools, "memory_list", r#"{"path_prefix":"skills"}"#);
    assert_eq!(as_f64(list.get("count").unwrap()), 0.0);
}

#[test]
fn ingesta_con_sintesis_crea_conceptos_sintetizados() {
    let mut tools = tools_with(FakeFetcher(skills_repo()), Some(Box::new(FakeSynthesizer)));
    let out = call(
        &mut tools,
        "skill_ingest",
        r#"{"source":"udapy/rust-agentic-skills","path_prefix":"skills/programming","synthesize":true}"#,
    );
    assert_eq!(as_f64(out.get("ingested").unwrap()), 2.0);
    let ids: Vec<&str> = out
        .get("concept_ids")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(ids, ["skills/programming/lint-hunter", "skills/programming/rust-kernel"]);
    let items = out.get("items").unwrap().as_array().unwrap();
    assert!(items
        .iter()
        .all(|i| i.get("mode").unwrap().as_str().unwrap() == "synthesized"));

    // El documento guardado es la síntesis envuelta, no el material.
    let doc = call(
        &mut tools,
        "memory_resolve",
        r#"{"concept_id":"skills/programming/rust-kernel"}"#,
    );
    let markdown = doc
        .get("document")
        .unwrap()
        .get("markdown")
        .unwrap()
        .as_str()
        .unwrap();
    assert!(markdown.contains("type: skill"));
    assert!(markdown.contains("  - synthesized"));
    assert!(markdown.contains("Resumen original de la skill"));
    assert!(!markdown.contains("Instrucciones."));
}

#[test]
fn por_defecto_conserva_el_contenido_integro() {
    // Aunque haya sintetizador configurado, sin `synthesize: true` la
    // conversión es determinista: el cuerpo del SKILL.md y sus
    // archivos auxiliares se conservan íntegros.
    let mut tools = tools_with(FakeFetcher(skills_repo()), Some(Box::new(FakeSynthesizer)));
    let out = call(
        &mut tools,
        "skill_ingest",
        r#"{"source":"udapy/rust-agentic-skills","path_prefix":"skills/programming"}"#,
    );
    assert_eq!(as_f64(out.get("ingested").unwrap()), 2.0);
    let items = out.get("items").unwrap().as_array().unwrap();
    assert!(items
        .iter()
        .all(|i| i.get("mode").unwrap().as_str().unwrap() == "verbatim"));

    let doc = call(
        &mut tools,
        "memory_resolve",
        r#"{"concept_id":"skills/programming/rust-kernel"}"#,
    );
    let markdown = doc
        .get("document")
        .unwrap()
        .get("markdown")
        .unwrap()
        .as_str()
        .unwrap();
    assert!(markdown.contains("  - verbatim-import"));
    assert!(markdown.contains("Instrucciones."));
    assert!(markdown.contains("scripts/run.sh"));
    assert!(markdown.contains("echo hola"));
}

#[test]
fn sintesis_sin_proveedor_falla_claro() {
    let mut tools = tools_with(FakeFetcher(skills_repo()), None);
    let args = json_mini::parse(
        r#"{"source":"udapy/rust-agentic-skills","path_prefix":"skills","synthesize":true}"#,
    )
    .unwrap();
    let err = tools.call("skill_ingest", &args).unwrap_err();
    let msg = format!("{err:?}");
    assert!(msg.contains("synthesis_unavailable"), "error inesperado: {msg}");
}

#[test]
fn reingesta_no_pisa_lo_existente() {
    let mut tools = tools_with(FakeFetcher(skills_repo()), Some(Box::new(FakeSynthesizer)));
    let primera = call(
        &mut tools,
        "skill_ingest",
        r#"{"source":"udapy/rust-agentic-skills","path_prefix":"skills/programming"}"#,
    );
    assert_eq!(as_f64(primera.get("ingested").unwrap()), 2.0);

    // Segunda pasada: misma conversión determinista → idempotente
    // (no_change), no un conflicto.
    let segunda = call(
        &mut tools,
        "skill_ingest",
        r#"{"source":"udapy/rust-agentic-skills","path_prefix":"skills/programming"}"#,
    );
    assert_eq!(as_f64(segunda.get("ingested").unwrap()), 2.0);
    let items = segunda.get("items").unwrap().as_array().unwrap();
    assert!(items
        .iter()
        .all(|i| i.get("created").unwrap() == &Value::Bool(false)));
}

#[test]
fn formato_okf_se_ingiere_verbatim() {
    let files = vec![SourceFile {
        path: "notas/alice.md".to_string(),
        content: "---\ntype: person\ntitle: Alice\n---\nHola.\n".to_string(),
    }];
    let mut tools = tools_with(FakeFetcher(files), None);
    let out = call(
        &mut tools,
        "skill_ingest",
        r#"{"source":"https://github.com/o/r","path_prefix":"importado"}"#,
    );
    assert_eq!(out.get("format").unwrap().as_str().unwrap(), "okf");
    assert_eq!(as_f64(out.get("ingested").unwrap()), 1.0);
    let doc = call(&mut tools, "memory_resolve", r#"{"concept_id":"importado/notas/alice"}"#);
    let markdown = doc
        .get("document")
        .unwrap()
        .get("markdown")
        .unwrap()
        .as_str()
        .unwrap();
    assert_eq!(markdown, "---\ntype: person\ntitle: Alice\n---\nHola.\n");
}

#[test]
fn argumentos_invalidos_se_rechazan() {
    let mut tools = tools_with(FakeFetcher(skills_repo()), None);
    for args in [
        r#"{"path_prefix":"skills"}"#,
        r#"{"source":"o/r"}"#,
        r#"{"source":"o/r","path_prefix":"Skills Mayúsculas"}"#,
        r#"{"source":"o/r","path_prefix":"skills","format":"magic"}"#,
    ] {
        let args = json_mini::parse(args).unwrap();
        assert!(tools.call("skill_ingest", &args).is_err(), "debía fallar: {args:?}");
    }
}
