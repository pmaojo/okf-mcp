# Tutorial — Construye un servidor MCP de memoria en Rust puro (`std`)

Un tutorial muy didáctico de Rust **y** de principios SOLID,
construyendo un sistema real: el servidor de memoria de este
repositorio. Cada capítulo sigue la misma plantilla:

> problema → invariante → implementación mínima → versión rota →
> por qué falla → memoria y asignación → tests → frontera de
> producción → **principios SOLID en juego** → ejercicios

## Hito 1 — El núcleo (este código)

| # | Capítulo | Rust | SOLID protagonista |
|---|----------|------|--------------------|
| 0 | [Introducción](00-introduccion.md) | el mapa del proyecto | visión de conjunto |
| 1 | [Identificadores seguros](01-identificadores-seguros.md) | newtypes, validación por lista blanca | S |
| 2 | [SHA-256: la identidad es el contenido](02-sha256-identidad.md) | wrapping arithmetic, estado incremental | S, cuándo NO abstraer (L) |
| 3 | [JSON a mano](03-json-a-mano.md) | enums recursivos, lifetimes, límites | S, deuda D documentada |
| 4 | [Frontmatter OKF](04-frontmatter-okf.md) | borrowing, offsets, máquinas de estados | S, O |
| 5 | [El grafo acotado](05-grafo-acotado.md) | traits, genéricos, `VecDeque` | **I**, D |
| 6 | [Compare-and-swap](06-conflictos-cas.md) | funciones puras, match exhaustivo | S, **O** |
| 7 | [El repositorio](07-repositorio.md) | ownership, `Arc<str>`, `&mut` como mutex | **L**, D |
| 8 | [El protocolo MCP](08-protocolo-mcp.md) | inyección por traits, E/S acotada | **D**, todo junto |

## Hito 2 — hacia la red (en curso)

| # | Capítulo | Rust | SOLID protagonista |
|---|----------|------|--------------------|
| 9 | [El contrato ejecutable](09-contrato-liskov.md) | traits genéricos como ley, fábricas `FnMut() -> R` | **L**, hecho literal |
| 10 | [HTTP sin estado](10-http-sin-estado.md) | parseo HTTP acotado, `TcpListener`, DNS-rebinding | S, D |
| 11 | [El adaptador de Vercel](11-adaptador-vercel.md) | `axum` extractors, la frontera hecha código | **D**, el arco cerrado |

Falta por escribir (necesita tus credenciales de Supabase): un
adaptador `SupabaseStore` que pase el contrato del capítulo 9 contra
Postgres real, y el outbox transaccional.

## Hitos siguientes (capítulos por escribir)

- **Hito 3:** OAuth 2.1 — el servidor MCP como *resource server*
  (JWKS, validación de JWT, audiencias RFC 8707, el token del
  cliente JAMÁS se reenvía a Supabase).
- **Hito 4:** worker de sincronización a Git y búsqueda semántica
  (pgvector), como índices derivados de los blobs inmutables.

## Cómo seguirlo

```bash
cargo test                      # todo el hito 1 debe estar en verde
./scripts/check-std-only.sh     # cero dependencias, cero unsafe
cargo run -p mcp-stdio          # el servidor, listo para un cliente MCP
```

Lee cada capítulo con el crate abierto al lado. Los ejercicios van
de guiados a abiertos; los abiertos no tienen solución única y esa
es la gracia.
