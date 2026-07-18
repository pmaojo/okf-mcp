# El día que el hasher no terminó — muestra de estilo libro

> **Qué es esto.** Una reescritura del capítulo 2 en voz narrativa,
> al estilo del *Ruby on Rails Tutorial* de Hartl: segunda persona,
> una historia continua, el código apareciendo cuando la historia lo
> pide. La versión plantilla sigue en
> [02-sha256-identidad.md](02-sha256-identidad.md); compara ambas y
> decide cuál quieres para el libro.

---

Llevas dos capítulos guardando documentos y todavía no puedes
responder la pregunta más simple del mundo: ¿estos dos documentos
son el mismo?

Podrías comparar los strings, claro. Funciona. Pero piensa en lo que
viene: vas a tener agentes editando en paralelo, y cada uno tendrá
que declarar "yo leí ESTA versión" antes de escribir. ¿Van a mandar
el documento entero como testigo? ¿Doscientos KiB de Markdown solo
para decir "la versión que vi"? Necesitas algo mejor: un nombre
corto que solo pueda pertenecer a un contenido. Treinta y dos bytes
que respondan por doscientos mil.

Eso es un hash criptográfico, y aquí viene la primera decisión
incómoda del capítulo: lo vamos a escribir a mano.

Sé lo que estás pensando, porque es lo que diría cualquier
ingeniero sensato: *la criptografía no se escribe a mano*. Y tienes
razón — a medias. La regla completa distingue para qué la usas. Si
verificas firmas de material AJENO bajo ataque (un JWT que llega de
internet), usas criptografía auditada, siempre, sin excepciones; los
canales laterales y los padding oracles no perdonan puertos
didácticos. Pero nosotros usamos SHA-256 para IDENTIDAD de contenido
propio: deduplicar y detectar ediciones concurrentes. Un SHA-256
correcto es correcto lo escriba quien lo escriba, y "correcto" aquí
es demostrable: el NIST publica los vectores oficiales. O tu
implementación los reproduce byte a byte, o no. No hay zona gris
donde esconderse — y por eso es el ejercicio perfecto para aprender.

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

Escribes `update`. La lógica se cae de madura: si había un bloque a
medias de la llamada anterior, complétalo primero; después traga
bloques enteros directamente del slice; lo que sobre, guárdalo para
la próxima. Escribes `finalize` con su padding del estándar: un byte
`0x80`, ceros hasta dejar sitio, la longitud en bits al final.
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
checker lo acepta encantado: préstamos de campos disjuntos no
entran en conflicto. El compilador no te estaba fastidiando; te
estaba señalando que la firma mentía sobre lo que la función
necesitaba. Esta escena se va a repetir: cuando Rust no te deja,
la pregunta correcta casi nunca es "cómo lo esquivo" sino "qué me
está diciendo de mi diseño".

## El tribunal

¿Y cómo sabes que TODO esto es correcto, no solo que no cuelga?
Aquí no hay opinión posible. FIPS 180-4 y los vectores CAVP del
NIST son el tribunal:

```rust
assert_eq!(
    hex(&sha256(b"abc")),
    "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
);
```

La suite completa incluye el string vacío, el clásico de dos bloques
y un millón de aes alimentadas de mil en mil — ese último ejercita
exactamente el camino incremental donde vivía nuestro bug. Desde el
capítulo 16, además, el vector de `"abc"` vive también en la
documentación publicada del crate: la página de `hash_core` no dice
que el hash funciona, lo demuestra en cada build.

Con el hasher demostrado, envolvemos los 32 bytes en un tipo con
nombre propio — `ContentId` — y ya tenemos la moneda del resto del
sistema: es lo que el almacén compara en cada escritura para
detectar al agente que llega tarde (capítulo 6), y lo que hace la
deduplicación gratis (dos documentos idénticos, un solo blob,
capítulo 7).

> 🧰 **La rueda de serie:** en producción usarías
> [`sha2`](https://docs.rs/sha2) — auditado, con SIMD, mantenido por
> RustCrypto — o [`blake3`](https://docs.rs/blake3) si el algoritmo
> lo eliges tú. Lo que acabas de escribir no compite con eso: te
> compró el derecho a saber qué hay dentro. El mapa completo:
> [La rueda de serie](la-rueda-de-serie.md).

---

*Fin de la muestra. Los ejercicios y las secciones de referencia
(memoria y asignación, SOLID) pueden sobrevivir como apéndice de
capítulo — el libro de Hartl hace exactamente eso con sus cajas y
ejercicios al final de cada sección.*
