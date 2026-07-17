# Capítulo 0 — Introducción: un servidor de memoria como escuela de Rust y de SOLID

## Qué vamos a construir

Un **servidor MCP de memoria persistente** para agentes de IA. Un
agente (Claude, Cursor, el que sea) se conecta y dispone de cuatro
herramientas:

- `memory_search` — buscar conceptos.
- `memory_resolve` — leer un concepto y su vecindario en el grafo.
- `memory_commit` — escribir con control de concurrencia.
- `memory_history` — consultar revisiones.

Los documentos son Markdown con frontmatter YAML (formato OKF):

```markdown
---
type: person
title: Alice García
tags:
  - engineering
  - rust
---

Alice trabaja en [[projects/okf-mcp]] junto a [[people/bob]].
```

Los enlaces `[[...]]` forman un **grafo de conocimiento** que el
servidor puede recorrer.

## La restricción que lo cambia todo

> **El motor de conocimiento usa exclusivamente la biblioteca
> estándar de Rust.** Ni `serde`, ni `tokio`, ni regex, ni un crate
> de YAML. En el hito 1, ni siquiera los adaptadores tienen
> dependencias: el JSON y el SHA-256 están escritos a mano.

¿Es esto lo que haría un equipo profesional? Para el JSON del
endpoint público, no: usaría `serde_json`. Y ese es exactamente el
segundo aprendizaje del proyecto:

> **Las dependencias se permiten en las fronteras del sistema;
> la lógica de negocio permanece portable, testeable y `std`-only.**

Escribir el parser de JSON a mano te enseña qué hace `serde_json`
por ti. Mantenerlo fuera del núcleo te enseña arquitectura. Las dos
lecciones importan, y este proyecto está diseñado para que no se
estorben: cuando en el hito 2 llegue el endpoint HTTPS de Vercel,
`serde_json`, `tokio` y la criptografía auditada vivirán en crates
adaptadores, y ni una línea del motor cambiará.

## El mapa del workspace

```text
crates/
  memory-model/    los sustantivos: ConceptId, ContentId, Budget, Revision
  hash-core/       SHA-256 a mano, verificado contra vectores NIST
  json-mini/       JSON a mano: parser con offsets de error y límites
  okf-core/        frontmatter YAML (subconjunto) + escáner de [[enlaces]]
  graph-core/      BFS acotado sobre un trait de adyacencia
  conflict-core/   decisiones compare-and-swap puras
  store-core/      trait MemoryRepository + contract.rs de Liskov
  memory-store/    InMemoryStore (implementación en RAM)
  memory-tools/    MemoryTools (las 4 herramientas MCP, genéricas sobre el trait)
  mcp-core/        JSON-RPC 2.0 + ciclo de vida MCP + despacho
  mcp-stdio/       transporte stdin/stdout
```

Dos reglas se cumplen en TODOS los crates y los tests lo verifican:

1. `Cargo.toml` sin dependencias externas (solo `path` internos).
2. `#![forbid(unsafe_code)]` en la primera línea de cada `lib.rs`.

## SOLID sin diapositivas

Este proyecto no "menciona" SOLID: lo necesita para existir. Cada
principio aparece porque un problema real lo exige, y cada capítulo
tiene una sección **«Principios SOLID en juego»** que señala dónde.
El resumen anticipado:

| Principio | Dónde lo verás | El problema que resuelve |
| --------- | -------------- | ------------------------ |
| **S** — Responsabilidad única | un crate = una razón de cambio | `hash-core` no sabe qué es un documento; `conflict-core` no sabe almacenar. Cuando cambie el formato OKF, solo recompila `okf-core`. |
| **O** — Abierto/cerrado | `ToolHandler`, `CommitDecision` | añadir la quinta herramienta MCP no toca el despachador; añadir merge a tres bandas no toca el almacén. |
| **L** — Sustitución de Liskov | `MemoryRepository` | el almacén en RAM (hito 1) y el de Supabase (hito 2) deben ser indistinguibles: mismos errores, mismas garantías CAS. Los tests del contrato se escriben una vez. |
| **I** — Segregación de interfaces | `NeighborSource` | el algoritmo de grafo pide `neighbors()` y `document_size()`. Nada más. Un test lo implementa con un `BTreeMap` en tres líneas. |
| **D** — Inversión de dependencias | todo el workspace | el núcleo define los traits; los adaptadores los implementan. La flecha de dependencia SIEMPRE apunta hacia el dominio. |

Fíjate en el detalle de la última fila, porque es la esencia del
proyecto: `graph-core` **no depende** de `memory-store`. Es
`memory-store` quien implementa el trait que `graph-core` define.
Cuando llegue Postgres, el algoritmo de BFS no se enterará.

## Los temas de Rust, por capítulo

| Capítulo | Construyes | Rust que aprendes |
| -------- | ---------- | ----------------- |
| 1 | identificadores seguros (`ConceptId`) | newtypes, validación por lista blanca, `Result` |
| 2 | SHA-256 (`hash-core`) | arrays, wrapping arithmetic, slices, estado incremental |
| 3 | JSON a mano (`json-mini`) | enums recursivos, `BTreeMap`, parsing con offsets, UTF-8 |
| 4 | frontmatter OKF (`okf-core`) | borrowing, `split_inclusive`, errores con línea |
| 5 | grafo acotado (`graph-core`) | traits, genéricos, `VecDeque`, presupuestos |
| 6 | CAS (`conflict-core`) | funciones puras, pattern matching exhaustivo |
| 7 | repositorio (`store-core` + `memory-store`) | ownership, `Arc<str>`, invariantes de almacén y contratos |
| 8 | protocolo (`mcp-core` + `memory-tools` + `mcp-stdio`) | inyección por traits, E/S acotada, integración y desacoplamiento de transportes |

## Cómo leer cada capítulo

Todos siguen la misma plantilla, pensada para que NO copies código
sin entenderlo:

1. **El problema** — qué rompe si no lo resolvemos.
2. **El invariante** — la frase que el código debe hacer verdadera.
3. **La implementación mínima** — el código real del repo, explicado.
4. **Una versión deliberadamente rota** — el error que habrías escrito.
5. **Por qué falla** — con el input concreto que la rompe.
6. **Memoria y asignación** — qué se asigna, cuándo y con qué tope.
7. **Tests** — los del repo, y qué invariante vigila cada uno.
8. **Frontera de producción** — qué haría un equipo con dependencias.
9. **Principios SOLID en juego** — nombrados sobre el código, no en abstracto.
10. **Ejercicios** — del más guiado al más abierto.

## Requisitos

- Rust estable (`rustup`), edición 2021.
- Ninguna cuenta de nada: el hito 1 corre entero en tu máquina.

```bash
cargo test              # si esto pasa, tienes todo lo necesario
cargo run -p mcp-stdio  # y esto es el servidor completo
```

Siguiente: [Capítulo 0.5 — Fundamentos de Rust: Lo mínimo para no perderse](00a-fundamentos-rust.md).
