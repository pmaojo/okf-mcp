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
import { SpecStatusSummary } from "@/shared/components/tool/SpecStatusSummary";
import { PeekConceptCard } from "@/shared/components/tool/PeekConceptCard";
import { AddContextButton } from "@/shared/components/tool/AddContextButton";
import { Input } from "@/shared/components/ui/input";
import { Button } from "@/shared/components/ui/button";
import { useServerTool } from "@/shared/hooks/useServerTool";
import { usePeekTool } from "@/shared/hooks/usePeekTool";
import { parseToolPayload } from "@/lib/tool-result";
import type { SpecStatusResult } from "@/lib/mcp-types";

export function SpecStatusView({ app, toolResult }: ToolComponentProps) {
  const [specId, setSpecId] = useState("");

  const { activeResult, isError, isLoading, executeTool, isManual } = useServerTool(
    app,
    "spec_status",
    toolResult
  );
  const parsed = parseToolPayload<SpecStatusResult>(activeResult);

  const resolvePeek = usePeekTool(app);

  const run = () => {
    if (!specId.trim()) return;
    resolvePeek.reset();
    void executeTool({ spec_id: specId.trim() });
  };

  return (
    <ToolLayout>
      <ToolHeader
        slug="spec_status"
        title="Where does this spec stand?"
        description="next_pending are tasks whose depends_on are all done (or none) — ready to start now. waiting_on_dependencies counts pending tasks that are not."
      />

      <RunPanel>
        <div className="grid gap-3 sm:grid-cols-[1fr_auto]">
          <Field id="ss-id" label="spec_id">
            <Input
              id="ss-id"
              value={specId}
              onChange={(e) => setSpecId(e.target.value)}
              placeholder="specs/hybrid-search-v2"
              onKeyDown={(e) => e.key === "Enter" && run()}
            />
          </Field>
          <Button onClick={run} disabled={isLoading} className="self-end">
            {isLoading ? "Checking…" : "Run spec_status"}
          </Button>
        </div>
      </RunPanel>

      {isError && <ErrorBanner title="spec_status failed" detail="Check the spec_id." />}

      {!isError && parsed && (
        <ToolSplit>
          <ResultPanel>
            <SpecStatusSummary
              status={parsed}
              onResolveTask={(taskId) => resolvePeek.peek("memory_resolve", { concept_id: taskId })}
            />
          </ResultPanel>
          <ResultPanel>
            {resolvePeek.result || resolvePeek.isLoading || resolvePeek.isError ? (
              <PeekConceptCard
                result={resolvePeek.result}
                isLoading={resolvePeek.isLoading}
                isError={resolvePeek.isError}
              />
            ) : (
              <EmptyBanner>Click a task on the left to resolve it here.</EmptyBanner>
            )}
            {isManual && (
              <AddContextButton
                app={app}
                label="Add status to agent context"
                text={`spec_status for ${parsed.spec_id}: ${parsed.spec_status}, ${Math.round(parsed.progress * 100)}% done (${parsed.by_status.done}/${parsed.tasks_total} tasks). Next up: ${parsed.next_pending.length > 0 ? parsed.next_pending.join(", ") : "nothing — all remaining tasks are waiting on dependencies or done"}.`}
              />
            )}
          </ResultPanel>
        </ToolSplit>
      )}

      {!isError && !parsed && !isLoading && (
        <EmptyBanner>Look up a spec_id to see its progress.</EmptyBanner>
      )}
    </ToolLayout>
  );
}
