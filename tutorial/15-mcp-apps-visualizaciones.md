# Capítulo 15 — MCP Apps: visualizaciones interactivas (grafo e historial)

Crates: [`crates/mcp-core`](../crates/mcp-core/src/lib.rs),
[`crates/graph-core`](../crates/graph-core/src/lib.rs),
[`crates/memory-tools`](../crates/memory-tools/src/lib.rs) — más la
app React en [`mcp-app/`](../mcp-app/) que compila el HTML que este
capítulo explica cómo servir.

> **Capítulo opcional.** Todo lo que sigue — los recursos `ui://`,
> `resources/list` y `resources/read` — es una extensión del
> protocolo, no parte del núcleo del servidor de memoria. Un cliente
> que no la entiende sigue usando `memory_search`, `memory_resolve`,
> `memory_commit` y `memory_history` exactamente igual que en el hito
> 1: JSON, sin HTML de por medio — porque ignora el `_meta` que no
> reconoce, no porque el servidor se lo esconda (sección 2 explica por
> qué esa distinción importa). Si no te interesa renderizar vistas,
> puedes saltarte este capítulo entero sin perder nada del resto del
> tutorial.

## 1. El problema

`memory_resolve` con `depth: 2` puede devolver perfectamente 40
vecinos. Como JSON son 40 líneas de `{"concept_id": ..., "depth": ..., "parent": ...}`
que un humano tiene que reconstruir mentalmente como
un árbol. El mismo dato, como grafo con anillos por profundidad y
líneas padre→hijo, se entiende en un vistazo. `memory_history` tiene
el mismo problema en su propia forma: una lista de revisiones es una
línea de tiempo que el cliente tiene que imaginarse.

MCP resuelve esto con una extensión ("MCP Apps",
[SEP-1724](https://github.com/modelcontextprotocol/ext-apps)):
un servidor puede declarar recursos `ui://` — HTML autocontenido — y
vincularlos a una herramienta. Si el cliente lo soporta, renderiza esa
vista en un iframe en vez de (o además de) el JSON crudo. Si no lo
soporta, la herramienta se comporta exactamente igual que antes: cero
regresión para clientes que no conocen la extensión.

Ese HTML autocontenido no se escribe a mano: vive como una app React
en [`mcp-app/`](../mcp-app/) (starter Vite + shadcn, bridge oficial
[`@modelcontextprotocol/ext-apps`](https://www.npmjs.com/package/@modelcontextprotocol/ext-apps)
en vez de un `postMessage` casero), con un componente por herramienta
enrutado en tiempo de ejecución por el `toolName` que inyecta el host,
[React Flow](https://reactflow.dev/ui) para los grafos
(`memory_resolve`, `memory_backlinks`) y
[Recharts](https://ui.shadcn.com/charts) para las estadísticas
(`memory_stats`, `memory_history`, `memory_validate`). `pnpm build`
compila todo eso — JS, CSS y los estilos de React Flow incluidos — a
un único `dist/mcp-app.html` con `vite-plugin-singlefile`, y
`pnpm run build:sync` lo copia a
`crates/memory-tools/assets/mcp-app.html`, que es el string que
`include_str!` empotra en el binario (sección 6). Ese archivo generado
tiene que vivir comiteado en el repo — el build de Rust nunca ejecuta
`pnpm` — así que
[`.github/workflows/mcp-app.yml`](../.github/workflows/mcp-app.yml)
reconstruye la UI en cada push/PR y falla si la copia del repo no
coincide con un build fresco.


El problema práctico lo ve la persona que depura una memoria grande: el JSON
es correcto, pero no explica el grafo. El peligro es que, al añadir UI,
rompamos clientes que solo conocen herramientas MCP básicas. Hexagonalmente,
la visualización debe colgar de recursos y metadatos opcionales; el núcleo
de herramientas sigue siendo el mismo puerto JSON.

## 2. El invariante

> **Un cliente que no entiende `_meta` sigue viendo exactamente las mismas herramientas, con el mismo JSON, que veía antes de que este capítulo existiera — porque el spec le OBLIGA a ignorar los campos `_meta` que no reconoce, no porque el servidor se los esconda.**

La primera versión de este capítulo tenía un invariante distinto y
más intuitivo — pero equivocado en la práctica (sección 5 cuenta la
historia completa): que el SERVIDOR debía negociar en `initialize` y
esconder `_meta.ui` a los clientes que no declararan soporte de la
extensión. Suena razonable, y cualquiera lo escribiría igual la
primera vez. El problema es empírico, no de diseño: **los clientes MCP Apps
reales — Claude incluido — nunca declaran esa capability.**
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


La versión rota es muy comprensible: leer la especificación, ver una
capability de cliente y esconder `_meta` hasta que el cliente la anuncie. Es
el diseño que uno dibujaría primero en una pizarra porque parece una
negociación limpia.

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

## 6. Memoria y asignación — y una URI por herramienta, no una compartida

El HTML de este servidor (compilado desde [`mcp-app/`](../mcp-app/) a
`crates/memory-tools/assets/mcp-app.html`) vive en un único módulo
constante, `APP_HTML: &'static str`, un literal embebido en el binario
— el mismo costo de memoria que cualquier otra constante del programa,
sin asignación en el heap ni en tiempo de arranque ni por petición, sin
importar que ese string mida 1 KB o 1 MB (React + React Flow + Recharts
inlineados suman poco más de 1 MB sin comprimir). `resources/read`
clona el `&'static str` en la respuesta JSON-RPC (una copia, como
cualquier otro campo de texto que ya serializábamos) — no hay lectura
de disco ni de red en el camino caliente.

`ui_resources()` devuelve 17 `UiResource`, uno por herramienta, y los
17 apuntan a `APP_HTML` — compartir el mismo `&'static str` (puntero +
longitud) 17 veces no duplica nada en el binario. Lo que SÍ es
deliberadamente distinto es la **URI** de cada uno
(`ui://okf-memory/memory_search`, `ui://okf-memory/memory_resolve`,
…): la primera versión de esta sección usaba una única
`ui://okf-memory/app` para las 13 herramientas de entonces, y no era
solo un detalle estético. Varios hosts MCP Apps (heredado del Apps SDK
de OpenAI, igual que `openai/outputTemplate` en la sección 3)
reutilizan el iframe ya abierto cuando `_meta.ui.resourceUri` no
cambia entre una llamada y la siguiente — es una optimización
intencional, pensada para apps con estado persistente entre llamadas.
Con una única URI compartida entre herramientas *distintas*, invocar
`memory_search` justo después de `memory_resolve` podía dejar la vista
pegada al `toolName` de la primera llamada, o directamente no reabrir
el panel al ver "la misma" URI de siempre — una pantalla en blanco sin
ningún error visible, exactamente el tipo de fallo silencioso que ya
apareció en la sección 5. Con URI distinta por herramienta, cada
llamada es inequívocamente un recurso nuevo para el host: el enrutado
interno por `hostContext.toolInfo.tool.name` (ver
`mcp-app/src/mcp-app.tsx`) sigue siendo necesario — sin él, 17 URIs
distintas cargarían igual el mismo HTML sin saber qué componente
montar — pero deja de ser la ÚNICA señal de la que depende el host
para decidir si hay que volver a renderizar.

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


El primer test TDD debe pedir `tools/list` como un cliente antiguo y
comprobar que las herramientas siguen presentes y sus esquemas no cambian.
Luego un test de `resources/read` verifica que el HTML embebido se entrega
por URI. El verde demuestra segregación: clientes de datos y clientes
visuales no comparten obligaciones.

## 8. Frontera de producción

> 🧰 **La rueda de serie:** para HTML generado con tipos y verificado en compilación, [`maud`](https://docs.rs/maud) o [`askama`](https://docs.rs/askama) en lugar de `&'static str`. El mapa completo y el criterio para elegir: [La rueda de serie](la-rueda-de-serie.md).

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

> **Actualización.** El caso de uso real llegó — no como base de
> conocimiento compartida a gran escala, sino como una pregunta de
> agente concreta ("¿Alice cuenta como `agent` si `person` es
> sub-clase de `agent`?"). El [capítulo 18](18-razonamiento-ligero.md)
> retoma exactamente este párrafo y explica por qué la respuesta —
> `ontology-core`, sin `oxigraph` ni ningún vocabulario obligatorio —
> no contradice lo que se acaba de argumentar aquí.

Lo que la vista de grafo hace en su lugar es deliberadamente modesto:
tres estados visuales fijos por nodo (raíz / vecino normal / enlace
roto — `ConceptGraph` en `mcp-app/src/shared/components/graph/`), sin
ningún intento de tipar la relación en sí. Ni siquiera distingue el
`doc_type` de los vecinos, porque `memory_resolve` no lo repite por
nodo del vecindario. Es una regla, no una ontología:

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
  solo sabe servir lo que el handler le dé. `mcp-app.html` vive en
  `memory-tools` (copiado ahí por `mcp-app/scripts/sync-to-rust.mjs`),
  el crate que sabe qué forma tienen sus 17 herramientas.

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
2. **Medio.** `memory_search` y `memory_list` devuelven exactamente el
   mismo `SearchHit[]` (ver `crates/memory-tools/src/lib.rs`), pero
   son dos herramientas — y dos componentes en `mcp-app/src/tools/`,
   `memory-search/view.tsx` y `memory-list/view.tsx`. Ábrelos junto a
   `ConceptTable` en `mcp-app/src/shared/components/tool/`: ¿qué parte
   comparten y qué parte no, y por qué no vale la pena fusionar los
   dos componentes en uno solo aunque su forma de datos sea idéntica?
3. **Abierto.** La sección 8 explica por qué NO se tipan los enlaces
   con OWL. Diseña la alternativa ligera que sí se descartó por
   alcance (no por mala idea): un frontmatter `links:` con pares
   `{target, kind}` en vez de `[[wiki-links]]` sin tipo. ¿Qué se
   rompe en `okf-core::scan_links` y en `memory_resolve`? ¿La vista de
   grafo podría entonces colorear ARISTAS, no solo nodos? (El
   [capítulo 18](18-razonamiento-ligero.md), ejercicio 1, resuelve la
   mitad de esta pregunta — el RAZONAMIENTO sobre relaciones tipadas —
   sin tocar la sintaxis `[[rel:destino]]` que ya existía desde el
   capítulo 4; la sintaxis alternativa sigue siendo tuya por diseñar.)

## 11. Actualización — formularios de entrada colapsables

Un problema práctico apareció al usar estas 17 vistas desde un agente
real en vez de a mano: `RunPanel` (los campos de búsqueda o edición)
se renderizaba SIEMPRE junto a `ResultPanel`, incluso cuando el
`toolResult` ya venía relleno porque el MODELO había llamado a la
herramienta — la persona solo quería ver el resultado, no un
formulario para repetir una búsqueda que el agente ya hizo. `RunPanel`
(`mcp-app/src/shared/components/tool/ToolLayout.tsx`) ahora acepta
`defaultOpen`, y las 15 vistas que lo usan pasan
`defaultOpen={!toolResult}`: colapsado cuando el host ya trajo un
resultado, expandido cuando se abre en frío (sin resultado, el
formulario es la única forma de usar la herramienta). Un botón "Edit
inputs" en la cabecera permite expandirlo en cualquier caso — la
persona sigue pudiendo reeditar y relanzar, solo que ya no es lo
primero que ve.

No fue una decisión de protocolo: `hostContext` (sección 3) no trae
ninguna señal de "esto lo abrió el modelo" frente a "esto lo abrió un
humano en frío" — solo `toolInfo.tool.name`. La señal que sí existe es
más simple y ya estaba en `useServerTool`: `toolResult`, la prop que
SOLO llega poblada cuando el host inyectó un resultado al abrir la
vista. Es el mismo principio que `isManual` (la misma hook, ver su
doc-comment) usa para decidir si un botón "Add to agent context" debe
aparecer: no inventar un canal nuevo cuando la pregunta ya tiene
respuesta en el estado que la vista ya recibe.

Siguiente: no hay — este es, por ahora, el último capítulo escrito.
