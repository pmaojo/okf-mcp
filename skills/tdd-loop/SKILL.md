---
name: tdd-loop
description: "Red-green-refactor loop for this workspace's crates, scoped with cargo test -p <crate>. Use before implementing any change to conflict-core, consolidate-core, memory-tools, or any other crate, especially where existing coverage is thin."
---

# TDD Loop for okf-mcp

Coverage is uneven across this workspace — `memory-model` and
`store-core` are thoroughly tested, but coordination-heavy crates like
`conflict-core` and `consolidate-core` have thinner suites. A change
that lands without a preceding failing test in one of those crates is
the likeliest place for a silent regression, since nothing else will
catch it.

## The loop

1. **RED** — write the test against the crate's existing test module
   (or add one) asserting the real expected behavior, not just that the
   code compiles. Run it scoped to the crate:
   ```sh
   cargo test -p <crate>
   ```
   Confirm it fails on an assertion, not a compile error. A compile
   error only proves the function doesn't exist yet — stub it, then get
   a real red.

2. **GREEN** — write the minimum implementation that passes. Don't
   implement adjacent behavior "while you're in there"; that belongs in
   its own RED step.

3. **REFACTOR** — with the test green, clean up under it. The
   assertion is frozen: changing what the test checks during refactor
   is a behavior change, not a refactor, and needs its own RED step.

## Which port or trait does this touch?

Before writing the test, check whether the change crosses the
hexagonal boundary:

- Pure core logic (`conflict-core`, `graph-core`, `okf-core`,
  `hash-core`) — test the crate directly, no mocks needed; these are
  `std`-only by design.
- `MemoryRepository` port (`store-core`) — a new method needs the same
  test written against **both** `InMemoryStore` and, where feasible,
  `SupabaseStore` (or a documented reason it can't be), so the Liskov
  contract in the trait's own tests stays enforced.
- `memory-tools` — test the tool's JSON-RPC handler behavior, not just
  the underlying core function; the tool layer is where malformed
  input, validation, and error shaping live.

## Before opening a PR

```sh
cargo test                                   # full workspace
cargo clippy --workspace --all-targets -- -D warnings
./scripts/check-docs.sh
```

All three are CI gates (`.github/workflows/rust.yml`) plus vord's own
SAST gate (`.github/workflows/vord.yml`). A green local run before
pushing is cheaper than a red CI run after.
