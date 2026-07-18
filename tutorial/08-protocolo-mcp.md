# Capítulo 8 — El protocolo MCP y el transporte: strings entran, strings salen

Crates: [`crates/mcp-core`](../crates/mcp-core/src/lib.rs), [`crates/memory-tools`](../crates/memory-tools/src/lib.rs) y [`crates/mcp-stdio`](../crates/mcp-stdio/src)

## 1. El problema

Todo lo construido necesita hablar con el mundo. MCP (Model Context
Protocol) define cómo: JSON-RPC 2.0 con un ciclo de vida
(`initialize` → `notifications/initialized` → operar) y un contrato
de herramientas (`tools/list`, `tools/call`). Y hay que servirlo por
DOS transportes con vidas muy distintas: stdio hoy (un proceso por
cliente, líneas por stdin/stdout) y HTTP sin estado en Vercel mañana
(una función efímera por petición).

Si el protocolo se implementa PEGADO al transporte, mañana se
reescribe. Ese es el problema real del capítulo.

## 2. El invariante

> **El núcleo del protocolo es una función sobre strings:**
> `handle_message(&str) -> Option<String>`.
> **Sin sockets, sin stdin, sin async.** Quien tenga un string que
> entregar — una línea de stdin, un body HTTP, un test — llama y
> recibe.

El `Option` codifica una regla de JSON-RPC que descubre quien lo
implementa: las NOTIFICACIONES (mensajes sin `id`) jamás se
responden, ni siquiera con errores. `None` es "no contestes nada".

## 3. La implementación mínima

**El despacho** (mcp-core) es un router de tres niveles:

```text
¿parsea como JSON?         no → error -32700 (id null)
¿tiene method?             no → error -32600
¿tiene id?                 no → es notificación → procesar sin responder
método conocido            no → error -32601
  initialize / ping / tools/list / tools/call
```

Con una distinción que vale un examen: **error de protocolo vs fallo de dominio**.
Una herramienta desconocida es `-32602` (el
CLIENTE programó mal). Pero un conflicto CAS NO es un error
JSON-RPC: es un resultado con `isError: true`:

```rust
// Fallo de dominio: respuesta correcta de protocolo con isError=true.
// Así el MODELO ve el conflicto y puede releer y reintentar.
Err(ToolError::Failed(msg)) => ok_response(id, tool_result(&msg, true)),
```

La razón es quién consume cada canal: los errores JSON-RPC los ve el
CÓDIGO cliente (y típicamente los convierte en excepciones); el
resultado con `isError` lo ve el MODELO, que es quien puede leer
"revision_conflict, current_hash es X" y actuar. Confundir los dos
canales hace tu servidor técnicamente correcto e inútil en la
práctica.

**Las herramientas** (memory-tools/src/lib.rs) son la capa de traducción
JSON ↔ dominio, genérica sobre el repositorio:

```rust
pub struct MemoryTools<R> { repo: R, actor: Principal, budget: Budget }

impl<R> ToolHandler for MemoryTools<R>
where R: MemoryRepository + NeighborSource<Error = Infallible>
```

Ese `where` es SOLID en una línea: las herramientas exigen
CAPACIDADES (repositorio + fuente de vecinos), no un tipo concreto.
`InMemoryStore` las tiene hoy; el adaptador Supabase las tendrá
mañana; el `McpServer` no distingue.

**El transporte** (mcp-stdio/main.rs) queda en ~60 líneas de E/S:

```rust
loop {
    read_bounded_line(&mut reader, &mut line, budget.max_request_bytes)?;
    if let Some(response) = server.handle_message(line.trim()) {
        writeln!(out, "{response}")?;
        out.flush()?;
    }
}
```

`read_bounded_line` merece su §4 propio, porque su versión ingenua
es el bug de memoria más común de los servidores de línea.

## 3.5. Conceptos de Rust en este capítulo

Este capítulo une todas las piezas del workspace y define la frontera de E/S:

* **Cláusulas `where` para restricciones de Genéricos:** Al declarar `impl<R> ToolHandler for MemoryTools<R> where R: MemoryRepository + NeighborSource<Error = Infallible>`, estamos usando genéricos restringidos. La palabra clave `where` permite especificar de forma muy legible los requisitos que debe cumplir el tipo genérico `R`: debe ser capaz de actuar como un almacén de memoria (`MemoryRepository`) y a la vez poder listar sus vecinos en el grafo (`NeighborSource`), garantizando además que no producirá errores al recorrer el grafo (`Error = Infallible`).
* **Entrada/Salida Síncrona y Búferes:** Para la comunicación del protocolo por línea de comandos (stdio), usamos los tipos estándares de entrada y salida (`std::io::stdin()` y `std::io::stdout()`). En Rust, realizar E/S se modela mediante traits como `std::io::Read` y `std::io::Write`. Usar un lector con búfer (`BufReader`) es indispensable para evitar hacer llamadas al sistema operativo por cada byte leído, acumulando los caracteres en memoria intermedia de forma automática.

## 4. Una versión deliberadamente rota

```rust
// ❌ NO HACER: el read_line de los tutoriales
let mut line = String::new();
stdin.read_line(&mut line)?;   // ¿cuánto asigna esto?
```

Y la segunda rotura, más sutil, dentro del lector acotado bien
intencionado: validar UTF-8 **trozo a trozo** según llegan del
buffer interno de 8 KiB.

## 5. Por qué falla

**La primera:** `read_line` asigna lo que haga falta hasta el
`\n`. Un cliente (malicioso o simplemente roto) que envía 10 GB sin
salto de línea se convierte en 10 GB de RAM del servidor. En Vercel:
OOM, instancia muerta, factura. La versión acotada cuenta ANTES de
extender y descarta hasta el `\n` para resincronizar, respondiendo
un error JSON-RPC en vez de morir.

**La segunda es una historia real de ESTE repositorio:** la primera
versión validaba cada trozo con `from_utf8(&chunk)`. Un carácter
multibyte (una `ñ`, un emoji) que caiga JUSTO en la costura de dos
lecturas del buffer se parte en dos trozos individualmente inválidos
→ el servidor rechaza una línea perfectamente legal, una vez de cada
ocho mil, según dónde caiga la ñ. Un heisenbug de manual. La
corrección: acumular BYTES y validar UNA vez al final
([main.rs](../crates/mcp-stdio/src/main.rs), busca "costura").
La moraleja generaliza: **UTF-8 es una propiedad del mensaje completo, no de sus fragmentos de transporte.**

(Y ya que estamos en historias reales: el `update()` de SHA-256 de
este repo se colgó en un bucle infinito por machacar `buffer_len`
con un resto vacío — capítulo 2, §5 del código. Los bugs de este
tutorial no son inventados; son los que escribimos nosotros y
cazaron los tests. Esa es la publicidad honesta de los tests.)

## 6. Memoria y asignación

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

## 7. Tests

El [test de integración](../crates/mcp-stdio/tests/integration.rs)
es la conversación completa de un cliente real: initialize →
initialized → tools/list → crear dos documentos enlazados → buscar →
resolver con vecindario → provocar el conflicto CAS → comprobar que
la historia registra 2 revisiones y no 3. Si mañana rompes cualquier
pieza de la pila, este test lo cuenta en el idioma del usuario
final: "la conversación ya no funciona".

Más `entradas_hostiles`: traversal (rechazado como `-32602`),
documento sin frontmatter (fallo de dominio legible), concepto
inexistente (`not_found` estructurado). Fíjate qué canal usa cada
uno — es la distinción del §3 hecha test.

## 8. Frontera de producción

> 🧰 **La rueda de serie:** en producción, [`rmcp`](https://docs.rs/rmcp) (el SDK oficial de MCP en Rust) o [`jsonrpsee`](https://docs.rs/jsonrpsee) para JSON-RPC genérico. El mapa completo y el criterio para elegir: [La rueda de serie](la-rueda-de-serie.md).

El hito 2 añade `vercel-entry`: una función que recibe `POST /mcp`,
saca el body y llama… exactamente a `handle_message`. El diseño
sin estado del hito 1 es lo que lo hace posible:

- Sin `MCP-Session-Id`, sin estado entre mensajes: cada petición es
  autónoma (las sesiones son opcionales en MCP Streamable HTTP).
- `GET /mcp` → `405` (no ofrecemos stream servidor→cliente).
- La autenticación OAuth (hito 3) envuelve la llamada: valida el
  JWT, construye el `Principal` real (hoy `local_dev()`), y ese
  principal ya fluye hasta las revisiones — mira `Revision.actor`:
  el hueco está esperando desde el capítulo 1.

Lo que se reescribirá honestamente: el `Value` de json-mini en el
endpoint público será `serde_json` en `json-wire`, y `mcp-core`
debería entonces hablar DTOs propios en vez de `Value` (la deuda
declarada del capítulo 3, §9).

## 9. Principios SOLID en juego

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

## 10. Ejercicios

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
   queda abierto es añadir un segundo tipo de `UiResource` (o un trait
   separado) para exponer `okf://` también. Las URIs ya viajan en
   `memory_search` y `memory_resolve` esperándote. Decide: ¿qué
   presupuesto aplica a `resources/read`?

---

Fin del hito 1. Tienes un servidor MCP completo, con memoria
versionada, grafo acotado y concurrencia optimista, en ~2 500 líneas
de Rust sin una sola dependencia. El hito 2 lo saca a internet:
Streamable HTTP en Vercel, Postgres en Supabase, y la frontera
`std`-only demostrará su valor: el núcleo no cambiará.
