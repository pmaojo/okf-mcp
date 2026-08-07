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
import { AddContextButton } from "@/shared/components/tool/AddContextButton";
import { Input } from "@/shared/components/ui/input";
import { Textarea } from "@/shared/components/ui/textarea";
import { Button } from "@/shared/components/ui/button";
import { Switch } from "@/shared/components/ui/switch";
import { Label } from "@/shared/components/ui/label";
import { useServerTool } from "@/shared/hooks/useServerTool";
import { parseToolPayload } from "@/lib/tool-result";
import type { MemoryCommitResult } from "@/lib/mcp-types";

const PLACEHOLDER_MARKDOWN = `---
type: person
title: Alice
tags:
  - dev
---
Works on [[projects/okf-mcp]].
`;

export function MemoryCommitView({ app, toolResult }: ToolComponentProps) {
  const [conceptId, setConceptId] = useState("");
  const [markdown, setMarkdown] = useState(PLACEHOLDER_MARKDOWN);
  const [reason, setReason] = useState("");
  const [expectedHash, setExpectedHash] = useState("");
  const [dryRun, setDryRun] = useState(true);

  const { activeResult, isError, isLoading, executeTool, isManual } = useServerTool(
    app,
    "memory_commit",
    toolResult
  );
  const parsed = parseToolPayload<MemoryCommitResult>(activeResult);

  const run = () => {
    if (!conceptId.trim() || !markdown.trim() || !reason.trim()) {
      toast.error("concept_id, markdown and reason are required.");
      return;
    }
    const args: Record<string, unknown> = {
      concept_id: conceptId.trim(),
      markdown,
      reason: reason.trim(),
      dry_run: dryRun,
    };
    if (expectedHash.trim()) args.expected_hash = expectedHash.trim();
    void executeTool(args);
  };

  return (
    <ToolLayout>
      <ToolHeader
        slug="memory_commit"
        title="Write a document"
        description="Omit expected_hash to create; pass the hash from memory_resolve to update with conflict detection. Dry-run before you commit for real."
      />
      <ToolSplit>
        <RunPanel defaultOpen={!toolResult}>
          <Field id="mc-id" label="concept_id">
            <Input
              id="mc-id"
              value={conceptId}
              onChange={(e) => setConceptId(e.target.value)}
              placeholder="people/alice"
            />
          </Field>
          <Field id="mc-markdown" label="Markdown (frontmatter + body)">
            <Textarea
              id="mc-markdown"
              rows={12}
              value={markdown}
              onChange={(e) => setMarkdown(e.target.value)}
            />
          </Field>
          <Field id="mc-reason" label="Reason">
            <Input
              id="mc-reason"
              value={reason}
              onChange={(e) => setReason(e.target.value)}
              placeholder="initial commit"
            />
          </Field>
          <Field
            id="mc-hash"
            label="expected_hash (updates only)"
            hint="Leave empty when creating a new concept."
          >
            <Input
              id="mc-hash"
              value={expectedHash}
              onChange={(e) => setExpectedHash(e.target.value)}
              placeholder="sha256 hex from memory_resolve"
              className="font-mono"
            />
          </Field>
          <div className="flex items-center gap-3">
            <Switch id="mc-dry-run" checked={dryRun} onCheckedChange={setDryRun} />
            <Label htmlFor="mc-dry-run" className="text-xs font-bold tracking-wide uppercase">
              Dry run (validate only, don't persist)
            </Label>
          </div>
          <Button onClick={run} disabled={isLoading} className="w-full">
            {isLoading ? "Committing…" : dryRun ? "Dry-run memory_commit" : "Run memory_commit"}
          </Button>
        </RunPanel>

        <ResultPanel>
          {isError && (
            <ErrorBanner
              title="Commit failed"
              detail="Likely a revision conflict (stale expected_hash) or invalid frontmatter."
            />
          )}
          {!isError && parsed && (
            <>
              <CommitResultCard result={parsed} />
              {isManual && !parsed.dry_run && (
                <AddContextButton
                  app={app}
                  text={`memory_commit just ${parsed.created ? "created" : "updated"} ${parsed.concept_id} (v${parsed.version}, reason: "${reason.trim()}")${parsed.no_change ? " — content was unchanged." : "."}`}
                />
              )}
            </>
          )}
          {!isError && !parsed && !isLoading && (
            <EmptyBanner>Fill in the form and run memory_commit.</EmptyBanner>
          )}
        </ResultPanel>
      </ToolSplit>
    </ToolLayout>
  );
}
