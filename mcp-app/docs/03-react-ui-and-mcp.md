# Development Guide: 03 - React UI And MCP

Use the official MCP Apps SDK bridge. Do not create parallel postMessage bridges or custom stores unless the SDK cannot cover a real requirement.

## `McpProvider`

`src/core/mcp/provider/McpProvider.tsx` owns the bridge setup:

- calls `useApp`;
- exposes `app`, `error`, `hostContext`, and `toolResult`;
- syncs host styles into CSS variables;
- caches the latest `toolResult`;
- wires host callbacks to `logger`.

## Calling Server Tools

Tool views should call the server through `useServerTool`:

```tsx
const { activeResult, isError, isLoading, executeTool } = useServerTool(
  app,
  manifest.slug,
  toolResult
);
```

The real MCP server remains the source of truth for validation, side effects, and business logic. `parseToolPayload` (`src/lib/tool-result.ts`) is the one place that reads `structuredContent`/`content[0].text` and returns typed JSON (see `src/lib/mcp-types.ts`) or `null` — views should treat `null` as "no result yet", not throw.

## Cross-tool calls

`useServerTool` locks onto one tool name (its own `manifest.slug`). When a view needs to call a *different* tool — e.g. `memory-resolve`'s graph re-running `memory_resolve` on a clicked neighbor is fine with `useServerTool` since it's the same tool, but `memory-backlinks` recentering on a clicked source also just re-runs `memory_backlinks` — call `app.callServerTool({ name, arguments })` directly instead of introducing a second `useServerTool` for a name that isn't this view's own slug.
