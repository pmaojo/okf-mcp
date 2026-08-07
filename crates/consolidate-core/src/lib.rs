//! Consolidación de sesión: convertir lo que un agente hizo durante
//! una sesión en un concepto OKF durable, sin que ningún LLM del lado
//! del servidor escriba markdown ni frontmatter.
//!
//! Mismo reparto de responsabilidades que `memory_commit` ya tiene:
//! quien redacta el contenido es el agente que llama a la
//! herramienta (ya es un LLM, con todo el contexto de la sesión) —
//! este crate solo VALIDA esa estructura y la RENDERIZA de forma
//! determinista a OKF. Ningún campo llega a bytes de frontmatter sin
//! pasar por [`render_digest`], y el resultado se re-valida con
//! [`okf_core::parse_document`] antes de devolverse: si esta función
//! devuelve `Ok`, el markdown ya es un documento OKF aceptable para
//! `MemoryRepository::commit`.
//!
//! Por qué un `SessionDigest` tipado en vez de markdown crudo del
//! agente (que también podría llamar directamente a `memory_commit`):
//! forzar campos separados (título, resumen, entidades, decisiones)
//! es lo que permite sanear cada uno con la regla que le corresponde
//! — un `concept_id` de entidad pasa por [`memory_model::ConceptId::parse`],
//! que ya rechaza *path traversal* y caracteres fuera de lista blanca
//! — en vez de fiarse de que el agente jamás cometa un error de
//! sintaxis YAML en medio de un resumen largo.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use memory_model::{Budget, ConceptId};
use std::fmt;

/// Una entidad (concepto ya existente o a crear) tocada durante la
/// sesión, con su relación en prosa libre si el agente la da.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DigestEntity {
    /// El concepto relacionado.
    pub concept_id: ConceptId,
    /// Relación en prosa libre, p. ej. "revisó", "creó por primera vez".
    pub relation: Option<String>,
}

/// Una decisión tomada durante la sesión, opcionalmente enlazada al
/// concepto que la registra o que resultó de ella.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DigestDecision {
    /// Texto de la decisión, en prosa libre.
    pub text: String,
    /// Concepto relacionado, si aplica.
    pub concept_id: Option<ConceptId>,
}

/// La estructura completa que el agente entrega para consolidar una
/// sesión. Ningún campo es markdown: son datos, el renderizado a OKF
/// es responsabilidad exclusiva de [`render_digest`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionDigest {
    /// Título corto de la sesión.
    pub title: String,
    /// Resumen en prosa de lo ocurrido. Puede contener `[[enlaces]]`
    /// OKF normales: son cuerpo del documento, no frontmatter, así
    /// que [`okf_core::scan_links`] los valida igual que en cualquier
    /// otro commit.
    pub summary: String,
    /// Entidades relacionadas con la sesión.
    pub entities: Vec<DigestEntity>,
    /// Decisiones tomadas durante la sesión.
    pub decisions: Vec<DigestDecision>,
}

/// Por qué [`render_digest`] rechazó un `SessionDigest`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsolidateError {
    /// `title` está vacío (o solo espacios).
    EmptyTitle,
    /// `summary` está vacío (o solo espacios).
    EmptySummary,
    /// El documento renderizado no pasó [`okf_core::parse_document`]
    /// (p. ej. supera `budget.max_document_bytes`, o un enlace roto
    /// de sintaxis).
    Okf(okf_core::OkfError),
}

impl fmt::Display for ConsolidateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConsolidateError::EmptyTitle => write!(f, "el título está vacío"),
            ConsolidateError::EmptySummary => write!(f, "el resumen está vacío"),
            ConsolidateError::Okf(e) => write!(f, "documento OKF inválido: {e}"),
        }
    }
}

impl std::error::Error for ConsolidateError {}

/// Sanea un valor de una sola línea (título, texto de lista): colapsa
/// saltos de línea y espacios a uno solo, recorta bordes. A
/// diferencia de un escalar de frontmatter no hace falta rechazar
/// `"`/`#`/marcadores YAML aquí — estos campos nunca se escriben tal
/// cual en la cabecera, solo en el cuerpo o (title) en un escalar ya
/// saneado aparte.
fn collapse_line(raw: &str) -> String {
    let mut out = String::new();
    let mut prev_space = false;
    for ch in raw.chars() {
        let c = if ch.is_control() { ' ' } else { ch };
        if c == ' ' {
            if !prev_space {
                out.push(' ');
            }
            prev_space = true;
        } else {
            out.push(c);
            prev_space = false;
        }
    }
    out.trim().to_string()
}

/// Sanea un valor para usarlo como escalar de frontmatter (el
/// `title:` del documento): sin saltos de línea, sin `"` ni `#`, sin
/// empezar por un marcador YAML rechazado. Mismo criterio que
/// `ingest_core::finish_document` aplica a su propio `title`.
fn sanitize_scalar(raw: &str) -> String {
    let collapsed = collapse_line(raw);
    let mut out: String = collapsed.chars().filter(|c| *c != '"' && *c != '#').collect();
    while let Some(rest) = out
        .clone()
        .strip_prefix(['|', '>', '&', '*', '{', '['])
        .map(|s| s.trim_start().to_string())
    {
        out = rest;
    }
    out
}

/// Renderiza un `SessionDigest` a un documento OKF completo
/// (`type: session-summary`) y lo valida con
/// [`okf_core::parse_document`] antes de devolverlo.
///
/// ```
/// use consolidate_core::{render_digest, SessionDigest, DigestDecision, DigestEntity};
/// use memory_model::{Budget, ConceptId};
///
/// let digest = SessionDigest {
///     title: "Sesión del 7 de agosto".to_string(),
///     summary: "Diseñamos memory_consolidate junto a [[projects/okf-mcp]].".to_string(),
///     entities: vec![DigestEntity {
///         concept_id: ConceptId::parse("projects/okf-mcp").unwrap(),
///         relation: Some("se amplió".to_string()),
///     }],
///     decisions: vec![DigestDecision {
///         text: "Consolidar sin LLM en el servidor".to_string(),
///         concept_id: None,
///     }],
/// };
/// let markdown = render_digest(&digest, &Budget::default()).unwrap();
/// assert!(markdown.starts_with("---\ntype: session-summary\n"));
/// ```
pub fn render_digest(digest: &SessionDigest, budget: &Budget) -> Result<String, ConsolidateError> {
    if digest.title.trim().is_empty() {
        return Err(ConsolidateError::EmptyTitle);
    }
    if digest.summary.trim().is_empty() {
        return Err(ConsolidateError::EmptySummary);
    }

    let mut out = String::new();
    out.push_str("---\n");
    out.push_str("type: session-summary\n");
    out.push_str(&format!("title: {}\n", sanitize_scalar(&digest.title)));
    out.push_str("tags:\n  - consolidation\n");
    out.push_str("---\n");
    out.push_str(digest.summary.trim());
    out.push('\n');

    if !digest.decisions.is_empty() {
        out.push_str("\n## Decisiones\n");
        for d in &digest.decisions {
            let text = collapse_line(&d.text);
            match &d.concept_id {
                Some(id) => out.push_str(&format!("- {text} → [[{}]]\n", id.as_str())),
                None => out.push_str(&format!("- {text}\n")),
            }
        }
    }

    if !digest.entities.is_empty() {
        out.push_str("\n## Entidades relacionadas\n");
        for e in &digest.entities {
            match &e.relation {
                Some(r) => out.push_str(&format!("- [[{}]] ({})\n", e.concept_id.as_str(), collapse_line(r))),
                None => out.push_str(&format!("- [[{}]]\n", e.concept_id.as_str())),
            }
        }
    }

    okf_core::parse_document(&out, budget).map_err(ConsolidateError::Okf)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(title: &str, summary: &str) -> SessionDigest {
        SessionDigest {
            title: title.to_string(),
            summary: summary.to_string(),
            entities: Vec::new(),
            decisions: Vec::new(),
        }
    }

    #[test]
    fn renders_minimal_digest_as_valid_okf() {
        let d = digest("Sesión mínima", "No pasó nada memorable.");
        let markdown = render_digest(&d, &Budget::default()).unwrap();
        let parsed = okf_core::parse_document(&markdown, &Budget::default()).unwrap();
        assert_eq!(parsed.doc_type, "session-summary");
        assert_eq!(parsed.title.as_deref(), Some("Sesión mínima"));
    }

    #[test]
    fn empty_title_is_rejected() {
        let d = digest("   ", "algo pasó");
        assert_eq!(render_digest(&d, &Budget::default()), Err(ConsolidateError::EmptyTitle));
    }

    #[test]
    fn empty_summary_is_rejected() {
        let d = digest("título", "  ");
        assert_eq!(render_digest(&d, &Budget::default()), Err(ConsolidateError::EmptySummary));
    }

    #[test]
    fn title_with_yaml_breaking_chars_is_sanitized_not_rejected() {
        let d = digest("Título \"con\" comillas # y salto\nde línea", "resumen");
        let markdown = render_digest(&d, &Budget::default()).unwrap();
        let parsed = okf_core::parse_document(&markdown, &Budget::default()).unwrap();
        assert_eq!(parsed.title.as_deref(), Some("Título con comillas  y salto de línea"));
    }

    #[test]
    fn decisions_and_entities_render_as_links() {
        let entity_id = ConceptId::parse("projects/okf-mcp").unwrap();
        let decision_id = ConceptId::parse("people/alice").unwrap();
        let d = SessionDigest {
            title: "Sesión con enlaces".to_string(),
            summary: "resumen".to_string(),
            entities: vec![DigestEntity { concept_id: entity_id.clone(), relation: Some("se revisó".to_string()) }],
            decisions: vec![DigestDecision { text: "avisar a alice".to_string(), concept_id: Some(decision_id.clone()) }],
        };
        let markdown = render_digest(&d, &Budget::default()).unwrap();
        assert!(markdown.contains("[[projects/okf-mcp]] (se revisó)"));
        assert!(markdown.contains("avisar a alice → [[people/alice]]"));
        let parsed = okf_core::parse_document(&markdown, &Budget::default()).unwrap();
        let linked: Vec<&str> = parsed.links.iter().map(|l| l.target.as_str()).collect();
        assert!(linked.contains(&entity_id.as_str()));
        assert!(linked.contains(&decision_id.as_str()));
    }

    #[test]
    fn oversized_summary_is_rejected_via_okf_budget() {
        let budget = Budget { max_document_bytes: 64, ..Budget::default() };
        let d = digest("t", &"x".repeat(200));
        assert!(matches!(render_digest(&d, &budget), Err(ConsolidateError::Okf(_))));
    }
}
