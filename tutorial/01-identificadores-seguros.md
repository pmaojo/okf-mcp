# Capítulo 1 — Identificadores que no pueden hacer daño

Crate: [`crates/memory-model`](../crates/memory-model/src/lib.rs)

## 1. El problema

Todo lo que el servidor hace gira alrededor de un identificador de
concepto: `people/alice`, `projects/okf-mcp`. Ese identificador llega
**del exterior** (de un agente de IA, es decir, de un texto generado)
y viaja por todo el sistema: al almacén, al grafo, a las URIs
`okf://...`, y en el hito 2 a consultas SQL y claves de objetos.

Si en algún rincón del sistema alguien concatena ese texto con una
ruta, una URL o una consulta, un identificador malicioso como
`../../etc/passwd` o `alice\0.md` se convierte en una vulnerabilidad.
La historia de la seguridad informática es, en buena parte, la
historia de strings que viajaron más lejos de lo que nadie validó.

## 2. El invariante

> **Si existe un valor de tipo `ConceptId`, entonces es válido.**
> No "probablemente válido", no "validado en el controlador de
> entrada": válido por construcción, porque es IMPOSIBLE crear uno
> inválido.

A esto se le llama *newtype con validación en la frontera* y es
posiblemente el patrón más rentable de todo Rust. Fíjate en la firma:

```rust
pub struct ConceptId(String);          // el campo es PRIVADO

impl ConceptId {
    pub fn parse(s: &str) -> Result<Self, ConceptIdError> { ... }
    pub fn as_str(&self) -> &str { ... }
}
```

El campo interno es privado y la única puerta de entrada es
`parse()`. El compilador convierte el invariante en un teorema: toda
función que reciba `ConceptId` puede asumir la validez **sin comprobar nada**,
porque no existe ningún camino del universo que
produzca un `ConceptId` sin pasar por `parse`.

## 3. La implementación mínima

La validación es por **lista blanca**: enumeramos lo permitido y
rechazamos todo lo demás. Las listas negras ("prohibir `..`") siempre
olvidan algo (¿`%2e%2e`? ¿`..\\`? ¿Unicode homoglifos?).

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

Detalles que merecen mirada:

- **Los errores llevan datos** (`byte`, `offset`, `segment`). Un
  error que solo dice "inválido" obliga a depurar con print; uno que
  dice "byte 0x2e en la posición 3" se explica solo. En un servidor
  para agentes de IA esto importa el doble: el MODELO lee el error y
  corrige su siguiente intento.
- **`s.bytes()` y no `s.chars()`**: como la lista blanca es ASCII
  puro, cualquier byte ≥ 0x80 (comienzo de un carácter multibyte)
  cae rechazado automáticamente. Validar bytes es más simple y más
  estricto a la vez.
- La comprobación de segmentos parece redundante — `.` ya está
  fuera de la lista blanca de bytes — pero protege el invariante
  frente a EDICIONES FUTURAS: si alguien añade `.` a la lista blanca
  dentro de un año, el test de `a/../b` seguirá fallando. Defensa en
  profundidad contra tu yo del futuro.

## 3.5. Conceptos de Rust en este capítulo

Si vienes de otro lenguaje, aquí tienes los detalles de la sintaxis usada:

* **Tuple Struct (`pub struct ConceptId(String);`):** Es una estructura de tupla que envuelve un solo tipo. Como el campo `String` no tiene nombre y es privado (no lleva la palabra `pub` dentro del paréntesis), nadie fuera de este módulo puede acceder al string interno ni crear un `ConceptId` directamente haciendo `ConceptId("invalido")`. Esto obliga a usar `ConceptId::parse()`.
* **`Result<Self, ConceptIdError>` y la palabra clave `Self`:** `Result` es el enum estándar para errores. `Self` (con la primera S mayúscula) es simplemente un alias que apunta al tipo sobre el que estamos implementando el método (en este caso, `ConceptId`).
* **`&str` vs `String`:** `parse` recibe `s: &str` (un préstamo de solo lectura de los caracteres) para validar sin gastar memoria. Si todo está correcto, hacemos `s.to_string()` que copia esos caracteres en un nuevo bloque de memoria en el heap (memoria dinámica) propiedad del nuevo `ConceptId`.
* **Iterar con `s.bytes().enumerate()`:** `.bytes()` nos da los bytes ASCII de la cadena uno a uno, y `.enumerate()` añade un contador que empieza en cero. La sintaxis `for (offset, byte)` desestructura esa pareja automáticamente en cada iteración para saber en qué posición exacta estamos.
* **La macro `matches!`:** Es una forma abreviada de preguntar si algo coincide con un patrón. Devuelve `true` o `false`. Por ejemplo, `matches!(resultado, Err(ConceptIdError::InvalidByte { .. }))` verifica si la operación devolvió un error de tipo `InvalidByte`.

## 4. Una versión deliberadamente rota

Así se valida en miles de servicios del mundo real:

```rust
// ❌ NO HACER: lista negra + validación separada del tipo
pub fn es_valido(s: &str) -> bool {
    !s.contains("..") && !s.contains('\\')
}

// y en otro archivo, semanas después…
let ruta = format!("documents/{}", concept_id_sin_tipo);
```

## 5. Por qué falla

Tres inputs concretos:

1. `"a/.%2e/b"` — la lista negra busca `..` literal; la versión
   percent-encoded pasa. Si CUALQUIER capa posterior (un proxy, un
   SDK de storage) decodifica `%2e` → `.`, el traversal revive.
2. `"a//../b"` con normalizadores intermedios: algunos colapsan
   `//` ANTES de resolver `..`, otros después. La lista negra validó
   un string distinto del que se usó.
3. El fallo estructural: `es_valido` devuelve `bool` y un `bool` no
   viaja con el string. En el commit 400 de un proyecto, alguien
   añade un endpoint nuevo y olvida llamar a `es_valido`. El
   compilador no protesta. Con el newtype, ese endpoint no compila:
   necesita un `ConceptId` y solo `parse()` fabrica uno.

La diferencia no es de rigor, es de **quién vigila**: en la versión
rota vigila la disciplina del equipo; en la buena vigila `rustc`.

## 6. Memoria y asignación

`parse()` hace exactamente **una** asignación: el `to_string()`
final, y solo en el caso de éxito (los caminos de error con segmento
asignan el segmento para el mensaje, aceptable en el camino frío).
Las validaciones recorren `&str` prestado sin copiar nada. El coste:
O(n) tiempo, una asignación, y n está acotado por
`MAX_CONCEPT_ID_LEN = 200` **comprobado antes** de tocar memoria.

Ese orden — límite primero, trabajo después — reaparece en cada
capítulo. Es la regla número uno del servidor sin sorpresas de RAM.

## 7. Tests

De [`memory-model/src/lib.rs`](../crates/memory-model/src/lib.rs):

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

Observa que el test enumera ATAQUES, no solo formatos: traversal,
barra invertida, byte NUL, percent-encoding. Cada assert es un CVE
histórico condensado en una línea.

## 8. Frontera de producción

> 🧰 **La rueda de serie:** en producción, el boilerplate de newtypes validados lo quitan [`nutype`](https://docs.rs/nutype) o [`validator`](https://docs.rs/validator). El mapa completo y el criterio para elegir: [La rueda de serie](la-rueda-de-serie.md).

Nada cambia aquí en producción: este código ES el de producción.
`memory-model` no tiene dependencias que sustituir. Lo único que se
añade en el hito 2 es presión: el `ConceptId` viajará a SQL (como
parámetro tipado, jamás concatenado) y a claves de Supabase Storage,
y este capítulo es la razón de que eso sea seguro.

## 9. Principios SOLID en juego

- **S:** `memory-model` define QUÉ es un identificador; no sabe
  parsear documentos ni servir protocolos. Su única razón de cambio
  es que cambien las reglas del dominio.
- **L (preparándolo):** todos los contratos que veremos
  (`MemoryRepository`, `NeighborSource`) hablan en `ConceptId`, no en
  `String`. Las implementaciones intercambiables del capítulo 7
  pueden serlo PORQUE el tipo garantiza las precondiciones: ninguna
  implementación necesita re-validar, así que ninguna puede
  divergir en cómo valida.
- **D (anticipo):** este crate está en el fondo de la pila de
  dependencias. Todo apunta hacia él; él no apunta a nada.

## 10. Ejercicios

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
