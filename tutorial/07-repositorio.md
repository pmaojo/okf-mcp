# Capítulo 7 — El repositorio: blobs inmutables, cabezas móviles

Crates: [`crates/store-core`](../crates/store-core/src/lib.rs) (API)
y [`crates/memory-store`](../crates/memory-store/src/lib.rs)
(en memoria) ·
[referencia](https://pmaojo.github.io/okf-mcp/store_core/)

Haz inventario de lo que llevas: identificadores que no pueden hacer
daño, identidad por contenido, un parser de documentos, un recorrido
de grafo con presupuesto y una política de conflictos pura. Piezas
impecables — y sueltas. Falta el órgano que las orquesta y GUARDA:
crear, leer, buscar, escribir con CAS, recordar revisiones y
alimentar el grafo.

Y falta con una exigencia extra que lo cambia todo: la versión en
RAM que vamos a escribir ahora (hito 1) y las de producción que vendrán
después —Supabase y GitHub (hito 2)— deben ser **intercambiables**. Ese requisito va a
decidir cada firma de este capítulo.

El modelo de datos entero cabe en una frase:

> **Los contenidos son inmutables y direccionados por su hash; lo
> único que cambia es a qué contenido apunta cada concepto.**

```text
blobs      : ContentId -> Arc<str>       (inmutable, deduplicado)
heads      : ConceptId -> Head           (puntero mutable + versión)
revisions  : Vec<Revision>               (append-only)
links      : ConceptId -> Vec<ConceptId> (índice DERIVADO)
```

Si esto te suena a Git, es porque ES la idea de Git — objetos
inmutables + refs móviles — reducida a su esencia. Y la
inmutabilidad compra en cascada: deduplicación gratis (dos conceptos
con el mismo texto comparten blob), historia barata (una revisión
son dos hashes), CAS posible (comparar 32 bytes), y compartición
segura en memoria (nadie puede mutar lo que otro está leyendo).

## El contrato primero

La pieza más importante del capítulo no es la implementación: es el
trait que la implementación tendrá que servir. Míralo con calma:

```rust
pub trait MemoryRepository {
    fn get(&self, id: &ConceptId) -> Result<Option<DocumentView>, StoreError>;
    fn search(&self, query: &SearchQuery, budget: &Budget) -> Result<Vec<SearchHit>, StoreError>;
    fn commit(&mut self, request: CommitRequest, actor: &Principal, budget: &Budget)
        -> Result<CommitOutcome, StoreError>;
    fn history(&self, id: &ConceptId, limit: usize, before_seq: Option<u64>)
        -> Result<Vec<Revision>, StoreError>;
}
```

Cuatro métodos = cuatro herramientas MCP. Nota lo que NO hay:
`delete_file`, `list_dir`, `rename`. El repositorio habla el idioma
del DOMINIO, no el de un sistema de ficheros que ya no existe
(recuerda dónde acabará esto: en Vercel no hay disco persistente).

El `commit` es la coreografía de todos los capítulos anteriores, en
tres compases:

```rust
// 1. Validar SIEMPRE antes del CAS (cap. 4): un documento inválido
//    no debe ni llegar a producir un conflicto.
let doc = okf_core::parse_document(&request.markdown, budget)?;

// 2. Identidad (cap. 2) y decisión pura (cap. 6).
let incoming = ContentId(sha256(request.markdown.as_bytes()));
let decision = decide(head.map(|h| h.content_id), request.expected, incoming);

// 3. Ejecutar la decisión: blob + cabeza + revisión + índice de
//    enlaces, atómicamente.
```

¿Y de dónde sale la atomicidad del paso 3, si no hay ningún lock a
la vista? De la firma: `commit(&mut self, ...)`. Un `&mut` es
exclusivo por definición del lenguaje — mientras este método corre,
NADIE más lee ni escribe el almacén, y eso lo garantiza el
compilador, no un semáforo. Es la misma jugada que salvó al capítulo
6 del TOCTOU: el borrow checker como mutex estático. (La versión
multihilo envolvería el almacén en `RwLock`; la de Postgres usará
una transacción. El trait no cambia.)

El otro detalle con miga es `Arc<str>` para los blobs. `Arc` es un
puntero con conteo de referencias atómico: `get` devuelve el
documento SIN copiar los bytes — devuelve un puntero más. ¿Y por qué
`<str>` y no `<String>`? Un `Arc<str>` apunta directo a los bytes
(una indirección); `Arc<String>` apunta a un struct que apunta a los
bytes (dos). Y `str`, sin capacidad de crecer, dice en el tipo lo
que el invariante dice en prosa: **esto no crecerá jamás**.

## El almacén "sencillo" al que le faltan los órganos

La versión rota de este capítulo es la que cualquiera escribe
primero, y con toda la razón aparente:

```rust
// ❌ NO HACER: documentos mutables in situ
pub struct Store {
    docs: HashMap<ConceptId, String>,
}
impl Store {
    pub fn write(&mut self, id: ConceptId, texto: String) {
        self.docs.insert(id, texto);   // pisa lo anterior
    }
}
```

No es que tenga un bug: es que le FALTAN los órganos, y se nota al
pedirle cada cosa que el sistema necesita. ¿Historia? Pisada —
`memory_history` es inimplementable. ¿CAS? ¿Contra qué comparas, si
lo anterior ya no existe? ¿Idempotencia? Cada escritura idéntica
parece un cambio. ¿Deduplicación? Cien conceptos con el mismo texto,
cien copias.

Y la sutil, la que solo aparece al escribir `get`: debe devolver o
bien `&String` (un préstamo que bloquea el almacén mientras se use)
o bien `String` clonada (256 KiB copiados por lectura). El diseño
inmutable con `Arc` esquiva el dilema entero: compartir es seguro
PORQUE nada muta.

La lección de diseño, que vale más que el código: la mutabilidad in
situ no es "la versión simple" — es una versión DISTINTA que cierra
puertas. La inmutabilidad no se añade después; se elige al
principio.

## El test que da nombre al capítulo

```rust
#[test]
fn una_base_obsoleta_nunca_pisa_una_escritura_mas_nueva() {
    // v1 → v2; luego alguien que leyó v1 intenta escribir.
    let err = commit(&mut s, "n", Some(v1.content_id), &doc("t", "pisotón")).unwrap_err();
    // conflicto CON los hashes correctos…
    // …y v2 sigue intacta:
    assert_eq!(s.get(&id("n")).unwrap().unwrap().content_id, v2.content_id);
}
```

Es la escena de los dos agentes del capítulo 6, ejecutada contra el
almacén real. Completan la suite: idempotencia (mismo texto →
`no_change`, CERO revisiones nuevas), documentos inválidos que no
dejan rastro, deduplicación de blobs, historia paginada, y el test
integrador donde el almacén alimenta al BFS del capítulo 5 vía
`NeighborSource` — la primera vez que todas las piezas suenan
juntas.

## La frontera de producción

> 🧰 **La rueda de serie:** en producción, [`sqlx`](https://docs.rs/sqlx) — y este repo YA lo usa en `supabase-store` (hito 2), sin tocar a los consumidores del trait. El mapa completo y el criterio para elegir: [La rueda de serie](la-rueda-de-serie.md).

La tabla de traducción a Supabase está decidida desde hoy:

| RAM (hito 1) | Postgres (hito 2) |
| ------------ | ----------------- |
| `blobs: HashMap` | tabla `okf_blobs (content_hash, content)` |
| `heads: BTreeMap` | tabla `okf_heads` con `UPDATE ... WHERE content_hash = expected` |
| `revisions: Vec` | tabla `okf_revisions` append-only |
| `links` derivado | tabla `okf_links`, reconstruida por commit |
| `&mut self` | transacción + la condición del UPDATE |

Más una pieza nueva sin equivalente en RAM: la **outbox
transaccional** (eventos "documento cambiado" insertados en la misma
transacción, consumidos por un worker para Git y embeddings —
capítulo 14). El trait `MemoryRepository` no se entera de nada de
esto.

---

## Apéndice del capítulo

### Conceptos de Rust

* **Punteros inteligentes con `Arc<str>`:** cuando quieres compartir la propiedad de un dato entre varios sitios sin copiarlo, usas `Arc` (Atomic Reference Counted). Clonar un `Arc` solo incrementa un contador (muy rápido) en lugar de duplicar los datos. Usamos `Arc<str>` en lugar de `Arc<String>`: un `str` es inmutable y de longitud exacta, lo que ahorra una indirección y asegura que los datos no puedan cambiar por accidente.
* **`&mut self` como mutex estático:** la regla de préstamos garantiza que si alguien tiene un `&mut`, nadie más lee ni escribe esa estructura a la vez. En un entorno síncrono, el compilador garantiza la atomicidad del commit sin locks en tiempo de ejecución.
* **El patrón Entry API:** `self.blobs.entry(hash).or_insert_with(...)` busca la clave y, si no existe, crea e inserta el valor — todo en una operación, sin buscar dos veces en el mapa.

### Memoria y asignación

- **Escritura:** una asignación grande (el blob, una vez) +
  metadatos. Blob repetido = cero (deduplicado por
  `entry().or_insert_with`).
- **Lectura:** cero copias del contenido (`Arc::clone` = un
  incremento atómico).
- **Búsqueda:** el hito 1 re-parsea cada documento al buscar —
  correcto y O(n·m), asumido conscientemente: la búsqueda real del
  hito 2 es de Postgres (índices, full-text). No optimizamos lo que
  vamos a reemplazar; sí dejamos el LÍMITE (`max_search_results`
  corta el bucle) porque los límites sí se quedan.

### SOLID en juego

- **L (el protagonista):** `InMemoryStore` y `SupabaseStore` deberán
  ser sustituibles bajo `MemoryRepository`. Liskov no es "misma
  firma": es **mismo contrato de comportamiento** — mismos errores
  ante las mismas situaciones (base obsoleta → `Conflict` con esos
  tres hashes, nunca un `Backend("update failed")` genérico), mismas
  garantías post-commit. La táctica del hito 2: los tests de este
  capítulo se factorizan en `fn contrato<R: MemoryRepository>(repo: R)`
  y se ejecutan contra ambas implementaciones. Un test compartido es
  Liskov ejecutable — el capítulo 9 lo cuenta entero.
- **S:** mira los `use` del crate: validación de okf-core, decisión
  de conflict-core, hash de hash-core. `memory-store` solo ORQUESTA
  y almacena. Cada regla vive en su casa y se testea en su casa.
- **D:** `StoreError::Conflict` ENVUELVE al `Conflict` de
  conflict-core en vez de redefinirlo: la política de concurrencia
  tiene un solo dueño.

### Ejercicios

1. **Guiado.** Implementa `delete` como un commit especial: la
   cabeza desaparece pero blob y revisiones QUEDAN (el invariante de
   inmutabilidad no se negocia). ¿Qué variante nueva necesita
   `CommitDecision`? ¿Qué pasa con los enlaces entrantes de otros
   documentos?
2. **Medio.** El índice `links` se reconstruye por commit pero nadie
   lo LIMPIA si un documento deja de tener enlaces… ¿o sí? Lee el
   código, decide si hay bug, y escribe el test que lo demuestre en
   un sentido u otro.
3. **Abierto.** Escribe `fn contrato<R: MemoryRepository>(mk: impl Fn() -> R)`
   con los 8 tests de este capítulo parametrizados. Es el arnés que
   el hito 2 ejecutará contra Supabase. ¿Qué tests NO pueden
   escribirse contra el trait (pista: `blobs.len()`) y qué te dice
   eso sobre qué es contrato y qué es detalle?

Siguiente: [Capítulo 8 — El protocolo MCP y el transporte](08-protocolo-mcp.md).
