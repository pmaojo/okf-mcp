# Capítulo 10 — HTTP sin estado: el mismo protocolo, otro sobre

Crate: [`crates/mcp-http`](../crates/mcp-http/src)

## 1. El problema

El capítulo 8 dejó `mcp-core::handle_message(&str) -> Option<String>`
deliberadamente ciego al transporte. Ahora toca cobrar esa apuesta:
servir MCP por HTTP — el transporte que Vercel entenderá — sin tocar
una sola línea del núcleo del protocolo.

Pero HTTP trae exigencias que stdio no tenía:

- Un cliente puede mentir sobre `Content-Length` (declarar 4 GiB y
  no enviar nada, o enviar menos de lo prometido).
- Un navegador manda la cabecera `Origin`; un ataque de
  *DNS rebinding* intenta hacer pasar una petición del navegador de
  la víctima como si viniera del propio servidor.
- No hay sesión que mantener: cada invocación en Vercel puede
  atenderla una instancia de función distinta, así que "recordar"
  algo entre peticiones en memoria de proceso es una ilusión.

## 2. El invariante

> **`route()` decide QUÉ responder sin abrir un socket, sin
> `tokio`, sin conocer `InMemoryStore`. `server.rs` decide CÓMO leer
> bytes de un `TcpStream` sin conocer JSON-RPC.**

Es el mismo movimiento del capítulo 8, una capa más arriba. Y una
segunda promesa, más estrecha:

> **Ninguna cabecera del cliente hace que el servidor asigne más
> memoria de la que su presupuesto permite — ni siquiera para
> RECHAZAR la petición.**

## 3. La implementación mínima

`route()` es una función pura sobre tipos propios, sin sockets:

```rust
pub fn route<H: ToolHandler>(
    req: &HttpRequest,
    budget: &Budget,
    allowed_origins: &[String],
    server: &mut McpServer<H>,
) -> HttpResponse
```

Genérica sobre `ToolHandler` — exactamente como `McpServer` del
capítulo 8. El endpoint HTTP no sabe qué es `MemoryTools` ni
`InMemoryStore`; los recibe inyectados desde `main.rs`. Esa es la
razón de que este archivo se pueda testear con **cero** red: los
ocho tests de `route()` construyen un `HttpRequest` a mano y
comparan el `HttpResponse`, en microsegundos.

Las comprobaciones van en el orden de MENOR a MAYOR coste — la regla
de oro de validar entradas hostiles:

```rust
if req.path != "/mcp" { return 404; }                    // gratis: comparar un string
if origin_no_permitido { return 403; }                    // gratis: comparar contra una lista
match req.method { "GET" | "DELETE" => return 405, ... }   // gratis
if req.body.len() > budget.max_request_bytes { return 413; } // ya tenemos el tamaño
if !es_json { return 415; }                                // gratis
if !es_utf8 { return 400; }                                // O(n), pero ya vamos a parsear
// solo AHORA delegamos en JSON-RPC (que a su vez valida MCP)
```

Nunca llamamos a `handle_message` — que parsea JSON, aloja un árbol,
ejecuta una herramienta — antes de agotar los rechazos baratos.

**La protección DNS-rebinding**, en dos líneas con más matiz del que
parece:

```rust
if let Some(origin) = &req.origin {
    if !allowed_origins.is_empty() && !allowed_origins.iter().any(|o| o == origin) {
        return 403;
    }
}
```

Si el `Origin` está presente y NO está en la lista, se rechaza. Si
está AUSENTE, se acepta sin más. ¿Por qué? Los navegadores mandan
`Origin` SIEMPRE en peticiones cross-origin — es el propio navegador
quien lo añade, el atacante no puede falsificarlo desde JavaScript.
Los SDK de agentes (curl, un cliente MCP nativo) normalmente no lo
mandan, porque no son un navegador y no tienen ese concepto. Exigir
`Origin` a TODOS rechazaría a los clientes legítimos que más nos
importan; comprobarlo SOLO cuando aparece cierra la puerta exacta
que un navegador malicioso necesitaría abrir.

**El parser HTTP** (`server.rs`) repite, casi literalmente, el
patrón del lector acotado de stdio (capítulo 8, §3), adaptado a
CRLF:

```rust
fn read_bounded_line<R: Read>(reader: &mut BufReader<R>, max: usize) -> std::io::Result<String> {
    let mut bytes = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        reader.read_exact(&mut byte)?;
        if byte[0] == b'\n' {
            if bytes.last() == Some(&b'\r') { bytes.pop(); }
            break;
        }
        bytes.push(byte[0]);
        if bytes.len() > max {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "línea demasiado larga"));
        }
    }
    String::from_utf8(bytes).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "UTF-8 inválido"))
}
```

Y el momento que da nombre al capítulo — leer el CUERPO sin
confiar ciegamente en lo que el cliente promete:

```rust
if content_length > budget.max_request_bytes {
    return Err(ParseFailure::BodyTooLarge(budget.max_request_bytes));
}
let mut body = vec![0u8; content_length];   // AQUÍ asignamos, no antes
reader.read_exact(&mut body)?;
```

El `vec![0u8; content_length]` solo se ejecuta DESPUÉS de comprobar
el presupuesto contra el número que dice la cabecera — nunca contra
lo que realmente llegó. Si el cliente promete 4 GiB, la petición
muere en la comparación, sin que se reserve ni un byte del cuerpo.

## 3.5. Conceptos de Rust en este capítulo

Este capítulo implementa un parser HTTP y nos enseña a manejar la asignación de memoria dinámica de forma segura:

* **Asignaciones de vectores controladas (`vec![value; size]`):** La macro `vec![0u8; content_length]` crea un vector en el heap lleno de ceros con la longitud exacta indicada. En Rust, esto reserva memoria inmediatamente. Si realizáramos esta operación confiando a ciegas en el encabezado `Content-Length` del cliente sin comprobar antes contra nuestro `budget.max_request_bytes`, un cliente malicioso podría agotar toda la memoria RAM del servidor de forma instantánea enviando un número enorme.
* **`read` frente a `read_exact` en Sockets:** Cuando leemos de un socket de red mediante el trait `Read`, el método `read()` estándar lee "lo que esté disponible en ese momento", lo que puede ser menos de lo solicitado (una lectura parcial). Si queremos rellenar un búfer de tamaño fijo completo, debemos usar `read_exact()`, el cual garantiza que se leerá la cantidad exacta de bytes solicitada o devolverá un error si el socket se cierra antes de tiempo.
* **Métodos útiles de colecciones (`bytes.last()`):** El método `.last()` de un vector devuelve un `Option<&T>` que contiene una referencia al último elemento si el vector no está vacío. En el parser HTTP, lo usamos para comprobar si la línea leída termina con el retorno de carro de HTTP (`\r` en `\r\n`) de forma limpia haciendo: `if bytes.last() == Some(&b'\r') { bytes.pop(); }`.

## 4. Una versión deliberadamente rota

```rust
// ❌ NO HACER: confiar en el cuerpo antes de medirlo
let mut body = Vec::new();
stream.read_to_end(&mut body)?;         // lee TODO lo que el cliente mande
if body.len() > budget.max_request_bytes {
    return error_413();
}
```

## 5. Por qué falla

`read_to_end` no para hasta que el cliente cierre la conexión de
escritura. Un cliente que abre la conexión y envía bytes sin parar
—o que simplemente no cierra nunca— mantiene el hilo del servidor
bloqueado leyendo indefinidamente, acumulando memoria sin límite
mientras tanto. La comprobación de presupuesto llega DESPUÉS del
daño: es una alarma de incendios que suena cuando la casa ya se ha
quemado.

Hay una segunda versión rota, más sutil, específica de HTTP:

```rust
// ❌ NO HACER: confiar en Content-Length sin verificar qué llegó
let mut body = vec![0u8; content_length];
stream.read(&mut body)?;   // read(), no read_exact()
```

`read()` puede devolver MENOS bytes de los pedidos (una lectura
parcial es comportamiento normal de sockets, no un error). Si el
cliente promete 1000 bytes y manda 200, `body` queda con 800 ceros
al final — un documento corrupto que el servidor procesa como si
fuera el que el cliente quiso enviar. `read_exact` sí distingue
esto: falla con un error de E/S si no consigue llenar el buffer
completo, en vez de aceptar en silencio un cuerpo a medias.

## 6. Memoria y asignación

El pico de memoria de una petición HTTP es, en orden:

```text
línea de petición   ≤ MAX_LINE_BYTES (8 KiB)
cabeceras acumuladas ≤ MAX_HEADER_BYTES (16 KiB)
cuerpo               ≤ budget.max_request_bytes (1 MiB), y NUNCA
                        asignado antes de comparar Content-Length
```

Los límites de línea y cabeceras son constantes DEL TRANSPORTE
(nadie necesita una URL de 8 KiB); el límite del cuerpo es el
PRESUPUESTO DEL DOMINIO que ya conocíamos del capítulo 8 — la misma
`Budget` viaja intacta de stdio a HTTP, prueba de que vivía en el
sitio correcto desde el principio.

## 7. Tests

Catorce tests repartidos en dos capas, y la razón de la separación
es el propio capítulo:

**`lib.rs` (8 tests, sin red):** cada rama de `route()` — 404, 403
con y sin `Origin`, 405 para `GET`/`DELETE`, 413, 415, 400 por UTF-8
inválido, 202 para notificaciones, 200 ejecutando una herramienta
real. Corren en microsegundos porque no hay socket de por medio.

**`server.rs` (3 tests, con `TcpListener` real):** una petición
HTTP/1.1 completa de extremo a extremo, un `GET` sobre socket real
devolviendo 405, y el test que demuestra la promesa del §2:

```rust
#[test]
fn content_length_que_excede_el_presupuesto_no_lee_el_cuerpo() {
    // Prometemos un cuerpo enorme pero NUNCA lo enviamos: si el
    // servidor intentara leerlo, este test se colgaría.
    let request = "POST /mcp HTTP/1.1\r\nContent-Length: 999999999\r\n\r\n";
    // ...
    assert!(resp.starts_with("HTTP/1.1 413"));
}
```

Ese test no verifica un valor: verifica una AUSENCIA de bloqueo. Si
`read_http_request` intentara `read_exact` de 999 999 999 bytes
antes de comparar contra el presupuesto, el test se quedaría
colgado para siempre (el cliente jamás manda esos bytes) y el
proceso de `cargo test` tendría que matarse a mano. Que el test
TERMINE es, en sí mismo, la prueba.

## 8. Frontera de producción

Esto ya casi ES la frontera. Lo que falta:

- **`vercel-entry`** (adaptador, con `tokio` + `vercel_runtime`):
  recibe la petición ya parseada por el runtime de Vercel, la
  convierte a `HttpRequest` y llama a `route()` — SIN reescribir
  `read_http_request`, porque Vercel ya hizo ese trabajo. El
  parser HTTP a mano de `server.rs` deja de usarse en producción y
  sigue viviendo como el arnés de desarrollo local.
- **`ALLOWED_ORIGINS` en producción** deja de ser opcional: hoy
  `main.rs` solo avisa por stderr si está vacío; en Vercel, el
  arranque debería FALLAR sin esa variable configurada. Es un
  ejercicio (#2) deliberadamente dejado abierto.
- **TLS** lo termina Vercel (o el balanceador delante), nunca este
  código: ni `mcp-http` ni su eventual sucesor `vercel-entry`
  manejan certificados.

## 9. Principios SOLID en juego

- **S, con una frontera muy visible:** `lib.rs` decide, `server.rs`
  transporta. La prueba de que la separación es real: los 8 tests
  de `route()` no importan `TcpListener`, y ninguna función de
  `server.rs` sabe qué es un `revision_conflict`.
- **D:** `route()` no depende de `MemoryTools` ni de
  `InMemoryStore` — los tests los inyectan porque son convenientes,
  no porque `route` los necesite. Cuando exista `SupabaseStore`,
  este archivo seguirá compilando sin tocarlo, igual que
  `mcp-stdio` (capítulo 8) no se enteró de la llegada de HTTP.
- **O:** añadir una tercera ruta (`GET /health`, digamos) es una
  rama más en el `match` de `route()`, no una reescritura del
  parser HTTP. El parser está cerrado sobre el FORMATO; el
  enrutador está abierto sobre las RUTAS.

## 10. Ejercicios

1. **Guiado.** Añade `GET /health` devolviendo `200 {"status":"ok"}`
   SIN pasar por `McpServer` (ni JSON-RPC, ni herramientas). ¿Dónde
   encaja en el orden de comprobaciones de `route()` y por qué ahí?
2. **Medio.** Haz que `main.rs` process::exit(1) si
   `ALLOWED_ORIGINS` está vacío Y una variable `REQUIRE_ORIGIN=1`
   está presente (para no romper el flujo de desarrollo local por
   defecto, pero permitir un modo estricto). Escribe el mensaje de
   error pensando en quien lo lee en un log de producción a las 3 AM.
3. **Abierto.** El servidor actual es monohilo: una petición lenta
   bloquea a las demás. Diseña (con solo `std::thread` — sigue sin
   dependencias) un pool de hilos fijo que comparta el
   `McpServer<MemoryTools<InMemoryStore>>` protegido por
   `Arc<Mutex<...>>`. ¿Qué pasa con la garantía de exclusividad del
   capítulo 7 (`&mut self` como mutex del compilador) cuando la
   exclusividad pasa a ser un `Mutex` en tiempo de ejecución en vez
   de una garantía en tiempo de compilación? ¿Qué ganas y qué
   pierdes?

---

Con esto, el hito 2 tiene ya sus dos piezas verificables sin ninguna
cuenta externa: el contrato ejecutable que cualquier backend futuro
deberá cumplir, y el transporte HTTP que cualquier plataforma futura
podrá envolver. Lo que queda del hito 2 — un adaptador de Supabase
real y el despliegue en Vercel — necesita tus credenciales; cuando
las tengas, seguimos desde aquí.
