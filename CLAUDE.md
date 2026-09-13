# CLAUDE.md

Guidance for an agent working in this repository. `README.md` is the
canonical architecture reference — read it first, especially the
dependency-inversion diagram and the "Workspace dependency rule"
section. This file only adds what an agent needs before writing code.

## What this is

A persistent-memory MCP server (`okf-memory`), hexagonal architecture,
`std`-only core. 21 crates under `crates/`; the ones with no external
production dependencies are the ones to protect (`memory-model`,
`hash-core`, `json-mini`, `okf-core`, `graph-core`, `conflict-core`,
`store-core`, `memory-store`, `memory-tools`, `mcp-core`, `mcp-stdio`,
`mcp-http`, `ingest-core`). Anything needing `tokio`, `serde`, `sqlx`,
etc. belongs in an adapter crate (`supabase-store`, `vercel-entry`,
`gemini-embeddings`, `outbox-worker`, `ingest-http`, `github-store`),
never in core.

## Before changing core or a port

- Check `store-core`'s `MemoryRepository` trait and its Liskov contract
  before adding a method — both `InMemoryStore` and `SupabaseStore`
  must keep satisfying it identically.
- Search existing memory before inventing vocabulary: `memory_search`
  with `path_prefix: "ontologies"` and `type: ontology` for shared
  relation/class names, and `type: skill` for procedures already
  ingested via `skill_ingest`. Reuse an existing `ontology_id` in
  `memory_reason` rather than re-declaring axioms by hand.

## Testing

```sh
cargo test               # full suite (doctests included)
./scripts/check-docs.sh  # core docs with no warnings + doctests
cargo clippy --workspace --all-targets -- -D warnings
cargo deny check          # dependency/license audit (deny.toml)
```

Coverage is uneven across crates — some (`memory-model`, `store-core`)
are thoroughly tested, others (notably the newer coordination logic in
`consolidate-core` and `conflict-core`) have thinner suites. Treat any
new behavior in `conflict-core`, `consolidate-core`, or `memory-tools`
as needing a failing test first — see `skills/tdd-loop/SKILL.md`.

All four of `cargo test`, `check-docs.sh`, `clippy -D warnings`, and
`cargo deny check` are hard CI gates (`.github/workflows/rust.yml`),
plus this repo's own PRs are gated by `pmaojo/vord` itself
(`.github/workflows/vord.yml`, `min_health_score = 85` per
`vord.toml`) — don't treat vord's SAST pass as optional just because
it's an external Action.

## Working alongside vord

This server is the durable-memory half of vord's `agent-dev-loop`
skill (`pmaojo/vord`'s `skills/agent-dev-loop/SKILL.md`): vord gates
writes at the source-analysis level, this server persists specs,
tasks, and consolidated session summaries across sessions via
`spec_propose`/`spec_tasks`/`memory_patch`/`memory_consolidate`. If a
session in this repo also has vord wired in (`.claude/settings.json`
hooks, or the `vord-guardrail` plugin), follow that loop rather than
inventing an ad hoc workflow — recover context with `memory_search`
before writing anything.

## Git safety

No dedicated hooks in this repo yet. At minimum: never push directly
to `main` (PRs only, gated by CI above), never force-push a shared
branch, and never hand-edit `vord.toml` or `deny.toml` to make a
failing gate pass — fix what it's flagging.
