# Capítulo 0.5 — Fundamentos de Rust: Lo mínimo para no perderse

Si nunca has programado en Rust, algunos patrones de este código te parecerán extraños, verbosos o innecesariamente estrictos. No te preocupes: es el precio que se paga por tener un software extremadamente seguro y rápido.

Este capítulo es una guía de referencia rápida para que entiendas la sintaxis y las reglas del compilador de Rust antes de analizar el servidor MCP.

---

## 1. El compilador y Cargo

Rust es un lenguaje compilado. El compilador (`rustc`) es famoso por ser muy estricto: si tu código compila, es muy probable que funcione correctamente y sea inmune a fallos de memoria.

Para gestionar proyectos, usamos **Cargo**, la herramienta oficial:
* `cargo build`: Compila tu proyecto.
* `cargo test`: Ejecuta los tests automáticos (usaremos mucho esto).
* `cargo run`: Compila y ejecuta el programa.

---

## 2. Variables y Mutabilidad

Por defecto, en Rust **todas las variables son inmutables**. Si declaras una variable, no puedes cambiar su valor a menos que uses la palabra clave `mut`:

```rust
let x = 5;       // Inmutable. Si haces `x = 6;` el compilador dará error.
let mut y = 10;  // Mutable.
y = 15;          // Totalmente válido.
```

---

## 3. Tipos de Datos Esenciales

Rust tiene tipos estáticos (el compilador debe saber qué tipo es cada cosa), pero a menudo los infiere solo.

* **Enteros:** `u32` (entero sin signo de 32 bits), `i32` (entero con signo), `usize` (entero sin signo cuyo tamaño depende de la arquitectura de la CPU, usado para indexar colecciones o medir longitudes).
* **Booleanos:** `bool` (`true` o `false`).
* **Arrays y Slices:**
  * Un **Array** tiene tamaño fijo conocido en tiempo de compilación: `[u32; 8]` es un array de 8 enteros.
  * Un **Slice** (rebanada) es una vista de tamaño dinámico sobre una secuencia de elementos en memoria: `&[u8]` es una referencia a una porción de bytes.
* **Strings (Cadenas de texto):** En Rust hay dos tipos principales de strings:
  1. `String`: Es una cadena que posee su memoria en el **Heap** (memoria dinámica). Puede crecer, modificarse y es la dueña de sus bytes.
  2. `&str` (string slice): Es una referencia (o "vista") inmutable y de solo lectura de una cadena de texto. No posee los bytes, solo los observa.
  
  *Conversión:* Puedes convertir un `&str` a `String` usando `.to_string()`.

---

## 4. Funciones

Se definen con `fn`. Si devuelven un valor, se indica el tipo de retorno después de una flecha `->`. Si omites el punto y coma `;` en la última expresión de una función, Rust la interpreta como el valor de retorno (retorno implícito):

```rust
fn sumar(a: i32, b: i32) -> i32 {
    a + b // Retorno implícito (sin 'return' ni ';')
}
```

---

## 5. La Regla de Oro: Propiedad (Ownership) y Préstamos (Borrowing)

Este es el concepto más importante de Rust. Para garantizar la seguridad sin usar un recolector de basura (como el de Java o Go), Rust sigue estas reglas:

### A. Propiedad (Ownership)
* Cada valor en Rust tiene una variable que es su **dueño** (owner).
* Solo puede haber **un dueño a la vez**.
* Cuando el dueño sale del ámbito (scope), el valor se destruye automáticamente de la memoria.

Si asignas una variable dueña a otra, la propiedad se **mueve** (Move). La primera variable ya no puede ser usada:

```rust
let s1 = String::from("hola");
let s2 = s1; // La propiedad de los bytes se ha movido a s2. s1 ya no es válida.
// println!("{}", s1); // ❌ ¡Error de compilación!
```

### B. Préstamos (Borrowing) y Referencias (`&`)
Para evitar mover la propiedad de los datos constantemente al llamar a funciones, podemos **prestar** los valores usando referencias (`&`):

```rust
fn longitud(s: &String) -> usize { // Recibe una referencia (préstamo inmutable)
    s.len()
}

let s1 = String::from("hola");
let len = longitud(&s1); // Prestamos s1 con '&'. Seguimos siendo dueños de s1.
```

Hay dos tipos de préstamos:
1. **Referencia inmutable (`&T`):** Puedes tener tantas como quieras a la vez, pero solo permiten LEER los datos.
2. **Referencia mutable (`&mut T`):** Te permite MODIFICAR los datos, pero **solo puedes tener una referencia mutable a la vez** en un ámbito dado, y ninguna otra inmutable.

> **Regla de oro:** O tienes muchas referencias de lectura (`&`), o tienes una sola referencia exclusiva de escritura (`&mut`), pero nunca ambas al mismo tiempo. Esto evita que dos partes del código lee y modifique el mismo dato a la vez (carreras de datos).

### C. Ciclos de Vida (Lifetimes)
A veces, el compilador necesita asegurarse de que una referencia no apunte a un dato que ya ha sido borrado de la memoria (referencia colgante). Para ello, usa **lifetimes** (anotados como `'a`). 
En este tutorial verás cosas como:

```rust
struct Parser<'a> {
    bytes: &'a [u8],
}
```

Esto solo le dice al compilador: *"La estructura `Parser` no puede vivir más tiempo que los `bytes` que tiene prestados"*. El compilador garantiza esto de forma estricta.

---

## 6. Estructuras (`struct`) y Enums

### Structs
Las estructuras agrupan datos. Rust ofrece structs clásicas y **Tuple Structs** (estructuras de tupla, que no tienen nombres de campo, solo tipos):

```rust
// Struct clásica
struct Persona {
    nombre: String,
    edad: u32,
}

// Tuple Struct (muy usada para crear tipos específicos seguros o "newtypes")
pub struct ConceptId(String);
```

### Enums (Enumeraciones)
Los enums en Rust son extremadamente potentes porque **cada variante puede llevar datos asociados** (en otros lenguajes se conocen como Tipos de Datos Algebraicos):

```rust
enum Mensaje {
    Salir,                        // No lleva datos
    Escribir(String),             // Lleva una cadena
    Mover { x: i32, y: i32 },     // Lleva una estructura anónima
}
```

---

## 7. Pattern Matching (Coincidencia de Patrones)

Para leer el contenido de un enum, usamos `match`. Rust te obliga a controlar **todas** las variantes posibles (exhaustividad):

```rust
match mensaje {
    Mensaje::Salir => println!("Saliendo"),
    Mensaje::Escribir(texto) => println!("Texto: {}", texto),
    Mensaje::Mover { x, y } => println!("Mover a {}, {}", x, y),
}
```

Si solo te interesa una variante, puedes usar `if let`:

```rust
if let Mensaje::Escribir(texto) = mensaje {
    println!("Solo me interesaba el texto: {}", texto);
}
```

También existe la macro `matches!`, que evalúa si una expresión coincide con un patrón y devuelve un booleano:

```rust
let es_salida = matches!(mensaje, Mensaje::Salir); // Devuelve true/false
```

---

## 8. Tratamiento de Errores: `Option` y `Result`

Rust no tiene excepciones (`throw`/`try`/`catch`) ni valores `null`. En su lugar, usa dos enums del sistema:

### Option<T>
Representa la posible ausencia de un valor:
```rust
pub enum Option<T> {
    Some(T),  // Contiene un valor de tipo T
    None,     // No contiene nada
}
```

### Result<T, E>
Representa el resultado de una operación que puede fallar:
```rust
pub enum Result<T, E> {
    Ok(T),   // La operación fue un éxito y contiene el resultado de tipo T
    Err(E),  // La operación falló y contiene el error de tipo E
}
```

### El Operador `?`
Para simplificar la propagación de errores, Rust proporciona el operador `?`. Si una función devuelve un `Result` (o un `Option`), poner `?` al final de una llamada significa: *"Si la llamada devuelve un error, sal inmediatamente de esta función y devuelve ese error. Si tiene éxito, extrae el valor y continúa"*.

```rust
fn leer_archivo() -> Result<String, ErrorDeArchivo> {
    let mut archivo = abrir_archivo("datos.txt")?; // Si falla, sale devolviendo el Err
    let contenido = leer_datos(&mut archivo)?;     // Si falla, sale devolviendo el Err
    Ok(contenido)                                  // Si todo sale bien, devuelve Ok
}
```

---

## 9. Cajas y Módulos: El Workspace

Este proyecto está estructurado como un **Workspace** (espacio de trabajo) de Cargo. Contiene múltiples carpetas independientes dentro de `crates/`.
Cada una de estas carpetas es un **Crate** (una biblioteca o ejecutable independiente) con su propio `Cargo.toml`. 

El compilador de Rust compila cada crate por separado y gestiona sus dependencias internas. Esto nos permite separar físicamente la lógica y cumplir los principios SOLID.

---

## 10. Recursos Oficiales para profundizar

Si quieres complementar lo aprendido en este tutorial con la documentación oficial del lenguaje, te recomendamos los siguientes recursos oficiales (disponibles en línea y offline mediante `rustup doc`):

* **[El Libro de Rust (The Rust Programming Language)](https://doc.rust-lang.org/stable/book/)**: El manual oficial definitivo. Ideal para entender a fondo las reglas de propiedad (ownership) y la filosofía del lenguaje.
* **[Rust by Example](https://doc.rust-lang.org/stable/rust-by-example/)**: Una colección de ejemplos prácticos y editables que muestran cómo usar la sintaxis de Rust sin rodeos teóricos.
* **[Ejercicios Interactivos Rustlings](https://github.com/rust-lang/rustlings)**: Pequeños ejercicios guiados para arreglar errores de compilación comunes y familiarizarte con el lenguaje.
* **[La Documentación de la Biblioteca Estándar (API std)](https://doc.rust-lang.org/stable/std/)**: La guía de referencia para todos los módulos nativos (ej. `std::io`, `std::collections`, `std::sync`, etc.).
* **[El Libro de Cargo](https://doc.rust-lang.org/stable/cargo/)**: El manual oficial de la herramienta de compilación y gestión de dependencias de Rust.
* **[The Rustonomicon](https://doc.rust-lang.org/stable/nomicon/)**: La guía oficial dedicada a los detalles más avanzados y oscuros de Rust (como el manejo de punteros crudos y código `unsafe`).

---

Ahora que tienes el mapa de Rust en tu cabeza, estás listo para ver cómo diseñamos identificadores que el compilador defiende de forma matemática.

Siguiente: [Capítulo 1 — Identificadores que no pueden hacer daño](01-identificadores-seguros.md).
