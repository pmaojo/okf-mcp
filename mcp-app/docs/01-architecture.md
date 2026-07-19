# Development Guide: 01 - Architecture

This starter is the browser UI layer for an MCP App. The MCP server and the MCP host are separate systems.

## Responsibilities

- The MCP host starts the interaction, renders the app resource, injects host context, and brokers calls between the UI and the server.
- The real MCP server registers tools, validates tool input, runs business logic, and serves `mcp-app.html` as an app resource.
- This repository builds the React UI that runs inside the host iframe or webview.

## UI Flow

1. The server registers a tool name that matches a UI `manifest.slug`.
2. The host invokes the tool and loads the app resource.
3. `McpProvider` initializes the SDK bridge with `useApp`.
4. `mcp-app.tsx` reads `hostContext.toolInfo.tool.name`.
5. `registry.ts` resolves the matching tool view.
6. The tool view uses `app.callServerTool` through `useServerTool` when it needs server data.

Keep feature behavior inside `src/tools/<tool>/`. Keep `src/mcp-app.tsx` as a small router shell.
