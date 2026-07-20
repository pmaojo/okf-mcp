import { useState } from "react";
import type { ToolComponentProps } from "@/core/framework/tool-contract";
import { ToolHeader } from "@/shared/components/tool/ToolHeader";
import { ToolLayout, RunPanel } from "@/shared/components/tool/ToolLayout";
import { Field } from "@/shared/components/tool/Field";
import { ErrorBanner, EmptyBanner } from "@/shared/components/tool/StatusBanner";
import {
  ConceptGraph,
  type GraphNodeSpec,
  type GraphEdgeSpec,
} from "@/shared/components/graph/ConceptGraph";
import { ConceptTable } from "@/shared/components/tool/ConceptTable";
import { Input } from "@/shared/components/ui/input";
import { Button } from "@/shared/components/ui/button";
import { Badge } from "@/shared/components/ui/badge";
import { useServerTool } from "@/shared/hooks/useServerTool";
import { parseToolPayload } from "@/lib/tool-result";
import type { MemoryBacklinksResult } from "@/lib/mcp-types";

export function MemoryBacklinksView({ app, toolResult }: ToolComponentProps) {
  const [conceptId, setConceptId] = useState("");

  const { activeResult, isError, isLoading, executeTool } = useServerTool(
    app,
    "memory_backlinks",
    toolResult
  );
  const parsed = parseToolPayload<MemoryBacklinksResult>(activeResult);

  const run = (id?: string) => {
    const target = (id ?? conceptId).trim();
    if (!target) return;
    if (id) setConceptId(id);
    void executeTool({ concept_id: target });
  };

  const rootId = conceptId.trim();
  const graphNodes: GraphNodeSpec[] = [];
  const graphEdges: GraphEdgeSpec[] = [];
  if (parsed && rootId) {
    graphNodes.push({ id: rootId, label: rootId, depth: 0, root: true });
    for (const link of parsed.backlinks) {
      graphNodes.push({ id: link.source.concept_id, label: link.source.concept_id, depth: 1 });
      graphEdges.push({
        source: link.source.concept_id,
        target: rootId,
        label: link.rel ?? undefined,
      });
    }
  }

  return (
    <ToolLayout>
      <ToolHeader
        slug="memory_backlinks"
        title="Who links here?"
        description="Every concept that references this one via [[concept_id]] — click a source to walk backwards through the graph."
      />

      <RunPanel>
        <div className="grid gap-3 sm:grid-cols-[1fr_auto]">
          <Field id="mb-id" label="concept_id">
            <Input
              id="mb-id"
              value={conceptId}
              onChange={(e) => setConceptId(e.target.value)}
              placeholder="projects/okf-mcp"
              onKeyDown={(e) => e.key === "Enter" && run()}
            />
          </Field>
          <Button onClick={() => run()} disabled={isLoading} className="self-end">
            {isLoading ? "Looking…" : "Run memory_backlinks"}
          </Button>
        </div>
      </RunPanel>

      {isError && (
        <ErrorBanner title="Backlinks failed" detail="Check the concept_id." />
      )}

      {!isError && parsed && (
        <>
          <Badge variant="secondary">{parsed.count} backlink(s)</Badge>
          <ConceptGraph
            nodes={graphNodes}
            edges={graphEdges}
            onNodeClick={(id) => id !== rootId && run(id)}
          />
          <ConceptTable
            rows={parsed.backlinks.map((b) => b.source)}
            onSelect={(id) => run(id)}
            emptyLabel="No concept links here yet."
          />
        </>
      )}

      {!isError && !parsed && !isLoading && (
        <EmptyBanner>Look up a concept_id to see its incoming links.</EmptyBanner>
      )}
    </ToolLayout>
  );
}
