//! Tests de `memory_distill` sobre `InMemoryStore`: determinismo,
//! idempotencia, paginación (más episodios que `max_search_results`),
//! poda por borrado lógico y confinamiento a `facts/` y `runs/`.

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

fn episode(tools: &mut MemoryTools<InMemoryStore>, n: usize, ok: bool) {
    let md = format!(
        "---\\ntype: episode\\ntitle: intento {n}\\nskill: manipulation.grasp\\ntarget: glass_cup\\ncontext: sim\\noutcome: {}\\n{}---\\ncuerpo\\n",
        if ok { "success" } else { "failure" },
        if ok { "" } else { "failure_reason: slip\\n" },
    );
    let args = format!(
        r#"{{"concept_id":"episodes/e{n:05}","markdown":"{md}","reason":"seed"}}"#
    );
    call(tools, "memory_commit", &args);
}

fn num(v: &Value, k: &str) -> u64 {
    v.get(k).unwrap().as_f64().unwrap() as u64
}

#[test]
fn se_anuncia_sin_ui() {
    let t = tools();
    let spec = t.tools().into_iter().find(|s| s.name == "memory_distill").unwrap();
    assert!(spec.ui_resource_uri.is_none());
}

/// V1 del paper: 120 episodios (más que `max_search_results` = 50, así
/// que la paginación es obligatoria) con 80 % de éxito.
#[test]
fn agrega_mas_episodios_que_una_pagina() {
    let mut t = tools();
    for i in 0..120 {
        episode(&mut t, i, i % 5 != 0);
    }
    let out = call(&mut t, "memory_distill", "{}");
    assert_eq!(num(&out, "episodes_used"), 120);
    assert_eq!(num(&out, "episodes_skipped"), 0);

    let doc = call(
        &mut t,
        "memory_resolve",
        r#"{"concept_id":"facts/skill_success_rate/manipulation-grasp/glass_cup/sim"}"#,
    );
    let md = doc.get("document").unwrap().get("markdown").unwrap().as_str().unwrap();
    assert!(md.contains("n_obs: 120"), "{md}");
    assert!(md.contains("successes: 96"), "{md}");
    assert!(md.contains("rate: 0.800000"), "{md}");
    assert!(md.contains("rule_version: v2"), "{md}");
    // confidence = n/(n+10) = 120/130; separation = (2·q−1)² con q = (96+5)/130.
    assert!(md.contains("confidence: 0.923077"), "{md}");
    assert!(md.contains("separation: 0.306746"), "{md}");
}

/// Memoria sin episodios: la pasada no escribe nada, ni siquiera un run.
#[test]
fn memoria_vacia_no_escribe_ni_un_run() {
    let mut t = tools();
    let out = call(&mut t, "memory_distill", "{}");
    assert_eq!(num(&out, "facts"), 0);
    assert_eq!(num(&out, "created"), 0);
    assert_eq!(num(&out, "unchanged"), 0);
    assert_eq!(out.get("run"), Some(&Value::Null));
    assert_eq!(num(&call(&mut t, "memory_list", r#"{"path_prefix":"runs"}"#), "count"), 0);
}

/// Un hecho y un run borrados lógicamente se RECREAN en la pasada
/// siguiente (contrato de store: `expected = None` sobre un borrado
/// lo recrea y continúa la historia), sin conflicto ni error.
#[test]
fn recrea_hechos_y_runs_borrados_logicamente() {
    let mut t = tools();
    for i in 0..4 {
        episode(&mut t, i, i % 2 == 0);
    }
    let first = call(&mut t, "memory_distill", "{}");
    let run = first.get("run").unwrap().as_str().unwrap().to_string();
    let fact = "facts/skill_success_rate/manipulation-grasp/glass_cup/sim";

    for id in [run.as_str(), fact] {
        let doc = call(&mut t, "memory_resolve", &format!(r#"{{"concept_id":"{id}"}}"#));
        let hash = doc.get("document").unwrap().get("hash").unwrap().as_str().unwrap().to_string();
        call(
            &mut t,
            "memory_delete",
            &format!(r#"{{"concept_id":"{id}","expected_hash":"{hash}","reason":"prueba"}}"#),
        );
    }

    let again = call(&mut t, "memory_distill", "{}");
    assert!(again.get("failed").unwrap().as_array().unwrap().is_empty());
    assert_eq!(num(&again, "created"), 2, "hecho y run vuelven a existir");
    assert_eq!(again.get("run").unwrap().as_str().unwrap(), run, "mismo id de run");
    let doc = call(&mut t, "memory_resolve", &format!(r#"{{"concept_id":"{fact}"}}"#));
    assert!(doc.get("document").unwrap().get("markdown").unwrap().as_str().unwrap().contains("n_obs: 4"));
}

#[test]
fn interaction_pattern_no_tiene_separation_y_usa_la_misma_confianza() {
    let mut t = tools();
    for i in 0..5 {
        episode(&mut t, i, false);
    }
    call(&mut t, "memory_distill", "{}");
    let doc = call(
        &mut t,
        "memory_resolve",
        r#"{"concept_id":"facts/interaction_pattern/manipulation-grasp/glass_cup/slip"}"#,
    );
    let md = doc.get("document").unwrap().get("markdown").unwrap().as_str().unwrap();
    assert!(md.contains("confidence: 0.333333"), "5/(5+10): {md}");
    assert!(!md.contains("separation:"), "{md}");
}

#[test]
fn segunda_pasada_no_escribe_nada() {
    let mut t = tools();
    for i in 0..30 {
        episode(&mut t, i, i % 3 != 0);
    }
    let first = call(&mut t, "memory_distill", "{}");
    assert!(num(&first, "created") > 0);
    let second = call(&mut t, "memory_distill", "{}");
    assert_eq!(num(&second, "created"), 0);
    assert_eq!(num(&second, "updated"), 0);
    assert_eq!(num(&second, "pruned"), 0);
    assert_eq!(num(&second, "unchanged"), num(&first, "facts") + 1, "hechos + run");
    assert_eq!(first.get("run"), second.get("run"), "mismo run: pasada idéntica");
}

#[test]
fn dry_run_no_escribe() {
    let mut t = tools();
    for i in 0..5 {
        episode(&mut t, i, true);
    }
    let out = call(&mut t, "memory_distill", r#"{"dry_run":true}"#);
    assert!(num(&out, "created") > 0);
    let facts = call(&mut t, "memory_list", r#"{"path_prefix":"facts"}"#);
    assert_eq!(num(&facts, "count"), 0);
    let runs = call(&mut t, "memory_list", r#"{"path_prefix":"runs"}"#);
    assert_eq!(num(&runs, "count"), 0);
}

/// R6: los conceptos curados quedan byte-iguales tras varias pasadas.
#[test]
fn no_toca_nada_fuera_de_facts_y_runs() {
    let mut t = tools();
    call(&mut t, "memory_commit", r#"{"concept_id":"people/alice","markdown":"---\ntype: person\ntitle: Alice\n---\nhola\n","reason":"seed"}"#);
    call(&mut t, "memory_commit", r#"{"concept_id":"skills/grasp","markdown":"---\ntype: skill\ntitle: Grasp\ntags:\n  - skill\n---\nhow to\n","reason":"seed"}"#);
    let before_alice = call(&mut t, "memory_resolve", r#"{"concept_id":"people/alice"}"#);
    let before_skill = call(&mut t, "memory_resolve", r#"{"concept_id":"skills/grasp"}"#);
    for i in 0..10 {
        episode(&mut t, i, i % 2 == 0);
    }
    for _ in 0..3 {
        call(&mut t, "memory_distill", "{}");
    }
    let after_alice = call(&mut t, "memory_resolve", r#"{"concept_id":"people/alice"}"#);
    let after_skill = call(&mut t, "memory_resolve", r#"{"concept_id":"skills/grasp"}"#);
    assert_eq!(before_alice.get("document").unwrap().get("hash"), after_alice.get("document").unwrap().get("hash"));
    assert_eq!(before_skill.get("document").unwrap().get("hash"), after_skill.get("document").unwrap().get("hash"));

    let all = call(&mut t, "memory_list", "{}");
    for r in all.get("results").unwrap().as_array().unwrap() {
        let id = r.get("concept_id").unwrap().as_str().unwrap();
        let ty = r.get("type").unwrap().as_str().unwrap();
        if ty == "semantic-fact" || ty == "consolidation-run" {
            assert!(id.starts_with("facts/") || id.starts_with("runs/"), "{id}");
        }
    }
}

#[test]
fn episodios_sin_estructura_se_ignoran() {
    let mut t = tools();
    episode(&mut t, 1, true);
    call(&mut t, "memory_commit", r#"{"concept_id":"episodes/libre","markdown":"---\ntype: episode\ntitle: texto libre\n---\nme fue regular\n","reason":"seed"}"#);
    let out = call(&mut t, "memory_distill", "{}");
    assert_eq!(num(&out, "episodes_used"), 1);
    assert_eq!(num(&out, "episodes_skipped"), 1);
}

/// Si se borran todos los episodios de un grupo, `prune` retira su
/// hecho (borrado lógico) y deja de aparecer en la búsqueda.
#[test]
fn poda_hechos_sin_soporte() {
    let mut t = tools();
    episode(&mut t, 1, true);
    episode(&mut t, 2, false);
    call(&mut t, "memory_distill", "{}");
    let before = call(&mut t, "memory_search", r#"{"type":"semantic-fact"}"#);
    assert!(num(&before, "count") > 0);

    for id in ["episodes/e00001", "episodes/e00002"] {
        let doc = call(&mut t, "memory_resolve", &format!(r#"{{"concept_id":"{id}"}}"#));
        let hash = doc.get("document").unwrap().get("hash").unwrap().as_str().unwrap().to_string();
        call(
            &mut t,
            "memory_delete",
            &format!(r#"{{"concept_id":"{id}","expected_hash":"{hash}","reason":"limpieza"}}"#),
        );
    }
    let out = call(&mut t, "memory_distill", "{}");
    assert!(num(&out, "pruned") > 0);
    let after = call(&mut t, "memory_search", r#"{"type":"semantic-fact"}"#);
    assert_eq!(num(&after, "count"), 0, "los borrados quedan excluidos de la búsqueda");
}

#[test]
fn rechaza_prior_invalido() {
    let mut t = tools();
    let args = json_mini::parse(r#"{"prior_mean": 2}"#).unwrap();
    assert!(t.call("memory_distill", &args).is_err());
}

#[test]
fn trazabilidad_hecho_a_episodios_por_backlinks() {
    let mut t = tools();
    episode(&mut t, 7, true);
    call(&mut t, "memory_distill", "{}");
    let back = call(&mut t, "memory_backlinks", r#"{"concept_id":"episodes/e00007"}"#);
    let rels: Vec<String> = back
        .get("backlinks")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b.get("source").unwrap().get("concept_id").unwrap().as_str().unwrap().to_string())
        .collect();
    assert!(rels.iter().any(|r| r.starts_with("facts/skill_success_rate/")), "{rels:?}");
}
