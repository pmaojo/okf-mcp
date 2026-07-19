use crate::{SupabaseStore, pg_query_scalar};
use graph_core::NeighborSource;
use memory_model::ConceptId;
use std::convert::Infallible;

impl NeighborSource for SupabaseStore {
    type Error = Infallible;

    fn neighbors(&self, id: &ConceptId) -> Result<Vec<ConceptId>, Self::Error> {
        let res = crate::block_on! {
            pg_query_scalar::<String>(
                "SELECT target_id FROM links WHERE source_id = $1 ORDER BY target_id",
            )
            .bind(id.as_str())
            .fetch_all(&self.pool)
            .await
        };

        match res {
            Ok(rows) => {
                let parsed = rows
                    .into_iter()
                    .filter_map(|r| ConceptId::parse(&r).ok())
                    .collect();
                Ok(parsed)
            }
            Err(e) => {
                eprintln!("Error cargando vecinos del concepto {id} desde la base de datos: {e}");
                Ok(Vec::new())
            }
        }
    }

    fn document_size(&self, id: &ConceptId) -> Result<Option<usize>, Self::Error> {
        let res = crate::block_on! {
            pg_query_scalar::<String>(
                "SELECT b.raw
                 FROM heads h
                 JOIN blobs b ON h.content_id = b.content_id
                 WHERE h.concept_id = $1",
            )
            .bind(id.as_str())
            .fetch_optional(&self.pool)
            .await
        };

        match res {
            Ok(Some(raw)) => Ok(Some(raw.len())),
            Ok(None) => Ok(None),
            Err(e) => {
                eprintln!("Error cargando tamaño del documento {id} desde la base de datos: {e}");
                Ok(None)
            }
        }
    }
}
