# okf-mcp — Servidor MCP de memoria en Rust (núcleo `std`-only)

Un servidor de memoria persistente para agentes (protocolo MCP) con
arquitectura hexagonal: el **motor de conocimiento** y los **puertos**
están escritos con la biblioteca estándar de Rust, sin frameworks, sin
`serde` y sin `tokio` en el núcleo. Las dependencias externas quedan
aisladas en adaptadores de frontera e infraestructura como Vercel,
Supabase, GitHub y los proveedores de LLM/embeddings (Gemini como
primario, con fallback multi-proveedor).

Este repositorio es a la vez un proyecto real y un **tutorial muy
didáctico** de Rust y de principios SOLID: ver [`tutorial/`](tutorial/).
La referencia de API generada con `cargo doc` se publica en
**<https://pmaojo.github.io/okf-mcp/>** en cada push a `main`
(capítulo 16 del tutorial).

## Estado

- ✅ **Hito 1:** núcleo `std`-only + servidor MCP por stdio.
- ✅ **Hito 2:** transporte HTTP sin estado (`mcp-http`) + contrato ejecutable `MemoryRepository` + adaptador de Vercel (`vercel-entry`) + adaptador de base de datos PostgreSQL (`supabase-store`).
- ✅ **Hito 3:** OAuth 2.1 (Resource Server, validación criptográfica de JWTs mediante firmas y JWKS).
- ✅ **Hito 4:** Transactional Outbox (`outbox-worker` con procesamiento concurrente `SKIP LOCKED`, sincronización con GitHub y embeddings con `pgvector`).
- ✅ **Hito 5:** MCP Apps — UI React interactiva ([`mcp-app/`](mcp-app/), tema brutalista) para las 13 herramientas `memory_*`, servida como un único recurso `ui://` (opcional, ver más abajo).

## Arquitectura

```text
                       Adaptadores de entrada
┌────────────────────────────────────────────────────────────────────┐
│ mcp-stdio    bin local por stdin/stdout                            │
│ mcp-http     bin HTTP/1.1 sin estado sobre TcpListener, POST /mcp  │
│ vercel-entry función serverless Axum/Vercel → mcp_http::route()    │
└───────────────┬────────────────────────────────────────────────────┘
                │
                ▼
                     Núcleo y puertos `std`-only
┌────────────────────────────────────────────────────────────────────┐
│ mcp-core     JSON-RPC 2.0 + ciclo de vida MCP + despacho           │
│ json-mini    parser/serializador JSON educativo                    │
│ memory-tools  17 herramientas MCP genéricas sobre MemoryRepository │
│ store-core   puerto MemoryRepository + contrato Liskov             │
│ memory-model ConceptId, ContentId, Budget, Revision, Principal     │
│ okf-core     frontmatter YAML (subconjunto) + enlaces [[...]]      │
│ graph-core   BFS acotado (trait NeighborSource)                    │
│ conflict-core decisiones compare-and-swap puras                    │
│ hash-core    SHA-256 a mano (vectores NIST)                        │
│ memory-store InMemoryStore para desarrollo y tests                 │
│ ingest-core  detección/planificación de skill_ingest + puertos     │
└───────────────┬────────────────────────────────────────────────────┘
                │
                ▼
                    Adaptadores de salida / infraestructura
┌────────────────────────────────────────────────────────────────────┐
│ supabase-store    SupabaseStore implementa MemoryRepository        │
│ outbox-worker     procesa outbox, GitHub y embeddings              │
│ gemini-embeddings  embeddings, fallback multi-proveedor            │
│ ingest-http        descarga de GitHub + detección de licencia      │
│ github-store      PROTOTIPO: GitHub como fuente de verdad          │
└────────────────────────────────────────────────────────────────────┘
```

La inversión de dependencias se mantiene en el límite hexagonal: las
herramientas dependen del trait `MemoryRepository` definido en
`store-core`, y tanto `InMemoryStore` como `SupabaseStore` implementan
ese puerto. Así el núcleo no conoce PostgreSQL, Vercel, GitHub ni
Gemini.

### Regla de dependencias del workspace

- **Núcleo y puertos sin dependencias externas de producción:**
  `memory-model`, `hash-core`, `json-mini`, `okf-core`, `graph-core`,
  `conflict-core`, `store-core`, `memory-store`, `memory-tools`,
  `mcp-core`, `mcp-stdio`, `mcp-http` e `ingest-core`.
- **Adaptadores con dependencias externas permitidas:**
  - `vercel-entry`: `tokio`, `axum`, `tower`, `tower-http`,
    `vercel_runtime`, `sqlx`, `jsonwebtoken`, `reqwest`, `serde` y
    `serde_json` para la función serverless, CORS, OAuth/JWT y acceso a
    PostgreSQL.
  - `supabase-store`: `tokio`, `sqlx`, `pgvector`, `serde`,
    `serde_json`, `reqwest` y `gemini-embeddings` para persistencia
    PostgreSQL/Supabase y búsqueda semántica opcional.
  - `outbox-worker`: `tokio`, `sqlx`, `pgvector`, `serde`,
    `serde_json`, `reqwest`, `base64` y `gemini-embeddings` para
    procesar eventos pendientes y sincronizar con servicios externos.
  - `gemini-embeddings`: `reqwest`, `serde` y `thiserror` para llamar a
    la API de embeddings de Gemini, Mistral o Cohere (fallback
    multi-proveedor con etiquetado de modelo, ver más abajo).
  - `ingest-http`: `tokio`, `reqwest`, `serde` y `serde_json` para
    descargar fuentes de GitHub y consultar su licencia (campo
    `license.spdx_id` de la API de repos) para la herramienta
    `skill_ingest` — sin cliente LLM: esa herramienta no sintetiza
    contenido con ningún modelo.
- `json-mini` aparece como *dev-dependency* en algunos crates solo para
  parsear aserciones de tests.

Las dependencias de los adaptadores se auditan en CI con `cargo deny`
(advisories RUSTSEC, lista blanca de licencias, duplicados y fuentes;
política en [`deny.toml`](deny.toml)) — es el criterio 2 del apéndice
[la rueda de serie](tutorial/la-rueda-de-serie.md) convertido en paso
de workflow.

Todos los crates llevan `#![forbid(unsafe_code)]`. Si se usa
`scripts/check-std-only.sh`, debe interpretarse como una comprobación
del núcleo y de los puertos `std`-only, excluyendo explícitamente los
adaptadores de frontera e infraestructura anteriores.

## Uso

```bash
cargo test               # toda la suite (doctests incluidos)
./scripts/check-docs.sh  # docs del núcleo sin warnings + doctests
cargo doc --no-deps --open        # la referencia de API, en local
cargo run -p mcp-stdio   # servidor MCP por stdio
PORT=8787 cargo run -p mcp-http   # servidor MCP por HTTP (POST /mcp)
```

Ejemplo de sesión manual (una petición JSON por línea):

```bash
cargo run -p mcp-stdio <<'EOF'
{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"manual","version":"0"}}}
{"jsonrpc":"2.0","method":"notifications/initialized"}
{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"memory_commit","arguments":{"concept_id":"people/alice","reason":"alta","markdown":"---\ntype: person\ntitle: Alice\n---\nTrabaja en [[projects/okf-mcp]].\n"}}}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"memory_resolve","arguments":{"concept_id":"people/alice"}}}
EOF
```

Para conectarlo a Claude Code como servidor MCP local:

```bash
claude mcp add okf-memory -- cargo run -q -p mcp-stdio
```

En local, `mcp-stdio` y `mcp-http` usan `InMemoryStore`: la memoria
vive en RAM y cada proceso empieza vacío. La persistencia de producción
existe en `SupabaseStore`, que usa PostgreSQL/Supabase desde el
adaptador `vercel-entry` cuando está configurada la variable
`POSTGRES_URL`.

Lo mismo por HTTP local:

```bash
PORT=8787 cargo run -q -p mcp-http &
curl -s -X POST http://127.0.0.1:8787/mcp -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"memory_commit","arguments":{"concept_id":"people/alice","reason":"alta","markdown":"---\ntype: person\ntitle: Alice\n---\nhola\n"}}}'
```

`ALLOWED_ORIGINS` (lista separada por comas) restringe qué `Origin` de
navegador se acepta; sin configurar, cualquier origin pasa — aceptable
en desarrollo, nunca en producción.

## Las 17 herramientas

| Herramienta | Qué hace |
| --- | --- |
| `memory_search` | candidatos compactos de búsqueda híbrida (textual + semántica) |
| `memory_resolve` | Markdown exacto + vecindario acotado del grafo de `[[enlaces]]` |
| `memory_commit` | escritura con compare-and-swap (`expected_hash`) y `dry_run` |
| `memory_history` | revisiones de más reciente a más antigua, paginadas |
| `memory_delete` | borrado lógico con expected_hash |
| `memory_list` | listar metadatos de conceptos bajo un prefijo sin leer contenido |
| `memory_backlinks` | obtener enlaces entrantes hacia un concepto |
| `memory_embed` | forzar generación e indexación de embeddings pendientes |
| `memory_patch` | actualizar campos de frontmatter selectivamente sin alterar el cuerpo |
| `memory_bulk_commit` | commits en lote, con opción de atómico (rollback completo) |
| `memory_validate` | reportar enlaces rotos, referencias a borrados y embeddings obsoletos |
| `memory_status` | resumen operativo rápido de la salud del sistema |
| `memory_stats` | estadísticas del grafo (hubs, huérfanos, recuentos de tipos/tags) |
| `spec_propose` | crear un `spec` (requisitos + diseño) spec-driven, antes de implementar |
| `spec_tasks` | descomponer un spec ya propuesto en tareas enlazadas y rastreables |
| `spec_status` | progreso de un spec en una sola llamada (retomar trabajo, o que otro agente pregunte) |
| `skill_ingest` | ingerir skills de un repo/carpeta/archivo externo del lado del servidor |

## UI interactiva (`mcp-app/`)

Las 13 herramientas `memory_*` (todas menos `spec_*` y `skill_ingest`)
anuncian `ui_resource_uri: Some("ui://okf-memory/app")`: un cliente MCP
Apps compatible (capítulo 15 del tutorial explica el protocolo) las
renderiza en un iframe en vez del JSON crudo.

Esa vista es una app React independiente en [`mcp-app/`](mcp-app/)
(starter Vite + shadcn + `@modelcontextprotocol/ext-apps`, tema
**brutalista**: negro/blanco, un acento amarillo eléctrico, cero
radio de esquina, sombras duras, monoespaciada), con un componente por
herramienta (`mcp-app/src/tools/<nombre>/`) enrutado en tiempo de
ejecución por el `toolName` que inyecta el host — el mismo patrón de
"micro-manifest" del starter. Dos piezas hacen el grafo de conceptos
interactivo:

- **[React Flow](https://reactflow.dev/ui)** (`@xyflow/react`) para
  `memory_resolve` (vecindario) y `memory_backlinks` (enlaces
  entrantes): layout radial determinista, click en un nodo vuelve a
  llamar a la herramienta con ese `concept_id` y recentra el grafo.
- **[Recharts](https://ui.shadcn.com/charts)** para `memory_stats`,
  `memory_history` y `memory_validate` (conteos por tipo/tag, hubs,
  revisiones por actor, salud del grafo).

`pnpm build` en `mcp-app/` compila TODO — JS, CSS y los estilos de
React Flow — en un único `dist/mcp-app.html` autocontenido
(`vite-plugin-singlefile`); `pnpm run build:sync` además lo copia a
[`crates/memory-tools/assets/mcp-app.html`](crates/memory-tools/assets/mcp-app.html),
que es lo que `include_str!` empotra en el binario. El build de
Vercel (sección siguiente) solo compila Rust — nunca ejecuta `pnpm` —
así que ese HTML compilado **tiene que estar comiteado**;
[`.github/workflows/mcp-app.yml`](.github/workflows/mcp-app.yml)
reconstruye la UI en cada push/PR y falla si la copia en el repo no
coincide con un build fresco, para que eso nunca quede desincronizado
en `main`. Detalles de arquitectura del propio starter en
[`mcp-app/docs/`](mcp-app/docs/).

### `skill_ingest`

Ingiere skills desde una fuente externa **sin que el contenido pase por
el contexto del modelo cliente, ni por ningún LLM del servidor**: el
servidor descarga, detecta el formato, empaqueta y commitea; al
cliente solo le llega el resumen del resultado. Es el mismo principio
que `memory_embed` — delegar el trabajo pesado al servidor.

```json
{"name":"skill_ingest","arguments":{
  "source":"udapy/rust-agentic-skills",
  "path_prefix":"skills/programming",
  "dry_run":true
}}
```

- `source`: URL de repo, subcarpeta (`.../tree/main/skills`) o archivo
  de GitHub, el atajo `owner/repo`, o una URL directa a un archivo.
- `format` (opcional): `auto` (por defecto), `agentic-skills`
  (convención `SKILL.md` por subdirectorio, la de `npx skills add`),
  `shadcn` (`components/ui/*.tsx`), `okf` (ya es OKF → commit con los
  bytes exactos) o `raw` (envolver markdown tal cual).
- `dry_run` (opcional): devuelve el plan (unidades, títulos, acciones,
  avisos) sin escribir nada.

La conversión es SIEMPRE determinista: se genera solo la cabecera OKF
(`type: skill`, `title`, `tags`, `source`, `license`) y el contenido
original se conserva ÍNTEGRO, etiquetado `verbatim-import`. No existe
un modo de síntesis con LLM — se evaluó y se descartó a propósito:
reescribir con un modelo no resuelve nada de licencia (una reescritura
sigue siendo obra derivada) y además cuesta cuota/tokens en cada
ingesta. El contenido de una skill ES la skill.

Un repo con varias skills (`skills/*/SKILL.md`) produce un concepto
por skill en una sola llamada, con el mismo mecanismo interno que
`memory_bulk_commit` (no atómico: cada unidad se aplica o se descarta
por su cuenta y el resumen lo cuenta todo). La reingesta es
idempotente si nada cambió; si el concepto ya existe con otro
contenido, la unidad se descarta con un aviso — actualizar exige
`memory_commit` con `expected_hash`, como cualquier otra escritura.

La herramienta solo se anuncia en despliegues con el adaptador de
descarga configurado (`vercel-entry`); `mcp-stdio` y `mcp-http`
locales son `std`-only y no la exponen. `GITHUB_TOKEN` (opcional)
sube el límite de peticiones de la API de GitHub y permite repos
privados.

#### Licencia y contenido sospechoso: señales deterministas, sin modelo

La respuesta de `skill_ingest` incluye dos señales que **nunca
bloquean nada**, calculadas sin llamar a ningún LLM (barato: solo texto
y una consulta HTTP ya necesaria):

- **`license`**: identificador SPDX de la fuente (campo
  `license.spdx_id` de la API de repos de GitHub — la misma llamada
  que ya se hace para resolver la rama por defecto). `null` si GitHub
  no lo detecta. Es puramente informativo: muchas fuentes de skills
  (pensadas para `npx skills add` y similares) se publican
  precisamente para copiarse, así que exigir una licencia confirmada
  aquí sería fricción sin valor real.
- **`warnings`** por unidad: heurísticos de texto (sin modelo, sin red)
  sobre contenido potencialmente malicioso en lo que se va a ingerir
  como instrucciones para un agente — frases de prompt injection
  conocidas (`"ignore previous instructions"` y similares), un
  `curl`/`wget` canalizado directo a un shell, o un bloque largo con
  pinta de base64. Revísalos tú (o un subagente) antes de confiar en el
  contenido; el servidor nunca decide por ti.

Owners de confianza (`SKILL_INGEST_TRUSTED_OWNERS`, por defecto solo
`anthropics`) siguen pasando por el heurístico, pero sus avisos no
viajan en la respuesta — sus repos de skills ya pasan por revisión
propia, así que el mismo escrutinio ahí sería ruido.

### Spec-driven development: `spec_propose` / `spec_tasks` / `spec_status`

Tres herramientas para el mismo patrón que popularizaron [OpenSpec](https://github.com/Fission-AI/OpenSpec)
y [GitHub Spec Kit](https://github.com/github/spec-kit) — acordar requisitos y
diseño ANTES de escribir código — pero sobre la memoria compartida en vez de
archivos locales: cualquier cliente MCP (Claude Code, ChatGPT, u otro) puede
proponer el spec, y **cualquier otro** (en otra sesión, en otro momento,
incluso en otro agente) puede retomarlo o preguntar el progreso, porque el
estado no vive en el contexto de una conversación — vive en el grafo.

No hay tipos ni tablas nuevas: un `spec` es un concepto `type: spec` con
secciones "Requisitos"/"Diseño"; una `task` es `type: task` enlazada de vuelta
con `[[implements:<spec_id>]]` y, opcionalmente, a otras tareas con
`[[depends_on:<task_id>]]`. El estado de ambos es un tag `status-*`
(`status-proposed`, `status-pending`, `status-in_progress`, `status-done`,
`status-blocked`), así que avanzar una tarea es un `memory_patch` normal
(`remove_tags`/`add_tags`) — no hace falta una cuarta herramienta para eso.

```json
{"name":"spec_propose","arguments":{
  "concept_id":"specs/busqueda-hibrida-real",
  "title":"Hybrid search en una sola consulta SQL",
  "requirements":"Combinar ranking textual y semántico en un solo ORDER BY...",
  "design":"Normalizar ambas distancias a [0,1] y sumarlas con un peso configurable..."
}}
```

```json
{"name":"spec_tasks","arguments":{
  "spec_id":"specs/busqueda-hibrida-real",
  "tasks":[
    {"title":"Normalizar distancia de coseno a 0-1"},
    {"title":"Añadir peso configurable", "description":"Via budget o argumento de memory_search",
     "depends_on":["Normalizar distancia de coseno a 0-1"]}
  ]
}}
```

`depends_on` acepta el título de otra tarea de este MISMO lote (como arriba),
o el `concept_id` de una tarea ya existente (dependencia cruzada con otro
`spec_tasks` anterior, incluso de otro spec).

```json
{"name":"spec_status","arguments":{"spec_id":"specs/busqueda-hibrida-real"}}
```

`spec_status` devuelve el estado del propio spec, cuántas tareas hay por
estado, el progreso (0-1), y dos listas separadas calculadas con `backlinks()`
(ya existente) sin releer cada tarea una por una:
- **`next_pending`**: tareas pendientes que YA se pueden empezar — todas sus
  `depends_on` están `done` (o no tienen ninguna).
- **`waiting_on_dependencies`** (recuento): pendientes que aún esperan por
  otra tarea. No aparecen en `next_pending` hasta que su dependencia se
  marque `done`.

## Desplegar en Vercel

1. En el dashboard de Vercel: **Add New Project** → importa
   `pmaojo/okf-mcp` desde GitHub.
2. **Importante:** en la configuración del proyecto, fija
   **Root Directory** = `crates/vercel-entry` — ahí es donde vive el
   `Cargo.toml` + `api/mcp.rs` que el *builder* de Rust de Vercel
   espera encontrar (el repo entero es un *workspace* de Cargo; este
   crate es el adaptador que sabe hablar con Vercel).
3. Configura las variables de entorno necesarias:
   - `POSTGRES_URL` (obligatoria): cadena de conexión PostgreSQL usada
     por `SupabaseStore` y por el endpoint de outbox.
   - `ALLOWED_ORIGINS` (muy recomendada): lista de origins permitidos,
     separada por comas. Sin ella, cualquier origin de navegador se
     acepta.
   - `JWKS_URL` y `JWT_AUDIENCE` (recomendadas en producción): activan
     validación criptográfica de JWTs; sin `JWKS_URL`, el adaptador MCP
     corre en modo local/desarrollo abierto.
   - `OAUTH_ISSUER` y `SUPABASE_ANON_KEY`: habilitan el proxy OAuth y la
     pantalla de consentimiento hacia Supabase.
   - `GEMINI_API_KEY` (opcional): primer proveedor de búsqueda
     semántica/embeddings; sin ella (o sin ningún proveedor
     configurado), la búsqueda degrada a coincidencia textual. No la
     usa `skill_ingest` — esa herramienta no llama a ningún LLM.
   - `MISTRAL_API_KEY`, `COHERE_API_KEY` (opcionales): respaldo
     automático de embeddings si Gemini falla — ver la tabla de
     proveedores de embeddings más abajo.
   - `GITHUB_TOKEN` (opcional): lo usa `skill_ingest` para subir el
     límite de peticiones de la API de GitHub y acceder a repos
     privados al descargar fuentes.
   - `SKILL_INGEST_TRUSTED_OWNERS` (opcional): lista separada por comas
     de owners cuyo escrutinio de contenido sospechoso en
     `skill_ingest` se omite en la respuesta; por defecto solo
     `anthropics`.
4. Si usas la integración de Supabase en el marketplace de Vercel,
   mapea sus credenciales a los nombres anteriores. El código actual
   espera `POSTGRES_URL` para la conexión de base de datos.

[`crates/vercel-entry/vercel.json`](crates/vercel-entry/vercel.json)
reescribe `/mcp` → `/api/mcp` (y el descubrimiento OAuth,
`/.well-known/oauth-protected-resource` → `/api/mcp`) para que la URL
pública sea la que promete el resto de esta documentación. Vive DENTRO
de `crates/vercel-entry`, no en la raíz del repo: como el **Root
Directory** del proyecto está fijado ahí (punto anterior), Vercel solo
lee `vercel.json` relativo a esa carpeta — un `vercel.json` en la raíz
del repo se ignora en silencio.

## Prototipo: GitHub como fuente de verdad (`github-store`)

`GithubStore` explora sustituir Postgres por un repositorio de GitHub
como almacén primario: los documentos son archivos markdown en una
rama, el CAS lo cierra el parámetro `sha` de la API de contents, la
historia de revisiones viaja en trailers `Memory-Rev:` de los mensajes
de commit, y el lote atómico usa la API de git data (tree → commit →
update de ref sin force, todo-o-nada real).

Pasa **la misma suite de contrato** que `InMemoryStore` y
`SupabaseStore` (`store_core::contract::run_all`), ejecutada contra
una API de GitHub falsa en memoria
(`crates/github-store/tests/contract.rs`); el smoke contra la API real
es `cargo run -p github-store --example smoke -- owner/repo` (escribe
de verdad: usar un repo de pruebas).

### Cómo activarlo

La variable `OKF_STORE` selecciona el backend en los entry points:

- `mcp-stdio`: `OKF_STORE=github` (defecto: `memory`)
- `vercel-entry`: `OKF_STORE=github` (defecto: `supabase`)

Variables de entorno de `GithubStore::from_env()`:

| Variable | Obligatoria | Formato | Defecto |
| -------- | ----------- | ------- | ------- |
| `GITHUB_REPO` | Sí | `owner/repo` | — |
| `GITHUB_TOKEN` | Sí | token PAT o fine-grained | — |
| `GITHUB_BRANCH` | No | nombre de rama | `main` |
| `GITHUB_PATH` | No | prefijo de directorio | `memoria` |

Para resolver esto, el servidor implementa el modo de almacenamiento compuesto **`IndexedStore`** (se activa automáticamente si `OKF_STORE=github` y `POSTGRES_URL` están configurados en el entorno): las escrituras van sincrónicamente a GitHub y el índice semántico se actualiza en Supabase. 

Además, el daemon de `outbox-worker`, el Cron de Vercel y el webhook de GitHub (`/api/github-webhook`, ver más abajo) incorporan un **bucle de reconciliación** (`reconcile_github_to_supabase`) que alinea Supabase con el estado real del repositorio de GitHub (reparando el índice ante caídas o cambios directos hechos en la web de GitHub).

En modo `IndexedStore`, `GithubStore` ya escribe cada commit/delete directamente en `{GITHUB_PATH}/{concept_id}.md` (git-data API); `outbox-worker::github_sync` (Contents API, ver la sección de Outbox) escribe en la misma ruta — comparten la función que resuelve `GITHUB_PATH` para que nunca puedan apuntar a carpetas distintas — pero se salta ese paso automáticamente cuando `OKF_STORE=github`, porque GitHub ya recibió la escritura y repetirla ahí sería un commit duplicado.


## Outbox, GitHub y embeddings

El hito 4 se implementa con un patrón **Transactional Outbox**: las
escrituras persistidas generan eventos pendientes, y un worker separado
los procesa por lotes con `SKIP LOCKED` para permitir concurrencia sin
pisarse.

Hay tres formas de dispararlo:

- `cargo run -p outbox-worker`: daemon de larga duración pensado para
  Fly.io, Railway, un contenedor o un VPS. Repite el procesamiento cada
  pocos segundos cuando no hay trabajo.
- `/api/outbox` en `vercel-entry`: handler serverless pensado para
  Vercel Cron. Ejecuta un lote por invocación; el cron está declarado en
  `crates/vercel-entry/vercel.json` (por defecto, una vez al día — es la
  red de seguridad, no el camino rápido).
- `/api/github-webhook` en `vercel-entry`: reacciona a un `push` real en
  el repositorio de GitHub y ejecuta `reconcile_github_to_supabase` al
  instante, en vez de esperar al próximo tick del cron. Ver la
  subsección siguiente.

Variables de entorno:

| Variable | Obligatoria | Uso |
| -------- | ----------- | --- |
| `POSTGRES_URL` | Sí | Conexión PostgreSQL/Supabase para leer y marcar eventos. |
| `GITHUB_TOKEN` | No | Token para sincronizar documentos con GitHub. Si falta, se omite esa sincronización. |
| `GITHUB_REPO` | No | Repositorio destino en formato `usuario/repositorio`. Si falta, se omite GitHub. |
| `GITHUB_PATH` | No | Prefijo de directorio (mismo default `memoria` que `GithubStore::from_env`, ver arriba). Comparte la misma resolución que la lectura, para que escritura y reconciliación nunca miren carpetas distintas. |
| `GEMINI_API_KEY` | No | Genera embeddings para `pgvector` (primer proveedor); si falta o falla, cae a `MISTRAL_API_KEY`/`COHERE_API_KEY` si están configuradas — ver la sección de embeddings más abajo. Si ninguna está presente, se omite esa parte. |
| `MISTRAL_API_KEY`, `COHERE_API_KEY` | No | Respaldo automático de embeddings si Gemini falla o no está configurada. |
| `ONCE` | No | En el daemon local, procesa un lote y sale cuando está presente. |
| `CRON_SECRET` | Recomendado en Vercel | Protege `/api/outbox` con `Authorization: Bearer <CRON_SECRET>`. Sin él, el endpoint queda abierto para desarrollo. |
| `GITHUB_WEBHOOK_SECRET` | Obligatoria para `/api/github-webhook` | Verifica la firma `X-Hub-Signature-256` (HMAC-SHA256) que GitHub envía en cada entrega. Sin ella, el endpoint responde `503` y no procesa nada — a diferencia de `CRON_SECRET`, aquí no hay modo abierto. |

Cuando `OKF_STORE=github` (`IndexedStore` activo), `process_batch` **no**
repite la sincronización con GitHub para los eventos `commit`/`delete`
del outbox — `IndexedStore` ya escribió ahí de forma síncrona antes de
encolar el evento, así que repetirlo sería un commit duplicado por cada
escritura. Este corte lo decide
[`outbox_worker::github_sync_credentials`](crates/outbox-worker/src/lib.rs),
que ambos disparadores (`main.rs` y `api/outbox.rs`) consultan en vez de
leer `GITHUB_TOKEN`/`GITHUB_REPO` directamente. El paso de embeddings del
outbox no se ve afectado — sigue funcionando como red de reintento si el
embedding inline del commit falló.

El crate `gemini-embeddings` centraliza el fallback multi-proveedor de
embeddings (Gemini, con Mistral y Cohere como respaldo automático —
ver la sección siguiente) y lo reutilizan tanto `supabase-store` para
búsqueda semántica como `outbox-worker` para materializar embeddings:
los dos DEBEN pasar por el mismo punto de entrada para que nunca
puedan divergir en qué proveedor llamaron ni en qué modelo etiquetaron
el vector resultante.

#### Fallback multi-proveedor de embeddings

Los embeddings de proveedores distintos **no son comparables entre
sí** aunque compartan dimensionalidad: cada modelo aprende su propio
espacio vectorial, y comparar por coseno un vector de un proveedor
contra el de otro no da un error, da un ranking sin ningún significado
(por esta misma razón `skill_ingest` no sintetiza contenido con
ningún LLM — ver la sección de `skill_ingest` más arriba). Por eso el
fallback de embeddings no es un simple "probar el siguiente" — cada
vector se persiste junto al identificador exacto del proveedor+modelo
que lo produjo (columna `embeddings.embedding_model`), y
`search_semantic` **solo** compara vectores con el mismo
`embedding_model` que la consulta. Un documento indexado con el
proveedor de respaldo mientras Gemini estaba caído simplemente queda
fuera del ranking semántico de una consulta embebida con otro
proveedor (sigue siendo encontrable por coincidencia de texto) hasta
que se re-indexe — degradación segura, nunca corrupción silenciosa.

| Variable | Proveedor | Modelo | Dimensiones |
| -------- | --------- | ------ | ------------ |
| `GEMINI_API_KEY` | Gemini (primero) | `gemini-embedding-001` (truncado) | 768 |
| `MISTRAL_API_KEY` | Mistral (respaldo) | `mistral-embed` | 1024 |
| `COHERE_API_KEY` | Cohere (respaldo) | `embed-english-v3.0` | 1024 |

### Webhook de GitHub (reconciliación instantánea)

Sin el webhook, un borrado o edición hecho directamente en GitHub (fuera
de las herramientas MCP) tarda hasta el próximo tick del cron en
reflejarse en Supabase — con el cron diario por defecto, hasta 24h en
las que la búsqueda seguiría devolviendo un concepto ya borrado. El
webhook cierra esa ventana a segundos.

Configuración, en el repositorio de GitHub que apunta `GITHUB_REPO`
(**no** en este repo de código — el webhook se registra donde vive el
contenido):

1. Genera un secreto: `openssl rand -hex 32`.
2. Configúralo como `GITHUB_WEBHOOK_SECRET` en las variables de entorno
   de Vercel.
3. En el repo de contenido → Settings → Webhooks → Add webhook:
   - Payload URL: `https://<tu-dominio>/api/github-webhook`
   - Content type: `application/json`
   - Secret: el mismo valor del paso 1
   - Which events: "Just the push event"

El handler verifica la firma en tiempo constante, ignora eventos que no
sean `push` o que no sean sobre `GITHUB_BRANCH` (por defecto `main`), y
delega en la misma `reconcile_github_to_supabase` que usa el cron — no
hay lógica de reconciliación duplicada entre ambos disparadores.

## Tutorial

En [`tutorial/`](tutorial/) — en español, un capítulo por invariante,
con la estructura: problema → invariante → implementación mínima →
versión rota → por qué falla → memoria y asignación → tests → frontera
de producción → principios SOLID en juego → ejercicios.
