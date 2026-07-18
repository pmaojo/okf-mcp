# Capítulo 3 — JSON a mano: el precio del texto

Crate: [`crates/json-mini`](../crates/json-mini/src/lib.rs) ·
[referencia](https://pmaojo.github.io/okf-mcp/json_mini/)

Ya sabes nombrar conceptos y darles identidad. Pero el servidor que
estamos construyendo habla con el mundo, y el mundo habla JSON: MCP
es JSON-RPC 2.0 por debajo. Antes de poder servir el protocolo
(capítulo 8) necesitas convertir texto en estructura y estructura en
texto.

La industria resuelve esto con `serde` + `serde_json`, y de paso
conviene deshacer un malentendido común: **`serde` solo no parsea
JSON**. `serde` es la maquinaria genérica de (de)serialización;
`serde_json` es el formato. Nosotros vamos a escribir el nuestro —
no por masoquismo, sino porque es la única forma de saber qué
compras cuando escribes `#[derive(Deserialize)]`. Al final del
capítulo, ese derive habrá dejado de ser magia.

Como todo lo que toca entrada hostil en este libro, el parser nace
con sus promesas por delante. Tres:

> 1. **Todo error lleva el offset del byte que lo causó.**
> 2. **Ningún input, por hostil que sea, asigna memoria sin límite
>    ni desborda la pila.**
> 3. **La serialización es determinista: el mismo `Value` produce
>    siempre el mismo string.**

## Un tipo para gobernarlos a todos

El corazón del crate es un enum recursivo — la forma natural de
modelar un árbol en Rust:

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

Seis variantes, ni una más, y cada una encierra sus datos: un
`Value` no puede ser "un número y quizá también un string" como un
`union` de C. Y el `match` sobre él es exhaustivo: si mañana
añadimos una variante, cada `match` del proyecto deja de compilar
hasta que la contemple. Los enums son la herramienta de diseño más
importante de Rust, y este crate es su demostración práctica.

La elección menos obvia está en la última variante. **¿Por qué
`BTreeMap` y no `HashMap`?** Por el invariante 3, determinismo: un
`BTreeMap` itera en orden de claves, así que serializar dos veces el
mismo objeto da el mismo string, los tests comparan con `==`, y una
respuesta cacheada por hash no se invalida por reordenamientos
fantasma. `HashMap` itera en un orden aleatorio POR DISEÑO (semilla
anti-DoS). Regla práctica que usaremos todo el libro: `BTreeMap`
cuando el orden se observa, `HashMap` para índices internos
calientes.

El parser en sí es descenso recursivo puro sobre bytes:

```rust
struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}
```

Ese `'a` es la primera lifetime visible del proyecto, y dice algo
importante: el parser NO copia la entrada, la presta. Solo asigna al
construir el árbol de salida.

¿Y el invariante 2? Vive en la primera línea de `parse_value`:

```rust
fn parse_value(&mut self, depth: usize) -> Result<Value, ParseError> {
    if depth > MAX_DEPTH {
        return Err(self.err("anidamiento demasiado profundo"));
    }
    ...
}
```

Cada `[` u `{` anidado incrementa `depth`. Piensa en lo que pasa sin
esa línea cuando llega `"[[[[[["` repetido cien mil veces: descenso
recursivo significa que la profundidad del JSON se convierte en
profundidad de la pila REAL del proceso. Sin el límite, el input
elige cuándo revienta tu stack. Con él, el atacante recibe un error
con offset y tú sigues sirviendo.

## El parser "pragmático" que sirve NaN

La versión rota de este capítulo no cuelga ni explota: hace algo
peor. Funciona casi siempre. Así parsearía números alguien con
prisa:

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

"Corto hasta el siguiente delimitador y que `f64::parse` decida."
El problema: `f64::parse` de Rust acepta MÁS que la gramática JSON.
`"inf"`, `"NaN"`, `"1."`, `"+5"`, `".5"` no existen en JSON, pero
algunos pasan por `parse()`. Sigue la bola de nieve: `NaN` entra al
árbol → la serialización emite `NaN` → el JSON resultante es
INVÁLIDO para cualquier otro parser. Un `Value` legal que produce
salida ilegal: el contrato más básico del crate, roto por una
cortesía mal entendida.

Y hay una consecuencia de seguridad con nombre propio: dos parsers
que discrepan sobre qué es un número son un *parser differential*,
un clásico vector de contrabando — el validador de seguridad lee una
cosa, el consumidor final otra.

La versión real valida la gramática JSON carácter a carácter (`-`,
parte entera sin ceros a la izquierda, fracción, exponente) y solo
DESPUÉS delega la conversión numérica, rechazando lo no finito:

```rust
let value: f64 = text.parse().map_err(...)?;
if !value.is_finite() {
    return Err(self.err("número fuera de rango"));
}
```

El otro agujero clásico son los strings con escapes Unicode:
`\uD83D` es solo medio carácter (un subrogado alto de UTF-16).
Nuestro parser exige la pareja y combina ambos en el codepoint real
(😀); un parser descuidado que los empuje sueltos fabricaría
strings inválidos… bueno, en Rust no puede: `char::from_u32(0xD800)`
devuelve `None`. **Rust hace imposible el bug** que en otros
lenguajes produce mojibake — siempre que uses `char::from_u32` en
lugar de un `unsafe` apresurado. Nuestro workspace tiene
`#![forbid(unsafe_code)]`: la vía rápida está cerrada por decreto.

## Lo que vigilan los tests

Cuatro grupos, los que todo parser debería tener:

| Test | Invariante vigilado |
| ---- | ------------------- |
| `ida_y_vuelta_basica` | `parse(to_string(v)) == v` |
| `rechaza_json_invalido` | 13 inputs malformados, incluido `{}extra` |
| `limite_de_profundidad` | `[[[[...` no revienta la pila |
| `objetos_deterministas` | `{"z":1,"a":2}` serializa como `{"a":2,"z":1}` |

Más dos que fijan DECISIONES, no correcciones: con claves duplicadas
gana la última (como `serde_json`), y los enteros son estables
(`42` → `42`, jamás `42.0`, porque un id JSON-RPC debe volver
byte-idéntico). Lo importante no es qué se decidió, sino que quedó
decidido y testeado: la alternativa es un bug futuro con opiniones.

## La frontera de producción

> 🧰 **La rueda de serie:** en producción, [`serde`](https://docs.rs/serde) + [`serde_json`](https://docs.rs/serde_json) sustituyen este capítulo entero con un `#[derive]`. El mapa completo y el criterio para elegir: [La rueda de serie](la-rueda-de-serie.md).

En el hito 2, el endpoint público de Vercel parseará con
`serde_json` en el crate adaptador, por tres razones honestas:
rendimiento (SIMD, años de optimización), fuzzing acumulado
(millones de horas de CPU buscando inputs raros), y structs tipados
con `#[derive(Deserialize)]` en lugar de navegar `Value` a mano.
`json-mini` seguirá sirviendo el transporte stdio y los tests. La
lección de arquitectura: **los dos conviven** porque `mcp-core`
recibe strings y devuelve strings — el formato es un detalle del
adaptador.

---

## Apéndice del capítulo

### Conceptos de Rust

* **Enums con datos asociados:** a diferencia de la mayoría de lenguajes donde un `enum` es una lista de números, en Rust las variantes llevan datos adjuntos: `Value::String(String)` contiene un string real. Esto modela el árbol JSON de forma completamente segura: un valor es una de estas cosas y solo una.
* **Tipos recursivos e indirección en el heap:** el compilador necesita saber el tamaño exacto de cada tipo. Un enum que se contuviera a sí mismo directamente tendría tamaño infinito; por eso usamos `Vec<Value>` y `BTreeMap<String, Value>`, que guardan un puntero (de tamaño fijo) a memoria dinámica (**heap**) donde residen los datos.
* **Ciclos de vida (lifetimes, `<'a>`):** en `struct Parser<'a> { bytes: &'a [u8], ... }`, el `'a` indica que los bytes son prestados de fuera: el `Parser` no puede sobrevivir a los bytes de origen. El compilador lo verifica y elimina las referencias colgadas sin recolector de basura.
* **El operador de propagación `?`:** `let value = text.parse()?` significa: si sale `Ok(val)`, asígnalo; si sale `Err(e)`, haz `return Err(e)` inmediato. Simplifica enormemente el parsing en cadena.

### Memoria y asignación

- **Parseo:** asigna exactamente el árbol de salida. Un string sin
  escapes se copia una vez (`push_str` del slice completo del
  chunk). La entrada jamás se copia entera.
- **Límites:** la profundidad la corta `MAX_DEPTH`; el TAMAÑO lo
  corta el transporte ANTES de llamar a `parse` (capítulo 8:
  `read_bounded_line` con `budget.max_request_bytes`). Separación
  deliberada: el parser limita la forma, el transporte limita el
  volumen. Cada capa vigila lo que puede ver.
- **Serialización:** un único `String` de salida que crece
  amortizado. El ejercicio 3 explora pre-dimensionarlo.

### SOLID en juego

- **S:** este crate convierte texto ↔ `Value`. No sabe qué es
  JSON-RPC, ni un id, ni un método. `mcp-core` construye ESO encima.
- **O:** los constructores `obj()`, `s()`, `n()`, `arr()` son la
  única API que el resto del proyecto usa para fabricar valores; si
  mañana `Value::Number` pasara a un decimal exacto, esos cuatro
  puntos absorben el cambio.
- **D (la decisión grande):** `mcp-core` depende de `json-mini` HOY,
  y eso es una concesión consciente que el hito 2 revisará: el plan
  es que el núcleo del protocolo hable con DTOs propios y que el
  formato viva en adaptadores (serde en el crate de frontera). Está
  documentado como deuda arquitectónica en el código. SOLID también
  es saber QUÉ regla estás doblando y anotar el precio.

### Ejercicios

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
