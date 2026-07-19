use crate::{SupabaseStore, pg_query, pg_query_scalar};
use store_core::{
    StoreMaintenance, LinkHealth, ValidationReport, GraphStats, StoreStatus, EmbedOutcome,
    StoreError,
};
use memory_model::{ConceptId, Budget};
use okf_core::Link;
use sqlx::Row;

impl StoreMaintenance for SupabaseStore {
    fn link_health(&self, id: &ConceptId) -> Result<LinkHealth, StoreError> {
        let exists = crate::block_on! {
            pg_query_scalar::<bool>(
                "SELECT EXISTS(SELECT 1 FROM heads WHERE concept_id = $1 AND deleted_at IS NULL)",
            )
            .bind(id.as_str())
            .fetch_one(&self.pool)
            .await
            .map_err(|e| StoreError::Backend(e.to_string()))
        }?;

        if !exists {
            return Err(StoreError::NotFound(id.clone()));
        }

        let rows = crate::block_on! {
            pg_query(
                "SELECT l.target_id, l.rel, (h.concept_id IS NOT NULL) as exists, (h.deleted_at IS NOT NULL) as is_deleted
                 FROM links l
                 LEFT JOIN heads h ON l.target_id = h.concept_id
                 WHERE l.source_id = $1
                 ORDER BY l.target_id",
            )
            .bind(id.as_str())
            .fetch_all(&self.pool)
            .await
            .map_err(|e| StoreError::Backend(e.to_string()))
        }?;

        let mut health = LinkHealth::default();
        for row in rows {
            let target_str: String = row.get("target_id");
            let target = ConceptId::parse(&target_str).map_err(|e| StoreError::Backend(e.to_string()))?;
            let rel: Option<String> = row.get("rel");
            let link = Link { target, rel };
            
            let exists: bool = row.get("exists");
            let is_deleted: bool = row.get("is_deleted");
            if !exists {
                health.broken.push(link);
            } else if is_deleted {
                health.deleted.push(link);
            } else {
                health.ok.push(link);
            }
        }
        Ok(health)
    }

    fn validate(
        &self,
        path_prefix: Option<&str>,
        budget: &Budget,
    ) -> Result<ValidationReport, StoreError> {
        let cap = budget.max_search_results;

        let rows = crate::block_on! {
            pg_query(
                "SELECT l.source_id, l.target_id, l.rel, (h.concept_id IS NOT NULL) as exists, (h.deleted_at IS NOT NULL) as is_deleted
                 FROM links l
                 JOIN heads sh ON l.source_id = sh.concept_id
                 LEFT JOIN heads h ON l.target_id = h.concept_id
                 WHERE sh.deleted_at IS NULL
                   AND ($1::text IS NULL OR l.source_id = $1 OR l.source_id LIKE $1 || '/%')
                 ORDER BY l.source_id, l.target_id",
            )
            .bind(path_prefix)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| StoreError::Backend(e.to_string()))
        }?;

        let mut report = ValidationReport::default();
        for row in rows {
            let source_str: String = row.get("source_id");
            let target_str: String = row.get("target_id");
            let source = ConceptId::parse(&source_str).map_err(|e| StoreError::Backend(e.to_string()))?;
            let target = ConceptId::parse(&target_str).map_err(|e| StoreError::Backend(e.to_string()))?;

            let exists: bool = row.get("exists");
            let is_deleted: bool = row.get("is_deleted");

            if !exists {
                report.broken_links_total += 1;
                if report.broken_links.len() < cap {
                    report.broken_links.push((source.clone(), target.clone()));
                }
            } else if is_deleted {
                report.deleted_referenced_total += 1;
                if report.deleted_referenced.len() < cap {
                    report.deleted_referenced.push((source.clone(), target.clone()));
                }
            }
        }

        if self.gemini_api_key.is_some() {
            let emb_rows = crate::block_on! {
                pg_query(
                    "SELECT h.concept_id
                     FROM heads h
                     LEFT JOIN embeddings e ON h.concept_id = e.concept_id
                     WHERE h.deleted_at IS NULL
                       AND ($1::text IS NULL OR h.concept_id = $1 OR h.concept_id LIKE $1 || '/%')
                       AND (e.concept_id IS NULL OR e.content_id IS NULL OR e.content_id <> h.content_id)
                     ORDER BY h.concept_id",
                )
                .bind(path_prefix)
                .fetch_all(&self.pool)
                .await
                .map_err(|e| StoreError::Backend(e.to_string()))
            }?;

            report.missing_embeddings_total = emb_rows.len();
            for row in emb_rows {
                if report.missing_embeddings.len() >= cap {
                    break;
                }
                let concept_str: String = row.get("concept_id");
                let concept = ConceptId::parse(&concept_str).map_err(|e| StoreError::Backend(e.to_string()))?;
                report.missing_embeddings.push(concept);
            }
        }

        Ok(report)
    }

    fn stats(&self, budget: &Budget) -> Result<GraphStats, StoreError> {
        let cap = budget.max_search_results;

        let (docs, del_docs) = crate::block_on! {
            let docs = pg_query_scalar::<i64>("SELECT COUNT(*) FROM heads WHERE deleted_at IS NULL")
                .fetch_one(&self.pool)
                .await;
            let del_docs = pg_query_scalar::<i64>("SELECT COUNT(*) FROM heads WHERE deleted_at IS NOT NULL")
                .fetch_one(&self.pool)
                .await;
            let docs = docs.map_err(|e| StoreError::Backend(e.to_string()))?;
            let del_docs = del_docs.map_err(|e| StoreError::Backend(e.to_string()))?;
            Ok::<_, StoreError>((docs, del_docs))
        }?;

        let type_rows = crate::block_on! {
            pg_query(
                "SELECT doc_type, COUNT(*) as cnt
                 FROM heads
                 WHERE deleted_at IS NULL
                 GROUP BY doc_type
                 ORDER BY cnt DESC, doc_type
                 LIMIT $1",
            )
            .bind(cap as i64)
            .fetch_all(&self.pool)
            .await
        }.map_err(|e| StoreError::Backend(e.to_string()))?;

        let tag_rows = crate::block_on! {
            pg_query(
                "SELECT unnest(tags) as tag, COUNT(*) as cnt
                 FROM heads
                 WHERE deleted_at IS NULL
                 GROUP BY tag
                 ORDER BY cnt DESC, tag
                 LIMIT $1",
            )
            .bind(cap as i64)
            .fetch_all(&self.pool)
            .await
        }.map_err(|e| StoreError::Backend(e.to_string()))?;

        let top_linked_rows = crate::block_on! {
            pg_query(
                "SELECT target_id, COUNT(*) as cnt
                 FROM links l
                 JOIN heads h ON l.source_id = h.concept_id
                 WHERE h.deleted_at IS NULL
                 GROUP BY target_id
                 ORDER BY cnt DESC, target_id
                 LIMIT $1",
            )
            .bind(cap as i64)
            .fetch_all(&self.pool)
            .await
        }.map_err(|e| StoreError::Backend(e.to_string()))?;

        let orphan_rows = crate::block_on! {
            pg_query(
                "SELECT concept_id FROM heads h
                 WHERE deleted_at IS NULL
                   AND NOT EXISTS (SELECT 1 FROM links WHERE source_id = h.concept_id)
                   AND NOT EXISTS (SELECT 1 FROM links WHERE target_id = h.concept_id)
                 ORDER BY concept_id
                 LIMIT $1",
            )
            .bind(cap as i64)
            .fetch_all(&self.pool)
            .await
        }.map_err(|e| StoreError::Backend(e.to_string()))?;

        let by_type = type_rows.into_iter().map(|r| (r.get("doc_type"), r.get::<i64, _>("cnt") as usize)).collect();
        let by_tag = tag_rows.into_iter().map(|r| (r.get("tag"), r.get::<i64, _>("cnt") as usize)).collect();
        let top_linked = top_linked_rows.into_iter().filter_map(|r| {
            let target_str: String = r.get("target_id");
            let target = ConceptId::parse(&target_str).ok()?;
            Some((target, r.get::<i64, _>("cnt") as usize))
        }).collect();
        let orphans = orphan_rows.into_iter().filter_map(|r| {
            let concept_str: String = r.get("concept_id");
            ConceptId::parse(&concept_str).ok()
        }).collect();

        Ok(GraphStats {
            documents: docs as usize,
            deleted_documents: del_docs as usize,
            by_type,
            by_tag,
            top_linked,
            orphans,
        })
    }

    fn status(&self) -> Result<StoreStatus, StoreError> {
        let row = crate::block_on! {
            pg_query(
                "SELECT
                   (SELECT COUNT(*) FROM heads WHERE deleted_at IS NULL) as docs,
                   (SELECT COUNT(*) FROM heads WHERE deleted_at IS NOT NULL) as deleted_docs,
                   (SELECT COUNT(*) FROM heads h LEFT JOIN embeddings e ON h.concept_id = e.concept_id WHERE h.deleted_at IS NULL AND (e.concept_id IS NULL OR e.content_id IS NULL OR e.content_id <> h.content_id)) as missing_embs,
                   (SELECT COUNT(*) FROM links l JOIN heads sh ON l.source_id = sh.concept_id LEFT JOIN heads h ON l.target_id = h.concept_id WHERE sh.deleted_at IS NULL AND h.concept_id IS NULL) as broken,
                   (SELECT COUNT(*) FROM links l JOIN heads sh ON l.source_id = sh.concept_id JOIN heads h ON l.target_id = h.concept_id WHERE sh.deleted_at IS NULL AND h.deleted_at IS NOT NULL) as del_ref,
                   (SELECT COUNT(*) FROM outbox WHERE status = 'pending') as pending_outbox,
                   (SELECT COUNT(*) FROM outbox WHERE status = 'failed') as failed_outbox"
            )
            .fetch_one(&self.pool)
            .await
        }.map_err(|e| StoreError::Backend(e.to_string()))?;

        Ok(StoreStatus {
            documents: row.get::<i64, _>("docs") as usize,
            deleted_documents: row.get::<i64, _>("deleted_docs") as usize,
            missing_embeddings: row.get::<i64, _>("missing_embs") as usize,
            broken_links: row.get::<i64, _>("broken") as usize,
            deleted_referenced: row.get::<i64, _>("del_ref") as usize,
            outbox_pending: row.get::<i64, _>("pending_outbox") as usize,
            outbox_failed: row.get::<i64, _>("failed_outbox") as usize,
        })
    }

    fn embed_pending(
        &mut self,
        path_prefix: Option<&str>,
        max: usize,
    ) -> Result<EmbedOutcome, StoreError> {
        let gemini_key = match &self.gemini_api_key {
            Some(k) => k.clone(),
            None => return Ok(EmbedOutcome::default()),
        };

        // Clonamos el pool ANTES DE CADA bloque async; el macro block_on!
        // captura por movimiento en async move, consumiendo la variable.
        let pool_1 = self.pool.clone();
        let rows = crate::block_on! {
            pg_query(
                "SELECT h.concept_id, b.raw, h.content_id
                 FROM heads h
                 JOIN blobs b ON h.content_id = b.content_id
                 LEFT JOIN embeddings e ON h.concept_id = e.concept_id
                 WHERE h.deleted_at IS NULL
                   AND ($1::text IS NULL OR h.concept_id = $1 OR h.concept_id LIKE $1 || '/%')
                   AND (e.concept_id IS NULL OR e.content_id IS NULL OR e.content_id <> h.content_id)
                 ORDER BY h.concept_id
                 LIMIT $2",
            )
            .bind(path_prefix)
            .bind(max as i64)
            .fetch_all(&pool_1)
            .await
        }.map_err(|e| StoreError::Backend(e.to_string()))?;
        // pool_1 se mueve aquí dentro; ya no está disponible.

        let mut outcome = EmbedOutcome::default();
        let client = reqwest::Client::new();

        let pool_2 = self.pool.clone();
        for row in rows {
            let concept_str: String = row.get("concept_id");
            let concept = ConceptId::parse(&concept_str).map_err(|e| StoreError::Backend(e.to_string()))?;
            let raw: String = row.get("raw");
            let content_hex: String = row.get("content_id");

            // Clonamos pool, client y gemini_key en cada ciclo para evitar
            // que el async move los consuma en la primera iteración
            let p = pool_2.clone();
            let c = client.clone();
            let k = gemini_key.clone();
            let res = crate::block_on! {
                crate::index_embedding(&p, &c, &k, &concept_str, &content_hex, &raw).await
            };
            match res {
                Ok(_) => outcome.embedded.push(concept),
                Err(e) => outcome.failed.push((concept, e.to_string())),
            }
        }
        // pool_2 se mueve aquí dentro (primera iteración); ya no está disponible.

        let pool_3 = self.pool.clone();
        let remaining = crate::block_on! {
            pg_query_scalar::<i64>(
                "SELECT COUNT(*)
                 FROM heads h
                 LEFT JOIN embeddings e ON h.concept_id = e.concept_id
                 WHERE h.deleted_at IS NULL
                   AND ($1::text IS NULL OR h.concept_id = $1 OR h.concept_id LIKE $1 || '/%')
                   AND (e.concept_id IS NULL OR e.content_id IS NULL OR e.content_id <> h.content_id)",
            )
            .bind(path_prefix)
            .fetch_one(&pool_3)
            .await
        }.map_err(|e| StoreError::Backend(e.to_string()))?;
        // pool_3 se mueve aquí dentro.

        outcome.remaining = remaining as usize;
        Ok(outcome)
    }
}
