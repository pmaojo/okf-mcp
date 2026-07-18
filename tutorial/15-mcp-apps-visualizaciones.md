# Capítulo 15 — MCP Apps: visualizaciones interactivas (grafo e historial)

Crates: [`crates/mcp-core`](../crates/mcp-core/src/lib.rs), [`crates/graph-core`](../crates/graph-core/src/lib.rs), [`crates/memory-tools`](../crates/memory-tools/src/lib.rs)

> **Capítulo opcional.** Todo lo que sigue — los recursos `ui://`, el
> HTML de `graph-view.html`/`history-view.html`, `resources/list` y
> `resources/read` — es una extensión del protocolo, no parte del
> núcleo del servidor de memoria. Un cliente que no la entiende sigue
> usando `memory_search`, `memory_resolve`, `memory_commit` y
> `memory_history` exactamente igual que en el hito 1: JSON, sin HTML
> de por medio — porque ignora el `_meta` que no reconoce, no porque
> el servidor se lo esconda (sección 2 explica por qué esa distinción
> importa). Si no te interesa renderizar vistas, puedes saltarte este
> capítulo entero sin perder nada del resto del tutorial.

## 1. El problema

`memory_resolve` con `depth: 2` puede devolver perfectamente 40
vecinos. Como JSON son 40 líneas de `{"concept_id": ..., "depth": ..., "parent": ...}`
que un humano tiene que reconstruir mentalmente como
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

> **Un cliente que no entiende `_meta` sigue viendo exactamente las mismas herramientas, con el mismo JSON, que veía antes de que este capítulo existiera — porque el spec le OBLIGA a ignorar los campos `_meta` que no reconoce, no porque el servidor se los esconda.**

La primera versión de este capítulo tenía un invariante distinto y
más intuitivo — pero equivocado en la práctica (sección 5 cuenta la
historia completa): que el SERVIDOR debía negociar en `initialize` y
esconder `_meta.ui` a los clientes que no declararan soporte de la
extensión. Suena razonable, y cualquiera lo escribiría igual la
primera vez. El problema es empírico, no de diseño: **los clientes MCP Apps reales — Claude incluido — nunca declaran esa capability.**
Un servidor que la exige nunca ve `_meta` llegar a nadie. La
degradación elegante real no depende de que el servidor adivine qué
sabe el cliente; depende de que el protocolo diga que `_meta`
desconocido se ignora, y de que el servidor confíe en eso.

## 3. La implementación mínima

Un `ToolSpec` lleva `ui_resource_uri: Option<&'static str>`.
`on_tools_list` añade `_meta` siempre que el tool lo declare — sin
mirar nada de lo que dijo el cliente en `initialize`:

```rust
if let Some(uri) = t.ui_resource_uri {
    fields.push((
        "_meta",
        obj([
            ("ui", obj([
                ("resourceUri", s(uri)),
                ("visibility", arr(vec![s("model"), s("app")])),
            ])),
            ("openai/outputTemplate", s(uri)),
            ("openai/widgetAccessible", Value::Bool(true)),
        ]),
    ));
}
```

Fíjate en el doble anuncio: `ui.resourceUri` es el campo que define
la extensión MCP Apps (SEP-1724, la spec "oficial"); `openai/outputTemplate`
+ `openai/widgetAccessible` son los que el host de Claude realmente
lee hoy (heredados del Apps SDK de OpenAI, que Claude adoptó como
formato de facto). No hay forma de saber de antemano cuál de los dos
interpretará el cliente que conecte — así que se mandan ambos. Esto
no es elegante ni definitivo: es lo que hace falta para que funcione
con el ecosistema real en vez de con la lectura literal del documento
de la extensión.

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

Esta vez la versión rota es la que este capítulo tenía originalmente
— y que cualquiera escribiría primero, porque es la lectura literal
de la spec:

```rust
// ❌ NO HACER: esconder `_meta.ui` tras una negociación que ningún
// cliente real completa nunca.
self.ui_apps_supported = params
    .get("capabilities")
    .and_then(|c| c.get("extensions"))
    .and_then(|e| e.get("io.modelcontextprotocol/ui"))
    .and_then(|ext| ext.get("mimeTypes"))
    .and_then(|mt| mt.as_array())
    .map(|types| types.iter().any(|t| t.as_str() == Some(UI_APPS_MIME_TYPE)))
    .unwrap_or(false);

// ... más tarde, en on_tools_list:
if let Some(uri) = t.ui_resource_uri {
    if self.ui_apps_supported {
        fields.push(("_meta", obj([("ui", obj([("resourceUri", s(uri))]))])));
    }
}
```

## 5. Por qué falla

Compilaba, pasaba los tests (`EchoWithUi` inicializado con y sin la
capability declarada a mano, en un test que también ha cambiado — ver
sección 7) y era, sobre el papel, exactamente lo que pide SEP-1724.
Falló de todos modos, y falló en silencio: conectado desde Claude de
verdad, `memory_resolve` y `memory_history` NUNCA llevaban `_meta`,
así que la vista de grafo y la línea de tiempo jamás aparecían — sin
ningún error, sin ningún log, sin nada que apuntara a `mcp-core`. Solo
"la UI no aparece".

La causa se encontró comparando con otro servidor MCP (en Python, sin
relación con este proyecto) que sí renderiza vistas en Claude en
producción: su capa de metadata **nunca comprueba ninguna capability del cliente** —
manda `_meta` siempre, en cada tool que tiene vista, y
además de `ui.resourceUri` manda `openai/outputTemplate` y
`openai/widgetAccessible`, que son el vocabulario real del Apps SDK de
OpenAI que Claude adoptó. Es decir: el cliente real ni declara la
extensión en `initialize` ni busca solo el campo que documenta
SEP-1724. La lección no es "la spec estaba mal escrita" — es que una
negociación de capability es una promesa entre dos partes, y aquí
solo una de las dos (el servidor) la estaba cumpliendo. `_meta`
desconocido es seguro de ignorar por spec; apostar la visibilidad
entera de la función a que el cliente además confirme que lo
entiende, cuando en la práctica no lo confirma, convierte una
extensión opcional en una función que nunca se activa.

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
`tools_list_anuncia_meta_ui_siempre_declare_o_no_el_cliente_la_extension`
— el mismo servidor, inicializado con y sin la capability declarada,
produce el MISMO `tools/list` con `_meta` en los dos casos. Es la
prueba directa del invariante nuevo (sección 2): antes este test
comprobaba lo contrario (dos `tools/list` distintos según lo que
declarara el cliente) y pasaba igual de verde — la señal de que un
test puede confirmar una implementación internamente consistente y
aun así no decir nada sobre si un cliente real la activa.

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

1. **Guiado.** La versión rota de la sección 4 guardaba
   `ui_apps_supported` como campo de `McpServer`, fijado una vez en
   `initialize` y leído después en `on_tools_list`. La versión buena
   no necesita ese campo en absoluto. ¿Qué categoría de bug
   desaparece al borrar un campo de estado mutable que solo existía
   para recordar algo que ya no hace falta decidir? (pista: piensa en
   qué pasaría si `initialize` se llamara dos veces, o si se
   reordenara respecto a `tools/list` en un transporte que no
   garantiza el orden).
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
