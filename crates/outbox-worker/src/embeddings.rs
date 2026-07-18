//! Generación e indexación de embeddings: pide a Gemini el vector del
//! Markdown de un concepto ([`gemini_embeddings`], compartido con el
//! camino de lectura en `supabase-store`) y lo guarda en `pgvector`.
//! Aislado de `lib.rs` porque no tiene relación con la sincronización
//! a GitHub, ver [`crate::github_sync`] — comparten `process_batch`
//! solo porque el mismo evento de outbox dispara ambos.

use crate::pg_query;
use gemini_embeddings::embed;
use pgvector::Vector;
use sqlx::PgPool;

pub async fn generate_and_save_embedding(
    pool: &PgPool,
    client: &reqwest::Client,
    gemini_key: &str,
    concept_id: &str,
    markdown: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let values = embed(client, gemini_key, markdown).await?;

    // Guardar o actualizar en pgvector
    pg_query(
        "INSERT INTO embeddings (concept_id, embedding)
         VALUES ($1, $2)
         ON CONFLICT (concept_id) DO UPDATE SET embedding = EXCLUDED.embedding"
    )
    .bind(concept_id)
    .bind(Vector::from(values))
    .execute(pool)
    .await?;

    println!("Embedding para {} generado e indexado exitosamente.", concept_id);
    Ok(())
}
