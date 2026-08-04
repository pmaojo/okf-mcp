use store_core::{MemoryRepository, SearchQuery};
use memory_model::{ConceptId, ContentId, Budget};
use sqlx::{PgPool, Row};
use std::collections::HashMap;

/// Reconcilia el estado de Supabase para alinear la base de datos al 100% con la verdad de GitHub.
pub async fn reconcile_github_to_supabase(
    pool: &PgPool,
    client: &reqwest::Client,
    github_store: &impl MemoryRepository,
    embedding_keys: &gemini_embeddings::EmbeddingKeys,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("Iniciando reconciliación de GitHub -> Supabase...");

    // 1. Obtener la lista de TODOS los conceptos vivos en GitHub.
    //
    // `Budget::default().max_search_results` (50) corta `search()` —
    // pensado para acotar respuestas a un cliente MCP, no para esta
    // comparación de conjuntos completos. Con ese budget, cualquier
    // concepto vivo más allá del puesto 50 (orden alfabético de
    // `GithubStore`) quedaba fuera de `github_live_concepts` y el
    // paso 4 de abajo lo marcaba como `deleted_at` en Supabase por
    // error, aunque siguiera vivo en GitHub. Sin tope aquí: el
    // reconciliador necesita ver el grafo entero para decidir qué
    // borrar.
    let budget = Budget { max_search_results: usize::MAX, ..Budget::default() };
    let query = SearchQuery::default();
    let github_hits = github_store.search(&query, &budget)?;
    
    let mut github_live_concepts = HashMap::new();
    for hit in github_hits {
        if let Some(doc) = github_store.get(&hit.concept_id)? {
            github_live_concepts.insert(hit.concept_id, doc);
        }
    }
    
    println!("Encontrados {} conceptos vivos en GitHub.", github_live_concepts.len());

    // 2. Obtener todos los conceptos registrados en Supabase (incluso los borrados lógicos)
    let db_rows = sqlx::query(
        "SELECT concept_id, content_id, version, (deleted_at IS NOT NULL) AS is_deleted FROM heads"
    )
    .fetch_all(pool)
    .await?;

    let mut db_concepts = HashMap::new();
    for row in db_rows {
        let concept_id: String = row.get("concept_id");
        let content_id: String = row.get("content_id");
        let version: i64 = row.get("version");
        let is_deleted: bool = row.get("is_deleted");
        
        let cid = ConceptId::parse(&concept_id)?;
        let content_id = ContentId::from_hex(&content_id)
            .ok_or_else(|| format!("invalid hex hash in database for concept {concept_id}"))?;
        db_concepts.insert(cid, (content_id, version, is_deleted));
    }

    // 3. Reconciliar diferencias de archivos vivos/modificados
    for (concept_id, github_doc) in &github_live_concepts {
        let concept_str = concept_id.as_str();
        let github_hash_hex = github_doc.content_id.to_hex();
        
        let mut needs_sync = false;
        let mut target_version = github_doc.version;

        if let Some((db_hash, db_version, db_deleted)) = db_concepts.get(concept_id) {
            // Si en Supabase tiene diferente hash, o está marcado como borrado
            if db_hash.to_hex() != github_hash_hex || *db_deleted {
                needs_sync = true;
                // Incrementamos la versión para registrar el cambio
                target_version = (db_version + 1).max(github_doc.version as i64) as u64;
            }
        } else {
            // No existe en Supabase
            needs_sync = true;
            target_version = github_doc.version;
        }

        if needs_sync {
            println!("Sincronizando/Actualizando concepto {} (versión={})...", concept_str, target_version);
            
            // Iniciar transacción en Postgres
            let mut tx = pool.begin().await?;
            
            // A. Insertar blob
            sqlx::query(
                "INSERT INTO blobs (content_id, raw) VALUES ($1, $2) ON CONFLICT (content_id) DO NOTHING"
            )
            .bind(&github_hash_hex)
            .bind(&*github_doc.raw)
            .execute(&mut *tx)
            .await?;

            // B. Actualizar head
            sqlx::query(
                "INSERT INTO heads (concept_id, content_id, version, doc_type, title, tags, deleted_at)
                 VALUES ($1, $2, $3, $4, $5, $6, NULL)
                 ON CONFLICT (concept_id)
                 DO UPDATE SET content_id = EXCLUDED.content_id,
                               version = EXCLUDED.version,
                               doc_type = EXCLUDED.doc_type,
                               title = EXCLUDED.title,
                               tags = EXCLUDED.tags,
                               deleted_at = NULL"
            )
            .bind(concept_str)
            .bind(&github_hash_hex)
            .bind(target_version as i64)
            .bind(&github_doc.doc_type)
            .bind(github_doc.title.as_deref())
            .bind(&github_doc.tags)
            .execute(&mut *tx)
            .await?;

            // C. Insertar revisión
            sqlx::query(
                "INSERT INTO revisions (concept_id, base, result, actor_subject, actor_client_id, reason)
                 VALUES ($1, $2, $3, 'system/reconciler', 'system/reconciler', 'Sincronización reconciliadora desde GitHub')"
            )
            .bind(concept_str)
            .bind(db_concepts.get(concept_id).map(|(h, _, _)| h.to_hex()))
            .bind(&github_hash_hex)
            .execute(&mut *tx)
            .await?;

            // D. Actualizar enlaces
            sqlx::query("DELETE FROM links WHERE source_id = $1")
                .bind(concept_str)
                .execute(&mut *tx)
                .await?;

            for link in &github_doc.links {
                sqlx::query(
                    "INSERT INTO links (source_id, target_id, rel) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING"
                )
                .bind(concept_str)
                .bind(link.target.as_str())
                .bind(link.rel.as_deref())
                .execute(&mut *tx)
                .await?;
            }

            tx.commit().await?;

            // E. Generar embeddings
            if embedding_keys.any_configured() {
                if let Err(e) = supabase_store::index_embedding(pool, client, embedding_keys, concept_str, &github_hash_hex, &github_doc.raw).await {
                    eprintln!("Error generando embeddings para {}: {}", concept_str, e);
                }
            }
        }
    }

    // 4. Identificar conceptos borrados (están vivos en Supabase, pero ya no están en GitHub)
    for (concept_id, (db_hash, _, db_deleted)) in &db_concepts {
        if !*db_deleted && !github_live_concepts.contains_key(concept_id) {
            let concept_str = concept_id.as_str();
            println!("Detectado concepto borrado en GitHub: {}. Sincronizando borrado lógico en Supabase...", concept_str);

            let mut tx = pool.begin().await?;

            // A. Marcar como borrado
            sqlx::query("UPDATE heads SET deleted_at = CURRENT_TIMESTAMP WHERE concept_id = $1")
                .bind(concept_str)
                .execute(&mut *tx)
                .await?;

            // B. Borrar enlaces
            sqlx::query("DELETE FROM links WHERE source_id = $1")
                .bind(concept_str)
                .execute(&mut *tx)
                .await?;

            // C. Borrar embedding
            sqlx::query("DELETE FROM embeddings WHERE concept_id = $1")
                .bind(concept_str)
                .execute(&mut *tx)
                .await?;

            // D. Crear revisión de borrado
            sqlx::query(
                "INSERT INTO revisions (concept_id, base, result, actor_subject, actor_client_id, reason)
                 VALUES ($1, $2, $3, 'system/reconciler', 'system/reconciler', 'Borrado detectado por sincronización desde GitHub')"
            )
            .bind(concept_str)
            .bind(db_hash.to_hex())
            .bind(db_hash.to_hex())
            .execute(&mut *tx)
            .await?;

            tx.commit().await?;
        }
    }

    println!("Reconciliación de GitHub -> Supabase finalizada con éxito.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use memory_store::InMemoryStore;
    use store_core::CommitRequest;
    use memory_model::{ConceptId, Principal, Budget};

    fn block_on<F: std::future::Future>(fut: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(fut)
    }

    #[test]
    fn test_reconciliation_flow() {
        let Ok(db_url) = std::env::var("TEST_DATABASE_URL") else {
            eprintln!("Saltando test de reconciliación: TEST_DATABASE_URL no está configurada.");
            return;
        };

        let pool = block_on(async {
            let pool = PgPool::connect(&db_url).await.expect("conectar a TEST_DATABASE_URL");
            sqlx::query(include_str!("../../supabase-store/schema.sql"))
                .execute(&pool)
                .await
                .expect("inicializar tablas en db de test");
            sqlx::query("TRUNCATE TABLE links, revisions, heads, blobs, embeddings CASCADE")
                .execute(&pool)
                .await
                .expect("vaciar tablas de test");
            pool
        });

        // 1. Simular un GithubStore (InMemoryStore) con algunos conceptos
        let mut github = InMemoryStore::new();
        let actor = Principal::local_dev();
        let budget = Budget::default();

        github.commit(
            CommitRequest {
                concept_id: ConceptId::parse("skills/reconciliation-test").unwrap(),
                expected: None,
                markdown: "---\ntype: skill\ntitle: Test Skill\ntags: [a, b]\n---\n[[projects/okf]]\n".to_string(),
                reason: "initial".to_string(),
            },
            &actor,
            &budget,
        ).unwrap();

        // 2. Ejecutar reconciliación
        let client = reqwest::Client::new();
        block_on(async {
            reconcile_github_to_supabase(&pool, &client, &github, &gemini_embeddings::EmbeddingKeys::default())
                .await
                .unwrap();
        });

        // 3. Verificar que Supabase tiene el concepto
        block_on(async {
            let row = sqlx::query("SELECT content_id, version, doc_type, title, tags FROM heads WHERE concept_id = 'skills/reconciliation-test'")
                .fetch_one(&pool)
                .await
                .unwrap();
            let version: i64 = row.get("version");
            let doc_type: String = row.get("doc_type");
            let title: Option<String> = row.get("title");
            let tags: Vec<String> = row.get("tags");
            
            assert_eq!(version, 1);
            assert_eq!(doc_type, "skill");
            assert_eq!(title, Some("Test Skill".to_string()));
            assert_eq!(tags, vec!["a".to_string(), "b".to_string()]);

            // Verificar links
            let link_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM links WHERE source_id = 'skills/reconciliation-test' AND target_id = 'projects/okf'")
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(link_count, 1);
        });

        // 4. Modificar el concepto en GitHub
        let view = github.get(&ConceptId::parse("skills/reconciliation-test").unwrap()).unwrap().unwrap();
        github.commit(
            CommitRequest {
                concept_id: ConceptId::parse("skills/reconciliation-test").unwrap(),
                expected: Some(view.content_id),
                markdown: "---\ntype: skill\ntitle: Updated Test Skill\ntags: [a, c]\n---\n".to_string(),
                reason: "update".to_string(),
            },
            &actor,
            &budget,
        ).unwrap();

        // Re-conciliar
        block_on(async {
            reconcile_github_to_supabase(&pool, &client, &github, &gemini_embeddings::EmbeddingKeys::default())
                .await
                .unwrap();
        });

        // Verificar actualización en Supabase
        block_on(async {
            let row = sqlx::query("SELECT version, title, tags FROM heads WHERE concept_id = 'skills/reconciliation-test'")
                .fetch_one(&pool)
                .await
                .unwrap();
            let version: i64 = row.get("version");
            let title: Option<String> = row.get("title");
            let tags: Vec<String> = row.get("tags");
            
            assert!(version > 1);
            assert_eq!(title, Some("Updated Test Skill".to_string()));
            assert_eq!(tags, vec!["a".to_string(), "c".to_string()]);

            // El link projects/okf debería estar borrado
            let link_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM links WHERE source_id = 'skills/reconciliation-test'")
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(link_count, 0);
        });

        // 5. Simular borrado en GitHub (el concepto desaparece de InMemoryStore)
        let view2 = github.get(&ConceptId::parse("skills/reconciliation-test").unwrap()).unwrap().unwrap();
        github.delete(
            &ConceptId::parse("skills/reconciliation-test").unwrap(),
            view2.content_id,
            &actor,
            "delete".to_string(),
        ).unwrap();

        // Re-conciliar
        block_on(async {
            reconcile_github_to_supabase(&pool, &client, &github, &gemini_embeddings::EmbeddingKeys::default())
                .await
                .unwrap();
        });

        // Verificar borrado lógico en Supabase (deleted_at IS NOT NULL)
        block_on(async {
            let is_deleted: bool = sqlx::query_scalar("SELECT deleted_at IS NOT NULL FROM heads WHERE concept_id = 'skills/reconciliation-test'")
                .fetch_one(&pool)
                .await
                .unwrap();
            assert!(is_deleted);
        });
    }

    /// Regresión: con más conceptos vivos que `Budget::default()
    /// .max_search_results` (50), la reconciliación NO debe marcar
    /// como borrados los que queden fuera de ese tope alfabético.
    #[test]
    fn test_reconciliation_no_marca_borrado_mas_alla_del_budget_de_busqueda() {
        let Ok(db_url) = std::env::var("TEST_DATABASE_URL") else {
            eprintln!("Saltando test de reconciliación: TEST_DATABASE_URL no está configurada.");
            return;
        };

        let pool = block_on(async {
            let pool = PgPool::connect(&db_url).await.expect("conectar a TEST_DATABASE_URL");
            sqlx::query(include_str!("../../supabase-store/schema.sql"))
                .execute(&pool)
                .await
                .expect("inicializar tablas en db de test");
            sqlx::query("TRUNCATE TABLE links, revisions, heads, blobs, embeddings CASCADE")
                .execute(&pool)
                .await
                .expect("vaciar tablas de test");
            pool
        });

        let mut github = InMemoryStore::new();
        let actor = Principal::local_dev();
        let budget = Budget::default();

        // 60 conceptos vivos: más que max_search_results (50).
        for i in 0..60 {
            github.commit(
                CommitRequest {
                    concept_id: ConceptId::parse(&format!("skills/concept-{i:02}")).unwrap(),
                    expected: None,
                    markdown: format!("---\ntype: skill\ntitle: Concept {i}\n---\ncontenido\n"),
                    reason: "seed".to_string(),
                },
                &actor,
                &budget,
            ).unwrap();
        }

        let client = reqwest::Client::new();
        block_on(async {
            reconcile_github_to_supabase(&pool, &client, &github, &gemini_embeddings::EmbeddingKeys::default())
                .await
                .unwrap();
        });

        block_on(async {
            let deleted_count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM heads WHERE deleted_at IS NOT NULL"
            )
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(deleted_count, 0, "ningún concepto vivo debe quedar marcado como borrado");

            let live_count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM heads WHERE deleted_at IS NULL"
            )
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(live_count, 60, "los 60 conceptos deben sincronizarse como vivos");
        });
    }
}
