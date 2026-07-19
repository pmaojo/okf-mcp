# Capítulo 8 — El protocolo MCP y el transporte: strings entran, strings salen

Crates: [`crates/mcp-core`](../crates/mcp-core/src/lib.rs),
[`crates/memory-tools`](../crates/memory-tools/src/lib.rs) y
[`crates/mcp-stdio`](../crates/mcp-stdio/src) ·
[referencia](https://pmaojo.github.io/okf-mcp/mcp_core/)

Este es el capítulo donde todo lo que llevas construido empieza a
hablar con el mundo. MCP (Model Context Protocol) define cómo:
JSON-RPC 2.0 con un ciclo de vida (`initialize` →
`notifications/initialized` → operar) y un contrato de herramientas
(`tools/list`, `tools/call`).

Pero hay una complicación de calendario que conviene mirar de
frente: el servidor va a vivir en DOS transportes con vidas muy
distintas. Hoy, stdio — un proceso por cliente, líneas por
stdin/stdout. Mañana, HTTP sin estado en Vercel — una función
efímera por petición. Si implementas el protocolo PEGADO al
transporte de hoy, mañana lo reescribes entero. Ese es el problema
real del capítulo, y su solución cabe en una firma:

> **El núcleo del protocolo es una función sobre strings:**
> `handle_message(&str) -> Option<String>`.
> **Sin sockets, sin stdin, sin async.** Quien tenga un string que
> entregar — una línea de stdin, un body HTTP, un test — llama y
> recibe.

El `Option` del retorno no es un capricho: codifica una regla de
JSON-RPC que descubre todo el que lo implementa. Las NOTIFICACIONES
(mensajes sin `id`) jamás se responden, ni siquiera con errores.
`None` significa "no contestes nada". (Es una de las cosas que la
documentación publicada de `McpServer` demuestra con un ejemplo
ejecutable — sin un solo socket.)

## Un router de tres niveles y dos canales de error

El despacho de `mcp-core` se lee como un embudo:

```text
¿parsea como JSON?         no → error -32700 (id null)
¿tiene method?             no → error -32600
¿tiene id?                 no → es notificación → procesar sin responder
método conocido            no → error -32601
  initialize / ping / tools/list / tools/call
```

Y esconde una distinción que vale un examen: **error de protocolo
frente a fallo de dominio**. Una herramienta desconocida es `-32602`
— el CLIENTE programó mal. Pero un conflicto CAS NO es un error
JSON-RPC: es un resultado legítimo con `isError: true`:

```rust
// Fallo de dominio: respuesta correcta de protocolo con isError=true.
// Así el MODELO ve el conflicto y puede releer y reintentar.
Err(ToolError::Failed(msg)) => ok_response(id, tool_result(&msg, true)),
```

¿Por qué tanto cuidado con el canal? Porque cada uno tiene un lector
distinto. Los errores JSON-RPC los ve el CÓDIGO cliente, que
típicamente los convierte en excepciones. El resultado con `isError`
lo ve el MODELO — el único que puede leer "revision_conflict,
current_hash es X", entender el hint del capítulo 6 y reintentar.
Confundir los dos canales produce un servidor técnicamente correcto
e inútil en la práctica.

## Herramientas genéricas, transporte de sesenta líneas

Entre el protocolo y el almacén está la capa de traducción JSON ↔
dominio, y su declaración es SOLID en una línea:

```rust
pub struct MemoryTools<R> { repo: R, actor: Principal, budget: Budget }

impl<R> ToolHandler for MemoryTools<R>
where R: MemoryRepository + NeighborSource<Error = Infallible>
```

Ese `where` exige CAPACIDADES (repositorio + fuente de vecinos), no
un tipo concreto. `InMemoryStore` las tiene hoy; el adaptador
Supabase las tendrá mañana; el `McpServer` no distingue.

> **Nota (estado actual):** `MemoryTools<R>` de hoy tiene bastantes más
> campos y muchas más herramientas que las cuatro de este capítulo (17 en
> total — ver el README para la lista completa), pero la firma genérica de
> arriba sigue intacta: cada herramienta nueva (`memory_patch`,
> `skill_ingest`, y las tres de spec-driven development,
> `spec_propose`/`spec_tasks`/`spec_status`) se añadió como un método más
> sobre el mismo `impl<R> ToolHandler for MemoryTools<R>`, sin tocar el
> `where` ni el transporte. Las tres últimas no traen ni esquema ni almacén
> nuevo: un `spec` y una `task` son conceptos OKF corrientes (`type: spec`,
> `type: task`, enlazados con `[[implements:...]]`), así que viven en este
> mismo archivo por la misma razón que todo lo demás — son traducción
> JSON↔dominio, no protocolo ni persistencia.

¿Y el transporte, el que parecía el protagonista? Sesenta líneas:

```rust
loop {
    read_bounded_line(&mut reader, &mut line, budget.max_request_bytes)?;
    if let Some(response) = server.handle_message(line.trim()) {
        writeln!(out, "{response}")?;
        out.flush()?;
    }
}
```

Todo el trabajo del capítulo fue empujar la complejidad LEJOS de
aquí. Pero ese `read_bounded_line` de aspecto inocente se merece su
propia sección, porque su versión ingenua es el bug de memoria más
común de los servidores de línea.

## Dos maneras de romper un lector de líneas

La primera está en todos los tutoriales de Rust:

```rust
// ❌ NO HACER: el read_line de los tutoriales
let mut line = String::new();
stdin.read_line(&mut line)?;   // ¿cuánto asigna esto?
```

Respuesta: lo que haga falta hasta el `\n`. Un cliente — malicioso o
simplemente roto — que envía 10 GB sin salto de línea se convierte
en 10 GB de RAM del servidor. En Vercel: OOM, instancia muerta,
factura. La versión acotada cuenta ANTES de extender y descarta
hasta el `\n` para resincronizar, respondiendo un error JSON-RPC en
vez de morir.

La segunda rotura es más sutil, y es una historia real de ESTE
repositorio. La primera versión del lector acotado validaba UTF-8
**trozo a trozo**, con `from_utf8(&chunk)` sobre cada lectura del
buffer interno de 8 KiB. Suena razonable. Ahora piensa en una `ñ` —
dos bytes — que cae JUSTO en la costura entre dos lecturas: cada
mitad es individualmente inválida, y el servidor rechaza una línea
perfectamente legal. Una vez de cada ocho mil, según dónde caiga la
ñ. Un heisenbug de manual. La corrección: acumular BYTES y validar
UNA vez al final ([main.rs](../crates/mcp-stdio/src/main.rs), busca
"costura"). La moraleja generaliza: **UTF-8 es una propiedad del
mensaje completo, no de sus fragmentos de transporte.**

(Segunda historia real en dos capítulos — recuerda el bucle
infinito del SHA-256. Los bugs de este libro no son inventados: son
los que escribimos nosotros y cazaron los tests. Esa es la
publicidad honesta de los tests.)

## La conversación completa, como test

El [test de integración](../crates/mcp-stdio/tests/integration.rs)
es un cliente real de principio a fin: initialize → initialized →
tools/list → crear dos documentos enlazados → buscar → resolver con
vecindario → provocar el conflicto CAS → comprobar que la historia
registra 2 revisiones y no 3. Si mañana rompes cualquier pieza de la
pila, este test lo cuenta en el idioma del usuario final: "la
conversación ya no funciona".

Lo acompaña `entradas_hostiles`: traversal (rechazado como
`-32602`), documento sin frontmatter (fallo de dominio legible),
concepto inexistente (`not_found` estructurado). Fíjate en qué canal
usa cada uno — es la distinción de los dos canales, hecha test.

## La frontera de producción

> 🧰 **La rueda de serie:** en producción, [`rmcp`](https://docs.rs/rmcp) (el SDK oficial de MCP en Rust) o [`jsonrpsee`](https://docs.rs/jsonrpsee) para JSON-RPC genérico. El mapa completo y el criterio para elegir: [La rueda de serie](la-rueda-de-serie.md).

El hito 2 añade `vercel-entry`: una función que recibe `POST /mcp`,
saca el body y llama… exactamente a `handle_message`. El diseño sin
estado del hito 1 es lo que lo hace posible:

- Sin `MCP-Session-Id`, sin estado entre mensajes: cada petición es
  autónoma (las sesiones son opcionales en MCP Streamable HTTP).
- `GET /mcp` → `405` (no ofrecemos stream servidor→cliente).
- La autenticación OAuth (hito 3) envuelve la llamada: valida el
  JWT, construye el `Principal` real (hoy `local_dev()`), y ese
  principal ya fluye hasta las revisiones — mira `Revision.actor`:
  el hueco está esperando desde el capítulo 1.

Lo que se reescribirá honestamente: el `Value` de json-mini en el
endpoint público será `serde_json` en el adaptador, y `mcp-core`
debería entonces hablar DTOs propios en vez de `Value` (la deuda
declarada del capítulo 3).

---

## Apéndice del capítulo

### Conceptos de Rust

* **Cláusulas `where` para genéricos restringidos:** `impl<R> ToolHandler for MemoryTools<R> where R: MemoryRepository + NeighborSource<Error = Infallible>` especifica de forma legible los requisitos del tipo `R`: actuar como almacén, listar vecinos, y garantizar que recorrer el grafo no falla (`Error = Infallible`).
* **E/S síncrona y búferes:** la comunicación por stdio usa `std::io::stdin()`/`stdout()`, modelada con los traits `Read` y `Write`. Un lector con búfer (`BufReader`) es indispensable para no hacer una llamada al sistema por byte: acumula en memoria intermedia automáticamente.

### Memoria y asignación

Presupuesto de una petición completa, de fuera adentro:

```text
línea de entrada   ≤ 1 MiB   (read_bounded_line, ANTES de asignar)
árbol JSON         ≤ O(entrada), profundidad ≤ 64 (json-mini)
documento          ≤ 256 KiB (okf-core, antes de parsear)
recorrido de grafo ≤ 128 nodos / 2 MiB (graph-core)
respuesta          una String, cota práctica por el presupuesto del grafo
```

Cada capa aplica SU límite con la información que SOLO ella tiene.
No hay un "límite global mágico": hay defensa en profundidad.

### SOLID en juego

- **D, el examen final:** dibuja las flechas. `main.rs` → `McpServer`
  → `ToolHandler` (trait) ← `MemoryTools<R>` → `MemoryRepository`
  (trait) ← `InMemoryStore`. Todas las flechas de implementación
  apuntan HACIA las abstracciones del dominio. Cambiar transporte,
  herramientas o almacén son tres operaciones independientes.
- **O:** la quinta herramienta será una entrada más en `tools()` y
  un brazo más en `call()`. `McpServer` ni se recompila con lógica
  nueva: está cerrado.
- **S:** tres archivos, tres razones de cambio: `mcp-core` cambia si
  cambia MCP; `lib.rs` si cambian las herramientas; `main.rs` si
  cambia el transporte. Cuando un cambio pide tocar dos a la vez,
  esa es la señal de revisar el diseño.
- **L:** `McpServer<EchoTools>` en los tests unitarios y
  `McpServer<MemoryTools<InMemoryStore>>` en integración: dos
  handlers sustituibles bajo el mismo trait, y el servidor no
  distingue. Ya lo has visto tres veces; ya es un patrón tuyo.

### Ejercicios

1. **Guiado.** Añade la herramienta `memory_stats` (número de
   conceptos, revisiones, bytes totales). ¿Qué método le falta a
   `MemoryRepository`? ¿Rompe eso a futuros implementadores?
   (bienvenido al dilema de evolucionar un trait público).
2. **Medio.** El servidor procesa una petición cada vez. Haz el
   transporte concurrente: N hilos de worker con
   `Arc<Mutex<McpServer<...>>>` (solo `std::thread` y
   `std::sync::mpsc`). ¿Qué garantía del capítulo 7 te salva de
   corromper datos y POR QUÉ compila sin cambiar el almacén?
3. **Abierto (parcialmente resuelto).** El capítulo 15 termina
   implementando `resources/list` y `resources/read` genéricos en
   `mcp-core` — pero para servir recursos `ui://` (vistas MCP Apps),
   no `okf://<concept-id>` (el Markdown exacto de un concepto). La
   máquina genérica ya existe y es la misma para ambos casos; lo que
   queda abierto es añadir un segundo tipo de `UiResource` (o un
   trait separado) para exponer `okf://` también. Las URIs ya viajan
   en `memory_search` y `memory_resolve` esperándote. Decide: ¿qué
   presupuesto aplica a `resources/read`?

---

Fin del hito 1. Tienes un servidor MCP completo, con memoria
versionada, grafo acotado y concurrencia optimista, en ~2 500 líneas
de Rust sin una sola dependencia. El hito 2 lo saca a internet:
Streamable HTTP en Vercel, Postgres en Supabase, y la frontera
`std`-only demostrará su valor: el núcleo no cambiará.
