# Capítulo 2 — SHA-256: la identidad es el contenido

Crate: [`crates/hash-core`](../crates/hash-core/src/lib.rs) ·
[referencia](https://pmaojo.github.io/okf-mcp/hash_core/)

Llevas un capítulo poniendo nombre a los conceptos y todavía no
puedes responder la pregunta más simple del mundo: ¿estos dos
documentos son el mismo?

Podrías comparar los strings, claro. Funciona. Pero piensa en lo que
viene: vas a tener agentes editando en paralelo, y cada uno tendrá
que declarar "yo leí ESTA versión" antes de escribir. ¿Van a mandar
el documento entero como testigo? ¿Doscientos KiB de Markdown solo
para decir "la versión que vi"? Necesitas algo mejor: un nombre
corto que solo pueda pertenecer a un contenido. Treinta y dos bytes
que respondan por doscientos mil.

Eso es un hash criptográfico, y aquí viene la primera decisión
incómoda del capítulo: lo vamos a escribir a mano.

Sé lo que estás pensando, porque es lo que diría cualquier ingeniero
sensato: *la criptografía no se escribe a mano*. Y tienes razón — a
medias. La regla completa distingue para qué la usas. Si verificas
firmas de material AJENO bajo ataque (un JWT que llega de internet),
usas criptografía auditada, siempre, sin excepciones; los canales
laterales y los padding oracles no perdonan puertos didácticos. Pero
nosotros usamos SHA-256 para IDENTIDAD de contenido propio:
deduplicar y detectar ediciones concurrentes. Un SHA-256 correcto es
correcto lo escriba quien lo escriba, y "correcto" aquí es
demostrable: el NIST publica los vectores oficiales. O tu
implementación los reproduce byte a byte, o no. No hay zona gris
donde esconderse — y por eso es el ejercicio perfecto para aprender.

Un apunte antes de arrancar: nuestro `ContentId` NO es un id de
objeto Git. Git hashea `"blob " + longitud + "\0" + contenido`, y
además alterna SHA-1 y SHA-256 según el repositorio. Definir nuestro
propio tipo — SHA-256 sobre los bytes exactos del Markdown, sin
prefijos — nos libera de imitar los detalles internos de Git; el
worker de sincronización del hito 4 ya creará blobs Git DE VERDAD
con una herramienta real.

## El estado que viaja en el tiempo

SHA-256 procesa la entrada en bloques de 64 bytes que van amasando
un estado de ocho enteros de 32 bits. La consecuencia de diseño es
inmediata: no necesitas el documento entero en memoria. Puedes
alimentar el hasher a trozos, según llegan, y pedirle el resultado
al final:

```rust
pub struct Sha256 {
    state: [u32; 8],      // el estado H que se va amasando
    buffer: [u8; 64],     // bloque parcial pendiente
    buffer_len: usize,
    total_len: u64,       // bytes totales, para el padding final
}
```

La API es tres verbos: `new()`, `update(&[u8])` tantas veces como
quieras, `finalize()` una sola. Y el invariante que lo gobierna todo
cabe en una frase: **el digest depende de los bytes y de nada más**.
Ni del troceo con que llegaron, ni del número de llamadas a
`update`. `update(b"hola mundo")` y `update(b"hola ") · update(b"mundo")`
tienen que dar, bit a bit, lo mismo.

Por dentro, el algoritmo es 64 rondas de aritmética de bits, y Rust
tiene tres cosas que decir al respecto.

**El desbordamiento se declara.** SHA-256 se define en aritmética
módulo 2³²: el desbordamiento no es un error, es el algoritmo. En C
esto pasa en silencio; en Rust, un `+` que desborda hace *panic* en
modo debug. Por eso cada suma del algoritmo lo dice explícitamente:

```rust
let temp1 = h
    .wrapping_add(s1)
    .wrapping_add(ch)
    .wrapping_add(K[i])
    .wrapping_add(w[i]);
```

El mismo lenguaje que te dará `checked_add` para los presupuestos
(capítulo 5) te da `wrapping_add` para la criptografía. La intención
queda escrita en el código, no en la cabeza de quien lo escribió.

**Las rotaciones vienen de serie.** `x.rotate_right(7)` existe en
`std` y compila a una sola instrucción. El clásico
`(x >> 7) | (x << 25)` de C arrastra un bug sutil — `x << 32` es
comportamiento indefinido cuando la rotación es 0 — que aquí
simplemente no puede escribirse.

**El orden importa.** El padding final reutiliza el propio `update`
para el byte `0x80` y los ceros… pero la longitud se escribe
DIRECTAMENTE en el buffer:

```rust
self.buffer[56..64].copy_from_slice(&bit_len.to_be_bytes());
```

¿Por qué? Porque `update` incrementa `total_len`, y la longitud que
debemos escribir es la del mensaje SIN el padding. Capturamos
`bit_len` antes de tocar nada. Este orden-importa es el pan de cada
día de los formatos binarios.

Con eso escribes `update`. La lógica se cae de madura: si había un
bloque a medias de la llamada anterior, complétalo primero; después
traga bloques enteros directamente del slice; lo que sobre,
guárdalo para la próxima. Escribes `finalize` con su padding.
Compila a la primera... bueno, a la tercera. Corres el test.

Y el test no falla. El test **no termina**.

## La depuración

Un test que cuelga es más interesante que un test rojo, porque no te
da ni el consuelo del mensaje de error. Añades un `eprintln!` en
`finalize` y ves el padding dando vueltas: `buffer_len` vale 3, tú
empujas un cero, y vale 3 otra vez. Empujas otro. Tres. El bucle
espera llegar a 56 y `buffer_len` no se mueve.

Miras `update` con otros ojos y ahí está. El caso "completar el
bloque parcial previo" termina, cuando el bloque se llena, con un
`compress` y un `buffer_len = 0`. Pero ¿y si los bytes nuevos NO
llenan el bloque? Entonces el control sigue hacia el paso siguiente,
el de "guardar el resto"... que calcula el resto desde el slice ya
consumido — vacío — y escribe `buffer_len = 0`. Acabas de borrar la
cuenta de los bytes que acababas de guardar. Por eso el padding
nunca avanza: cada cero que entra se descuenta a sí mismo.

La corrección es un `return` temprano de dos líneas — si todo cupo
en el bloque parcial, hemos terminado — y el comentario que lo
custodia en `hash-core` lo dice sin rodeos:

```rust
// Si todo cupo en el bloque parcial, terminamos AQUÍ.
// Sin este return, el paso 3 machacaría buffer_len con
// el resto vacío (= 0) y perdería los bytes pendientes.
// Ese bug exacto colgaba finalize() en un bucle infinito.
if data.is_empty() {
    return;
}
```

Fíjate en lo que acaba de pasar, porque es el patrón de todo el
libro: el invariante ("independiente del troceo") no era una frase
bonita de documentación. Era un test — hay uno que trocea la misma
entrada por cuatro puntos distintos y exige el mismo digest — y el
test convirtió un bug sutil de estado en un fallo reproducible en
milisegundos.

## El préstamo que no te dejan pedir

Queda una pelea con el compilador, y es de las buenas. `compress`
necesita dos cosas: el estado (mutable) y el bloque (solo lectura).
Tu primer instinto es un método:

```rust
fn compress(&mut self, block: &[u8; 64])   // ← no va a poder ser
```

Pero al llamarlo desde `update` con `self.compress(&self.buffer)`,
el borrow checker te para: llamar al método pide prestado `self`
ENTERO en mutable, y `&self.buffer` lo pide en inmutable a la vez.
Dos préstamos incompatibles sobre la misma cosa. La salida fácil es
copiar el buffer a una variable local — 64 bytes copiados en cada
bloque solo para esquivar al compilador. La salida elegante es
contarle la verdad al compilador: `compress` no necesita `self`,
necesita dos campos DISTINTOS de `self`:

```rust
fn compress(state: &mut [u32; 8], block: &[u8; 64])
```

Como función asociada, la llamada es
`Self::compress(&mut self.state, &self.buffer)`, y eso el borrow
checker lo acepta encantado: préstamos de campos disjuntos no entran
en conflicto. El compilador no te estaba fastidiando; te estaba
señalando que la firma mentía sobre lo que la función necesitaba.
Esta escena se va a repetir: cuando Rust no te deja, la pregunta
correcta casi nunca es "cómo lo esquivo" sino "qué me está diciendo
de mi diseño".

## El tribunal

¿Y cómo sabes que TODO esto es correcto, no solo que no cuelga? Aquí
no hay opinión posible. FIPS 180-4 y los vectores CAVP del NIST son
el tribunal:

```rust
assert_eq!(
    hex(&sha256(b"abc")),
    "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
);
```

Los vectores oficiales son la única manera honesta de testear
criptografía escrita a mano: no puedes "razonar" que 64 rondas de
bits están bien; las comparas con la referencia. La suite completa
incluye el string vacío, el clásico de dos bloques y un millón de
aes alimentadas de mil en mil — ese último golpea justo donde viven
los bugs: las costuras entre bloques, donde vivía el nuestro. Desde
el capítulo 16, además, el vector de `"abc"` vive también en la
documentación publicada del crate: la página de `hash_core` no dice
que el hash funciona, lo demuestra en cada build.

## La otra manera de equivocarse

El bug del bucle infinito era ruidoso. Hay una familia de errores de
identidad mucho más silenciosa, y conviene verla antes de dar el
capítulo por ganado. Supón que alguien, razonablemente, decide
hashear el documento "por piezas":

```rust
// ❌ NO HACER: hashear la concatenación de piezas con separador
pub fn identidad(frontmatter: &str, body: &str) -> [u8; 32] {
    sha256(format!("{frontmatter}|{body}").as_bytes())
}
```

Es el clásico **ataque de ambigüedad de concatenación**: las parejas
`("a|b", "c")` y `("a", "b|c")` producen la misma entrada `a|b|c` y
por tanto la misma identidad para documentos distintos. Toda
identidad compuesta necesita enmarcar longitudes (como hace Git con
su cabecera) o, mejor, no ser compuesta. Nosotros hasheamos **los
bytes exactos y completos del documento**: no hay piezas, no hay
ambigüedad.

El segundo fallo típico es aún más sutil: hashear una versión
NORMALIZADA (sin espacios finales, con claves YAML reordenadas…).
Entonces dos bytes distintos comparten id, el almacén deduplica, y
un día le devuelve al agente un documento distinto byte a byte del
que escribió. Regla del proyecto, que el capítulo 4 elevará a ley:
**los bytes originales son la verdad**. El hash la respeta hasheando
exactamente esos bytes.

Con el hasher demostrado, envolvemos los 32 bytes en el tipo con
nombre del capítulo 1 — `ContentId` — y ya tenemos la moneda del
resto del sistema: es lo que el almacén comparará en cada escritura
para detectar al agente que llega tarde (capítulo 6), y lo que hará
la deduplicación gratis (dos documentos idénticos, un solo blob,
capítulo 7).

## La frontera de producción

> 🧰 **La rueda de serie:** en producción, [`sha2`](https://docs.rs/sha2) (auditado, SIMD) o [`blake3`](https://docs.rs/blake3) si el algoritmo lo eliges tú. El mapa completo y el criterio para elegir: [La rueda de serie](la-rueda-de-serie.md).

Para IDENTIDAD DE CONTENIDO, este código puede quedarse: es correcto
(vectores NIST) y su rendimiento es adecuado para documentos de
cientos de KiB. Aun así, en el hito 2 lo natural será medirlo contra
`sha2` y decidir con números.

Para lo que NO puede quedarse jamás: verificación de firmas JWT. La
criptografía de AUTENTICACIÓN (RSA, ECDSA, comparaciones en tiempo
constante) tiene clases enteras de ataques — canales laterales,
padding oracles — que un port didáctico no mitiga. El hito 3 usará
criptografía auditada. La regla que lo resume:

> Hash de contenido propio: puedes escribirlo y demostrarlo con
> vectores. Verificación de material ajeno bajo ataque: cómpralo
> auditado.

---

## Apéndice del capítulo

### Conceptos de Rust

* **Arrays (`[T; N]`) vs Slices (`&[T]`):**
  * En `Sha256` definimos `state: [u32; 8]` y `buffer: [u8; 64]`. Son arrays de tamaño fijo y viven en el **stack** (la pila de memoria local rápida). Su tamaño no puede cambiar.
  * Sin embargo, `update` recibe `data: &[u8]`. Esto es un **slice** (rebanada), una vista de solo lectura que apunta a cualquier cantidad de bytes que estén guardados en otra parte (un array o un vector). Siempre se usa con `&` porque su tamaño no se conoce en tiempo de compilación.
* **Aritmética de desbordamiento (`wrapping_add`):** si una operación clásica como `a + b` supera el máximo del tipo (2³² − 1 para `u32`), Rust detiene el programa (*panic*) en modo de depuración para evitar corrupciones. Como SHA-256 requiere que los números den la vuelta, usamos `wrapping_add`: aritmética modular declarada.
* **Métodos integrados (`to_be_bytes`, `rotate_right`):** los tipos numéricos primitivos traen métodos muy útiles. `bit_len.to_be_bytes()` convierte un `u64` en `[u8; 8]` en Big Endian (el orden de red); `rotate_right(7)` rota bits de forma segura y compila a la instrucción nativa.

### Memoria y asignación

El hasher usa memoria CONSTANTE: 64 bytes de buffer + 32 de estado +
la palabra `w` de 256 bytes en el stack durante `compress`. Hashear
un documento de 256 KiB no asigna nada en el heap. Por eso `update`
recibe `&[u8]` y procesa `chunks_exact(64)` directamente del slice
prestado: los bloques completos ni siquiera pasan por el buffer.

### SOLID en juego

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

### Ejercicios

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
