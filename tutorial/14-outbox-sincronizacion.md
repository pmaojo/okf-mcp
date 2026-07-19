# Capítulo 14 — Transactional Outbox: Sincronización en red y pgvector

Crate: [`crates/outbox-worker`](../crates/outbox-worker)

## 1. El problema

Cuando guardamos un documento en nuestra memoria MCP relacional, queremos
realizar tres acciones derivadas:
1. Confirmar el cambio en PostgreSQL (fuente de verdad).
2. Sincronizar el contenido con un repositorio Git (como archivos markdown).
3. Generar un vector de embeddings para realizar búsquedas semánticas.

Si intentamos hacer las tres operaciones dentro de la petición HTTP del usuario:
* **Escritura Doble:** ¿Qué pasa si la base de datos se actualiza pero el push a Git falla? ¿O si Git se actualiza pero la llamada al modelo de embeddings da un timeout? El sistema quedará desincronizado.
* **Latencia Excesiva:** Llamar a la API de GitHub y a la API de Gemini suma segundos de espera para el usuario, anulando los beneficios de un endpoint HTTP rápido.


El conflicto aparece después del commit feliz: PostgreSQL ya confirmó, pero
GitHub devuelve 500 o el proveedor de embeddings agota tiempo. Si todo vivía
en la petición, el usuario queda atrapado entre latencia y desincronización.
La frontera hexagonal separa la fuente de verdad del adaptador de
integración: el núcleo escribe una intención duradera; los sistemas externos
se alcanzan después.

## 2. El invariante

> **Toda confirmación exitosa en el repositorio de documentos garantiza que, eventualmente, el cambio se propagará a Git y sus embeddings se indexarán de forma consistente en la base de datos vectorial.**

Y una segunda promesa de aislamiento:

> **El procesamiento y reintento de eventos fallidos por fallas de red externas nunca bloquea ni revierte la transacción original de almacenamiento del documento.**

## 3. La implementación mínima

Para conseguir esto, aplicamos el patrón **Transactional Outbox**. En lugar
de escribir en los sistemas externos durante el commit, registramos un
"evento" en la tabla `outbox` dentro de la misma transacción SQL en la que
actualizamos `heads` y `revisions`:

```sql
CREATE TABLE outbox (
    seq BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    event_type VARCHAR(50) NOT NULL,
    concept_id VARCHAR(255) NOT NULL,
    content_id VARCHAR(64) NOT NULL,
    payload JSONB NOT NULL,
    status VARCHAR(20) NOT NULL DEFAULT 'pending',
    attempts INT NOT NULL DEFAULT 0,
    created_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP,
    processed_at TIMESTAMP WITH TIME ZONE
);
```

Un servicio independiente,
[`outbox-worker`](../crates/outbox-worker/src/main.rs), procesa los eventos
en segundo plano. Para escalar de forma segura con múltiples réplicas
concurrentes del worker, consumimos los eventos usando `FOR UPDATE SKIP
LOCKED`:

```sql
SELECT seq, event_type, concept_id, content_id, payload
FROM outbox
WHERE status = 'pending'
ORDER BY seq
LIMIT 10
FOR UPDATE SKIP LOCKED
```

Esto bloquea únicamente las filas seleccionadas y hace que otros workers
ignoren de manera transparente las filas que ya se están procesando en otro
proceso.

### A. Sincronización con GitHub
Dado que las funciones serverless de Vercel no disponen de binario `git` ni
filesystem local persistente, interactuamos con el repositorio directamente
usando la **API de Contenidos de GitHub** (vía HTTPS REST). Realizamos una
petición `PUT` con el contenido base64 del archivo y el `sha` de la versión
anterior (si ya existía):

```rust
let content_b64 = BASE64_STANDARD.encode(markdown.as_bytes());
let put_req = GithubPutRequest {
    message: reason.to_string(),
    content: content_b64,
    sha,
};
```

### B. Indexación en `pgvector`
Para habilitar búsquedas vectoriales, cargamos la extensión `vector` en
PostgreSQL y guardamos los vectores obtenidos de la API de Gemini:

```sql
CREATE EXTENSION IF NOT EXISTS vector;
CREATE TABLE embeddings (
    concept_id VARCHAR(255) PRIMARY KEY REFERENCES heads(concept_id) ON DELETE CASCADE,
    embedding vector(768)
);
```

Para pasar el vector flotante de Gemini a la consulta usamos el crate
[`pgvector`](https://docs.rs/pgvector) (con su feature `sqlx`), que
enseña a `sqlx` el tipo `vector` de PostgreSQL y lo transmite en
binario, tipado, como cualquier otro parámetro de `bind`:

```rust
use pgvector::Vector;

sqlx::query("INSERT INTO embeddings (concept_id, embedding) VALUES ($1, $2)")
    .bind(concept_id)
    .bind(Vector::from(values))
```

Aquí ya estamos en un adaptador, así que aplica la segunda mitad de la
regla del apéndice [la rueda de serie](la-rueda-de-serie.md):
*despliega la de serie*. El vector viaja tipado y en binario, sin
ida-y-vuelta por texto.

## 3.5. Conceptos de Rust en este capítulo

Este capítulo combina APIs de terceros y transformaciones de datos para la sincronización eventual:

* **Tipos de terceros en `bind` (newtypes sobre el driver):** `pgvector::Vector` es un newtype sobre `Vec<f32>` que implementa los traits `Type`/`Encode` de `sqlx`; por eso puede pasarse a `bind()` igual que un `&str` o un `i64`. Es el mismo patrón newtype del capítulo 1, aplicado por un crate externo para extender un driver sin tocarlo (principio O de SOLID).
* **Bucles en segundo plano (Daemon loops):** Para que el worker procese eventos continuamente en segundo plano sin devorar el 100% de la CPU, usamos un bucle `loop` combinado con pausas asíncronas (`tokio::time::sleep()`). Esto suspende la ejecución del worker temporalmente, liberando la CPU hasta que transcurra el tiempo configurado o llegue una señal del sistema.
* **Integración con crates externos (Base64):** Rust no incluye codificación base64 en su biblioteca estándar. En este capítulo se utiliza el crate `base64` para codificar los archivos en el formato que exige la API REST de GitHub. En el `Cargo.toml` del worker declaramos esta dependencia, la cual se descarga y compila de forma aislada, manteniendo las fronteras limpias.

## 4. Una versión deliberadamente rota

Imagina realizar las llamadas de API dentro de la lógica del repositorio:

```rust
// ❌ NO HACER: Escritura dual en línea
self.db.update_head(concept_id, incoming_hex)?;
self.github_api.push_file(concept_id, markdown)?; // Si esto falla, ¿hacemos rollback de la DB?
self.gemini_api.generate_embedding(markdown)?;
```


La versión rota es tentadora porque parece transaccional en código:
`commit`, luego `push`, luego `embed`, todo en una función lineal. Una
persona razonable la escribe porque quiere que el usuario vea todo terminado
antes de responder.

## 5. Por qué falla

Esta versión rota sufre de fallas de escritura dual. Si el servidor de
GitHub tiene una caída del servicio o nuestras credenciales expiran, la
llamada de GitHub fallará. Si hacemos rollback de la base de datos, el
usuario no podrá guardar su documento aunque nuestra base de datos esté
perfectamente sana. Si NO hacemos rollback, la base de datos tendrá el
cambio pero el archivo en Git se perderá para siempre. Decoplar las
escrituras mediante una cola transaccional local (`outbox`) elimina este
problema.

## 6. Memoria y asignación

El worker procesa cada evento en su propia transacción atómica. Si una API
externa falla, el worker captura el error, incrementa la columna `attempts`
y marca la fila en el outbox para posterior reintento, liberando la fila
para que no bloquee al resto de la cola. Al alcanzar los 5 intentos
fallidos, el estado cambia a `failed` para intervención humana o alarma,
manteniendo la cola fluida.

## 7. Tests

El worker ofrece dos modos de ejecución:
1. **Daemon:** Ciclo continuo con sleep de 5 segundos (por defecto para despliegue de contenedores).
2. **One-shot:** Se activa y procesa hasta vaciar la cola, y luego finaliza
(activado con la variable `ONCE=1`). Esto es ideal para ejecutarse mediante
triggers serverless (Vercel Cron) o suites de tests.


En TDD, el primer test sustituye GitHub o embeddings por un adaptador que
falla y comprueba que el documento persiste y queda un evento pendiente. El
rojo que buscamos observar no es un error HTTP bonito, sino una pérdida de
sincronización: commit confirmado sin outbox o outbox procesado dos veces.

## 7b. El Reconciliador Inverso: GitHub -> Supabase

Cuando el servidor MCP opera en modo híbrido (`IndexedStore`), las escrituras del usuario viajan directamente a GitHub (la fuente de verdad última) y se hace un intento de mejor esfuerzo (*best-effort*) de actualizar Supabase sincrónicamente. 

Para garantizar la consistencia eventual si Supabase estuvo caída, o para sincronizar cambios empujados directamente al repositorio de GitHub (por ejemplo, mediante un `git push` de la terminal de un usuario o una edición en la web de GitHub), implementamos un **bucle de reconciliación inversa** en el `outbox-worker`.

Este bucle de reconciliación actúa como un controlador de estado continuo y sigue este algoritmo en cada ciclo:

1.  **Obtención de la realidad (GitHub)**: Llama a `github_store.search` y `github_store.get` (usando la interfaz pública abstracta de `MemoryRepository`) para obtener todos los conceptos vivos en el repositorio con sus contenidos, hashes y versiones.
2.  **Lectura del índice actual (Supabase)**: Consulta las tablas `heads` y `blobs` de Supabase para tener un mapa de lo que está registrado actualmente en la base de datos (incluyendo si está marcado como borrado lógico).
3.  **Conciliación de diferencias**:
    *   **Crear o Actualizar**: Si un concepto de GitHub no existe en Supabase, o si su hash de contenido (`content_id`) o versión difieren, se inicia una transacción SQL para:
        *   Insertar el raw text en `blobs`.
        *   Actualizar `heads` (limpiando `deleted_at`).
        *   Insertar una entrada en `revisions` firmada por `system/reconciler`.
        *   Reconstruir las relaciones en `links`.
        *   *Opcional*: Solicitar a la API de Gemini el vector de embeddings y guardarlo en la tabla `embeddings`.
    *   **Borrado Lógico**: Si un concepto existe como activo en Supabase pero no se encuentra en el listado de GitHub, significa que fue eliminado externamente. Se realiza un borrado lógico en Supabase (`deleted_at = CURRENT_TIMESTAMP`), se borran sus enlaces salientes y se elimina su embedding vectorial.

### Optimización y Rate Limits

Dado que la API REST de GitHub tiene un límite de cuota (típicamente 5000 peticiones por hora para tokens autenticados), no podemos escanear GitHub cada 5 segundos. Por ello:
*   En el endpoint serverless de Vercel `/api/outbox`, la reconciliación se ejecuta una vez por llamada (controlado por Vercel Cron, ej. cada 10 minutos).
*   En el daemon persistente de `outbox-worker`, la reconciliación corre inmediatamente al iniciar y luego se repite periódicamente cada 5 minutos (aproximadamente 60 ciclos de inactividad de 5 segundos).

Esto mantiene tu base de datos de Supabase en sincronía eventual y garantiza que la búsqueda semántica (`memory_search`) siempre tenga acceso a la realidad de los archivos de GitHub.

## 8. Frontera de producción

> 🧰 **La rueda de serie:** el patrón outbox es SQL + disciplina (sin crate), pero un framework de jobs como [`apalis`](https://docs.rs/apalis) añade reintentos, backoff y métricas. El mapa completo y el criterio para elegir: [La rueda de serie](la-rueda-de-serie.md).

Para desplegar el worker:
1. Configura `GITHUB_TOKEN` (Personal Access Token de GitHub con permisos de escritura en el repo).
2. Configura `GITHUB_REPO` (formato `usuario/repositorio`).
3. Configura `GEMINI_API_KEY` (clave de la API de Google AI Studio).
4. Configura `POSTGRES_URL` apuntando a tu base de datos de Supabase.

### Despliegue en Railway

El repositorio incluye configuración lista para desplegar el daemon en
[Railway](https://railway.app):

- `Procfile`: define el proceso `worker` que ejecuta el binario compilado.
- `railway.toml`: configura el build de Nixpacks y el comando de inicio.
- `DEPLOY_RAILWAY.md`: guía paso a paso con variables de entorno y solución de problemas.

Pasos resumidos:

1. Crea un proyecto en Railway conectando el repo `pmaojo/okf-mcp`.
2. Añade una base de datos PostgreSQL (o usa Supabase) y configura `POSTGRES_URL`.
3. Configura `GITHUB_TOKEN`, `GITHUB_REPO` y, opcionalmente, `GEMINI_API_KEY`.
4. Railway compilará el workspace con `cargo build --release -p outbox-worker` y ejecutará el daemon.

Para más detalles, consulta [`DEPLOY_RAILWAY.md`](../DEPLOY_RAILWAY.md).

## 9. Principios SOLID en juego

* **S (Responsabilidad Única):** El worker no valida el formato de los documentos ni procesa peticiones JSON-RPC. Su única y exclusiva responsabilidad es despachar de forma eventual e idempotente los eventos del outbox hacia los sistemas satélite.
* **D (Inversión de Dependencias):** El repositorio principal (`SupabaseStore`) no necesita conocer qué es una llamada HTTP o una API de embeddings. Simplemente inserta metadatos estructurados en una tabla de eventos, dejando que el worker se acople a los proveedores concretos.

## 10. Ejercicios

1. **Guiado.** ¿Por qué es necesario usar `SKIP LOCKED` en lugar de un
simple `FOR UPDATE` al procesar la cola con múltiples workers? Simula la
llegada de tres workers concurrentes.
2. **Medio.** Modifica el worker para implementar un exponencial backoff
(tiempo de espera progresivo) antes de reintentar eventos que hayan fallado
temporalmente.
3. **Abierto.** Diseña una consulta SQL que permita realizar búsquedas
semánticas combinadas (Hybrid Search): buscar conceptos que coincidan con un
término de búsqueda relacional y ordenarlos por distancia de coseno (`<=>`)
de sus vectores de embeddings en una sola consulta SQL.

   El camino de lectura ya está resuelto en
[`SupabaseStore::search`](../crates/supabase-store/src/lib.rs) — cuando hay
`GEMINI_API_KEY` configurada, embebe el texto de la consulta (mismo cliente
que el worker,
[`gemini-embeddings`](../crates/gemini-embeddings/src/lib.rs), para
garantizar el mismo modelo en ambos lados) y ordena por `<=>` contra
`embeddings`; si no hay clave, o si Gemini falla, cae de vuelta a la
búsqueda `ILIKE` de siempre. Eso NO es todavía el hybrid search que pide
este ejercicio: son dos consultas alternativas, no una sola consulta que
combine ambas señales de relevancia en un mismo `ORDER BY` — sigue siendo un
ejercicio abierto diseñar esa combinación.
