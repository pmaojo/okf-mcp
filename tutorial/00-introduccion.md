# Capítulo 0 — Lo que vamos a construir (y por qué a mano)

Los agentes de IA tienen un defecto de fábrica: amnesia. Cada
conversación empieza de cero. Claude, Cursor, el agente que sea,
puede razonar brillantemente durante una sesión y olvidarlo todo al
cerrarla. La industria tiene un protocolo para remediarlo — MCP, el
Model Context Protocol — que permite conectarle herramientas a un
modelo. Lo que va a ocupar este libro es construir una de esas
herramientas desde el primer byte: un **servidor de memoria
persistente**, donde un agente guarda lo que sabe, lo consulta, lo
corrige y lo enlaza.

Cuando terminemos, un agente conectado a tu servidor dispondrá de
cuatro verbos:

- `memory_search` — buscar conceptos.
- `memory_resolve` — leer un concepto y su vecindario en el grafo.
- `memory_commit` — escribir con control de concurrencia.
- `memory_history` — consultar revisiones.

Y la memoria en sí tendrá una forma deliberadamente humilde:
documentos Markdown con un encabezado YAML (el formato OKF):

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

Fíjate en los dobles corchetes. Cada `[[...]]` es un enlace a otro
concepto, y el conjunto forma un **grafo de conocimiento** que el
servidor sabrá recorrer. Documentos planos, enlaces explícitos, y
encima un protocolo estándar: nada exótico. Lo exótico va a ser el
CÓMO.

## La restricción que lo cambia todo

Aquí está la decisión que define este libro, y conviene que la leas
dos veces:

> **El motor de conocimiento usa exclusivamente la biblioteca
> estándar de Rust.** Ni `serde`, ni `tokio`, ni regex, ni un crate
> de YAML. En el hito 1, ni siquiera los adaptadores tienen
> dependencias: el JSON y el SHA-256 están escritos a mano.

¿Es esto lo que haría un equipo profesional? Para el JSON de un
endpoint público, desde luego que no: usaría `serde_json` y a otra
cosa. Y ese es exactamente el segundo aprendizaje del proyecto,
tan importante como el primero:

> **Las dependencias se permiten en las fronteras del sistema; la
> lógica de negocio permanece portable, testeable y `std`-only.**

Escribir el parser de JSON a mano te enseña qué hace `serde_json`
por ti. Mantenerlo fuera del núcleo te enseña arquitectura. Las dos
lecciones importan, y el proyecto está diseñado para que no se
estorben: cuando en el hito 2 llegue el endpoint HTTPS de Vercel,
`serde_json`, `tokio` y la criptografía auditada vivirán en crates
adaptadores, y ni una línea del motor cambiará. Cada capítulo cierra
señalando la rueda de serie — el crate que usarías en producción —
y el mapa completo vive en [La rueda de serie](la-rueda-de-serie.md).
Reinventamos para entender; desplegamos la de serie.

## El mapa del territorio

Un workspace de Cargo es una federación de crates (paquetes) que
comparten un `Cargo.lock` y un directorio `target`. El nuestro
empieza así — no intentes memorizarlo, vas a construir cada pieza:

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

Dos reglas se cumplen en TODOS los crates, y no por disciplina sino
porque un script las verifica (`./scripts/check-std-only.sh`, y CI
lo ejecuta en cada push):

1. `Cargo.toml` sin dependencias externas (solo `path` internos).
2. `#![forbid(unsafe_code)]` en la primera línea de cada `lib.rs`.

## SOLID sin diapositivas

Este proyecto no "menciona" SOLID: lo necesita para existir. Cada
principio va a aparecer porque un problema real lo exige, y el
apéndice de cada capítulo señala dónde. El anticipo, para que
reconozcas las escenas cuando lleguen:

| Principio | Dónde lo verás | El problema que resuelve |
| --------- | -------------- | ------------------------ |
| **S** — Responsabilidad única | un crate = una razón de cambio | `hash-core` no sabe qué es un documento; `conflict-core` no sabe almacenar. Cuando cambie el formato OKF, solo recompila `okf-core`. |
| **O** — Abierto/cerrado | `ToolHandler`, `CommitDecision` | añadir la quinta herramienta MCP no toca el despachador; añadir merge a tres bandas no toca el almacén. |
| **L** — Sustitución de Liskov | `MemoryRepository` | el almacén en RAM (hito 1) y el de Supabase (hito 2) deben ser indistinguibles: mismos errores, mismas garantías CAS. Los tests del contrato se escriben una vez. |
| **I** — Segregación de interfaces | `NeighborSource` | el algoritmo de grafo pide `neighbors()` y `document_size()`. Nada más. Un test lo implementa con un `BTreeMap` en tres líneas. |
| **D** — Inversión de dependencias | todo el workspace | el núcleo define los traits; los adaptadores los implementan. La flecha de dependencia SIEMPRE apunta hacia el dominio. |

Quédate con el detalle de la última fila, porque es la esencia del
proyecto: `graph-core` **no depende** de `memory-store`. Es
`memory-store` quien implementa el trait que `graph-core` define.
Cuando llegue Postgres, el algoritmo de BFS no se enterará.

## El Rust que vas a aprender, por capítulo

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

## Cómo leer este libro

Cada capítulo es una historia con la misma columna vertebral,
aunque no siempre con los mismos huesos a la vista: un problema que
rompe algo si no se resuelve, el invariante que el código debe hacer
verdadero, la implementación real del repositorio, y — esta es la
parte que más vas a agradecer — **la versión rota**: el error que
habrías escrito tú (o que escribimos nosotros, con su depuración
incluida) y el input concreto que lo delata.

Al final de cada capítulo encontrarás un apéndice con el material de
referencia: qué asigna memoria y con qué tope, qué principios SOLID
estaban en juego, y ejercicios que van del más guiado al más
abierto (los abiertos no tienen solución única, y esa es la gracia).
Léelo con el crate abierto al lado y con la
[referencia de API publicada](https://pmaojo.github.io/okf-mcp/) en
otra pestaña: cada afirmación de este libro sobre el código se
verifica en cada build (capítulo 16 — la documentación aquí
compila).

## Requisitos

- Rust estable (`rustup`), edición 2021.
- Ninguna cuenta de nada: el hito 1 corre entero en tu máquina.

```bash
cargo test              # si esto pasa, tienes todo lo necesario
cargo run -p mcp-stdio  # y esto es el servidor completo
```

Siguiente: [Capítulo 0.5 — Fundamentos de Rust: Lo mínimo para no perderse](00a-fundamentos-rust.md).
