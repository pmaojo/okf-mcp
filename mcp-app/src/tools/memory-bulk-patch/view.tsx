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
import { Button } from "@/shared/components/ui/button";
import { Switch } from "@/shared/components/ui/switch";
import { Label } from "@/shared/components/ui/label";
import { Badge } from "@/shared/components/ui/badge";
import { useServerTool } from "@/shared/hooks/useServerTool";
import { parseToolPayload } from "@/lib/tool-result";
import type { MemoryBulkPatchResult } from "@/lib/mcp-types";

interface PatchRow {
  concept_id: string;
  expected_hash: string;
  reason: string;
  set: string;
  remove: string;
  add_tags: string;
  remove_tags: string;
}

function emptyRow(): PatchRow {
  return { concept_id: "", expected_hash: "", reason: "", set: "", remove: "", add_tags: "", remove_tags: "" };
}

function splitCsv(value: string): string[] {
  return value
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean);
}

/** "key=value, other=thing" -> { key: "value", other: "thing" }. Ignores malformed pairs. */
function parseSet(value: string): Record<string, string> {
  const out: Record<string, string> = {};
  for (const pair of splitCsv(value)) {
    const idx = pair.indexOf("=");
    if (idx <= 0) continue;
    out[pair.slice(0, idx).trim()] = pair.slice(idx + 1).trim();
  }
  return out;
}

export function MemoryBulkPatchView({ app, toolResult }: ToolComponentProps) {
  const [rows, setRows] = useState<PatchRow[]>([emptyRow()]);
  const [atomic, setAtomic] = useState(true);

  const { activeResult, isError, isLoading, executeTool, isManual } = useServerTool(
    app,
    "memory_bulk_patch",
    toolResult
  );
  const parsed = parseToolPayload<MemoryBulkPatchResult>(activeResult);

  const updateRow = (i: number, patch: Partial<PatchRow>) =>
    setRows((rs) => rs.map((r, idx) => (idx === i ? { ...r, ...patch } : r)));

  const run = () => {
    const patches = rows
      .filter((r) => r.concept_id.trim() && r.expected_hash.trim() && r.reason.trim())
      .map((r) => {
        const req: Record<string, unknown> = {
          concept_id: r.concept_id.trim(),
          expected_hash: r.expected_hash.trim(),
          reason: r.reason.trim(),
        };
        const set = parseSet(r.set);
        if (Object.keys(set).length > 0) req.set = set;
        const remove = splitCsv(r.remove);
        if (remove.length > 0) req.remove = remove;
        const addTags = splitCsv(r.add_tags);
        if (addTags.length > 0) req.add_tags = addTags;
        const removeTags = splitCsv(r.remove_tags);
        if (removeTags.length > 0) req.remove_tags = removeTags;
        return req;
      });
    if (patches.length === 0) {
      toast.error("Add at least one complete patch (concept_id, expected_hash, reason).");
      return;
    }
    void executeTool({ patches, atomic });
  };

  return (
    <ToolLayout>
      <ToolHeader
        slug="memory_bulk_patch"
        title="Patch a batch"
        description="Atomic mode persists nothing if any item fails (pre-validation or CAS); non-atomic patches what it can and reports each outcome."
      />
      <ToolSplit>
        <RunPanel defaultOpen={!toolResult}>
          <div className="space-y-4">
            {rows.map((row, i) => (
              <div key={i} className="space-y-2 border-2 border-border p-3">
                <div className="flex items-center justify-between">
                  <span className="text-xs font-bold tracking-wide uppercase text-muted-foreground">
                    Patch {i + 1}
                  </span>
                  {rows.length > 1 && (
                    <Button
                      type="button"
                      variant="outline"
                      size="icon-sm"
                      onClick={() => setRows((rs) => rs.filter((_, idx) => idx !== i))}
                      aria-label="remove patch"
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
                <Input
                  value={row.expected_hash}
                  onChange={(e) => updateRow(i, { expected_hash: e.target.value })}
                  placeholder="expected_hash"
                  className="font-mono"
                />
                <Input
                  value={row.reason}
                  onChange={(e) => updateRow(i, { reason: e.target.value })}
                  placeholder="reason"
                />
                <Input
                  value={row.set}
                  onChange={(e) => updateRow(i, { set: e.target.value })}
                  placeholder="set: key=value, other=thing"
                />
                <div className="grid grid-cols-3 gap-2">
                  <Input
                    value={row.remove}
                    onChange={(e) => updateRow(i, { remove: e.target.value })}
                    placeholder="remove keys (csv)"
                  />
                  <Input
                    value={row.add_tags}
                    onChange={(e) => updateRow(i, { add_tags: e.target.value })}
                    placeholder="add_tags (csv)"
                  />
                  <Input
                    value={row.remove_tags}
                    onChange={(e) => updateRow(i, { remove_tags: e.target.value })}
                    placeholder="remove_tags (csv)"
                  />
                </div>
              </div>
            ))}
            <Button
              type="button"
              variant="secondary"
              size="sm"
              onClick={() => setRows((rs) => [...rs, emptyRow()])}
            >
              + add patch
            </Button>
          </div>

          <div className="flex items-center gap-3">
            <Switch id="mbp-atomic" checked={atomic} onCheckedChange={setAtomic} />
            <Label htmlFor="mbp-atomic" className="text-xs font-bold tracking-wide uppercase">
              Atomic (all-or-nothing)
            </Label>
          </div>
          <Button onClick={run} disabled={isLoading} className="w-full">
            {isLoading ? "Patching batch…" : "Run memory_bulk_patch"}
          </Button>
        </RunPanel>

        <ResultPanel>
          {isError && (
            <ErrorBanner title="Bulk patch failed" detail="Check each patch's arguments." />
          )}
          {!isError && parsed && (
            <>
              <Badge variant={parsed.applied ? "default" : "destructive"}>
                {parsed.applied ? "applied" : "not applied"}
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
                        v{item.version} · {item.hash.slice(0, 12)}…
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
                  text={`memory_bulk_patch ${parsed.applied ? "applied" : "did not apply"} a batch of ${parsed.items.length} patch(es): ${parsed.items.filter((i) => i.status === "done").length} done, ${parsed.items.filter((i) => i.status === "failed").length} failed, ${parsed.items.filter((i) => i.status === "skipped").length} skipped.`}
                />
              )}
            </>
          )}
          {!isError && !parsed && !isLoading && (
            <EmptyBanner>Add patches and run the batch.</EmptyBanner>
          )}
        </ResultPanel>
      </ToolSplit>
    </ToolLayout>
  );
}
