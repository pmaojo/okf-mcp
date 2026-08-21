# okf-memory MCP app

Interactive UI for the 20 tools exposed by
[`crates/memory-tools`](../crates/memory-tools) — the 16 `memory_*`
tools plus `spec_propose`/`spec_tasks`/`spec_status`/`skill_ingest`.
React + TypeScript + Vite, compiled to a single self-contained
`dist/mcp-app.html` that the Rust server embeds at compile time via
`include_str!` and serves under a distinct `ui://okf-memory/<tool>`
resource per tool (see
[tutorial chapter 15](../tutorial/15-mcp-apps-visualizaciones.md) for
the protocol side of that, including why each tool gets its own URI
instead of one shared one).

Built on top of the
[MCP App Vite Starter](https://github.com/modelcontextprotocol/ext-apps)
— React 19, shadcn-style primitives on Base UI, and the official
`@modelcontextprotocol/ext-apps` bridge instead of a hand-rolled
`postMessage` layer.

## Design

**Brutalist theme:** flat black/white, one electric-yellow accent, red
for destructive actions, zero corner radius anywhere (enforced
globally in `src/index.css`, not per-component), thick borders, hard
offset shadows (no blur), monospace type throughout, uppercase
headings. Tokens live in `src/index.css` (`@theme inline` + `:root`/`.dark`).

**Graphs** ([React Flow](https://reactflow.dev/ui), `@xyflow/react`):
`memory_resolve`'s neighborhood and `memory_backlinks`'s incoming
links render as a deterministic radial layout
(`src/shared/components/graph/ConceptGraph.tsx`). Clicking a node
re-invokes the same tool with that `concept_id` and recenters the
graph — no page navigation, just another `callServerTool`.

**Charts** ([shadcn charts](https://ui.shadcn.com/charts) primitives
over Recharts, `src/shared/components/ui/chart.tsx` +
`src/shared/components/charts/BarChartCard.tsx`): used by
`memory_stats` (counts by type, top-linked hubs), `memory_history`
(revisions by actor) and `memory_validate` (issue counts).

**Cross-tool peeks** (`useServerTool` `usePeekTool` in
`src/shared/hooks/`): a view isn't limited to calling its own tool.
`spec_status` resolves a `next_pending` task with `memory_resolve`
inline; `spec_propose` can chain into `spec_tasks`; `skill_ingest`
resolves a freshly-imported skill — all via `app.callServerTool`
without leaving the view.

**Handing state back to the model** (`AddContextButton` in
`src/shared/components/tool/`, wrapping `app.updateModelContext` —
deliberately not `app.sendMessage`, which would inject a visible fake
user turn): when a human re-runs a write or diagnostic tool from
inside the UI with arguments the model never saw
(`memory_commit`/`memory_patch`/`memory_delete`/`memory_bulk_commit`/
`memory_bulk_patch`/`memory_validate`/`spec_propose`/`spec_tasks`/
`spec_status`/`skill_ingest`), that result only exists in the browser. This button
folds a summary into the model's context for its next turn, silently.
It's gated on `useServerTool`'s `isManual` flag — it never fires for
the initial host-provided result, since the model already has that
(it was the model's own tool call).

## Stack

- **React 19** + **TypeScript** (strict)
- **Vite 8** + `vite-plugin-singlefile` → one HTML file output
- **Tailwind v4** via `@tailwindcss/vite` (no PostCSS config)
- **shadcn-style primitives** powered by **Base UI**
- **`@modelcontextprotocol/ext-apps`** — the official MCP Apps bridge
- **`@xyflow/react`** (React Flow) for concept graphs
- **Recharts** for statistics
- **Vitest** + Testing Library for unit tests
- **pnpm** as the package manager

## Commands

```bash
pnpm install
pnpm run build        # typecheck + compile -> dist/mcp-app.html
pnpm run sync         # copy dist/mcp-app.html -> ../crates/memory-tools/assets/mcp-app.html
pnpm run build:sync   # both of the above
pnpm run watch        # rebuild dist/mcp-app.html on change
pnpm run test         # vitest
pnpm run lint         # eslint
```

`pnpm run build:sync` is what CI runs
([`.github/workflows/mcp-app.yml`](../.github/workflows/mcp-app.yml)):
it rebuilds the UI on every push/PR touching this folder and fails if
the checked-in `crates/memory-tools/assets/mcp-app.html` doesn't match
a fresh build. **Rust never runs pnpm** — Vercel's builder only
compiles `crates/vercel-entry` — so that generated file has to be
committed after every UI change; run `pnpm run build:sync` and commit
the result before opening a PR.

## Project shape

```text
mcp-app/
├── mcp-app.html                 Vite entry, compiled to one self-contained HTML
├── scripts/sync-to-rust.mjs     Copies dist/mcp-app.html into the Rust crate
├── src/
│   ├── mcp-app.tsx              UI shell that routes by MCP tool slug
│   ├── core/
│   │   ├── framework/           Tool contract types (ToolManifest, ToolComponentProps)
│   │   └── mcp/
│   │       ├── provider/McpProvider.tsx  SDK bridge: host context, result cache, theme sync
│   │       └── logger/          Toast + host log integration
│   ├── lib/
│   │   ├── mcp-types.ts         TS mirrors of the tools' JSON response shapes
│   │   └── tool-result.ts       Parses CallToolResult -> typed payload
│   ├── shared/
│   │   ├── components/ui/       shadcn-style primitives (Base UI under the hood)
│   │   ├── components/tool/     Layout, tables, stat tiles, commit/spec result cards,
│   │   │                        PeekConceptCard, AddContextButton
│   │   ├── components/graph/    ConceptGraph (React Flow)
│   │   ├── components/charts/   BarChartCard (Recharts)
│   │   ├── components/ErrorBoundary.tsx  Per-tool crash containment
│   │   └── hooks/useServerTool.ts, usePeekTool.ts
│   └── tools/
│       ├── memory-search/ … skill-ingest/   one manifest.ts + view.tsx per tool
│       └── registry.ts          Static registry mapping slug -> component
└── docs/                        Architecture notes carried over from the starter
```

## How the UI talks to MCP

1. `crates/memory-tools` registers each `ToolSpec` with its own
   `ui_resource_uri: Some("ui://okf-memory/<tool_name>")` — a distinct
   URI per tool, all pointing at the same compiled HTML. This matters:
   some MCP Apps hosts reuse an already-open iframe when the URI
   doesn't change between calls, which breaks routing if two different
   tools share one URI (see tutorial chapter 15, section 6).
2. A compatible MCP client opens that resource in an iframe and
   injects host context.
3. `McpProvider` (`src/core/mcp/provider/McpProvider.tsx`) calls
   `useApp` from `@modelcontextprotocol/ext-apps/react` to set up the
   bridge and exposes `app`, `hostContext`, and `toolResult` via
   `useMcp()`.
4. `src/mcp-app.tsx` reads `hostContext.toolInfo.tool.name` and
   resolves the matching component from `src/tools/registry.ts` — the
   slug must equal the exact server-side tool name (e.g.
   `"memory_search"`) — then mounts it inside a `ToolErrorBoundary`
   keyed by that name, so a render crash shows a message instead of a
   blank screen.
5. Views call `useServerTool(app, manifest.slug, toolResult)` to run
   their own tool, or `app.callServerTool({ name, arguments })` /
   `usePeekTool` directly for cross-tool calls (e.g. clicking a graph
   node, or `spec_status` resolving a task).
6. When a manual re-run (`useServerTool`'s `isManual`) produces state
   the model hasn't seen, views offer `AddContextButton` to fold a
   summary into `app.updateModelContext` for the model's next turn.

## Adding or changing a tool view

Each tool lives in `src/tools/<tool-name>/`:

- **`manifest.ts`** — `slug` must exactly match the tool name in
  `crates/memory-tools/src/lib.rs`.
- **`view.tsx`** — the React component. If the JSON shape changed on
  the Rust side, update `src/lib/mcp-types.ts` first.

Register new tools with a **static import** in `src/tools/registry.ts`
— dynamic `import()` breaks the single-file build (`vite-plugin-singlefile`
inlines only what's statically reachable from `mcp-app.html`).

## Things to avoid

- Don't hardcode local absolute paths in code, tests, or configuration.
- Don't commit secrets.
- Don't break the single-file build: no runtime `import()`, no
  `import.meta.glob`, no external asset URLs — everything has to be
  statically reachable so Vite can inline it.
- Don't forget `pnpm run build:sync` after a UI change — CI will
  catch it, but it's faster to catch it yourself.

## PR checklist

```bash
pnpm run lint
pnpm run test
pnpm run build:sync   # then git add crates/memory-tools/assets/mcp-app.html
```
