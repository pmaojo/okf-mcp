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

export function RunPanel({ children }: { children: React.ReactNode }) {
  return (
    <div className="border-2 border-foreground bg-card p-4 shadow-brutal">
      <div className="space-y-4">{children}</div>
    </div>
  );
}

export function ResultPanel({ children }: { children: React.ReactNode }) {
  return <div className="space-y-4">{children}</div>;
}
