//! Pool de conexiones compartido por los binarios serverless de este
//! crate (`api/mcp.rs`, `api/outbox.rs`): mismo workaround del pooler
//! de Supabase y mismo bootstrap de esquema en ambos cold starts, sin
//! duplicar la lógica entre los dos. Cada binario sigue siendo un
//! proceso serverless independiente, así que cada uno obtiene su
//! propia instancia de `DB_POOL` — extraer la función no cambia ese
//! comportamiento, solo evita mantener dos copias del mismo código.

use sqlx::postgres::PgConnectOptions;
use std::str::FromStr;
use std::sync::OnceLock;

static DB_POOL: OnceLock<sqlx::PgPool> = OnceLock::new();

/// Obtiene o inicializa el pool de conexiones de base de datos de manera thread-safe.
///
/// En el primer cold start de cada instancia serverless, aplica
/// `schema.sql` contra `POSTGRES_URL` (mismo patrón que
/// `outbox-worker`, ver `crates/outbox-worker/src/main.rs`). Es
/// idempotente (`CREATE TABLE IF NOT EXISTS`), así que repetirlo en
/// cada deploy/instancia nueva es seguro.
pub async fn get_db_pool(db_url: &str) -> sqlx::PgPool {
    if let Some(pool) = DB_POOL.get() {
        return pool.clone();
    }
    // `POSTGRES_URL` apunta al pooler de Supabase en modo transacción
    // (puerto 6543, necesario en serverless con autoescala — ver
    // capítulo 12 del tutorial). Ese pooler NO garantiza que la misma
    // conexión física atienda siempre a la misma sesión lógica de
    // sqlx: puede entregar la conexión a otra invocación entre
    // transacciones, y un prepared statement CON nombre (`sqlx_s_N`)
    // que sobrevive en el backend colisiona con el homónimo de otra
    // sesión ("prepared statement \"sqlx_s_N\" already exists").
    //
    // La protección contra eso son DOS piezas, y esta es la menor:
    //
    // 1. `persistent(false)` en CADA consulta de `supabase-store` y
    //    `outbox-worker` (ver `pg_query` en ambos crates): sqlx usa el
    //    statement SIN nombre del protocolo, que no puede colisionar.
    //    Es la protección real — verificado en el código de sqlx 0.8
    //    (y 0.9): `statement_cache_capacity(0)` NO evita que un
    //    `sqlx::query` normal reciba nombre, solo evita cachearlo.
    // 2. `statement_cache_capacity(0)` aquí: cinturón extra para que
    //    ninguna consulta futura que olvide `persistent(false)` deje
    //    además statements cacheados vivos entre invocaciones.
    let connect_options = PgConnectOptions::from_str(db_url)
        .expect("Invalid POSTGRES_URL")
        .statement_cache_capacity(0);
    let pool = sqlx::PgPool::connect_with(connect_options)
        .await
        .expect("Failed to connect to Supabase PostgreSQL database");
    // `schema.sql` trae varias sentencias separadas por `;`. `sqlx::query`
    // usa el protocolo extendido (prepared statement), que Postgres
    // rechaza si el texto trae más de un comando ("cannot insert
    // multiple commands into a prepared statement"). `raw_sql` usa el
    // protocolo simple, que sí soporta scripts multi-sentencia.
    sqlx::raw_sql(include_str!("../../supabase-store/schema.sql"))
        .execute(&pool)
        .await
        .expect("Failed to apply database schema");
    let _ = DB_POOL.set(pool.clone());
    pool
}
