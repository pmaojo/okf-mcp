# Capítulo 16 — `cargo doc`: la documentación que compila

Crates: todos los del núcleo. Referencia navegable publicada:
[pmaojo.github.io/okf-mcp](https://pmaojo.github.io/okf-mcp/)

## 1. El problema

Todo lo escrito sobre el código, salvo el código, envejece mal. Un
README dice que `parse` devuelve el offset del error; seis meses
después alguien cambia el parser y el README sigue diciendo lo
mismo, ahora en falso. El comentario no se entera. El tutorial no se
entera. El lector nuevo se entera el último, y a mala hora.

Este repositorio tiene un tutorial entero apoyado en afirmaciones
sobre el código ("el CAS nunca sobreescribe en silencio", "el digest
no depende del troceo"). Si esas afirmaciones viven solo en Markdown,
son promesas. Queremos que sean **hechos verificados en cada build**.

Rust trae la herramienta de serie: `rustdoc`, invocado con
`cargo doc`, convierte los comentarios `///` y `//!` en un sitio web
navegable con búsqueda, enlaces entre tipos y — la joya de la corona
— **ejemplos que compilan y se ejecutan como tests**.

```bash
cargo doc --no-deps --open   # genera target/doc y lo abre
cargo test --doc             # ejecuta TODOS los ejemplos
```


El conflicto lo sufre quien entra al proyecto después: confía en un ejemplo
de la documentación, lo copia y descubre que ya no compila. Peor aún, un
adaptador nuevo implementa mal un trait porque la página del contrato omitía
una precondición. `cargo doc` importa como arquitectura: la documentación
pública es parte del puerto, y sus ejemplos deben obedecer el mismo TDD que
el código.

## 2. El invariante

> **Nada de lo que afirma la documentación queda sin verificar.**
> Los ejemplos compilan y pasan (`cargo test --doc`); los enlaces
> entre items resuelven o el build avisa; y en los crates del núcleo
> ningún item público queda sin documentar
> (`#![warn(missing_docs)]`).

La consecuencia práctica: la documentación deja de ser un artefacto
paralelo que hay que "mantener sincronizado" y pasa a ser parte del
programa. Se rompe con un `assert_eq!` rojo, igual que el resto.

## 3. La implementación mínima

### 3.1. Dos clases de comentario, dos audiencias

```rust
//! Tipos de dominio del motor de memoria.        ← documenta el CRATE
//! (va al principio de lib.rs, con //!)

/// Identificador lógico de un concepto.           ← documenta el ITEM
/// (va justo encima de structs, fns, campos…)
pub struct ConceptId(String);
```

`//!` habla del contenedor desde dentro; `///` habla del item que
sigue. La primera línea de cada `///` es sagrada: es el resumen que
rustdoc muestra en los índices y en la búsqueda. Una línea, una
afirmación.

### 3.2. Enlaces intra-doc: el grafo de la API

Entre corchetes y backticks, cualquier ruta de Rust se convierte en
un enlace verificado por el compilador:

```rust
/// - longitud entre 1 y [`MAX_CONCEPT_ID_LEN`]
///
/// Un rechazo llega como [`StoreError::Conflict`] con los hashes
/// para releer y reintentar.
```

No son strings: si mañana `MAX_CONCEPT_ID_LEN` se renombra y el
enlace queda colgando, `cargo doc` emite un warning
(`broken_intra_doc_links`) — y nuestro CI convierte los warnings en
errores. Los enlaces cruzan crates: `okf-core` enlaza a
`memory_model::Budget` y rustdoc teje el grafo completo. El
resultado publicado se navega igual que el grafo de conceptos que
este servidor almacena: de [`InMemoryStore`] a [`MemoryRepository`],
de ahí al módulo `contract`… la arquitectura entera, a golpe de clic.

[`InMemoryStore`]: https://pmaojo.github.io/okf-mcp/memory_store/struct.InMemoryStore.html
[`MemoryRepository`]: https://pmaojo.github.io/okf-mcp/store_core/trait.MemoryRepository.html

### 3.3. Doctests: la afirmación ejecutable

Así documenta `hash-core` que su SHA-256 es correcto:

```rust
//! # Ejemplo
//!
//! ```
//! let digest = hash_core::sha256(b"abc");
//! let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
//! assert_eq!(
//!     hex,
//!     "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
//! );
//! ```
```

Ese hex es un vector oficial de NIST. La página generada no dice "el
hash funciona": lo demuestra, y `cargo test --doc` lo re-demuestra en
cada ejecución. Cada bloque de código en la documentación es, por
defecto, un test.

### 3.4. El doctest que debe NO compilar

La afirmación más fuerte del capítulo 1 era "un `ConceptId` no se
puede fabricar sin validar". ¿Cómo se documenta eso de forma
verificable? Con un ejemplo anotado `compile_fail`:

```rust
/// ```compile_fail
/// let id = memory_model::ConceptId("../etc".to_string());
/// ```
```

rustdoc compila el bloque y **exige que falle la compilación**. Si
algún día alguien hace público el campo interno, este "test" se pone
rojo: el sistema de tipos y la documentación se vigilan mutuamente.
El resto de anotaciones útiles: `no_run` (compila pero no ejecuta,
para ejemplos con red o disco), `ignore` (ni compila; siempre con un
motivo escrito al lado, como hace el módulo `contract` de
`store-core`, que no puede importar `memory-store` sin crear un
ciclo de dependencias).

### 3.5. Conceptos de Rust en este capítulo

- **Cada doctest es un crate.** rustdoc envuelve el bloque en un
  `fn main()` (si no lo tiene), lo compila como crate independiente
  contra tu biblioteca y lo ejecuta. Por eso los doctests solo ven la
  API PÚBLICA: son el primer consumidor real de tu crate.
- **Líneas ocultas.** Una línea que empieza por `# ` se compila pero
  no se muestra. Así el ejemplo enseña lo esencial sin esconder el
  código de apoyo al compilador:

  ```rust
  /// let id = ConceptId::parse("people/alice")?;
  /// # Ok::<(), memory_model::ConceptIdError>(())
  ```

  (ese `Ok` oculto le da tipo al `?` del ejemplo).
- **`#![warn(missing_docs)]`** es un lint de crate: cada item
  público sin `///` — funciones, structs, campos, variantes,
  constantes — produce un warning. Localmente avisa; en CI, con
  `RUSTDOCFLAGS="-D warnings"`, bloquea el merge.

## 4. Una versión deliberadamente rota

El capítulo 3 presumía de errores con offset. Documentémoslo… mal:

```rust
/// # Errores
///
/// ```
/// let err = json_mini::parse(r#"{"a":1}basura"#).unwrap_err();
/// assert_eq!(err.offset, 6); // ← mentira piadosa: era 7
/// ```
pub fn parse(input: &str) -> Result<Value, ParseError> {
```


La versión rota se escribe con buena intención: dejar ejemplos en Markdown
porque son más legibles y no obligan a pelear con imports. Durante semanas
ayudan; después se convierten en deuda silenciosa porque ningún compilador
los lee.

## 5. Por qué falla

```text
$ cargo test --doc -p json-mini
---- crates/json-mini/src/lib.rs - parse (line 171) stdout ----
assertion `left == right` failed
  left: 7
 right: 6
error: doctest failed
```

La mentira dura exactamente un `cargo test`. Compárese con la misma
mentira en un README: duraría hasta que un lector la sufriera. La
diferencia no es cosmética, es estructural — la documentación
ejecutable está DENTRO del ciclo de compilación-test, y todo lo que
está dentro del ciclo se mantiene solo.

El mismo mecanismo protege los enlaces: `` [`ConceptID`] `` (con la
D final equivocada) no resuelve a ningún item, rustdoc lo reporta, y
`-D warnings` lo convierte en fallo de build.

## 6. Memoria y asignación

El coste de rustdoc es de **compilación, no de ejecución**: los
`///` desaparecen del binario (son atributos `#[doc]`, metadatos que
el codegen descarta) y `target/doc` es HTML estático que se sirve
sin ningún servidor de aplicación.

El coste real está en los doctests: N bloques = N crates que
compilar. En este workspace son un puñado y tardan segundos; en
crates enormes se nota, y por eso existen `no_run` e `ignore` — pagar
solo la verificación que aporta. Regla del proyecto: `ignore`
siempre lleva al lado el porqué.

## 7. Tests

La puerta de calidad completa vive en un script, hermano del
`check-std-only.sh` del capítulo 0:

```bash
./scripts/check-docs.sh
# 1. RUSTDOCFLAGS="-D warnings" cargo doc --no-deps -p <núcleo…>
#    → falla si hay items públicos sin documentar o enlaces rotos
# 2. cargo test --doc -p <núcleo…>
#    → falla si algún ejemplo miente
```

CI lo ejecuta en cada push y cada PR (`.github/workflows/rust.yml`).
Nótese la asimetría deliberada: la puerta estricta cubre el núcleo
(hito 1, solo `std`); los adaptadores de frontera se documentan
igual pero su calidad se vigila con sus tests de contrato.


El primer test TDD es un doctest mínimo en la página del trait: crear el
tipo, llamar la función y fijar el resultado esperado. El rojo aparece como
error de compilación, enlace roto o `assert_eq!`; el verde exige que la API
documentada y la API real vuelvan a coincidir.

## 8. Frontera de producción

Un crate publicado en crates.io recibe esto gratis: docs.rs ejecuta
`cargo doc` y hospeda el resultado para siempre, por versión. Este
workspace no se publica, así que hacemos nosotros el papel de
docs.rs con GitHub Pages: `.github/workflows/docs.yml` construye
`cargo doc --workspace --no-deps` en cada push a `main` y despliega
`target/doc` tal cual (más una portada que redirige a
`memory_model`, el crate por el que empieza el tutorial).

El resultado, siempre fresco:
**<https://pmaojo.github.io/okf-mcp/>**

Cada capítulo de este tutorial enlaza al código fuente; la
referencia publicada es la otra mitad del mapa — la API vista desde
fuera, con la búsqueda de rustdoc (tecla `s`) como atajo. Para
estudiar las tripas privadas de un crate, genera la variante
completa en local:

```bash
cargo doc --no-deps --document-private-items --open
```

## 9. Principios SOLID en juego

- **L (Liskov), hecho literal otra vez.** El capítulo 9 convirtió el
  contrato en tests; este capítulo lo convierte en la PÁGINA del
  trait: la documentación de `MemoryRepository` enlaza al módulo
  `contract` y declara que toda implementación debe pasarlo. Quien
  llegue por la referencia publicada aterriza en las mismas reglas
  que el compilador y los tests hacen cumplir.
- **D (inversión de dependencias).** Documentar el trait, no la
  implementación: los ejemplos de `bounded_bfs` implementan
  `NeighborSource` con un `BTreeMap` de juguete, enseñando que el
  algoritmo no sabe nada del almacén — el mismo argumento del
  capítulo 5, ahora ejecutándose dentro de la documentación.
- **S (responsabilidad única).** Cada crate abre con un `//!` que
  cuenta SU historia y solo la suya ("este crate decide; no
  almacena, no hashea, no serializa"). Si el párrafo de cabecera de
  un crate necesita conjunciones para enumerar responsabilidades,
  sobra un crate o sobra una responsabilidad.

## 10. Ejercicios

1. **Guiado.** Rompe un doctest a propósito (cambia el `7` del
   ejemplo de `json_mini::parse` por un `6`) y ejecuta
   `cargo test --doc -p json-mini`. Lee el error completo: ¿qué
   línea del archivo reporta? ¿Por qué esa y no la del `assert`?
2. **Guiado.** Rompe un enlace intra-doc (`` [`Budget`] `` →
   `` [`Budgett`] ``) y ejecuta `./scripts/check-docs.sh`. Localiza
   el nombre del lint en la salida.
3. **Medio.** `Sha256::update` y `finalize` no tienen sección
   `# Ejemplo` propia. Escribe un doctest para `finalize` que
   demuestre que consumir el hasher (se toma por valor: `mut self`)
   impide usarlo dos veces — necesitarás `compile_fail`.
4. **Medio.** Añade `#![warn(missing_docs)]` a `mcp-http` y haz
   inventario: ¿cuántos items públicos sin documentar aparecen?
   Documenta los tres que más te costaron entender — esa dificultad
   es la señal de que la documentación faltaba.
5. **Abierto.** El ejercicio de diseño: la portada publicada
   redirige a `memory_model`, pero podría ser una página índice que
   cuente el mapa de crates por hitos, como el capítulo 0. Modifica
   el paso "Portada" de `docs.yml` para generarla (HTML a mano, sin
   dependencias — el espíritu del hito 1). ¿Qué debería enlazar
   primero una portada pensada para estudiar: los crates, o los
   capítulos?
