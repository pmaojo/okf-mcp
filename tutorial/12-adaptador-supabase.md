# Capítulo 12 — El adaptador de Supabase: Persistencia relacional de Liskov

Crate: [`crates/supabase-store`](../crates/supabase-store)

## 1. El problema

En el capítulo 11 vimos cómo desplegar el servidor MCP en Vercel, pero
reconocimos un límite importante: el almacén de datos era temporal
(`InMemoryStore`). En un entorno serverless de funciones efímeras, las
instancias nacen y mueren continuamente. Si un agente escribe en memoria en
una petición y la siguiente petición cae en una instancia fría, todo lo
recordado desaparece.

Necesitamos una base de datos real (PostgreSQL proporcionado por Supabase)
para guardar permanentemente los blobs de contenido, las revisiones del
historial y las cabezas de los documentos. Pero la red y el mundo relacional
traen nuevos desafíos:
* **Condiciones de carrera:** Dos agentes concurrentes que leen el mismo concepto no deben pisarse al hacer commit (CAS seguro).
* **Bridging síncrono/asíncrono:** El trait `MemoryRepository` es síncrono, pero los clientes de base de datos como `sqlx` son inherentemente asíncronos.
* **Sustituibilidad de Liskov:** Quien consuma el servidor MCP no debe notar si por debajo los datos están en RAM o en PostgreSQL.


Aquí el conflicto lo provoca la concurrencia real: dos agentes leen la misma
cabeza, ambos preparan un commit válido desde su punto de vista y PostgreSQL
recibe las escrituras casi a la vez. Si el adaptador no preserva el CAS del
núcleo, una persona pierde trabajo sin saberlo. Liskov y hexagonalidad
importan porque Supabase debe sustituir a RAM sin rebajar ni una garantía
del puerto.

## 2. El invariante

> **El adaptador de Supabase debe cumplir exactamente las mismas garantías y fallar ante los mismos escenarios de error que el almacén de memoria, certificado por el mismísimo contrato ejecutable de Liskov.**

Y una segunda promesa de concurrencia:

> **Ningún commit sobre un concepto puede prosperar si su hash base difiere de la cabeza actual de la base de datos al momento exacto de escribir — garantizado mediante un bloqueo de fila transaccional.**

## 3. La implementación mínima

El esquema relacional ([schema.sql](../crates/supabase-store/schema.sql))
refleja directamente el modelo de datos purificado de la memoria del hito 1,
con el añadido de campos derivados de indexación en la tabla `heads`:

```sql
CREATE TABLE blobs (
    content_id VARCHAR(64) PRIMARY KEY,
    raw TEXT NOT NULL
);

CREATE TABLE heads (
    concept_id VARCHAR(255) PRIMARY KEY,
    content_id VARCHAR(64) NOT NULL REFERENCES blobs(content_id),
    version BIGINT NOT NULL,
    doc_type VARCHAR(100) NOT NULL,
    title VARCHAR(255),
    status VARCHAR(100),
    tags TEXT[] NOT NULL
);
```

Las columnas `doc_type`, `title`, `status` y `tags` son metadatos
DERIVADOS del frontmatter (los bytes de `blobs.raw` siguen siendo la
verdad); existen para que los filtros estructurados de `memory_search`
— `type`, `status`, `path_prefix`, `tags` con modo `any`/`all` — sean
cláusulas `WHERE` literales que se aplican tanto a la búsqueda por
palabra clave (`ILIKE`) como al ranking semántico (`pgvector`). Si
ningún documento cumple los filtros, la respuesta es vacía: la
similitud semántica ordena candidatos DENTRO del subconjunto filtrado,
nunca lo amplía.

Para implementar la concurrencia optimista (Compare-and-Swap) de forma
segura contra una base de datos multi-cliente, `SupabaseStore::commit`
utiliza una transacción que adquiere un bloqueo exclusivo sobre la fila de
la cabeza mediante `FOR UPDATE`:

```rust
let mut tx = self.pool.begin().await?;

let current_head = sqlx::query(
    "SELECT content_id, version FROM heads WHERE concept_id = $1 FOR UPDATE"
)
.bind(request.concept_id.as_str())
.fetch_optional(&mut *tx)
.await?;
```

Si el `content_id` cargado no coincide con el `expected_hash` provisto en el
`CommitRequest`, la transacción hace `rollback()` y se aborta inmediatamente
con un `StoreError::Conflict`, salvaguardando la integridad del historial.

Para resolver la desconexión entre el trait síncrono `MemoryRepository` y el
driver asíncrono `sqlx`, introducimos una función puente `block_on` que
detecta de manera segura si ya nos encontramos en un hilo de ejecución de
Tokio y delega el bloqueo correspondientemente:

```rust
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
```

## 3.5. Conceptos de Rust en este capítulo

Este capítulo aborda la convivencia entre el código síncrono clásico y el
asíncrono que requiere la red:

* **El puente entre Síncrono y Asíncrono (`block_on`):** El trait `MemoryRepository` define métodos síncronos (sin `async`), pero las llamadas a bases de datos con `sqlx` son obligatoriamente asíncronas. Para poder llamarlas, usamos la función `block_on`. Si ya estamos dentro del entorno de ejecución de Tokio (ej. procesando una petición de `axum`), usamos `block_in_place` para indicarle a Tokio que este hilo se va a bloquear temporalmente y que debe mover otras tareas a hilos libres. Si no, creamos un runtime rápido en el hilo actual para ejecutar el futuro.
* **Consultas e inyección con SQLx (`bind`):** La sintaxis `sqlx::query("SELECT ... WHERE concept_id = $1").bind(...)` es la forma segura de interactuar con la base de datos. El método `bind()` asocia los parámetros de forma tipada, lo que impide por completo los ataques de inyección SQL (SQL Injection), uno de los fallos de seguridad más comunes de la web.
* **El modismo de re-préstamo mutable (`&mut *tx`):** En la llamada `.fetch_optional(&mut *tx)`, el operador `*` desreferencia la transacción `tx` (que es un wrapper mutable) y luego `&mut` vuelve a tomar un préstamo mutable del valor interno de la transacción. Este es un patrón muy común en Rust para poder pasar el control de la transacción a una consulta sin perder la propiedad de la variable `tx` para los siguientes pasos del commit.

## 4. Una versión deliberadamente rota

Imagina implementar el CAS en la aplicación en lugar de la base de datos:

```rust
// ❌ NO HACER: CAS sin transacciones ni bloqueos en la base de datos
let actual_content_id = self.db_get_content_id(concept_id);
if actual_content_id != expected_content_id {
    return Err(StoreError::Conflict);
}
// Un hilo concurrente escribe aquí...
self.db_write_head(concept_id, incoming_hex); // Pisotón concurrente
```


La versión rota nace de una traducción aparentemente fiel: tomar el
`HashMap` mental del capítulo 7 y convertirlo en `SELECT` seguido de
`UPDATE`. En una terminal con un solo cliente parece correcto, porque nunca
hay intercalado entre lectura y escritura.

## 5. Por qué falla

La base de datos relacional es compartida por múltiples instancias de
funciones de Vercel independientes y concurrentes. Si el chequeo de "lo que
hay en la base de datos" y la subsiguiente "escritura" ocurren como
sentencias SQL separadas y desprotegidas, se abre una ventana temporal
(condición de carrera). Otro hilo u otra función serverless puede intercalar
su propia transacción en medio, y nuestra posterior actualización de cabeza
pisoteará su commit exitoso sin enterarse. `FOR UPDATE` en PostgreSQL es
quien cierra esa ventana bloqueando el registro del concepto hasta que
nuestra transacción finaliza.

## 6. Memoria y asignación

En lugar de mantener un mapa de hashes en memoria RAM, `SupabaseStore` aloja
únicamente un pool de conexiones `PgPool`. El coste de asignación se
traslada del almacenamiento en memoria al tráfico de red y las conexiones
del servidor PostgreSQL.
Para mitigar la latencia y evitar el agotamiento de sockets en entornos
efímeros (serverless), el pool de conexiones se define en un `OnceLock` en
`shared_server()`. En un despliegue serverless de producción, se debe
conectar a través de la URL de transacción de Supabase Connection Pooler
(`Supavisor`) en el puerto 6543, lo que minimiza el impacto de inicializar
múltiples pools efímeros.

## 7. Tests

La prueba de fuego de que `SupabaseStore` es un sustituto exacto de
`InMemoryStore` reside en su arnés de integración
([contract.rs](../crates/supabase-store/tests/contract.rs)). A diferencia de
los tests unitarios duplicados, el arnés ejecuta exactamente la misma suite
parametrizada que ya pasa la memoria RAM:

```rust
#[test]
fn supabase_cumple_el_contrato() {
    let Ok(db_url) = std::env::var("TEST_DATABASE_URL") else { return; };
    // Un ÚNICO runtime multihilo vivo durante todo el test...
    rt.block_on(async move {
        let pool = PgPool::connect(&db_url).await.expect("conectar");
        sqlx::raw_sql(include_str!("../schema.sql")).execute(&pool).await.expect("esquema");
        tokio::task::spawn(async move {
            tokio::task::block_in_place(|| {
                store_core::contract::run_all(|| {
                    let pool = pool.clone();
                    block_on(async { /* TRUNCATE links, revisions, heads, blobs */ });
                    SupabaseStore::new(pool, None)
                });
            });
        }).await.expect("el contrato completo debe pasar");
    });
}
```

Cada iteración de la suite realiza un `TRUNCATE` de la base de datos de
pruebas para asegurar un estado limpio (aislamiento completo). Dos
detalles del arnés que costaron un rojo cada uno: el esquema se aplica
con `raw_sql` y no con `query` (varias sentencias separadas por `;` no
caben en un prepared statement, ver §8), y TODO el test comparte un
único runtime multihilo — crear un runtime nuevo por operación deja las
conexiones del pool huérfanas del driver de IO del runtime destruido y
el test muere en `PoolTimedOut`.


En TDD, el primer rojo debe venir del contrato: `base_obsoleta_no_pisa`
contra una base de pruebas. Para concurrencia, el siguiente test lanza dos
commits con la misma base y espera un ganador y un `Conflict`; si vemos dos
revisiones aceptadas, la transacción no protege la frontera.

## 8. Frontera de producción

> 🧰 **La rueda de serie:** ya estamos al otro lado de la frontera: [`sqlx`](https://docs.rs/sqlx) es la rueda de serie. Alternativas con más abstracción: [`sea-orm`](https://docs.rs/sea-orm), [`diesel`](https://docs.rs/diesel). El mapa completo y el criterio para elegir: [La rueda de serie](la-rueda-de-serie.md).

Para conectar Supabase con Vercel:
1. Añade la variable de entorno `POSTGRES_URL` en tu proyecto de Vercel.
2. Asegúrate de que apunta al pooler de Supabase en modo de transacción
(puerto 6543) en entornos con autoescala.
3. El adaptador serverless traducirá las llamadas HTTP entrantes al
repositorio Supabase de forma completamente transparente.

El pooler en modo transacción tiene una consecuencia que no se ve hasta
producción: entre transacción y transacción puede entregar la misma
conexión física de Postgres a otra sesión lógica. Un prepared statement
CON nombre (`sqlx` los llama `sqlx_s_N`) sobrevive en esa conexión, así
que dos sesiones pueden chocar sobre el mismo nombre: `prepared
statement "sqlx_s_N" already exists`. Por eso todas las consultas de
`supabase-store` y `outbox-worker` se construyen con
**`.persistent(false)`** (ver el helper `pg_query` en ambos crates):
sqlx usa entonces el *statement sin nombre* del protocolo extendido,
que se re-prepara en cada uso y no puede colisionar. Ojo con el falso
amigo: `statement_cache_capacity(0)` en el pool (que `vercel-entry`
también configura, como cinturón extra) solo desactiva la *caché*; el
statement seguiría recibiendo nombre si la consulta no marca
`persistent(false)`.

No hace falta ejecutar `schema.sql` a mano contra la base de producción:
como todas sus sentencias son idempotentes (`CREATE TABLE IF NOT EXISTS`,
`ALTER TABLE ... ADD COLUMN IF NOT EXISTS`, `CREATE INDEX IF NOT EXISTS`),
tanto `vercel-entry` ([`db::get_db_pool`](../crates/vercel-entry/src/db.rs),
compartido por `api/mcp.rs` y `api/outbox.rs`) como `outbox-worker`
([main.rs](../crates/outbox-worker/src/main.rs), el daemon standalone) lo
aplican vía **`sqlx::raw_sql(include_str!(...))`** en su primer arranque
(cold start). Usar `raw_sql` (que emplea el protocolo de consulta simple de
Postgres) es indispensable en lugar de `sqlx::query` (que usa prepared
statements), ya que `schema.sql` contiene múltiples sentencias separadas por
punto y coma, y Postgres rechaza múltiples comandos en una consulta
preparada. Repetirlo en cada deploy o instancia nueva es seguro por
construcción; `schema.sql` es la única fuente de verdad del esquema — si
cambias una tabla, edítalo ahí y ambos binarios recogen el cambio en su
próximo arranque.

## 9. Principios SOLID en juego

* **L (Sustituibilidad de Liskov) en su máxima expresión:** `SupabaseStore` y `InMemoryStore` son completamente intercambiables para las herramientas de memoria (`MemoryTools`). Ambos se miden con la misma barra síncrona del trait `MemoryRepository` y pasan el mismo conjunto de leyes operacionales.
* **S:** `SupabaseStore` solo se encarga de traducir las peticiones a comandos SQL y transacciones atómicas. El parseo de documentos es de `okf-core`, la lógica de CAS es de `conflict-core`, y el enrutamiento es de `mcp-http`.
* **I (Segregación de Interfaces):** `SupabaseStore` implementa `NeighborSource` de forma independiente de `MemoryRepository`, permitiendo que el módulo de recorrido de grafos BFS (`graph-core`) consuma las relaciones sin conocer detalles de transacciones SQL o blobs de contenido.

## 10. Ejercicios

1. **Guiado.** ¿Qué sucede si dos hilos intentan hacer commit de conceptos
diferentes de forma simultánea? ¿Provoca `SELECT ... FOR UPDATE` un cuello
de botella? Justifica con el modelo de bloqueo de PostgreSQL.
2. **Medio.** Modifica el script de esquemas SQL para añadir restricciones
únicas o índices que eviten que un blob idéntico se inserte dos veces si por
error se elude el `ON CONFLICT DO NOTHING`.
3. **Abierto.** En el Hito 4 implementaremos el outbox transaccional. ¿Cómo
afecta esto a la transacción del `commit()` en `SupabaseStore`? Modifica la
transacción mentalmente para entender cómo el registro del outbox comparte
el mismo ciclo de vida que la cabeza del documento.
