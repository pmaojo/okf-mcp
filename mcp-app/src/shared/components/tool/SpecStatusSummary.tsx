import { Badge } from "@/shared/components/ui/badge";
import { StatTile, StatTileGrid } from "@/shared/components/tool/StatTile";
import type { SpecStatusResult } from "@/lib/mcp-types";

export function SpecStatusSummary({
  status,
  onResolveTask,
}: {
  status: SpecStatusResult;
  onResolveTask?: (taskId: string) => void;
}) {
  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-center gap-2">
        <Badge>{status.spec_status}</Badge>
        <span className="font-mono text-sm font-bold">
          {status.spec_title ?? status.spec_id}
        </span>
      </div>

      <div className="h-4 w-full border-2 border-foreground bg-muted">
        <div
          className="h-full bg-accent"
          style={{ width: `${Math.round(status.progress * 100)}%` }}
        />
      </div>
      <p className="text-xs text-muted-foreground">
        {Math.round(status.progress * 100)}% done · {status.tasks_total} task(s)
      </p>

      <StatTileGrid>
        <StatTile label="Pending" value={status.by_status.pending} />
        <StatTile label="In progress" value={status.by_status.in_progress} tone="accent" />
        <StatTile label="Done" value={status.by_status.done} />
        <StatTile
          label="Blocked"
          value={status.by_status.blocked}
          tone={status.by_status.blocked > 0 ? "danger" : "default"}
        />
      </StatTileGrid>

      {status.next_pending.length > 0 && (
        <div>
          <p className="mb-1 text-xs font-bold tracking-wide uppercase text-muted-foreground">
            Next up ({status.waiting_on_dependencies} more waiting on dependencies)
          </p>
          <div className="space-y-1">
            {status.next_pending.map((id) =>
              onResolveTask ? (
                <button
                  key={id}
                  type="button"
                  onClick={() => onResolveTask(id)}
                  className="block w-full border-2 border-foreground bg-card px-2 py-1 text-left font-mono text-xs shadow-brutal-sm hover:bg-accent hover:text-accent-foreground"
                >
                  {id}
                </button>
              ) : (
                <div key={id} className="font-mono text-xs">
                  {id}
                </div>
              )
            )}
          </div>
        </div>
      )}
    </div>
  );
}
