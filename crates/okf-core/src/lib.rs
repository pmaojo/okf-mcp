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
#![warn(missing_docs)]

use memory_model::{Budget, ConceptId};
use std::collections::BTreeMap;
use std::fmt;

/// Valor de frontmatter soportado por el subconjunto.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FmValue {
    /// Escalar de una línea: `title: Alice García`.
    Scalar(String),
    /// Lista en bloque: `tags:` seguido de líneas `  - item`.
    List(Vec<String>),
}

/// Longitud máxima de una relación de enlace (`[[rel:destino]]`).
pub const MAX_LINK_REL_LEN: usize = 32;

/// Enlace saliente de un documento: destino validado y relación
/// tipada opcional.
///
/// La sintaxis `[[people/alice]]` produce `rel = None` (enlace
/// genérico); `[[depends_on:people/alice]]` produce
/// `rel = Some("depends_on")`. El separador `:` es inequívoco:
/// [`ConceptId`] lo prohíbe por lista blanca, así que ningún enlace
/// antiguo cambia de significado.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    /// El concepto destino, ya validado.
    pub target: ConceptId,
    /// Relación tipada (`depends_on`, `supersedes`, `related`, …) o
    /// `None` para el enlace genérico de siempre.
    pub rel: Option<String>,
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
    /// Enlaces salientes `[[concepto]]` o `[[rel:concepto]]`, ya
    /// validados (ver [`Link`]).
    pub links: Vec<Link>,
}

/// Por qué un documento no es OKF válido. Siempre con línea u
/// offset: rechazar sin decir dónde no enseña el formato.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OkfError {
    /// El documento no empieza por `---\n`.
    MissingFrontmatter,
    /// No se encontró el `---` de cierre.
    UnterminatedFrontmatter,
    /// El frontmatter supera `budget.max_frontmatter_bytes`.
    FrontmatterTooLarge {
        /// Bytes de frontmatter encontrados (o cota inferior).
        len: usize,
        /// El máximo permitido por el presupuesto.
        max: usize,
    },
    /// El documento supera `budget.max_document_bytes`.
    DocumentTooLarge {
        /// Bytes del documento recibido.
        len: usize,
        /// El máximo permitido por el presupuesto.
        max: usize,
    },
    /// Sintaxis no soportada por el subconjunto.
    Unsupported {
        /// Línea del problema, 1-based sobre el documento completo.
        line: usize,
        /// Qué sintaxis se rechazó y, si procede, la alternativa.
        reason: String,
    },
    /// Clave repetida en el frontmatter.
    DuplicateKey {
        /// Línea de la segunda aparición, 1-based.
        line: usize,
        /// La clave duplicada.
        key: String,
    },
    /// Falta el campo obligatorio `type`.
    MissingType,
    /// Un enlace `[[...]]` no es un ConceptId válido.
    InvalidLink {
        /// Offset en bytes del enlace sobre el documento completo.
        offset: usize,
        /// El destino inválido, tal cual aparece entre corchetes.
        target: String,
    },
    /// Más enlaces que `budget.max_links_per_document`.
    TooManyLinks {
        /// El máximo permitido por el presupuesto.
        max: usize,
    },
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
///
/// # Ejemplo
///
/// ```
/// use memory_model::Budget;
///
/// let raw = "---\ntype: person\ntitle: Alice García\ntags:\n  - rust\n---\nTrabaja con [[people/bob]].\n";
/// let doc = okf_core::parse_document(raw, &Budget::default())?;
///
/// assert_eq!(doc.doc_type, "person");
/// assert_eq!(doc.title.as_deref(), Some("Alice García"));
/// assert_eq!(doc.tags, vec!["rust"]);
/// assert_eq!(doc.links[0].target.as_str(), "people/bob");
/// assert_eq!(doc.links[0].rel, None);
/// // Los bytes originales siguen siendo la verdad: el cuerpo se
/// // recupera por offset, nunca reformateado.
/// assert_eq!(&raw[doc.body_offset..], "Trabaja con [[people/bob]].\n");
/// # Ok::<(), okf_core::OkfError>(())
/// ```
///
/// # Errores
///
/// Lo que el subconjunto no entiende se rechaza con línea u offset,
/// nunca se acepta en silencio:
///
/// ```
/// use memory_model::Budget;
/// use okf_core::{parse_document, OkfError};
///
/// assert_eq!(
///     parse_document("sin frontmatter", &Budget::default()).unwrap_err(),
///     OkfError::MissingFrontmatter,
/// );
/// ```
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

/// Escanea enlaces `[[concepto]]` y `[[rel:concepto]]` en una sola
/// pasada, sin regex.
///
/// `base_offset` permite informar offsets absolutos sobre el
/// documento completo aunque solo escaneemos el cuerpo.
///
/// Cada destino aparece UNA vez en el resultado: la primera
/// aparición gana, incluida su relación. `[[uses:a]] … [[a]]` da un
/// único enlace a `a` con `rel = Some("uses")`.
pub fn scan_links(
    body: &str,
    base_offset: usize,
    budget: &Budget,
) -> Result<Vec<Link>, OkfError> {
    let bytes = body.as_bytes();
    let mut links: Vec<Link> = Vec::new();
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
                    let raw_target = &body[start..start + len];
                    match parse_link(raw_target) {
                        Some(link) => {
                            if seen.insert(link.target.clone()) {
                                if links.len() >= budget.max_links_per_document {
                                    return Err(OkfError::TooManyLinks {
                                        max: budget.max_links_per_document,
                                    });
                                }
                                links.push(link);
                            }
                        }
                        None => {
                            return Err(OkfError::InvalidLink {
                                offset: base_offset + start,
                                target: raw_target.to_string(),
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

/// Analiza el interior de un `[[...]]`. Con `:` es `rel:destino`;
/// sin él, un destino a secas. La relación usa el mismo alfabeto que
/// un segmento de [`ConceptId`] (minúsculas ASCII, dígitos, `-`,
/// `_`) y como mucho [`MAX_LINK_REL_LEN`] bytes.
fn parse_link(raw: &str) -> Option<Link> {
    match raw.split_once(':') {
        None => ConceptId::parse(raw).ok().map(|target| Link { target, rel: None }),
        Some((rel, target)) => {
            let rel_ok = !rel.is_empty()
                && rel.len() <= MAX_LINK_REL_LEN
                && rel
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_');
            if !rel_ok {
                return None;
            }
            ConceptId::parse(target)
                .ok()
                .map(|target| Link { target, rel: Some(rel.to_string()) })
        }
    }
}

// ---------------------------------------------------------------
// Patch de frontmatter: cirugía línea a línea
// ---------------------------------------------------------------

/// Cambios a aplicar sobre el frontmatter de un documento, sin tocar
/// el cuerpo. Ver [`patch_frontmatter`].
#[derive(Debug, Clone, Default)]
pub struct FrontmatterPatch {
    /// Campos a escribir (crear o reemplazar), en este orden.
    pub set: Vec<(String, FmValue)>,
    /// Campos a eliminar. `type` no se puede eliminar (es
    /// obligatorio); pedirlo es un error.
    pub remove: Vec<String>,
    /// Tags a añadir (ignorando los ya presentes).
    pub add_tags: Vec<String>,
    /// Tags a quitar (ignorando los ausentes).
    pub remove_tags: Vec<String>,
}

/// Aplica un [`FrontmatterPatch`] a un documento OKF y devuelve el
/// documento nuevo COMPLETO, listo para un commit normal.
///
/// La regla de oro del crate ("los bytes originales son la verdad")
/// se respeta al máximo posible: el cuerpo se copia byte a byte, y
/// del frontmatter solo se reescriben las líneas de los campos
/// tocados — comentarios, líneas en blanco y campos ajenos al patch
/// quedan intactos. Los campos nuevos se añaden justo antes del
/// `---` de cierre.
///
/// # Ejemplo
///
/// ```
/// use memory_model::Budget;
/// use okf_core::{patch_frontmatter, FmValue, FrontmatterPatch};
///
/// let raw = "---\ntype: note\ntitle: Borrador\n# revisar en marzo\ntags:\n  - draft\n---\ncuerpo intacto\n";
/// let patch = FrontmatterPatch {
///     set: vec![("status".to_string(), FmValue::Scalar("active".to_string()))],
///     add_tags: vec!["ready".to_string()],
///     remove_tags: vec!["draft".to_string()],
///     ..FrontmatterPatch::default()
/// };
/// let out = patch_frontmatter(raw, &patch, &Budget::default())?;
/// assert_eq!(out, "---\ntype: note\ntitle: Borrador\n# revisar en marzo\ntags:\n  - ready\nstatus: active\n---\ncuerpo intacto\n");
/// # Ok::<(), okf_core::OkfError>(())
/// ```
///
/// # Errores
///
/// El documento de entrada debe ser OKF válido; el resultado se
/// re-valida antes de devolverse, así que un patch que produjera un
/// documento inválido (p. ej. eliminar `type`) se rechaza entero.
pub fn patch_frontmatter(
    raw: &str,
    patch: &FrontmatterPatch,
    budget: &Budget,
) -> Result<String, OkfError> {
    let doc = parse_document(raw, budget)?;

    let error = |reason: String| OkfError::Unsupported { line: 0, reason };

    // Reglas del patch antes de tocar nada.
    if patch.remove.iter().any(|k| k == "type") {
        return Err(error("'type' es obligatorio: no se puede eliminar".to_string()));
    }
    for (key, _) in &patch.set {
        if patch.remove.contains(key) {
            return Err(error(format!("la clave {key:?} está en 'set' y en 'remove' a la vez")));
        }
    }

    // Tags finales: set explícito > tags actuales; luego añadir/quitar.
    let base_tags = patch
        .set
        .iter()
        .find(|(k, _)| k == "tags")
        .map(|(_, v)| match v {
            FmValue::List(items) => Ok(items.clone()),
            FmValue::Scalar(_) => Err(error("'tags' debe ser una lista".to_string())),
        })
        .transpose()?
        .unwrap_or_else(|| doc.tags.clone());
    let mut final_tags: Vec<String> = base_tags
        .into_iter()
        .filter(|t| !patch.remove_tags.contains(t))
        .collect();
    for tag in &patch.add_tags {
        if !final_tags.contains(tag) {
            final_tags.push(tag.clone());
        }
    }
    let touch_tags = patch.set.iter().any(|(k, _)| k == "tags")
        || !patch.add_tags.is_empty()
        || !patch.remove_tags.is_empty();

    // Qué se reescribe en su sitio y qué se elimina del texto.
    let mut replacements: Vec<(String, FmValue)> = Vec::new();
    for (key, value) in &patch.set {
        if key == "tags" {
            continue; // las tags van con su propia lógica, abajo
        }
        replacements.push((key.clone(), value.clone()));
    }
    if touch_tags {
        replacements.push(("tags".to_string(), FmValue::List(final_tags.clone())));
    }

    let (fm_text, body_offset) = split_frontmatter(raw, budget)?;

    // Cirugía línea a línea, con la misma máquina de estados que
    // parse_frontmatter: cada línea pertenece a una clave (o a
    // ninguna, si es blanca o comentario) y se decide en su sitio.
    let mut out = String::with_capacity(raw.len() + 64);
    out.push_str("---\n");
    let mut emitted: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut open_list: Option<String> = None;

    for raw_line in fm_text.lines() {
        let line = raw_line.trim_end();
        let owner: Option<String> = if line.is_empty() || line.trim_start().starts_with('#') {
            None
        } else if line.trim_start().starts_with("- ") {
            open_list.clone()
        } else {
            let key = line[..line.find(':').unwrap_or(line.len())].trim().to_string();
            let rest_empty = line.find(':').map(|c| line[c + 1..].trim().is_empty());
            open_list = if rest_empty == Some(true) { Some(key.clone()) } else { None };
            Some(key)
        };

        match owner {
            None => {
                out.push_str(raw_line);
                out.push('\n');
            }
            Some(key) => {
                if patch.remove.contains(&key) || (key == "tags" && touch_tags && final_tags.is_empty()) {
                    continue; // eliminada: todas sus líneas se omiten
                }
                match replacements.iter().find(|(k, _)| *k == key) {
                    None => {
                        out.push_str(raw_line);
                        out.push('\n');
                    }
                    Some((_, value)) => {
                        // La primera línea de la clave emite el valor
                        // nuevo; las siguientes (items de lista) se
                        // omiten.
                        if emitted.insert(key.clone()) {
                            serialize_field(&mut out, &key, value)?;
                        }
                    }
                }
            }
        }
    }

    // Campos del patch que no existían: se añaden antes del cierre.
    for (key, value) in &replacements {
        if key == "tags" && final_tags.is_empty() {
            continue;
        }
        if !emitted.contains(key) {
            serialize_field(&mut out, key, value)?;
        }
    }

    out.push_str("---\n");
    out.push_str(&raw[body_offset..]); // el cuerpo, byte a byte

    // El resultado debe ser OKF válido o el patch entero se rechaza.
    parse_document(&out, budget)?;
    Ok(out)
}

/// Serializa `clave: valor` (o una lista en bloque) en el subconjunto
/// YAML del crate, de forma que [`parse_frontmatter`] lo lea de
/// vuelta EXACTAMENTE igual.
fn serialize_field(out: &mut String, key: &str, value: &FmValue) -> Result<(), OkfError> {
    let key_ok = !key.is_empty()
        && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if !key_ok {
        return Err(OkfError::Unsupported { line: 0, reason: format!("clave inválida {key:?}") });
    }
    match value {
        FmValue::Scalar(v) => {
            out.push_str(key);
            out.push_str(": ");
            out.push_str(&serialize_scalar(v)?);
            out.push('\n');
        }
        FmValue::List(items) => {
            out.push_str(key);
            out.push_str(":\n");
            for item in items {
                out.push_str("  - ");
                out.push_str(&serialize_scalar(item)?);
                out.push('\n');
            }
        }
    }
    Ok(())
}

/// El inverso de `parse_scalar`: texto plano cuando es seguro,
/// comillas cuando el texto plano se malinterpretaría, error cuando
/// el subconjunto no puede representarlo.
fn serialize_scalar(v: &str) -> Result<String, OkfError> {
    if v.contains('\n') || v.contains('"') || v.contains('\\') {
        return Err(OkfError::Unsupported {
            line: 0,
            reason: format!("el subconjunto YAML no puede representar el valor {v:?}"),
        });
    }
    let necesita_comillas = v.is_empty()
        || v.starts_with(['|', '>', '&', '*', '{', '[', '"', ' ', '\t', '#'])
        || v.ends_with([' ', '\t'])
        || v.contains(" #");
    if necesita_comillas {
        Ok(format!("\"{v}\""))
    } else {
        Ok(v.to_string())
    }
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
        let links: Vec<&str> = doc.links.iter().map(|l| l.target.as_str()).collect();
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

    #[test]
    fn enlaces_tipados_y_genericos_conviven() {
        let raw = "---\ntype: nota\n---\nver [[depends_on:libs/sqlx]] y [[people/bob]]\n";
        let doc = parse_document(raw, &Budget::default()).unwrap();
        assert_eq!(doc.links.len(), 2);
        assert_eq!(doc.links[0].target.as_str(), "libs/sqlx");
        assert_eq!(doc.links[0].rel.as_deref(), Some("depends_on"));
        assert_eq!(doc.links[1].target.as_str(), "people/bob");
        assert_eq!(doc.links[1].rel, None);
    }

    #[test]
    fn la_primera_aparicion_de_un_destino_gana() {
        let raw = "---\ntype: nota\n---\n[[uses:a]] y luego [[a]] otra vez\n";
        let doc = parse_document(raw, &Budget::default()).unwrap();
        assert_eq!(doc.links.len(), 1);
        assert_eq!(doc.links[0].rel.as_deref(), Some("uses"));
    }

    #[test]
    fn relaciones_invalidas_se_rechazan() {
        for raw in [
            "---\ntype: nota\n---\n[[:a]]\n",              // rel vacía
            "---\ntype: nota\n---\n[[Mayus:a]]\n",         // mayúsculas
            "---\ntype: nota\n---\n[[re/l:a]]\n",          // '/' en la rel
            "---\ntype: nota\n---\n[[uses:../etc]]\n",     // destino inválido
        ] {
            assert!(
                matches!(parse_document(raw, &Budget::default()), Err(OkfError::InvalidLink { .. })),
                "aceptó: {raw:?}"
            );
        }
    }

    #[test]
    fn patch_reemplaza_solo_las_lineas_tocadas() {
        let raw = "---\ntype: note\n# comentario que sobrevive\ntitle: Vieja\nstatus: draft\n---\ncuerpo\n";
        let patch = FrontmatterPatch {
            set: vec![("title".to_string(), FmValue::Scalar("Nueva".to_string()))],
            ..FrontmatterPatch::default()
        };
        let out = patch_frontmatter(raw, &patch, &Budget::default()).unwrap();
        assert_eq!(
            out,
            "---\ntype: note\n# comentario que sobrevive\ntitle: Nueva\nstatus: draft\n---\ncuerpo\n"
        );
    }

    #[test]
    fn patch_de_tags_añade_y_quita() {
        let raw = "---\ntype: note\ntags:\n  - draft\n  - rust\n---\ncuerpo\n";
        let patch = FrontmatterPatch {
            add_tags: vec!["ready".to_string(), "rust".to_string()], // rust ya está
            remove_tags: vec!["draft".to_string()],
            ..FrontmatterPatch::default()
        };
        let out = patch_frontmatter(raw, &patch, &Budget::default()).unwrap();
        assert_eq!(out, "---\ntype: note\ntags:\n  - rust\n  - ready\n---\ncuerpo\n");
    }

    #[test]
    fn patch_crea_campos_nuevos_y_elimina_existentes() {
        let raw = "---\ntype: note\nstatus: draft\n---\ncuerpo\n";
        let patch = FrontmatterPatch {
            set: vec![("owner".to_string(), FmValue::Scalar("pelayo".to_string()))],
            remove: vec!["status".to_string()],
            add_tags: vec!["nuevo".to_string()],
            ..FrontmatterPatch::default()
        };
        let out = patch_frontmatter(raw, &patch, &Budget::default()).unwrap();
        assert_eq!(out, "---\ntype: note\nowner: pelayo\ntags:\n  - nuevo\n---\ncuerpo\n");
    }

    #[test]
    fn patch_no_puede_eliminar_type_ni_dejar_documento_invalido() {
        let raw = "---\ntype: note\n---\ncuerpo\n";
        let patch = FrontmatterPatch {
            remove: vec!["type".to_string()],
            ..FrontmatterPatch::default()
        };
        assert!(patch_frontmatter(raw, &patch, &Budget::default()).is_err());

        // Un valor irrepresentable en el subconjunto también se rechaza.
        let patch = FrontmatterPatch {
            set: vec![("title".to_string(), FmValue::Scalar("con \"comillas\"".to_string()))],
            ..FrontmatterPatch::default()
        };
        assert!(patch_frontmatter(raw, &patch, &Budget::default()).is_err());
    }

    #[test]
    fn patch_de_valores_que_necesitan_comillas() {
        let raw = "---\ntype: note\n---\ncuerpo\n";
        let patch = FrontmatterPatch {
            set: vec![("title".to_string(), FmValue::Scalar("nota # con almohadilla".to_string()))],
            ..FrontmatterPatch::default()
        };
        let out = patch_frontmatter(raw, &patch, &Budget::default()).unwrap();
        assert_eq!(out, "---\ntype: note\ntitle: \"nota # con almohadilla\"\n---\ncuerpo\n");
        let doc = parse_document(&out, &Budget::default()).unwrap();
        assert_eq!(doc.title.as_deref(), Some("nota # con almohadilla"));
    }

    #[test]
    fn quitar_todas_las_tags_elimina_el_bloque() {
        let raw = "---\ntype: note\ntags:\n  - solo\n---\ncuerpo\n";
        let patch = FrontmatterPatch {
            remove_tags: vec!["solo".to_string()],
            ..FrontmatterPatch::default()
        };
        let out = patch_frontmatter(raw, &patch, &Budget::default()).unwrap();
        assert_eq!(out, "---\ntype: note\n---\ncuerpo\n");
    }
}
