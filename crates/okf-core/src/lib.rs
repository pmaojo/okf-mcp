//! Parser OKF educativo: frontmatter YAML (subconjunto documentado)
//! + escáner de enlaces `[[concepto]]` en una sola pasada.
//!
//! REGLA DE ORO: los bytes originales del Markdown son la verdad.
//! Este crate solo DERIVA metadatos; jamás regenera el documento.
//! Por eso `OkfDocument` guarda offsets sobre el texto original en
//! lugar de copias reformateadas.
//!
//! Subconjunto de YAML soportado (`MiniOkfParser`):
//!
//! ```yaml
//! ---
//! type: person            # escalar obligatorio
//! title: Alice García     # escalares de una línea
//! tags:                   # listas de strings
//!   - engineering
//!   - rust
//! ---
//! ```
//!
//! Todo lo demás (anidamiento, multilínea, anclas, `|`, `>`) se
//! rechaza con un error que incluye la línea. Un parser que acepta
//! en silencio lo que no entiende corrompe datos; uno que rechaza
//! con precisión enseña el formato.
//!
//! La frontera de producción: un `ConformantOkfParser` con un crate
//! YAML maduro vivirá en `okf-yaml` (hito 2). El almacén valida con
//! ese; este subconjunto existe para aprender y para los tests.

#![forbid(unsafe_code)]

use memory_model::{Budget, ConceptId};
use std::collections::BTreeMap;
use std::fmt;

/// Valor de frontmatter soportado por el subconjunto.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FmValue {
    Scalar(String),
    List(Vec<String>),
}

/// Documento OKF analizado. `raw` no se copia: los campos derivados
/// referencian posiciones del texto original.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OkfDocument {
    /// Campo `type`, obligatorio en OKF.
    pub doc_type: String,
    /// Campo `title` si existe.
    pub title: Option<String>,
    /// Campo `tags` si existe.
    pub tags: Vec<String>,
    /// Resto de campos del frontmatter, en orden determinista.
    pub extra: BTreeMap<String, FmValue>,
    /// Offset en bytes donde empieza el cuerpo Markdown (tras `---`).
    pub body_offset: usize,
    /// Enlaces salientes `[[concepto]]`, ya validados como ConceptId.
    pub links: Vec<ConceptId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OkfError {
    /// El documento no empieza por `---\n`.
    MissingFrontmatter,
    /// No se encontró el `---` de cierre.
    UnterminatedFrontmatter,
    /// El frontmatter supera `budget.max_frontmatter_bytes`.
    FrontmatterTooLarge { len: usize, max: usize },
    /// El documento supera `budget.max_document_bytes`.
    DocumentTooLarge { len: usize, max: usize },
    /// Sintaxis no soportada por el subconjunto. `line` es 1-based.
    Unsupported { line: usize, reason: String },
    /// Clave repetida en el frontmatter.
    DuplicateKey { line: usize, key: String },
    /// Falta el campo obligatorio `type`.
    MissingType,
    /// Un enlace `[[...]]` no es un ConceptId válido. `offset` en bytes.
    InvalidLink { offset: usize, target: String },
    /// Más enlaces que `budget.max_links_per_document`.
    TooManyLinks { max: usize },
}

impl fmt::Display for OkfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OkfError::MissingFrontmatter => {
                write!(f, "el documento debe empezar con '---' en la primera línea")
            }
            OkfError::UnterminatedFrontmatter => {
                write!(f, "frontmatter sin '---' de cierre")
            }
            OkfError::FrontmatterTooLarge { len, max } => {
                write!(f, "frontmatter de {len} bytes; máximo {max}")
            }
            OkfError::DocumentTooLarge { len, max } => {
                write!(f, "documento de {len} bytes; máximo {max}")
            }
            OkfError::Unsupported { line, reason } => {
                write!(f, "línea {line}: {reason}")
            }
            OkfError::DuplicateKey { line, key } => {
                write!(f, "línea {line}: clave duplicada {key:?}")
            }
            OkfError::MissingType => write!(f, "falta el campo obligatorio 'type'"),
            OkfError::InvalidLink { offset, target } => {
                write!(f, "enlace inválido {target:?} en el byte {offset}")
            }
            OkfError::TooManyLinks { max } => {
                write!(f, "el documento supera el máximo de {max} enlaces")
            }
        }
    }
}

impl std::error::Error for OkfError {}

/// Analiza un documento OKF completo bajo un presupuesto.
pub fn parse_document(raw: &str, budget: &Budget) -> Result<OkfDocument, OkfError> {
    if raw.len() > budget.max_document_bytes {
        return Err(OkfError::DocumentTooLarge { len: raw.len(), max: budget.max_document_bytes });
    }

    let (fm_text, body_offset) = split_frontmatter(raw, budget)?;
    let fields = parse_frontmatter(fm_text)?;

    let mut doc_type = None;
    let mut title = None;
    let mut tags = Vec::new();
    let mut extra = BTreeMap::new();

    for (key, value) in fields {
        match (key.as_str(), &value) {
            ("type", FmValue::Scalar(s)) => doc_type = Some(s.clone()),
            ("type", FmValue::List(_)) => {
                return Err(OkfError::Unsupported {
                    line: 0,
                    reason: "'type' debe ser un escalar".to_string(),
                });
            }
            ("title", FmValue::Scalar(s)) => title = Some(s.clone()),
            ("tags", FmValue::List(items)) => tags = items.clone(),
            ("tags", FmValue::Scalar(_)) => {
                return Err(OkfError::Unsupported {
                    line: 0,
                    reason: "'tags' debe ser una lista".to_string(),
                });
            }
            _ => {
                extra.insert(key, value);
            }
        }
    }

    let doc_type = doc_type.ok_or(OkfError::MissingType)?;
    let links = scan_links(&raw[body_offset..], body_offset, budget)?;

    Ok(OkfDocument { doc_type, title, tags, extra, body_offset, links })
}

/// Separa el frontmatter del cuerpo. Devuelve el texto YAML (sin los
/// delimitadores) y el offset del cuerpo en `raw`.
fn split_frontmatter<'a>(raw: &'a str, budget: &Budget) -> Result<(&'a str, usize), OkfError> {
    // Aceptamos "---\n" y "---\r\n"; el delimitador debe ser la
    // primera línea EXACTA, sin espacios.
    let after_open = if let Some(rest) = raw.strip_prefix("---\n") {
        rest
    } else if let Some(rest) = raw.strip_prefix("---\r\n") {
        rest
    } else {
        return Err(OkfError::MissingFrontmatter);
    };
    let open_len = raw.len() - after_open.len();

    // Buscar la línea de cierre "---" recorriendo líneas, no con un
    // find("---") ingenuo: "---" puede aparecer dentro de un valor.
    let mut offset = 0usize; // relativo a after_open
    for line in after_open.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\n', '\r']);
        if trimmed == "---" {
            let fm_text = &after_open[..offset];
            if fm_text.len() > budget.max_frontmatter_bytes {
                return Err(OkfError::FrontmatterTooLarge {
                    len: fm_text.len(),
                    max: budget.max_frontmatter_bytes,
                });
            }
            let body_offset = open_len + offset + line.len();
            return Ok((fm_text, body_offset));
        }
        offset += line.len();
        if offset > budget.max_frontmatter_bytes {
            return Err(OkfError::FrontmatterTooLarge {
                len: offset,
                max: budget.max_frontmatter_bytes,
            });
        }
    }
    Err(OkfError::UnterminatedFrontmatter)
}

/// Parsea el subconjunto YAML línea a línea.
fn parse_frontmatter(text: &str) -> Result<Vec<(String, FmValue)>, OkfError> {
    let mut fields: Vec<(String, FmValue)> = Vec::new();
    // Clave de la lista abierta, si la línea anterior fue "key:".
    let mut open_list: Option<String> = None;

    for (idx, raw_line) in text.lines().enumerate() {
        let line_no = idx + 2; // +1 por 0-based, +1 por el '---' inicial
        let line = raw_line.trim_end();

        if line.is_empty() {
            continue;
        }
        // Comentario de línea completa.
        if line.trim_start().starts_with('#') {
            continue;
        }

        // Elemento de lista: "  - valor"
        if let Some(item) = line.trim_start().strip_prefix("- ") {
            let key = open_list.clone().ok_or_else(|| OkfError::Unsupported {
                line: line_no,
                reason: "elemento de lista sin clave previa".to_string(),
            })?;
            let value = parse_scalar(item, line_no)?;
            match fields.iter_mut().find(|(k, _)| *k == key) {
                Some((_, FmValue::List(items))) => items.push(value),
                _ => unreachable!("open_list siempre apunta a una lista existente"),
            }
            continue;
        }

        // Si la línea no es elemento de lista, la lista abierta se cierra.
        open_list = None;

        if line.starts_with(' ') || line.starts_with('\t') {
            return Err(OkfError::Unsupported {
                line: line_no,
                reason: "anidamiento no soportado por el subconjunto".to_string(),
            });
        }

        // "clave: valor" o "clave:" (inicio de lista)
        let colon = line.find(':').ok_or_else(|| OkfError::Unsupported {
            line: line_no,
            reason: "se esperaba 'clave: valor'".to_string(),
        })?;
        let key = line[..colon].trim();
        if key.is_empty()
            || !key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return Err(OkfError::Unsupported {
                line: line_no,
                reason: format!("clave inválida {key:?}"),
            });
        }
        if fields.iter().any(|(k, _)| k == key) {
            return Err(OkfError::DuplicateKey { line: line_no, key: key.to_string() });
        }

        let rest = line[colon + 1..].trim();
        if rest.is_empty() {
            // Inicio de lista.
            fields.push((key.to_string(), FmValue::List(Vec::new())));
            open_list = Some(key.to_string());
        } else {
            let value = parse_scalar(rest, line_no)?;
            fields.push((key.to_string(), FmValue::Scalar(value)));
        }
    }
    Ok(fields)
}

/// Escalares: texto plano o entre comillas dobles simples (sin
/// escapes complejos). Rechazamos la sintaxis YAML que no cubrimos.
fn parse_scalar(s: &str, line_no: usize) -> Result<String, OkfError> {
    let no_soportado = |reason: &str| OkfError::Unsupported {
        line: line_no,
        reason: reason.to_string(),
    };
    if let Some(inner) = s.strip_prefix('"') {
        let inner = inner.strip_suffix('"').ok_or_else(|| no_soportado("comillas sin cerrar"))?;
        if inner.contains('\\') || inner.contains('"') {
            return Err(no_soportado("escapes dentro de comillas no soportados"));
        }
        return Ok(inner.to_string());
    }
    for marcador in ["|", ">", "&", "*", "{", "["] {
        if s.starts_with(marcador) {
            let sugerencia = if marcador == "[" {
                " — usa lista en bloque: la clave sola en su línea, luego '  - item' en líneas propias"
            } else {
                ""
            };
            return Err(no_soportado(&format!(
                "sintaxis YAML {marcador:?} fuera del subconjunto{sugerencia}"
            )));
        }
    }
    // Cortar comentario en línea: "valor  # comentario"
    let value = match s.find(" #") {
        Some(pos) => s[..pos].trim_end(),
        None => s,
    };
    Ok(value.to_string())
}

/// Escanea enlaces `[[concepto]]` en una sola pasada, sin regex.
///
/// `base_offset` permite informar offsets absolutos sobre el
/// documento completo aunque solo escaneemos el cuerpo.
pub fn scan_links(
    body: &str,
    base_offset: usize,
    budget: &Budget,
) -> Result<Vec<ConceptId>, OkfError> {
    let bytes = body.as_bytes();
    let mut links = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut i = 0usize;
    let mut in_code_fence = false;
    let mut at_line_start = true;

    while i < bytes.len() {
        if at_line_start && bytes[i..].starts_with(b"```") {
            in_code_fence = !in_code_fence;
        }
        at_line_start = bytes[i] == b'\n';

        if !in_code_fence && bytes[i..].starts_with(b"[[") {
            let start = i + 2;
            match body[start..].find("]]") {
                Some(len) => {
                    let target = &body[start..start + len];
                    match ConceptId::parse(target) {
                        Ok(id) => {
                            if seen.insert(id.clone()) {
                                if links.len() >= budget.max_links_per_document {
                                    return Err(OkfError::TooManyLinks {
                                        max: budget.max_links_per_document,
                                    });
                                }
                                links.push(id);
                            }
                        }
                        Err(_) => {
                            return Err(OkfError::InvalidLink {
                                offset: base_offset + start,
                                target: target.to_string(),
                            });
                        }
                    }
                    i = start + len + 2;
                    continue;
                }
                None => break, // "[[" sin cerrar: no es un enlace
            }
        }
        i += 1;
    }
    Ok(links)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "---\ntype: person\ntitle: Alice García\ntags:\n  - engineering\n  - rust\nrole: staff # comentario\n---\n\nAlice trabaja con [[projects/okf-mcp]] junto a [[people/bob]].\n\n```\n[[esto/no-cuenta]]\n```\n\nY otra vez [[people/bob]].\n";

    #[test]
    fn parsea_documento_completo() {
        let doc = parse_document(DOC, &Budget::default()).unwrap();
        assert_eq!(doc.doc_type, "person");
        assert_eq!(doc.title.as_deref(), Some("Alice García"));
        assert_eq!(doc.tags, vec!["engineering", "rust"]);
        assert_eq!(
            doc.extra.get("role"),
            Some(&FmValue::Scalar("staff".to_string()))
        );
        // Enlaces: deduplicados, sin los del bloque de código.
        let links: Vec<&str> = doc.links.iter().map(|l| l.as_str()).collect();
        assert_eq!(links, vec!["projects/okf-mcp", "people/bob"]);
        // El cuerpo empieza justo tras el segundo '---\n'.
        assert!(DOC[doc.body_offset..].starts_with("\nAlice"));
    }

    #[test]
    fn exige_type() {
        let raw = "---\ntitle: Sin tipo\n---\ncuerpo\n";
        assert_eq!(parse_document(raw, &Budget::default()), Err(OkfError::MissingType));
    }

    #[test]
    fn exige_frontmatter() {
        assert_eq!(
            parse_document("solo cuerpo", &Budget::default()),
            Err(OkfError::MissingFrontmatter)
        );
        assert_eq!(
            parse_document("---\ntype: a\nsin cierre", &Budget::default()),
            Err(OkfError::UnterminatedFrontmatter)
        );
    }

    #[test]
    fn rechaza_yaml_fuera_del_subconjunto() {
        for raw in [
            "---\ntype: a\nnested:\n  key: valor\n---\n",   // anidamiento
            "---\ntype: a\ntexto: |\n  bloque\n---\n",       // bloque literal
            "---\ntype: a\ntype: b\n---\n",                  // clave duplicada
            "---\ntype: a\n- suelto\n---\n",                 // lista sin clave
        ] {
            assert!(parse_document(raw, &Budget::default()).is_err(), "aceptó: {raw:?}");
        }
    }

    #[test]
    fn rechaza_enlaces_invalidos() {
        let raw = "---\ntype: nota\n---\nver [[../etc/passwd]]\n";
        assert!(matches!(
            parse_document(raw, &Budget::default()),
            Err(OkfError::InvalidLink { .. })
        ));
    }

    #[test]
    fn respeta_presupuesto_de_enlaces() {
        let mut budget = Budget::default();
        budget.max_links_per_document = 2;
        let raw = "---\ntype: nota\n---\n[[a]] [[b]] [[c]]\n";
        assert_eq!(
            parse_document(raw, &budget),
            Err(OkfError::TooManyLinks { max: 2 })
        );
    }

    #[test]
    fn respeta_presupuesto_de_tamano() {
        let mut budget = Budget::default();
        budget.max_document_bytes = 10;
        assert!(matches!(
            parse_document("---\ntype: a\n---\ncuerpo", &budget),
            Err(OkfError::DocumentTooLarge { .. })
        ));
    }

    #[test]
    fn tres_guiones_dentro_de_valor_no_cierran() {
        let raw = "---\ntype: nota\ntitle: uso de --- en medio\n---\ncuerpo\n";
        let doc = parse_document(raw, &Budget::default()).unwrap();
        assert_eq!(doc.title.as_deref(), Some("uso de --- en medio"));
    }
}
