# Capítulo 5 — El grafo acotado: BFS con presupuesto

Crate: [`crates/graph-core`](../crates/graph-core/src/lib.rs) ·
[referencia](https://pmaojo.github.io/okf-mcp/graph_core/)

El capítulo anterior terminó con los enlaces `[[...]]` extraídos y
validados. Ahora hay que usarlos: `memory_resolve` no devuelve solo
un documento, devuelve su **vecindario** — los conceptos alcanzables
siguiendo enlaces. Eso es un recorrido de grafo, un algoritmo que
probablemente ya escribiste alguna vez en un ejercicio de clase.

El problema es que los grafos de conocimiento reales no son los de
los ejercicios. Tienen dos propiedades incómodas. La primera:
**ciclos**. `alice → proyecto → alice` es lo normal, no la
excepción, y un recorrido ingenuo no termina jamás. La segunda:
**explosión**. Un concepto "hub" — una etiqueta popular, una persona
central — alcanza miles de nodos en tres saltos.

Y el escenario donde va a correr esto agrava ambas: un proceso
serverless con memoria y tiempo tasados. Ahí, "cargo el grafo entero
y luego decido" no es una opción de diseño: es la definición de un
incidente. De esa presión nace el invariante del capítulo:

> **El recorrido jamás excede su presupuesto — nodos, profundidad,
> bytes — y SIEMPRE informa de si truncó y por qué.**

La segunda mitad es tan importante como la primera, y es fácil
pasarla por alto. Un agente que recibe 128 nodos necesita saber si
son TODOS los vecinos o los primeros 128: en el primer caso puede
razonar "no hay más relaciones"; en el segundo debe pedir más o
refinar. Por eso el resultado (`Traversal`) lleva tres flags
separados — `truncated_by_nodes`, `by_depth`, `by_bytes` — en lugar
de un booleano genérico: cada uno sugiere una acción distinta a
quien lo lee.

## Dos métodos y ni uno más

Antes del algoritmo, la decisión de diseño que hace especial a este
crate. ¿De dónde saca el BFS los vecinos? La respuesta cómoda sería
"del almacén": recibir un `&InMemoryStore` y listo. La respuesta de
`graph-core` es no conocer al almacén en absoluto:

```rust
pub trait NeighborSource {
    type Error;
    fn neighbors(&self, id: &ConceptId) -> Result<Vec<ConceptId>, Self::Error>;
    fn document_size(&self, id: &ConceptId) -> Result<Option<usize>, Self::Error>;
}
```

`graph-core` **no importa** `memory-store`. Pide exactamente dos
cosas y no le importa si detrás hay un `BTreeMap` de test, el
almacén en RAM o una consulta a Postgres. El `type Error` asociado
deja que cada backend traiga su propio error: el de test usa
`Infallible` (un enum sin variantes: el compilador SABE que no puede
fallar) y el algoritmo lo propaga con `?` sin enterarse.

Guárdate esta escena para el apéndice SOLID: la interfaz pequeña no
es minimalismo estético. Si el grafo recibiera el almacén entero,
alguien acabaría llamando a `commit` desde dentro de un recorrido.
Dos métodos significan que esa capacidad NO EXISTE en este contexto.

El BFS en sí es el clásico, con los tres cortes injertados:

```rust
let mut visited_set: BTreeSet<ConceptId> = BTreeSet::new();
let mut queue: VecDeque<(ConceptId, u8)> = VecDeque::new();

visited_set.insert(start.clone());
queue.push_back((start.clone(), 0));

while let Some((id, depth)) = queue.pop_front() {
    if order.len() >= budget.max_graph_nodes { /* corte 1 */ }
    // acumular bytes → corte 3
    if depth >= budget.max_graph_depth { /* corte 2 */ continue; }
    for neighbor in source.neighbors(&id)? {
        if visited_set.insert(neighbor.clone()) {
            queue.push_back((neighbor, depth + 1));
        }
    }
}
```

Cuatro detalles del código real merecen que te pares:

- **`VecDeque`** es la cola FIFO de `std` (un ring buffer). Usar
  `Vec::remove(0)` sería O(n) por extracción; `pop_front` es O(1).
- **`visited_set.insert` devuelve `bool`** — `false` si ya estaba.
  Ese booleano ES la protección contra ciclos, en una línea.
- **El marcado ocurre al ENCOLAR, no al desencolar.** Si marcáramos
  al procesar, un rombo (`a→b`, `a→c`, `b→d`, `c→d`) encolaría `d`
  dos veces. No es incorrecto (se filtraría después) pero infla la
  cola — y la cola también es memoria.
- **Los enlaces rotos son datos, no errores**: `document_size`
  devuelve `Option`, y un `None` produce `Visited { exists: false }`.
  En una base de conocimiento viva siempre hay enlaces a conceptos
  que aún no se escribieron; el agente los ve y puede decidir
  crearlos.

## La versión de la entrevista de trabajo

Así resolvería el recorrido un candidato con prisa, y así estuvo a
punto de resolverse en más de un sistema real:

```rust
// ❌ NO HACER: recursión + "ya limitamos la profundidad"
fn explora(source: &S, id: &ConceptId, depth: u8, out: &mut Vec<ConceptId>) {
    if depth > 4 { return; }
    out.push(id.clone());
    for n in source.neighbors(id).unwrap() {
        explora(source, &n, depth + 1, out);
    }
}
```

"Limité la profundidad, estoy a salvo." Cuenta conmigo los cortes
que faltan.

**Sin `visited`:** el ciclo `a→b→a` con límite de profundidad 4 no
cuelga… pero visita `a` ocho veces. Con profundidad 10 y un grafo
denso, la duplicación es exponencial: 4 saltos con factor de
ramificación 20 son hasta 160 000 visitas para quizá 200 nodos
únicos.

**Sin límite de nodos:** la profundidad NO acota el trabajo. Un hub
con 5 000 vecinos directos revienta el presupuesto en el nivel 1,
a profundidad reglamentaria.

**DFS en vez de BFS:** la recursión explora "rama profunda primero".
Para memoria de agentes, los vecinos CERCANOS son los relevantes;
BFS los da primero, y el truncado por nodos conserva exactamente los
más próximos — que es lo que quieres conservar.

**El `.unwrap()`:** sobre Postgres, un timeout de red mata el
proceso entero en vez de devolver un error al cliente.

Cuatro bugs en seis líneas, y ninguno aparece en una demo pequeña.
Todos aparecen en producción.

## El arnés de ocho líneas

¿Y cómo se testea un algoritmo de grafos? Aquí es donde la interfaz
de dos métodos paga el alquiler:

```rust
struct MapGraph {
    edges: BTreeMap<ConceptId, Vec<ConceptId>>,
    sizes: BTreeMap<ConceptId, usize>,
}
impl NeighborSource for MapGraph { /* 8 líneas */ }
```

Sin base de datos, sin fixtures, sin mocks con framework: un mapa.
Cada corte tiene su test (`respeta_max_nodes`, `respeta_max_depth`,
`respeta_presupuesto_de_bytes`), más ciclos y enlaces rotos. Y desde
el capítulo 16, el ejemplo de la documentación publicada de
`bounded_bfs` ES una de estas implementaciones de juguete,
compilando y pasando en cada build.

## La frontera de producción

> 🧰 **La rueda de serie:** en producción, [`petgraph`](https://docs.rs/petgraph) — aquí la lección era el ACOTADO por presupuesto, no el BFS. El mapa completo y el criterio para elegir: [La rueda de serie](la-rueda-de-serie.md).

Sobre Supabase, `neighbors` no puede ser una consulta SQL por nodo
(el clásico N+1). El adaptador del hito 2 hará *batch*: traer la
adyacencia de un conjunto de candidatos en una consulta y servir
`neighbors` desde ese caché local a la petición. El trait no cambia;
cambia la estrategia de quién lo implementa. También aparecerá un
`type Error` real (timeout, conexión) que `bounded_bfs` ya propaga
sin haberlo conocido jamás.

---

## Apéndice del capítulo

### Conceptos de Rust

* **Traits (interfaces):** un `trait` define un contrato de comportamiento. `pub trait NeighborSource` establece qué funciones debe ofrecer cualquier objeto para que el BFS pueda consultarle vecinos. A diferencia de otros lenguajes, los traits se implementan por separado de la estructura, con `impl Trait for MiEstructura`.
* **Tipos asociados (`type Error`):** un hueco para que cada implementación decida su propio tipo de error. En los tests usamos `Infallible` (el tipo que indica que el error no puede ocurrir); en producción, Postgres traerá el suyo. El BFS es genérico y sirve a ambos sin cambiar una línea.
* **`VecDeque` para colas eficientes:** `Vec` es rápido por el final y lento por el principio (desplaza todo lo demás). `VecDeque` es una cola de dos extremos sobre un búfer circular: `pop_front()` en O(1), ideal para BFS.
* **`.clone()` y propiedad:** `ConceptId` contiene un `String` (heap), así que no se copia solo. Si insertas el id en `visited_set`, esa variable toma la propiedad; para meterlo también en la cola hay que duplicarlo explícitamente con `.clone()`. El coste queda a la vista.
* **Aritmética segura con `checked_add`:** los bytes acumulados no se suman con `+` sino con `checked_add`, que devuelve `None` si desborda en lugar de dar la vuelta en silencio — un contador de presupuesto que se resetea solo sería un bug de seguridad.

### Memoria y asignación

El pico de memoria del BFS es `O(visited + queue)`, y ambos están
acotados: `visited` por el corte de nodos, y la cola porque solo se
encola desde nodos procesados (≤ max_nodes) × sus vecinos
(≤ max_links_per_document, garantizado al ESCRIBIR por el capítulo
4). Los presupuestos de escritura y de lectura colaboran: como
ningún documento tiene más de 512 enlaces, la cola no puede superar
128 × 512 entradas ni en el peor caso teórico — y el corte de bytes
la corta muchísimo antes.

La suma de bytes usa `checked_add` con saturación a `usize::MAX`:
un overflow de contador de presupuesto sería un bug de seguridad
silencioso (el contador da la vuelta y el presupuesto "se resetea").

### SOLID en juego

- **I (el protagonista):** `NeighborSource` tiene dos métodos. La
  alternativa — que el grafo reciba `&InMemoryStore` — funcionaría
  hoy y sería una trampa mañana: el grafo podría (y alguien lo
  haría) llamar a `commit` desde dentro de un recorrido. Una
  interfaz pequeña no es minimalismo estético: es eliminar
  capacidades que no deben existir en ese contexto.
- **D:** la flecha de dependencia apunta al dominio: `memory-store`
  (capítulo 7) implementa el trait DE `graph-core`, no al revés. El
  algoritmo no se recompilará cuando cambie el almacén.
- **O:** ¿ranking de vecinos por relevancia? Se añade como un orden
  sobre `Traversal.visited` fuera del BFS, o como una segunda
  función de recorrido. El BFS con sus garantías no se toca: está
  cerrado a modificación, abierto a composición.

### Ejercicios

1. ~~**Guiado.** Añade `Visited::parent: Option<ConceptId>` para poder
   reconstruir el CAMINO desde el origen a cada nodo. ¿Dónde se
   captura el padre con el mínimo de clones?~~ Implementado en el
   capítulo 15 — resultó ser exactamente lo que hacía falta para que
   una visualización de grafo pudiera dibujar aristas reales en vez
   de solo profundidades. El padre se captura en el único punto donde
   se ENCOLA un vecino nuevo
   (`queue.push_back((neighbor, depth + 1, Some(id.clone())))`) —
   un clon más, del nodo que ya tenías en la mano, no de toda la
   cadena.
2. **Medio.** Implementa `NeighborSource` para un grafo INVERSO
   (¿quién enlaza A alice?) sin cambiar `graph-core`. ¿Qué índice
   necesita mantener el almacén para servirlo en O(1)?
3. **Abierto.** El BFS trata todas las aristas igual. Diseña un
   recorrido con prioridad (los vecinos con más enlaces entrantes
   primero) usando `BinaryHeap` de `std`. ¿Qué pasa con la garantía
   "los truncados son los más lejanos"? ¿Merece la pena perderla?

Siguiente: [Capítulo 6 — Compare-and-swap: nadie pierde una escritura](06-conflictos-cas.md).
