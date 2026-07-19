import { useEffect } from "react";
import type { ToolComponentProps } from "@/core/framework/tool-contract";
import { ToolHeader } from "@/shared/components/tool/ToolHeader";
import { ToolLayout } from "@/shared/components/tool/ToolLayout";
import { ErrorBanner, EmptyBanner } from "@/shared/components/tool/StatusBanner";
import { StatTile, StatTileGrid } from "@/shared/components/tool/StatTile";
import { BarChartCard } from "@/shared/components/charts/BarChartCard";
import { Button } from "@/shared/components/ui/button";
import { Badge } from "@/shared/components/ui/badge";
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "@/shared/components/ui/card";
import { useServerTool } from "@/shared/hooks/useServerTool";
import { parseToolPayload } from "@/lib/tool-result";
import type { MemoryStatsResult } from "@/lib/mcp-types";

export function MemoryStatsView({ app, toolResult }: ToolComponentProps) {
  const { activeResult, isError, isLoading, executeTool } = useServerTool(
    app,
    "memory_stats",
    toolResult
  );
  const parsed = parseToolPayload<MemoryStatsResult>(activeResult);

  useEffect(() => {
    if (app && !toolResult) void executeTool({});
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [app]);

  return (
    <ToolLayout>
      <ToolHeader
        slug="memory_stats"
        title="Graph statistics"
        description="Distribution by type and tag, the most-linked hubs, and orphaned concepts with no incoming links."
      />

      <div>
        <Button onClick={() => executeTool({})} disabled={isLoading} variant="outline">
          {isLoading ? "Refreshing…" : "Refresh"}
        </Button>
      </div>

      {isError && <ErrorBanner title="Stats failed" />}

      {!isError && parsed && (
        <>
          <StatTileGrid>
            <StatTile label="Documents" value={parsed.documents} tone="accent" />
            <StatTile label="Deleted" value={parsed.deleted_documents} />
            <StatTile label="Types" value={parsed.by_type.length} />
            <StatTile label="Orphans" value={parsed.orphans.length} tone={parsed.orphans.length > 0 ? "danger" : "default"} />
          </StatTileGrid>

          <div className="grid gap-4 lg:grid-cols-2">
            <div>
              <p className="mb-2 text-xs font-bold tracking-wide uppercase text-muted-foreground">By type</p>
              <BarChartCard
                data={parsed.by_type.map((t) => ({ label: t.type, value: t.count }))}
                seriesLabel="Documents"
              />
            </div>
            <div>
              <p className="mb-2 text-xs font-bold tracking-wide uppercase text-muted-foreground">Top linked (hubs)</p>
              <BarChartCard
                data={parsed.top_linked.map((t) => ({ label: t.concept_id, value: t.incoming_links }))}
                seriesLabel="Incoming links"
                layout="horizontal"
              />
            </div>
          </div>

          <div className="grid gap-4 md:grid-cols-2">
            <Card>
              <CardHeader>
                <CardTitle className="text-sm">By tag</CardTitle>
              </CardHeader>
              <CardContent>
                <div className="flex flex-wrap gap-1.5">
                  {parsed.by_tag.length === 0 && (
                    <p className="text-xs text-muted-foreground">No tags yet.</p>
                  )}
                  {parsed.by_tag.map((t) => (
                    <Badge key={t.tag} variant="secondary" className="normal-case">
                      {t.tag} · {t.count}
                    </Badge>
                  ))}
                </div>
              </CardContent>
            </Card>
            <Card>
              <CardHeader>
                <CardTitle className="text-sm">Orphans</CardTitle>
              </CardHeader>
              <CardContent className="space-y-1 text-xs">
                {parsed.orphans.length === 0 && (
                  <p className="text-muted-foreground">No orphaned concepts.</p>
                )}
                {parsed.orphans.map((id) => (
                  <div key={id} className="font-mono">
                    {id}
                  </div>
                ))}
              </CardContent>
            </Card>
          </div>
        </>
      )}

      {!isError && !parsed && !isLoading && (
        <EmptyBanner>Waiting for stats…</EmptyBanner>
      )}
    </ToolLayout>
  );
}
