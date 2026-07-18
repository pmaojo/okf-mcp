//! Tipos de dominio del motor de memoria.
//!
//! Este crate no sabe nada de JSON, HTTP, YAML ni bases de datos.
//! Solo define QUÉ es un concepto ([`ConceptId`]), un hash de
//! contenido ([`ContentId`]), una revisión ([`Revision`]) y un
//! presupuesto de recursos ([`Budget`]). Todo lo demás son
//! adaptadores.
//!
//! # Ejemplo
//!
//! ```
//! use memory_model::{Budget, ConceptId};
//!
//! let id = ConceptId::parse("people/alice")?;
//! let budget = Budget::default();
//! assert!(id.as_str().len() <= memory_model::MAX_CONCEPT_ID_LEN);
//! assert!(budget.max_graph_depth >= 1);
//! # Ok::<(), memory_model::ConceptIdError>(())
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::fmt;

/// Longitud máxima de un `ConceptId` en bytes.
pub const MAX_CONCEPT_ID_LEN: usize = 200;

/// Identificador lógico de un concepto, p. ej. `people/alice`.
///
/// Es una ruta LÓGICA: nunca se traduce a una ruta de sistema de
/// ficheros. Aun así se valida como si pudiera serlo, porque el día
/// que un adaptador la use mal, la validación ya nos habrá salvado.
///
/// Invariantes garantizados por construcción:
/// - UTF-8 (viene de `&str`), ASCII minúsculo, dígitos, `-`, `_`, `/`
/// - sin segmentos vacíos (`a//b`), sin `.` ni `..` como segmento
/// - no empieza ni termina en `/`
/// - longitud entre 1 y [`MAX_CONCEPT_ID_LEN`]
///
/// # Ejemplos
///
/// ```
/// use memory_model::ConceptId;
///
/// let id = ConceptId::parse("people/alice").unwrap();
/// assert_eq!(id.as_str(), "people/alice");
///
/// // La lista blanca rechaza cualquier intento de traversal:
/// assert!(ConceptId::parse("../etc/passwd").is_err());
/// assert!(ConceptId::parse("a/../b").is_err());
/// ```
///
/// Y el invariante no se puede esquivar: el campo interno es
/// privado, así que fabricar un `ConceptId` sin pasar por
/// [`ConceptId::parse`] ni siquiera compila. rustdoc verifica esta
/// afirmación en cada `cargo test`:
///
/// ```compile_fail
/// let id = memory_model::ConceptId("../etc".to_string());
/// ```
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConceptId(String);

/// Por qué un texto no es un `ConceptId` válido.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConceptIdError {
    /// La cadena está vacía.
    Empty,
    /// Supera [`MAX_CONCEPT_ID_LEN`].
    TooLong {
        /// Longitud recibida, en bytes.
        len: usize,
        /// El máximo permitido ([`MAX_CONCEPT_ID_LEN`]).
        max: usize,
    },
    /// Byte no permitido y su posición.
    InvalidByte {
        /// El byte rechazado.
        byte: u8,
        /// Posición del byte en la cadena de entrada.
        offset: usize,
    },
    /// Segmento vacío, `.` o `..`.
    InvalidSegment {
        /// El segmento rechazado, tal cual llegó.
        segment: String,
    },
    /// Empieza o termina con `/`.
    LeadingOrTrailingSlash,
}

impl fmt::Display for ConceptIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConceptIdError::Empty => write!(f, "el identificador está vacío"),
            ConceptIdError::TooLong { len, max } => {
                write!(f, "identificador de {len} bytes; el máximo es {max}")
            }
            ConceptIdError::InvalidByte { byte, offset } => {
                write!(f, "byte no permitido 0x{byte:02x} en la posición {offset}")
            }
            ConceptIdError::InvalidSegment { segment } => {
                write!(f, "segmento no permitido: {segment:?}")
            }
            ConceptIdError::LeadingOrTrailingSlash => {
                write!(f, "no puede empezar ni terminar con '/'")
            }
        }
    }
}

impl std::error::Error for ConceptIdError {}

impl ConceptId {
    /// Valida y construye. La validación es por lista blanca:
    /// rechazamos todo lo que no esté explícitamente permitido.
    ///
    /// # Errores
    ///
    /// Devuelve la variante de [`ConceptIdError`] que describe la
    /// PRIMERA regla violada, con el byte y la posición exactos
    /// cuando aplica. Un validador que dice "inválido" sin decir
    /// dónde no enseña nada.
    pub fn parse(s: &str) -> Result<Self, ConceptIdError> {
        if s.is_empty() {
            return Err(ConceptIdError::Empty);
        }
        if s.len() > MAX_CONCEPT_ID_LEN {
            return Err(ConceptIdError::TooLong { len: s.len(), max: MAX_CONCEPT_ID_LEN });
        }
        if s.starts_with('/') || s.ends_with('/') {
            return Err(ConceptIdError::LeadingOrTrailingSlash);
        }
        for (offset, byte) in s.bytes().enumerate() {
            let ok = byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || byte == b'-'
                || byte == b'_'
                || byte == b'/';
            if !ok {
                return Err(ConceptIdError::InvalidByte { byte, offset });
            }
        }
        for segment in s.split('/') {
            if segment.is_empty() || segment == "." || segment == ".." {
                return Err(ConceptIdError::InvalidSegment { segment: segment.to_string() });
            }
        }
        Ok(ConceptId(s.to_string()))
    }

    /// La ruta lógica validada, como `&str` prestado.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ConceptId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Identidad de contenido: SHA-256 de los bytes UTF-8 exactos del
/// documento. No es un id de objeto Git (Git mezcla tipo y longitud
/// en el hash); es NUESTRO tipo, con nuestras reglas.
///
/// # Ejemplo
///
/// La ida y vuelta hexadecimal es exacta o no es:
///
/// ```
/// use memory_model::ContentId;
///
/// let id = ContentId([0xab; 32]);
/// let hex = id.to_hex();
/// assert_eq!(hex.len(), 64);
/// assert_eq!(ContentId::from_hex(&hex), Some(id));
/// assert_eq!(ContentId::from_hex("demasiado-corto"), None);
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContentId(pub [u8; 32]);

impl ContentId {
    /// Representación hexadecimal en minúsculas (64 caracteres).
    pub fn to_hex(&self) -> String {
        let mut s = String::with_capacity(64);
        for b in self.0 {
            use fmt::Write;
            let _ = write!(s, "{b:02x}");
        }
        s
    }

    /// Parsea 64 caracteres hexadecimales. Devuelve `None` si el
    /// formato no es exacto: sin prefijos, sin mayúsculas mezcladas
    /// prohibidas (aceptamos ambas cajas), sin longitudes raras.
    pub fn from_hex(s: &str) -> Option<Self> {
        let bytes = s.as_bytes();
        if bytes.len() != 64 {
            return None;
        }
        let mut out = [0u8; 32];
        for (i, chunk) in bytes.chunks_exact(2).enumerate() {
            let hi = hex_val(chunk[0])?;
            let lo = hex_val(chunk[1])?;
            out[i] = (hi << 4) | lo;
        }
        Some(ContentId(out))
    }
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

impl fmt::Debug for ContentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ContentId({})", self.to_hex())
    }
}

impl fmt::Display for ContentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// Quién realiza una operación. En el hito 1 (local) hay un único
/// principal de desarrollo; en producción saldrá de un JWT validado.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Principal {
    /// `sub` del token (o "local" en desarrollo).
    pub subject: String,
    /// `client_id` OAuth (o "dev" en desarrollo).
    pub client_id: String,
}

impl Principal {
    /// El principal fijo del hito 1: `subject = "local"`,
    /// `client_id = "dev"`. En producción lo sustituye el `sub` de
    /// un JWT validado.
    pub fn local_dev() -> Self {
        Principal { subject: "local".to_string(), client_id: "dev".to_string() }
    }
}

/// Metadatos de una revisión confirmada.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Revision {
    /// Número de secuencia global, monótono creciente (1, 2, 3, …).
    pub seq: u64,
    /// Concepto al que pertenece la revisión.
    pub concept_id: ConceptId,
    /// Hash sobre el que se basó la edición (`None` si el documento
    /// se creó en esta revisión).
    pub base: Option<ContentId>,
    /// Hash resultante.
    pub result: ContentId,
    /// Quién confirmó la revisión.
    pub actor: Principal,
    /// Motivo declarado por el agente ("añadí el nuevo proyecto…").
    pub reason: String,
}

/// Presupuesto explícito de recursos por petición.
///
/// Cada operación que consume memoria o produce salida debe
/// comprobar el presupuesto ANTES de asignar, no después de fallar.
///
/// # Ejemplo
///
/// El [`Default`] es el punto de partida; cada frontera lo ajusta:
///
/// ```
/// use memory_model::Budget;
///
/// let budget = Budget { max_graph_depth: 2, ..Budget::default() };
/// assert_eq!(budget.max_graph_depth, 2);
/// assert_eq!(budget.max_graph_nodes, 128); // el resto, intacto
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    /// Tamaño máximo de una petición entrante, en bytes.
    pub max_request_bytes: usize,
    /// Tamaño máximo de una respuesta serializada, en bytes.
    pub max_response_bytes: usize,
    /// Tamaño máximo de un documento Markdown completo, en bytes.
    pub max_document_bytes: usize,
    /// Tamaño máximo del frontmatter YAML, en bytes.
    pub max_frontmatter_bytes: usize,
    /// Nodos máximos que un recorrido de grafo puede visitar.
    pub max_graph_nodes: usize,
    /// Profundidad máxima de un recorrido de grafo.
    pub max_graph_depth: u8,
    /// Enlaces `[[...]]` máximos por documento.
    pub max_links_per_document: usize,
    /// Resultados máximos de una búsqueda.
    pub max_search_results: usize,
}

impl Default for Budget {
    fn default() -> Self {
        Budget {
            max_request_bytes: 1 << 20,       // 1 MiB
            max_response_bytes: 2 << 20,      // 2 MiB
            max_document_bytes: 256 << 10,    // 256 KiB
            max_frontmatter_bytes: 16 << 10,  // 16 KiB
            max_graph_nodes: 128,
            max_graph_depth: 4,
            max_links_per_document: 512,
            max_search_results: 50,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concept_id_acepta_rutas_validas() {
        for ok in ["a", "people/alice", "a/b/c", "notas-2026/rust_std", "0/1"] {
            assert!(ConceptId::parse(ok).is_ok(), "debería aceptar {ok:?}");
        }
    }

    #[test]
    fn concept_id_rechaza_traversal_y_bytes_raros() {
        assert_eq!(ConceptId::parse(""), Err(ConceptIdError::Empty));
        assert!(matches!(ConceptId::parse("../etc"), Err(ConceptIdError::InvalidByte { .. })));
        assert!(matches!(
            ConceptId::parse("a/../b"),
            Err(ConceptIdError::InvalidByte { .. }) // el '.' ya no está en la lista blanca
        ));
        assert_eq!(ConceptId::parse("/abs"), Err(ConceptIdError::LeadingOrTrailingSlash));
        assert_eq!(ConceptId::parse("fin/"), Err(ConceptIdError::LeadingOrTrailingSlash));
        assert!(matches!(ConceptId::parse("a//b"), Err(ConceptIdError::InvalidSegment { .. })));
        assert!(matches!(ConceptId::parse("A"), Err(ConceptIdError::InvalidByte { .. })));
        assert!(matches!(ConceptId::parse("a\\b"), Err(ConceptIdError::InvalidByte { .. })));
        assert!(matches!(ConceptId::parse("a\0b"), Err(ConceptIdError::InvalidByte { .. })));
        assert!(matches!(ConceptId::parse("a%2e%2e"), Err(ConceptIdError::InvalidByte { .. })));
        let largo = "a/".repeat(200) + "a";
        assert!(matches!(ConceptId::parse(&largo), Err(ConceptIdError::TooLong { .. })));
    }

    #[test]
    fn content_id_hex_ida_y_vuelta() {
        let id = ContentId([0xab; 32]);
        let hex = id.to_hex();
        assert_eq!(hex.len(), 64);
        assert_eq!(ContentId::from_hex(&hex), Some(id));
        assert_eq!(ContentId::from_hex("corto"), None);
        assert_eq!(ContentId::from_hex(&"zz".repeat(32)), None);
    }
}
