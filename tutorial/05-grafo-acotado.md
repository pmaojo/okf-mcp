# Capítulo 5 — El grafo acotado: BFS con presupuesto

Crate: [`crates/graph-core`](../crates/graph-core/src/lib.rs)

## 1. El problema

`memory_resolve` no devuelve solo un documento: devuelve su
**vecindario** — los conceptos alcanzables siguiendo enlaces
`[[...]]`. Eso es un recorrido de grafo, y los grafos de
conocimiento reales tienen dos propiedades incómodas:

- **Ciclos**: `alice → proyecto → alice`. Un recorrido ingenuo no
  termina jamás.
- **Explosión**: un concepto "hub" (una etiqueta popular, una
  persona central) puede alcanzar miles de nodos en 3 saltos.

En un proceso serverless con memoria y tiempo tasados, "cargar el
grafo entero y luego decidir" no es una opción: es la definición de
un incidente.

## 2. El invariante

> **El recorrido jamás excede su presupuesto — nodos, profundidad,
> bytes — y SIEMPRE informa de si truncó y por qué.**

La segunda mitad es tan importante como la primera. Un agente que
recibe 128 nodos necesita saber si son TODOS los vecinos o los
primeros 128: en el primer caso puede razonar "no hay más relaciones";
en el segundo debe pedir más o refinar. Por eso `Traversal` lleva
tres flags separados (`truncated_by_nodes`, `by_depth`, `by_bytes`)
en lugar de un booleano genérico — cada uno sugiere una acción
distinta al cliente.

## 3. La implementación mínima

Primero, la abstracción (el corazón SOLID del capítulo):

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

El BFS clásico, con los tres cortes:

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

Detalles de Rust:

- **`VecDeque`** es la cola FIFO de `std` (un ring buffer). Usar
  `Vec::remove(0)` sería O(n) por extracción; `pop_front` es O(1).
- **`visited_set.insert` devuelve `bool`** — `false` si ya estaba.
  Ese booleano ES la protección contra ciclos, en una línea.
- **El marcado ocurre al ENCOLAR, no al desencolar.** Si marcáramos
  al procesar, un rombo (`a→b`, `a→c`, `b→d`, `c→d`) encolaría `d`
  dos veces. No es incorrecto (se filtraría después) pero infla la
  cola — y la cola también es memoria.
- **Enlaces rotos son datos, no errores**: `document_size` devuelve
  `Option`, y un `None` produce `Visited { exists: false }`. En una
  base de conocimiento viva siempre hay enlaces a conceptos que aún
  no se escribieron; el agente los ve y puede decidir crearlos.

## 4. Una versión deliberadamente rota

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

## 5. Por qué falla

Cuenta los cortes que faltan:

1. **Sin `visited`**: el ciclo `a→b→a` con límite de profundidad 4
   no cuelga… pero visita `a` 8 veces. Con profundidad 10 y un grafo
   denso, la duplicación es exponencial: 4 saltos con factor de
   ramificación 20 son hasta 160 000 visitas para quizá 200 nodos
   únicos.
2. **Sin límite de nodos**: la profundidad NO acota el trabajo. Un
   hub con 5 000 vecinos directos revienta el presupuesto en el
   nivel 1.
3. **DFS en vez de BFS**: los resultados salen en orden de "rama
   profunda primero". Para memoria de agentes, los vecinos CERCANOS
   son los relevantes; BFS los da primero y el truncado por nodos
   conserva exactamente los más próximos, que es lo que quieres
   conservar.
4. **`.unwrap()`**: sobre Postgres, un timeout de red mata el
   proceso entero en vez de devolver un error al cliente.

## 6. Memoria y asignación

El pico de memoria del BFS es `O(visited + queue)`, y ambos están
acotados: `visited` por el corte de nodos, y la cola porque solo se
encola desde nodos procesados (≤ max_nodes) × sus vecinos (≤
max_links_per_document, garantizado al ESCRIBIR por el capítulo 4).
Los presupuestos de escritura y de lectura colaboran: como ningún
documento tiene más de 512 enlaces, la cola no puede superar
128 × 512 entradas ni en el peor caso teórico — y el corte de bytes
la corta muchísimo antes.

La suma de bytes usa `checked_add` con saturación a `usize::MAX`:
un overflow de contador de presupuesto sería un bug de seguridad
silencioso (el contador da la vuelta y el presupuesto "se resetea").

## 7. Tests

La joya es lo pequeño que es el arnés gracias a SOLID-I:

```rust
struct MapGraph {
    edges: BTreeMap<ConceptId, Vec<ConceptId>>,
    sizes: BTreeMap<ConceptId, usize>,
}
impl NeighborSource for MapGraph { /* 8 líneas */ }
```

Sin base de datos, sin fixtures, sin mocks con framework: un mapa.
Cada corte tiene su test (`respeta_max_nodes`, `respeta_max_depth`,
`respeta_presupuesto_de_bytes`), más ciclos y enlaces rotos.

## 8. Frontera de producción

Sobre Supabase, `neighbors` no puede ser una consulta SQL por nodo
(el clásico N+1). El adaptador del hito 2 hará *batch*: traer la
adyacencia de un conjunto de candidatos en una consulta y servir
`neighbors` desde ese caché local a la petición. El trait no cambia;
cambia la estrategia de quién lo implementa. También aparecerá un
`type Error` real (timeout, conexión) que `bounded_bfs` ya propaga
sin haberlo conocido jamás.

## 9. Principios SOLID en juego

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

## 10. Ejercicios

1. **Guiado.** Añade `Visited::parent: Option<ConceptId>` para poder
   reconstruir el CAMINO desde el origen a cada nodo. ¿Dónde se
   captura el padre con el mínimo de clones?
2. **Medio.** Implementa `NeighborSource` para un grafo INVERSO
   (¿quién enlaza A alice?) sin cambiar `graph-core`. ¿Qué índice
   necesita mantener el almacén para servirlo en O(1)?
3. **Abierto.** El BFS trata todas las aristas igual. Diseña un
   recorrido con prioridad (los vecinos con más enlaces entrantes
   primero) usando `BinaryHeap` de `std`. ¿Qué pasa con la garantía
   "los truncados son los más lejanos"? ¿Merece la pena perderla?

Siguiente: [Capítulo 6 — Compare-and-swap: nadie pierde una escritura](06-conflictos-cas.md).
