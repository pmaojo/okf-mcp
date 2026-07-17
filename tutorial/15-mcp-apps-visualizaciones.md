# Capítulo 15 — MCP Apps: visualizaciones interactivas (grafo e historial)

Crates: [`crates/mcp-core`](../crates/mcp-core/src/lib.rs), [`crates/graph-core`](../crates/graph-core/src/lib.rs), [`crates/memory-tools`](../crates/memory-tools/src/lib.rs)

> **Capítulo opcional.** Todo lo que sigue — los recursos `ui://`, el
> HTML de `graph-view.html`/`history-view.html`, `resources/list` y
> `resources/read` — es una extensión negociable del protocolo, no
> parte del núcleo del servidor de memoria. Un cliente que no la
> declara en `initialize` sigue usando `memory_search`,
> `memory_resolve`, `memory_commit` y `memory_history` exactamente
> igual que en el hito 1: JSON, sin HTML de por medio. Si no te
> interesa renderizar vistas, puedes saltarte este capítulo entero sin
> perder nada del resto del tutorial.

## 1. El problema

`memory_resolve` con `depth: 2` puede devolver perfectamente 40
vecinos. Como JSON son 40 líneas de `{"concept_id": ..., "depth": ...,
"parent": ...}` que un humano tiene que reconstruir mentalmente como
un árbol. El mismo dato, como grafo con anillos por profundidad y
líneas padre→hijo, se entiende en un vistazo. `memory_history` tiene
el mismo problema en su propia forma: una lista de revisiones es una
línea de tiempo que el cliente tiene que imaginarse.

MCP resuelve esto con una extensión ("MCP Apps", [SEP-1724](https://github.com/modelcontextprotocol/ext-apps)):
un servidor puede declarar recursos `ui://` — HTML autocontenido — y
vincularlos a una herramienta. Si el cliente lo soporta, renderiza esa
vista en un iframe en vez de (o además de) el JSON crudo. Si no lo
soporta, la herramienta se comporta exactamente igual que antes: cero
regresión para clientes que no conocen la extensión.

## 2. El invariante

> **Un cliente que no declaró soporte de la extensión MCP Apps ve
> exactamente las mismas herramientas, con el mismo JSON, que veía
> antes de que este capítulo existiera.**

La degradación elegante no es un detalle de implementación: es LA
garantía que hace que añadir una vista sea un cambio compatible en
vez de una ruptura. Un servidor de memoria personal puede tener un
único usuario hoy y verse desde tres clientes distintos mañana; solo
uno de esos tres tiene que entender HTML en un iframe.

## 3. La implementación mínima

La negociación ocurre una vez, en `initialize`. El cliente declara la
extensión con su identificador reservado y el mimeType que soporta:

```json
{"capabilities": {"extensions": {"io.modelcontextprotocol/ui": {"mimeTypes": ["text/html;profile=mcp-app"]}}}}
```

`McpServer` lo lee con la misma API de `json_mini::Value` que ya usa
todo el crate — no hace falta ningún parser nuevo:

```rust
self.ui_apps_supported = params
    .get("capabilities")
    .and_then(|c| c.get("extensions"))
    .and_then(|e| e.get(UI_APPS_EXTENSION))
    .and_then(|ext| ext.get("mimeTypes"))
    .and_then(|mt| mt.as_array())
    .map(|types| types.iter().any(|t| t.as_str() == Some(UI_APPS_MIME_TYPE)))
    .unwrap_or(false);
```

Un `ToolSpec` ahora puede llevar `ui_resource_uri: Option<&'static
str>`. `on_tools_list` solo añade `_meta.ui.resourceUri` cuando AMBAS
condiciones se cumplen — el tool lo pide, y el cliente lo entiende:

```rust
if let Some(uri) = t.ui_resource_uri {
    if self.ui_apps_supported {
        fields.push(("_meta", obj([("ui", obj([("resourceUri", s(uri))]))])));
    }
}
```

Y dos métodos JSON-RPC nuevos, genéricos sobre cualquier
`ToolHandler` (no solo `MemoryTools`):

```rust
"resources/list" => self.on_resources_list(id),
"resources/read" => self.on_resources_read(id, params),
```

`resources/read` busca el recurso por URI y devuelve su HTML tal
cual, embebido en tiempo de compilación con `include_str!` — el
servidor nunca genera HTML en caliente, sirve siempre el mismo string
estático.

## 3.5. Conceptos de Rust en este capítulo

* **Métodos de trait con cuerpo por defecto:** `ToolHandler` gana
  `fn ui_resources(&self) -> Vec<UiResource> { Vec::new() }`. Un
  cuerpo por defecto en un trait es la forma de Rust de añadir
  capacidad a una interfaz sin romper a nadie que ya la implementaba
  — `EchoTools` (los tests de `mcp-core`) no tuvo que cambiar una
  sola línea a pesar de que el trait creció.
* **`&'static str` para contenido embebido:** tanto `UiResource::html`
  como `ToolSpec::ui_resource_uri` son `&'static str`, no `String`.
  Como el HTML viene de `include_str!` (un literal insertado en
  tiempo de compilación) y las URIs son literales de código, viven
  para siempre en el binario — no hay asignación, ni copia, ni
  gestión de tiempo de vida que pensar.
* **`Vec<(&str, Value)>` → `Value::Object` a mano:** `on_tools_list`
  construye cada tool con un `Vec` de campos en vez del helper `obj`
  (que exige un array de tamaño fijo conocido en compilación) porque
  el número de campos varía: con `_meta` o sin él. El propio tipo de
  la variante `Value::Object(BTreeMap<String, Value>)` es suficiente
  para que `.collect()` infiera el tipo de destino sin anotarlo.

## 4. Una versión deliberadamente rota

```rust
// ❌ NO HACER: anunciar `_meta.ui` sin comprobar la capability del cliente
fields.push(("_meta", obj([("ui", obj([("resourceUri", s(uri))]))])));
```

## 5. Por qué falla

Un cliente que no implementa MCP Apps no sabe qué hacer con
`_meta.ui.resourceUri` — en el mejor de los casos lo ignora
silenciosamente (el spec no obliga a los clientes a validar campos
`_meta` desconocidos), en el peor un cliente estricto podría
rechazarlo como un tool con forma inesperada. La comprobación de
`self.ui_apps_supported` no es una optimización: es la diferencia
entre "extensión opcional" y "cambio incompatible disfrazado de
opcional".

## 6. Memoria y asignación

Los dos recursos `ui://` de este servidor (`graph-view.html`,
`history-view.html`) son literales `&'static str` embebidos en el
binario — el mismo costo de memoria que cualquier otra constante del
programa, sin asignación en el heap ni en tiempo de arranque ni por
petición. `resources/read` clona el `&'static str` en la respuesta
JSON-RPC (una copia, como cualquier otro campo de texto que ya
serializábamos) — no hay lectura de disco ni de red en el camino
caliente.

## 7. Tests

`mcp-core` prueba el contrato completo con un handler de juguete
(`EchoWithUi`) que expone un recurso `ui://test/echo-view`:
`resources_list_y_read_devuelven_el_recurso_ui`,
`resources_read_de_uri_desconocida_es_invalid_params`, y sobre todo
`tools_list_solo_anuncia_meta_ui_si_el_cliente_declaro_la_extension`
— el mismo servidor, inicializado dos veces con dos capabilities
distintas, produce dos `tools/list` distintos. Es la prueba directa
del invariante del capítulo.

`graph-core` prueba que `Visited::parent` reconstruye un árbol sin
ciclos: desde cualquier nodo visitado, subir por `parent` termina
siempre en `None` (la raíz) en como mucho `visited.len()` pasos.

## 8. Frontera de producción

Lo que este capítulo NO hace, a propósito: no adopta OWL/RDF ni
ningún modelo de ontología formal para tipar los enlaces `[[...]]` o
los `doc_type`. Se consideró explícitamente y se descartó para este
proyecto: OWL trae consigo clases, subclases, propiedades tipadas y
(para sacarle partido real) un razonador de subsunción — maquinaria
pensada para bases de conocimiento compartidas a gran escala con
múltiples ontologías que se cruzan. Este servidor es la memoria de un
único agente, sobre un grafo acotado por presupuesto; `doc_type` ya
es un string libre y los enlaces ya son la única relación que existe.
No hay vocabulario tipado que extraer todavía, y añadir el andamiaje
de una ontología formal antes de tener un caso de uso real que lo
necesite sería exactamente la clase de abstracción prematura que este
tutorial lleva 14 capítulos evitando.

Lo que la vista de grafo hace en su lugar es deliberadamente modesto:
un color por `doc_type` calculado con una función hash a una paleta
fija de 8 colores (`hashColor` en `graph-view.html`) — ni siquiera
sabe el `doc_type` de los vecinos, porque `memory_resolve` no lo
repite por nodo del vecindario. Es una regla, no una ontología:

> Tipar un dato tiene sentido cuando alguien va a RAZONAR sobre esos
> tipos (subclases, herencia, inferencia). Colorear un dato para que
> un humano lo lea más rápido no necesita más que una función
> determinista de string a color.

Lo que sí queda abierto (ver ejercicios) es que ni `memory_resolve` ni
`memory_history` traen hoy los campos que una vista más rica querría
— `doc_type` por vecino, `created_at` por revisión. Añadirlos es
trabajo de una frase en `memory-tools`, no una decisión arquitectónica.

## 9. Principios SOLID en juego

* **O (el protagonista):** `ToolHandler::ui_resources()` con cuerpo
  por defecto es el Abierto/Cerrado de manual — el trait se abrió
  para soportar un caso nuevo (servir vistas) sin cerrar (romper) a
  ningún implementador existente.
* **I:** `UiResource` y `ToolSpec.ui_resource_uri` son campos
  opcionales, independientes del resto del contrato de `ToolHandler`.
  Un handler que no sabe nada de MCP Apps no necesita conocer ni el
  tipo `UiResource` para compilar.
* **D:** `mcp-core` no conoce el contenido de ningún HTML concreto —
  solo sabe servir lo que el handler le dé. `graph-view.html` y
  `history-view.html` viven en `memory-tools`, el crate que sabe qué
  forma tienen `memory_resolve`/`memory_history`.

## 10. Ejercicios

1. **Guiado.** El test
   `tools_list_solo_anuncia_meta_ui_si_el_cliente_declaro_la_extension`
   inicializa el mismo tipo de servidor dos veces. ¿Por qué no se
   puede reutilizar la MISMA instancia de `McpServer` para probar
   ambos casos? (pista: `ui_apps_supported` es un campo del servidor,
   fijado una vez en `initialize`).
2. **Medio.** Añade una vista `ui://okf-memory/search-view` para
   `memory_search` (hoy en texto plano a propósito). ¿Qué justifica
   el cambio de opinión — qué gana un humano viendo resultados de
   búsqueda como tarjetas en vez de JSON?
3. **Abierto.** La sección 8 explica por qué NO se tipan los enlaces
   con OWL. Diseña la alternativa ligera que sí se descartó por
   alcance (no por mala idea): un frontmatter `links:` con pares
   `{target, kind}` en vez de `[[wiki-links]]` sin tipo. ¿Qué se
   rompe en `okf-core::scan_links` y en `memory_resolve`? ¿La vista de
   grafo podría entonces colorear ARISTAS, no solo nodos?

Siguiente: no hay — este es, por ahora, el último capítulo escrito.
