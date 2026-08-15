//! Herramientas de memoria MCP sobre cualquier [`MemoryRepository`].
//!
//! SOLID en juego:
//! - **D:** `MemoryTools<R>` es genérico sobre el trait del
//!   repositorio. Hoy se instancia con `InMemoryStore`; en el hito 2
//!   con el adaptador de Supabase, SIN tocar este archivo.
//! - **S:** este crate traduce entre el mundo JSON de MCP y el
//!   dominio tipado. No implementa ni protocolo ni almacenamiento.
//!
//! Las cuatro herramientas originales (pocas y orientadas a
//! resultados; el resto del archivo fue añadiendo más sin cambiar el
//! principio):
//! - `memory_search`  — candidatos compactos.
//! - `memory_resolve` — documento + vecindario acotado del grafo.
//! - `memory_commit`  — escritura con compare-and-swap.
//! - `memory_history` — revisiones compactas, paginadas.
//!
//! `spec_propose`/`spec_tasks`/`spec_status` añaden un flujo
//! spec-driven (requisitos + diseño acordados antes de implementar,
//! descompuestos en tareas rastreables) sin esquema nuevo: son
//! conceptos `type: spec`/`type: task` con la convención de siempre
//! (`[[rel:destino]]` para el enlace, tags para el estado) — cualquier
//! cliente MCP puede proponer o retomar, porque el estado vive en la
//! memoria compartida, no en una conversación concreta.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use consolidate_core::{ConsolidateError, DigestDecision, DigestEntity, SessionDigest};
use graph_core::NeighborSource;
use ingest_core::{IngestError, PlannedAction, SkillFormat, SourceFetcher};
use json_mini::{arr, n, obj, s, Value};
use mcp_core::{ToolError, ToolHandler, ToolSpec, UiResource};
use memory_model::{Budget, ConceptId, ContentId, Principal};
use ontology_core::{
    materialize, parse_ontology_document, triples_from_document, Object, Ontology, PropertyAxiom,
    ReasoningBudget, SubClassOf, Triple,
};
use std::collections::BTreeMap;
use store_core::{CommitRequest, MemoryRepository, SearchQuery, StoreError, StoreMaintenance, TripleStore};
use std::convert::Infallible;

/// La UI React de las 13 herramientas `memory_*` (`mcp-app/`),
/// compilada a un único HTML autocontenido y sincronizada aquí por
/// `mcp-app/scripts/sync-to-rust.mjs`. Un solo `include_str!`: las 13
/// entradas de [`MemoryTools::ui_resources`] comparten este mismo
/// `&'static str` bajo URIs distintas, sin duplicar el binario.
const APP_HTML: &str = include_str!("../assets/mcp-app.html");

/// El [`ToolHandler`] de memoria, genérico sobre el repositorio.
pub struct MemoryTools<R> {
    repo: R,
    actor: Principal,
    budget: Budget,
    fetcher: Option<Box<dyn SourceFetcher>>,
    /// Owners de fuentes cuyo contenido ya se considera revisado (p.
    /// ej. `anthropics`): [`ingest_core::scan_suspicious_patterns`] se
    /// sigue ejecutando igual, pero sus avisos no se incluyen en la
    /// respuesta — no aportan nada sobre una fuente de confianza y
    /// serían ruido. Ver [`MemoryTools::with_ingest`].
    trusted_owners: Vec<String>,
}

impl<R> MemoryTools<R>
where
    R: MemoryRepository + StoreMaintenance + NeighborSource<Error = Infallible> + TripleStore,
{
    /// Envuelve un repositorio con el actor y el presupuesto que
    /// gobernarán TODAS las llamadas. Sin capacidad de ingesta: la
    /// herramienta `skill_ingest` solo se anuncia tras
    /// [`MemoryTools::with_ingest`].
    pub fn new(repo: R, actor: Principal, budget: Budget) -> Self {
        MemoryTools { repo, actor, budget, fetcher: None, trusted_owners: Vec::new() }
    }

    /// Acceso de solo lectura al repositorio envuelto — para tests
    /// que necesitan verificar efectos secundarios (p. ej. que
    /// `memory_reason` persistió triples) sin pasar de nuevo por el
    /// protocolo JSON. Nunca lo uses fuera de tests: cualquier
    /// herramienta que necesite esto en producción es una señal de
    /// que le falta una respuesta propia, no de que le falte este
    /// atajo.
    pub fn repo(&self) -> &R {
        &self.repo
    }

    /// Activa `skill_ingest` con un descargador de fuentes. La
    /// conversión es SIEMPRE determinista (contenido original íntegro
    /// bajo cabecera OKF, etiquetado `verbatim-import`) — no hay
    /// síntesis con LLM: reescribir con un modelo no resuelve nada de
    /// licencia (sigue siendo obra derivada) y cuesta cuota por cada
    /// skill. `trusted_owners` son los owners cuyo escrutinio de
    /// contenido sospechoso se omite en la respuesta (ver el campo del
    /// mismo nombre).
    pub fn with_ingest(mut self, fetcher: Box<dyn SourceFetcher>, trusted_owners: Vec<String>) -> Self {
        self.fetcher = Some(fetcher);
        self.trusted_owners = trusted_owners;
        self
    }

    // ---- helpers de argumentos --------------------------------------

    fn arg_str(args: &Value, key: &str) -> Option<String> {
        args.get(key).and_then(|v| v.as_str()).map(str::to_string)
    }

    fn require_str(args: &Value, key: &str) -> Result<String, ToolError> {
        Self::arg_str(args, key)
            .ok_or_else(|| ToolError::InvalidArguments(format!("falta el argumento '{key}' (string)")))
    }

    fn arg_usize(args: &Value, key: &str) -> Result<Option<usize>, ToolError> {
        match args.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => v
                .as_u64()
                .map(|u| Some(u as usize))
                .ok_or_else(|| ToolError::InvalidArguments(format!("'{key}' debe ser un entero >= 0"))),
        }
    }

    fn concept_id(raw: &str) -> Result<ConceptId, ToolError> {
        ConceptId::parse(raw)
            .map_err(|e| ToolError::InvalidArguments(format!("concept_id inválido: {e}")))
    }

    /// Traduce errores de dominio a fallos que el MODELO puede leer.
    /// Un conflicto CAS incluye los hashes para releer y reintentar.
    fn domain_error(err: StoreError) -> ToolError {
        let payload = match &err {
            StoreError::Conflict(c) => obj([
                ("kind", s("revision_conflict")),
                ("expected_hash", opt_hash(c.expected)),
                ("current_hash", opt_hash(c.current)),
                ("incoming_hash", s(&c.incoming.to_hex())),
                ("hint", s("relee el documento, re-aplica tu cambio sobre current_hash y reintenta")),
            ]),
            StoreError::Okf(e) => obj([
                ("kind", s("invalid_okf_document")),
                ("detail", s(&e.to_string())),
            ]),
            StoreError::NotFound(id) => obj([
                ("kind", s("not_found")),
                ("concept_id", s(id.as_str())),
            ]),
            StoreError::Backend(msg) => obj([
                ("kind", s("backend_error")),
                ("detail", s(msg)),
            ]),
        };
        ToolError::Failed(json_mini::to_string(&payload))
    }

    /// Traduce errores de ingesta a fallos legibles por el MODELO.
    fn ingest_error(err: IngestError) -> ToolError {
        let kind = match &err {
            IngestError::InvalidSource(_) => "invalid_source",
            IngestError::Fetch(_) => "fetch_failed",
            IngestError::EmptySource => "source_empty",
        };
        ToolError::Failed(json_mini::to_string(&obj([
            ("kind", s(kind)),
            ("detail", s(&err.to_string())),
        ])))
    }

    // ---- herramientas ------------------------------------------------

    fn memory_search(&mut self, args: &Value) -> Result<Value, ToolError> {
        let query = SearchQuery {
            text: Self::arg_str(args, "query"),
            doc_type: Self::arg_str(args, "type"),
            tag: Self::arg_str(args, "tag"),
            path_prefix: Self::arg_str(args, "path_prefix"),
            limit: Self::arg_usize(args, "limit")?,
        };
        let hits = self.repo.search(&query, &self.budget).map_err(Self::domain_error)?;
        let items: Vec<Value> = hits
            .iter()
            .map(|h| {
                obj([
                    ("concept_id", s(h.concept_id.as_str())),
                    ("hash", s(&h.content_id.to_hex())),
                    ("type", s(&h.doc_type)),
                    ("title", h.title.as_deref().map(s).unwrap_or(Value::Null)),
                    ("tags", arr(h.tags.iter().map(|t| s(t)).collect())),
                    ("uri", s(&format!("okf://{}", h.concept_id))),
                ])
            })
            .collect();
        Ok(obj([("results", arr(items)), ("count", n(hits.len() as f64))]))
    }

    fn memory_resolve(&mut self, args: &Value) -> Result<Value, ToolError> {
        let id = Self::concept_id(&Self::require_str(args, "concept_id")?)?;
        let mut budget = self.budget;
        if let Some(depth) = Self::arg_usize(args, "depth")? {
            budget.max_graph_depth = (depth as u8).min(self.budget.max_graph_depth);
        }
        if let Some(max_bytes) = Self::arg_usize(args, "max_bytes")? {
            budget.max_response_bytes = max_bytes.min(self.budget.max_response_bytes);
        }

        let doc = self
            .repo
            .get(&id)
            .map_err(Self::domain_error)?
            .ok_or_else(|| Self::domain_error(StoreError::NotFound(id.clone())))?;

        let traversal = match graph_core::bounded_bfs(&self.repo, &id, &budget) {
            Ok(t) => t,
            Err(never) => match never {},
        };
        let neighborhood: Vec<Value> = traversal
            .visited
            .iter()
            .skip(1) // el origen ya va como documento completo
            .map(|v| {
                obj([
                    ("concept_id", s(v.id.as_str())),
                    ("depth", n(v.depth as f64)),
                    ("exists", Value::Bool(v.exists)),
                    ("uri", s(&format!("okf://{}", v.id))),
                    (
                        "parent",
                        v.parent.as_ref().map(|p| s(p.as_str())).unwrap_or(Value::Null),
                    ),
                ])
            })
            .collect();

        Ok(obj([
            (
                "document",
                obj([
                    ("concept_id", s(doc.concept_id.as_str())),
                    ("hash", s(&doc.content_id.to_hex())),
                    ("version", n(doc.version as f64)),
                    ("type", s(&doc.doc_type)),
                    ("title", doc.title.as_deref().map(s).unwrap_or(Value::Null)),
                    ("tags", arr(doc.tags.iter().map(|t| s(t)).collect())),
                    ("markdown", s(&doc.raw)),
                ]),
            ),
            ("neighborhood", arr(neighborhood)),
            (
                "truncated",
                obj([
                    ("by_nodes", Value::Bool(traversal.truncated_by_nodes)),
                    ("by_depth", Value::Bool(traversal.truncated_by_depth)),
                    ("by_bytes", Value::Bool(traversal.truncated_by_bytes)),
                ]),
            ),
        ]))
    }

    /// Traduce el argumento `classes` (array de `{subclass,
    /// superclass}`) y `properties` (array de `{kind, ...}`) a una
    /// [`Ontology`]. Ninguno de los dos es obligatorio: sin ellos,
    /// `Ontology::default()` hace que [`materialize`] sea un no-op —
    /// razonar es estrictamente opt-in (capítulo 18 del tutorial).
    fn parse_ontology(args: &Value) -> Result<Ontology, ToolError> {
        let classes = match args.get("classes").and_then(|v| v.as_array()) {
            None => Vec::new(),
            Some(items) => items
                .iter()
                .map(|item| {
                    Ok(SubClassOf {
                        subclass: Self::require_str(item, "subclass")?,
                        superclass: Self::require_str(item, "superclass")?,
                    })
                })
                .collect::<Result<Vec<_>, ToolError>>()?,
        };
        let properties = match args.get("properties").and_then(|v| v.as_array()) {
            None => Vec::new(),
            Some(items) => items
                .iter()
                .map(Self::parse_property_axiom)
                .collect::<Result<Vec<_>, ToolError>>()?,
        };
        Ok(Ontology { classes, properties })
    }

    fn parse_property_axiom(item: &Value) -> Result<PropertyAxiom, ToolError> {
        let kind = Self::require_str(item, "kind")?;
        match kind.as_str() {
            "transitive" => Ok(PropertyAxiom::Transitive(Self::require_str(item, "property")?)),
            "symmetric" => Ok(PropertyAxiom::Symmetric(Self::require_str(item, "property")?)),
            "sub_property_of" => Ok(PropertyAxiom::SubPropertyOf {
                sub: Self::require_str(item, "sub")?,
                sup: Self::require_str(item, "sup")?,
            }),
            "inverse_of" => Ok(PropertyAxiom::InverseOf {
                property: Self::require_str(item, "property")?,
                inverse: Self::require_str(item, "inverse")?,
            }),
            other => Err(ToolError::InvalidArguments(format!(
                "'kind' de axioma de propiedad desconocido: {other:?} (usa transitive | symmetric | sub_property_of | inverse_of)"
            ))),
        }
    }

    fn object_to_json(object: &Object) -> Value {
        match object {
            Object::Concept(id) => obj([("kind", s("concept")), ("value", s(id.as_str()))]),
            Object::Literal(text) => obj([("kind", s("literal")), ("value", s(text))]),
        }
    }

    fn triple_to_json(triple: &Triple, derived: bool) -> Value {
        obj([
            ("subject", s(triple.subject.as_str())),
            ("predicate", s(&triple.predicate)),
            ("object", Self::object_to_json(&triple.object)),
            ("derived", Value::Bool(derived)),
        ])
    }

    /// Carga la [`Ontology`] declarada en el CUERPO de un documento
    /// `type: ontology` referenciado por `id` (sintaxis: ver
    /// [`ontology_core::parse_ontology_document`]) — en el cuerpo, no
    /// en el frontmatter, porque el subconjunto YAML de `okf-core` no
    /// soporta listas de objetos anidados.
    fn load_ontology_document(&self, id: &ConceptId) -> Result<Ontology, ToolError> {
        let doc = self
            .repo
            .get(id)
            .map_err(Self::domain_error)?
            .ok_or_else(|| Self::domain_error(StoreError::NotFound(id.clone())))?;
        if doc.doc_type != "ontology" {
            return Err(ToolError::InvalidArguments(format!(
                "ontology_id {id} no es 'type: ontology' (es 'type: {}')",
                doc.doc_type
            )));
        }
        let parsed = okf_core::parse_document(&doc.raw, &self.budget)
            .map_err(StoreError::from)
            .map_err(Self::domain_error)?;
        let body = &doc.raw[parsed.body_offset..];
        parse_ontology_document(body)
            .map_err(|e| ToolError::InvalidArguments(format!("ontology_id {id}: {e}")))
    }

    /// Reúne los hechos (`rdf:type`, tags, enlaces tipados) del
    /// vecindario acotado de `concept_id` — el mismo recorrido que
    /// `memory_resolve` — y aplica el razonador de `ontology-core`
    /// sobre `classes`/`properties` inline y, si se da `ontology_id`,
    /// también sobre la ontología de ESE documento (las dos fuentes
    /// se combinan; ninguna reemplaza a la otra). Persiste la
    /// clausura derivada (agrupada por sujeto: cada concepto visto se
    /// reemplaza con los triples que le corresponden en ESTA
    /// ejecución) salvo que `persist: false`. Ver capítulo 18 del
    /// tutorial.
    fn memory_reason(&mut self, args: &Value) -> Result<Value, ToolError> {
        let root = Self::concept_id(&Self::require_str(args, "concept_id")?)?;
        let ontology_id: Option<ConceptId> =
            Self::arg_str(args, "ontology_id").map(|raw| Self::concept_id(&raw)).transpose()?;
        let mut budget = self.budget;
        if let Some(depth) = Self::arg_usize(args, "depth")? {
            budget.max_graph_depth = (depth as u8).min(self.budget.max_graph_depth);
        }

        let traversal = match graph_core::bounded_bfs(&self.repo, &root, &budget) {
            Ok(t) => t,
            Err(never) => match never {},
        };

        let mut facts: Vec<Triple> = Vec::new();
        for visited in &traversal.visited {
            if !visited.exists {
                continue; // enlace roto: no hay documento del que extraer hechos
            }
            let doc = self.repo.get(&visited.id).map_err(Self::domain_error)?;
            let Some(doc) = doc else { continue };
            let okf_doc = okf_core::OkfDocument {
                doc_type: doc.doc_type,
                title: doc.title,
                tags: doc.tags,
                extra: BTreeMap::new(),
                body_offset: 0,
                links: doc.links,
            };
            facts.extend(triples_from_document(&visited.id, &okf_doc));
        }

        let mut ontology = Self::parse_ontology(args)?;
        if let Some(id) = &ontology_id {
            let referenced = self.load_ontology_document(id)?;
            ontology.classes.extend(referenced.classes);
            ontology.properties.extend(referenced.properties);
        }
        let mut reasoning_budget = ReasoningBudget::default();
        if let Some(max_iterations) = Self::arg_usize(args, "max_iterations")? {
            reasoning_budget.max_iterations = max_iterations;
        }
        if let Some(max_triples) = Self::arg_usize(args, "max_triples")? {
            reasoning_budget.max_triples = max_triples;
        }

        let materialized = materialize(&facts, &ontology, &reasoning_budget);
        // Marca cada triple del resultado como asertado o derivado
        // comparando contra el conjunto de partida — `materialize`
        // devuelve la unión ya deduplicada, así que la pertenencia a
        // `asserted_set` es la única señal fiable (el ORDEN de
        // `facts` no sobrevive al `BTreeSet` interno).
        let asserted_set: std::collections::BTreeSet<Triple> = facts.into_iter().collect();

        let persist = args.get("persist").and_then(|v| v.as_bool()).unwrap_or(true);
        if persist {
            let mut by_subject: BTreeMap<ConceptId, Vec<Triple>> = BTreeMap::new();
            for triple in &materialized.triples {
                by_subject.entry(triple.subject.clone()).or_default().push(triple.clone());
            }
            for (subject, triples) in &by_subject {
                self.repo.save_triples(subject, triples).map_err(Self::domain_error)?;
            }
        }

        let neighborhood: Vec<Value> = traversal
            .visited
            .iter()
            .map(|v| {
                obj([
                    ("concept_id", s(v.id.as_str())),
                    ("depth", n(v.depth as f64)),
                    ("exists", Value::Bool(v.exists)),
                ])
            })
            .collect();

        Ok(obj([
            ("root", s(root.as_str())),
            ("asserted_count", n(asserted_set.len() as f64)),
            ("derived_count", n(materialized.triples.len().saturating_sub(asserted_set.len()) as f64)),
            (
                "triples",
                arr(materialized
                    .triples
                    .iter()
                    .map(|t| Self::triple_to_json(t, !asserted_set.contains(t)))
                    .collect()),
            ),
            ("neighborhood", arr(neighborhood)),
            ("ontology_id", ontology_id.as_ref().map(|id| s(id.as_str())).unwrap_or(Value::Null)),
            ("persisted", Value::Bool(persist)),
            (
                "truncated",
                obj([
                    ("traversal_by_nodes", Value::Bool(traversal.truncated_by_nodes)),
                    ("traversal_by_depth", Value::Bool(traversal.truncated_by_depth)),
                    ("traversal_by_bytes", Value::Bool(traversal.truncated_by_bytes)),
                    ("reasoning_by_iterations", Value::Bool(materialized.truncated_by_iterations)),
                    ("reasoning_by_triples", Value::Bool(materialized.truncated_by_triples)),
                ]),
            ),
        ]))
    }

    fn memory_commit(&mut self, args: &Value) -> Result<Value, ToolError> {
        let id = Self::concept_id(&Self::require_str(args, "concept_id")?)?;
        let markdown = Self::require_str(args, "markdown")?;
        let reason = Self::require_str(args, "reason")?;
        let expected = match args.get("expected_hash") {
            None | Some(Value::Null) => None,
            Some(v) => {
                let hex = v.as_str().ok_or_else(|| {
                    ToolError::InvalidArguments("'expected_hash' debe ser string".to_string())
                })?;
                Some(ContentId::from_hex(hex).ok_or_else(|| {
                    ToolError::InvalidArguments(
                        "'expected_hash' debe ser 64 caracteres hexadecimales".to_string(),
                    )
                })?)
            }
        };
        let dry_run = args.get("dry_run").and_then(|v| v.as_bool()).unwrap_or(false);

        if dry_run {
            let _doc = okf_core::parse_document(&markdown, &self.budget).map_err(StoreError::from).map_err(Self::domain_error)?;
            let current = self.repo.get(&id).map_err(Self::domain_error)?;
            let current_id = current.as_ref().map(|d| d.content_id);
            let incoming = ContentId(hash_core::sha256(markdown.as_bytes()));
            let decision = conflict_core::decide(current_id, expected, incoming);
            if let conflict_core::CommitDecision::Conflict(c) = decision {
                return Err(Self::domain_error(StoreError::Conflict(c)));
            }
            return Ok(obj([
                ("concept_id", s(id.as_str())),
                ("hash", s(&incoming.to_hex())),
                ("version", n((current.as_ref().map(|d| d.version).unwrap_or(0) + 1) as f64)),
                ("created", Value::Bool(current.is_none())),
                ("no_change", Value::Bool(matches!(decision, conflict_core::CommitDecision::NoChange))),
                ("revision_seq", Value::Null),
                ("dry_run", Value::Bool(true)),
            ]));
        }

        let outcome = self
            .repo
            .commit(
                CommitRequest { concept_id: id.clone(), expected, markdown, reason },
                &self.actor,
                &self.budget,
            )
            .map_err(Self::domain_error)?;

        Ok(obj([
            ("concept_id", s(id.as_str())),
            ("hash", s(&outcome.content_id.to_hex())),
            ("version", n(outcome.version as f64)),
            ("created", Value::Bool(outcome.created)),
            ("no_change", Value::Bool(outcome.no_change)),
            (
                "revision_seq",
                outcome.revision.as_ref().map(|r| n(r.seq as f64)).unwrap_or(Value::Null),
            ),
        ]))
    }

    /// Traduce el argumento `entities` (array de `{concept_id,
    /// relation?}`) a `Vec<DigestEntity>`, o vacío si se omite.
    fn parse_digest_entities(args: &Value) -> Result<Vec<DigestEntity>, ToolError> {
        let Some(items) = args.get("entities").and_then(|v| v.as_array()) else {
            return Ok(Vec::new());
        };
        items
            .iter()
            .map(|item| {
                let concept_id = Self::concept_id(&Self::require_str(item, "concept_id")?)?;
                let relation = Self::arg_str(item, "relation");
                Ok(DigestEntity { concept_id, relation })
            })
            .collect()
    }

    /// Traduce el argumento `decisions` (array de `{text,
    /// concept_id?}`) a `Vec<DigestDecision>`, o vacío si se omite.
    fn parse_digest_decisions(args: &Value) -> Result<Vec<DigestDecision>, ToolError> {
        let Some(items) = args.get("decisions").and_then(|v| v.as_array()) else {
            return Ok(Vec::new());
        };
        items
            .iter()
            .map(|item| {
                let text = Self::require_str(item, "text")?;
                let concept_id = Self::arg_str(item, "concept_id")
                    .map(|raw| Self::concept_id(&raw))
                    .transpose()?;
                Ok(DigestDecision { text, concept_id })
            })
            .collect()
    }

    /// Traduce un rechazo de [`consolidate_core::render_digest`] a un
    /// fallo legible por el MODELO.
    fn consolidate_error(err: ConsolidateError) -> ToolError {
        let kind = match &err {
            ConsolidateError::EmptyTitle => "empty_title",
            ConsolidateError::EmptySummary => "empty_summary",
            ConsolidateError::Okf(_) => "invalid_okf_document",
        };
        ToolError::Failed(json_mini::to_string(&obj([
            ("kind", s(kind)),
            ("detail", s(&err.to_string())),
        ])))
    }

    /// Consolida lo ocurrido en una sesión como un concepto
    /// `type: session-summary` durable. QUIEN redacta título, resumen,
    /// entidades y decisiones es el agente que llama a esta
    /// herramienta (ya es un LLM con todo el contexto de la sesión) —
    /// el servidor solo valida esa estructura y la renderiza a OKF de
    /// forma determinista vía `consolidate_core::render_digest`,
    /// nunca sintetiza contenido con ningún modelo propio. Mismo
    /// reparto de responsabilidades que `memory_commit` ya tiene con
    /// su markdown.
    fn memory_consolidate(&mut self, args: &Value) -> Result<Value, ToolError> {
        let title = Self::require_str(args, "title")?;
        let summary = Self::require_str(args, "summary")?;
        let entities = Self::parse_digest_entities(args)?;
        let decisions = Self::parse_digest_decisions(args)?;

        let id = match Self::arg_str(args, "concept_id") {
            Some(raw) => Self::concept_id(&raw)?,
            None => Self::concept_id(&format!("sessions/{}", ingest_core::slugify(&title)))?,
        };
        let reason = Self::arg_str(args, "reason").unwrap_or_else(|| "consolidación de sesión".to_string());
        let expected = match args.get("expected_hash") {
            None | Some(Value::Null) => None,
            Some(v) => {
                let hex = v.as_str().ok_or_else(|| {
                    ToolError::InvalidArguments("'expected_hash' debe ser string".to_string())
                })?;
                Some(ContentId::from_hex(hex).ok_or_else(|| {
                    ToolError::InvalidArguments(
                        "'expected_hash' debe ser 64 caracteres hexadecimales".to_string(),
                    )
                })?)
            }
        };

        let digest = SessionDigest { title, summary, entities, decisions };
        let markdown = consolidate_core::render_digest(&digest, &self.budget)
            .map_err(Self::consolidate_error)?;

        let outcome = self
            .repo
            .commit(
                CommitRequest { concept_id: id.clone(), expected, markdown, reason },
                &self.actor,
                &self.budget,
            )
            .map_err(Self::domain_error)?;

        Ok(obj([
            ("concept_id", s(id.as_str())),
            ("hash", s(&outcome.content_id.to_hex())),
            ("version", n(outcome.version as f64)),
            ("created", Value::Bool(outcome.created)),
            ("no_change", Value::Bool(outcome.no_change)),
            (
                "revision_seq",
                outcome.revision.as_ref().map(|r| n(r.seq as f64)).unwrap_or(Value::Null),
            ),
        ]))
    }

    fn memory_delete(&mut self, args: &Value) -> Result<Value, ToolError> {
        let id = Self::concept_id(&Self::require_str(args, "concept_id")?)?;
        let expected_hex = Self::require_str(args, "expected_hash")?;
        let expected = ContentId::from_hex(&expected_hex)
            .ok_or_else(|| ToolError::InvalidArguments("expected_hash inválido (esperaba hexadecimal de 64 caracteres)".to_string()))?;
        let reason = Self::require_str(args, "reason")?;

        let outcome = self.repo.delete(&id, expected, &self.actor, reason)
            .map_err(Self::domain_error)?;

        Ok(obj([
            ("concept_id", s(id.as_str())),
            ("hash", s(&outcome.content_id.to_hex())),
            ("version", n(outcome.version as f64)),
            ("revision_seq", n(outcome.revision.seq as f64)),
        ]))
    }

    fn memory_list(&mut self, args: &Value) -> Result<Value, ToolError> {
        let query = SearchQuery {
            text: None,
            doc_type: None,
            tag: None,
            path_prefix: Self::arg_str(args, "path_prefix"),
            limit: Self::arg_usize(args, "limit")?,
        };
        let hits = self.repo.search(&query, &self.budget).map_err(Self::domain_error)?;
        let items: Vec<Value> = hits
            .iter()
            .map(|h| {
                obj([
                    ("concept_id", s(h.concept_id.as_str())),
                    ("hash", s(&h.content_id.to_hex())),
                    ("type", s(&h.doc_type)),
                    ("title", h.title.as_deref().map(s).unwrap_or(Value::Null)),
                    ("tags", arr(h.tags.iter().map(|t| s(t)).collect())),
                    ("uri", s(&format!("okf://{}", h.concept_id))),
                ])
            })
            .collect();
        Ok(obj([("results", arr(items)), ("count", n(hits.len() as f64))]))
    }

    fn memory_backlinks(&mut self, args: &Value) -> Result<Value, ToolError> {
        let id = Self::concept_id(&Self::require_str(args, "concept_id")?)?;
        let backlinks = self.repo.backlinks(&id).map_err(Self::domain_error)?;
        let items: Vec<Value> = backlinks
            .iter()
            .map(|b| {
                obj([
                    ("source", obj([
                        ("concept_id", s(b.source.concept_id.as_str())),
                        ("hash", s(&b.source.content_id.to_hex())),
                        ("type", s(&b.source.doc_type)),
                        ("title", b.source.title.as_deref().map(s).unwrap_or(Value::Null)),
                        ("tags", arr(b.source.tags.iter().map(|t| s(t)).collect())),
                        ("uri", s(&format!("okf://{}", b.source.concept_id))),
                    ])),
                    ("rel", b.rel.as_deref().map(s).unwrap_or(Value::Null)),
                ])
            })
            .collect();
        Ok(obj([("backlinks", arr(items)), ("count", n(backlinks.len() as f64))]))
    }

    fn memory_embed(&mut self, args: &Value) -> Result<Value, ToolError> {
        let path_prefix = Self::arg_str(args, "path_prefix");
        let max = Self::arg_usize(args, "max")?.unwrap_or(10).max(1);

        let outcome = self.repo.embed_pending(path_prefix.as_deref(), max)
            .map_err(Self::domain_error)?;

        let embedded: Vec<Value> = outcome.embedded.iter().map(|id| s(id.as_str())).collect();
        let failed: Vec<Value> = outcome.failed.iter().map(|(id, err)| {
            obj([("concept_id", s(id.as_str())), ("error", s(err))])
        }).collect();

        Ok(obj([
            ("embedded", arr(embedded)),
            ("failed", arr(failed)),
            ("remaining", n(outcome.remaining as f64)),
        ]))
    }

    fn memory_patch(&mut self, args: &Value) -> Result<Value, ToolError> {
        let id = Self::concept_id(&Self::require_str(args, "concept_id")?)?;
        let expected_hex = Self::require_str(args, "expected_hash")?;
        let expected = ContentId::from_hex(&expected_hex)
            .ok_or_else(|| ToolError::InvalidArguments("expected_hash inválido".to_string()))?;
        let reason = Self::require_str(args, "reason")?;
        let dry_run = args.get("dry_run").and_then(|v| v.as_bool()).unwrap_or(false);

        let current_view = self.repo.get(&id).map_err(Self::domain_error)?
            .ok_or_else(|| ToolError::Failed(format!("no se encontró el documento {id} para aplicar el patch")))?;

        let mut set_fields = Vec::new();
        if let Some(set_val) = args.get("set") {
            if let Some(obj_map) = set_val.as_object() {
                for (k, v) in obj_map {
                    let fm_val = if let Some(s_val) = v.as_str() {
                        okf_core::FmValue::Scalar(s_val.to_string())
                    } else if let Some(arr_val) = v.as_array() {
                        let mut items = Vec::new();
                        for item in arr_val {
                            if let Some(s_item) = item.as_str() {
                                items.push(s_item.to_string());
                            } else {
                                return Err(ToolError::InvalidArguments("valores en listas de 'set' deben ser strings".to_string()));
                            }
                        }
                        okf_core::FmValue::List(items)
                    } else if let Some(b_val) = v.as_bool() {
                        okf_core::FmValue::Scalar(b_val.to_string())
                    } else if let Some(n_val) = v.as_f64() {
                        okf_core::FmValue::Scalar(n_val.to_string())
                    } else {
                        return Err(ToolError::InvalidArguments(format!("valor no soportado en 'set' para la clave {k}")));
                    };
                    set_fields.push((k.clone(), fm_val));
                }
            } else {
                return Err(ToolError::InvalidArguments("'set' debe ser un objeto/mapa".to_string()));
            }
        }

        let remove = args.get("remove")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|x| x.as_str().map(String::from)).collect())
            .unwrap_or_default();
        let add_tags: Vec<String> = args.get("add_tags")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|x| x.as_str().map(String::from)).collect())
            .unwrap_or_default();
        let remove_tags: Vec<String> = args.get("remove_tags")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|x| x.as_str().map(String::from)).collect())
            .unwrap_or_default();

        // Gauntlet de old-coder (github.com/AmazingAng/old-coder), aplicado
        // a `type: task`: "failing gauntlet blocks done" — el patch que
        // pondría `status-done` se rechaza si el cuerpo del documento no
        // tiene ya una sección `## Evidencia` (comandos + resultados reales,
        // añadida antes con `memory_commit`). `memory_patch` sigue sin tocar
        // el cuerpo; solo lee el que ya está comiteado.
        if current_view.doc_type == "task" {
            let mut final_tags: Vec<&str> = current_view.tags.iter().map(String::as_str).collect();
            final_tags.retain(|t| !remove_tags.iter().any(|r| r == t));
            for t in &add_tags {
                if !final_tags.contains(&t.as_str()) {
                    final_tags.push(t);
                }
            }
            if final_tags.contains(&"status-done") && !has_gauntlet_evidence(&current_view.raw) {
                return Err(ToolError::InvalidArguments(
                    "no se puede poner status-done en una tarea sin una sección '## Evidencia' \
                     en el cuerpo (comandos ejecutados y resultados reales — gauntlet de \
                     old-coder, github.com/AmazingAng/old-coder); añádela primero con \
                     memory_commit y luego reintenta el patch".to_string(),
                ));
            }
        }

        let patch = okf_core::FrontmatterPatch {
            set: set_fields,
            remove,
            add_tags,
            remove_tags,
        };

        let patched_markdown = okf_core::patch_frontmatter(&current_view.raw, &patch, &self.budget)
            .map_err(|e| ToolError::Failed(format!("error aplicando el patch: {e}")))?;

        if dry_run {
            let incoming = ContentId(hash_core::sha256(patched_markdown.as_bytes()));
            let decision = conflict_core::decide(Some(current_view.content_id), Some(expected), incoming);
            if let conflict_core::CommitDecision::Conflict(c) = decision {
                return Err(Self::domain_error(StoreError::Conflict(c)));
            }
            return Ok(obj([
                ("concept_id", s(id.as_str())),
                ("hash", s(&incoming.to_hex())),
                ("version", n((current_view.version + 1) as f64)),
                ("created", Value::Bool(false)),
                ("no_change", Value::Bool(matches!(decision, conflict_core::CommitDecision::NoChange))),
                ("revision_seq", Value::Null),
                ("dry_run", Value::Bool(true)),
            ]));
        }

        let outcome = self.repo.commit(
            CommitRequest { concept_id: id.clone(), expected: Some(expected), markdown: patched_markdown, reason },
            &self.actor,
            &self.budget,
        )
        .map_err(Self::domain_error)?;

        Ok(obj([
            ("concept_id", s(id.as_str())),
            ("hash", s(&outcome.content_id.to_hex())),
            ("version", n(outcome.version as f64)),
            ("created", Value::Bool(outcome.created)),
            ("no_change", Value::Bool(outcome.no_change)),
            (
                "revision_seq",
                outcome.revision.as_ref().map(|r| n(r.seq as f64)).unwrap_or(Value::Null),
            ),
        ]))
    }

    fn memory_bulk_commit(&mut self, args: &Value) -> Result<Value, ToolError> {
        let requests_arr = args.get("requests")
            .and_then(|v| v.as_array())
            .ok_or_else(|| ToolError::InvalidArguments("falta el argumento 'requests' (array)".to_string()))?;
        let atomic = args.get("atomic").and_then(|v| v.as_bool()).unwrap_or(true);

        let mut requests = Vec::new();
        for val in requests_arr {
            let concept_id = Self::concept_id(&Self::require_str(val, "concept_id")?)?;
            let markdown = Self::require_str(val, "markdown")?;
            let reason = Self::require_str(val, "reason")?;
            let expected_hash = Self::arg_str(val, "expected_hash");
            let expected = expected_hash
                .map(|hex| ContentId::from_hex(&hex)
                     .ok_or_else(|| ToolError::InvalidArguments(format!("expected_hash inválido para {concept_id}"))))
                .transpose()?;

            requests.push(CommitRequest {
                concept_id,
                markdown,
                expected,
                reason,
            });
        }

        let outcome = self.repo.commit_bulk(requests, atomic, &self.actor, &self.budget)
            .map_err(Self::domain_error)?;

        let items: Vec<Value> = outcome.items.iter().map(|item| {
            match item {
                store_core::BulkItem::Done(out) => obj([
                    ("status", s("done")),
                    ("hash", s(&out.content_id.to_hex())),
                    ("version", n(out.version as f64)),
                    ("created", Value::Bool(out.created)),
                    ("no_change", Value::Bool(out.no_change)),
                ]),
                store_core::BulkItem::Failed(err) => {
                    let err_val = match err {
                        StoreError::Conflict(c) => obj([
                            ("kind", s("revision_conflict")),
                            ("expected_hash", opt_hash(c.expected)),
                            ("current_hash", opt_hash(c.current)),
                        ]),
                        _ => obj([
                            ("kind", s("error")),
                            ("detail", s(&err.to_string())),
                        ]),
                    };
                    obj([("status", s("failed")), ("error", err_val)])
                }
                store_core::BulkItem::Skipped => obj([("status", s("skipped"))]),
            }
        }).collect();

        Ok(obj([
            ("applied", Value::Bool(outcome.applied)),
            ("items", arr(items)),
        ]))
    }

    fn skill_ingest(&mut self, args: &Value) -> Result<Value, ToolError> {
        let source = Self::require_str(args, "source")?;
        let path_prefix = Self::require_str(args, "path_prefix")?;
        ConceptId::parse(&path_prefix).map_err(|e| {
            ToolError::InvalidArguments(format!("path_prefix inválido: {e}"))
        })?;
        let format = match Self::arg_str(args, "format") {
            None => SkillFormat::Auto,
            Some(f) => SkillFormat::parse(&f).ok_or_else(|| {
                ToolError::InvalidArguments(format!(
                    "format desconocido {f:?} (usa auto | agentic-skills | shadcn | okf | raw)"
                ))
            })?,
        };
        let dry_run = args.get("dry_run").and_then(|v| v.as_bool()).unwrap_or(false);

        let fetcher = self.fetcher.as_ref().ok_or_else(|| {
            ToolError::Failed(json_mini::to_string(&obj([
                ("kind", s("ingest_unavailable")),
                ("detail", s("este despliegue no tiene descargador de fuentes configurado")),
            ])))
        })?;

        let files = fetcher.fetch(&source).map_err(Self::ingest_error)?;
        // Licencia: metadato determinista (sin ningún modelo), nunca
        // bloqueante. Si el fetcher no sabe determinarla (o falla la
        // consulta), se trata igual que "no determinada" — nunca es
        // motivo para fallar la ingesta completa.
        let license = fetcher.license_spdx_id(&source).unwrap_or(None);
        let plan = ingest_core::plan_ingest(
            &source,
            &files,
            format,
            &path_prefix,
            &self.budget,
            license.as_deref(),
        )
        .map_err(Self::ingest_error)?;
        let format_str = plan.format.as_str();
        // Fuentes de confianza (p. ej. `anthropics`): igual se ejecuta
        // el heurístico de contenido sospechoso, pero sus avisos no
        // viajan en la respuesta — ver `MemoryTools::trusted_owners`.
        let trusted = ingest_core::is_trusted_owner(&source, &self.trusted_owners);
        let warnings_of = |w: &[String]| -> Value {
            if trusted { arr(vec![]) } else { arr(w.iter().map(|x| s(x)).collect()) }
        };
        let mut skipped: Vec<Value> = plan
            .skipped
            .iter()
            .map(|(item, reason)| obj([("item", s(item)), ("reason", s(reason))]))
            .collect();

        if dry_run {
            let units: Vec<Value> = plan
                .units
                .iter()
                .map(|u| {
                    let action = match &u.action {
                        PlannedAction::Commit { .. } => "commit",
                        PlannedAction::Convert { .. } => "convert-verbatim",
                    };
                    obj([
                        ("concept_id", s(u.concept_id.as_str())),
                        ("title", s(&u.title)),
                        ("action", s(action)),
                        ("warnings", warnings_of(&u.warnings)),
                    ])
                })
                .collect();
            return Ok(obj([
                ("source_url", s(&source)),
                ("format", s(format_str)),
                ("license", opt_str(license.as_deref())),
                ("dry_run", Value::Bool(true)),
                ("units", arr(units)),
                ("skipped", arr(skipped)),
            ]));
        }

        // Materializar cada unidad: siempre determinista (OKF
        // verbatim, o cabecera generada + contenido original íntegro).
        // El contenido de una skill ES la skill.
        let mut requests: Vec<CommitRequest> = Vec::new();
        let mut unit_warnings: Vec<Value> = Vec::new();
        for u in plan.units {
            let markdown = match u.action {
                PlannedAction::Commit { markdown } => markdown,
                PlannedAction::Convert { deterministic } => deterministic,
            };
            unit_warnings.push(warnings_of(&u.warnings));
            requests.push(CommitRequest {
                concept_id: u.concept_id,
                expected: None,
                markdown,
                reason: format!("skill_ingest desde {source}"),
            });
        }

        let ids: Vec<ConceptId> = requests.iter().map(|r| r.concept_id.clone()).collect();
        let outcome = self
            .repo
            .commit_bulk(requests, false, &self.actor, &self.budget)
            .map_err(Self::domain_error)?;

        let mut concept_ids: Vec<Value> = Vec::new();
        let mut items: Vec<Value> = Vec::new();
        for ((id, warnings), item) in ids.iter().zip(unit_warnings).zip(outcome.items) {
            match item {
                store_core::BulkItem::Done(out) => {
                    concept_ids.push(s(id.as_str()));
                    items.push(obj([
                        ("concept_id", s(id.as_str())),
                        ("mode", s("verbatim")),
                        ("hash", s(&out.content_id.to_hex())),
                        ("version", n(out.version as f64)),
                        ("created", Value::Bool(out.created)),
                        ("warnings", warnings),
                    ]));
                }
                store_core::BulkItem::Failed(StoreError::Conflict(_)) => {
                    skipped.push(obj([
                        ("item", s(id.as_str())),
                        (
                            "reason",
                            s("ya existe con otro contenido: usa memory_commit con expected_hash para actualizarlo"),
                        ),
                    ]));
                }
                store_core::BulkItem::Failed(e) => {
                    skipped.push(obj([("item", s(id.as_str())), ("reason", s(&e.to_string()))]));
                }
                store_core::BulkItem::Skipped => {
                    skipped.push(obj([
                        ("item", s(id.as_str())),
                        ("reason", s("omitido por el lote")),
                    ]));
                }
            }
        }

        Ok(obj([
            ("source_url", s(&source)),
            ("format", s(format_str)),
            ("license", opt_str(license.as_deref())),
            ("ingested", n(concept_ids.len() as f64)),
            ("concept_ids", arr(concept_ids)),
            ("items", arr(items)),
            ("skipped", arr(skipped)),
        ]))
    }

    // ---- spec-driven development (requisitos → diseño → tareas) -----
    //
    // Tres herramientas sobre pura convención de OKF, sin esquema
    // nuevo: un `spec` es un concepto `type: spec` con secciones
    // "Requisitos"/"Diseño"; una `task` es `type: task` enlazada de
    // vuelta con `[[implements:<spec_id>]]`, y opcionalmente a otras
    // tareas con `[[depends_on:<task_id>]]`. El estado de ambos viaja
    // en un tag `status-*` (`memory_patch` ya sabe cambiarlo). Con
    // esto, cualquier cliente MCP (Claude, ChatGPT, u otro) puede
    // proponer specs y cualquier otro puede retomar el trabajo o
    // preguntar el progreso — el estado vive en la memoria compartida,
    // no en el contexto de una conversación concreta.
    //
    // `spec_status` calcula `next_pending` (lo que ya se puede
    // empezar) filtrando las tareas `status-pending` cuyas
    // `depends_on` NO están todas `done` — esas se cuentan aparte en
    // `waiting_on_dependencies`. Así "qué sigue" respeta el orden real
    // entre tareas, no solo su propio estado.

    fn spec_propose(&mut self, args: &Value) -> Result<Value, ToolError> {
        let id = Self::concept_id(&Self::require_str(args, "concept_id")?)?;
        let title = Self::require_str(args, "title")?;
        let requirements = Self::require_str(args, "requirements")?;
        let design = Self::require_str(args, "design")?;
        let reason = Self::arg_str(args, "reason").unwrap_or_else(|| "spec_propose".to_string());

        let markdown = format!(
            "---\ntype: spec\ntitle: {}\ntags:\n  - spec\n  - status-proposed\n---\n\n## Requisitos\n\n{}\n\n## Diseño\n\n{}\n",
            sanitize_title(&title),
            requirements.trim(),
            design.trim(),
        );

        let outcome = self
            .repo
            .commit(
                CommitRequest { concept_id: id.clone(), expected: None, markdown, reason },
                &self.actor,
                &self.budget,
            )
            .map_err(Self::domain_error)?;

        Ok(obj([
            ("concept_id", s(id.as_str())),
            ("hash", s(&outcome.content_id.to_hex())),
            ("version", n(outcome.version as f64)),
            ("created", Value::Bool(outcome.created)),
        ]))
    }

    fn spec_tasks(&mut self, args: &Value) -> Result<Value, ToolError> {
        let spec_id = Self::concept_id(&Self::require_str(args, "spec_id")?)?;
        self.repo
            .get(&spec_id)
            .map_err(Self::domain_error)?
            .ok_or_else(|| Self::domain_error(StoreError::NotFound(spec_id.clone())))?;

        let tasks_arr = args
            .get("tasks")
            .and_then(|v| v.as_array())
            .ok_or_else(|| ToolError::InvalidArguments("falta el argumento 'tasks' (array)".to_string()))?;
        if tasks_arr.is_empty() {
            return Err(ToolError::InvalidArguments("'tasks' no puede estar vacío".to_string()));
        }

        // Primera pasada: id/título/descripción/`depends_on` en bruto
        // de cada tarea, para poder resolver dependencias que apunten
        // a OTRA tarea de este mismo lote por su título exacto.
        struct Staged {
            concept_id: ConceptId,
            title: String,
            description: String,
            depends_on_raw: Vec<String>,
        }
        let mut staged: Vec<Staged> = Vec::new();
        for (i, val) in tasks_arr.iter().enumerate() {
            let title = Self::require_str(val, "title")?;
            let description = Self::arg_str(val, "description").unwrap_or_default();
            let depends_on_raw: Vec<String> = val
                .get("depends_on")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                .unwrap_or_default();
            let slug = ingest_core::slugify(&title);
            let concept_id = ConceptId::parse(&format!("{spec_id}/tasks/{:02}-{slug}", i + 1))
                .map_err(|e| ToolError::InvalidArguments(format!("tarea {i}: id inválido: {e}")))?;
            staged.push(Staged { concept_id, title, description, depends_on_raw });
        }

        // Segunda pasada: cada `depends_on` o coincide con el TÍTULO
        // exacto de otra tarea de este mismo lote, o es ya un
        // concept_id (dependencia cruzada con una tarea previa/de otro
        // spec) — se acepta tal cual, sin exigir que ya exista: puede
        // llegar en un `spec_tasks` posterior.
        let mut requests: Vec<CommitRequest> = Vec::new();
        for task in &staged {
            let mut targets: Vec<ConceptId> = Vec::new();
            for dep in &task.depends_on_raw {
                if let Some(other) = staged.iter().find(|t| &t.title == dep) {
                    targets.push(other.concept_id.clone());
                } else {
                    let target = ConceptId::parse(dep).map_err(|e| {
                        ToolError::InvalidArguments(format!(
                            "tarea {:?}: depends_on {dep:?} no es ni el título de otra tarea del lote ni un concept_id válido: {e}",
                            task.title
                        ))
                    })?;
                    targets.push(target);
                }
            }
            let mut body = format!("Parte de [[implements:{spec_id}]].\n");
            for t in &targets {
                body.push_str(&format!("\nDepende de [[depends_on:{t}]].\n"));
            }
            body.push('\n');
            body.push_str(task.description.trim());
            body.push('\n');
            let markdown = format!(
                "---\ntype: task\ntitle: {}\ntags:\n  - task\n  - status-pending\n---\n\n{}",
                sanitize_title(&task.title),
                body,
            );
            requests.push(CommitRequest {
                concept_id: task.concept_id.clone(),
                expected: None,
                markdown,
                reason: format!("spec_tasks desde {spec_id}"),
            });
        }

        let ids: Vec<ConceptId> = requests.iter().map(|r| r.concept_id.clone()).collect();
        let outcome = self
            .repo
            .commit_bulk(requests, false, &self.actor, &self.budget)
            .map_err(Self::domain_error)?;

        let mut task_ids: Vec<Value> = Vec::new();
        let mut skipped: Vec<Value> = Vec::new();
        for (id, item) in ids.iter().zip(outcome.items) {
            match item {
                store_core::BulkItem::Done(_) => task_ids.push(s(id.as_str())),
                store_core::BulkItem::Failed(e) => {
                    skipped.push(obj([("item", s(id.as_str())), ("reason", s(&e.to_string()))]));
                }
                store_core::BulkItem::Skipped => {
                    skipped.push(obj([("item", s(id.as_str())), ("reason", s("omitido por el lote"))]));
                }
            }
        }

        Ok(obj([
            ("spec_id", s(spec_id.as_str())),
            ("created", n(task_ids.len() as f64)),
            ("task_ids", arr(task_ids)),
            ("skipped", arr(skipped)),
        ]))
    }

    fn spec_status(&mut self, args: &Value) -> Result<Value, ToolError> {
        let spec_id = Self::concept_id(&Self::require_str(args, "spec_id")?)?;
        let spec = self
            .repo
            .get(&spec_id)
            .map_err(Self::domain_error)?
            .ok_or_else(|| Self::domain_error(StoreError::NotFound(spec_id.clone())))?;

        let backlinks = self.repo.backlinks(&spec_id).map_err(Self::domain_error)?;
        let tasks: Vec<_> = backlinks
            .iter()
            .filter(|b| b.rel.as_deref() == Some("implements") && b.source.doc_type == "task")
            .collect();
        let status_by_id: std::collections::HashMap<&ConceptId, &str> =
            tasks.iter().map(|t| (&t.source.concept_id, status_tag(&t.source.tags))).collect();

        let mut pending = 0usize;
        let mut in_progress = 0usize;
        let mut done = 0usize;
        let mut blocked = 0usize;
        let mut unknown = 0usize;
        let mut waiting_on_dependencies = 0usize;
        let mut next_pending: Vec<Value> = Vec::new();
        for t in &tasks {
            match status_tag(&t.source.tags) {
                "pending" => {
                    pending += 1;
                    // Una tarea pendiente solo es "próxima" si TODAS
                    // sus dependencias (`[[depends_on:...]]`) ya están
                    // done. Las de fuera de este spec se resuelven con
                    // un `get` puntual; no encontrarla cuenta como no
                    // resuelta (dependencia colgante = sigue bloqueada).
                    let full = self.repo.get(&t.source.concept_id).map_err(Self::domain_error)?;
                    let mut unmet = false;
                    if let Some(doc) = full {
                        for link in doc.links.iter().filter(|l| l.rel.as_deref() == Some("depends_on")) {
                            let done_dep = match status_by_id.get(&link.target) {
                                Some(st) => *st == "done",
                                None => self
                                    .repo
                                    .get(&link.target)
                                    .map_err(Self::domain_error)?
                                    .is_some_and(|d| status_tag(&d.tags) == "done"),
                            };
                            if !done_dep {
                                unmet = true;
                                break;
                            }
                        }
                    }
                    if unmet {
                        waiting_on_dependencies += 1;
                    } else {
                        next_pending.push(s(t.source.concept_id.as_str()));
                    }
                }
                "in_progress" => in_progress += 1,
                "done" => done += 1,
                "blocked" => blocked += 1,
                _ => unknown += 1,
            }
        }
        let total = tasks.len();

        Ok(obj([
            ("spec_id", s(spec_id.as_str())),
            ("spec_status", s(status_tag(&spec.tags))),
            ("spec_title", spec.title.as_deref().map(s).unwrap_or(Value::Null)),
            ("tasks_total", n(total as f64)),
            (
                "by_status",
                obj([
                    ("pending", n(pending as f64)),
                    ("in_progress", n(in_progress as f64)),
                    ("done", n(done as f64)),
                    ("blocked", n(blocked as f64)),
                    ("unknown", n(unknown as f64)),
                ]),
            ),
            ("progress", n(if total == 0 { 0.0 } else { done as f64 / total as f64 })),
            ("next_pending", arr(next_pending)),
            ("waiting_on_dependencies", n(waiting_on_dependencies as f64)),
        ]))
    }

    fn memory_validate(&mut self, args: &Value) -> Result<Value, ToolError> {
        let path_prefix = Self::arg_str(args, "path_prefix");
        let report = self.repo.validate(path_prefix.as_deref(), &self.budget)
            .map_err(Self::domain_error)?;

        let broken: Vec<Value> = report.broken_links.iter().map(|(src, trg)| {
            obj([("source", s(src.as_str())), ("target", s(trg.as_str()))])
        }).collect();

        let deleted: Vec<Value> = report.deleted_referenced.iter().map(|(src, trg)| {
            obj([("source", s(src.as_str())), ("target", s(trg.as_str()))])
        }).collect();

        let missing: Vec<Value> = report.missing_embeddings.iter().map(|id| s(id.as_str())).collect();

        Ok(obj([
            ("broken_links", arr(broken)),
            ("broken_links_total", n(report.broken_links_total as f64)),
            ("deleted_referenced", arr(deleted)),
            ("deleted_referenced_total", n(report.deleted_referenced_total as f64)),
            ("missing_embeddings", arr(missing)),
            ("missing_embeddings_total", n(report.missing_embeddings_total as f64)),
        ]))
    }

    fn memory_status(&mut self, _args: &Value) -> Result<Value, ToolError> {
        let status = self.repo.status().map_err(Self::domain_error)?;
        Ok(obj([
            ("documents", n(status.documents as f64)),
            ("deleted_documents", n(status.deleted_documents as f64)),
            ("missing_embeddings", n(status.missing_embeddings as f64)),
            ("broken_links", n(status.broken_links as f64)),
            ("deleted_referenced", n(status.deleted_referenced as f64)),
            ("outbox_pending", n(status.outbox_pending as f64)),
            ("outbox_failed", n(status.outbox_failed as f64)),
        ]))
    }

    fn memory_stats(&mut self, _args: &Value) -> Result<Value, ToolError> {
        let stats = self.repo.stats(&self.budget).map_err(Self::domain_error)?;

        let by_type: Vec<Value> = stats.by_type.iter().map(|(ty, cnt)| {
            obj([("type", s(ty)), ("count", n(*cnt as f64))])
        }).collect();

        let by_tag: Vec<Value> = stats.by_tag.iter().map(|(tag, cnt)| {
            obj([("tag", s(tag)), ("count", n(*cnt as f64))])
        }).collect();

        let top_linked: Vec<Value> = stats.top_linked.iter().map(|(id, cnt)| {
            obj([("concept_id", s(id.as_str())), ("incoming_links", n(*cnt as f64))])
        }).collect();

        let orphans: Vec<Value> = stats.orphans.iter().map(|id| s(id.as_str())).collect();

        Ok(obj([
            ("documents", n(stats.documents as f64)),
            ("deleted_documents", n(stats.deleted_documents as f64)),
            ("by_type", arr(by_type)),
            ("by_tag", arr(by_tag)),
            ("top_linked", arr(top_linked)),
            ("orphans", arr(orphans)),
        ]))
    }

    fn memory_history(&mut self, args: &Value) -> Result<Value, ToolError> {
        let id = Self::concept_id(&Self::require_str(args, "concept_id")?)?;
        let limit = Self::arg_usize(args, "limit")?.unwrap_or(10).clamp(1, 100);
        let before = Self::arg_usize(args, "before_seq")?.map(|v| v as u64);
        let revisions = self.repo.history(&id, limit, before).map_err(Self::domain_error)?;
        let items: Vec<Value> = revisions
            .iter()
            .map(|r| {
                obj([
                    ("seq", n(r.seq as f64)),
                    ("base_hash", opt_hash(r.base)),
                    ("result_hash", s(&r.result.to_hex())),
                    ("actor", s(&r.actor.subject)),
                    ("client_id", s(&r.actor.client_id)),
                    ("reason", s(&r.reason)),
                ])
            })
            .collect();
        Ok(obj([("concept_id", s(id.as_str())), ("revisions", arr(items))]))
    }
}

fn opt_hash(h: Option<ContentId>) -> Value {
    h.map(|h| s(&h.to_hex())).unwrap_or(Value::Null)
}

fn opt_str(v: Option<&str>) -> Value {
    v.map(s).unwrap_or(Value::Null)
}

/// Sanea un título arbitrario para usarlo como escalar de frontmatter:
/// sin saltos de línea ni comillas/`#` que rompan el YAML. Igual de
/// estricto que `ingest_core::finish_document`, pero vive aquí porque
/// `spec_propose`/`spec_tasks` no dependen de ese crate para nada más.
fn sanitize_title(raw: &str) -> String {
    let mut out = String::new();
    for ch in raw.chars() {
        match ch {
            '\n' | '\r' | '\t' => out.push(' '),
            '"' | '#' => {}
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    let trimmed = out.trim();
    if trimmed.is_empty() { "sin título".to_string() } else { trimmed.to_string() }
}

/// El primer tag `status-<algo>` de `tags`, sin el prefijo — o
/// `"unknown"` si no hay ninguno. Así se guarda el estado de un
/// `spec`/`task` (mutable con `memory_patch`, sin reescribir el
/// documento) y así lo agrega `spec_status` en una sola pasada.
fn status_tag(tags: &[String]) -> &str {
    tags.iter()
        .find_map(|t| t.strip_prefix("status-"))
        .unwrap_or("unknown")
}

/// El gauntlet de EVIDENCE de old-coder (github.com/AmazingAng/old-coder)
/// aplicado a una `task`: hace falta una sección `## Evidencia` con
/// contenido real (comandos ejecutados, resultados con números), no un
/// título vacío. Heurística deliberadamente simple — igual que
/// `spec_propose`/`spec_tasks`, pura convención sobre Markdown, sin
/// esquema nuevo: cuenta los caracteres no vacíos entre el encabezado y
/// el siguiente `## ` (o el final del documento).
fn has_gauntlet_evidence(raw: &str) -> bool {
    let Some(idx) = raw.find("## Evidencia") else { return false };
    let after = &raw[idx..];
    let body_start = after.find('\n').map(|i| i + 1).unwrap_or(after.len());
    let section = after[body_start..].split("\n## ").next().unwrap_or("");
    section.trim().chars().count() >= 40
}

/// Esquema JSON mínimo: `{"type":"object","properties":{...},"required":[...]}`.
fn schema<const N: usize, const M: usize>(
    props: [(&str, &str, &str); N],
    required: [&str; M],
) -> Value {
    let properties: Vec<(&str, Value)> = props
        .iter()
        .map(|(name, ty, desc)| (*name, obj([("type", s(ty)), ("description", s(desc))])))
        .collect();
    let mut properties_map = std::collections::BTreeMap::new();
    for (k, v) in properties {
        properties_map.insert(k.to_string(), v);
    }
    obj([
        ("type", s("object")),
        ("properties", Value::Object(properties_map)),
        ("required", arr(required.iter().map(|r| s(r)).collect())),
    ])
}

impl<R> ToolHandler for MemoryTools<R>
where
    R: MemoryRepository + StoreMaintenance + NeighborSource<Error = Infallible> + TripleStore,
{
    fn instructions(&self) -> Option<&str> {
        Some(include_str!("../assets/instructions.txt"))
    }

    fn tools(&self) -> Vec<ToolSpec> {
        let mut specs = vec![
            ToolSpec {
                name: "memory_search",
                description: include_str!("../assets/memory_search.txt"),
                input_schema: schema(
                    [
                        ("query", "string", "subcadena a buscar en id, título, tags y cuerpo"),
                        ("type", "string", "filtra por el campo 'type' del frontmatter"),
                        ("tag", "string", "filtra por tag exacto"),
                        ("path_prefix", "string", "filtra por prefijo de ruta lógica (ej. 'people')"),
                        ("limit", "integer", "máximo de resultados"),
                    ],
                    [],
                ),
                ui_resource_uri: Some("ui://okf-memory/memory_search"),
            },
            ToolSpec {
                name: "memory_resolve",
                description: include_str!("../assets/memory_resolve.txt"),
                input_schema: schema(
                    [
                        ("concept_id", "string", "id lógico, p. ej. people/alice"),
                        ("depth", "integer", "profundidad máxima del vecindario del grafo a retornar"),
                        ("max_bytes", "integer", "presupuesto de bytes máximo para la respuesta (truncado automático si excede)"),
                    ],
                    ["concept_id"],
                ),
                ui_resource_uri: Some("ui://okf-memory/memory_resolve"),
            },
            ToolSpec {
                name: "memory_reason",
                description: include_str!("../assets/memory_reason.txt"),
                input_schema: schema(
                    [
                        ("concept_id", "string", "id lógico raíz: mismo vecindario acotado que memory_resolve"),
                        ("depth", "integer", "profundidad máxima del vecindario a considerar (por defecto el presupuesto del servidor)"),
                        ("ontology_id", "string", "concept_id de un documento 'type: ontology' cuyo CUERPO declara axiomas (una línea por axioma: 'subclass_of: X -> Y', 'transitive: P', 'symmetric: P', 'sub_property_of: X -> Y', 'inverse_of: X -> Y'); se COMBINA con classes/properties inline, no los reemplaza"),
                        ("classes", "array", "axiomas de subclase inline: lista de {subclass, superclass} (rdfs:subClassOf)"),
                        ("properties", "array", "axiomas de propiedad inline: lista de {kind, ...}. kind='transitive'|'symmetric' con {property}; kind='sub_property_of' con {sub, sup}; kind='inverse_of' con {property, inverse}"),
                        ("max_iterations", "integer", "tope de rondas de punto fijo (por defecto 16)"),
                        ("max_triples", "integer", "tope de triples totales, asertados + derivados (por defecto 10000)"),
                        ("persist", "boolean", "si es true (por defecto), guarda los triples derivados para lecturas futuras sin volver a razonar"),
                    ],
                    ["concept_id"],
                ),
                ui_resource_uri: Some("ui://okf-memory/memory_reason"),
            },
            ToolSpec {
                name: "memory_commit",
                description: include_str!("../assets/memory_commit.txt"),
                input_schema: schema(
                    [
                        ("concept_id", "string", "id lógico del concepto (ej: people/alice, projects/mcp)"),
                        ("markdown", "string", "documento completo con frontmatter YAML + Markdown, ej:\n---\ntype: person\ntitle: Alice\ntags:\n  - dev\n---\nCuerpo del documento en Markdown con [[enlaces]] a otros concept_id."),
                        ("reason", "string", "motivo o explicación del cambio (se registrará en la historia de revisiones)"),
                        ("expected_hash", "string", "hash SHA-256 hex del contenido actual obtenido previamente vía memory_resolve (obligatorio para actualizaciones, omitir en creación)"),
                        ("dry_run", "boolean", "si es true, valida el documento y chequea conflictos sin guardar nada"),
                    ],
                    ["concept_id", "markdown", "reason"],
                ),
                ui_resource_uri: Some("ui://okf-memory/memory_commit"),
            },
            ToolSpec {
                name: "memory_consolidate",
                description: include_str!("../assets/memory_consolidate.txt"),
                input_schema: schema(
                    [
                        ("title", "string", "título corto de la sesión (deriva el concept_id si no se da 'concept_id')"),
                        ("summary", "string", "resumen en prosa de lo ocurrido; puede contener [[enlaces]] OKF normales"),
                        ("entities", "array", "entidades relacionadas: lista de {concept_id, relation?} (relation en prosa libre)"),
                        ("decisions", "array", "decisiones tomadas: lista de {text, concept_id?}"),
                        ("concept_id", "string", "id lógico del documento de sesión (por defecto sessions/<slug-del-título>)"),
                        ("reason", "string", "motivo para la historia de revisiones (por defecto 'consolidación de sesión')"),
                        ("expected_hash", "string", "hash SHA-256 hex leído previamente, obligatorio para actualizar una sesión ya existente"),
                    ],
                    ["title", "summary"],
                ),
                ui_resource_uri: None,
            },
            ToolSpec {
                name: "memory_history",
                description: include_str!("../assets/memory_history.txt"),
                input_schema: schema(
                    [
                        ("concept_id", "string", "id lógico del concepto"),
                        ("limit", "integer", "máximo de revisiones a retornar (1-100)"),
                        ("before_seq", "integer", "solo revisiones anteriores a este seq (para paginar)"),
                    ],
                    ["concept_id"],
                ),
                ui_resource_uri: Some("ui://okf-memory/memory_history"),
            },
            ToolSpec {
                name: "memory_delete",
                description: include_str!("../assets/memory_delete.txt"),
                input_schema: schema(
                    [
                        ("concept_id", "string", "id lógico del concepto a borrar"),
                        ("expected_hash", "string", "hash SHA-256 hex del contenido actual"),
                        ("reason", "string", "motivo de la baja o borrado lógico"),
                    ],
                    ["concept_id", "expected_hash", "reason"],
                ),
                ui_resource_uri: Some("ui://okf-memory/memory_delete"),
            },
            ToolSpec {
                name: "memory_list",
                description: include_str!("../assets/memory_list.txt"),
                input_schema: schema(
                    [
                        ("path_prefix", "string", "prefijo de ruta por segmentos (ej: 'people')"),
                        ("limit", "integer", "máximo de resultados a retornar"),
                    ],
                    [],
                ),
                ui_resource_uri: Some("ui://okf-memory/memory_list"),
            },
            ToolSpec {
                name: "memory_backlinks",
                description: include_str!("../assets/memory_backlinks.txt"),
                input_schema: schema(
                    [
                        ("concept_id", "string", "id lógico del concepto de interés"),
                    ],
                    ["concept_id"],
                ),
                ui_resource_uri: Some("ui://okf-memory/memory_backlinks"),
            },
            ToolSpec {
                name: "memory_embed",
                description: include_str!("../assets/memory_embed.txt"),
                input_schema: schema(
                    [
                        ("path_prefix", "string", "opcional, filtra por prefijo de ruta lógica"),
                        ("max", "integer", "lote máximo de documentos a procesar en esta llamada"),
                    ],
                    [],
                ),
                ui_resource_uri: Some("ui://okf-memory/memory_embed"),
            },
            ToolSpec {
                name: "memory_patch",
                description: include_str!("../assets/memory_patch.txt"),
                input_schema: schema(
                    [
                        ("concept_id", "string", "id lógico del concepto"),
                        ("expected_hash", "string", "hash SHA-256 hex del contenido actual"),
                        ("reason", "string", "motivo del cambio de metadatos"),
                        ("set", "object", "mapa de campos clave-valor a escribir/reemplazar en el frontmatter"),
                        ("remove", "array", "lista de claves de frontmatter a eliminar (ej. ['tags'])"),
                        ("add_tags", "array", "lista de tags a añadir a la lista existente"),
                        ("remove_tags", "array", "lista de tags a eliminar de la lista existente"),
                        ("dry_run", "boolean", "si es true, simula el patch sin persistir"),
                    ],
                    ["concept_id", "expected_hash", "reason"],
                ),
                ui_resource_uri: Some("ui://okf-memory/memory_patch"),
            },
            ToolSpec {
                name: "memory_bulk_commit",
                description: include_str!("../assets/memory_bulk_commit.txt"),
                input_schema: schema(
                    [
                        ("requests", "array", "lista de peticiones de commit (cada una con concept_id, markdown, reason, y expected_hash opcional)"),
                        ("atomic", "boolean", "si es true, revierte todo el lote ante cualquier fallo o conflicto de CAS (rollback)"),
                    ],
                    ["requests"],
                ),
                ui_resource_uri: Some("ui://okf-memory/memory_bulk_commit"),
            },
            ToolSpec {
                name: "memory_validate",
                description: include_str!("../assets/memory_validate.txt"),
                input_schema: schema(
                    [
                        ("path_prefix", "string", "opcional, valida solo bajo este prefijo de ruta"),
                    ],
                    [],
                ),
                ui_resource_uri: Some("ui://okf-memory/memory_validate"),
            },
            ToolSpec {
                name: "memory_status",
                description: include_str!("../assets/memory_status.txt"),
                input_schema: schema([], []),
                ui_resource_uri: Some("ui://okf-memory/memory_status"),
            },
            ToolSpec {
                name: "memory_stats",
                description: include_str!("../assets/memory_stats.txt"),
                input_schema: schema([], []),
                ui_resource_uri: Some("ui://okf-memory/memory_stats"),
            },
            ToolSpec {
                name: "spec_propose",
                description: include_str!("../assets/spec_propose.txt"),
                input_schema: schema(
                    [
                        ("concept_id", "string", "id lógico del spec, p. ej. 'specs/skill-ingest-v2'"),
                        ("title", "string", "título humano del spec"),
                        ("requirements", "string", "sección de requisitos, en Markdown libre"),
                        ("design", "string", "sección de diseño técnico, en Markdown libre"),
                        ("reason", "string", "motivo del commit (opcional, para la historia de revisiones)"),
                    ],
                    ["concept_id", "title", "requirements", "design"],
                ),
                ui_resource_uri: Some("ui://okf-memory/spec_propose"),
            },
            ToolSpec {
                name: "spec_tasks",
                description: include_str!("../assets/spec_tasks.txt"),
                input_schema: schema(
                    [
                        ("spec_id", "string", "concept_id de un spec ya creado con spec_propose"),
                        ("tasks", "array", "lista de tareas: cada una {title, description opcional, depends_on opcional (lista de títulos de otras tareas de este lote, o concept_ids de tareas ya existentes)}"),
                    ],
                    ["spec_id", "tasks"],
                ),
                ui_resource_uri: Some("ui://okf-memory/spec_tasks"),
            },
            ToolSpec {
                name: "spec_status",
                description: include_str!("../assets/spec_status.txt"),
                input_schema: schema(
                    [("spec_id", "string", "concept_id de un spec")],
                    ["spec_id"],
                ),
                ui_resource_uri: Some("ui://okf-memory/spec_status"),
            },
        ];
        // `skill_ingest` solo se anuncia si el despliegue configuró un
        // descargador de fuentes (adaptador `ingest-http`): anunciar
        // una herramienta que siempre falla no ayuda al modelo.
        if self.fetcher.is_some() {
            specs.push(ToolSpec {
                name: "skill_ingest",
                description: "Ingiere skills desde una fuente externa (repo de GitHub, subcarpeta o archivo; también el atajo 'owner/repo') SIN pasar el contenido por tu contexto ni por ningún LLM del servidor: el servidor descarga, detecta el formato (SKILL.md, shadcn, OKF, markdown suelto) y commitea un concepto por skill conservando el contenido original ÍNTEGRO bajo una cabecera OKF (nunca reescribe ni resume con IA). La respuesta incluye 'license' (identificador SPDX detectado, informativo, nunca bloquea) y 'warnings' por unidad (heurísticos de texto sin modelo sobre contenido potencialmente sospechoso, p. ej. posible prompt injection — revísalos tú antes de confiar en el contenido; vacío para fuentes de confianza configuradas en el servidor).",
                input_schema: schema(
                    [
                        ("source", "string", "URL de repo, subcarpeta o archivo (GitHub u otra), o el atajo 'owner/repo'"),
                        ("path_prefix", "string", "prefijo lógico destino, p. ej. 'skills/programming'"),
                        ("format", "string", "auto | agentic-skills | shadcn | okf | raw (por defecto auto)"),
                        ("dry_run", "boolean", "si es true, devuelve el plan (unidades, acciones y warnings) sin guardar nada"),
                    ],
                    ["source", "path_prefix"],
                ),
                ui_resource_uri: Some("ui://okf-memory/skill_ingest"),
            });
        }
        specs
    }

    fn ui_resources(&self) -> Vec<UiResource> {
        // Un `UiResource` por herramienta, con URI propia — todas
        // sirven el MISMO `APP_HTML` (un solo `include_str!`;
        // `&'static str` es puntero+longitud, así que repetirlo aquí
        // no duplica nada en el binario). La URI propia por tool NO es
        // cosmética: varios hosts MCP Apps (heredado del Apps SDK de
        // OpenAI — ver capítulo 15 del tutorial) reutilizan el iframe
        // ya abierto cuando `_meta.ui.resourceUri` no cambia entre una
        // llamada y la siguiente. Con una única URI compartida entre
        // varias herramientas, invocar `memory_search` justo después
        // de `memory_resolve` podía dejar la vista pegada al
        // `toolName` de la primera llamada, o directamente no reabrir
        // el panel al ver "la misma" URI de siempre. Con URI distinta
        // por herramienta, cada llamada es inequívocamente un recurso
        // nuevo para el host.
        const DESCRIPTION: &str = "UI React interactiva (tema brutalista): formularios, grafo de conceptos (React Flow) y gráficas (Recharts). Compilada en un único HTML autocontenido desde mcp-app/ (ver mcp-app/README.md); enruta internamente por el nombre de la herramienta invocada.";
        let mut resources = vec![
            UiResource { uri: "ui://okf-memory/memory_search", name: "okf-memory · Buscar", description: DESCRIPTION, html: APP_HTML },
            UiResource { uri: "ui://okf-memory/memory_resolve", name: "okf-memory · Resolver", description: DESCRIPTION, html: APP_HTML },
            UiResource { uri: "ui://okf-memory/memory_reason", name: "okf-memory · Razonar", description: DESCRIPTION, html: APP_HTML },
            UiResource { uri: "ui://okf-memory/memory_commit", name: "okf-memory · Commit", description: DESCRIPTION, html: APP_HTML },
            UiResource { uri: "ui://okf-memory/memory_history", name: "okf-memory · Historial", description: DESCRIPTION, html: APP_HTML },
            UiResource { uri: "ui://okf-memory/memory_delete", name: "okf-memory · Borrar", description: DESCRIPTION, html: APP_HTML },
            UiResource { uri: "ui://okf-memory/memory_list", name: "okf-memory · Listar", description: DESCRIPTION, html: APP_HTML },
            UiResource { uri: "ui://okf-memory/memory_backlinks", name: "okf-memory · Backlinks", description: DESCRIPTION, html: APP_HTML },
            UiResource { uri: "ui://okf-memory/memory_embed", name: "okf-memory · Embeddings", description: DESCRIPTION, html: APP_HTML },
            UiResource { uri: "ui://okf-memory/memory_patch", name: "okf-memory · Patch", description: DESCRIPTION, html: APP_HTML },
            UiResource { uri: "ui://okf-memory/memory_bulk_commit", name: "okf-memory · Commit en lote", description: DESCRIPTION, html: APP_HTML },
            UiResource { uri: "ui://okf-memory/memory_validate", name: "okf-memory · Validar", description: DESCRIPTION, html: APP_HTML },
            UiResource { uri: "ui://okf-memory/memory_status", name: "okf-memory · Estado", description: DESCRIPTION, html: APP_HTML },
            UiResource { uri: "ui://okf-memory/memory_stats", name: "okf-memory · Estadísticas", description: DESCRIPTION, html: APP_HTML },
            UiResource { uri: "ui://okf-memory/spec_propose", name: "okf-memory · Proponer spec", description: DESCRIPTION, html: APP_HTML },
            UiResource { uri: "ui://okf-memory/spec_tasks", name: "okf-memory · Tareas de spec", description: DESCRIPTION, html: APP_HTML },
            UiResource { uri: "ui://okf-memory/spec_status", name: "okf-memory · Estado de spec", description: DESCRIPTION, html: APP_HTML },
        ];
        // Mismo guard que `tools()`: sin descargador de fuentes
        // configurado, `skill_ingest` no se anuncia — así que tampoco
        // tiene sentido anunciar su vista.
        if self.fetcher.is_some() {
            resources.push(UiResource {
                uri: "ui://okf-memory/skill_ingest",
                name: "okf-memory · Ingerir skill",
                description: DESCRIPTION,
                html: APP_HTML,
            });
        }
        resources
    }

    fn call(&mut self, name: &str, arguments: &Value) -> Result<Value, ToolError> {
        match name {
            "memory_search" => self.memory_search(arguments),
            "memory_resolve" => self.memory_resolve(arguments),
            "memory_reason" => self.memory_reason(arguments),
            "memory_commit" => self.memory_commit(arguments),
            "memory_consolidate" => self.memory_consolidate(arguments),
            "memory_history" => self.memory_history(arguments),
            "memory_delete" => self.memory_delete(arguments),
            "memory_list" => self.memory_list(arguments),
            "memory_backlinks" => self.memory_backlinks(arguments),
            "memory_embed" => self.memory_embed(arguments),
            "memory_patch" => self.memory_patch(arguments),
            "memory_bulk_commit" => self.memory_bulk_commit(arguments),
            "skill_ingest" => self.skill_ingest(arguments),
            "memory_validate" => self.memory_validate(arguments),
            "memory_status" => self.memory_status(arguments),
            "memory_stats" => self.memory_stats(arguments),
            "spec_propose" => self.spec_propose(arguments),
            "spec_tasks" => self.spec_tasks(arguments),
            "spec_status" => self.spec_status(arguments),
            _ => Err(ToolError::UnknownTool),
        }
    }
}
