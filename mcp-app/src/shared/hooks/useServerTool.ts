import { useState, useCallback } from "react";
import type { App } from "@modelcontextprotocol/ext-apps";
import type { CallToolResult } from "@modelcontextprotocol/sdk/types.js";

interface UseServerToolResult {
  activeResult: CallToolResult | null;
  isError: boolean;
  isLoading: boolean;
  executeTool: (args?: Record<string, unknown>) => Promise<void>;
}

export function useServerTool(
  app: App | null,
  toolName: string,
  hostProvidedResult: CallToolResult | null
): UseServerToolResult {
  const [manualResult, setManualResult] = useState<CallToolResult | null>(null);
  const [manualError, setManualError] = useState(false);
  const [isLoading, setIsLoading] = useState(false);

  const executeTool = useCallback(
    async (args: Record<string, unknown> = {}) => {
      if (!app) return;
      try {
        setIsLoading(true);
        setManualError(false);
        const result = await app.callServerTool({
          name: toolName,
          arguments: args,
        });
        setManualResult(result);
      } catch {
        setManualError(true);
      } finally {
        setIsLoading(false);
      }
    },
    [app, toolName]
  );

  // Once the user re-runs a tool from the UI, prefer that newer result over
  // the one injected during the initial host invocation.
  const activeResult = manualResult ?? hostProvidedResult;
  const isError = manualError || activeResult?.isError === true;

  return {
    activeResult,
    isError,
    isLoading,
    executeTool,
  };
}
