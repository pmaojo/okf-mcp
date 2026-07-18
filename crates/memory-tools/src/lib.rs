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
#![warn(missing_docs)]

use graph_core::NeighborSource;
use json_mini::{arr, n, obj, s, Value};
use mcp_core::{ToolError, ToolHandler, ToolSpec, UiResource};
use memory_model::{Budget, ConceptId, ContentId, Principal};
use store_core::{CommitRequest, MemoryRepository, SearchQuery, StoreError, StoreMaintenance};
use std::convert::Infallible;

/// El [`ToolHandler`] de memoria, genérico sobre el repositorio.
pub struct MemoryTools<R> {
    repo: R,
    actor: Principal,
    budget: Budget,
}

impl<R> MemoryTools<R>
where
    R: MemoryRepository + StoreMaintenance + NeighborSource<Error = Infallible>,
{
    /// Envuelve un repositorio con el actor y el presupuesto que
    /// gobernarán TODAS las llamadas.
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
        let add_tags = args.get("add_tags")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|x| x.as_str().map(String::from)).collect())
            .unwrap_or_default();
        let remove_tags = args.get("remove_tags")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|x| x.as_str().map(String::from)).collect())
            .unwrap_or_default();

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
    R: MemoryRepository + StoreMaintenance + NeighborSource<Error = Infallible>,
{
    fn tools(&self) -> Vec<ToolSpec> {
        vec![
            ToolSpec {
                name: "memory_search",
                description: "Busca conceptos en la memoria de manera híbrida: primero coincidencias exactas por subcadena, luego similitud semántica. Devuelve candidatos compactos con su hash y URI; usa memory_resolve para leer el contenido.",
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
                ui_resource_uri: None,
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
                ui_resource_uri: Some("ui://okf-memory/graph-view"),
            },
            ToolSpec {
                name: "memory_commit",
                description: "Escribe un documento OKF (frontmatter YAML + Markdown) con concurrencia optimista. expected_hash es obligatorio salvo en creación.",
                input_schema: schema(
                    [
                        ("concept_id", "string", "id lógico del concepto"),
                        ("markdown", "string", "documento completo: '---' + frontmatter YAML + '---' + cuerpo Markdown."),
                        ("reason", "string", "por qué se hace este cambio"),
                        ("expected_hash", "string", "hash SHA-256 hex del contenido leído"),
                        ("dry_run", "boolean", "si es true, valida el documento y chequea conflictos sin guardar nada"),
                    ],
                    ["concept_id", "markdown", "reason"],
                ),
                ui_resource_uri: None,
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
                ui_resource_uri: Some("ui://okf-memory/history-view"),
            },
            ToolSpec {
                name: "memory_delete",
                description: "Borrado lógico de un concepto. Requiere expected_hash para concurrencia optimista. Se registra la baja en la historia.",
                input_schema: schema(
                    [
                        ("concept_id", "string", "id lógico del concepto a borrar"),
                        ("expected_hash", "string", "hash SHA-256 hex del contenido actual"),
                        ("reason", "string", "motivo de la baja"),
                    ],
                    ["concept_id", "expected_hash", "reason"],
                ),
                ui_resource_uri: None,
            },
            ToolSpec {
                name: "memory_list",
                description: "Lista conceptos (concept_id, hash, type, tags) bajo un prefijo de ruta lógica opcional sin leer su contenido completo.",
                input_schema: schema(
                    [
                        ("path_prefix", "string", "prefijo de ruta por segmentos"),
                        ("limit", "integer", "máximo de resultados"),
                    ],
                    [],
                ),
                ui_resource_uri: None,
            },
            ToolSpec {
                name: "memory_backlinks",
                description: "Devuelve todos los enlaces entrantes (backlinks) hacia un concepto, indicando qué documentos le enlazan y con qué tipo de relación.",
                input_schema: schema(
                    [
                        ("concept_id", "string", "id lógico del concepto de interés"),
                    ],
                    ["concept_id"],
                ),
                ui_resource_uri: None,
            },
            ToolSpec {
                name: "memory_embed",
                description: "Fuerza la indexación semántica inmediata de documentos cuyo embedding falta o quedó obsoleto.",
                input_schema: schema(
                    [
                        ("path_prefix", "string", "opcional, filtra por prefijo de ruta lógica"),
                        ("max", "integer", "lote máximo de documentos a procesar en esta llamada"),
                    ],
                    [],
                ),
                ui_resource_uri: None,
            },
            ToolSpec {
                name: "memory_patch",
                description: "Actualiza selectivamente campos del frontmatter sin reenviar ni modificar el cuerpo Markdown del documento.",
                input_schema: schema(
                    [
                        ("concept_id", "string", "id lógico del concepto"),
                        ("expected_hash", "string", "hash SHA-256 hex del contenido actual"),
                        ("reason", "string", "motivo del cambio"),
                        ("set", "object", "mapa de campos clave-valor a escribir/reemplazar"),
                        ("remove", "array", "lista de claves de frontmatter a eliminar (ej. ['tags'])"),
                        ("add_tags", "array", "tags a añadir a la lista existente"),
                        ("remove_tags", "array", "tags a eliminar de la lista existente"),
                        ("dry_run", "boolean", "si es true, simula el patch sin persistir"),
                    ],
                    ["concept_id", "expected_hash", "reason"],
                ),
                ui_resource_uri: None,
            },
            ToolSpec {
                name: "memory_bulk_commit",
                description: "Aplica un conjunto de commits en lote. En modo atómico se aplica todo o nada.",
                input_schema: schema(
                    [
                        ("requests", "array", "lista de peticiones de commit (con concept_id, markdown, reason, y expected_hash opcional)"),
                        ("atomic", "boolean", "si es true, revierte todo el lote ante cualquier fallo (rollback)"),
                    ],
                    ["requests"],
                ),
                ui_resource_uri: None,
            },
            ToolSpec {
                name: "memory_validate",
                description: "Valida la integridad del grafo o un subárbol, devolviendo enlaces rotos, referencias a borrados y embeddings ausentes.",
                input_schema: schema(
                    [
                        ("path_prefix", "string", "opcional, valida solo bajo este prefijo de ruta"),
                    ],
                    [],
                ),
                ui_resource_uri: None,
            },
            ToolSpec {
                name: "memory_status",
                description: "Consulta rápida del estado de salud operativo del sistema (enlaces rotos, cola outbox, embeddings pendientes).",
                input_schema: schema([], []),
                ui_resource_uri: None,
            },
            ToolSpec {
                name: "memory_stats",
                description: "Devuelve métricas detalladas del grafo (recuentos de tipos/tags, hubs de enlaces, documentos huérfanos).",
                input_schema: schema([], []),
                ui_resource_uri: None,
            },
        ]
    }

    fn ui_resources(&self) -> Vec<UiResource> {
        vec![
            UiResource {
                uri: "ui://okf-memory/graph-view",
                name: "Vista de grafo",
                description: "Vecindario de un concepto como grafo interactivo (nodos por profundidad, aristas reales padre→hijo).",
                html: include_str!("../assets/graph-view.html"),
            },
            UiResource {
                uri: "ui://okf-memory/history-view",
                name: "Línea de tiempo",
                description: "Historial de revisiones de un concepto como línea de tiempo vertical.",
                html: include_str!("../assets/history-view.html"),
            },
        ]
    }

    fn call(&mut self, name: &str, arguments: &Value) -> Result<Value, ToolError> {
        match name {
            "memory_search" => self.memory_search(arguments),
            "memory_resolve" => self.memory_resolve(arguments),
            "memory_commit" => self.memory_commit(arguments),
            "memory_history" => self.memory_history(arguments),
            "memory_delete" => self.memory_delete(arguments),
            "memory_list" => self.memory_list(arguments),
            "memory_backlinks" => self.memory_backlinks(arguments),
            "memory_embed" => self.memory_embed(arguments),
            "memory_patch" => self.memory_patch(arguments),
            "memory_bulk_commit" => self.memory_bulk_commit(arguments),
            "memory_validate" => self.memory_validate(arguments),
            "memory_status" => self.memory_status(arguments),
            "memory_stats" => self.memory_stats(arguments),
            _ => Err(ToolError::UnknownTool),
        }
    }
}
