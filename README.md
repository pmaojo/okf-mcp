# okf-mcp — Servidor MCP de memoria en Rust (núcleo `std`-only)

Un servidor de memoria persistente para agentes (protocolo MCP) con
arquitectura hexagonal: el **motor de conocimiento** y los **puertos**
están escritos con la biblioteca estándar de Rust, sin frameworks, sin
`serde` y sin `tokio` en el núcleo. Las dependencias externas quedan
aisladas en adaptadores de frontera e infraestructura como Vercel,
Supabase, GitHub y Gemini.

Este repositorio es a la vez un proyecto real y un **tutorial muy
didáctico** de Rust y de principios SOLID: ver [`tutorial/`](tutorial/).
La referencia de API generada con `cargo doc` se publica en
**<https://pmaojo.github.io/okf-mcp/>** en cada push a `main`
(capítulo 16 del tutorial).

## Estado

- ✅ **Hito 1:** núcleo `std`-only + servidor MCP por stdio.
- ✅ **Hito 2:** transporte HTTP sin estado (`mcp-http`) + contrato ejecutable `MemoryRepository` + adaptador de Vercel (`vercel-entry`) + adaptador de base de datos PostgreSQL (`supabase-store`).
- ✅ **Hito 3:** OAuth 2.1 (Resource Server, validación criptográfica de JWTs mediante firmas y JWKS).
- ✅ **Hito 4:** Transactional Outbox (`outbox-worker` con procesamiento concurrente `SKIP LOCKED`, sincronización con GitHub y embeddings con `pgvector`).
- ✅ **Hito 5:** MCP Apps (visualizaciones interactivas de grafo e historial mediante recursos `ui://`, opcional).

## Arquitectura

```text
                       Adaptadores de entrada
┌────────────────────────────────────────────────────────────────────┐
│ mcp-stdio    bin local por stdin/stdout                            │
│ mcp-http     bin HTTP/1.1 sin estado sobre TcpListener, POST /mcp  │
│ vercel-entry función serverless Axum/Vercel → mcp_http::route()    │
└───────────────┬────────────────────────────────────────────────────┘
                │
                ▼
                     Núcleo y puertos `std`-only
┌────────────────────────────────────────────────────────────────────┐
│ mcp-core     JSON-RPC 2.0 + ciclo de vida MCP + despacho           │
│ json-mini    parser/serializador JSON educativo                    │
│ memory-tools 14 herramientas MCP genéricas sobre MemoryRepository  │
│ store-core   puerto MemoryRepository + contrato Liskov             │
│ memory-model ConceptId, ContentId, Budget, Revision, Principal     │
│ okf-core     frontmatter YAML (subconjunto) + enlaces [[...]]      │
│ graph-core   BFS acotado (trait NeighborSource)                    │
│ conflict-core decisiones compare-and-swap puras                    │
│ hash-core    SHA-256 a mano (vectores NIST)                        │
│ memory-store InMemoryStore para desarrollo y tests                 │
│ ingest-core  detección/planificación de skill_ingest + puertos     │
└───────────────┬────────────────────────────────────────────────────┘
                │
                ▼
                    Adaptadores de salida / infraestructura
┌────────────────────────────────────────────────────────────────────┐
│ supabase-store    SupabaseStore implementa MemoryRepository        │
│ outbox-worker     procesa outbox, GitHub y embeddings              │
│ gemini-embeddings cliente del proveedor de embeddings              │
│ ingest-http       fetch de GitHub + síntesis Gemini (skill_ingest) │
└────────────────────────────────────────────────────────────────────┘
```

La inversión de dependencias se mantiene en el límite hexagonal: las
herramientas dependen del trait `MemoryRepository` definido en
`store-core`, y tanto `InMemoryStore` como `SupabaseStore` implementan
ese puerto. Así el núcleo no conoce PostgreSQL, Vercel, GitHub ni
Gemini.

### Regla de dependencias del workspace

- **Núcleo y puertos sin dependencias externas de producción:**
  `memory-model`, `hash-core`, `json-mini`, `okf-core`, `graph-core`,
  `conflict-core`, `store-core`, `memory-store`, `memory-tools`,
  `mcp-core`, `mcp-stdio`, `mcp-http` e `ingest-core`.
- **Adaptadores con dependencias externas permitidas:**
  - `vercel-entry`: `tokio`, `axum`, `tower`, `tower-http`,
    `vercel_runtime`, `sqlx`, `jsonwebtoken`, `reqwest`, `serde` y
    `serde_json` para la función serverless, CORS, OAuth/JWT y acceso a
    PostgreSQL.
  - `supabase-store`: `tokio`, `sqlx`, `pgvector`, `serde`,
    `serde_json`, `reqwest` y `gemini-embeddings` para persistencia
    PostgreSQL/Supabase y búsqueda semántica opcional.
  - `outbox-worker`: `tokio`, `sqlx`, `pgvector`, `serde`,
    `serde_json`, `reqwest`, `base64` y `gemini-embeddings` para
    procesar eventos pendientes y sincronizar con servicios externos.
  - `gemini-embeddings`: `reqwest`, `serde` y `thiserror` para llamar a
    la API de embeddings de Gemini.
  - `ingest-http`: `tokio`, `reqwest`, `serde` y `serde_json` para
    descargar fuentes de GitHub y sintetizar con Gemini
    (`generateContent`) en la herramienta `skill_ingest`.
- `json-mini` aparece como *dev-dependency* en algunos crates solo para
  parsear aserciones de tests.

Las dependencias de los adaptadores se auditan en CI con `cargo deny`
(advisories RUSTSEC, lista blanca de licencias, duplicados y fuentes;
política en [`deny.toml`](deny.toml)) — es el criterio 2 del apéndice
[la rueda de serie](tutorial/la-rueda-de-serie.md) convertido en paso
de workflow.

Todos los crates llevan `#![forbid(unsafe_code)]`. Si se usa
`scripts/check-std-only.sh`, debe interpretarse como una comprobación
del núcleo y de los puertos `std`-only, excluyendo explícitamente los
adaptadores de frontera e infraestructura anteriores.

## Uso

```bash
cargo test               # toda la suite (doctests incluidos)
./scripts/check-docs.sh  # docs del núcleo sin warnings + doctests
cargo doc --no-deps --open        # la referencia de API, en local
cargo run -p mcp-stdio   # servidor MCP por stdio
PORT=8787 cargo run -p mcp-http   # servidor MCP por HTTP (POST /mcp)
```

Ejemplo de sesión manual (una petición JSON por línea):

```bash
cargo run -p mcp-stdio <<'EOF'
{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"manual","version":"0"}}}
{"jsonrpc":"2.0","method":"notifications/initialized"}
{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"memory_commit","arguments":{"concept_id":"people/alice","reason":"alta","markdown":"---\ntype: person\ntitle: Alice\n---\nTrabaja en [[projects/okf-mcp]].\n"}}}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"memory_resolve","arguments":{"concept_id":"people/alice"}}}
EOF
```

Para conectarlo a Claude Code como servidor MCP local:

```bash
claude mcp add okf-memory -- cargo run -q -p mcp-stdio
```

En local, `mcp-stdio` y `mcp-http` usan `InMemoryStore`: la memoria
vive en RAM y cada proceso empieza vacío. La persistencia de producción
existe en `SupabaseStore`, que usa PostgreSQL/Supabase desde el
adaptador `vercel-entry` cuando está configurada la variable
`POSTGRES_URL`.

Lo mismo por HTTP local:

```bash
PORT=8787 cargo run -q -p mcp-http &
curl -s -X POST http://127.0.0.1:8787/mcp -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"memory_commit","arguments":{"concept_id":"people/alice","reason":"alta","markdown":"---\ntype: person\ntitle: Alice\n---\nhola\n"}}}'
```

`ALLOWED_ORIGINS` (lista separada por comas) restringe qué `Origin` de
navegador se acepta; sin configurar, cualquier origin pasa — aceptable
en desarrollo, nunca en producción.

## Las 14 herramientas

| Herramienta | Qué hace |
| --- | --- |
| `memory_search` | candidatos compactos de búsqueda híbrida (textual + semántica) |
| `memory_resolve` | Markdown exacto + vecindario acotado del grafo de `[[enlaces]]` |
| `memory_commit` | escritura con compare-and-swap (`expected_hash`) y `dry_run` |
| `memory_history` | revisiones de más reciente a más antigua, paginadas |
| `memory_delete` | borrado lógico con expected_hash |
| `memory_list` | listar metadatos de conceptos bajo un prefijo sin leer contenido |
| `memory_backlinks` | obtener enlaces entrantes hacia un concepto |
| `memory_embed` | forzar generación e indexación de embeddings pendientes |
| `memory_patch` | actualizar campos de frontmatter selectivamente sin alterar el cuerpo |
| `memory_bulk_commit` | commits en lote, con opción de atómico (rollback completo) |
| `memory_validate` | reportar enlaces rotos, referencias a borrados y embeddings obsoletos |
| `memory_status` | resumen operativo rápido de la salud del sistema |
| `memory_stats` | estadísticas del grafo (hubs, huérfanos, recuentos de tipos/tags) |
| `skill_ingest` | ingerir skills de un repo/carpeta/archivo externo del lado del servidor |

### `skill_ingest`

Ingiere skills desde una fuente externa **sin que el contenido pase por
el contexto del modelo cliente**: el servidor descarga, detecta el
formato, empaqueta y commitea; al cliente solo le llega el resumen del
resultado. Es el mismo principio que `memory_embed` — delegar el
trabajo pesado al servidor.

```json
{"name":"skill_ingest","arguments":{
  "source":"udapy/rust-agentic-skills",
  "path_prefix":"skills/programming",
  "dry_run":true
}}
```

- `source`: URL de repo, subcarpeta (`.../tree/main/skills`) o archivo
  de GitHub, el atajo `owner/repo`, o una URL directa a un archivo.
- `format` (opcional): `auto` (por defecto), `agentic-skills`
  (convención `SKILL.md` por subdirectorio, la de `npx skills add`),
  `shadcn` (`components/ui/*.tsx`), `okf` (ya es OKF → commit con los
  bytes exactos) o `raw` (envolver markdown tal cual).
- `synthesize` (opcional): por defecto la conversión es
  **determinista** — se genera solo la cabecera OKF (`type: skill`,
  `title`, `tags`, `source`) y el contenido original se conserva
  íntegro, etiquetado `verbatim-import`. Con `synthesize: true` el
  servidor genera con Gemini un resumen original (etiquetado
  `synthesized`) en lugar del texto de terceros — útil si la licencia
  de la fuente no permite copiarlo.
- `dry_run` (opcional): devuelve el plan (unidades, títulos, acciones)
  sin escribir nada.

Un repo con varias skills (`skills/*/SKILL.md`) produce un concepto
por skill en una sola llamada, con el mismo mecanismo interno que
`memory_bulk_commit` (no atómico: cada unidad se aplica o se descarta
por su cuenta y el resumen lo cuenta todo). La reingesta es
idempotente si nada cambió; si el concepto ya existe con otro
contenido, la unidad se descarta con un aviso — actualizar exige
`memory_commit` con `expected_hash`, como cualquier otra escritura.

La herramienta solo se anuncia en despliegues con el adaptador de
descarga configurado (`vercel-entry`); `mcp-stdio` y `mcp-http`
locales son `std`-only y no la exponen. `GITHUB_TOKEN` (opcional)
sube el límite de peticiones de la API de GitHub y permite repos
privados; `GEMINI_SYNTHESIS_MODEL` (opcional) cambia el modelo de
síntesis sin redesplegar (por defecto `gemini-3.5-flash`).

## Desplegar en Vercel

1. En el dashboard de Vercel: **Add New Project** → importa
   `pmaojo/okf-mcp` desde GitHub.
2. **Importante:** en la configuración del proyecto, fija
   **Root Directory** = `crates/vercel-entry` — ahí es donde vive el
   `Cargo.toml` + `api/mcp.rs` que el *builder* de Rust de Vercel
   espera encontrar (el repo entero es un *workspace* de Cargo; este
   crate es el adaptador que sabe hablar con Vercel).
3. Configura las variables de entorno necesarias:
   - `POSTGRES_URL` (obligatoria): cadena de conexión PostgreSQL usada
     por `SupabaseStore` y por el endpoint de outbox.
   - `ALLOWED_ORIGINS` (muy recomendada): lista de origins permitidos,
     separada por comas. Sin ella, cualquier origin de navegador se
     acepta.
   - `JWKS_URL` y `JWT_AUDIENCE` (recomendadas en producción): activan
     validación criptográfica de JWTs; sin `JWKS_URL`, el adaptador MCP
     corre en modo local/desarrollo abierto.
   - `OAUTH_ISSUER` y `SUPABASE_ANON_KEY`: habilitan el proxy OAuth y la
     pantalla de consentimiento hacia Supabase.
   - `GEMINI_API_KEY` (opcional): habilita búsqueda semántica y
     embeddings; sin ella, la búsqueda degrada a coincidencia textual.
     También habilita el modo `synthesize: true` de `skill_ingest`.
   - `GITHUB_TOKEN` (opcional): lo usa `skill_ingest` para subir el
     límite de peticiones de la API de GitHub y acceder a repos
     privados al descargar fuentes.
   - `GEMINI_SYNTHESIS_MODEL` (opcional): modelo de generación para la
     síntesis de `skill_ingest` (por defecto `gemini-3.5-flash`).
4. Si usas la integración de Supabase en el marketplace de Vercel,
   mapea sus credenciales a los nombres anteriores. El código actual
   espera `POSTGRES_URL` para la conexión de base de datos.

[`crates/vercel-entry/vercel.json`](crates/vercel-entry/vercel.json)
reescribe `/mcp` → `/api/mcp` (y el descubrimiento OAuth,
`/.well-known/oauth-protected-resource` → `/api/mcp`) para que la URL
pública sea la que promete el resto de esta documentación. Vive DENTRO
de `crates/vercel-entry`, no en la raíz del repo: como el **Root
Directory** del proyecto está fijado ahí (punto anterior), Vercel solo
lee `vercel.json` relativo a esa carpeta — un `vercel.json` en la raíz
del repo se ignora en silencio.

## Outbox, GitHub y embeddings

El hito 4 se implementa con un patrón **Transactional Outbox**: las
escrituras persistidas generan eventos pendientes, y un worker separado
los procesa por lotes con `SKIP LOCKED` para permitir concurrencia sin
pisarse.

Hay dos formas de ejecutarlo:

- `cargo run -p outbox-worker`: daemon de larga duración pensado para
  Fly.io, Railway, un contenedor o un VPS. Repite el procesamiento cada
  pocos segundos cuando no hay trabajo.
- `/api/outbox` en `vercel-entry`: handler serverless pensado para
  Vercel Cron. Ejecuta un lote por invocación; el cron está declarado en
  `crates/vercel-entry/vercel.json`.

Variables de entorno:

| Variable | Obligatoria | Uso |
| -------- | ----------- | --- |
| `POSTGRES_URL` | Sí | Conexión PostgreSQL/Supabase para leer y marcar eventos. |
| `GITHUB_TOKEN` | No | Token para sincronizar documentos con GitHub. Si falta, se omite esa sincronización. |
| `GITHUB_REPO` | No | Repositorio destino en formato `usuario/repositorio`. Si falta, se omite GitHub. |
| `GEMINI_API_KEY` | No | Genera embeddings para `pgvector`; si falta, se omite esa parte. |
| `ONCE` | No | En el daemon local, procesa un lote y sale cuando está presente. |
| `CRON_SECRET` | Recomendado en Vercel | Protege `/api/outbox` con `Authorization: Bearer <CRON_SECRET>`. Sin él, el endpoint queda abierto para desarrollo. |

El crate `gemini-embeddings` centraliza el modelo en la constante
`MODEL`, actualmente `gemini-embedding-001`, y lo reutilizan tanto
`supabase-store` para búsqueda semántica como `outbox-worker` para
materializar embeddings.

## Tutorial

En [`tutorial/`](tutorial/) — en español, un capítulo por invariante,
con la estructura: problema → invariante → implementación mínima →
versión rota → por qué falla → memoria y asignación → tests → frontera
de producción → principios SOLID en juego → ejercicios.
