//! Razonamiento ligero sobre el grafo OKF: triples derivados de la
//! misma marca (`type` + `[[rel:destino]]`) que ya usa `okf-core`, y
//! un motor de encadenamiento hacia adelante para un subconjunto de
//! OWL-RL/RDFS (`subClassOf`, `subPropertyOf`, transitividad,
//! simetría, `inverseOf`).
//!
//! # Por qué no un almacén RDF completo
//!
//! Este workspace es std-only por regla del proyecto
//! (`scripts/check-std-only.sh`): ningún crate del núcleo declara
//! dependencias externas. Un almacén RDF + SPARQL completo (p. ej.
//! `oxigraph`) es mucho más de lo que un agente necesita para deducir
//! "si A es un `person` y `person` es sub-clase de `agent`, entonces
//! A es un `agent`". Este crate implementa justo ese subconjunto,
//! acotado por presupuesto igual que `graph_core::bounded_bfs`, y sin
//! ninguna dependencia externa.
//!
//! # De marca a triples
//!
//! [`triples_from_document`] traduce un `okf_core::OkfDocument` ya
//! parseado:
//! - `type: person` → `(sujeto, "rdf:type", Literal("person"))`
//! - `[[depends_on:libs/sqlx]]` → `(sujeto, "depends_on", Concept(libs/sqlx))`
//! - un enlace sin relación (`[[people/bob]]`) → predicado
//!   [`RELATED`]
//! - cada tag → `(sujeto, "tag", Literal(tag))`
//!
//! # Razonamiento
//!
//! [`materialize`] aplica un punto fijo acotado sobre una
//! [`Ontology`] declarada a mano (no hay parser OWL: los axiomas se
//! escriben como datos Rust, igual de deterministas que el resto del
//! núcleo).

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use memory_model::ConceptId;
use okf_core::{Link, OkfDocument};
use std::collections::BTreeSet;
use std::fmt;

/// El objeto de un triple: o bien otro concepto del grafo, o un
/// literal de texto (p. ej. un `type` o un `tag`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Object {
    /// Referencia a otro documento del grafo.
    Concept(ConceptId),
    /// Valor de texto plano, sin identidad propia.
    Literal(String),
}

/// Un hecho `(sujeto, predicado, objeto)`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Triple {
    /// El sujeto: siempre un concepto real del grafo.
    pub subject: ConceptId,
    /// El predicado: [`RDF_TYPE`], la relación tipada de un enlace,
    /// [`RELATED`], [`TAG`], o uno definido por la ontología.
    pub predicate: String,
    /// El objeto del hecho.
    pub object: Object,
}

/// Predicado reservado para las declaraciones de tipo
/// (`rdf:type` de toda la vida).
pub const RDF_TYPE: &str = "rdf:type";
/// Predicado de los enlaces `[[concepto]]` sin relación tipada.
pub const RELATED: &str = "related";
/// Predicado de cada entrada de `tags`.
pub const TAG: &str = "tag";

/// Deriva los triples de un documento OKF ya parseado. No decide
/// nada por sí mismo: `okf_core::parse_document` sigue siendo la
/// única fuente de verdad sobre qué es un documento válido; esta
/// función solo re-etiqueta lo que ya extrajo.
///
/// # Ejemplo
///
/// ```
/// use memory_model::{Budget, ConceptId};
/// use ontology_core::{triples_from_document, Object, RDF_TYPE, RELATED, TAG};
///
/// let raw = "---\ntype: person\ntags:\n  - staff\n---\nTrabaja con [[reports_to:people/bob]].\n";
/// let doc = okf_core::parse_document(raw, &Budget::default()).unwrap();
/// let alice = ConceptId::parse("people/alice").unwrap();
///
/// let triples = triples_from_document(&alice, &doc);
/// assert!(triples.iter().any(|t| t.predicate == RDF_TYPE && t.object == Object::Literal("person".to_string())));
/// assert!(triples.iter().any(|t| t.predicate == TAG && t.object == Object::Literal("staff".to_string())));
/// assert!(triples.iter().any(|t| t.predicate == "reports_to"));
/// assert!(!triples.iter().any(|t| t.predicate == RELATED)); // el único enlace es tipado
/// ```
pub fn triples_from_document(subject: &ConceptId, doc: &OkfDocument) -> Vec<Triple> {
    let mut out = Vec::with_capacity(1 + doc.tags.len() + doc.links.len());
    out.push(Triple {
        subject: subject.clone(),
        predicate: RDF_TYPE.to_string(),
        object: Object::Literal(doc.doc_type.clone()),
    });
    for tag in &doc.tags {
        out.push(Triple {
            subject: subject.clone(),
            predicate: TAG.to_string(),
            object: Object::Literal(tag.clone()),
        });
    }
    for link in &doc.links {
        out.push(triple_from_link(subject, link));
    }
    out
}

fn triple_from_link(subject: &ConceptId, link: &Link) -> Triple {
    Triple {
        subject: subject.clone(),
        predicate: link.rel.clone().unwrap_or_else(|| RELATED.to_string()),
        object: Object::Concept(link.target.clone()),
    }
}

/// Axioma de clase: `subclass` es sub-clase de `superclass`
/// (`rdfs:subClassOf`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubClassOf {
    /// La clase más específica.
    pub subclass: String,
    /// La clase más general.
    pub superclass: String,
}

/// Características de un predicado que el motor sabe explotar. Cada
/// variante es UNA regla de inferencia del subconjunto OWL-RL/RDFS
/// cubierto, no una implementación completa de OWL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PropertyAxiom {
    /// `sub` es sub-propiedad de `sup`: todo `(x, sub, y)` implica
    /// `(x, sup, y)`.
    SubPropertyOf {
        /// La propiedad específica.
        sub: String,
        /// La propiedad general.
        sup: String,
    },
    /// La propiedad nombrada es transitiva:
    /// `(x,P,y) ∧ (y,P,z) ⇒ (x,P,z)`.
    Transitive(String),
    /// La propiedad nombrada es simétrica: `(x,P,y) ⇒ (y,P,x)`.
    Symmetric(String),
    /// `property` e `inverse` son inversas:
    /// `(x,property,y) ⇒ (y,inverse,x)`.
    InverseOf {
        /// La propiedad original.
        property: String,
        /// Su inversa.
        inverse: String,
    },
}

/// El conjunto de axiomas que gobierna [`materialize`]: la
/// "ontología". Se declara a mano en Rust — determinista, sin parser
/// YAML/OWL de por medio, igual que el resto del núcleo.
#[derive(Debug, Clone, Default)]
pub struct Ontology {
    /// Jerarquía de clases.
    pub classes: Vec<SubClassOf>,
    /// Características de propiedades.
    pub properties: Vec<PropertyAxiom>,
}

/// Cuánto puede crecer una llamada a [`materialize`] antes de
/// detenerse. Tan necesario aquí como `Budget` lo es para
/// `bounded_bfs`: sin cota, un ciclo de `subClassOf` o de propiedades
/// simétricas/inversas itera para siempre.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReasoningBudget {
    /// Máximo de rondas de punto fijo.
    pub max_iterations: usize,
    /// Máximo de triples totales (asertados + derivados) en el resultado.
    pub max_triples: usize,
}

impl Default for ReasoningBudget {
    fn default() -> Self {
        ReasoningBudget { max_iterations: 16, max_triples: 10_000 }
    }
}

/// Resultado de [`materialize`], con las razones de truncado
/// explícitas — el mismo contrato que `graph_core::Traversal`: el
/// agente que consume esto DEBE poder saber si vio la clausura
/// completa o una vista parcial.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Materialized {
    /// Hechos asertados + inferidos, sin duplicados, en orden estable.
    pub triples: Vec<Triple>,
    /// Se alcanzó `budget.max_iterations` sin llegar a punto fijo.
    pub truncated_by_iterations: bool,
    /// Se alcanzó `budget.max_triples` con inferencias pendientes.
    pub truncated_by_triples: bool,
}

/// Encadenamiento hacia adelante acotado: aplica los axiomas de
/// `ontology` sobre `facts` hasta alcanzar un punto fijo o agotar
/// `budget`.
///
/// # Ejemplo
///
/// ```
/// use memory_model::ConceptId;
/// use ontology_core::{
///     materialize, Object, Ontology, ReasoningBudget, SubClassOf, Triple, RDF_TYPE,
/// };
///
/// let alice = ConceptId::parse("people/alice").unwrap();
/// let facts = vec![Triple {
///     subject: alice.clone(),
///     predicate: RDF_TYPE.to_string(),
///     object: Object::Literal("person".to_string()),
/// }];
/// let ontology = Ontology {
///     classes: vec![SubClassOf {
///         subclass: "person".to_string(),
///         superclass: "agent".to_string(),
///     }],
///     properties: vec![],
/// };
///
/// let out = materialize(&facts, &ontology, &ReasoningBudget::default());
/// assert!(out.triples.iter().any(|t| {
///     t.subject == alice
///         && t.predicate == RDF_TYPE
///         && t.object == Object::Literal("agent".to_string())
/// }));
/// assert!(!out.truncated_by_iterations && !out.truncated_by_triples);
/// ```
pub fn materialize(facts: &[Triple], ontology: &Ontology, budget: &ReasoningBudget) -> Materialized {
    let mut known: BTreeSet<Triple> = BTreeSet::new();
    let mut truncated_by_triples = false;
    for fact in facts {
        if known.len() >= budget.max_triples {
            truncated_by_triples = true;
            break;
        }
        known.insert(fact.clone());
    }

    let mut truncated_by_iterations = false;
    let mut iterations_used = 0usize;
    while !truncated_by_triples && iterations_used < budget.max_iterations {
        iterations_used += 1;

        let derived = apply_rules_once(&known, ontology);
        let before = known.len();
        for triple in derived {
            if known.len() >= budget.max_triples {
                truncated_by_triples = true;
                break;
            }
            known.insert(triple);
        }
        if known.len() == before {
            break; // punto fijo: ninguna regla aportó nada nuevo esta ronda
        }
        if iterations_used == budget.max_iterations {
            truncated_by_iterations = true;
        }
    }

    Materialized {
        triples: known.into_iter().collect(),
        truncated_by_iterations,
        truncated_by_triples,
    }
}

/// Una sola ronda de todas las reglas, sobre el conjunto de hechos
/// conocido hasta ahora. Puede devolver triples ya presentes en
/// `known` — [`materialize`] los deduplica al insertarlos en el
/// `BTreeSet`, así que esta función se mantiene simple a propósito.
fn apply_rules_once(known: &BTreeSet<Triple>, ontology: &Ontology) -> Vec<Triple> {
    let mut out = Vec::new();

    // subClassOf: (x, rdf:type, C) ∧ subClassOf(C, D) ⇒ (x, rdf:type, D)
    for triple in known {
        if triple.predicate != RDF_TYPE {
            continue;
        }
        let Object::Literal(class) = &triple.object else { continue };
        for axiom in &ontology.classes {
            if &axiom.subclass == class {
                out.push(Triple {
                    subject: triple.subject.clone(),
                    predicate: RDF_TYPE.to_string(),
                    object: Object::Literal(axiom.superclass.clone()),
                });
            }
        }
    }

    for axiom in &ontology.properties {
        match axiom {
            PropertyAxiom::SubPropertyOf { sub, sup } => {
                for triple in known.iter().filter(|t| &t.predicate == sub) {
                    out.push(Triple {
                        subject: triple.subject.clone(),
                        predicate: sup.clone(),
                        object: triple.object.clone(),
                    });
                }
            }
            PropertyAxiom::Symmetric(p) => {
                for triple in known.iter().filter(|t| &t.predicate == p) {
                    if let Object::Concept(target) = &triple.object {
                        out.push(Triple {
                            subject: target.clone(),
                            predicate: p.clone(),
                            object: Object::Concept(triple.subject.clone()),
                        });
                    }
                }
            }
            PropertyAxiom::InverseOf { property, inverse } => {
                for triple in known.iter().filter(|t| &t.predicate == property) {
                    if let Object::Concept(target) = &triple.object {
                        out.push(Triple {
                            subject: target.clone(),
                            predicate: inverse.clone(),
                            object: Object::Concept(triple.subject.clone()),
                        });
                    }
                }
            }
            PropertyAxiom::Transitive(p) => {
                let edges: Vec<(&ConceptId, &ConceptId)> = known
                    .iter()
                    .filter(|t| &t.predicate == p)
                    .filter_map(|t| match &t.object {
                        Object::Concept(target) => Some((&t.subject, target)),
                        Object::Literal(_) => None,
                    })
                    .collect();
                for (x, y) in &edges {
                    for (y2, z) in &edges {
                        if y == y2 {
                            out.push(Triple {
                                subject: (*x).clone(),
                                predicate: p.clone(),
                                object: Object::Concept((*z).clone()),
                            });
                        }
                    }
                }
            }
        }
    }

    out
}

// ---------------------------------------------------------------
// Ontología por referencia: un `Ontology` como documento OKF
// ---------------------------------------------------------------

/// Por qué una línea del cuerpo de un documento `type: ontology` no
/// se pudo interpretar como axioma. Siempre con línea, por la misma
/// razón que `okf_core::OkfError`: rechazar sin decir dónde no
/// enseña el formato.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OntologyDocError {
    /// La línea no tiene la forma `palabra_clave: resto`.
    Malformed {
        /// Línea del problema, 1-based.
        line: usize,
    },
    /// La palabra clave se reconoció pero `resto` no tiene la forma
    /// que esa palabra clave exige (p. ej. falta `->`, o un lado
    /// queda vacío).
    InvalidAxiom {
        /// Línea del problema, 1-based.
        line: usize,
        /// Qué se esperaba en su lugar.
        reason: String,
    },
    /// La palabra clave no es ninguna de las cinco soportadas.
    UnknownKeyword {
        /// Línea del problema, 1-based.
        line: usize,
        /// La palabra clave tal cual apareció.
        keyword: String,
    },
}

impl fmt::Display for OntologyDocError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OntologyDocError::Malformed { line } => {
                write!(f, "línea {line}: se esperaba 'palabra_clave: resto'")
            }
            OntologyDocError::InvalidAxiom { line, reason } => {
                write!(f, "línea {line}: {reason}")
            }
            OntologyDocError::UnknownKeyword { line, keyword } => write!(
                f,
                "línea {line}: palabra clave desconocida {keyword:?} (usa subclass_of | transitive | symmetric | sub_property_of | inverse_of)"
            ),
        }
    }
}

impl std::error::Error for OntologyDocError {}

/// Parsea el cuerpo de un documento `type: ontology` a una
/// [`Ontology`]. Cinco palabras clave, una por línea, sin YAML ni
/// JSON — el mismo espíritu que el subconjunto de frontmatter de
/// `okf-core`: un formato mínimo, deliberadamente rígido, que
/// rechaza con línea y motivo en vez de aceptar en silencio lo que
/// no entiende.
///
/// ```text
/// subclass_of: student -> person
/// transitive: depends_on
/// symmetric: married_to
/// sub_property_of: depends_on -> related
/// inverse_of: manages -> managed_by
/// ```
///
/// Líneas en blanco y comentarios (`# ...`) se ignoran. El cuerpo
/// completo de un documento OKF normal —el que
/// `okf_core::parse_document` ya separó del frontmatter— es lo que
/// se le pasa a esta función; no hace falta que el documento sea
/// "solo axiomas": cualquier prosa que no empiece con una de las
/// cinco palabras clave se rechaza, así que en la práctica sí lo es.
///
/// # Ejemplo
///
/// ```
/// use ontology_core::{parse_ontology_document, PropertyAxiom};
///
/// let body = "\
/// subclass_of: student -> person
/// subclass_of: person -> agent
/// transitive: depends_on
/// ";
/// let ontology = parse_ontology_document(body).unwrap();
/// assert_eq!(ontology.classes.len(), 2);
/// assert_eq!(ontology.properties, vec![PropertyAxiom::Transitive("depends_on".to_string())]);
/// ```
///
/// # Errores
///
/// ```
/// use ontology_core::{parse_ontology_document, OntologyDocError};
///
/// assert_eq!(
///     parse_ontology_document("depends_on transitiva quizás").unwrap_err(),
///     OntologyDocError::Malformed { line: 1 },
/// );
/// assert_eq!(
///     parse_ontology_document("transitive: ").unwrap_err(),
///     OntologyDocError::InvalidAxiom {
///         line: 1,
///         reason: "falta el nombre de la propiedad".to_string(),
///     },
/// );
/// ```
pub fn parse_ontology_document(body: &str) -> Result<Ontology, OntologyDocError> {
    let mut ontology = Ontology::default();
    for (idx, raw_line) in body.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (keyword, rest) =
            line.split_once(':').ok_or(OntologyDocError::Malformed { line: line_no })?;
        let keyword = keyword.trim();
        let rest = rest.trim();

        match keyword {
            "subclass_of" => {
                let (subclass, superclass) = split_arrow(rest, line_no)?;
                ontology.classes.push(SubClassOf { subclass, superclass });
            }
            "transitive" => {
                ontology.properties.push(PropertyAxiom::Transitive(non_empty(
                    rest,
                    line_no,
                    "falta el nombre de la propiedad",
                )?));
            }
            "symmetric" => {
                ontology.properties.push(PropertyAxiom::Symmetric(non_empty(
                    rest,
                    line_no,
                    "falta el nombre de la propiedad",
                )?));
            }
            "sub_property_of" => {
                let (sub, sup) = split_arrow(rest, line_no)?;
                ontology.properties.push(PropertyAxiom::SubPropertyOf { sub, sup });
            }
            "inverse_of" => {
                let (property, inverse) = split_arrow(rest, line_no)?;
                ontology.properties.push(PropertyAxiom::InverseOf { property, inverse });
            }
            other => {
                return Err(OntologyDocError::UnknownKeyword {
                    line: line_no,
                    keyword: other.to_string(),
                })
            }
        }
    }
    Ok(ontology)
}

fn non_empty(s: &str, line: usize, reason: &str) -> Result<String, OntologyDocError> {
    if s.is_empty() {
        return Err(OntologyDocError::InvalidAxiom { line, reason: reason.to_string() });
    }
    Ok(s.to_string())
}

fn split_arrow(rest: &str, line: usize) -> Result<(String, String), OntologyDocError> {
    let (a, b) = rest.split_once("->").ok_or_else(|| OntologyDocError::InvalidAxiom {
        line,
        reason: "se esperaba 'origen -> destino'".to_string(),
    })?;
    let a = a.trim();
    let b = b.trim();
    if a.is_empty() || b.is_empty() {
        return Err(OntologyDocError::InvalidAxiom {
            line,
            reason: "'origen -> destino': ningún lado puede quedar vacío".to_string(),
        });
    }
    Ok((a.to_string(), b.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> ConceptId {
        ConceptId::parse(s).unwrap()
    }

    fn type_fact(subject: &ConceptId, class: &str) -> Triple {
        Triple {
            subject: subject.clone(),
            predicate: RDF_TYPE.to_string(),
            object: Object::Literal(class.to_string()),
        }
    }

    fn link_fact(subject: &ConceptId, predicate: &str, target: &ConceptId) -> Triple {
        Triple {
            subject: subject.clone(),
            predicate: predicate.to_string(),
            object: Object::Concept(target.clone()),
        }
    }

    #[test]
    fn triples_from_document_extrae_tipo_tags_y_enlaces() {
        let raw = "---\ntype: person\ntags:\n  - staff\n  - rust\n---\nVer [[uses:libs/sqlx]] y [[people/bob]].\n";
        let doc = okf_core::parse_document(raw, &memory_model::Budget::default()).unwrap();
        let alice = id("people/alice");

        let triples = triples_from_document(&alice, &doc);

        assert!(triples.contains(&type_fact(&alice, "person")));
        assert!(triples.iter().any(
            |t| t.subject == alice && t.predicate == TAG && t.object == Object::Literal("staff".to_string())
        ));
        assert!(triples.contains(&link_fact(&alice, "uses", &id("libs/sqlx"))));
        assert!(triples.contains(&link_fact(&alice, RELATED, &id("people/bob"))));
    }

    #[test]
    fn subclass_of_es_transitivo_por_encadenamiento() {
        let alice = id("people/alice");
        let facts = vec![type_fact(&alice, "student")];
        let ontology = Ontology {
            classes: vec![
                SubClassOf { subclass: "student".to_string(), superclass: "person".to_string() },
                SubClassOf { subclass: "person".to_string(), superclass: "agent".to_string() },
            ],
            properties: vec![],
        };

        let out = materialize(&facts, &ontology, &ReasoningBudget::default());

        assert!(out.triples.contains(&type_fact(&alice, "student")));
        assert!(out.triples.contains(&type_fact(&alice, "person")));
        assert!(out.triples.contains(&type_fact(&alice, "agent")));
        assert!(!out.truncated_by_iterations && !out.truncated_by_triples);
    }

    #[test]
    fn propiedad_transitiva_deriva_la_cadena_completa() {
        let a = id("a");
        let b = id("b");
        let c = id("c");
        let facts = vec![link_fact(&a, "ancestor_of", &b), link_fact(&b, "ancestor_of", &c)];
        let ontology = Ontology {
            classes: vec![],
            properties: vec![PropertyAxiom::Transitive("ancestor_of".to_string())],
        };

        let out = materialize(&facts, &ontology, &ReasoningBudget::default());

        assert!(out.triples.contains(&link_fact(&a, "ancestor_of", &c)));
    }

    #[test]
    fn propiedad_simetrica_deriva_el_par_inverso() {
        let a = id("a");
        let b = id("b");
        let facts = vec![link_fact(&a, "married_to", &b)];
        let ontology = Ontology {
            classes: vec![],
            properties: vec![PropertyAxiom::Symmetric("married_to".to_string())],
        };

        let out = materialize(&facts, &ontology, &ReasoningBudget::default());

        assert!(out.triples.contains(&link_fact(&b, "married_to", &a)));
    }

    #[test]
    fn inverse_of_deriva_el_predicado_contrario() {
        let alice = id("people/alice");
        let bob = id("people/bob");
        let facts = vec![link_fact(&alice, "manages", &bob)];
        let ontology = Ontology {
            classes: vec![],
            properties: vec![PropertyAxiom::InverseOf {
                property: "manages".to_string(),
                inverse: "managed_by".to_string(),
            }],
        };

        let out = materialize(&facts, &ontology, &ReasoningBudget::default());

        assert!(out.triples.contains(&link_fact(&bob, "managed_by", &alice)));
    }

    #[test]
    fn sub_property_of_propaga_hacia_la_propiedad_general() {
        let alice = id("people/alice");
        let sqlx = id("libs/sqlx");
        let facts = vec![link_fact(&alice, "depends_on", &sqlx)];
        let ontology = Ontology {
            classes: vec![],
            properties: vec![PropertyAxiom::SubPropertyOf {
                sub: "depends_on".to_string(),
                sup: "related".to_string(),
            }],
        };

        let out = materialize(&facts, &ontology, &ReasoningBudget::default());

        assert!(out.triples.contains(&link_fact(&alice, "related", &sqlx)));
    }

    #[test]
    fn respeta_presupuesto_de_triples() {
        let a = id("a");
        let b = id("b");
        let c = id("c");
        let facts = vec![link_fact(&a, "ancestor_of", &b), link_fact(&b, "ancestor_of", &c)];
        let ontology = Ontology {
            classes: vec![],
            properties: vec![PropertyAxiom::Transitive("ancestor_of".to_string())],
        };
        let budget = ReasoningBudget { max_iterations: 16, max_triples: 2 };

        let out = materialize(&facts, &ontology, &budget);

        assert_eq!(out.triples.len(), 2);
        assert!(out.truncated_by_triples);
    }

    #[test]
    fn respeta_presupuesto_de_iteraciones() {
        // Cadena de 5 subClassOf: alcanzar la cima exige 4 rondas.
        // Con solo 1 iteración permitida, la clausura queda a medias.
        let alice = id("people/alice");
        let facts = vec![type_fact(&alice, "c0")];
        let classes = (0..4)
            .map(|i| SubClassOf { subclass: format!("c{i}"), superclass: format!("c{}", i + 1) })
            .collect();
        let ontology = Ontology { classes, properties: vec![] };
        let budget = ReasoningBudget { max_iterations: 1, max_triples: 10_000 };

        let out = materialize(&facts, &ontology, &budget);

        assert!(out.triples.contains(&type_fact(&alice, "c1")));
        assert!(!out.triples.contains(&type_fact(&alice, "c4")));
        assert!(out.truncated_by_iterations);
        assert!(!out.truncated_by_triples);
    }

    #[test]
    fn ciclos_de_subclass_of_no_cuelgan_el_motor() {
        let alice = id("people/alice");
        let facts = vec![type_fact(&alice, "a")];
        let ontology = Ontology {
            classes: vec![
                SubClassOf { subclass: "a".to_string(), superclass: "b".to_string() },
                SubClassOf { subclass: "b".to_string(), superclass: "a".to_string() },
            ],
            properties: vec![],
        };

        let out = materialize(&facts, &ontology, &ReasoningBudget::default());

        assert!(out.triples.contains(&type_fact(&alice, "a")));
        assert!(out.triples.contains(&type_fact(&alice, "b")));
        assert!(!out.truncated_by_iterations);
    }

    #[test]
    fn parse_ontology_document_lee_las_cinco_palabras_clave() {
        let body = "\
subclass_of: student -> person
transitive: depends_on
symmetric: married_to
sub_property_of: depends_on -> related
inverse_of: manages -> managed_by
";
        let ontology = parse_ontology_document(body).unwrap();
        assert_eq!(
            ontology.classes,
            vec![SubClassOf { subclass: "student".to_string(), superclass: "person".to_string() }]
        );
        assert_eq!(
            ontology.properties,
            vec![
                PropertyAxiom::Transitive("depends_on".to_string()),
                PropertyAxiom::Symmetric("married_to".to_string()),
                PropertyAxiom::SubPropertyOf { sub: "depends_on".to_string(), sup: "related".to_string() },
                PropertyAxiom::InverseOf { property: "manages".to_string(), inverse: "managed_by".to_string() },
            ]
        );
    }

    #[test]
    fn parse_ontology_document_ignora_blancos_y_comentarios() {
        let body = "\n  \n# comentario\nsubclass_of: a -> b\n   # otro comentario\n";
        let ontology = parse_ontology_document(body).unwrap();
        assert_eq!(ontology.classes.len(), 1);
    }

    #[test]
    fn parse_ontology_document_rechaza_linea_sin_dos_puntos() {
        assert_eq!(
            parse_ontology_document("esto no es un axioma").unwrap_err(),
            OntologyDocError::Malformed { line: 1 }
        );
    }

    #[test]
    fn parse_ontology_document_rechaza_keyword_desconocida() {
        assert_eq!(
            parse_ontology_document("owl_equivalent_class: a -> b").unwrap_err(),
            OntologyDocError::UnknownKeyword { line: 1, keyword: "owl_equivalent_class".to_string() }
        );
    }

    #[test]
    fn parse_ontology_document_rechaza_flecha_ausente() {
        assert_eq!(
            parse_ontology_document("subclass_of: solo-un-lado").unwrap_err(),
            OntologyDocError::InvalidAxiom {
                line: 1,
                reason: "se esperaba 'origen -> destino'".to_string()
            }
        );
    }

    #[test]
    fn parse_ontology_document_rechaza_lado_vacio() {
        assert_eq!(
            parse_ontology_document("inverse_of: manages -> ").unwrap_err(),
            OntologyDocError::InvalidAxiom {
                line: 1,
                reason: "'origen -> destino': ningún lado puede quedar vacío".to_string()
            }
        );
    }

    #[test]
    fn parse_ontology_document_reporta_la_linea_correcta_en_documentos_multilinea() {
        let body = "subclass_of: a -> b\ntransitive: x\nesto rompe\n";
        assert_eq!(
            parse_ontology_document(body).unwrap_err(),
            OntologyDocError::Malformed { line: 3 }
        );
    }

    #[test]
    fn parse_ontology_document_vacio_da_ontologia_vacia() {
        let ontology = parse_ontology_document("").unwrap();
        assert!(ontology.classes.is_empty() && ontology.properties.is_empty());
    }
}
