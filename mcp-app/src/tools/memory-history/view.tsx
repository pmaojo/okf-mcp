import { useMemo, useState } from "react";
import type { ToolComponentProps } from "@/core/framework/tool-contract";
import { ToolHeader } from "@/shared/components/tool/ToolHeader";
import { ToolLayout, RunPanel } from "@/shared/components/tool/ToolLayout";
import { Field } from "@/shared/components/tool/Field";
import { ErrorBanner, EmptyBanner } from "@/shared/components/tool/StatusBanner";
import { BarChartCard } from "@/shared/components/charts/BarChartCard";
import { Input } from "@/shared/components/ui/input";
import { Button } from "@/shared/components/ui/button";
import { Badge } from "@/shared/components/ui/badge";
import { useServerTool } from "@/shared/hooks/useServerTool";
import { parseToolPayload } from "@/lib/tool-result";
import type { MemoryHistoryResult } from "@/lib/mcp-types";

export function MemoryHistoryView({ app, toolResult }: ToolComponentProps) {
  const [conceptId, setConceptId] = useState("");
  const [limit, setLimit] = useState("10");

  const { activeResult, isError, isLoading, executeTool } = useServerTool(
    app,
    "memory_history",
    toolResult
  );
  const parsed = parseToolPayload<MemoryHistoryResult>(activeResult);

  const run = () => {
    if (!conceptId.trim()) return;
    const args: Record<string, unknown> = { concept_id: conceptId.trim() };
    const limitNum = Number(limit);
    if (Number.isFinite(limitNum) && limitNum > 0) args.limit = limitNum;
    void executeTool(args);
  };

  const byActor = useMemo(() => {
    if (!parsed) return [];
    const counts = new Map<string, number>();
    for (const rev of parsed.revisions) {
      counts.set(rev.actor, (counts.get(rev.actor) ?? 0) + 1);
    }
    return [...counts.entries()].map(([label, value]) => ({ label, value }));
  }, [parsed]);

  return (
    <ToolLayout>
      <ToolHeader
        slug="memory_history"
        title="Revision timeline"
        description="Newest revisions first — pass before_seq (via a future call) to page further back."
      />

      <RunPanel defaultOpen={!toolResult}>
        <div className="grid gap-3 sm:grid-cols-[1fr_auto_auto]">
          <Field id="mh-id" label="concept_id">
            <Input
              id="mh-id"
              value={conceptId}
              onChange={(e) => setConceptId(e.target.value)}
              placeholder="people/alice"
              onKeyDown={(e) => e.key === "Enter" && run()}
            />
          </Field>
          <Field id="mh-limit" label="Limit">
            <Input
              id="mh-limit"
              type="number"
              min={1}
              max={100}
              className="w-24"
              value={limit}
              onChange={(e) => setLimit(e.target.value)}
            />
          </Field>
          <Button onClick={run} disabled={isLoading} className="self-end">
            {isLoading ? "Loading…" : "Run memory_history"}
          </Button>
        </div>
      </RunPanel>

      {isError && <ErrorBanner title="History failed" detail="Check the concept_id." />}

      {!isError && parsed && (
        <>
          <Badge variant="secondary">{parsed.revisions.length} revision(s)</Badge>

          {byActor.length > 1 && (
            <BarChartCard data={byActor} seriesLabel="Revisions" layout="horizontal" height={Math.max(140, byActor.length * 40)} />
          )}

          <div className="border-l-4 border-foreground pl-4">
            {parsed.revisions.length === 0 && (
              <p className="text-sm text-muted-foreground">No revisions yet for {parsed.concept_id}.</p>
            )}
            {parsed.revisions.map((rev) => (
              <div key={rev.seq} className="relative mb-4 last:mb-0">
                <span className="absolute -left-[22px] top-1 size-3 border-2 border-foreground bg-accent" />
                <div className="font-bold">{rev.reason}</div>
                <div className="text-xs text-muted-foreground">
                  seq {rev.seq} · {rev.actor} ({rev.client_id})
                </div>
                <div className="mt-1 inline-block border border-border bg-muted px-1.5 py-0.5 font-mono text-xs">
                  {rev.result_hash.slice(0, 8)}
                  {rev.base_hash ? ` ← ${rev.base_hash.slice(0, 8)}` : " (created)"}
                </div>
              </div>
            ))}
          </div>
        </>
      )}

      {!isError && !parsed && !isLoading && (
        <EmptyBanner>Look up a concept_id to see its revision history.</EmptyBanner>
      )}
    </ToolLayout>
  );
}
