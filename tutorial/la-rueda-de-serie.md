# La rueda de serie — qué usar cuando no estás aprendiendo

Este proyecto reinventa ruedas A PROPÓSITO: escribir un SHA-256, un
parser JSON o un servidor HTTP a mano es la forma más honesta de
entender qué hacen `sha2`, `serde_json` y `axum` por ti. Pero la
regla tiene dos mitades, y la segunda importa igual:

> Reinventa para entender. Despliega la rueda de serie.

Esta página es la segunda mitad: para cada pieza que construimos a
mano, el crate (o los crates) que usarías en un proyecto real, y el
criterio para elegir. Cada capítulo enlaza aquí desde su sección
"Frontera de producción".

## El mapa

| Lo que construimos | Capítulo | La rueda de serie | Notas |
|---|---|---|---|
| Newtypes validados (`ConceptId`) | 1 | [`nutype`](https://docs.rs/nutype), [`validator`](https://docs.rs/validator) | El patrón newtype es idiomático a mano; estos crates quitan el boilerplate cuando tienes docenas. |
| SHA-256 (`hash-core`) | 2 | [`sha2`](https://docs.rs/sha2) (RustCrypto), [`blake3`](https://docs.rs/blake3) | `sha2` es el estándar auditado con SIMD; `blake3` si eliges tú el algoritmo y quieres velocidad. Para AUTENTICACIÓN, siempre auditado: jamás un port didáctico. |
| Parser/serializador JSON (`json-mini`) | 3 | [`serde`](https://docs.rs/serde) + [`serde_json`](https://docs.rs/serde_json) | El ecosistema entero habla serde. `#[derive(Serialize, Deserialize)]` sustituye TODO el capítulo 3. |
| Frontmatter YAML + enlaces (`okf-core`) | 4 | [`gray_matter`](https://docs.rs/gray_matter), [`pulldown-cmark`](https://docs.rs/pulldown-cmark) | Ojo: `serde_yaml` está archivado desde 2024; para YAML general mira sus sucesores mantenidos (`serde_yaml_ng`, `saphyr`). Para escanear Markdown de verdad (enlaces, code fences), `pulldown-cmark`. |
| BFS acotado (`graph-core`) | 5 | [`petgraph`](https://docs.rs/petgraph) | Grafos genéricos con todos los algoritmos clásicos. Nuestra versión existe porque el ACOTADO por presupuesto es la lección. |
| Decisiones CAS (`conflict-core`) | 6 | — (y eso es la lección) | Hay ruedas que son 50 líneas puras: envolverlas en una dependencia sería peor. Para el merge a tres bandas futuro: [`similar`](https://docs.rs/similar), [`diffy`](https://docs.rs/diffy). |
| Repositorio en RAM (`memory-store`) | 7 | [`sqlx`](https://docs.rs/sqlx), [`diesel`](https://docs.rs/diesel), [`sea-orm`](https://docs.rs/sea-orm) | La versión de producción de este repo YA usa `sqlx` (hito 2, `supabase-store`): mira cómo el contrato del capítulo 9 hizo el cambio indoloro. |
| JSON-RPC + ciclo MCP (`mcp-core`) | 8 | [`rmcp`](https://docs.rs/rmcp) (SDK oficial de MCP en Rust), [`jsonrpsee`](https://docs.rs/jsonrpsee) | `rmcp` te da protocolo, transportes y macros de herramientas. Nuestro crate existe para que sepas qué hay debajo cuando `rmcp` haga algo raro. |
| Tests de contrato (`store-core::contract`) | 9 | [`proptest`](https://docs.rs/proptest), [`quickcheck`](https://docs.rs/quickcheck) | El siguiente nivel del contrato ejecutable: propiedades con entradas generadas, no elegidas. |
| Servidor HTTP/1.1 (`mcp-http`) | 10 | [`axum`](https://docs.rs/axum) + [`tower`](https://docs.rs/tower) + [`hyper`](https://docs.rs/hyper), sobre [`tokio`](https://docs.rs/tokio) | La pila estándar. El repo ya la usa donde toca: `vercel-entry` (hito 2). Cliente HTTP: [`reqwest`](https://docs.rs/reqwest) (async) o [`ureq`](https://docs.rs/ureq) (sync). |
| Validación JWT + JWKS (hito 3) | 13 | [`jsonwebtoken`](https://docs.rs/jsonwebtoken), [`oauth2`](https://docs.rs/oauth2) | Firmas RSA/ECDSA con criptografía auditada debajo. Esta frontera NUNCA se cruza a mano (capítulo 2, regla de oro). |
| Outbox + worker (hito 4) | 14 | [`apalis`](https://docs.rs/apalis), [`tokio-cron-scheduler`](https://docs.rs/tokio-cron-scheduler) | El patrón outbox no necesita crate (es SQL + disciplina), pero un framework de jobs te da reintentos, backoff y métricas. |
| HTML embebido MCP Apps (hito 5) | 15 | [`maud`](https://docs.rs/maud), [`askama`](https://docs.rs/askama) | Plantillas HTML tipadas y verificadas en compilación, en lugar de `&'static str`. |

## Las que usarías en casi cualquier proyecto

No corresponden a un capítulo: corresponden a todos.

| Necesidad | Crate | Por qué |
|---|---|---|
| Errores de biblioteca | [`thiserror`](https://docs.rs/thiserror) | Deriva los `impl Display`/`Error` que en este tutorial escribimos a mano una y otra vez (esa repetición era deliberada: ahora sabes exactamente qué te ahorra). |
| Errores de aplicación | [`anyhow`](https://docs.rs/anyhow) | Un tipo de error universal con contexto para binarios, donde no necesitas que el llamante distinga variantes. |
| Logs estructurados | [`tracing`](https://docs.rs/tracing) + [`tracing-subscriber`](https://docs.rs/tracing-subscriber) | Spans con contexto, no `println!`. Imprescindible en serverless. |
| Argumentos de CLI | [`clap`](https://docs.rs/clap) | Derive de structs a flags, con ayuda generada. |
| Snapshots de test | [`insta`](https://docs.rs/insta) | Compara salidas grandes (JSON, HTML) contra archivos versionados. |
| Benchmarks | [`criterion`](https://docs.rs/criterion) | Para el "medir contra `sha2` y decidir con números" del capítulo 2. |

## Cómo se elige una rueda (criterio, no lista)

Las listas caducan; el criterio no. Antes de añadir una dependencia:

1. **Mantenimiento.** ¿Última release, issues atendidas, más de un
   mantenedor? Un crate brillante y abandonado es deuda
   (el caso `serde_yaml`, archivado en 2024, es la lección canónica).
2. **Seguridad.** `cargo audit` consulta la base RUSTSEC;
   `cargo deny` te deja vetar licencias y duplicados en CI. Ambos
   son un paso de workflow, como nuestro `check-std-only.sh`.
3. **Peso transitivo.** `cargo tree` antes y después: cada
   dependencia trae las suyas, y el binario de Vercel del hito 2
   paga cada una en arranque en frío.
4. **La prueba del capítulo 16:** ¿su documentación tiene ejemplos
   que compilan? Un crate sin doctests te está contando algo.
5. **Reversibilidad.** ¿Puedes esconderla detrás de un trait tuyo
   (capítulos 5, 8, 9)? Si la respuesta es sí, equivocarse es
   barato. Ese es el motivo profundo de la regla D de SOLID en este
   repo: las dependencias se inyectan para poder despedirlas.

Y el orden de este libro en una línea: **primero la rueda a mano en
`std` (entender), después la de serie detrás de un trait (producir),
nunca la de serie sin saber qué esconde (rezar).**
