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
import { ConceptTable } from "@/shared/components/tool/ConceptTable";
import { Input } from "@/shared/components/ui/input";
import { Button } from "@/shared/components/ui/button";
import { Badge } from "@/shared/components/ui/badge";
import { useServerTool } from "@/shared/hooks/useServerTool";
import { parseToolPayload } from "@/lib/tool-result";
import type { MemoryListResult } from "@/lib/mcp-types";

export function MemoryListView({ app, toolResult }: ToolComponentProps) {
  const [pathPrefix, setPathPrefix] = useState("");
  const [limit, setLimit] = useState("50");

  const { activeResult, isError, isLoading, executeTool } = useServerTool(
    app,
    "memory_list",
    toolResult
  );
  const parsed = parseToolPayload<MemoryListResult>(activeResult);

  const run = () => {
    const args: Record<string, unknown> = {};
    if (pathPrefix.trim()) args.path_prefix = pathPrefix.trim();
    const limitNum = Number(limit);
    if (Number.isFinite(limitNum) && limitNum > 0) args.limit = limitNum;
    void executeTool(args);
  };

  return (
    <ToolLayout>
      <ToolHeader
        slug="memory_list"
        title="Browse by prefix"
        description="No query needed — just walk a path prefix (e.g. 'people') to see everything filed under it."
      />
      <ToolSplit>
        <RunPanel>
          <Field
            id="ml-prefix"
            label="Path prefix"
            hint="Leave empty to list everything (bounded by limit)."
          >
            <Input
              id="ml-prefix"
              value={pathPrefix}
              onChange={(e) => setPathPrefix(e.target.value)}
              placeholder="people"
              onKeyDown={(e) => e.key === "Enter" && run()}
            />
          </Field>
          <Field id="ml-limit" label="Limit">
            <Input
              id="ml-limit"
              type="number"
              min={1}
              value={limit}
              onChange={(e) => setLimit(e.target.value)}
            />
          </Field>
          <Button onClick={run} disabled={isLoading} className="w-full">
            {isLoading ? "Listing…" : "Run memory_list"}
          </Button>
        </RunPanel>

        <ResultPanel>
          {isError && (
            <ErrorBanner title="List failed" detail="Check the path prefix." />
          )}
          {!isError && parsed && (
            <>
              <div>
                <Badge variant="secondary">{parsed.count} concept(s)</Badge>
              </div>
              <ConceptTable rows={parsed.results} />
            </>
          )}
          {!isError && !parsed && !isLoading && (
            <EmptyBanner>Run a listing to see concepts here.</EmptyBanner>
          )}
        </ResultPanel>
      </ToolSplit>
    </ToolLayout>
  );
}
