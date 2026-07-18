# Capítulo 1 — Identificadores que no pueden hacer daño

Crate: [`crates/memory-model`](../crates/memory-model/src/lib.rs) ·
[referencia](https://pmaojo.github.io/okf-mcp/memory_model/)

Todo lo que este servidor va a hacer gira alrededor de un
identificador de concepto: `people/alice`, `projects/okf-mcp`. Parece
el lugar más aburrido posible para empezar un libro. Es exactamente
el contrario, y para verlo solo hace falta seguirle la pista a ese
string: llega **del exterior** — de un agente de IA, es decir, de un
texto generado por un modelo — y viaja por todo el sistema: al
almacén, al grafo, a las URIs `okf://...`, y dentro de unos capítulos
a consultas SQL y claves de objetos en la nube.

Ahora imagina que en algún rincón del sistema, dentro de un año,
alguien concatena ese texto con una ruta, una URL o una consulta. Un
identificador malicioso como `../../etc/passwd` o `alice\0.md` deja
de ser un nombre y se convierte en una vulnerabilidad. La historia de
la seguridad informática es, en buena parte, la historia de strings
que viajaron más lejos de lo que nadie validó.

La respuesta habitual es "validar en la entrada". La respuesta de
este capítulo es más ambiciosa:

> **Si existe un valor de tipo `ConceptId`, entonces es válido.**
> No "probablemente válido", no "validado en el controlador de
> entrada": válido por construcción, porque es IMPOSIBLE crear uno
> inválido.

## Un tipo con una sola puerta

A este patrón se le llama *newtype con validación en la frontera* y
es posiblemente el más rentable de todo Rust. Cabe en cinco líneas:

```rust
pub struct ConceptId(String);          // el campo es PRIVADO

impl ConceptId {
    pub fn parse(s: &str) -> Result<Self, ConceptIdError> { ... }
    pub fn as_str(&self) -> &str { ... }
}
```

El campo interno es privado y la única puerta de entrada es
`parse()`. Con eso, el compilador convierte el invariante en un
teorema: toda función que reciba un `ConceptId` puede asumir la
validez **sin comprobar nada**, porque no existe ningún camino del
universo que produzca un `ConceptId` sin pasar por `parse`. No es
una promesa del equipo; es un hecho del sistema de tipos. (Tan hecho
es, que la documentación del crate lo demuestra con un test que
exige que `ConceptId("../etc".to_string())` NO compile — capítulo
16.)

¿Y qué comprueba `parse`? Aquí viene la segunda decisión con
moraleja: la validación es por **lista blanca**. Enumeramos lo
permitido y rechazamos todo lo demás. Las listas negras ("prohibir
`..`") siempre olvidan algo: ¿`%2e%2e`? ¿`..\`? ¿homoglifos
Unicode?

```rust
pub fn parse(s: &str) -> Result<Self, ConceptIdError> {
    if s.is_empty() {
        return Err(ConceptIdError::Empty);
    }
    if s.len() > MAX_CONCEPT_ID_LEN {
        return Err(ConceptIdError::TooLong { len: s.len(), max: MAX_CONCEPT_ID_LEN });
    }
    if s.starts_with('/') || s.ends_with('/') {
        return Err(ConceptIdError::LeadingOrTrailingSlash);
    }
    for (offset, byte) in s.bytes().enumerate() {
        let ok = byte.is_ascii_lowercase()
            || byte.is_ascii_digit()
            || byte == b'-' || byte == b'_' || byte == b'/';
        if !ok {
            return Err(ConceptIdError::InvalidByte { byte, offset });
        }
    }
    for segment in s.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err(ConceptIdError::InvalidSegment { segment: segment.to_string() });
        }
    }
    Ok(ConceptId(s.to_string()))
}
```

Tres detalles de este código te van a acompañar todo el libro.

**Los errores llevan datos** (`byte`, `offset`, `segment`). Un error
que solo dice "inválido" obliga a depurar con print; uno que dice
"byte 0x2e en la posición 3" se explica solo. En un servidor para
agentes de IA esto importa el doble: quien lee el error es un
MODELO, y con la posición exacta puede corregir su siguiente
intento.

**`s.bytes()` y no `s.chars()`.** Como la lista blanca es ASCII
puro, cualquier byte ≥ 0x80 (el comienzo de un carácter multibyte)
cae rechazado automáticamente. Validar bytes es más simple y más
estricto a la vez.

**La comprobación de segmentos parece redundante** — el punto ya
está fuera de la lista blanca de bytes — pero protege el invariante
frente a EDICIONES FUTURAS: si alguien añade `.` a la lista blanca
dentro de un año, el test de `a/../b` seguirá fallando. Defensa en
profundidad contra tu yo del futuro.

## La versión que habrías escrito (y que el mundo escribe)

Así se valida en miles de servicios reales, hoy:

```rust
// ❌ NO HACER: lista negra + validación separada del tipo
pub fn es_valido(s: &str) -> bool {
    !s.contains("..") && !s.contains('\\')
}

// y en otro archivo, semanas después…
let ruta = format!("documents/{}", concept_id_sin_tipo);
```

Se lee razonable. Veamos tres inputs concretos derribarla.

Primero, `"a/.%2e/b"`: la lista negra busca `..` literal y la
versión percent-encoded pasa limpia. Si CUALQUIER capa posterior —
un proxy, un SDK de storage — decodifica `%2e` en `.`, el traversal
revive lejos del código que "validó".

Segundo, `"a//../b"` frente a normalizadores intermedios: algunos
colapsan `//` ANTES de resolver `..`, otros después. La lista negra
validó un string distinto del que acabó usándose.

Y tercero, el fallo estructural, el que de verdad importa:
`es_valido` devuelve un `bool`, y un `bool` no viaja con el string.
En el commit 400 del proyecto, alguien añade un endpoint nuevo y
olvida llamar a `es_valido`. El compilador no protesta — un string
es un string. Con el newtype, ese endpoint ni siquiera compila:
necesita un `ConceptId` y solo `parse()` fabrica uno.

La diferencia entre las dos versiones no es de rigor, es de **quién
vigila**: en la rota vigila la disciplina del equipo; en la buena
vigila `rustc`. La disciplina se cansa. `rustc` no.

Los tests del crate cierran el círculo enumerando ATAQUES, no solo
formatos — cada assert es un CVE histórico condensado en una línea:

```rust
#[test]
fn concept_id_rechaza_traversal_y_bytes_raros() {
    assert!(matches!(ConceptId::parse("../etc"), Err(ConceptIdError::InvalidByte { .. })));
    assert_eq!(ConceptId::parse("/abs"), Err(ConceptIdError::LeadingOrTrailingSlash));
    assert!(matches!(ConceptId::parse("a//b"), Err(ConceptIdError::InvalidSegment { .. })));
    assert!(matches!(ConceptId::parse("a\\b"), Err(ConceptIdError::InvalidByte { .. })));
    assert!(matches!(ConceptId::parse("a\0b"), Err(ConceptIdError::InvalidByte { .. })));
    assert!(matches!(ConceptId::parse("a%2e%2e"), Err(ConceptIdError::InvalidByte { .. })));
    // ...
}
```

## La frontera de producción

> 🧰 **La rueda de serie:** en producción, el boilerplate de newtypes validados lo quitan [`nutype`](https://docs.rs/nutype) o [`validator`](https://docs.rs/validator). El mapa completo y el criterio para elegir: [La rueda de serie](la-rueda-de-serie.md).

Y aquí, una rareza que no volverá a repetirse en el libro: nada
cambia en producción. Este código ES el de producción.
`memory-model` no tiene dependencias que sustituir. Lo único que se
añade en el hito 2 es presión: el `ConceptId` viajará a SQL (como
parámetro tipado, jamás concatenado) y a claves de Supabase Storage,
y este capítulo es la razón de que eso sea seguro.

---

## Apéndice del capítulo

### Conceptos de Rust

Si vienes de otro lenguaje, los detalles de la sintaxis usada:

* **Tuple Struct (`pub struct ConceptId(String);`):** una estructura de tupla que envuelve un solo tipo. Como el campo `String` no tiene nombre y es privado (no lleva `pub` dentro del paréntesis), nadie fuera de este módulo puede acceder al string interno ni crear un `ConceptId` haciendo `ConceptId("invalido")`. Esto obliga a usar `ConceptId::parse()`.
* **`Result<Self, ConceptIdError>` y la palabra clave `Self`:** `Result` es el enum estándar para errores. `Self` (con S mayúscula) es un alias del tipo sobre el que estamos implementando (aquí, `ConceptId`).
* **`&str` vs `String`:** `parse` recibe `s: &str` (un préstamo de solo lectura) para validar sin gastar memoria. Si todo está correcto, `s.to_string()` copia los caracteres a un bloque nuevo del heap, propiedad del nuevo `ConceptId`.
* **Iterar con `s.bytes().enumerate()`:** `.bytes()` da los bytes uno a uno y `.enumerate()` añade un contador desde cero. La sintaxis `for (offset, byte)` desestructura la pareja en cada iteración.
* **La macro `matches!`:** pregunta si algo encaja con un patrón y devuelve `true`/`false`. Por ejemplo, `matches!(resultado, Err(ConceptIdError::InvalidByte { .. }))`.

### Memoria y asignación

`parse()` hace exactamente **una** asignación: el `to_string()`
final, y solo en el caso de éxito (los caminos de error con segmento
asignan el segmento para el mensaje, aceptable en el camino frío).
Las validaciones recorren `&str` prestado sin copiar nada. El coste:
O(n) tiempo, una asignación, y n está acotado por
`MAX_CONCEPT_ID_LEN = 200` **comprobado antes** de tocar memoria.

Ese orden — límite primero, trabajo después — reaparece en cada
capítulo. Es la regla número uno del servidor sin sorpresas de RAM.

### SOLID en juego

- **S:** `memory-model` define QUÉ es un identificador; no sabe
  parsear documentos ni servir protocolos. Su única razón de cambio
  es que cambien las reglas del dominio.
- **L (preparándolo):** todos los contratos que veremos
  (`MemoryRepository`, `NeighborSource`) hablan en `ConceptId`, no en
  `String`. Las implementaciones intercambiables del capítulo 7
  pueden serlo PORQUE el tipo garantiza las precondiciones: ninguna
  implementación necesita re-validar, así que ninguna puede divergir
  en cómo valida.
- **D (anticipo):** este crate está en el fondo de la pila de
  dependencias. Todo apunta hacia él; él no apunta a nada.

### Ejercicios

1. **Guiado.** Añade el método `ConceptId::parent(&self) -> Option<ConceptId>`
   que devuelva `people` para `people/alice`. ¿Puedes construir el
   resultado SIN volver a validar? Justifica por qué es seguro
   (pista: ¿qué invariantes de un id válido hereda todo prefijo que
   termina en frontera de segmento?).
2. **Medio.** Cambia `MAX_CONCEPT_ID_LEN` a 10 y ejecuta la suite.
   ¿Qué tests fallan y qué te dice eso sobre acoplar tests a
   constantes?
3. **Abierto.** Diseña (sin implementar) un `ConceptId` que permita
   Unicode (títulos en español con ñ). Enumera: qué normalización
   (NFC/NFKC), qué pasa con homoglifos (`а` cirílica vs `a` latina),
   y por qué este proyecto eligió ASCII. ¿Qué opinaría un usuario
   japonés de esa decisión?

Siguiente: [Capítulo 2 — SHA-256: la identidad es el contenido](02-sha256-identidad.md).
