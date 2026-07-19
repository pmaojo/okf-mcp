import type { SearchHit } from "@/lib/mcp-types";
import { Badge } from "@/shared/components/ui/badge";

export function ConceptTable({
  rows,
  onSelect,
  emptyLabel = "No concepts matched.",
}: {
  rows: SearchHit[];
  onSelect?: (conceptId: string) => void;
  emptyLabel?: string;
}) {
  if (rows.length === 0) {
    return (
      <p className="border-2 border-dashed border-border p-6 text-center text-sm text-muted-foreground">
        {emptyLabel}
      </p>
    );
  }

  return (
    <div className="overflow-x-auto border-2 border-foreground">
      <table className="w-full border-collapse text-sm">
        <thead>
          <tr className="border-b-2 border-foreground bg-secondary text-left text-xs font-bold tracking-wide uppercase">
            <th className="px-3 py-2">Concept</th>
            <th className="px-3 py-2">Type</th>
            <th className="px-3 py-2">Tags</th>
            <th className="px-3 py-2">Hash</th>
          </tr>
        </thead>
        <tbody>
          {rows.map((hit) => (
            <tr
              key={hit.concept_id}
              className="border-b border-border last:border-b-0 hover:bg-muted"
            >
              <td className="px-3 py-2 font-mono">
                {onSelect ? (
                  <button
                    type="button"
                    onClick={() => onSelect(hit.concept_id)}
                    className="text-left underline decoration-dotted underline-offset-4 hover:text-accent-foreground hover:decoration-solid"
                  >
                    {hit.concept_id}
                  </button>
                ) : (
                  hit.concept_id
                )}
                {hit.title && (
                  <div className="font-sans text-xs font-normal text-muted-foreground">
                    {hit.title}
                  </div>
                )}
              </td>
              <td className="px-3 py-2">
                <Badge variant="outline">{hit.type}</Badge>
              </td>
              <td className="px-3 py-2">
                <div className="flex flex-wrap gap-1">
                  {hit.tags.map((tag) => (
                    <Badge key={tag} variant="secondary" className="normal-case">
                      {tag}
                    </Badge>
                  ))}
                </div>
              </td>
              <td className="px-3 py-2 font-mono text-xs text-muted-foreground">
                {hit.hash.slice(0, 10)}…
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
