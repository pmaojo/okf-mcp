import { useState } from "react";

export function ToolLayout({ children }: { children: React.ReactNode }) {
  return (
    <div className="mx-auto flex w-full max-w-6xl flex-col gap-6 p-4 md:p-6">
      {children}
    </div>
  );
}

export function ToolSplit({ children }: { children: React.ReactNode }) {
  return (
    <div className="grid items-start gap-6 xl:grid-cols-[1fr_1.3fr]">
      {children}
    </div>
  );
}

/**
 * The tool's input form. Pass `defaultOpen={false}` when the view already
 * has a host-provided result (the model called this tool on the user's
 * behalf) — there's nothing for a human to fill in, so the form starts
 * collapsed behind a toggle instead of competing with the result for
 * attention. Opened cold, with no result yet, it should stay expanded so
 * a human can drive the tool manually.
 */
export function RunPanel({
  children,
  defaultOpen = true,
}: {
  children: React.ReactNode;
  defaultOpen?: boolean;
}) {
  const [open, setOpen] = useState(defaultOpen);
  return (
    <div className="border-2 border-foreground bg-card p-4 shadow-brutal">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        aria-expanded={open}
        aria-label={open ? "Hide inputs" : "Edit inputs"}
        className="flex w-full items-center justify-between text-left text-xs font-bold uppercase tracking-wide text-muted-foreground"
      >
        <span aria-hidden="true">Inputs</span>
        <span aria-hidden="true">{open ? "− hide" : "+ edit"}</span>
      </button>
      {open && <div className="mt-4 space-y-4">{children}</div>}
    </div>
  );
}

export function ResultPanel({ children }: { children: React.ReactNode }) {
  return <div className="space-y-4">{children}</div>;
}
