# Capítulo 3 — JSON a mano: el precio del texto

Crate: [`crates/json-mini`](../crates/json-mini/src/lib.rs)

## 1. El problema

MCP habla JSON-RPC 2.0. Antes de poder servir el protocolo
(capítulo 8) necesitamos convertir texto en estructura y estructura
en texto. La industria resuelve esto con `serde` + `serde_json` — y
conviene deshacer un malentendido común: **`serde` solo no parsea JSON**.
`serde` es la maquinaria genérica de (de)serialización;
`serde_json` es el formato. Escribir el nuestro nos enseña qué
compramos cuando los usamos.

## 2. El invariante

Tres, en realidad:

> 1. **Todo error lleva el offset del byte que lo causó.**
> 2. **Ningún input, por hostil que sea, asigna memoria sin límite ni desborda la pila.**
> 3. **La serialización es determinista: el mismo `Value` produce siempre el mismo string.**

## 3. La implementación mínima

El tipo central es un enum recursivo — la forma natural de modelar
un árbol en Rust:

```rust
pub enum Value {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Value>),
    Object(BTreeMap<String, Value>),
}
```

Cada variante encierra sus datos: un `Value` no puede ser "un número
y quizá también un string" como un `union` de C. El `match` sobre él
es exhaustivo: si mañana añadimos una variante, cada `match` del
proyecto deja de compilar hasta que la contemple. Los enums son la
herramienta de diseño más importante de Rust y este crate es su
demostración.

**¿Por qué `BTreeMap` y no `HashMap`?** Determinismo (invariante 3).
Un `BTreeMap` itera en orden de claves; serializar dos veces el
mismo objeto da el mismo string, los tests comparan con `==`, y una
respuesta cacheada por hash no se invalida por reordenamientos
fantasma. `HashMap` itera en un orden aleatorio POR DISEÑO (semilla
anti-DoS). Regla práctica del proyecto: `BTreeMap` cuando el orden
se observa, `HashMap` para índices internos calientes.

El parser es descenso recursivo puro sobre `&[u8]`:

```rust
struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}
```

Ese `'a` es la primera lifetime visible del proyecto: el parser NO
copia la entrada, la presta. Solo asigna al construir el árbol de
salida.

Y el invariante 2, en la primera línea de `parse_value`:

```rust
fn parse_value(&mut self, depth: usize) -> Result<Value, ParseError> {
    if depth > MAX_DEPTH {
        return Err(self.err("anidamiento demasiado profundo"));
    }
    ...
}
```

Cada `[` u `{` anidado incrementa `depth`. Sin esto, el input
`"[[[[[["` × 100 000 revienta la pila del proceso: descenso
recursivo = profundidad de JSON → profundidad de stack real.

## 3.5. Conceptos de Rust en este capítulo

Este capítulo introduce dos de las herramientas más potentes del sistema de tipos de Rust:

* **Enums con datos asociados:** A diferencia de la mayoría de los lenguajes donde un `enum` es solo una lista de números, en Rust las variantes de un enum pueden llevar datos adjuntos. E.g., `Value::String(String)` contiene un string real. Esto permite modelar el árbol del JSON de forma completamente segura: un valor es una de estas cosas y solo una.
* **Tipos recursivos e indirección en el Heap:** El compilador de Rust necesita saber el tamaño exacto en memoria de cada tipo al compilar. Si tuviéramos un enum que se contiene a sí mismo directamente (ej. `Array(Value)`), su tamaño teórico sería infinito. Para solucionarlo, usamos colecciones como `Vec<Value>` y `BTreeMap<String, Value>`. Estas estructuras no guardan los valores dentro del enum, sino que guardan un puntero (de tamaño fijo y conocido) a una zona de memoria dinámica (**heap**) donde residen los datos.
* **Ciclos de vida (Lifetimes, `<'a>`):** En la estructura `struct Parser<'a> { bytes: &'a [u8], pos: usize }`, el parámetro `'a` es un *lifetime*. Le indica al compilador que los bytes del JSON son "prestados" de fuera y que el `Parser` no puede existir si esos bytes de origen se borran o modifican. El compilador valida esto y te impide cometer fallos de referencias nulas o colgadas sin usar recolector de basura.
* **El operador de propagación `?`:** Es un atajo para el control de errores. Si escribimos `let value = text.parse()?`, significa: *"intenta parsear, si sale `Ok(val)` asígnalo a `value`; si sale `Err(error)`, haz un `return Err(error)` inmediato saliendo de la función actual"*. Simplifica enormemente el código de parsing en cadena.

## 4. Una versión deliberadamente rota

Un parser de números "pragmático":

```rust
// ❌ NO HACER: delegar en parse() de f64 sin validar la gramática
fn parse_number(&mut self) -> Result<Value, ParseError> {
    let start = self.pos;
    while matches!(self.peek(), Some(b) if !b" ,]}\n\t".contains(&b)) {
        self.pos += 1;
    }
    let text = std::str::from_utf8(&self.bytes[start..self.pos]).unwrap();
    text.parse::<f64>()
        .map(Value::Number)
        .map_err(|_| self.err("número inválido"))
}
```

## 5. Por qué falla

`f64::parse` de Rust acepta MÁS que la gramática JSON: `"inf"`,
`"NaN"`, `"1."`, `"+5"`, `".5"`, `"0x1p3"` no existen en JSON pero
algunos pasan por `parse()`. Consecuencias concretas:

- `NaN` entra al árbol → la serialización emite `NaN` → el JSON
  resultante es INVÁLIDO para cualquier otro parser → un `Value`
  legal produce salida ilegal, rompiendo el contrato más básico.
- Dos parsers que discrepan sobre qué es un número son un clásico
  vector de contrabando (*parser differential*): el validador de
  seguridad lee una cosa, el consumidor final otra.

La versión real valida la gramática JSON carácter a carácter
(`-`, entera sin ceros a la izquierda, fracción, exponente) y
DESPUÉS delega la conversión numérica, rechazando `inf`:

```rust
let value: f64 = text.parse().map_err(...)?;
if !value.is_finite() {
    return Err(self.err("número fuera de rango"));
}
```

El otro agujero didáctico son los strings: `\uD83D` solo es medio
carácter (subrogado alto de UTF-16). Nuestro parser exige la pareja
`😀` y los combina en el codepoint real; un parser que los
pusha sueltos fabrica Strings de Rust inválidos… bueno, no puede:
`char::from_u32(0xD800)` devuelve `None`. **Rust hace imposible el bug**
que en otros lenguajes produce mojibake — pero solo si usas
`char::from_u32` en lugar de un `unsafe` apresurado. Nuestro
workspace tiene `#![forbid(unsafe_code)]`: la vía rápida está
cerrada por decreto.

## 6. Memoria y asignación

- **Parseo:** asigna exactamente el árbol de salida. Un string sin
  escapes se copia una vez (`push_str` del slice completo del
  chunk). La entrada jamás se copia entera.
- **Límites:** la profundidad la corta `MAX_DEPTH`; el TAMAÑO lo
  corta el transporte ANTES de llamar a `parse` (capítulo 8:
  `read_bounded_line` con `budget.max_request_bytes`). Separación
  deliberada: el parser limita la forma, el transporte limita el
  volumen. Cada capa vigila lo que puede ver.
- **Serialización:** un único `String` de salida que crece
  amortizado. Ejercicio 3 explora pre-dimensionarlo.

## 7. Tests

Los cuatro grupos que todo parser debería tener:

| Test | Invariante vigilado |
| ---- | ------------------- |
| `ida_y_vuelta_basica` | `parse(to_string(v)) == v` |
| `rechaza_json_invalido` | 13 inputs malformados, incluido `{}extra` |
| `limite_de_profundidad` | `[[[[...` no revienta la pila |
| `objetos_deterministas` | `{"z":1,"a":2}` serializa como `{"a":2,"z":1}` |

Más dos de decisiones documentadas: claves duplicadas (gana la
última, como `serde_json`) y enteros estables (`42` → `42`, jamás
`42.0`, porque un id JSON-RPC debe volver byte-idéntico).

## 8. Frontera de producción

> 🧰 **La rueda de serie:** en producción, [`serde`](https://docs.rs/serde) + [`serde_json`](https://docs.rs/serde_json) sustituyen este capítulo entero con un `#[derive]`. El mapa completo y el criterio para elegir: [La rueda de serie](la-rueda-de-serie.md).

En el hito 2, el endpoint público de Vercel parseará con
`serde_json` en el crate adaptador `json-wire`, por tres razones
honestas: rendimiento (SIMD, años de optimización), fuzzing
acumulado (millones de horas de CPU buscando inputs raros), y
structs tipados con `#[derive(Deserialize)]` en lugar de navegar
`Value` a mano. `json-mini` seguirá sirviendo el transporte stdio y
los tests. La lección de arquitectura: **los dos conviven** porque
`mcp-core` recibe strings y devuelve strings — el formato es un
detalle del adaptador.

## 9. Principios SOLID en juego

- **S:** este crate convierte texto ↔ `Value`. No sabe qué es
  JSON-RPC, ni un id, ni un método. `mcp-core` construye ESO encima.
- **O:** los constructores `obj()`, `s()`, `n()`, `arr()` son la
  única API que el resto del proyecto usa para fabricar valores; si
  mañana `Value::Number` pasara a un decimal exacto, esos cuatro
  puntos absorben el cambio.
- **D (la decisión grande):** `mcp-core` depende de `json-mini` HOY,
  y eso es una concesión consciente que el hito 2 revisará: el plan
  del proyecto es que el núcleo del protocolo hable con DTOs propios
  y que el formato viva en adaptadores (`json-wire` con serde). Está
  documentado como deuda arquitectónica en el código. SOLID también
  es saber QUÉ regla estás doblando y anotar el precio.

## 10. Ejercicios

1. **Guiado.** Añade `Value::pointer("/params/name")` al estilo
   JSON Pointer (RFC 6901, sin `~` escapes). Escribe primero los
   tests de: clave ausente, índice de array, índice no numérico.
2. **Medio.** Nuestro parser acepta `"\u0000"` (NUL escapado) dentro
   de strings. ¿Debería? Investiga qué hace `serde_json`, decide, y
   escribe el test que fije tu decisión. No hay respuesta única:
   hay decisión documentada o bug futuro.
3. **Abierto.** `to_string` no pre-dimensiona el `String` de salida.
   Implementa `estimated_size(&Value) -> usize` (una pasada) y usa
   `String::with_capacity`. Mide con un `Value` de 1 MB si la doble
   pasada gana o pierde. Explica el resultado en términos de
   reasignaciones amortizadas (duplicación de capacidad).

Siguiente: [Capítulo 4 — Frontmatter OKF: los bytes son la verdad](04-frontmatter-okf.md).
