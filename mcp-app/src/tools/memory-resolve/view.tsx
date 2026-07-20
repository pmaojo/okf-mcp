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
import type { MemoryResolveResult } from "@/lib/mcp-types";

export function MemoryResolveView({ app, toolResult }: ToolComponentProps) {
  const [conceptId, setConceptId] = useState("");
  const [depth, setDepth] = useState("2");
  const [showMarkdown, setShowMarkdown] = useState(false);

  const { activeResult, isError, isLoading, executeTool } = useServerTool(
    app,
    "memory_resolve",
    toolResult
  );
  const parsed = parseToolPayload<MemoryResolveResult>(activeResult);

  const run = (id?: string) => {
    const target = (id ?? conceptId).trim();
    if (!target) return;
    if (id) setConceptId(id);
    const args: Record<string, unknown> = { concept_id: target };
    const depthNum = Number(depth);
    if (Number.isFinite(depthNum) && depthNum >= 0) args.depth = depthNum;
    void executeTool(args);
  };

  const graphNodes: GraphNodeSpec[] = [];
  const graphEdges: GraphEdgeSpec[] = [];
  if (parsed) {
    graphNodes.push({ id: parsed.document.concept_id, label: parsed.document.concept_id, depth: 0, root: true });
    for (const n of parsed.neighborhood) {
      graphNodes.push({ id: n.concept_id, label: n.concept_id, depth: n.depth, broken: !n.exists });
      if (n.parent) graphEdges.push({ source: n.parent, target: n.concept_id });
    }
  }

  return (
    <ToolLayout>
      <ToolHeader
        slug="memory_resolve"
        title="Resolve a concept + its graph neighborhood"
        description="Click any neighbor node to jump the graph over to it — each click re-runs memory_resolve on the server."
      />

      <RunPanel>
        <div className="grid gap-3 sm:grid-cols-[1fr_auto_auto]">
          <Field id="mr-id" label="concept_id">
            <Input
              id="mr-id"
              value={conceptId}
              onChange={(e) => setConceptId(e.target.value)}
              placeholder="people/alice"
              onKeyDown={(e) => e.key === "Enter" && run()}
            />
          </Field>
          <Field id="mr-depth" label="Depth">
            <Input
              id="mr-depth"
              type="number"
              min={0}
              className="w-24"
              value={depth}
              onChange={(e) => setDepth(e.target.value)}
            />
          </Field>
          <Button onClick={() => run()} disabled={isLoading} className="self-end">
            {isLoading ? "Resolving…" : "Run memory_resolve"}
          </Button>
        </div>
      </RunPanel>

      {isError && (
        <ErrorBanner
          title="Resolve failed"
          detail="The concept_id may not exist, or arguments are invalid."
        />
      )}

      {!isError && parsed && (
        <>
          <Card>
            <CardHeader>
              <div className="flex flex-wrap items-center gap-2">
                <CardTitle className="text-lg">
                  {parsed.document.title ?? parsed.document.concept_id}
                </CardTitle>
                <Badge variant="outline">{parsed.document.type}</Badge>
                <Badge variant="secondary">v{parsed.document.version}</Badge>
                {(parsed.truncated.by_nodes ||
                  parsed.truncated.by_depth ||
                  parsed.truncated.by_bytes) && (
                  <Badge variant="destructive">truncated by budget</Badge>
                )}
              </div>
            </CardHeader>
            <CardContent className="space-y-3">
              <div className="flex flex-wrap gap-1">
                {parsed.document.tags.map((tag) => (
                  <Badge key={tag} variant="secondary" className="normal-case">
                    {tag}
                  </Badge>
                ))}
              </div>
              <p className="font-mono text-xs text-muted-foreground">
                {parsed.document.concept_id} · {parsed.document.hash.slice(0, 12)}…
              </p>
              <Button
                variant="outline"
                size="sm"
                onClick={() => setShowMarkdown((v) => !v)}
              >
                {showMarkdown ? "Hide markdown" : "Show markdown"}
              </Button>
              {showMarkdown && (
                <pre className="max-h-64 overflow-auto border-2 border-foreground bg-muted p-3 text-xs whitespace-pre-wrap">
                  {parsed.document.markdown}
                </pre>
              )}
            </CardContent>
          </Card>

          <ConceptGraph
            nodes={graphNodes}
            edges={graphEdges}
            onNodeClick={(id) => id !== parsed.document.concept_id && run(id)}
          />
        </>
      )}

      {!isError && !parsed && !isLoading && (
        <EmptyBanner>Resolve a concept_id to see its document and graph.</EmptyBanner>
      )}
    </ToolLayout>
  );
}
