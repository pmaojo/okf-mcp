use sqlx::PgPool;
use supabase_store::SupabaseStore;

fn block_on<F: std::future::Future>(fut: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(fut)
}

#[test]
fn supabase_cumple_el_contrato() {
    let Ok(db_url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("Saltando test de contrato de Supabase: TEST_DATABASE_URL no está configurada.");
        return;
    };

    let pool = block_on(async {
        let pool = PgPool::connect(&db_url).await.expect("conectar a TEST_DATABASE_URL");
        
        // Crear esquema si no existe
        let schema = include_str!("../schema.sql");
        sqlx::query(schema).execute(&pool).await.expect("inicializar tablas en db de test");
        
        pool
    });

    memory_store::contract::run_all(|| {
        let pool = pool.clone();
        block_on(async {
            sqlx::query("TRUNCATE TABLE links, revisions, heads, blobs CASCADE")
                .execute(&pool)
                .await
                .expect("vaciar tablas de test");
        });
        SupabaseStore::new(pool)
    });
}
