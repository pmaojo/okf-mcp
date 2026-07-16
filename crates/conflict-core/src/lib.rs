//! Motor de decisiones de concurrencia optimista (compare-and-swap).
//!
//! SOLID en juego:
//! - **S:** este crate decide; no almacena, no hashea, no serializa.
//!   La función [`decide`] es pura: mismos argumentos, misma
//!   decisión. Eso la hace trivial de testear exhaustivamente.
//! - **O:** cuando llegue el merge a tres bandas (hito futuro), se
//!   añadirá una variante a [`CommitDecision`] sin tocar el almacén.
//!
//! La regla que protege contra actualizaciones perdidas:
//!
//! > Toda escritura declara el hash que el agente LEYÓ. Si la cabeza
//! > actual ya no es ese hash, alguien escribió en medio y el commit
//! > se rechaza con un conflicto estructurado. Nunca se sobreescribe
//! > en silencio.

#![forbid(unsafe_code)]

use memory_model::ContentId;

/// Qué hacer con un intento de escritura.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitDecision {
    /// Crear el documento (no existía y el agente no esperaba nada).
    Create,
    /// Avanzar la cabeza del hash esperado al entrante.
    Update,
    /// El contenido entrante es idéntico al actual: no hay nada que
    /// escribir. Idempotencia gratis gracias al hash de contenido.
    NoChange,
    /// La cabeza cambió desde que el agente leyó. Datos para que el
    /// cliente relea, re-aplique y reintente.
    Conflict(Conflict),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    /// Lo que el agente creía que había (None = creía que no existía).
    pub expected: Option<ContentId>,
    /// Lo que hay realmente ahora (None = no existe).
    pub current: Option<ContentId>,
    /// Lo que el agente intentaba escribir.
    pub incoming: ContentId,
}

/// Decide el destino de un commit.
///
/// * `head`      — hash actual de la cabeza, si el documento existe.
/// * `expected`  — hash que el cliente declara haber leído
///                 (`None` = "estoy creando este documento").
/// * `incoming`  — hash del contenido que quiere escribir.
pub fn decide(
    head: Option<ContentId>,
    expected: Option<ContentId>,
    incoming: ContentId,
) -> CommitDecision {
    match (head, expected) {
        // No existe y el cliente no esperaba que existiera: creación.
        (None, None) => CommitDecision::Create,

        // No existe pero el cliente creía que sí: alguien lo borró
        // (o el cliente habla de otro almacén). Conflicto.
        (None, Some(exp)) => CommitDecision::Conflict(Conflict {
            expected: Some(exp),
            current: None,
            incoming,
        }),

        // Existe pero el cliente creía que no: creación concurrente.
        (Some(cur), None) => {
            if cur == incoming {
                CommitDecision::NoChange
            } else {
                CommitDecision::Conflict(Conflict {
                    expected: None,
                    current: Some(cur),
                    incoming,
                })
            }
        }

        // Existe y el cliente declara base: el caso central del CAS.
        (Some(cur), Some(exp)) => {
            if cur != exp {
                if cur == incoming {
                    // La "otra" escritura dejó exactamente lo que
                    // íbamos a escribir. No hay pérdida posible.
                    CommitDecision::NoChange
                } else {
                    CommitDecision::Conflict(Conflict {
                        expected: Some(exp),
                        current: Some(cur),
                        incoming,
                    })
                }
            } else if cur == incoming {
                CommitDecision::NoChange
            } else {
                CommitDecision::Update
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(byte: u8) -> ContentId {
        ContentId([byte; 32])
    }

    #[test]
    fn creacion_limpia() {
        assert_eq!(decide(None, None, h(1)), CommitDecision::Create);
    }

    #[test]
    fn actualizacion_limpia() {
        assert_eq!(decide(Some(h(1)), Some(h(1)), h(2)), CommitDecision::Update);
    }

    #[test]
    fn escritura_identica_es_nochange() {
        assert_eq!(decide(Some(h(1)), Some(h(1)), h(1)), CommitDecision::NoChange);
    }

    #[test]
    fn base_obsoleta_es_conflicto() {
        let d = decide(Some(h(3)), Some(h(1)), h(2));
        assert_eq!(
            d,
            CommitDecision::Conflict(Conflict {
                expected: Some(h(1)),
                current: Some(h(3)),
                incoming: h(2),
            })
        );
    }

    #[test]
    fn creacion_concurrente_es_conflicto() {
        assert!(matches!(decide(Some(h(1)), None, h(2)), CommitDecision::Conflict(_)));
        // …salvo que ambas creaciones escriban lo mismo.
        assert_eq!(decide(Some(h(1)), None, h(1)), CommitDecision::NoChange);
    }

    #[test]
    fn borrado_concurrente_es_conflicto() {
        assert!(matches!(decide(None, Some(h(1)), h(2)), CommitDecision::Conflict(_)));
    }

    #[test]
    fn convergencia_es_nochange() {
        // Otro cliente ya escribió exactamente nuestro contenido.
        assert_eq!(decide(Some(h(2)), Some(h(1)), h(2)), CommitDecision::NoChange);
    }
}
