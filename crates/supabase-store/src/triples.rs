//! [`TripleStore`] sobre Postgres: reemplaza (DELETE + INSERT en una
//! transacción, igual que `links` en `repository.rs`) los triples de
//! un sujeto, y los relee ordenados de forma determinista.

use crate::pg_query;
use memory_model::ConceptId;
use ontology_core::{Object, Triple};
use sqlx::Row;
use store_core::{StoreError, TripleStore};

fn object_columns(object: &Object) -> (&'static str, &str) {
    match object {
        Object::Concept(id) => ("concept", id.as_str()),
        Object::Literal(text) => ("literal", text.as_str()),
    }
}

impl TripleStore for crate::SupabaseStore {
    fn save_triples(&mut self, subject: &ConceptId, triples: &[Triple]) -> Result<(), StoreError> {
        crate::block_on! {
            let mut tx = self.pool.begin().await.map_err(|e| StoreError::Backend(e.to_string()))?;

            pg_query("DELETE FROM triples WHERE subject_id = $1")
                .bind(subject.as_str())
                .execute(&mut *tx)
                .await
                .map_err(|e| StoreError::Backend(e.to_string()))?;

            for triple in triples {
                let (kind, value) = object_columns(&triple.object);
                pg_query(
                    "INSERT INTO triples (subject_id, predicate, object_kind, object_value)
                     VALUES ($1, $2, $3, $4)
                     ON CONFLICT DO NOTHING",
                )
                .bind(subject.as_str())
                .bind(&triple.predicate)
                .bind(kind)
                .bind(value)
                .execute(&mut *tx)
                .await
                .map_err(|e| StoreError::Backend(e.to_string()))?;
            }

            tx.commit().await.map_err(|e| StoreError::Backend(e.to_string()))?;
            Ok(())
        }
    }

    fn load_triples(&self, subject: &ConceptId) -> Result<Vec<Triple>, StoreError> {
        crate::block_on! {
            let rows = pg_query(
                "SELECT predicate, object_kind, object_value FROM triples
                 WHERE subject_id = $1
                 ORDER BY predicate, object_kind, object_value",
            )
            .bind(subject.as_str())
            .fetch_all(&self.pool)
            .await
            .map_err(|e| StoreError::Backend(e.to_string()))?;

            let mut out = Vec::with_capacity(rows.len());
            for r in rows {
                let predicate: String = r.get("predicate");
                let object_kind: String = r.get("object_kind");
                let object_value: String = r.get("object_value");
                let object = match object_kind.as_str() {
                    "concept" => {
                        let id = ConceptId::parse(&object_value)
                            .map_err(|e| StoreError::Backend(e.to_string()))?;
                        Object::Concept(id)
                    }
                    _ => Object::Literal(object_value),
                };
                out.push(Triple { subject: subject.clone(), predicate, object });
            }
            Ok(out)
        }
    }
}
