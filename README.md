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
- 🟨 **Hito 2 (en curso):** transporte HTTP sin estado (`mcp-http`,
  local con `TcpListener`, mismo contrato que usará Vercel) +
  contrato ejecutable `MemoryRepository` para el futuro adaptador
  Supabase. Falta: el adaptador Supabase real y el despliegue en
  Vercel (requieren tus credenciales; ver `tutorial/09-http-sin-estado.md`).
- ⬜ Hito 3: OAuth 2.1 (resource server, JWKS, RFC 8707).
- ⬜ Hito 4: outbox → Git y embeddings (pgvector).

## Arquitectura

```text
mcp-stdio (bin)          E/S por stdin/stdout, presupuesto de lectura
mcp-http  (bin)          HTTP/1.1 sin estado sobre TcpListener, POST /mcp
   │           │
   │           └── server.rs   parseo HTTP acotado ↔ route() (puro)
   │
   ├── mcp-core          JSON-RPC 2.0 + ciclo de vida MCP + despacho
   │      └── json-mini  parser/serializador JSON educativo
   │
   └── MemoryTools       las 4 herramientas, genéricas sobre el trait
          │
          └── memory-store   blobs inmutables + cabezas + revisiones
                 │      └── contract.rs   contrato ejecutable de MemoryRepository
                 ├── okf-core        frontmatter YAML (subconjunto) + enlaces [[...]]
                 ├── graph-core      BFS acotado (trait NeighborSource)
                 ├── conflict-core   decisiones compare-and-swap puras
                 ├── hash-core       SHA-256 a mano (vectores NIST)
                 └── memory-model    ConceptId, ContentId, Budget, Revision
```

Regla del workspace: **ningún crate declara dependencias externas** en
sus dependencias de producción. Todos los `Cargo.toml` solo
referencian crates internos por `path` (la única excepción es
`json-mini` como *dev-dependency* de test en `mcp-http`, para parsear
aserciones — nunca se usa en el código servido). Todos los crates
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

## Tutorial

En [`tutorial/`](tutorial/) — en español, un capítulo por invariante,
con la estructura: problema → invariante → implementación mínima →
versión rota → por qué falla → memoria y asignación → tests →
frontera de producción → principios SOLID en juego → ejercicios.
