# Capítulo 14 — Transactional Outbox: Sincronización en red y pgvector

Crate: [`crates/outbox-worker`](../crates/outbox-worker)

## 1. El problema

Cuando guardamos un documento en nuestra memoria MCP relacional, queremos realizar tres acciones derivadas:
1. Confirmar el cambio en PostgreSQL (fuente de verdad).
2. Sincronizar el contenido con un repositorio Git (como archivos markdown).
3. Generar un vector de embeddings para realizar búsquedas semánticas.

Si intentamos hacer las tres operaciones dentro de la petición HTTP del usuario:
* **Escritura Doble:** ¿Qué pasa si la base de datos se actualiza pero el push a Git falla? ¿O si Git se actualiza pero la llamada al modelo de embeddings da un timeout? El sistema quedará desincronizado.
* **Latencia Excesiva:** Llamar a la API de GitHub y a la API de Gemini suma segundos de espera para el usuario, anulando los beneficios de un endpoint HTTP rápido.

## 2. El invariante

> **Toda confirmación exitosa en el repositorio de documentos garantiza que, eventualmente, el cambio se propagará a Git y sus embeddings se indexarán de forma consistente en la base de datos vectorial.**

Y una segunda promesa de aislamiento:

> **El procesamiento y reintento de eventos fallidos por fallas de red externas nunca bloquea ni revierte la transacción original de almacenamiento del documento.**

## 3. La implementación mínima

Para conseguir esto, aplicamos el patrón **Transactional Outbox**. En lugar de escribir en los sistemas externos durante el commit, registramos un "evento" en la tabla `outbox` dentro de la misma transacción SQL en la que actualizamos `heads` y `revisions`:

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

Un servicio independiente, [`outbox-worker`](../crates/outbox-worker/src/main.rs), procesa los eventos en segundo plano. Para escalar de forma segura con múltiples réplicas concurrentes del worker, consumimos los eventos usando `FOR UPDATE SKIP LOCKED`:

```sql
SELECT seq, event_type, concept_id, content_id, payload
FROM outbox
WHERE status = 'pending'
ORDER BY seq
LIMIT 10
FOR UPDATE SKIP LOCKED
```

Esto bloquea únicamente las filas seleccionadas y hace que otros workers ignoren de manera transparente las filas que ya se están procesando en otro proceso.

### A. Sincronización con GitHub
Dado que las funciones serverless de Vercel no disponen de binario `git` ni filesystem local persistente, interactuamos con el repositorio directamente usando la **API de Contenidos de GitHub** (vía HTTPS REST). Realizamos una petición `PUT` con el contenido base64 del archivo y el `sha` de la versión anterior (si ya existía):

```rust
let content_b64 = BASE64_STANDARD.encode(markdown.as_bytes());
let put_req = GithubPutRequest {
    message: reason.to_string(),
    content: content_b64,
    sha,
};
```

### B. Indexación en `pgvector`
Para habilitar búsquedas vectoriales, cargamos la extensión `vector` en PostgreSQL y guardamos los vectores obtenidos de la API de Gemini:

```sql
CREATE EXTENSION IF NOT EXISTS vector;
CREATE TABLE embeddings (
    concept_id VARCHAR(255) PRIMARY KEY REFERENCES heads(concept_id) ON DELETE CASCADE,
    embedding vector(768)
);
```

Como el driver de base de datos no siempre implementa tipos de vectores nativos, formateamos el vector flotante de Gemini como una cadena de texto estructurada `"[0.1, 0.2, ...]"`. PostgreSQL y `pgvector` interpretan este formato de forma nativa e independiente del controlador:

```rust
let vector_str = format!("[{}]", values.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(","));
```

## 4. Una versión deliberadamente rota

Imagina realizar las llamadas de API dentro de la lógica del repositorio:

```rust
// ❌ NO HACER: Escritura dual en línea
self.db.update_head(concept_id, incoming_hex)?;
self.github_api.push_file(concept_id, markdown)?; // Si esto falla, ¿hacemos rollback de la DB?
self.gemini_api.generate_embedding(markdown)?;
```

## 5. Por qué falla

Esta versión rota sufre de fallas de escritura dual. Si el servidor de GitHub tiene una caída del servicio o nuestras credenciales expiran, la llamada de GitHub fallará. Si hacemos rollback de la base de datos, el usuario no podrá guardar su documento aunque nuestra base de datos esté perfectamente sana. Si NO hacemos rollback, la base de datos tendrá el cambio pero el archivo en Git se perderá para siempre. Decoplar las escrituras mediante una cola transaccional local (`outbox`) elimina este problema.

## 6. Memoria y asignación

El worker procesa cada evento en su propia transacción atómica. Si una API externa falla, el worker captura el error, incrementa la columna `attempts` y marca la fila en el outbox para posterior reintento, liberando la fila para que no bloquee al resto de la cola. Al alcanzar los 5 intentos fallidos, el estado cambia a `failed` para intervención humana o alarma, manteniendo la cola fluida.

## 7. Tests

El worker ofrece dos modos de ejecución:
1. **Daemon:** Ciclo continuo con sleep de 5 segundos (por defecto para despliegue de contenedores).
2. **One-shot:** Se activa y procesa hasta vaciar la cola, y luego finaliza (activado con la variable `ONCE=1`). Esto es ideal para ejecutarse mediante triggers serverless (Vercel Cron) o suites de tests.

## 8. Frontera de producción

Para desplegar el worker:
1. Configura `GITHUB_TOKEN` (Personal Access Token de GitHub con permisos de escritura en el repo).
2. Configura `GITHUB_REPO` (formato `usuario/repositorio`).
3. Configura `GEMINI_API_KEY` (clave de la API de Google AI Studio).
4. Configura `DATABASE_URL` apuntando a tu base de datos de Supabase.

## 9. Principios SOLID en juego

* **S (Responsabilidad Única):** El worker no valida el formato de los documentos ni procesa peticiones JSON-RPC. Su única y exclusiva responsabilidad es despachar de forma eventual e idempotente los eventos del outbox hacia los sistemas satélite.
* **D (Inversión de Dependencias):** El repositorio principal (`SupabaseStore`) no necesita conocer qué es una llamada HTTP o una API de embeddings. Simplemente inserta metadatos estructurados en una tabla de eventos, dejando que el worker se acople a los proveedores concretos.

## 10. Ejercicios

1. **Guiado.** ¿Por qué es necesario usar `SKIP LOCKED` en lugar de un simple `FOR UPDATE` al procesar la cola con múltiples workers? Simula la llegada de tres workers concurrentes.
2. **Medio.** Modifica el worker para implementar un exponencial backoff (tiempo de espera progresivo) antes de reintentar eventos que hayan fallado temporalmente.
3. **Abierto.** Diseña una consulta SQL que permita realizar búsquedas semánticas combinadas (Hybrid Search): buscar conceptos que coincidan con un término de búsqueda relacional y ordenarlos por distancia de coseno (`<=>`) de sus vectores de embeddings en una sola consulta SQL.
