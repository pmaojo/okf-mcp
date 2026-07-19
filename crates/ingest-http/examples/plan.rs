//! Prueba manual del camino completo de `skill_ingest` sin servidor:
//! descarga una fuente real y muestra el plan que se commitearía.
//!
//! ```bash
//! cargo run -p ingest-http --example plan -- anthropics/skills skills/importadas
//! ```

use ingest_core::{plan_ingest, PlannedAction, SkillFormat, SourceFetcher};
use memory_model::Budget;

fn main() {
    let mut args = std::env::args().skip(1);
    let source = args.next().unwrap_or_else(|| "anthropics/skills".to_string());
    let prefix = args.next().unwrap_or_else(|| "skills/importadas".to_string());

    let fetcher = ingest_http::GithubFetcher::from_env();
    let files = match fetcher.fetch(&source) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("fetch falló: {e}");
            std::process::exit(1);
        }
    };
    println!("archivos descargados: {}", files.len());
    for f in &files {
        println!("  - {} ({} bytes)", f.path, f.content.len());
    }

    let license = fetcher.license_spdx_id(&source).unwrap_or(None);
    let plan = match plan_ingest(
        &source,
        &files,
        SkillFormat::Auto,
        &prefix,
        &Budget::default(),
        license.as_deref(),
    ) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("plan falló: {e}");
            std::process::exit(1);
        }
    };
    println!("\nlicencia detectada: {}", license.as_deref().unwrap_or("(ninguna)"));
    println!("formato detectado: {}", plan.format.as_str());
    println!("unidades: {}", plan.units.len());
    for u in &plan.units {
        let (accion, bytes) = match &u.action {
            PlannedAction::Commit { markdown } => ("commit-okf", markdown.len()),
            PlannedAction::Convert { deterministic } => ("convert-verbatim", deterministic.len()),
        };
        println!("  - {} [{}] «{}» ({} bytes)", u.concept_id, accion, u.title, bytes);
        for w in &u.warnings {
            println!("      ⚠ {w}");
        }
    }
    if !plan.skipped.is_empty() {
        println!("descartados: {}", plan.skipped.len());
        for (item, reason) in &plan.skipped {
            println!("  - {item}: {reason}");
        }
    }
}
