# Development Guide: 02 - Vite And Packaging

MCP Apps are easiest to ship when the UI compiles to a single HTML file. This project uses Vite with `vite-plugin-singlefile` so the final app can be served by the real MCP server without managing a folder of assets.

## Build Output

```bash
pnpm run build
```

The build command type-checks the UI and writes `dist/mcp-app.html`. JavaScript and CSS are inlined into that file.

## Tailwind

Tailwind v4 is wired through `@tailwindcss/vite`. There is no PostCSS config in this starter because the Vite plugin is enough for this build.

## Server Integration

This repository does not build or ship the MCP server — that's `crates/memory-tools` in the Rust workspace. Run `pnpm run build:sync` (or `pnpm run build` + `pnpm run sync`) to write `dist/mcp-app.html` and copy it to `crates/memory-tools/assets/mcp-app.html`, the file `include_str!` embeds in the binary. Keep each tool's `manifest.ts` `slug` aligned with its `ToolSpec::name` in `crates/memory-tools/src/lib.rs`.
