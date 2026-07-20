import { useEffect } from "react";
import type { ToolComponentProps } from "@/core/framework/tool-contract";
import { ToolHeader } from "@/shared/components/tool/ToolHeader";
import { ToolLayout } from "@/shared/components/tool/ToolLayout";
import { ErrorBanner, EmptyBanner } from "@/shared/components/tool/StatusBanner";
import { StatTile, StatTileGrid } from "@/shared/components/tool/StatTile";
import { Button } from "@/shared/components/ui/button";
import { useServerTool } from "@/shared/hooks/useServerTool";
import { parseToolPayload } from "@/lib/tool-result";
import type { MemoryStatusResult } from "@/lib/mcp-types";

export function MemoryStatusView({ app, toolResult }: ToolComponentProps) {
  const { activeResult, isError, isLoading, executeTool } = useServerTool(
    app,
    "memory_status",
    toolResult
  );
  const parsed = parseToolPayload<MemoryStatusResult>(activeResult);

  // memory_status takes no arguments — fetch once as soon as the host
  // bridge is ready, instead of making the user press a button first.
  useEffect(() => {
    if (app && !toolResult) void executeTool({});
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [app]);

  const hasIssues =
    parsed &&
    (parsed.missing_embeddings > 0 ||
      parsed.broken_links > 0 ||
      parsed.deleted_referenced > 0 ||
      parsed.outbox_failed > 0);

  return (
    <ToolLayout>
      <ToolHeader
        slug="memory_status"
        title="System status"
        description="A fast operational read: document counts, integrity issues, and outbox backlog."
      />

      <div>
        <Button onClick={() => executeTool({})} disabled={isLoading} variant="outline">
          {isLoading ? "Refreshing…" : "Refresh"}
        </Button>
      </div>

      {isError && <ErrorBanner title="Status check failed" />}

      {!isError && parsed && (
        <StatTileGrid>
          <StatTile label="Documents" value={parsed.documents} tone="accent" />
          <StatTile label="Deleted documents" value={parsed.deleted_documents} />
          <StatTile
            label="Missing embeddings"
            value={parsed.missing_embeddings}
            tone={parsed.missing_embeddings > 0 ? "danger" : "default"}
          />
          <StatTile
            label="Broken links"
            value={parsed.broken_links}
            tone={parsed.broken_links > 0 ? "danger" : "default"}
          />
          <StatTile
            label="Refs to deleted"
            value={parsed.deleted_referenced}
            tone={parsed.deleted_referenced > 0 ? "danger" : "default"}
          />
          <StatTile label="Outbox pending" value={parsed.outbox_pending} />
          <StatTile
            label="Outbox failed"
            value={parsed.outbox_failed}
            tone={parsed.outbox_failed > 0 ? "danger" : "default"}
          />
          <StatTile
            label="Overall"
            value={hasIssues ? "issues" : "healthy"}
            tone={hasIssues ? "danger" : "accent"}
          />
        </StatTileGrid>
      )}

      {!isError && !parsed && !isLoading && (
        <EmptyBanner>Waiting for the status check…</EmptyBanner>
      )}
    </ToolLayout>
  );
}
