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
import { ErrorBanner, EmptyBanner } from "@/shared/components/tool/StatusBanner";
import { AddContextButton } from "@/shared/components/tool/AddContextButton";
import { Input } from "@/shared/components/ui/input";
import { Textarea } from "@/shared/components/ui/textarea";
import { Button } from "@/shared/components/ui/button";
import { Switch } from "@/shared/components/ui/switch";
import { Label } from "@/shared/components/ui/label";
import { Badge } from "@/shared/components/ui/badge";
import { useServerTool } from "@/shared/hooks/useServerTool";
import { parseToolPayload } from "@/lib/tool-result";
import type { MemoryBulkCommitResult } from "@/lib/mcp-types";

interface RequestRow {
  concept_id: string;
  markdown: string;
  reason: string;
  expected_hash: string;
}

function emptyRow(): RequestRow {
  return { concept_id: "", markdown: "", reason: "", expected_hash: "" };
}

export function MemoryBulkCommitView({ app, toolResult }: ToolComponentProps) {
  const [rows, setRows] = useState<RequestRow[]>([emptyRow()]);
  const [atomic, setAtomic] = useState(true);

  const { activeResult, isError, isLoading, executeTool, isManual } = useServerTool(
    app,
    "memory_bulk_commit",
    toolResult
  );
  const parsed = parseToolPayload<MemoryBulkCommitResult>(activeResult);

  const updateRow = (i: number, patch: Partial<RequestRow>) =>
    setRows((rs) => rs.map((r, idx) => (idx === i ? { ...r, ...patch } : r)));

  const run = () => {
    const requests = rows
      .filter((r) => r.concept_id.trim() && r.markdown.trim() && r.reason.trim())
      .map((r) => {
        const req: Record<string, unknown> = {
          concept_id: r.concept_id.trim(),
          markdown: r.markdown,
          reason: r.reason.trim(),
        };
        if (r.expected_hash.trim()) req.expected_hash = r.expected_hash.trim();
        return req;
      });
    if (requests.length === 0) {
      toast.error("Add at least one complete request (concept_id, markdown, reason).");
      return;
    }
    void executeTool({ requests, atomic });
  };

  return (
    <ToolLayout>
      <ToolHeader
        slug="memory_bulk_commit"
        title="Commit a batch"
        description="Atomic mode rolls back the whole batch on any conflict; non-atomic applies what it can and reports each outcome."
      />
      <ToolSplit>
        <RunPanel>
          <div className="space-y-4">
            {rows.map((row, i) => (
              <div key={i} className="space-y-2 border-2 border-border p-3">
                <div className="flex items-center justify-between">
                  <span className="text-xs font-bold tracking-wide uppercase text-muted-foreground">
                    Request {i + 1}
                  </span>
                  {rows.length > 1 && (
                    <Button
                      type="button"
                      variant="outline"
                      size="icon-sm"
                      onClick={() => setRows((rs) => rs.filter((_, idx) => idx !== i))}
                      aria-label="remove request"
                    >
                      ×
                    </Button>
                  )}
                </div>
                <Input
                  value={row.concept_id}
                  onChange={(e) => updateRow(i, { concept_id: e.target.value })}
                  placeholder="concept_id, e.g. people/bob"
                />
                <Textarea
                  rows={5}
                  value={row.markdown}
                  onChange={(e) => updateRow(i, { markdown: e.target.value })}
                  placeholder={"---\ntype: person\ntitle: Bob\n---\nBody…"}
                />
                <Input
                  value={row.reason}
                  onChange={(e) => updateRow(i, { reason: e.target.value })}
                  placeholder="reason"
                />
                <Input
                  value={row.expected_hash}
                  onChange={(e) => updateRow(i, { expected_hash: e.target.value })}
                  placeholder="expected_hash (optional, updates only)"
                  className="font-mono"
                />
              </div>
            ))}
            <Button
              type="button"
              variant="secondary"
              size="sm"
              onClick={() => setRows((rs) => [...rs, emptyRow()])}
            >
              + add request
            </Button>
          </div>

          <div className="flex items-center gap-3">
            <Switch id="mbc-atomic" checked={atomic} onCheckedChange={setAtomic} />
            <Label htmlFor="mbc-atomic" className="text-xs font-bold tracking-wide uppercase">
              Atomic (all-or-nothing rollback)
            </Label>
          </div>
          <Button onClick={run} disabled={isLoading} className="w-full">
            {isLoading ? "Committing batch…" : "Run memory_bulk_commit"}
          </Button>
        </RunPanel>

        <ResultPanel>
          {isError && (
            <ErrorBanner title="Bulk commit failed" detail="Check each request's arguments." />
          )}
          {!isError && parsed && (
            <>
              <Badge variant={parsed.applied ? "default" : "destructive"}>
                {parsed.applied ? "applied" : "rolled back"}
              </Badge>
              <div className="space-y-2">
                {parsed.items.map((item, i) => (
                  <div
                    key={i}
                    className="border-2 border-foreground bg-card p-3 text-sm shadow-brutal-sm"
                  >
                    <div className="mb-1 flex items-center gap-2">
                      <Badge
                        variant={
                          item.status === "done"
                            ? "default"
                            : item.status === "failed"
                              ? "destructive"
                              : "outline"
                        }
                      >
                        {item.status}
                      </Badge>
                      <span className="text-xs text-muted-foreground">#{i + 1}</span>
                    </div>
                    {item.status === "done" && (
                      <div className="font-mono text-xs">
                        v{item.version} · {item.hash.slice(0, 12)}…{" "}
                        {item.created ? "(created)" : "(updated)"}
                        {item.no_change && " · no change"}
                      </div>
                    )}
                    {item.status === "failed" && (
                      <div className="font-mono text-xs text-destructive">
                        {item.error.kind === "revision_conflict"
                          ? `conflict: expected ${item.error.expected_hash?.slice(0, 10) ?? "—"} got ${item.error.current_hash?.slice(0, 10) ?? "—"}`
                          : item.error.detail}
                      </div>
                    )}
                  </div>
                ))}
              </div>
              {isManual && (
                <AddContextButton
                  app={app}
                  text={`memory_bulk_commit ${parsed.applied ? "applied" : "rolled back"} a batch of ${parsed.items.length} request(s): ${parsed.items.filter((i) => i.status === "done").length} done, ${parsed.items.filter((i) => i.status === "failed").length} failed, ${parsed.items.filter((i) => i.status === "skipped").length} skipped.`}
                />
              )}
            </>
          )}
          {!isError && !parsed && !isLoading && (
            <EmptyBanner>Add requests and run the batch commit.</EmptyBanner>
          )}
        </ResultPanel>
      </ToolSplit>
    </ToolLayout>
  );
}
