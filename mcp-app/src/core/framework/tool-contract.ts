/**
 * @file Tool Contract & Micro-Manifest Architecture Definitions
 *
 * @description
 * This file defines the core types that drive the application's "Micro-Manifest Architecture".
 * Instead of a monolithic application where the core router imports deeply nested components
 * and logic, each feature (tool) is self-contained. Tools expose a `ToolManifest` which is
 * then collected by a central registry (`src/tools/registry.ts`).
 *
 * This design enforces loose coupling, makes adding new tools purely additive, and ensures
 * that metadata required for routing and configuration is colocated with the tool's source code.
 */

import type { App, McpUiHostContext } from "@modelcontextprotocol/ext-apps";
import type { CallToolResult } from "@modelcontextprotocol/sdk/types.js";

/**
 * Standard properties passed to every Tool's React component.
 *
 * @property {App | null} app - The core MCP App instance allowing host communication (e.g., `sendMessage`).
 * @property {CallToolResult | null} toolResult - The result payload returned by the server-side tool execution.
 * @property {McpUiHostContext} [hostContext] - Context injected by the host, including details about the invoked tool.
 */
export interface ToolComponentProps {
  app: App | null;
  toolResult: CallToolResult | null;
  hostContext?: McpUiHostContext;
}

/**
 * The unified descriptor for a self-contained MCP Tool integration.
 *
 * @description
 * Every tool module must export an object conforming to this interface.
 * The registry uses this metadata to map the `toolName` provided by the host context
 * to the corresponding React component.
 *
 * @property {string} slug - Unique identifier for the tool. **Must strictly match the tool name**
 *                           registered on the server side (e.g., `"memory_search"` in
 *                           `crates/memory-tools/src/lib.rs`).
 * @property {React.ComponentType<ToolComponentProps>} component - The UI root component for the tool.
 *                           Note: Standard static imports are preferred over `React.lazy()`
 *                           in the registry to preserve strong type-checking and immediate availability.
 */
export interface ToolManifest {
  slug: string;
  title: string;
  description: string;
  version: string;
  component: React.ComponentType<ToolComponentProps>;
  config?: {
    isExperimental?: boolean;
    requiredPermissions?: Array<
      "sendMessage" | "sendLog" | "openLink" | "callServerTool"
    >;
  };
}
