# okf-mcp — Servidor MCP de memoria en Rust (núcleo 100 % `std`)

Un servidor de memoria persistente para agentes (protocolo MCP) cuyo
**motor de conocimiento está escrito exclusivamente con la biblioteca
estándar de Rust**: sin frameworks, sin `serde`, sin `tokio` en el
núcleo. Las dependencias solo se permitirán en los adaptadores de
frontera (HTTP, TLS, JWT, Supabase, Vercel) del hito 2.

Este repositorio es a la vez un proyecto real y un **tutorial muy
didáctico** de Rust y de principios SOLID: ver [`tutorial/`](tutorial/).

## Estado

- ✅ **Hito 1:** núcleo `std`-only + servidor MCP por stdio.
- ✅ **Hito 2:** transporte HTTP sin estado (`mcp-http`) + contrato ejecutable `MemoryRepository` + adaptador de Vercel (`vercel-entry`) + adaptador de base de datos PostgreSQL (`supabase-store`).
- ✅ **Hito 3:** OAuth 2.1 (Resource Server, validación criptográfica de JWTs mediante firmas y JWKS).
- ✅ **Hito 4:** Transactional Outbox (`outbox-worker` con procesamiento concurrente `SKIP LOCKED` sincronizando a Git y base de datos vectorial `pgvector`).

## Arquitectura

```text
mcp-stdio    (bin)  E/S por stdin/stdout, presupuesto de lectura
mcp-http     (bin)  HTTP/1.1 sin estado sobre TcpListener, POST /mcp
vercel-entry (bin)  ADAPTADOR: axum + vercel_runtime → route() (mismo código)
   │            │           │
   │            │           └── el único crate con deps externas de producción
   │            └── server.rs   parseo HTTP acotado ↔ route() (puro)
   │
   ├── mcp-core          JSON-RPC 2.0 + ciclo de vida MCP + despacho
   │      └── json-mini  parser/serializador JSON educativo
   │
   ├── memory-tools      MemoryTools (las 4 herramientas MCP, genéricas sobre el trait)
   │
   └── store-core        contrato de datos (trait MemoryRepository + contract.rs de Liskov)
          │
          ├── memory-store   InMemoryStore (implementación en RAM)
          ├── supabase-store SupabaseStore (adaptador de base de datos)
          ├── okf-core       frontmatter YAML (subconjunto) + enlaces [[...]]
          ├── graph-core     BFS acotado (trait NeighborSource)
          ├── conflict-core  decisiones compare-and-swap puras
          ├── hash-core      SHA-256 a mano (vectores NIST)
          └── memory-model   ConceptId, ContentId, Budget, Revision
```

Regla del workspace: **ningún crate declara dependencias externas** en
sus dependencias de producción, con UNA excepción marcada
explícitamente: `crates/vercel-entry` (necesita `tokio` + `axum` +
`vercel_runtime` — no hay forma std-only de arrancar una función
serverless de Vercel; ver `tutorial/11-adaptador-vercel.md`). Su
`Cargo.toml` lleva el marcador `# ADAPTADOR: dependencias externas
permitidas` en la primera línea, y `scripts/check-std-only.sh` lo
reconoce y lo excluye explícitamente del resto de la comprobación.
(La otra excepción, menor, es `json-mini` como *dev-dependency* de
test en `mcp-http`, solo para parsear aserciones.) Todos los crates
llevan `#![forbid(unsafe_code)]`.

## Uso

```bash
cargo test               # toda la suite
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

(La memoria vive en RAM: cada proceso empieza vacío. La persistencia
llega con el adaptador Supabase, todavía por construir.)

Lo mismo por HTTP:

```bash
PORT=8787 cargo run -q -p mcp-http &
curl -s -X POST http://127.0.0.1:8787/mcp -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"memory_commit","arguments":{"concept_id":"people/alice","reason":"alta","markdown":"---\ntype: person\ntitle: Alice\n---\nhola\n"}}}'
```

`ALLOWED_ORIGINS` (lista separada por comas) restringe qué
`Origin` de navegador se acepta; sin configurar, cualquier origin
pasa — aceptable en desarrollo, nunca en producción.

## Las cuatro herramientas

| Herramienta      | Qué hace                                                        |
| ---------------- | --------------------------------------------------------------- |
| `memory_search`  | candidatos compactos (id, hash, tipo, título, tags, URI)         |
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
   crate es el único que sabe hablar con Vercel).
3. Configura la variable de entorno `ALLOWED_ORIGINS` (lista
   separada por comas) antes de servir tráfico real — sin ella,
   cualquier `Origin` de navegador se acepta.
4. La integración de Supabase en el marketplace de Vercel puede
   inyectar las credenciales (`SUPABASE_URL`, claves, cadena de
   conexión) directamente como variables de entorno del proyecto;
   el adaptador que las use todavía está por construir.

[`crates/vercel-entry/vercel.json`](crates/vercel-entry/vercel.json)
reescribe `/mcp` → `/api/mcp` (y el descubrimiento OAuth,
`/.well-known/oauth-protected-resource` → `/api/mcp`) para que la URL
pública sea la que promete el resto de esta documentación. Vive
DENTRO de `crates/vercel-entry`, no en la raíz del repo: como el
**Root Directory** del proyecto está fijado ahí (punto anterior),
Vercel solo lee `vercel.json` relativo a esa carpeta — un
`vercel.json` en la raíz del repo se ignora en silencio.

## Tutorial

En [`tutorial/`](tutorial/) — en español, un capítulo por invariante,
con la estructura: problema → invariante → implementación mínima →
versión rota → por qué falla → memoria y asignación → tests →
frontera de producción → principios SOLID en juego → ejercicios.
