# Capítulo 2 — SHA-256: la identidad es el contenido

Crate: [`crates/hash-core`](../crates/hash-core/src/lib.rs)

## 1. El problema

Necesitamos responder tres preguntas continuamente:

- ¿Estos dos documentos son EL MISMO contenido? (deduplicación)
- ¿El documento cambió desde que este agente lo leyó? (concurrencia)
- ¿Esta revisión es la que dice ser? (integridad)

Comparar strings completos funciona pero es O(n) por comparación y
obliga a transportar el documento entero para hablar de él. Un hash
criptográfico comprime la identidad a 32 bytes: si dos hashes
coinciden, el contenido coincide (con probabilidad tan alta que el
hardware fallará antes que el hash).

**Importante:** nuestro `ContentId` NO es un id de objeto Git. Git
hashea `"blob " + longitud + "\0" + contenido`, y además tiene formato
SHA-1 y SHA-256 según el repositorio. Definir nuestro propio tipo
(SHA-256 sobre los bytes exactos del Markdown, sin prefijos) nos
libera de imitar los detalles internos de Git — el worker de
sincronización del hito 4 ya creará blobs Git DE VERDAD con una
herramienta real.

## 2. El invariante

> **`content_id(doc)` depende de todos los bytes de `doc` y de nada
> más.** Ni del troceo con que llegaron, ni del orden de llamadas,
> ni de metadatos externos.

La segunda mitad del invariante es la interesante para Rust: el
hasher es INCREMENTAL (`update` × n + `finalize`), y aún así el
resultado debe ser idéntico al de un solo `update` gigante. Hay un
test dedicado a eso.

## 3. La implementación mínima

SHA-256 procesa el mensaje en bloques de 64 bytes sobre un estado de
8 palabras de 32 bits. La estructura del hasher refleja eso:

```rust
pub struct Sha256 {
    state: [u32; 8],      // el estado H
    buffer: [u8; 64],     // bloque parcial pendiente
    buffer_len: usize,
    total_len: u64,       // para el padding final
}
```

Tres piezas de Rust merecen atención:

**Aritmética con desbordamiento explícito.** SHA-256 se define en
aritmética módulo 2³², es decir, el desbordamiento no es un error:
es el algoritmo. En C esto pasa en silencio; en Rust debug un `+`
que desborda hace *panic*. Por eso cada suma del algoritmo es
`wrapping_add`:

```rust
let temp1 = h
    .wrapping_add(s1)
    .wrapping_add(ch)
    .wrapping_add(K[i])
    .wrapping_add(w[i]);
```

Rust te obliga a DECLARAR que quieres módulo 2³². El mismo lenguaje
que te da `checked_add` para los presupuestos (capítulo 5) te da
`wrapping_add` para la criptografía. La intención queda en el código.

**Rotaciones de bits.** `x.rotate_right(7)` existe en `std` y compila
a una sola instrucción. Compárala con el clásico
`(x >> 7) | (x << 25)` de C: la versión de Rust no tiene el bug
sutil de `x << 32` (comportamiento indefinido en C cuando la
rotación es 0).

**El truco del `update` en `finalize`.** El padding reutiliza el
propio `update` para el byte `0x80` y los ceros… pero la longitud
final se escribe DIRECTAMENTE en el buffer:

```rust
self.buffer[56..64].copy_from_slice(&bit_len.to_be_bytes());
```

¿Por qué? Porque `update` incrementa `total_len`, y la longitud que
debemos escribir es la del mensaje SIN el padding. Capturamos
`bit_len` antes de tocar nada. Este tipo de orden-importa es el pan
de cada día de los formatos binarios.

## 3.5. Conceptos de Rust en este capítulo

En este capítulo hemos visto estructuras de bajo nivel bastante interesantes:

* **Arrays (`[T; N]`) vs Slices (`&[T]`):** 
  * En `Sha256` definimos `state: [u32; 8]` y `buffer: [u8; 64]`. Son arrays de tamaño fijo y viven en el **stack** (la pila de memoria local rápida). Su tamaño no puede cambiar.
  * Sin embargo, `update` recibe `data: &[u8]`. Esto es un **slice** (rebanada), una vista de solo lectura que apunta a cualquier cantidad de bytes que estén guardados en otra parte (ya sea un array o un vector). Siempre se usan con `&` porque su tamaño no se conoce en tiempo de compilación.
* **Aritmética de desbordamiento (`wrapping_add`):** Por seguridad, si una operación aritmética clásica como `a + b` supera el valor máximo del tipo (2³² - 1 para `u32`), Rust detiene el programa en seco (*panic*) en modo de depuración para evitar bugs de corrupción. Como el algoritmo SHA-256 requiere que los números den la vuelta al desbordar, usamos `wrapping_add`, que realiza aritmética modular sin provocar errores.
* **Métodos integrados (`to_be_bytes`, `rotate_right`):** En Rust, los tipos numéricos primitivos tienen métodos muy útiles. Por ejemplo, `bit_len.to_be_bytes()` convierte un `u64` en un array de bytes `[u8; 8]` en formato Big Endian (el orden estándar de bytes en red). `rotate_right(7)` rota los bits del número a la derecha de forma segura e inmediata, compilando directamente a la instrucción nativa del procesador.

## 4. Una versión deliberadamente rota

```rust
// ❌ NO HACER: hashear la concatenación de piezas con separador
pub fn identidad(frontmatter: &str, body: &str) -> [u8; 32] {
    sha256(format!("{frontmatter}|{body}").as_bytes())
}
```

## 5. Por qué falla

Es el clásico **ataque de ambigüedad de concatenación**: las parejas
`("a|b", "c")` y `("a", "b|c")` producen la misma entrada `a|b|c` y
por tanto la misma identidad para documentos distintos. Toda
identidad compuesta necesita enmarcar longitudes (como hace Git con
su cabecera) o, mejor, no ser compuesta: nosotros hasheamos **los
bytes exactos y completos del documento**, que es la representación
canónica por definición. No hay piezas, no hay ambigüedad.

El segundo fallo típico es más sutil: hashear una versión
NORMALIZADA (sin espacios finales, con claves YAML reordenadas…).
Entonces dos bytes distintos comparten id, el almacén deduplica y
devuelve al agente un documento distinto byte a byte del que
escribió. Regla del proyecto: **los bytes originales son la verdad**
(capítulo 4); el hash respeta esa regla hasheando exactamente esos
bytes.

## 6. Memoria y asignación

El hasher usa memoria CONSTANTE: 64 bytes de buffer + 32 de estado +
la palabra `w` de 256 bytes en el stack durante `compress`. Hashear
un documento de 256 KiB no asigna nada en el heap. Por eso `update`
recibe `&[u8]` y procesa `chunks_exact(64)` directamente del slice
prestado: los bloques completos ni siquiera pasan por el buffer.

## 7. Tests

Dos familias:

```rust
#[test]
fn vectores_nist() { ... }            // "", "abc", el vector largo, 1M de 'a'
#[test]
fn independiente_del_troceo() { ... } // update(x[..i]) + update(x[i..]) == update(x)
```

Los vectores de NIST son la única manera honesta de testear
criptografía escrita a mano: no puedes "razonar" que 64 rondas de
bits están bien; las comparas con la referencia oficial. El test del
millón de `'a'` en trozos de 1000 golpea justo donde viven los bugs:
las costuras entre bloques.

## 8. Frontera de producción

Para IDENTIDAD DE CONTENIDO, este código puede quedarse: es correcto
(vectores NIST) y su rendimiento es adecuado para documentos de
cientos de KiB. Aun así, en el hito 2 lo natural será medirlo contra
`sha2` (el crate auditado con SIMD) y decidir con números.

Para lo que NO puede quedarse jamás: verificación de firmas JWT. La
criptografía de AUTENTICACIÓN (RSA, ECDSA, comparaciones en tiempo
constante) tiene clases enteras de ataques —canales laterales,
padding oracles— que un port didáctico no mitiga. `auth-adapter`
(hito 3) usará criptografía auditada. La regla que lo resume:

> Hash de contenido propio: puedes escribirlo y demostrarlo con
> vectores. Verificación de material ajeno bajo ataque: cómpralo
> auditado.

## 9. Principios SOLID en juego

- **S:** `hash-core` transforma bytes en 32 bytes. No sabe qué es un
  documento, un concepto ni un commit. Podrías extraerlo a cualquier
  otro proyecto sin arrastrar nada.
- **D:** ¿quién conoce a quién? `memory-store` (capítulo 7) llama a
  `sha256`; `hash-core` no importa nada de nadie. Las utilidades
  puras van al fondo de la pila de dependencias.
- **L (contraste):** fíjate en que aquí NO hay trait `Hasher`
  intercambiable. Sería fácil añadirlo… y sería mentira: el hash no
  es intercambiable, porque los `ContentId` almacenados dependen de
  él para siempre. Cambiar de algoritmo es una MIGRACIÓN de datos,
  no un swap de implementación. SOLID también es saber cuándo NO
  abstraer.

## 10. Ejercicios

1. **Guiado.** Implementa `sha256_hex(data: &[u8]) -> String` usando
   el `ContentId::to_hex` del capítulo 1 como referencia. ¿Cuántas
   asignaciones hace tu versión?
2. **Medio.** Rompe el padding a propósito: escribe la longitud en
   little-endian (`to_le_bytes`). ¿Qué tests fallan? ¿Fallaría el
   test de troceo? Explica por qué sí o por qué no ANTES de probarlo.
3. **Abierto.** Mide `hash-core` contra el crate `sha2` con un
   documento de 1 MiB (en un proyecto aparte, para no romper la regla
   de cero dependencias). ¿Cuál es la diferencia y de dónde sale?
   (pista: busca "SHA-NI" y "sha2 asm").

Siguiente: [Capítulo 3 — JSON a mano: el precio del texto](03-json-a-mano.md).
