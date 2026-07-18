use sqlx::PgPool;
use supabase_store::SupabaseStore;

/// El mismo puente síncrono→asíncrono que usa el adaptador: si ya hay
/// un runtime en contexto, `block_in_place` + `block_on` sobre su
/// handle. Aquí SIEMPRE hay runtime (ver el comentario del test): un
/// runtime nuevo por operación rompería el pool, porque las conexiones
/// quedarían huérfanas del driver de IO del runtime destruido y el
/// pool acabaría en `PoolTimedOut`.
fn block_on<F: std::future::Future>(fut: F) -> F::Output {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => tokio::task::block_in_place(|| handle.block_on(fut)),
        Err(_) => tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(fut),
    }
}

#[test]
fn supabase_cumple_el_contrato() {
    let Ok(db_url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("Saltando test de contrato de Supabase: TEST_DATABASE_URL no está configurada.");
        return;
    };

    // Un ÚNICO runtime multihilo vivo durante todo el test: el pool y
    // todas sus conexiones pertenecen a él, igual que en producción
    // (axum/Vercel). El contrato en sí es síncrono, así que corre
    // dentro de una tarea con `block_in_place` — que exige un worker
    // real del runtime, no el hilo de `Runtime::block_on`.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();

    rt.block_on(async move {
        let pool = PgPool::connect(&db_url).await.expect("conectar a TEST_DATABASE_URL");

        // Crear esquema si no existe. raw_sql (protocolo simple) y no
        // query (prepared statement): schema.sql trae varias sentencias
        // separadas por ';' y Postgres rechaza múltiples comandos en una
        // consulta preparada — igual que hacen vercel-entry y outbox-worker.
        sqlx::raw_sql(include_str!("../schema.sql"))
            .execute(&pool)
            .await
            .expect("inicializar tablas en db de test");

        tokio::task::spawn(async move {
            tokio::task::block_in_place(|| {
                store_core::contract::run_all(|| {
                    let pool = pool.clone();
                    block_on(async {
                        sqlx::query("TRUNCATE TABLE links, revisions, heads, blobs CASCADE")
                            .execute(&pool)
                            .await
                            .expect("vaciar tablas de test");
                    });
                    SupabaseStore::new(pool, None)
                });
            });
        })
        .await
        .expect("el contrato completo debe pasar");
    });
}
