import { useState } from "react";
import { toast } from "sonner";
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
import { AddContextButton } from "@/shared/components/tool/AddContextButton";
import { Input } from "@/shared/components/ui/input";
import { Textarea } from "@/shared/components/ui/textarea";
import { Button } from "@/shared/components/ui/button";
import { Badge } from "@/shared/components/ui/badge";
import { useServerTool } from "@/shared/hooks/useServerTool";
import { usePeekTool } from "@/shared/hooks/usePeekTool";
import { parseToolPayload } from "@/lib/tool-result";
import type { SpecProposeResult, SpecStatusResult } from "@/lib/mcp-types";

export function SpecProposeView({ app, toolResult }: ToolComponentProps) {
  const [conceptId, setConceptId] = useState("");
  const [title, setTitle] = useState("");
  const [requirements, setRequirements] = useState("");
  const [design, setDesign] = useState("");
  const [firstTask, setFirstTask] = useState("");

  const { activeResult, isError, isLoading, executeTool, isManual } = useServerTool(
    app,
    "spec_propose",
    toolResult
  );
  const parsed = parseToolPayload<SpecProposeResult>(activeResult);

  const status = usePeekTool(app);
  const addTask = usePeekTool(app);
  const statusResult = parseToolPayload<SpecStatusResult>(status.result);

  const run = () => {
    if (!conceptId.trim() || !title.trim() || !requirements.trim() || !design.trim()) {
      toast.error("concept_id, title, requirements and design are required.");
      return;
    }
    status.reset();
    addTask.reset();
    void executeTool({
      concept_id: conceptId.trim(),
      title: title.trim(),
      requirements: requirements.trim(),
      design: design.trim(),
    });
  };

  const runAddFirstTask = () => {
    if (!parsed || !firstTask.trim()) return;
    void addTask.peek("spec_tasks", {
      spec_id: parsed.concept_id,
      tasks: [{ title: firstTask.trim() }],
    });
  };

  return (
    <ToolLayout>
      <ToolHeader
        slug="spec_propose"
        title="Propose a spec"
        description="Requirements + design, agreed before implementation. Any MCP client can pick this up later — the state lives in shared memory, not a conversation."
      />
      <ToolSplit>
        <RunPanel defaultOpen={!toolResult}>
          <Field id="sp-id" label="concept_id" hint="e.g. specs/hybrid-search-v2">
            <Input
              id="sp-id"
              value={conceptId}
              onChange={(e) => setConceptId(e.target.value)}
              placeholder="specs/hybrid-search-v2"
            />
          </Field>
          <Field id="sp-title" label="Title">
            <Input
              id="sp-title"
              value={title}
              onChange={(e) => setTitle(e.target.value)}
              placeholder="Hybrid search in one SQL query"
            />
          </Field>
          <Field id="sp-req" label="Requirements">
            <Textarea
              id="sp-req"
              rows={4}
              value={requirements}
              onChange={(e) => setRequirements(e.target.value)}
              placeholder="Combine textual + semantic ranking in one ORDER BY…"
            />
          </Field>
          <Field id="sp-design" label="Design">
            <Textarea
              id="sp-design"
              rows={4}
              value={design}
              onChange={(e) => setDesign(e.target.value)}
              placeholder="Normalize both distances to [0,1] and sum with a configurable weight…"
            />
          </Field>
          <Button onClick={run} disabled={isLoading} className="w-full">
            {isLoading ? "Proposing…" : "Run spec_propose"}
          </Button>
        </RunPanel>

        <ResultPanel>
          {isError && <ErrorBanner title="Propose failed" />}
          {!isError && parsed && (
            <>
              <div className="border-2 border-foreground bg-card p-4 shadow-brutal-sm">
                <div className="mb-2 flex flex-wrap gap-2">
                  {parsed.created ? <Badge>created</Badge> : <Badge variant="secondary">updated</Badge>}
                </div>
                <p className="font-mono text-sm">{parsed.concept_id}</p>
                <p className="mt-1 font-mono text-xs text-muted-foreground">
                  v{parsed.version} · {parsed.hash.slice(0, 12)}…
                </p>
              </div>

              <div className="flex flex-wrap gap-2">
                <Button
                  variant="outline"
                  size="sm"
                  disabled={status.isLoading}
                  onClick={() => status.peek("spec_status", { spec_id: parsed.concept_id })}
                >
                  {status.isLoading ? "Checking…" : "Check status (spec_status)"}
                </Button>
                {isManual && (
                  <AddContextButton
                    app={app}
                    label="Add spec to agent context"
                    text={`Spec ${parsed.concept_id} (${parsed.created ? "just created" : "updated"}, v${parsed.version}) is ready for review:\n\n- Title: ${title.trim()}\n- Requirements: ${requirements.trim()}\n- Design: ${design.trim()}\n\nIf this looks right, break it into tasks with spec_tasks.`}
                  />
                )}
              </div>
              {status.isError && <p className="text-xs text-destructive">Status check failed.</p>}
              {statusResult && <SpecStatusSummary status={statusResult} />}

              <div className="space-y-2 border-2 border-dashed border-border p-3">
                <p className="text-xs font-bold tracking-wide uppercase text-muted-foreground">
                  Add a first task (spec_tasks)
                </p>
                <div className="flex gap-2">
                  <Input
                    value={firstTask}
                    onChange={(e) => setFirstTask(e.target.value)}
                    placeholder="Normalize cosine distance to 0-1"
                  />
                  <Button size="sm" disabled={addTask.isLoading} onClick={runAddFirstTask}>
                    {addTask.isLoading ? "Adding…" : "Add"}
                  </Button>
                </div>
                {addTask.isError && (
                  <p className="text-xs text-destructive">Could not add the task.</p>
                )}
                {addTask.result && !addTask.isError && (
                  <p className="text-xs text-muted-foreground">
                    Task added — open spec_tasks or spec_status to see the full list.
                  </p>
                )}
              </div>
            </>
          )}
          {!isError && !parsed && !isLoading && (
            <EmptyBanner>Fill in the form and propose a spec.</EmptyBanner>
          )}
        </ResultPanel>
      </ToolSplit>
    </ToolLayout>
  );
}
