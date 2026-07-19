import { Badge } from "@/shared/components/ui/badge";
import type { CommitLikeResult, MemoryDeleteResult } from "@/lib/mcp-types";

function Row({ label, value }: { label: string; value: React.ReactNode }) {
  return (
    <div className="flex items-baseline justify-between gap-4 border-b border-border py-1.5 last:border-b-0">
      <span className="text-xs font-bold tracking-wide text-muted-foreground uppercase">
        {label}
      </span>
      <span className="truncate font-mono text-sm">{value}</span>
    </div>
  );
}

export function CommitResultCard({ result }: { result: CommitLikeResult }) {
  return (
    <div className="border-2 border-foreground bg-card p-4 shadow-brutal-sm">
      <div className="mb-2 flex flex-wrap gap-2">
        {result.dry_run && <Badge variant="outline">dry run</Badge>}
        {result.created ? (
          <Badge>created</Badge>
        ) : (
          <Badge variant="secondary">updated</Badge>
        )}
        {result.no_change && <Badge variant="outline">no change</Badge>}
      </div>
      <Row label="concept_id" value={result.concept_id} />
      <Row label="hash" value={result.hash} />
      <Row label="version" value={result.version} />
      <Row
        label="revision_seq"
        value={result.revision_seq ?? "— (dry run)"}
      />
    </div>
  );
}

export function DeleteResultCard({ result }: { result: MemoryDeleteResult }) {
  return (
    <div className="border-2 border-destructive bg-card p-4 shadow-brutal-sm">
      <div className="mb-2">
        <Badge variant="destructive">deleted</Badge>
      </div>
      <Row label="concept_id" value={result.concept_id} />
      <Row label="hash" value={result.hash} />
      <Row label="version" value={result.version} />
      <Row label="revision_seq" value={result.revision_seq} />
    </div>
  );
}
