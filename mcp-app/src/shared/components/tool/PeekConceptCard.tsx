import { Badge } from "@/shared/components/ui/badge";
import { parseToolPayload } from "@/lib/tool-result";
import type { MemoryResolveResult } from "@/lib/mcp-types";
import type { CallToolResult } from "@modelcontextprotocol/sdk/types.js";

/** Compact preview of a memory_resolve peek, used by cross-tool "jump" actions. */
export function PeekConceptCard({
  result,
  isLoading,
  isError,
}: {
  result: CallToolResult | null;
  isLoading: boolean;
  isError: boolean;
}) {
  if (isLoading) {
    return <p className="text-xs text-muted-foreground">Resolving…</p>;
  }
  if (isError) {
    return <p className="text-xs text-destructive">Could not resolve that concept.</p>;
  }
  const parsed = parseToolPayload<MemoryResolveResult>(result);
  if (!parsed) return null;

  return (
    <div className="border-2 border-foreground bg-card p-3 shadow-brutal-sm">
      <div className="mb-1 flex flex-wrap items-center gap-2">
        <span className="font-mono text-xs font-bold">{parsed.document.concept_id}</span>
        <Badge variant="outline">{parsed.document.type}</Badge>
      </div>
      {parsed.document.title && (
        <p className="text-sm font-bold">{parsed.document.title}</p>
      )}
      <pre className="mt-2 max-h-40 overflow-auto text-xs whitespace-pre-wrap text-muted-foreground">
        {parsed.document.markdown}
      </pre>
    </div>
  );
}
