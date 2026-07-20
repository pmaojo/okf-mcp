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
import type { MemorySearchResult } from "@/lib/mcp-types";

export function MemorySearchView({ app, toolResult }: ToolComponentProps) {
  const [query, setQuery] = useState("");
  const [type, setType] = useState("");
  const [tag, setTag] = useState("");
  const [pathPrefix, setPathPrefix] = useState("");
  const [limit, setLimit] = useState("20");

  const { activeResult, isError, isLoading, executeTool } = useServerTool(
    app,
    "memory_search",
    toolResult
  );
  const parsed = parseToolPayload<MemorySearchResult>(activeResult);

  const run = () => {
    const args: Record<string, unknown> = {};
    if (query.trim()) args.query = query.trim();
    if (type.trim()) args.type = type.trim();
    if (tag.trim()) args.tag = tag.trim();
    if (pathPrefix.trim()) args.path_prefix = pathPrefix.trim();
    const limitNum = Number(limit);
    if (Number.isFinite(limitNum) && limitNum > 0) args.limit = limitNum;
    void executeTool(args);
  };

  return (
    <ToolLayout>
      <ToolHeader
        slug="memory_search"
        title="Search the memory graph"
        description="Compact candidates ranked by textual (and, when configured, semantic) match — resolve a hit with memory_resolve to get the full document."
      />
      <ToolSplit>
        <RunPanel>
          <Field id="ms-query" label="Query">
            <Input
              id="ms-query"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="substring in id, title, tags, body"
              onKeyDown={(e) => e.key === "Enter" && run()}
            />
          </Field>
          <div className="grid grid-cols-2 gap-3">
            <Field id="ms-type" label="Type">
              <Input
                id="ms-type"
                value={type}
                onChange={(e) => setType(e.target.value)}
                placeholder="person"
              />
            </Field>
            <Field id="ms-tag" label="Tag">
              <Input
                id="ms-tag"
                value={tag}
                onChange={(e) => setTag(e.target.value)}
                placeholder="dev"
              />
            </Field>
          </div>
          <div className="grid grid-cols-2 gap-3">
            <Field id="ms-prefix" label="Path prefix">
              <Input
                id="ms-prefix"
                value={pathPrefix}
                onChange={(e) => setPathPrefix(e.target.value)}
                placeholder="people"
              />
            </Field>
            <Field id="ms-limit" label="Limit">
              <Input
                id="ms-limit"
                type="number"
                min={1}
                value={limit}
                onChange={(e) => setLimit(e.target.value)}
              />
            </Field>
          </div>
          <Button onClick={run} disabled={isLoading} className="w-full">
            {isLoading ? "Searching…" : "Run memory_search"}
          </Button>
        </RunPanel>

        <ResultPanel>
          {isError && (
            <ErrorBanner
              title="Search failed"
              detail="Check the arguments or the host logs for details."
            />
          )}
          {!isError && parsed && (
            <>
              <div>
                <Badge variant="secondary">{parsed.count} result(s)</Badge>
              </div>
              <ConceptTable rows={parsed.results} />
            </>
          )}
          {!isError && !parsed && !isLoading && (
            <EmptyBanner>Run a search to see candidates here.</EmptyBanner>
          )}
        </ResultPanel>
      </ToolSplit>
    </ToolLayout>
  );
}
