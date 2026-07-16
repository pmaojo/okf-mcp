# Capítulo 11 — El adaptador de Vercel: una frontera, un archivo

Crate: [`crates/vercel-entry`](../crates/vercel-entry)

## 1. El problema

Los capítulos 8 y 10 construyeron dos transportes — stdio y HTTP —
sin que `mcp-core` supiera de ninguno de los dos. Ahora toca la
prueba de fuego de esa apuesta: ¿de verdad podemos servir MCP en
Vercel sin reescribir el protocolo?

Vercel exige un puente concreto para Rust: el crate `vercel_runtime`
(sobre `tokio` y, con la *feature* `axum`, sobre `axum`). No hay
forma std-only de arrancar una función serverless — alguien tiene
que hablar el protocolo interno entre el runtime de Vercel y tu
binario, y ese "alguien" no lo vamos a escribir nosotros a mano
(al contrario que el SHA-256 del capítulo 2: aquí no hay vectores
NIST con los que demostrar que una implementación casera es
correcta, y el coste de equivocarse es un servicio caído).

## 2. El invariante

> **Todo lo que vive fuera de `crates/vercel-entry` sigue siendo
> exactamente el mismo código que corre en tu máquina.** Este crate
> traduce; no decide.

Es la frase que cierra el capítulo 10, llevada a sus últimas
consecuencias: si `route()` tuviera que cambiar para "funcionar en
Vercel", la frontera habría fallado.

## 3. La implementación mínima

Todo el archivo [`api/mcp.rs`](../crates/vercel-entry/api/mcp.rs)
cabe en menos de 90 líneas, y la razón es que casi todas hacen UNA
sola cosa: convertir tipos.

```rust
async fn mcp_handler(method: Method, headers: HeaderMap, body: Bytes) -> Response {
    let http_req = HttpRequest {
        method: method.to_string(),
        path: "/mcp".to_string(),
        origin: headers.get("origin").and_then(|v| v.to_str().ok()).map(str::to_string),
        content_type: headers.get("content-type").and_then(|v| v.to_str().ok()).map(str::to_string),
        body: body.to_vec(),
    };

    let mut server = shared_server().lock().expect("route() nunca hace panic");
    let resp = route(&http_req, &Budget::default(), &allowed_origins(), &mut server);

    let status = StatusCode::from_u16(resp.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, [("content-type", resp.content_type)], resp.body).into_response()
}
```

`axum::extract::{Method, HeaderMap, Bytes}` son EXTRACTORES: axum
inspecciona la firma de la función y les entrega justo esas tres
piezas de la petición, sin que nosotros toquemos un `http::Request`
crudo. Fíjate en la línea central: `route(&http_req, ...)` es
LITERALMENTE la misma llamada que hace `mcp-http/src/server.rs` tras
parsear un socket TCP a mano (capítulo 10, §3). El adaptador de
Vercel no reimplementa el enrutado, los códigos de estado ni el
presupuesto — los HEREDA.

El router es deliberadamente vacío de rutas:

```rust
let router = Router::new().fallback(mcp_handler);
let app = ServiceBuilder::new().layer(VercelLayer::new()).service(router);
run(app).await
```

`fallback` en axum captura CUALQUIER petición que no matchee una
ruta registrada — y como no registramos ninguna, captura TODO:
cualquier método, cualquier path que Vercel le entregue a esta
función. La decisión de "esto no es `/mcp`, esto es GET, esto pesa
demasiado" sigue viviendo en `route()`, no en el router de axum. Dos
motores de enrutado (Vercel + axum) delante de un único árbitro.

**El detalle que no es un descuido**, comentado en el propio código:

```rust
fn shared_server() -> &'static Mutex<McpServer<MemoryTools<InMemoryStore>>> {
    static SERVER: OnceLock<Mutex<McpServer<MemoryTools<InMemoryStore>>>> = OnceLock::new();
    ...
}
```

`InMemoryStore` sigue siendo el almacén — porque el adaptador de
Supabase todavía no existe. El `OnceLock` cachea la instancia
mientras la función de Vercel esté CALIENTE (instancias reutilizadas
entre invocaciones próximas), pero una instancia fría empieza vacía.
Esto NO es persistencia real; es honestidad sobre el estado actual
del proyecto, documentada en el propio código en vez de escondida.
Cuando `SupabaseStore` implemente `MemoryRepository` (y pase el
contrato del capítulo 9), este `OnceLock<Mutex<InMemoryStore>>` se
sustituye por un cliente HTTP hacia Postgres — y `mcp_handler` no
cambia una línea, porque `MemoryTools<R>` ya era genérico sobre `R`
desde el capítulo 8.

## 4. Una versión deliberadamente rota

```rust
// ❌ NO HACER: reimplementar la lógica de enrutado en axum
let router = Router::new()
    .route("/mcp", post(memory_commit_handler))
    .route("/mcp", get(|| async { StatusCode::METHOD_NOT_ALLOWED }))
    // ... una ruta de axum por cada regla de route() ...
```

## 5. Por qué falla

No es que no compile: es que ahora hay DOS lugares donde vive la
regla "GET a /mcp es 405" — el `match` de `route()` (capítulo 10) y
el router de axum de este archivo. El día que alguien añada una
quinta comprobación (una cabecera nueva, un límite distinto), tiene
que recordar tocar AMBOS. Y peor: `mcp-http` (el transporte local
sobre `TcpListener`) seguiría aplicando la regla vieja, porque nadie
sincronizó los dos sitios. Es el mismo bug de fondo que el capítulo
9 resolvió para el almacén (duplicar el contrato en vez de
compartirlo) — aquí, aplicado al enrutado en vez de a la persistencia.

La lección se repite porque es LA lección del hito 2: cada vez que
un transporte nuevo aparece, la tentación es reescribir la lógica en
el idioma del framework de moda. Resistirla es lo que hace que
`route()` siga siendo una sola fuente de verdad.

## 6. Memoria y asignación

Nada nuevo se asigna aquí que no asignara ya `route()`. Lo único
propio del adaptador es el `body.to_vec()` — una copia de los bytes
que `axum::body::Bytes` ya tenía en un buffer compartido por
referencia. Esa copia es el precio de cruzar la frontera de tipos
(`Bytes` de `axum`/`hyper` no es el `Vec<u8>` que `HttpRequest`
espera) y es aceptable: ocurre UNA vez, sobre datos ya acotados por
el propio Vercel (que impone sus propios límites de tamaño de
payload antes de que nuestro código se ejecute siquiera).

## 7. Tests

Este archivo, a propósito, **no tiene tests unitarios propios**. No
porque no importe, sino porque no hay nada que probar aquí que no
esté ya probado: la lógica de decisión vive en `route()` (12 tests,
capítulo 10) y el contrato del almacén vive en `contract.rs`
(capítulo 9). Lo único verificable de este archivo es "¿compila
contra las versiones reales de `vercel_runtime` y `axum`?" — y eso
se comprueba con `cargo build -p vercel-entry`, no con un test.
Escribir un test que solo repitiera "route() hace lo que route()
hace" sería duplicar aserciones sin añadir garantías: el antipatrón
exacto del capítulo 9, aplicado a tests en vez de a repositorios.

## 8. Frontera de producción

Esta ES la frontera de producción — el propio capítulo lo es. Lo que
falta para un despliegue real:

- **`ALLOWED_ORIGINS`** como variable de entorno del proyecto en
  Vercel (Project Settings → Environment Variables), no opcional
  aquí como en desarrollo local.
- **`Root Directory` = `crates/vercel-entry`** en la configuración
  del proyecto Vercel: el `Cargo.toml` de este crate y su carpeta
  `api/` deben estar donde el *builder* de Rust de Vercel los busca,
  y como este repositorio es un *workspace* con `crates/` en la
  raíz, hay que decirle a Vercel dónde mirar. Las rutas `path =
  "../memory-model"` etc. del `Cargo.toml` de este crate siguen
  resolviendo bien porque Cargo encuentra la raíz real del
  *workspace* subiendo directorios — Vercel solo necesita saber
  desde dónde ARRANCAR el build.
- **El almacén Supabase real**, sustituyendo `InMemoryStore` — el
  próximo capítulo, cuando exista.

## 9. Principios SOLID en juego

- **D, la demostración final:** este es el único archivo del
  proyecto que importa `axum`, `tokio` y `vercel_runtime` — y aun
  así, no hay ni una línea de LÓGICA de MCP aquí. Todo lo que
  importa (parseo, presupuesto, herramientas, conflictos) vive en
  crates que NUNCA oyeron hablar de Vercel. Si mañana Vercel
  desapareciera y hubiera que migrar a otra plataforma, este es el
  ÚNICO archivo que se reescribe.
- **S, con el ejemplo más pequeño del proyecto:** la única razón de
  cambio de `api/mcp.rs` es que cambie el PUENTE (una nueva versión
  de `vercel_runtime`, un extractor distinto). Nunca cambia porque
  cambie una regla de negocio — esas viven aguas abajo.
- **Todo el arco del tutorial, cerrado:** desde el capítulo 1
  (`ConceptId` no puede hacer daño) hasta aquí, cada pieza se separó
  precisamente para que ESTE capítulo pudiera ser tan corto. La
  arquitectura no se diseñó "por si acaso"; se diseñó para este
  momento exacto.

## 10. Ejercicios

1. **Guiado.** Añade `GET /api/mcp/health` (una ruta REAL en el
   router de axum, no vía `route()`) que responda `200 OK` sin tocar
   el almacén. Justifica por qué esta SÍ es una excepción legítima a
   "todo pasa por route()" (pista: no es una regla de NEGOCIO, es un
   *health check* de la propia plataforma).
2. **Medio.** Configura `ALLOWED_ORIGINS` como variable de entorno
   REQUERIDA: haz que `main()` falle rápido (con un mensaje claro en
   los logs de Vercel) si no está definida, salvo que exista también
   `ALLOW_ANY_ORIGIN=1` para desarrollo. Reutiliza el diseño que
   dejaste pendiente en el ejercicio 2 del capítulo 10.
3. **Abierto.** Cuando exista `supabase-adapter::SupabaseStore`,
   sustituye `InMemoryStore` en `shared_server()` por él. ¿Qué pasa
   con el `Mutex` cuando el backend real ya no necesita exclusión
   mutua en el proceso (porque Postgres la da por transacción)? ¿Te
   conviene seguir compartiendo una instancia cacheada entre
   invocaciones, o cada petición debería abrir su propia conexión?
   Argumenta con el modelo de concurrencia de Vercel Fluid compute
   en mente (instancias reutilizadas, pero nunca garantizado).

---

Con este capítulo, el hito 2 tiene las tres piezas que no dependían
de tus credenciales: el contrato ejecutable (capítulo 9), el
transporte HTTP verificado con sockets reales (capítulo 10), y el
adaptador de Vercel compilando contra las dependencias reales
(capítulo 11). Lo único que falta para un despliegue de verdad es
que tú importes el repositorio en Vercel (con `Root Directory` =
`crates/vercel-entry`) y conectes Supabase — y entonces construimos
el adaptador de persistencia sobre el contrato que ya te espera.
