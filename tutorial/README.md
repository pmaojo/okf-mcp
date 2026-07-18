# Tutorial — Construye un servidor MCP de memoria en Rust puro (`std`)

Un tutorial de Rust **y** de principios SOLID,
construyendo un sistema real: el servidor de memoria de este
repositorio. Cada capítulo sigue la misma plantilla:

> problema → invariante → implementación mínima → versión rota →
> por qué falla → memoria y asignación → tests → frontera de
> producción → **principios SOLID en juego** → ejercicios

## Hito 1 — El núcleo (este código)

| # | Capítulo | Rust | SOLID protagonista |
|---|----------|------|--------------------|
| 0 | [Introducción](00-introduccion.md) | el mapa del proyecto | visión de conjunto |
| 0.5 | [Fundamentos de Rust](00a-fundamentos-rust.md) | sintaxis y conceptos clave | preparación |
| 1 | [Identificadores seguros](01-identificadores-seguros.md) | newtypes, validación por lista blanca | S |
| 2 | [SHA-256: la identidad es el contenido](02-sha256-identidad.md) | wrapping arithmetic, estado incremental | S, cuándo NO abstraer (L) |
| 3 | [JSON a mano](03-json-a-mano.md) | enums recursivos, lifetimes, límites | S, deuda D documentada |
| 4 | [Frontmatter OKF](04-frontmatter-okf.md) | borrowing, offsets, máquinas de estados | S, O |
| 5 | [El grafo acotado](05-grafo-acotado.md) | traits, genéricos, `VecDeque` | **I**, D |
| 6 | [Compare-and-swap](06-conflictos-cas.md) | funciones puras, match exhaustivo | S, **O** |
| 7 | [El repositorio](07-repositorio.md) | ownership, `Arc<str>`, `&mut` como mutex | **L**, D |
| 8 | [El protocolo MCP](08-protocolo-mcp.md) | inyección por traits, E/S acotada | **D**, todo junto |

## Hito 2 — hacia la red (Persistencia relacional)

| # | Capítulo | Rust | SOLID protagonista |
|---|----------|------|--------------------|
| 9 | [El contrato ejecutable](09-contrato-liskov.md) | traits genéricos como ley, fábricas `FnMut() -> R` | **L**, hecho literal |
| 10 | [HTTP sin estado](10-http-sin-estado.md) | parseo HTTP acotado, `TcpListener`, DNS-rebinding | S, D |
| 11 | [El adaptador de Vercel](11-adaptador-vercel.md) | `axum` extractors, la frontera hecha código | **D**, el arco cerrado |
| 12 | [El adaptador de Supabase](12-adaptador-supabase.md) | `sqlx`, CAS relacional, `FOR UPDATE` | **L**, S, I |

## Hito 3 — OAuth 2.1 (Autenticación sin estado)

| # | Capítulo | Rust | SOLID protagonista |
|---|----------|------|--------------------|
| 13 | [OAuth 2.1 Resource Server](13-oauth2-resource-server.md) | firmas JWT, JWKS caching asíncrono | S, D, O |

## Hito 4 — Eventual Consistency (Integraciones asíncronas)

| # | Capítulo | Rust | SOLID protagonista |
|---|----------|------|--------------------|
| 14 | [Outbox transaccional y pgvector](14-outbox-sincronizacion.md) | outbox pattern, `SKIP LOCKED`, pgvector | S, D |

## Hito 5 — MCP Apps (Visualizaciones interactivas, opcional)

| # | Capítulo | Rust | SOLID protagonista |
|---|----------|------|--------------------|
| 15 | [MCP Apps: grafo e historial](15-mcp-apps-visualizaciones.md) (opcional — extensión negociable, no forma parte del núcleo) | métodos de trait con cuerpo por defecto, `&'static str` embebido | **O**, I, D |

## Hito 6 — Diagnóstico y Mantenimiento (Observabilidad)

| # | Capítulo | Rust | SOLID protagonista |
|---|----------|------|--------------------|
| 17 | [Mantenimiento y Observabilidad](17-mantenimiento-observabilidad.md) | segregación de interfaces, consultas agregadas de base de datos | **I**, S |

## Transversal — La documentación ejecutable

| # | Capítulo | Rust | SOLID protagonista |
|---|----------|------|--------------------|
| 16 | [`cargo doc`: la documentación que compila](16-cargo-doc.md) | doctests, enlaces intra-doc, `missing_docs`, `compile_fail` | **L** como página del trait, S por crate |

## La rueda de serie

Reinventamos ruedas para entenderlas, no para desplegarlas. Cada
capítulo señala en su "Frontera de producción" el crate idóneo
(🧰), y el mapa completo — con el criterio para elegir dependencias
— vive en [La rueda de serie](la-rueda-de-serie.md).

## La referencia navegable

La API completa del workspace, generada con `cargo doc` y publicada
en cada push a `main`:

> **<https://pmaojo.github.io/okf-mcp/>**

Es la otra mitad de este tutorial: cada capítulo cuenta el PORQUÉ
con el código al lado; la referencia muestra el QUÉ, con búsqueda
(tecla `s`) y enlaces entre tipos. Los ejemplos que ves ahí no son
decoración — compilan y se ejecutan en cada `cargo test` (capítulo
16). Para generarla en local, tripas privadas incluidas:

```bash
cargo doc --no-deps --document-private-items --open
```

## Cómo seguirlo

```bash
cargo test                      # todo el hito 1 debe estar en verde
./scripts/check-std-only.sh     # cero dependencias, cero unsafe
./scripts/check-docs.sh         # docs sin warnings, doctests en verde
cargo doc --no-deps --open      # la referencia, en tu navegador
cargo run -p mcp-stdio          # el servidor, listo para un cliente MCP
```

Lee cada capítulo con el crate abierto al lado — y con su página de
la referencia en otra pestaña. Los ejercicios van de guiados a
abiertos; los abiertos no tienen solución única y esa es la gracia.
