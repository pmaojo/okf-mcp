/**
 * @file Root entry point for the MCP App starter UI.
 */
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { Toaster } from "./shared/components/ui/sonner";
import { ToolErrorBoundary } from "@/shared/components/ErrorBoundary";
import { McpProvider, useMcp } from "@/core/mcp/provider/McpProvider";
import { TOOL_COMPONENTS } from "./tools/registry";
import "./index.css";

/**
 * Main Content Component.
 *
 * @description
 * This component acts as the router for the MCP application.
 * It reads the `toolName` provided by the host context and renders
 * the corresponding tool component from the registry.
 */
export const AppContent = () => {
  const { app, error, hostContext, toolResult } = useMcp();

  if (error) {
    return (
      <div className="p-4 text-destructive flex flex-col items-center justify-center h-screen">
        <strong>Error:</strong> {error.message}
      </div>
    );
  }

  if (!app) {
    return (
      <div className="p-4 flex items-center justify-center h-screen text-muted-foreground">
        Connecting...
      </div>
    );
  }

  // Read the tool slug injected by the host environment.
  const toolName = hostContext?.toolInfo?.tool?.name;

  if (!toolName) {
    return (
      <div className="p-4">
        No tool context was provided yet. Waiting for the host to open a
        tool.
      </div>
    );
  }

  // Resolve the React view registered for that slug.
  const ToolComponent = TOOL_COMPONENTS[toolName];

  if (!ToolComponent) {
    return <div className="p-4">Tool UI for &quot;{toolName}&quot; was not found.</div>;
  }

  return (
    // Keyed by toolName: switching tools always mounts a fresh
    // component + boundary, instead of a crashed tree (or one holding
    // stale props from a previous tool) sticking around.
    <ToolErrorBoundary key={toolName} toolName={toolName}>
      <ToolComponent
        app={app}
        toolResult={toolResult}
        hostContext={hostContext}
      />
    </ToolErrorBoundary>
  );
};

// Guard the bootstrap so tests and server-side environments do not try to mount.
if (typeof document !== "undefined" && document.getElementById("root")) {
  createRoot(document.getElementById("root")!).render(
    <StrictMode>
      <McpProvider>
        <AppContent />
        <Toaster />
      </McpProvider>
    </StrictMode>
  );
}
