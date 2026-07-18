# Capítulo 6 — Compare-and-swap: nadie pierde una escritura

Crate: [`crates/conflict-core`](../crates/conflict-core/src/lib.rs) ·
[referencia](https://pmaojo.github.io/okf-mcp/conflict_core/)

Dos agentes leen `people/alice` a la vez. El agente A añade un
proyecto; el agente B corrige el cargo. B escribe primero. Si A
escribe después "lo que él tiene" — su edición sobre la versión que
leyó — la corrección de B **desaparece sin dejar rastro**.

Acabas de ver la *actualización perdida*, el bug de concurrencia más
antiguo del mundo. En la mayoría de sistemas es un caso raro que
aparece bajo carga; con agentes de IA editando memoria compartida
deja de ser teórico y pasa a ser el caso normal: los agentes leen,
piensan durante segundos o minutos, y escriben. Ese hueco entre leer
y escribir es una autopista para el conflicto.

¿Cerrojos? Piénsalo dos veces. Un lock que dura lo que tarda un
modelo en razonar, sostenido en un servidor sin estado donde Vercel
puede matar la instancia a mitad… es cambiar un incidente por otro.
La respuesta de este capítulo es concurrencia OPTIMISTA: no
impedimos el conflicto — lo detectamos con precisión quirúrgica y lo
devolvemos con los datos para resolverlo. El invariante:

> **Toda escritura declara la base sobre la que se hizo. Si la base
> ya no es la cabeza actual, la escritura se rechaza con un
> conflicto estructurado. El almacén jamás pisa en silencio.**

## Una función, cuatro destinos

Lo primero que llama la atención de `conflict-core` es lo que NO
tiene: almacén, hash, E/S. Es UNA función pura:

```rust
pub fn decide(
    head: Option<ContentId>,      // qué hay ahora (None = no existe)
    expected: Option<ContentId>,  // qué creyó leer el cliente
    incoming: ContentId,          // qué quiere escribir
) -> CommitDecision
```

con un resultado que enumera TODOS los destinos posibles:

```rust
pub enum CommitDecision {
    Create,               // no existía, nadie esperaba que existiera
    Update,               // base correcta: avanza la cabeza
    NoChange,             // el contenido ya es exactamente ese
    Conflict(Conflict),   // la base es obsoleta: datos para reintentar
}
```

El cuerpo es un `match` sobre `(head, expected)` — cuatro
combinaciones de `Option` — y el compilador exige contemplarlas
todas. Esa exhaustividad no es una cuestión de estilo: es la
especificación hecha código. La tabla completa:

| head | expected | relación | decisión |
| ---- | -------- | -------- | -------- |
| None | None | — | `Create` |
| None | Some | borrado concurrente | `Conflict` |
| Some | None | creación concurrente | `Conflict` (o `NoChange` si escriben lo mismo) |
| Some(h) | Some(e), h == e | base correcta | `Update` (o `NoChange` si incoming == h) |
| Some(h) | Some(e), h != e | base obsoleta | `Conflict` (o `NoChange` si incoming == h: convergencia) |

Fíjate en los dos `NoChange` "raros" de la última columna, porque
son el capítulo 2 pagando dividendos: si dos agentes escriben byte a
byte lo mismo, no hay nada que perder y por tanto no hay conflicto
que declarar. Es **idempotencia por identidad de contenido** — un
reintento de red duplicado no crea una revisión duplicada, sin una
sola línea de código de deduplicación.

## La versión que casi todo el mundo escribe primero

```rust
// ❌ NO HACER: comparar versiones… leídas en otra consulta
let version_actual = store.get_version(&id);      // paso 1
if version_actual == version_esperada {
    store.escribir(&id, contenido);               // paso 2
}
```

Parece exactamente lo que pide el invariante: comprobar antes de
escribir. El defecto está en el espacio en blanco entre las líneas.
Entre el paso 1 y el paso 2 hay un hueco, y dos peticiones
concurrentes pueden AMBAS leer `version_actual == 7`, ambas pasar el
`if`, y ambas escribir: la segunda pisa a la primera exactamente
como si la comprobación no existiera. Este patrón de fallo tiene
nombre — TOCTOU, *time of check to time of use* — y la moraleja es
que la comprobación y el uso deben ser UN acto atómico.

¿Y por qué nuestro `decide` puro no sufre TOCTOU, si él tampoco es
atómico? Porque la atomicidad no es responsabilidad de la decisión,
sino de quien la ejecuta con exclusividad. Y aquí el proyecto juega
la misma carta dos veces:

- **Hito 1 (RAM):** `commit` recibe `&mut self`. El sistema de
  préstamos de Rust garantiza EN COMPILACIÓN que nadie más toca el
  almacén entre la lectura de la cabeza y la escritura. El borrow
  checker actúa de mutex estático.
- **Hito 2 (Postgres):** el mismo `decide` se convierte en el
  `WHERE` de un `UPDATE`:
  ```sql
  UPDATE okf_heads SET content_hash = :incoming, version = version + 1
  WHERE concept_id = :id AND content_hash = :expected;
  ```
  Si afecta 0 filas → conflicto. La atomicidad la da la base de
  datos; la SEMÁNTICA (qué significa cada caso) ya está definida y
  testeada aquí, en 50 líneas puras.

## Testear una decisión, no una carrera

Como la función es pura, cada fila de la tabla es un test de tres
líneas:

```rust
#[test]
fn base_obsoleta_es_conflicto() {
    let d = decide(Some(h(3)), Some(h(1)), h(2));
    assert_eq!(d, CommitDecision::Conflict(Conflict {
        expected: Some(h(1)), current: Some(h(3)), incoming: h(2),
    }));
}
```

Ahora imagina testear la versión rota: dos hilos, un sleep
estratégico y una oración. **La pureza no es estética funcional: es
testeabilidad comprada al precio de mover la E/S a otra parte.** El
test de integración del capítulo 8 cerrará el círculo con la escena
que abrió este capítulo: dos agentes, mismo `expected_hash`, el
segundo recibe `revision_conflict` y el contenido del primero sigue
intacto.

## La frontera de producción

> 🧰 **La rueda de serie:** esta rueda son 50 líneas puras: no hay crate que mejore eso. Para el merge a tres bandas futuro, [`similar`](https://docs.rs/similar) o [`diffy`](https://docs.rs/diffy). El mapa completo y el criterio para elegir: [La rueda de serie](la-rueda-de-serie.md).

En producción, `decide` se traduce al `UPDATE ... WHERE` condicional
que ya viste, y el `Conflict` viaja al cliente como JSON:

```json
{
  "kind": "revision_conflict",
  "expected_hash": "abc…",
  "current_hash": "def…",
  "incoming_hash": "789…",
  "hint": "relee el documento, re-aplica tu cambio sobre current_hash y reintenta"
}
```

Ese `hint` no es decoración: el consumidor es un MODELO de lenguaje,
y un error que explica el protocolo de recuperación convierte un
fallo en un bucle de reintento que funciona solo.

---

## Apéndice del capítulo

### Conceptos de Rust

* **Funciones puras y determinismo:** `decide` toma datos de entrada y devuelve un resultado sin leer disco, red ni modificar variables externas. Es predecible al 100 % y testearla no requiere mocks ni simulaciones.
* **Pattern matching sobre tuplas de `Option`:** en lugar de anidar `if`s, Rust permite agrupar valores en una tupla y compararlos a la vez: `match (head, expected)`. El compilador analiza todas las combinaciones de `Some`/`None` y obliga a manejarlas todas; si olvidas una, no compila.
* **El trait `Copy`:** `ContentId` implementa `Copy` porque solo guarda `[u8; 32]`: copiarlo es un copiado de bits en el stack, automático y barato, sin `.clone()` ni peleas con el borrow checker. Los tipos que manejan heap (como `String`) no pueden ser `Copy`. Cuando diseñes tipos de dominio, "¿puede ser `Copy`?" es una pregunta de ergonomía importante.

### Memoria y asignación

Cero asignaciones: `ContentId` es `Copy` (32 bytes en el stack) y
`decide` solo compara. Ese es todo el presupuesto del crate.

### SOLID en juego

- **S en su forma más pura:** este crate es una función. Su única
  razón de cambio es que cambie la POLÍTICA de concurrencia. Ni el
  formato de documento, ni el protocolo, ni el backend la tocan.
- **O:** el hito futuro de merge a tres bandas añadirá
  `CommitDecision::Merged { .. }` — una variante nueva. Los `match`
  existentes dejarán de compilar hasta contemplarla: el compilador
  repartirá la tarea de integración. Extensión guiada por tipos.
- **D:** `memory-store` (capítulo 7) depende de `conflict-core`.
  La política no sabe quién la ejecuta; los ejecutores importan la
  política. Si mañana hay un segundo camino de escritura (import
  masivo de un bundle), reutiliza `decide` y es CONSISTENTE por
  construcción con el camino normal.

### Ejercicios

1. **Guiado.** Añade a `Conflict` el campo `concept_id`. ¿Qué firmas
   cambian en cadena? Eso que acabas de medir es el acoplamiento
   real del tipo.
2. **Medio.** Un cliente reintenta un commit que YA fue aplicado
   (timeout de red tras el éxito). Traza qué devuelve `decide` en el
   reintento y por qué el resultado es el correcto sin código
   especial de deduplicación.
3. **Abierto.** Diseña el merge a tres bandas: con `base`, `current`
   e `incoming` disponibles como textos, ¿cuándo es seguro
   auto-fusionar? Escribe los TESTS primero (frontmatter disjunto,
   párrafos disjuntos, edición solapada) y decide qué casos rechazas
   siempre. Compara tu diseño con el de Git (`diff3`).

Siguiente: [Capítulo 7 — El repositorio: blobs inmutables, cabezas móviles](07-repositorio.md).
