import { useState } from "react";
import type { ToolComponentProps } from "@/core/framework/tool-contract";
import { ToolHeader } from "@/shared/components/tool/ToolHeader";
import { ToolLayout, RunPanel } from "@/shared/components/tool/ToolLayout";
import { Field } from "@/shared/components/tool/Field";
import { ErrorBanner, EmptyBanner } from "@/shared/components/tool/StatusBanner";
import { AddContextButton } from "@/shared/components/tool/AddContextButton";
import { BarChartCard } from "@/shared/components/charts/BarChartCard";
import { Input } from "@/shared/components/ui/input";
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
import type { MemoryValidateResult } from "@/lib/mcp-types";

export function MemoryValidateView({ app, toolResult }: ToolComponentProps) {
  const [pathPrefix, setPathPrefix] = useState("");

  const { activeResult, isError, isLoading, executeTool, isManual } = useServerTool(
    app,
    "memory_validate",
    toolResult
  );
  const parsed = parseToolPayload<MemoryValidateResult>(activeResult);

  const run = () => {
    const args: Record<string, unknown> = {};
    if (pathPrefix.trim()) args.path_prefix = pathPrefix.trim();
    void executeTool(args);
  };

  const chartData = parsed
    ? [
        { label: "Broken links", value: parsed.broken_links_total },
        { label: "Refs to deleted", value: parsed.deleted_referenced_total },
        { label: "Missing embeddings", value: parsed.missing_embeddings_total },
      ]
    : [];

  const isHealthy =
    parsed &&
    parsed.broken_links_total === 0 &&
    parsed.deleted_referenced_total === 0 &&
    parsed.missing_embeddings_total === 0;

  return (
    <ToolLayout>
      <ToolHeader
        slug="memory_validate"
        title="Health report"
        description="Scan the graph for broken [[links]], references to deleted concepts, and documents missing embeddings."
      />

      <RunPanel defaultOpen={!toolResult}>
        <div className="grid gap-3 sm:grid-cols-[1fr_auto]">
          <Field id="mv-prefix" label="Path prefix" hint="Optional, scope the scan.">
            <Input
              id="mv-prefix"
              value={pathPrefix}
              onChange={(e) => setPathPrefix(e.target.value)}
              placeholder="people"
              onKeyDown={(e) => e.key === "Enter" && run()}
            />
          </Field>
          <Button onClick={run} disabled={isLoading} className="self-end">
            {isLoading ? "Scanning…" : "Run memory_validate"}
          </Button>
        </div>
      </RunPanel>

      {isError && <ErrorBanner title="Validate failed" />}

      {!isError && parsed && (
        <>
          {isHealthy && <Badge>all clear</Badge>}
          <BarChartCard data={chartData} seriesLabel="Issues" layout="horizontal" height={200} />
          {isManual && !isHealthy && (
            <AddContextButton
              app={app}
              text={`memory_validate found ${parsed.broken_links_total} broken link(s), ${parsed.deleted_referenced_total} reference(s) to deleted concepts, and ${parsed.missing_embeddings_total} document(s) missing embeddings${pathPrefix.trim() ? ` under "${pathPrefix.trim()}"` : ""}.`}
            />
          )}

          <div className="grid gap-4 md:grid-cols-3">
            <Card>
              <CardHeader>
                <CardTitle className="text-sm">Broken links</CardTitle>
              </CardHeader>
              <CardContent className="space-y-1 text-xs">
                {parsed.broken_links.length === 0 && (
                  <p className="text-muted-foreground">None.</p>
                )}
                {parsed.broken_links.map((l, i) => (
                  <div key={i} className="font-mono">
                    {l.source} → {l.target}
                  </div>
                ))}
              </CardContent>
            </Card>
            <Card>
              <CardHeader>
                <CardTitle className="text-sm">References to deleted</CardTitle>
              </CardHeader>
              <CardContent className="space-y-1 text-xs">
                {parsed.deleted_referenced.length === 0 && (
                  <p className="text-muted-foreground">None.</p>
                )}
                {parsed.deleted_referenced.map((l, i) => (
                  <div key={i} className="font-mono">
                    {l.source} → {l.target}
                  </div>
                ))}
              </CardContent>
            </Card>
            <Card>
              <CardHeader>
                <CardTitle className="text-sm">Missing embeddings</CardTitle>
              </CardHeader>
              <CardContent className="space-y-1 text-xs">
                {parsed.missing_embeddings.length === 0 && (
                  <p className="text-muted-foreground">None.</p>
                )}
                {parsed.missing_embeddings.map((id) => (
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
        <EmptyBanner>Run a scan to see the health report.</EmptyBanner>
      )}
    </ToolLayout>
  );
}
