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
import { CommitResultCard } from "@/shared/components/tool/CommitResultCard";
import { Input } from "@/shared/components/ui/input";
import { Button } from "@/shared/components/ui/button";
import { Switch } from "@/shared/components/ui/switch";
import { Label } from "@/shared/components/ui/label";
import { useServerTool } from "@/shared/hooks/useServerTool";
import { parseToolPayload } from "@/lib/tool-result";
import type { MemoryPatchResult } from "@/lib/mcp-types";

interface SetRow {
  key: string;
  value: string;
}

function splitCsv(value: string): string[] {
  return value
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean);
}

export function MemoryPatchView({ app, toolResult }: ToolComponentProps) {
  const [conceptId, setConceptId] = useState("");
  const [expectedHash, setExpectedHash] = useState("");
  const [reason, setReason] = useState("");
  const [setRows, setSetRows] = useState<SetRow[]>([{ key: "", value: "" }]);
  const [removeKeys, setRemoveKeys] = useState("");
  const [addTags, setAddTags] = useState("");
  const [removeTags, setRemoveTags] = useState("");
  const [dryRun, setDryRun] = useState(true);

  const { activeResult, isError, isLoading, executeTool } = useServerTool(
    app,
    "memory_patch",
    toolResult
  );
  const parsed = parseToolPayload<MemoryPatchResult>(activeResult);

  const updateRow = (i: number, patch: Partial<SetRow>) =>
    setSetRows((rows) => rows.map((r, idx) => (idx === i ? { ...r, ...patch } : r)));

  const run = () => {
    if (!conceptId.trim() || !expectedHash.trim() || !reason.trim()) {
      toast.error("concept_id, expected_hash and reason are required.");
      return;
    }
    const set = Object.fromEntries(
      setRows.filter((r) => r.key.trim()).map((r) => [r.key.trim(), r.value])
    );
    const args: Record<string, unknown> = {
      concept_id: conceptId.trim(),
      expected_hash: expectedHash.trim(),
      reason: reason.trim(),
      dry_run: dryRun,
    };
    if (Object.keys(set).length > 0) args.set = set;
    const remove = splitCsv(removeKeys);
    if (remove.length > 0) args.remove = remove;
    const addTagList = splitCsv(addTags);
    if (addTagList.length > 0) args.add_tags = addTagList;
    const removeTagList = splitCsv(removeTags);
    if (removeTagList.length > 0) args.remove_tags = removeTagList;
    void executeTool(args);
  };

  return (
    <ToolLayout>
      <ToolHeader
        slug="memory_patch"
        title="Patch frontmatter"
        description="Update or remove specific frontmatter keys and tags — the Markdown body is left untouched."
      />
      <ToolSplit>
        <RunPanel>
          <div className="grid grid-cols-2 gap-3">
            <Field id="mp-id" label="concept_id">
              <Input
                id="mp-id"
                value={conceptId}
                onChange={(e) => setConceptId(e.target.value)}
                placeholder="people/alice"
              />
            </Field>
            <Field id="mp-hash" label="expected_hash">
              <Input
                id="mp-hash"
                value={expectedHash}
                onChange={(e) => setExpectedHash(e.target.value)}
                className="font-mono"
                placeholder="sha256 hex"
              />
            </Field>
          </div>
          <Field id="mp-reason" label="Reason">
            <Input
              id="mp-reason"
              value={reason}
              onChange={(e) => setReason(e.target.value)}
              placeholder="promote to lead"
            />
          </Field>

          <Field id="mp-set" label="Set fields" hint="Key/value pairs written into frontmatter.">
            <div className="space-y-2">
              {setRows.map((row, i) => (
                <div key={i} className="flex gap-2">
                  <Input
                    aria-label={`set key ${i}`}
                    value={row.key}
                    onChange={(e) => updateRow(i, { key: e.target.value })}
                    placeholder="key"
                    className="w-1/3"
                  />
                  <Input
                    aria-label={`set value ${i}`}
                    value={row.value}
                    onChange={(e) => updateRow(i, { value: e.target.value })}
                    placeholder="value"
                  />
                  <Button
                    type="button"
                    variant="outline"
                    size="icon"
                    onClick={() => setSetRows((rows) => rows.filter((_, idx) => idx !== i))}
                    aria-label="remove row"
                  >
                    ×
                  </Button>
                </div>
              ))}
              <Button
                type="button"
                variant="secondary"
                size="sm"
                onClick={() => setSetRows((rows) => [...rows, { key: "", value: "" }])}
              >
                + add field
              </Button>
            </div>
          </Field>

          <Field id="mp-remove" label="Remove keys" hint="Comma-separated frontmatter keys.">
            <Input
              id="mp-remove"
              value={removeKeys}
              onChange={(e) => setRemoveKeys(e.target.value)}
              placeholder="deprecated_field"
            />
          </Field>
          <div className="grid grid-cols-2 gap-3">
            <Field id="mp-add-tags" label="Add tags" hint="Comma-separated.">
              <Input
                id="mp-add-tags"
                value={addTags}
                onChange={(e) => setAddTags(e.target.value)}
                placeholder="lead"
              />
            </Field>
            <Field id="mp-remove-tags" label="Remove tags" hint="Comma-separated.">
              <Input
                id="mp-remove-tags"
                value={removeTags}
                onChange={(e) => setRemoveTags(e.target.value)}
                placeholder="junior"
              />
            </Field>
          </div>

          <div className="flex items-center gap-3">
            <Switch id="mp-dry-run" checked={dryRun} onCheckedChange={setDryRun} />
            <Label htmlFor="mp-dry-run" className="text-xs font-bold tracking-wide uppercase">
              Dry run (validate only, don't persist)
            </Label>
          </div>
          <Button onClick={run} disabled={isLoading} className="w-full">
            {isLoading ? "Patching…" : dryRun ? "Dry-run memory_patch" : "Run memory_patch"}
          </Button>
        </RunPanel>

        <ResultPanel>
          {isError && (
            <ErrorBanner
              title="Patch failed"
              detail="Likely a stale expected_hash, or the document doesn't exist."
            />
          )}
          {!isError && parsed && <CommitResultCard result={parsed} />}
          {!isError && !parsed && !isLoading && (
            <EmptyBanner>Fill in the form and run memory_patch.</EmptyBanner>
          )}
        </ResultPanel>
      </ToolSplit>
    </ToolLayout>
  );
}
