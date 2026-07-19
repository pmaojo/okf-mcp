import { cn } from "@/lib/utils";

export function StatTile({
  label,
  value,
  tone = "default",
}: {
  label: string;
  value: string | number;
  tone?: "default" | "accent" | "danger";
}) {
  return (
    <div
      className={cn(
        "border-2 border-foreground p-4 shadow-brutal-sm",
        tone === "accent" && "bg-accent text-accent-foreground",
        tone === "danger" && "bg-destructive text-white",
        tone === "default" && "bg-card"
      )}
    >
      <div className="text-3xl font-extrabold tabular-nums">{value}</div>
      <div className="mt-1 text-xs font-bold tracking-wide uppercase opacity-80">
        {label}
      </div>
    </div>
  );
}

export function StatTileGrid({ children }: { children: React.ReactNode }) {
  return (
    <div className="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-4">
      {children}
    </div>
  );
}
