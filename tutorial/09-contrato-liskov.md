# Capítulo 9 — El contrato ejecutable: Liskov antes de que exista Supabase

Crate: [`crates/store-core/src/contract.rs`](../crates/store-core/src/contract.rs)

## 1. El problema

El capítulo 7 prometió algo concreto: `InMemoryStore` (hito 1) y el
futuro adaptador de Supabase (hito 2) deben ser **indistinguibles**
para quien use `MemoryRepository`. Pero una promesa en un comentario
no es una promesa: es una esperanza. El día que alguien implemente
`SupabaseStore`, ¿quién comprueba que de verdad se comporta igual?
¿Que un conflicto CAS produce el mismo `StoreError::Conflict` con
los mismos hashes? ¿Que un commit inválido no deja rastro?

Sin un mecanismo, la respuesta es "un humano lo revisa a ojo" — y
los humanos se cansan, tienen fecha de entrega, y no comparan campo
a campo dos implementaciones cada vez que una cambia.

## 2. El invariante

> **El principio de sustitución de Liskov no es una propiedad que se
> declara: es una propiedad que se EJECUTA.** Si dos tipos implementan
> el mismo trait y pasan la misma batería de tests de comportamiento,
> son sustituibles. Si uno falla un test que el otro pasa, no lo son
> — y lo sabemos en segundos, no en producción.

## 3. La implementación mínima

`contract.rs` no importa `InMemoryStore`. Es la pieza clave: sus
funciones son genéricas sobre `R: MemoryRepository`, así que no
pueden — ni por accidente — apoyarse en un detalle de la
implementación en RAM.

```rust
pub fn run_all<R: MemoryRepository>(mut mk: impl FnMut() -> R) {
    crear_leer_y_versionar(mk());
    base_obsoleta_no_pisa(mk());
    commit_identico_es_idempotente(mk());
    documento_invalido_no_deja_rastro(mk());
    inexistente_es_none_y_notfound(mk());
    busqueda_respeta_filtros_y_limite(mk());
    historia_reciente_primero_y_paginada(mk());
}
```

Fíjate en la firma de `mk`: `impl FnMut() -> R`, una FÁBRICA, no un
valor. Cada propiedad necesita un repositorio VACÍO — si
compartiéramos una sola instancia entre las siete comprobaciones, el
concepto `"c"` que crea la primera contaminaría la segunda, y un
fallo de aislamiento se disfrazaría de fallo de lógica. La fábrica
es la garantía de que cada test empieza desde cero, sea cual sea el
backend.

Cada propiedad narra una GARANTÍA, no una implementación:

```rust
pub fn base_obsoleta_no_pisa<R: MemoryRepository>(mut repo: R) {
    let v1 = commit(&mut repo, "c", None, &doc("t", "v1")).unwrap();
    let v2 = commit(&mut repo, "c", Some(v1.content_id), &doc("t", "v2")).unwrap();

    let err = commit(&mut repo, "c", Some(v1.content_id), &doc("t", "pisotón")).unwrap_err();
    match err {
        StoreError::Conflict(c) => {
            assert_eq!(c.expected, Some(v1.content_id));
            assert_eq!(c.current, Some(v2.content_id));
        }
        other => panic!("una base obsoleta debe ser Conflict, fue {other:?}"),
    }
    let view = repo.get(&id("c")).unwrap().unwrap();
    assert_eq!(view.content_id, v2.content_id, "la escritura buena sigue intacta");
}
```

Nada aquí menciona `HashMap`, `BTreeMap` ni SQL. Es exactamente el
mismo escenario del capítulo 6 (§1), pero ahora expresado como
LEY DEL TRAIT en vez de test de una implementación concreta.

El uso, en [`tests/contract.rs`](../crates/memory-store/tests/contract.rs),
es deliberadamente aburrido:

```rust
#[test]
fn in_memory_cumple_el_contrato() {
    store_core::contract::run_all(memory_store::InMemoryStore::new);
}
```

El día que exista `supabase_adapter::SupabaseStore`, su test de
integración será literalmente esta misma línea con el nombre
cambiado — apuntando a una base de datos real de pruebas.

## 3.5. Conceptos de Rust en este capítulo

Este capítulo hace un uso avanzado de genéricos y clausuras (closures) para automatizar el diseño:

* **Funciones Genéricas con restricciones de Trait (`run_all<R: MemoryRepository>`):** El uso de `<R: MemoryRepository>` le dice al compilador: *"esta función puede trabajar con cualquier tipo `R`, siempre y cuando implemente el trait `MemoryRepository`"*. Esto nos permite escribir tests abstractos que sirven para probar la base de datos en memoria o en la nube, garantizando que ambas se comportan idénticamente.
* **Clausuras (Closures) y el trait `FnMut`:** Una clausura en Rust es una función anónima (o lambda) que puede capturar variables de su entorno. En `impl FnMut() -> R`, estamos pidiendo una función fábrica que puede ser ejecutada varias veces y que devuelve una instancia de `R`. Al llamarla antes de cada test como `mk()`, creamos un almacén vacío y limpio para cada caso de prueba, evitando que el estado de un test contamine a los demás.
* **Desestructuración en Pattern Matching (`StoreError::Conflict(c)`):** En el `match err`, cuando el error coincide con la variante `StoreError::Conflict`, no solo validamos el tipo de error, sino que extraemos el objeto `c` de su interior (`Conflict`) para inspeccionar sus propiedades (como `c.expected` y `c.current`). Esto permite realizar aserciones detalladas de los datos del error directamente en los brazos del `match`.

## 4. Una versión deliberadamente rota

La alternativa que todo el mundo prueba primero: escribir los tests
DENTRO de cada crate de implementación, duplicados.

```rust
// ❌ NO HACER: el mismo escenario, copiado dos veces
// en memory-store/src/lib.rs:
#[test]
fn base_obsoleta_no_pisa_en_ram() { /* ... 15 líneas ... */ }

// meses después, en supabase-adapter/src/lib.rs:
#[test]
fn base_obsoleta_no_pisa_en_supabase() { /* ... 15 líneas MUY parecidas ... */ }
```

## 5. Por qué falla

No es que no funcione — ambos tests pueden pasar hoy. El fallo
aparece en el PRÓXIMO cambio: alguien decide que un conflicto
también debería incluir `concept_id` (ejercicio 1 del capítulo 6).
Actualiza el test de `memory-store`. El de `supabase-adapter`, en
otro archivo, en otro crate, revisado por otra persona en otro PR,
se queda con la aserción vieja. Los dos backends DIVERGEN en
silencio: uno cumple el contrato nuevo, el otro el viejo, y no hay
ni una línea de código que lo note.

Duplicar tests de contrato no es "más seguro por partida doble": es
crear DOS fuentes de verdad que se pueden desincronizar. La fuente
única (`contract.rs`) hace estructuralmente imposible ese drift: hay
un solo lugar donde vive la ley, y todo backend se mide contra él.

## 6. Memoria y asignación

Nada nuevo respecto al capítulo 7: cada función del contrato hace
un puñado de commits sobre un repositorio recién creado. El coste es
el de la implementación bajo prueba, no el del arnés — que es
exactamente el punto: el contrato no debe imponer NINGÚN supuesto de
rendimiento o memoria; eso es asunto de cada backend.

## 7. Tests

Es tests todo el capítulo. La lista, y qué invariante narra cada uno:

| Función | Invariante que ejecuta |
| ------- | ----------------------- |
| `crear_leer_y_versionar` | crear = versión 1; los bytes vueltos son los escritos |
| `base_obsoleta_no_pisa` | el escenario central del capítulo 6, §1 |
| `commit_identico_es_idempotente` | mismo contenido → `no_change`, sin nueva revisión |
| `documento_invalido_no_deja_rastro` | validar ANTES de tocar el almacén (capítulo 4 + 7) |
| `inexistente_es_none_y_notfound` | dos formas distintas de "no está", cada una en su canal |
| `busqueda_respeta_filtros_y_limite` | combinación AND de filtros + corte por `limit` |
| `historia_reciente_primero_y_paginada` | orden y paginación sin solapes |

## 8. Frontera de producción

Este ES el mecanismo de frontera. Cuando el hito 2 traiga
`supabase-adapter` (con `tokio`, un cliente HTTP y SQL), su
obligación de entrada no es "revisar el código a mano": es hacer
pasar `contract::run_all(SupabaseStore::new_test_instance)` contra
una base de datos de pruebas real (via `docker compose` o un
proyecto Supabase de staging). Si pasa, es sustituible. Si no,
sabemos EXACTAMENTE qué garantía rompe, con el nombre de la función
en el mensaje de fallo.

## 9. Principios SOLID en juego

- **L, hecho literal:** este capítulo ES el principio de Liskov.
  No hay metáfora: el contrato son tests parametrizados sobre el
  trait, y "sustituible" pasa de ser una intención de diseño a ser
  un `cargo test` en verde o en rojo.
- **D:** `contract.rs` depende SOLO de `MemoryRepository` (el
  trait), nunca de una implementación. Si por error importara
  `InMemoryStore`, el compilador no lo impediría, pero el diseño sí
  lo hace incómodo — la fábrica genérica es la forma natural, y
  desviarse de ella requeriría ir a contracorriente del propio código.
- **S:** la única razón de cambio de este archivo es que cambien las
  GARANTÍAS del repositorio (lo que promete el trait), nunca que
  cambie CÓMO las cumple un backend concreto.

## 10. Ejercicios

1. **Guiado.** Añade `enlaces_entrantes_son_consultables` como nueva
   propiedad del contrato (si decides implementar esa capacidad tras
   el ejercicio 2 del capítulo 7). Escribe la propiedad ANTES que la
   implementación — así defines el contrato primero y la
   implementación lo persigue, no al revés.
2. **Medio.** ¿Qué propiedades del capítulo 7 (los tests de
   `memory-store/src/lib.rs`) NO pertenecen al contrato porque
   inspeccionan detalles internos (`s.blobs.len()`)? Sepáralas
   explícitamente en un comentario que diga "detalle de
   implementación, no contrato" y explica el criterio que usaste.
3. **Abierto.** Diseña `contract::run_all_concurrent` que lance N
   hilos escribiendo el MISMO concepto con la misma base esperada, y
   verifique que exactamente UNO gana y los demás reciben
   `Conflict`. ¿Pasa `InMemoryStore` hoy? (pista: revisa qué exclusividad
   da realmente `&mut self` cuando el propio arnés de test es
   quien invoca los métodos desde varios hilos con un `Mutex`
   alrededor — ¿estás probando el repositorio o tu propio mutex?)

Siguiente: [Capítulo 10 — HTTP sin estado: el mismo protocolo, otro sobre](10-http-sin-estado.md).
