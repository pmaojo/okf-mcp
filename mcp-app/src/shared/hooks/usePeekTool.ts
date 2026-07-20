import { useCallback, useState } from "react";
import type { App } from "@modelcontextprotocol/ext-apps";
import type { CallToolResult } from "@modelcontextprotocol/sdk/types.js";

/**
 * For cross-tool calls that aren't "this view's own tool" — e.g.
 * spec_status peeking at memory_resolve for one of its next_pending
 * tasks. Unlike useServerTool this isn't locked to one tool name and
 * doesn't read the host-injected initial toolResult; it's purely for
 * on-demand inline peeks triggered from within a view.
 */
export function usePeekTool(app: App | null) {
  const [result, setResult] = useState<CallToolResult | null>(null);
  const [toolName, setToolName] = useState<string | null>(null);
  const [isLoading, setIsLoading] = useState(false);
  const [isError, setIsError] = useState(false);

  const peek = useCallback(
    async (name: string, args: Record<string, unknown> = {}) => {
      if (!app) return;
      setIsLoading(true);
      setIsError(false);
      setToolName(name);
      try {
        const res = await app.callServerTool({ name, arguments: args });
        setResult(res);
      } catch {
        setIsError(true);
      } finally {
        setIsLoading(false);
      }
    },
    [app]
  );

  const reset = useCallback(() => {
    setResult(null);
    setToolName(null);
    setIsError(false);
  }, []);

  return { result, toolName, isLoading, isError, peek, reset };
}
