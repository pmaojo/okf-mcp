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
import { PeekConceptCard } from "@/shared/components/tool/PeekConceptCard";
import { AddContextButton } from "@/shared/components/tool/AddContextButton";
import { Input } from "@/shared/components/ui/input";
import { Button } from "@/shared/components/ui/button";
import { Switch } from "@/shared/components/ui/switch";
import { Label } from "@/shared/components/ui/label";
import { Badge } from "@/shared/components/ui/badge";
import { useServerTool } from "@/shared/hooks/useServerTool";
import { usePeekTool } from "@/shared/hooks/usePeekTool";
import { parseToolPayload } from "@/lib/tool-result";
import type { SkillIngestResult } from "@/lib/mcp-types";

const FORMATS = ["auto", "agentic-skills", "shadcn", "okf", "raw"] as const;

export function SkillIngestView({ app, toolResult }: ToolComponentProps) {
  const [source, setSource] = useState("");
  const [pathPrefix, setPathPrefix] = useState("");
  const [format, setFormat] = useState<(typeof FORMATS)[number]>("auto");
  const [dryRun, setDryRun] = useState(true);

  const { activeResult, isError, isLoading, executeTool, isManual } = useServerTool(
    app,
    "skill_ingest",
    toolResult
  );
  const parsed = parseToolPayload<SkillIngestResult>(activeResult);
  const resolvePeek = usePeekTool(app);

  const run = (overrideDryRun?: boolean) => {
    if (!source.trim() || !pathPrefix.trim()) {
      toast.error("source and path_prefix are required.");
      return;
    }
    resolvePeek.reset();
    void executeTool({
      source: source.trim(),
      path_prefix: pathPrefix.trim(),
      format,
      dry_run: overrideDryRun ?? dryRun,
    });
  };

  return (
    <ToolLayout>
      <ToolHeader
        slug="skill_ingest"
        title="Ingest a skill from an external source"
        description="Content is always kept verbatim under a generated OKF header — never rewritten or summarized by any model. Not available if this deployment has no source fetcher configured."
      />
      <ToolSplit>
        <RunPanel>
          <Field id="si-source" label="Source" hint="Repo URL, owner/repo, or a direct file URL.">
            <Input
              id="si-source"
              value={source}
              onChange={(e) => setSource(e.target.value)}
              placeholder="anthropics/skills"
            />
          </Field>
          <Field id="si-prefix" label="path_prefix">
            <Input
              id="si-prefix"
              value={pathPrefix}
              onChange={(e) => setPathPrefix(e.target.value)}
              placeholder="skills/programming"
            />
          </Field>
          <Field id="si-format" label="Format">
            <select
              id="si-format"
              value={format}
              onChange={(e) => setFormat(e.target.value as (typeof FORMATS)[number])}
              className="h-9 w-full border-2 border-input bg-background px-3 text-sm"
            >
              {FORMATS.map((f) => (
                <option key={f} value={f}>
                  {f}
                </option>
              ))}
            </select>
          </Field>
          <div className="flex items-center gap-3">
            <Switch id="si-dry-run" checked={dryRun} onCheckedChange={setDryRun} />
            <Label htmlFor="si-dry-run" className="text-xs font-bold tracking-wide uppercase">
              Dry run (show the plan, don't commit)
            </Label>
          </div>
          <Button onClick={() => run()} disabled={isLoading} className="w-full">
            {isLoading ? "Ingesting…" : dryRun ? "Dry-run skill_ingest" : "Run skill_ingest"}
          </Button>
        </RunPanel>

        <ResultPanel>
          {isError && (
            <ErrorBanner
              title="Ingest failed"
              detail="Check the source URL/shortcut and that this deployment has a fetcher configured."
            />
          )}
          {!isError && parsed && (
            <>
              <div className="flex flex-wrap items-center gap-2">
                <Badge variant="outline">{parsed.format}</Badge>
                {parsed.license && <Badge variant="secondary">{parsed.license}</Badge>}
                {parsed.dry_run && <Badge variant="outline">dry run</Badge>}
              </div>

              {parsed.dry_run && parsed.units && (
                <div className="space-y-2">
                  {parsed.units.map((u) => (
                    <div key={u.concept_id} className="border-2 border-foreground bg-card p-3 shadow-brutal-sm">
                      <div className="flex flex-wrap items-center gap-2">
                        <span className="font-mono text-xs font-bold">{u.concept_id}</span>
                        <Badge variant="outline">{u.action}</Badge>
                      </div>
                      <p className="mt-1 text-sm">{u.title}</p>
                      {u.warnings.length > 0 && (
                        <div className="mt-1 space-y-0.5">
                          {u.warnings.map((w, i) => (
                            <p key={i} className="text-xs text-destructive">
                              ⚠ {w}
                            </p>
                          ))}
                        </div>
                      )}
                    </div>
                  ))}
                  <Button
                    variant="destructive"
                    size="sm"
                    disabled={isLoading}
                    onClick={() => {
                      setDryRun(false);
                      run(false);
                    }}
                  >
                    Looks good — run for real
                  </Button>
                </div>
              )}

              {!parsed.dry_run && parsed.items && (
                <div className="space-y-2">
                  <Badge>{parsed.ingested} ingested</Badge>
                  {parsed.items.map((item) => (
                    <div key={item.concept_id} className="border-2 border-foreground bg-card p-3 shadow-brutal-sm">
                      <div className="flex flex-wrap items-center justify-between gap-2">
                        <span className="font-mono text-xs font-bold">{item.concept_id}</span>
                        <Button
                          variant="outline"
                          size="icon-sm"
                          aria-label="resolve"
                          onClick={() => resolvePeek.peek("memory_resolve", { concept_id: item.concept_id })}
                        >
                          →
                        </Button>
                      </div>
                      {item.warnings.length > 0 && (
                        <div className="mt-1 space-y-0.5">
                          {item.warnings.map((w, i) => (
                            <p key={i} className="text-xs text-destructive">
                              ⚠ {w}
                            </p>
                          ))}
                        </div>
                      )}
                    </div>
                  ))}
                  {isManual && (
                    <AddContextButton
                      app={app}
                      label="Add ingest summary to agent context"
                      text={`skill_ingest imported ${parsed.ingested} skill(s) from ${parsed.source_url} into ${pathPrefix.trim()}:\n${(parsed.concept_ids ?? []).map((id) => `- ${id}`).join("\n")}`}
                    />
                  )}
                </div>
              )}

              {parsed.skipped.length > 0 && (
                <div className="space-y-1">
                  <p className="text-xs font-bold tracking-wide uppercase text-muted-foreground">Skipped</p>
                  {parsed.skipped.map((s, i) => (
                    <p key={i} className="text-xs text-muted-foreground">
                      {s.item}: {s.reason}
                    </p>
                  ))}
                </div>
              )}

              {(resolvePeek.result || resolvePeek.isLoading || resolvePeek.isError) && (
                <PeekConceptCard
                  result={resolvePeek.result}
                  isLoading={resolvePeek.isLoading}
                  isError={resolvePeek.isError}
                />
              )}
            </>
          )}
          {!isError && !parsed && !isLoading && (
            <EmptyBanner>Fill in the form and run skill_ingest.</EmptyBanner>
          )}
        </ResultPanel>
      </ToolSplit>
    </ToolLayout>
  );
}
