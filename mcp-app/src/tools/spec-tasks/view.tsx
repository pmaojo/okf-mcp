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
import type { SpecStatusResult, SpecTasksResult } from "@/lib/mcp-types";

interface TaskRow {
  title: string;
  description: string;
  dependsOn: string;
}

function emptyRow(): TaskRow {
  return { title: "", description: "", dependsOn: "" };
}

export function SpecTasksView({ app, toolResult }: ToolComponentProps) {
  const [specId, setSpecId] = useState("");
  const [rows, setRows] = useState<TaskRow[]>([emptyRow()]);

  const { activeResult, isError, isLoading, executeTool, isManual } = useServerTool(
    app,
    "spec_tasks",
    toolResult
  );
  const parsed = parseToolPayload<SpecTasksResult>(activeResult);

  const status = usePeekTool(app);
  const statusResult = parseToolPayload<SpecStatusResult>(status.result);

  const updateRow = (i: number, patch: Partial<TaskRow>) =>
    setRows((rs) => rs.map((r, idx) => (idx === i ? { ...r, ...patch } : r)));

  const run = () => {
    if (!specId.trim()) {
      toast.error("spec_id is required.");
      return;
    }
    const tasks = rows
      .filter((r) => r.title.trim())
      .map((r) => {
        const task: Record<string, unknown> = { title: r.title.trim() };
        if (r.description.trim()) task.description = r.description.trim();
        const deps = r.dependsOn
          .split(",")
          .map((s) => s.trim())
          .filter(Boolean);
        if (deps.length > 0) task.depends_on = deps;
        return task;
      });
    if (tasks.length === 0) {
      toast.error("Add at least one task with a title.");
      return;
    }
    status.reset();
    void executeTool({ spec_id: specId.trim(), tasks });
  };

  return (
    <ToolLayout>
      <ToolHeader
        slug="spec_tasks"
        title="Decompose into tasks"
        description="depends_on accepts another task's exact title from this same batch, or the concept_id of a task already committed (even from a previous spec_tasks call)."
      />
      <ToolSplit>
        <RunPanel defaultOpen={!toolResult}>
          <Field id="st-spec-id" label="spec_id" hint="Created earlier with spec_propose.">
            <Input
              id="st-spec-id"
              value={specId}
              onChange={(e) => setSpecId(e.target.value)}
              placeholder="specs/hybrid-search-v2"
            />
          </Field>

          <div className="space-y-3">
            {rows.map((row, i) => (
              <div key={i} className="space-y-2 border-2 border-border p-3">
                <div className="flex items-center justify-between">
                  <span className="text-xs font-bold tracking-wide uppercase text-muted-foreground">
                    Task {i + 1}
                  </span>
                  {rows.length > 1 && (
                    <Button
                      type="button"
                      variant="outline"
                      size="icon-sm"
                      onClick={() => setRows((rs) => rs.filter((_, idx) => idx !== i))}
                      aria-label="remove task"
                    >
                      ×
                    </Button>
                  )}
                </div>
                <Input
                  value={row.title}
                  onChange={(e) => updateRow(i, { title: e.target.value })}
                  placeholder="Normalize cosine distance to 0-1"
                />
                <Textarea
                  rows={2}
                  value={row.description}
                  onChange={(e) => updateRow(i, { description: e.target.value })}
                  placeholder="description (optional)"
                />
                <Input
                  value={row.dependsOn}
                  onChange={(e) => updateRow(i, { dependsOn: e.target.value })}
                  placeholder="depends_on (comma-separated titles or concept_ids, optional)"
                />
              </div>
            ))}
            <Button
              type="button"
              variant="secondary"
              size="sm"
              onClick={() => setRows((rs) => [...rs, emptyRow()])}
            >
              + add task
            </Button>
          </div>

          <Button onClick={run} disabled={isLoading} className="w-full">
            {isLoading ? "Creating tasks…" : "Run spec_tasks"}
          </Button>
        </RunPanel>

        <ResultPanel>
          {isError && (
            <ErrorBanner
              title="spec_tasks failed"
              detail="Check that spec_id exists and depends_on references resolve."
            />
          )}
          {!isError && parsed && (
            <>
              <div className="border-2 border-foreground bg-card p-4 shadow-brutal-sm">
                <Badge>{parsed.created} created</Badge>
                <div className="mt-2 space-y-1">
                  {parsed.task_ids.map((id) => (
                    <div key={id} className="font-mono text-xs">
                      {id}
                    </div>
                  ))}
                </div>
                {parsed.skipped.length > 0 && (
                  <div className="mt-2 space-y-1">
                    <p className="text-xs font-bold tracking-wide uppercase text-destructive">
                      Skipped
                    </p>
                    {parsed.skipped.map((s, i) => (
                      <div key={i} className="text-xs text-destructive">
                        {s.item}: {s.reason}
                      </div>
                    ))}
                  </div>
                )}
              </div>

              <div className="flex flex-wrap gap-2">
                <Button
                  variant="outline"
                  size="sm"
                  disabled={status.isLoading}
                  onClick={() => status.peek("spec_status", { spec_id: parsed.spec_id })}
                >
                  {status.isLoading ? "Checking…" : "Check status (spec_status)"}
                </Button>
                {isManual && (
                  <AddContextButton
                    app={app}
                    label="Add tasks to agent context"
                    text={`spec_tasks added ${parsed.created} task(s) to ${parsed.spec_id}:\n${parsed.task_ids.map((id) => `- ${id}`).join("\n")}${parsed.skipped.length > 0 ? `\n\nSkipped:\n${parsed.skipped.map((s) => `- ${s.item}: ${s.reason}`).join("\n")}` : ""}`}
                  />
                )}
              </div>
              {statusResult && <SpecStatusSummary status={statusResult} />}
            </>
          )}
          {!isError && !parsed && !isLoading && (
            <EmptyBanner>Add tasks and run spec_tasks.</EmptyBanner>
          )}
        </ResultPanel>
      </ToolSplit>
    </ToolLayout>
  );
}
