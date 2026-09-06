# okf-mcp — a persistent-memory MCP server in Rust (`std`-only core)

A persistent memory server for agents (MCP protocol) built with a
hexagonal architecture: the **knowledge engine** and its **ports** are
written against Rust's standard library only — no frameworks, no
`serde`, no `tokio` in the core. External dependencies are confined to
boundary/infrastructure adapters: Vercel, Supabase, GitHub, and
LLM/embedding providers (Gemini as primary, with automatic
multi-provider fallback).

This repository is both a working project and a hands-on **Rust and
SOLID-principles tutorial** — see [`tutorial/`](tutorial/) (in
Spanish). Generated API docs (`cargo doc`) are published at
**<https://pmaojo.github.io/okf-mcp/>** on every push to `main`
(tutorial chapter 16).

## Status

- `std`-only core + stdio MCP server.
- Stateless HTTP transport (`mcp-http`) + an executable `MemoryRepository` contract + a Vercel adapter (`vercel-entry`) + a PostgreSQL adapter (`supabase-store`).
- OAuth 2.1 (Resource Server, cryptographic JWT validation via signatures and JWKS).
- Transactional Outbox (`outbox-worker` with concurrent `SKIP LOCKED` processing, GitHub sync, and `pgvector` embeddings).
- MCP Apps — an interactive React UI ([`mcp-app/`](mcp-app/), brutalist theme) for 18 of the 19 tools, each with its own `ui://` resource (optional — see below).
- Lightweight reasoning (`ontology-core`, tutorial chapter 18) — triples derived from existing frontmatter/links, a bounded OWL-RL/RDFS fixed point, persisted via `TripleStore` (`triples` table in Supabase) with no `oxigraph` and no external dependencies. Ontologies are declared **once** as a `type: ontology` document and reused by `ontology_id` — `ToolHandler::instructions()` tells the agent this at `initialize`, before it invents axioms of its own.
- Ranked lexical search fallback (`memory-store`) — when no embeddings provider is configured, `memory_search` no longer returns unranked substring matches: hits are scored by field (title > tags > id > body, body capped so a long document can't win purely on repetition) so the most relevant concept surfaces first even without semantic search.

## Architecture

```text
                          Inbound adapters
┌────────────────────────────────────────────────────────────────────┐
│ mcp-stdio    local binary over stdin/stdout                        │
│ mcp-http     stateless HTTP/1.1 binary on TcpListener, POST /mcp   │
│ vercel-entry Axum/Vercel serverless function → mcp_http::route()  │
└───────────────┬────────────────────────────────────────────────────┘
                │
                ▼
                      `std`-only core and ports
┌────────────────────────────────────────────────────────────────────┐
│ mcp-core     JSON-RPC 2.0 + MCP lifecycle + dispatch               │
│ json-mini    educational JSON parser/serializer                    │
│ memory-tools 19 generic MCP tools over MemoryRepository            │
│ store-core   MemoryRepository port + Liskov contract               │
│ memory-model ConceptId, ContentId, Budget, Revision, Principal     │
│ okf-core     YAML frontmatter (subset) + [[...]] links             │
│ graph-core   bounded BFS (NeighborSource trait)                    │
│ conflict-core pure compare-and-swap decisions                      │
│ hash-core    hand-written SHA-256 (NIST vectors)                   │
│ memory-store InMemoryStore for development and tests               │
│ ingest-core  skill_ingest detection/planning + ports               │
│ consolidate-core deterministic session-summary validation/render   │
└───────────────┬────────────────────────────────────────────────────┘
                │
                ▼
                    Outbound / infrastructure adapters
┌────────────────────────────────────────────────────────────────────┐
│ supabase-store    SupabaseStore implements MemoryRepository        │
│ outbox-worker     processes the outbox, GitHub sync, embeddings    │
│ gemini-embeddings  embeddings, automatic multi-provider fallback   │
│ ingest-http        GitHub downloads + license detection            │
│ github-store      PROTOTYPE: GitHub as source of truth             │
└────────────────────────────────────────────────────────────────────┘
```

Dependency inversion is enforced at the hexagonal boundary: tools
depend on the `MemoryRepository` trait defined in `store-core`, and
both `InMemoryStore` and `SupabaseStore` implement that port. The core
never knows about PostgreSQL, Vercel, GitHub, or Gemini.

### Workspace dependency rule

- **Core and ports with no external production dependencies:**
  `memory-model`, `hash-core`, `json-mini`, `okf-core`, `graph-core`,
  `conflict-core`, `store-core`, `memory-store`, `memory-tools`,
  `mcp-core`, `mcp-stdio`, `mcp-http`, and `ingest-core`.
- **Adapters allowed external dependencies:**
  - `vercel-entry`: `tokio`, `axum`, `tower`, `tower-http`,
    `vercel_runtime`, `sqlx`, `jsonwebtoken`, `reqwest`, `serde`, and
    `serde_json` for the serverless function, CORS, OAuth/JWT, and
    PostgreSQL access.
  - `supabase-store`: `tokio`, `sqlx`, `pgvector`, `serde`,
    `serde_json`, `reqwest`, and `gemini-embeddings` for
    PostgreSQL/Supabase persistence and optional semantic search.
  - `outbox-worker`: `tokio`, `sqlx`, `pgvector`, `serde`,
    `serde_json`, `reqwest`, `base64`, and `gemini-embeddings` to
    process pending events and sync with external services.
  - `gemini-embeddings`: `reqwest`, `serde`, and `thiserror` to call
    the Gemini, Mistral, or Cohere embeddings API (automatic
    multi-provider fallback with model tagging — see below).
  - `ingest-http`: `tokio`, `reqwest`, `serde`, and `serde_json` to
    download sources from GitHub and query their license
    (`license.spdx_id` from the repos API) for the `skill_ingest`
    tool — no LLM client: that tool never synthesizes content with a
    model.
- `json-mini` appears as a *dev-dependency* in a few crates purely to
  parse test assertions.

Adapter dependencies are audited in CI with `cargo deny` (RUSTSEC
advisories, license allowlist, duplicates, and sources; policy in
[`deny.toml`](deny.toml)) — this is criterion 2 of the
[la rueda de serie](tutorial/la-rueda-de-serie.md) appendix turned
into a workflow step.

Every crate declares `#![forbid(unsafe_code)]`. When run,
`scripts/check-std-only.sh` should be understood as a check on the
`std`-only core and ports, explicitly excluding the boundary and
infrastructure adapters listed above.

## Usage

```bash
cargo test               # full suite (doctests included)
./scripts/check-docs.sh  # core docs with no warnings + doctests
cargo doc --no-deps --open        # API reference, locally
cargo run -p mcp-stdio   # MCP server over stdio
PORT=8787 cargo run -p mcp-http   # MCP server over HTTP (POST /mcp)
```

Example manual session (one JSON request per line):

```bash
cargo run -p mcp-stdio <<'EOF'
{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"manual","version":"0"}}}
{"jsonrpc":"2.0","method":"notifications/initialized"}
{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"memory_commit","arguments":{"concept_id":"people/alice","reason":"created","markdown":"---\ntype: person\ntitle: Alice\n---\nWorks at [[projects/okf-mcp]].\n"}}}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"memory_resolve","arguments":{"concept_id":"people/alice"}}}
EOF
```

To register it as a local MCP server in Claude Code:

```bash
claude mcp add okf-memory -- cargo run -q -p mcp-stdio
```

Locally, `mcp-stdio` and `mcp-http` use `InMemoryStore`: memory lives
in RAM and each process starts empty. Production persistence is
`SupabaseStore`, used by the `vercel-entry` adapter via
PostgreSQL/Supabase once the `POSTGRES_URL` variable is configured.

Same thing over local HTTP:

```bash
PORT=8787 cargo run -q -p mcp-http &
curl -s -X POST http://127.0.0.1:8787/mcp -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"memory_commit","arguments":{"concept_id":"people/alice","reason":"created","markdown":"---\ntype: person\ntitle: Alice\n---\nhello\n"}}}'
```

`ALLOWED_ORIGINS` (comma-separated list) restricts which browser
`Origin` is accepted; if unset, any origin is allowed — fine for
development, never for production.

## Installing as an Agent Plugin

`plugin.json` and `mcp.json` at the repo root follow the
[Agent Plugins Specification 1.0.0](https://github.com/agentplugins/agent-plugins-spec):
any compatible client discovers the MCP server (`cargo run --release
-p mcp-stdio`, no required environment variables — defaults to
`InMemoryStore`) by reading those two files, with no manual
configuration. `OKF_STORE=github` or `OKF_STORE=supabase` switch the
backend (env vars documented in `crates/mcp-stdio/src/main.rs`);
`mcp.json` deliberately doesn't set them, since they're
credentials/deployment concerns, not part of the manifest.

Installing SKILLS (content, not the server) is a call to the
`skill_ingest` tool at runtime — see the table below — not a plugin
installation step: the server ships with no preloaded skills; the
agent pulls them from whatever source it needs, per session.

## The 20 tools

| Tool | What it does |
| --- | --- |
| `memory_search` | compact hybrid-search candidates (text + semantic); `not_type` excludes a `type`, and logically deleted items are always excluded without asking |
| `memory_resolve` | exact Markdown + a bounded neighborhood of `[[links]]` |
| `memory_reason` | bounded OWL-RL/RDFS reasoning (`ontology-core`) over the neighborhood: subclasses, transitivity, symmetry, inverse properties |
| `memory_commit` | compare-and-swap write (`expected_hash`) with `dry_run` |
| `memory_consolidate` | consolidate a session (title/summary/entities/decisions authored by the agent) as `type: session-summary`, validated and rendered to OKF with no server-side LLM |
| `memory_history` | revisions newest to oldest, paginated |
| `memory_delete` | logical deletion with `expected_hash` |
| `memory_list` | list concept metadata under a prefix without reading content |
| `memory_backlinks` | get inbound links to a concept |
| `memory_embed` | force generation and indexing of pending embeddings |
| `memory_patch` | selectively update frontmatter fields without touching the body |
| `memory_bulk_commit` | batch commits, with an optional atomic mode (full rollback) |
| `memory_bulk_patch` | batch-patch frontmatter across multiple concepts (set/remove/add_tags/remove_tags), with an optional atomic mode |
| `memory_validate` | report broken links, references to deleted concepts, and stale embeddings |
| `memory_status` | quick operational summary of system health |
| `memory_stats` | graph statistics (hubs, orphans, type/tag counts) |
| `spec_propose` | create a spec-driven `spec` (requirements + design) before implementing |
| `spec_tasks` | break a proposed spec into linked, trackable tasks |
| `spec_status` | a spec's progress in one call (resume work, or let another agent ask) |
| `skill_ingest` | ingest skills from an external repo/folder/file, server-side |

## Interactive UI (`mcp-app/`)

19 of the 20 tools (the 13 original `memory_*` tools plus
`memory_reason`, `memory_bulk_patch`, `spec_propose`, `spec_tasks`,
`spec_status`, and `skill_ingest` when advertised) return
`ui_resource_uri: Some("ui://okf-memory/<name>")` — a URI **unique per
tool**, not a shared one: several MCP Apps hosts reuse an already-open
iframe when the URI doesn't change between calls, so a per-tool URI is
what guarantees each invocation opens the right view (see tutorial
chapter 15, section 6). `memory_consolidate` is the only one without
its own view (`ui_resource_uri: None`): its result is the same compact
JSON as `memory_commit`, with nothing to gain from a dedicated iframe.
A compliant MCP Apps client renders that URI in an iframe instead of
raw JSON.

That view is a standalone React app in [`mcp-app/`](mcp-app/) (Vite +
shadcn + `@modelcontextprotocol/ext-apps`, **brutalist** theme:
black/white, one electric-yellow accent, zero corner radius, hard
shadows, monospace), with one component per tool
(`mcp-app/src/tools/<name>/`) routed at runtime by the `toolName` the
host injects — the same "micro-manifest" pattern as the starter kit,
wrapped in a per-tool `ErrorBoundary` so a render failure never blanks
the whole screen. Pieces that make the UI genuinely interactive, not
just a viewer:

- **[React Flow](https://reactflow.dev/ui)** (`@xyflow/react`) for
  `memory_resolve` (neighborhood) and `memory_backlinks` (inbound
  links): deterministic radial layout; clicking a node re-calls the
  tool with that `concept_id` and recenters the graph.
- **[Recharts](https://ui.shadcn.com/charts)** for `memory_stats`,
  `memory_history`, and `memory_validate` (counts by type/tag, hubs,
  revisions by actor, graph health).
- **Cross-tool calls** (`usePeekTool`, via `app.callServerTool`):
  `spec_status` resolves a `next_pending` task with `memory_resolve`
  without leaving the view; `spec_propose` can chain into
  `spec_tasks`; `skill_ingest` resolves each newly imported skill.
- **`app.updateModelContext`** (`AddContextButton`, **not**
  `app.sendMessage`): when a HUMAN re-runs a write/diagnostic tool
  from the UI (`memory_commit`, `memory_patch`, `memory_delete`,
  `memory_bulk_commit`, `memory_bulk_patch`, `memory_validate`,
  `spec_propose`, `spec_tasks`, `spec_status`, `skill_ingest`) with
  arguments the model never saw, that result exists only in the
  browser — the model learns nothing about it unless the UI says so
  explicitly. `updateModelContext` delivers it deferred (no
  interruption, no immediate response triggered) for the next turn;
  that's why it only fires when `useServerTool` reports `isManual` (a
  UI-triggered re-run), never for the initial result the host already
  injected — that one is already in the model's context because it
  was the model's own call.

`pnpm build` in `mcp-app/` compiles EVERYTHING — JS, CSS, and React
Flow's styles — into a single self-contained `dist/mcp-app.html`
(`vite-plugin-singlefile`); `pnpm run build:sync` also copies it to
[`crates/memory-tools/assets/mcp-app.html`](crates/memory-tools/assets/mcp-app.html),
which is what `include_str!` embeds into the binary. The Vercel build
(next section) only compiles Rust — it never runs `pnpm` — so that
compiled HTML **must be committed**;
[`.github/workflows/mcp-app.yml`](.github/workflows/mcp-app.yml)
rebuilds the UI on every push/PR and fails if the copy in the repo
doesn't match a fresh build, so it can never drift on `main`. Starter
architecture details live in [`mcp-app/docs/`](mcp-app/docs/).

### `skill_ingest`

Ingests skills from an external source **without the content ever
passing through the client model's context, or through any
server-side LLM**: the server downloads, detects the format, packages,
and commits; the client only receives a summary of the result. Same
principle as `memory_embed` — delegate the heavy work to the server.

```json
{"name":"skill_ingest","arguments":{
  "source":"udapy/rust-agentic-skills",
  "path_prefix":"skills/programming",
  "dry_run":true
}}
```

- `source`: a GitHub repo URL, subfolder (`.../tree/main/skills`) or
  file URL, the `owner/repo` shorthand, or a direct file URL.
- `format` (optional): `auto` (default), `agentic-skills` (the
  per-subdirectory `SKILL.md` convention used by `npx skills add`),
  `shadcn` (`components/ui/*.tsx`), `okf` (already OKF → commit the
  exact bytes), or `raw` (wrap the markdown as-is).
- `dry_run` (optional): returns the plan (units, titles, actions,
  warnings) without writing anything.

Conversion is ALWAYS deterministic: only the OKF header is generated
(`type: skill`, `title`, `tags`, `source`, `license`), and the
original content is preserved IN FULL, tagged `verbatim-import`. There
is no LLM synthesis mode — it was considered and deliberately dropped:
rewriting with a model doesn't resolve any licensing concern (a
rewrite is still a derivative work) and it costs quota/tokens on every
ingest. The content of a skill IS the skill.

A repo with multiple skills (`skills/*/SKILL.md`) produces one concept
per skill in a single call, using the same internal mechanism as
`memory_bulk_commit` (non-atomic: each unit is applied or dropped on
its own, and the summary reports everything). Re-ingesting is
idempotent if nothing changed; if the concept already exists with
different content, that unit is dropped with a warning — updating it
requires `memory_commit` with `expected_hash`, like any other write.

The tool is only advertised on deployments with the download adapter
configured (`vercel-entry`); local `mcp-stdio` and `mcp-http` are
`std`-only and don't expose it. `GITHUB_TOKEN` (optional) raises the
GitHub API rate limit and allows access to private repos.

#### License and suspicious content: deterministic signals, no model

The `skill_ingest` response includes two signals that **never block
anything**, computed without calling any LLM (cheap: plain text plus
one HTTP call that's already required):

- **`license`**: the source's SPDX identifier (`license.spdx_id` from
  the GitHub repos API — the same call already made to resolve the
  default branch). `null` if GitHub doesn't detect one. Purely
  informational: many skill sources (built for `npx skills add` and
  similar tools) are published specifically to be copied, so requiring
  a confirmed license here would be friction with no real value.
- **`warnings`** per unit: text-only heuristics (no model, no network)
  over content that's about to be ingested as instructions for an
  agent — known prompt-injection phrases (`"ignore previous
  instructions"` and similar), a `curl`/`wget` piped straight into a
  shell, or a long block that looks like base64. Review these yourself
  (or with a subagent) before trusting the content; the server never
  decides for you.

Trusted owners (`SKILL_INGEST_TRUSTED_OWNERS`, defaults to
`anthropics` only) still go through the heuristic, but their warnings
don't travel in the response — their skill repos already go through
their own review, so the same scrutiny here would just be noise.

### Spec-driven development: `spec_propose` / `spec_tasks` / `spec_status`

Three tools for the same pattern popularized by
[OpenSpec](https://github.com/Fission-AI/OpenSpec) and
[GitHub Spec Kit](https://github.com/github/spec-kit) — agree on
requirements and design BEFORE writing code — but over shared memory
instead of local files: any MCP client (Claude Code, ChatGPT, or
another) can propose the spec, and **any other** (in a different
session, at a different time, even a different agent) can resume it or
ask about progress, because the state doesn't live in one
conversation's context — it lives in the graph.

No new types or tables: a `spec` is a `type: spec` concept with
"Requirements"/"Design" sections; a `task` is `type: task`, linked back
with `[[implements:<spec_id>]]` and, optionally, to other tasks with
`[[depends_on:<task_id>]]`. Status for both is a `status-*` tag
(`status-proposed`, `status-pending`, `status-in_progress`,
`status-done`, `status-blocked`), so advancing a task is a plain
`memory_patch` (`remove_tags`/`add_tags`) — no need for a fourth tool.

```json
{"name":"spec_propose","arguments":{
  "concept_id":"specs/real-hybrid-search",
  "title":"Hybrid search in a single SQL query",
  "requirements":"Combine textual and semantic ranking in one ORDER BY...",
  "design":"Normalize both distances to [0,1] and sum them with a configurable weight..."
}}
```

```json
{"name":"spec_tasks","arguments":{
  "spec_id":"specs/real-hybrid-search",
  "tasks":[
    {"title":"Normalize cosine distance to 0-1"},
    {"title":"Add a configurable weight", "description":"Via budget or a memory_search argument",
     "depends_on":["Normalize cosine distance to 0-1"]}
  ]
}}
```

`depends_on` accepts either the title of another task in this SAME
batch (as above), or the `concept_id` of an existing task
(cross-referencing a prior `spec_tasks` call, even from a different
spec).

```json
{"name":"spec_status","arguments":{"spec_id":"specs/real-hybrid-search"}}
```

`spec_status` returns the spec's own status, how many tasks exist per
status, progress (0-1), and two lists computed from the already
existing `backlinks()` without re-reading every task one by one:
- **`next_pending`**: pending tasks that are ready to start now — all
  their `depends_on` are `done` (or there are none).
- **`waiting_on_dependencies`** (a count): pending tasks still waiting
  on another task. They don't appear in `next_pending` until their
  dependency is marked `done`.

#### Evidence gauntlet before `status-done`

The transition to `status-done` for a task is **enforced, not just
documented**: `memory_patch` rejects the patch if the document body
doesn't already have a `## Evidence` section with real content
(commands run and results with numbers, not an empty heading). This is
the EVIDENCE pattern from
[old-coder](https://github.com/AmazingAng/old-coder) — "the human
doesn't read the implementation; their confidence comes from two
artifacts: an executable specification approved before writing code,
and an evidence report afterward" — applied on top of the existing
`spec_propose`/`spec_tasks`/`spec_status` flow, with no new tool or
schema: `spec_propose` already covers the SPEC gate (requirements +
design approved before implementing); this gate covers EVIDENCE.

```json
{"name":"memory_commit","arguments":{
  "concept_id":"specs/real-hybrid-search/tasks/01-normalize-cosine",
  "expected_hash":"<hash from the previous memory_resolve>",
  "markdown":"<existing markdown>\n## Evidence\n\ncargo test -p store-core: 12 passed; 0 failed. line coverage on changed lines: 100% (9/9).\n",
  "reason":"gauntlet evidence"
}}
```

Only then does `memory_patch` accept `add_tags: ["status-done"]`. The
heuristic is deliberately simple (counts non-empty characters after
the heading) — it's not a substitute for a real CI gauntlet (tests,
coverage, mutation), it just prevents a task from being marked `done`
without leaving a trace of why. The CI workflow itself
([`.github/workflows/rust.yml`](.github/workflows/rust.yml)) adds the
gauntlet's lint layer: `cargo clippy --workspace --all-targets -- -D
warnings`, which fails (nonzero exit) instead of just reporting — the
old-coder rule that "printing the percentage and exiting 0 is a
report, not a restriction."

## Deploying to Vercel

1. In the Vercel dashboard: **Add New Project** → import
   `pmaojo/okf-mcp` from GitHub.
2. **Important:** in the project settings, set **Root Directory** =
   `crates/vercel-entry` — that's where the `Cargo.toml` + `api/mcp.rs`
   that Vercel's Rust builder expects live (the whole repo is a Cargo
   *workspace*; this crate is the adapter that knows how to talk to
   Vercel).
3. Configure the required environment variables:
   - `POSTGRES_URL` (required): PostgreSQL connection string used by
     `SupabaseStore` and the outbox endpoint.
   - `ALLOWED_ORIGINS` (strongly recommended): comma-separated list of
     allowed origins. Without it, any browser origin is accepted.
   - `JWKS_URL` and `JWT_AUDIENCE` (recommended in production): enable
     cryptographic JWT validation; without `JWKS_URL`, the MCP adapter
     runs in open local/development mode.
   - `OAUTH_ISSUER` and `SUPABASE_ANON_KEY`: enable the OAuth proxy and
     the Supabase consent screen.
   - `GEMINI_API_KEY` (optional): first semantic search/embeddings
     provider; without it (or any provider configured), search
     degrades to text matching. Not used by `skill_ingest` — that tool
     never calls an LLM.
   - `MISTRAL_API_KEY`, `COHERE_API_KEY` (optional): automatic
     embeddings fallback if Gemini fails — see the embeddings provider
     table below.
   - `GITHUB_TOKEN` (optional): used by `skill_ingest` to raise the
     GitHub API rate limit and access private repos when downloading
     sources.
   - `SKILL_INGEST_TRUSTED_OWNERS` (optional): comma-separated list of
     owners whose suspicious-content scrutiny in `skill_ingest` is
     omitted from the response; defaults to `anthropics` only.
4. If you use the Supabase integration from the Vercel marketplace,
   map its credentials to the names above. The current code expects
   `POSTGRES_URL` for the database connection.

[`crates/vercel-entry/vercel.json`](crates/vercel-entry/vercel.json)
rewrites `/mcp` → `/api/mcp` (and OAuth discovery,
`/.well-known/oauth-protected-resource` → `/api/mcp`) so the public URL
matches the rest of this documentation. It lives INSIDE
`crates/vercel-entry`, not at the repo root: since the project's
**Root Directory** is set there (previous point), Vercel only reads
`vercel.json` relative to that folder — a `vercel.json` at the repo
root is silently ignored.

## Prototype: GitHub as source of truth (`github-store`)

`GithubStore` explores replacing Postgres with a GitHub repository as
the primary store: documents are markdown files on a branch, CAS is
enforced by the `sha` parameter of the contents API, revision history
travels in `Memory-Rev:` commit message trailers, and the atomic batch
uses the git data API (tree → commit → non-force ref update, real
all-or-nothing).

It passes **the same contract suite** as `InMemoryStore` and
`SupabaseStore` (`store_core::contract::run_all`), run against a fake
in-memory GitHub API (`crates/github-store/tests/contract.rs`); the
real-API smoke test is `cargo run -p github-store --example smoke --
owner/repo` (it writes for real — use a test repo).

### Enabling it

The `OKF_STORE` variable selects the backend at the entry points:

- `mcp-stdio`: `OKF_STORE=github` (default: `memory`)
- `vercel-entry`: `OKF_STORE=github` (default: `supabase`)

`GithubStore::from_env()` environment variables:

| Variable | Required | Format | Default |
| -------- | -------- | ------ | ------- |
| `GITHUB_REPO` | Yes | `owner/repo` | — |
| `GITHUB_TOKEN` | Yes | PAT or fine-grained token | — |
| `GITHUB_BRANCH` | No | branch name | `main` |
| `GITHUB_PATH` | No | directory prefix | `memoria` |

To reconcile the two stores, the server implements the composite
**`IndexedStore`** storage mode (activated automatically when
`OKF_STORE=github` and `POSTGRES_URL` are both set): writes go
synchronously to GitHub, and the semantic index is updated in
Supabase.

In addition, the `outbox-worker` daemon, the Vercel Cron job, and the
GitHub webhook (`/api/github-webhook`, see below) all run a
**reconciliation loop** (`reconcile_github_to_supabase`) that aligns
Supabase with the real state of the GitHub repository (repairing the
index after outages or direct edits made on GitHub's web UI).

In `IndexedStore` mode, `GithubStore` already writes every commit/delete
directly to `{GITHUB_PATH}/{concept_id}.md` (git-data API);
`outbox-worker::github_sync` (Contents API, see the Outbox section
below) writes to the same path — they share the function that resolves
`GITHUB_PATH` so they can never point at different folders — but it
skips that step automatically when `OKF_STORE=github`, since GitHub
already received the write and repeating it there would be a duplicate
commit.

## Outbox, GitHub, and embeddings

Milestone 4 is implemented with a **Transactional Outbox** pattern:
persisted writes generate pending events, and a separate worker
processes them in batches with `SKIP LOCKED` to allow concurrency
without stepping on other workers.

Three ways to trigger it:

- `cargo run -p outbox-worker`: a long-running daemon meant for
  Fly.io, Railway, a container, or a VPS. Retries processing every few
  seconds when there's no work.
- `/api/outbox` in `vercel-entry`: a serverless handler meant for
  Vercel Cron. Runs one batch per invocation; the cron is declared in
  `crates/vercel-entry/vercel.json` (once a day by default — this is
  the safety net, not the fast path).
- `/api/github-webhook` in `vercel-entry`: reacts to a real `push` on
  the GitHub repository and runs `reconcile_github_to_supabase`
  instantly, instead of waiting for the next cron tick. See the
  subsection below.

Environment variables:

| Variable | Required | Use |
| -------- | -------- | --- |
| `POSTGRES_URL` | Yes | PostgreSQL/Supabase connection to read and mark events. |
| `GITHUB_TOKEN` | No | Token to sync documents with GitHub. If missing, that sync step is skipped. |
| `GITHUB_REPO` | No | Target repository, `user/repo` format. If missing, GitHub sync is skipped. |
| `GITHUB_PATH` | No | Directory prefix (same `memoria` default as `GithubStore::from_env`, above). Shares the same resolution as reads, so writes and reconciliation never look at different folders. |
| `GEMINI_API_KEY` | No | Generates embeddings for `pgvector` (first provider); if missing or it fails, falls back to `MISTRAL_API_KEY`/`COHERE_API_KEY` if configured — see the embeddings section below. If none are present, that step is skipped. |
| `MISTRAL_API_KEY`, `COHERE_API_KEY` | No | Automatic embeddings fallback if Gemini fails or isn't configured. |
| `ONCE` | No | In the local daemon, process one batch and exit when present. |
| `CRON_SECRET` | Recommended on Vercel | Protects `/api/outbox` with `Authorization: Bearer <CRON_SECRET>`. Without it, the endpoint is open — fine for development. |
| `GITHUB_WEBHOOK_SECRET` | Required for `/api/github-webhook` | Verifies the `X-Hub-Signature-256` (HMAC-SHA256) signature GitHub sends with each delivery. Without it, the endpoint returns `503` and processes nothing — unlike `CRON_SECRET`, there's no open mode here. |

When `OKF_STORE=github` (`IndexedStore` active), `process_batch` does
**not** repeat the GitHub sync for `commit`/`delete` outbox events —
`IndexedStore` already wrote there synchronously before enqueueing the
event, so repeating it would be a duplicate commit per write. This cut
is decided by
[`outbox_worker::github_sync_credentials`](crates/outbox-worker/src/lib.rs),
which both triggers (`main.rs` and `api/outbox.rs`) consult instead of
reading `GITHUB_TOKEN`/`GITHUB_REPO` directly. The embeddings step of
the outbox is unaffected — it still works as a retry net if the
commit's inline embedding failed.

The `gemini-embeddings` crate centralizes the multi-provider embeddings
fallback (Gemini, with Mistral and Cohere as automatic backups — see
next section) and is reused by both `supabase-store` for semantic
search and `outbox-worker` to materialize embeddings: both MUST go
through the same entry point so they can never diverge on which
provider was called or which model tagged the resulting vector.

#### Multi-provider embeddings fallback

Embeddings from different providers are **not comparable with each
other** even at the same dimensionality: every model learns its own
vector space, and comparing a vector from one provider against another
by cosine similarity doesn't error out — it produces a ranking with no
meaning (the same reason `skill_ingest` never synthesizes content with
an LLM — see the `skill_ingest` section above). So the embeddings
fallback isn't a simple "try the next one" — every vector is persisted
alongside the exact provider+model identifier that produced it
(`embeddings.embedding_model` column), and `search_semantic` **only**
compares vectors with the same `embedding_model` as the query. A
document indexed with the backup provider while Gemini was down simply
falls out of the semantic ranking for a query embedded with a
different provider (it's still findable via text matching) until it's
re-indexed — safe degradation, never silent corruption.

| Variable | Provider | Model | Dimensions |
| -------- | -------- | ----- | ---------- |
| `GEMINI_API_KEY` | Gemini (primary) | `gemini-embedding-001` (truncated) | 768 |
| `MISTRAL_API_KEY` | Mistral (fallback) | `mistral-embed` | 1024 |
| `COHERE_API_KEY` | Cohere (fallback) | `embed-english-v3.0` | 1024 |

### GitHub webhook (instant reconciliation)

Without the webhook, a delete or edit made directly on GitHub (outside
the MCP tools) can take up to the next cron tick to show up in
Supabase — with the default daily cron, up to 24h during which search
would keep returning an already-deleted concept. The webhook closes
that window to seconds.

Configuration, in the GitHub repository that `GITHUB_REPO` points to
(**not** this code repository — the webhook is registered where the
content lives):

1. Generate a secret: `openssl rand -hex 32`.
2. Set it as `GITHUB_WEBHOOK_SECRET` in Vercel's environment variables.
3. In the content repo → Settings → Webhooks → Add webhook:
   - Payload URL: `https://<your-domain>/api/github-webhook`
   - Content type: `application/json`
   - Secret: the same value from step 1
   - Which events: "Just the push event"

The handler verifies the signature in constant time, ignores events
that aren't `push` or aren't on `GITHUB_BRANCH` (default `main`), and
delegates to the same `reconcile_github_to_supabase` used by the cron
— there's no duplicated reconciliation logic between the two triggers.

## Tutorial

In [`tutorial/`](tutorial/) — in Spanish, one chapter per invariant,
following the structure: problem → invariant → minimal implementation
→ broken version → why it fails → memory and allocation → tests →
production boundary → SOLID principles at play → exercises.

## Contributing

- Run `cargo test` and `./scripts/check-docs.sh` before opening a PR.
- `cargo clippy --workspace --all-targets -- -D warnings` must be
  clean — CI enforces it as a hard gate, not a report.
- Keep the dependency rule above: no external production dependency
  in the `std`-only core or ports; anything new goes in an adapter.
- `cargo deny check` must pass against [`deny.toml`](deny.toml)
  (advisories, licenses, duplicates, sources).

## License

[MIT](LICENSE)
