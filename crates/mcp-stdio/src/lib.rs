//! Herramientas de memoria MCP sobre cualquier [`MemoryRepository`].
//!
//! SOLID en juego:
//! - **D:** `MemoryTools<R>` es genérico sobre el trait del
//!   repositorio. Hoy se instancia con `InMemoryStore`; en el hito 2
//!   con el adaptador de Supabase, SIN tocar este archivo.
//! - **S:** este crate traduce entre el mundo JSON de MCP y el
//!   dominio tipado. No implementa ni protocolo ni almacenamiento.
//!
//! Las cuatro herramientas (pocas y orientadas a resultados):
//! - `memory_search`  — candidatos compactos.
//! - `memory_resolve` — documento + vecindario acotado del grafo.
//! - `memory_commit`  — escritura con compare-and-swap.
//! - `memory_history` — revisiones compactas, paginadas.

#![forbid(unsafe_code)]

use graph_core::NeighborSource;
use json_mini::{arr, n, obj, s, Value};
use mcp_core::{ToolError, ToolHandler, ToolSpec};
use memory_model::{Budget, ConceptId, ContentId, Principal};
use memory_store::{CommitRequest, MemoryRepository, SearchQuery, StoreError};
use std::convert::Infallible;

pub struct MemoryTools<R> {
    repo: R,
    actor: Principal,
    budget: Budget,
}

impl<R> MemoryTools<R>
where
    R: MemoryRepository + NeighborSource<Error = Infallible>,
{
    pub fn new(repo: R, actor: Principal, budget: Budget) -> Self {
        MemoryTools { repo, actor, budget }
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

    // ---- herramientas ------------------------------------------------

    fn memory_search(&mut self, args: &Value) -> Result<Value, ToolError> {
        let query = SearchQuery {
            text: Self::arg_str(args, "query"),
            doc_type: Self::arg_str(args, "type"),
            tag: Self::arg_str(args, "tag"),
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
    R: MemoryRepository + NeighborSource<Error = Infallible>,
{
    fn tools(&self) -> Vec<ToolSpec> {
        vec![
            ToolSpec {
                name: "memory_search",
                description: "Busca conceptos en la memoria. Devuelve candidatos compactos con su hash y URI; usa memory_resolve para leer el contenido.",
                input_schema: schema(
                    [
                        ("query", "string", "subcadena a buscar en id, título, tags y cuerpo"),
                        ("type", "string", "filtra por el campo 'type' del frontmatter"),
                        ("tag", "string", "filtra por tag exacto"),
                        ("limit", "integer", "máximo de resultados"),
                    ],
                    [],
                ),
            },
            ToolSpec {
                name: "memory_resolve",
                description: "Devuelve un concepto completo (Markdown exacto) más su vecindario acotado en el grafo de enlaces [[...]]. Indica si el resultado fue truncado por presupuesto.",
                input_schema: schema(
                    [
                        ("concept_id", "string", "id lógico, p. ej. people/alice"),
                        ("depth", "integer", "profundidad máxima del vecindario"),
                        ("max_bytes", "integer", "presupuesto de bytes de la respuesta"),
                    ],
                    ["concept_id"],
                ),
            },
            ToolSpec {
                name: "memory_commit",
                description: "Escribe un documento OKF (frontmatter YAML + Markdown) con concurrencia optimista: pasa el expected_hash que leíste; si la memoria cambió en medio recibirás un revision_conflict con los hashes para reintentar. Omite expected_hash solo al crear.",
                input_schema: schema(
                    [
                        ("concept_id", "string", "id lógico del concepto"),
                        ("markdown", "string", "documento completo, con frontmatter '---'"),
                        ("reason", "string", "por qué se hace este cambio"),
                        ("expected_hash", "string", "hash SHA-256 hex del contenido leído"),
                    ],
                    ["concept_id", "markdown", "reason"],
                ),
            },
            ToolSpec {
                name: "memory_history",
                description: "Historia de revisiones de un concepto, de más reciente a más antigua. Pagina con before_seq.",
                input_schema: schema(
                    [
                        ("concept_id", "string", "id lógico del concepto"),
                        ("limit", "integer", "máximo de revisiones (1-100)"),
                        ("before_seq", "integer", "solo revisiones anteriores a este seq"),
                    ],
                    ["concept_id"],
                ),
            },
        ]
    }

    fn call(&mut self, name: &str, arguments: &Value) -> Result<Value, ToolError> {
        match name {
            "memory_search" => self.memory_search(arguments),
            "memory_resolve" => self.memory_resolve(arguments),
            "memory_commit" => self.memory_commit(arguments),
            "memory_history" => self.memory_history(arguments),
            _ => Err(ToolError::UnknownTool),
        }
    }
}
