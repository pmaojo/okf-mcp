import { useState } from "react";
import type { ToolComponentProps } from "@/core/framework/tool-contract";
import { ToolHeader } from "@/shared/components/tool/ToolHeader";
import {
  ToolLayout,
  ToolSplit,
  RunPanel,
  ResultPanel,
} from "@/shared/components/tool/ToolLayout";
import { Field } from "@/shared/components/tool/Field";
import { ErrorBanner, EmptyBanner } from "@/shared/components/tool/StatusBanner";
import { StatTile, StatTileGrid } from "@/shared/components/tool/StatTile";
import { Input } from "@/shared/components/ui/input";
import { Button } from "@/shared/components/ui/button";
import { Badge } from "@/shared/components/ui/badge";
import { useServerTool } from "@/shared/hooks/useServerTool";
import { parseToolPayload } from "@/lib/tool-result";
import type { MemoryEmbedResult } from "@/lib/mcp-types";

export function MemoryEmbedView({ app, toolResult }: ToolComponentProps) {
  const [pathPrefix, setPathPrefix] = useState("");
  const [max, setMax] = useState("10");

  const { activeResult, isError, isLoading, executeTool } = useServerTool(
    app,
    "memory_embed",
    toolResult
  );
  const parsed = parseToolPayload<MemoryEmbedResult>(activeResult);

  const run = () => {
    const args: Record<string, unknown> = {};
    if (pathPrefix.trim()) args.path_prefix = pathPrefix.trim();
    const maxNum = Number(max);
    if (Number.isFinite(maxNum) && maxNum > 0) args.max = maxNum;
    void executeTool(args);
  };

  return (
    <ToolLayout>
      <ToolHeader
        slug="memory_embed"
        title="Backfill embeddings"
        description="Runs one batch at a time — call again to keep draining the queue until 'remaining' hits zero."
      />
      <ToolSplit>
        <RunPanel>
          <Field id="me-prefix" label="Path prefix" hint="Optional, filters by logical path.">
            <Input
              id="me-prefix"
              value={pathPrefix}
              onChange={(e) => setPathPrefix(e.target.value)}
              placeholder="people"
            />
          </Field>
          <Field id="me-max" label="Max batch size">
            <Input
              id="me-max"
              type="number"
              min={1}
              value={max}
              onChange={(e) => setMax(e.target.value)}
            />
          </Field>
          <Button onClick={run} disabled={isLoading} className="w-full">
            {isLoading ? "Embedding…" : "Run memory_embed"}
          </Button>
        </RunPanel>

        <ResultPanel>
          {isError && (
            <ErrorBanner title="Embed batch failed" detail="Check server-side embedding provider configuration." />
          )}
          {!isError && parsed && (
            <>
              <StatTileGrid>
                <StatTile label="Embedded" value={parsed.embedded.length} tone="accent" />
                <StatTile label="Failed" value={parsed.failed.length} tone={parsed.failed.length > 0 ? "danger" : "default"} />
                <StatTile label="Remaining" value={parsed.remaining} />
              </StatTileGrid>

              {parsed.embedded.length > 0 && (
                <div>
                  <p className="mb-1 text-xs font-bold tracking-wide uppercase text-muted-foreground">Embedded</p>
                  <div className="flex flex-wrap gap-1">
                    {parsed.embedded.map((id) => (
                      <Badge key={id} variant="secondary" className="font-mono normal-case">{id}</Badge>
                    ))}
                  </div>
                </div>
              )}

              {parsed.failed.length > 0 && (
                <div className="space-y-1">
                  <p className="text-xs font-bold tracking-wide uppercase text-muted-foreground">Failed</p>
                  {parsed.failed.map((f) => (
                    <div key={f.concept_id} className="border-2 border-destructive bg-card p-2 text-xs">
                      <span className="font-mono font-bold">{f.concept_id}</span>: {f.error}
                    </div>
                  ))}
                </div>
              )}
            </>
          )}
          {!isError && !parsed && !isLoading && (
            <EmptyBanner>Run a batch to backfill embeddings.</EmptyBanner>
          )}
        </ResultPanel>
      </ToolSplit>
    </ToolLayout>
  );
}
