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
import { DeleteResultCard } from "@/shared/components/tool/CommitResultCard";
import { Input } from "@/shared/components/ui/input";
import { Button } from "@/shared/components/ui/button";
import { useServerTool } from "@/shared/hooks/useServerTool";
import { parseToolPayload } from "@/lib/tool-result";
import type { MemoryDeleteResult } from "@/lib/mcp-types";

export function MemoryDeleteView({ app, toolResult }: ToolComponentProps) {
  const [conceptId, setConceptId] = useState("");
  const [expectedHash, setExpectedHash] = useState("");
  const [reason, setReason] = useState("");
  const [confirmText, setConfirmText] = useState("");

  const { activeResult, isError, isLoading, executeTool } = useServerTool(
    app,
    "memory_delete",
    toolResult
  );
  const parsed = parseToolPayload<MemoryDeleteResult>(activeResult);
  const confirmed = confirmText.trim() === conceptId.trim() && conceptId.trim().length > 0;

  const run = () => {
    if (!conceptId.trim() || !expectedHash.trim() || !reason.trim()) {
      toast.error("concept_id, expected_hash and reason are required.");
      return;
    }
    if (!confirmed) {
      toast.error("Type the concept_id in the confirm box to enable delete.");
      return;
    }
    void executeTool({
      concept_id: conceptId.trim(),
      expected_hash: expectedHash.trim(),
      reason: reason.trim(),
    });
  };

  return (
    <ToolLayout>
      <ToolHeader
        slug="memory_delete"
        title="Delete a concept"
        description="Logical delete, guarded by expected_hash — this is destructive and cannot be undone from this UI."
        kicker="danger"
      />
      <ToolSplit>
        <RunPanel>
          <Field id="md-id" label="concept_id">
            <Input
              id="md-id"
              value={conceptId}
              onChange={(e) => setConceptId(e.target.value)}
              placeholder="people/alice"
            />
          </Field>
          <Field id="md-hash" label="expected_hash">
            <Input
              id="md-hash"
              value={expectedHash}
              onChange={(e) => setExpectedHash(e.target.value)}
              className="font-mono"
              placeholder="sha256 hex from memory_resolve"
            />
          </Field>
          <Field id="md-reason" label="Reason">
            <Input
              id="md-reason"
              value={reason}
              onChange={(e) => setReason(e.target.value)}
              placeholder="duplicate entry"
            />
          </Field>
          <Field
            id="md-confirm"
            label={`Type "${conceptId.trim() || "concept_id"}" to confirm`}
          >
            <Input
              id="md-confirm"
              value={confirmText}
              onChange={(e) => setConfirmText(e.target.value)}
              placeholder={conceptId.trim() || "concept_id"}
            />
          </Field>
          <Button
            onClick={run}
            disabled={isLoading || !confirmed}
            variant="destructive"
            className="w-full"
          >
            {isLoading ? "Deleting…" : "Run memory_delete"}
          </Button>
        </RunPanel>

        <ResultPanel>
          {isError && (
            <ErrorBanner
              title="Delete failed"
              detail="Likely a stale expected_hash — resolve the concept again."
            />
          )}
          {!isError && parsed && <DeleteResultCard result={parsed} />}
          {!isError && !parsed && !isLoading && (
            <EmptyBanner>Fill in the form and confirm to delete.</EmptyBanner>
          )}
        </ResultPanel>
      </ToolSplit>
    </ToolLayout>
  );
}
