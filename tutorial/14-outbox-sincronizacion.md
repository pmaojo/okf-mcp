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

La ruta del archivo (`{GITHUB_PATH}/{concept_id}.md`, con el mismo
default `memoria` que usa `GithubStore` en el capítulo 12b) la resuelve
`concept_path()` en [`github_sync.rs`](../crates/outbox-worker/src/github_sync.rs),
en vez de tenerla escrita dos veces. Estuvo hardcodeada como `docs/` en
una versión anterior de este código — dos rutas independientes para el
mismo archivo, una para escribir (aquí) y otra para leer (el
reconciliador de la sección 7b), sin nada que las mantuviera
sincronizadas. Si `GITHUB_PATH` se configuraba a cualquier valor
distinto de `docs`, el reconciliador miraba una carpeta vacía y
concluía que **todos** los conceptos habían sido borrados de GitHub —
ver la sección 7b para las consecuencias de ese escenario.

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

> **Nota (estado actual):** el esquema real de `crates/supabase-store/schema.sql`
> ya no es exactamente este. Una vez que la síntesis de `skill_ingest` se topó
> con un 429 por cuota agotada de Gemini en producción, `gemini-embeddings`
> ganó el mismo tipo de fallback multi-proveedor (Mistral y Cohere como
> respaldo). Como esos proveedores no truncan a 768 dimensiones como hace
> Gemini, la columna pasó de `vector(768)` a `vector` (dimensión variable), y
> se añadió `embedding_model VARCHAR(64)` para registrar con qué
> proveedor+modelo se generó cada vector — sin eso, mezclar por accidente un
> vector de un proveedor con el de otro en el mismo `ORDER BY ... <=>` no daría
> error, daría un ranking sin ningún significado.
>
> La propia síntesis de `skill_ingest` que originó esta historia ya no existe:
> se evaluó y se descartó (reescribir con un LLM no resuelve nada de licencia,
> y cuesta cuota en cada ingesta), así que hoy esa herramienta es puramente
> determinista — el único LLM que sigue en pie en este proyecto es el de
> embeddings, con el fallback que se acaba de describir.

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

El "mejor esfuerzo" hacia Supabase que abre esta sección usa el mismo `SupabaseStore::commit`/`delete` que el modo `OKF_STORE=supabase` normal — y ese método siempre encola un evento en el `outbox`, sin saber si quien lo llama es el flujo normal (donde Supabase es la fuente de verdad y ese evento es lo único que sincroniza a GitHub) o `IndexedStore` (donde GitHub *ya* recibió la escritura, de forma síncrona, un paso antes). Sin distinguir los dos casos, el paso A de la sección 3 repetiría, vía la API de Contents, un commit que la API de git data ya hizo — dos commits por cada guardado del usuario. `outbox_worker::github_sync_credentials()` (sección 8) resuelve esto devolviendo `(None, None)` cuando `OKF_STORE=github`, así que el paso A se salta sin tocar el paso B (embeddings), que sigue siendo la red de reintento legítima si el embedding inline falló.

### Optimización y Rate Limits

Dado que la API REST de GitHub tiene un límite de cuota (típicamente 5000 peticiones por hora para tokens autenticados), no podemos escanear GitHub cada 5 segundos. Por ello:
*   En el endpoint serverless de Vercel `/api/outbox`, la reconciliación se ejecuta una vez por llamada, controlada por Vercel Cron — declarado en [`crates/vercel-entry/vercel.json`](../crates/vercel-entry/vercel.json), por defecto una vez al día (`0 3 * * *`). Es deliberadamente poco frecuente: es la red de seguridad, no el camino rápido (ver 7c).
*   En el daemon persistente de `outbox-worker`, la reconciliación corre inmediatamente al iniciar y luego se repite periódicamente cada 5 minutos (aproximadamente 60 ciclos de inactividad de 5 segundos).

Esto mantiene tu base de datos de Supabase en sincronía eventual y garantiza que la búsqueda semántica (`memory_search`) siempre tenga acceso a la realidad de los archivos de GitHub — con una ventana de hasta 24h (cron diario) en la que un borrado hecho directamente en GitHub seguiría siendo "visible" en `memory_search`. La sección 7c cierra esa ventana.

## 7c. Reconciliación instantánea: el webhook de GitHub

Esperar al cron para reaccionar a un cambio hecho directamente en GitHub (fuera de las herramientas MCP) es aceptable como red de seguridad, pero deja una ventana incómoda: con el cron diario del proyecto, un concepto borrado en la web de GitHub sigue apareciendo en `memory_search` hasta 24h después. `/api/github-webhook` en `vercel-entry` cierra esa ventana a segundos, sin sustituir el cron — lo complementa.

El flujo es:

1. GitHub manda un `POST` con el payload del evento y la cabecera `X-Hub-Signature-256` (HMAC-SHA256 del cuerpo crudo, firmado con el secreto configurado al crear el webhook).
2. El handler recalcula el HMAC con `GITHUB_WEBHOOK_SECRET` y lo compara con `hmac::Mac::verify_slice` — comparación en tiempo constante, no un `==` sobre bytes, para no filtrar el secreto por temporización.
3. Filtra por cabecera `X-GitHub-Event` (solo `push`; `ping` se responde `200` sin trabajo) y por rama (`payload["ref"]` debe ser `refs/heads/{GITHUB_BRANCH}`).
4. Si pasa ambos filtros, llama exactamente a la misma `reconcile_github_to_supabase` que usa el cron — no hay una segunda implementación de la lógica de reconciliación, solo un disparador distinto.

Un detalle de diseño no evidente: la reconciliación nunca escribe de vuelta a GitHub ni encola eventos de outbox (ver 7b), así que un `push` disparado por el propio `outbox-worker` (cuando `OKF_STORE=supabase` y el paso A sincroniza a GitHub) hace que el webhook se dispare sobre sí mismo — pero al llegar, el hash en Supabase ya coincide con el de GitHub, así que la reconciliación no hace nada (`needs_sync = false`) y no hay bucle. Sin esa propiedad (una reconciliación que solo lee de GitHub y solo escribe en Supabase, nunca al revés), habría un ciclo real: push → webhook → reconciliar → escribir en GitHub → push → webhook → ...

Sin `GITHUB_WEBHOOK_SECRET` configurada, el endpoint responde `503` y no procesa nada — a diferencia de `CRON_SECRET` (sección 8), aquí no existe un modo abierto para desarrollo, porque un webhook público sin firma sería una invitación a disparar reconciliaciones (llamadas de pago a GitHub y Gemini) para cualquiera que adivine la URL.

## 8. Frontera de producción

> 🧰 **La rueda de serie:** el patrón outbox es SQL + disciplina (sin crate), pero un framework de jobs como [`apalis`](https://docs.rs/apalis) añade reintentos, backoff y métricas. El mapa completo y el criterio para elegir: [La rueda de serie](la-rueda-de-serie.md).

Para desplegar el worker:
1. Configura `GITHUB_TOKEN` (Personal Access Token de GitHub con permisos de escritura en el repo).
2. Configura `GITHUB_REPO` (formato `usuario/repositorio`).
3. Configura `GEMINI_API_KEY` (clave de la API de Google AI Studio); opcionalmente `MISTRAL_API_KEY`/`COHERE_API_KEY` como respaldo automático si Gemini falla o agota cuota (ver la nota sobre `embedding_model` más abajo).
4. Configura `POSTGRES_URL` apuntando a tu base de datos de Supabase.
5. Opcional: `GITHUB_BRANCH` (rama a sincronizar, por defecto `main`) y
   `GITHUB_PATH` (prefijo de directorio, por defecto `memoria` —
   compartido con `GithubStore::from_env`, capítulo 12b; ver la
   sección 3.A sobre por qué esto tiene que ser una sola función y no
   dos rutas escritas por separado).
6. Si vas a usar el webhook de la sección 7c: `GITHUB_WEBHOOK_SECRET`
   (obligatoria para ese endpoint — sin ella responde `503`) y el
   webhook registrado en GitHub (Settings → Webhooks, en el repo de
   contenido, no en el repo de código de este servidor).
7. Si `OKF_STORE=github` (modo `IndexedStore`, capítulo 12b): no hace
   falta ninguna variable extra para evitar el commit duplicado — lo
   resuelve automáticamente `github_sync_credentials()` (sección 7b).

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
[`SupabaseStore::search`](../crates/supabase-store/src/lib.rs) — con algún
proveedor de embeddings configurado, embebe el texto de la consulta (mismo
cliente que el worker,
[`gemini-embeddings`](../crates/gemini-embeddings/src/lib.rs), para
garantizar que ambos lados llaman al mismo fallback y nunca puedan
divergir) y ordena por `<=>` contra `embeddings`; si no hay ninguna clave
configurada, o si todos los proveedores fallan, cae de vuelta a la
búsqueda `ILIKE` de siempre. Eso NO es todavía el hybrid search que pide
este ejercicio: son dos consultas alternativas, no una sola consulta que
combine ambas señales de relevancia en un mismo `ORDER BY` — sigue siendo un
ejercicio abierto diseñar esa combinación.

   Una complicación adicional desde que `gemini-embeddings` ganó un
fallback multi-proveedor (Gemini con Mistral y Cohere como respaldo si
Gemini falla o agota cuota): vectores de proveedores distintos NO son
comparables entre sí aunque compartan dimensionalidad — cada modelo
aprende su propio espacio vectorial, y una distancia de coseno entre dos
espacios distintos no da error, da un ranking sin significado. Por eso la
columna `embeddings.embedding_model` etiqueta con qué proveedor+modelo se
generó cada vector, y `search_semantic` filtra SIEMPRE por
`embedding_model = <el de la consulta actual>` antes de ordenar por
`<=>` — cualquier diseño de hybrid search para este ejercicio tiene que
respetar ese mismo filtro, o arriesga mezclar espacios vectoriales
incompatibles en el mismo `ORDER BY`.
