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
│ memory-tools 4 herramientas MCP genéricas sobre MemoryRepository   │
│ store-core   puerto MemoryRepository + contrato Liskov             │
│ memory-model ConceptId, ContentId, Budget, Revision, Principal     │
│ okf-core     frontmatter YAML (subconjunto) + enlaces [[...]]      │
│ graph-core   BFS acotado (trait NeighborSource)                    │
│ conflict-core decisiones compare-and-swap puras                    │
│ hash-core    SHA-256 a mano (vectores NIST)                        │
│ memory-store InMemoryStore para desarrollo y tests                 │
└───────────────┬────────────────────────────────────────────────────┘
                │
                ▼
                    Adaptadores de salida / infraestructura
┌────────────────────────────────────────────────────────────────────┐
│ supabase-store    SupabaseStore implementa MemoryRepository        │
│ outbox-worker     procesa outbox, GitHub y embeddings              │
│ gemini-embeddings cliente del proveedor de embeddings              │
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
  `mcp-core`, `mcp-stdio` y `mcp-http`.
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

## Las cuatro herramientas

| Herramienta      | Qué hace                                                        |
| ---------------- | --------------------------------------------------------------- |
| `memory_search`  | candidatos compactos; filtros literales (`type`, `status`, `path_prefix`, `tags` + `tags_mode` any/all) en AND con la `query` semántica |
| `memory_resolve` | Markdown exacto + vecindario acotado del grafo de `[[enlaces]]`  |
| `memory_commit`  | escritura con compare-and-swap (`expected_hash`)                 |
| `memory_history` | revisiones de más reciente a más antigua, paginadas              |

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
