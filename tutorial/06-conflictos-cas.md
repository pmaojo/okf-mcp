# Capítulo 6 — Compare-and-swap: nadie pierde una escritura

Crate: [`crates/conflict-core`](../crates/conflict-core/src/lib.rs)

## 1. El problema

Dos agentes leen `people/alice` a la vez. El agente A añade un
proyecto; el agente B corrige el cargo. B escribe primero. Si A
escribe después "lo que él tiene" — su edición sobre la versión que
leyó — la corrección de B **desaparece sin dejar rastro**. Es la
*actualización perdida*, el bug de concurrencia más antiguo del
mundo, y con agentes de IA editando memoria compartida deja de ser
teórico: es el caso normal.

Los cerrojos (locks) no sirven aquí: el "leer, pensar, escribir" de
un agente dura segundos o minutos, y un lock de esa duración en un
servidor sin estado (¡Vercel puede matar la instancia!) es otra
categoría de incidente.

## 2. El invariante

> **Toda escritura declara la base sobre la que se hizo. Si la base ya no es la cabeza actual, la escritura se rechaza con un conflicto estructurado. El almacén jamás pisa en silencio.**

Concurrencia OPTIMISTA: no impedimos el conflicto, lo detectamos con
precisión y lo devolvemos con los datos para resolverlo (releer,
re-aplicar, reintentar).

## 3. La implementación mínima

Lo primero es notar lo que este crate NO tiene: almacén, hash, E/S.
Es UNA función pura:

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
todas. Esa exhaustividad no es estilo: es la especificación hecha
código. La tabla completa:

| head | expected | relación | decisión |
| ---- | -------- | -------- | -------- |
| None | None | — | `Create` |
| None | Some | borrado concurrente | `Conflict` |
| Some | None | creación concurrente | `Conflict` (o `NoChange` si escriben lo mismo) |
| Some(h) | Some(e), h == e | base correcta | `Update` (o `NoChange` si incoming == h) |
| Some(h) | Some(e), h != e | base obsoleta | `Conflict` (o `NoChange` si incoming == h: convergencia) |

Los dos `NoChange` "raros" salen gratis del hash de contenido
(capítulo 2): si dos agentes escriben byte a byte lo mismo, no hay
nada que perder y por tanto no hay conflicto que declarar. Es
**idempotencia por identidad de contenido** — un reintento de red
duplicado no crea una revisión duplicada.

## 3.5. Conceptos de Rust en este capítulo

Este capítulo destaca por su sencillez estructural gracias a dos conceptos de Rust:

* **Funciones puras y determinismo:** En Rust, las funciones son inmutables por defecto. La función `decide` es una *función pura*: toma datos de entrada por valor y devuelve un resultado sin realizar lecturas de disco, red ni modificar variables externas. Esto hace que sea predecible al 100% y que testearla requiera solo una línea de código, sin necesidad de simulaciones (mocks) complejas.
* **Pattern matching sobre tuplas de `Option`:** En lugar de anidar múltiples sentencias `if`, Rust permite agrupar varios valores en una tupla y compararlos a la vez: `match (head, expected)`. El compilador analiza de forma matemática todas las combinaciones posibles de `Some` y `None` y te obliga a manejarlas todas. Si olvidas alguna, el programa no compila.
* **El trait `Copy`:** El tipo `ContentId` implementa `Copy` porque internamente solo guarda un array fijo de 32 bytes (`[u8; 32]`). En Rust, los tipos simples y pequeños que implementan `Copy` se copian de forma automática y barata (un simple copiado de bits en el stack) al pasarse como parámetros o asignarse a otras variables. Esto elimina la necesidad de llamar a `.clone()` y evita problemas con el borrow checker. Los tipos grandes que manejan memoria en el heap (como `String`) no pueden implementar `Copy`.

## 4. Una versión deliberadamente rota

La versión que casi todo el mundo escribe primero:

```rust
// ❌ NO HACER: comparar versiones… leídas en otra consulta
let version_actual = store.get_version(&id);      // paso 1
if version_actual == version_esperada {
    store.escribir(&id, contenido);               // paso 2
}
```

## 5. Por qué falla

Entre el paso 1 y el paso 2 hay un hueco. Dos peticiones
concurrentes pueden AMBAS leer `version_actual == 7`, ambas pasar el
`if`, y ambas escribir: la segunda pisa a la primera exactamente
como si no hubiera comprobación. Es un TOCTOU (*time of check to
time of use*): la comprobación y el uso deben ser UN acto atómico.

¿Y por qué nuestro `decide` puro no tiene este problema? Porque la
atomicidad no es responsabilidad de la decisión, sino de quien la
ejecuta con exclusividad:

- **Hito 1 (RAM):** `commit` recibe `&mut self`. El sistema de
  préstamos de Rust garantiza EN COMPILACIÓN que nadie más toca el
  almacén entre la lectura de la cabeza y la escritura. El borrow
  checker es aquí un mutex estático.
- **Hito 2 (Postgres):** el mismo `decide` se convierte en el
  `WHERE` de un `UPDATE`:
  ```sql
  UPDATE okf_heads SET content_hash = :incoming, version = version + 1
  WHERE concept_id = :id AND content_hash = :expected;
  ```
  Si afecta 0 filas → conflicto. La atomicidad la da la base de
  datos; la SEMÁNTICA (qué significa cada caso) ya está definida y
  testeada aquí.

## 6. Memoria y asignación

Cero asignaciones: `ContentId` es `Copy` (32 bytes en el stack) y
`decide` solo compara. Vale la pena notar el porqué: un `[u8; 32]`
es `Copy` porque copiarlo es un `memcpy` trivial sin recursos que
liberar. `String` no puede serlo. Cuando diseñes tipos de dominio,
"¿puede ser `Copy`?" es una pregunta de ergonomía importante: los
tipos `Copy` fluyen por el código sin peleas con el borrow checker.

## 7. Tests

Siete tests, uno por fila de la tabla de decisión (más las
convergencias). Como la función es pura, cada test son tres líneas:

```rust
#[test]
fn base_obsoleta_es_conflicto() {
    let d = decide(Some(h(3)), Some(h(1)), h(2));
    assert_eq!(d, CommitDecision::Conflict(Conflict {
        expected: Some(h(1)), current: Some(h(3)), incoming: h(2),
    }));
}
```

Compáralo con testear la versión rota: necesitarías dos hilos, un
sleep estratégico y una oración. **La pureza no es estética funcional: es testeabilidad comprada al precio de mover la E/S a otra parte.**

El test de integración del capítulo 8 cierra el círculo con el
escenario narrado en §1: dos agentes, mismo `expected_hash`, el
segundo recibe `revision_conflict` y el contenido del primero sigue
intacto.

## 8. Frontera de producción

Ya contada en §5: `decide` se traduce a un `UPDATE ... WHERE`
condicional y la fila afectada (1 o 0) selecciona la rama. El enum
`Conflict` viaja al cliente como JSON:

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

## 9. Principios SOLID en juego

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

## 10. Ejercicios

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
