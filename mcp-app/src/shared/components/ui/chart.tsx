import * as React from "react";
import * as RechartsPrimitive from "recharts";

import { cn } from "@/lib/utils";

/**
 * Trimmed port of the shadcn chart primitives (ui.shadcn.com/charts),
 * kept to the pieces this app actually uses: a themed container that
 * wires `--color-<key>` CSS variables from a config, plus a tooltip
 * that reads them back out.
 */

export type ChartConfig = Record<
  string,
  { label: string; color: string }
>;

type ChartContextValue = { config: ChartConfig };
const ChartContext = React.createContext<ChartContextValue | null>(null);

function useChart() {
  const ctx = React.useContext(ChartContext);
  if (!ctx) throw new Error("Chart components must be used within <ChartContainer>");
  return ctx;
}

export function ChartContainer({
  config,
  className,
  children,
  ...props
}: React.ComponentProps<"div"> & {
  config: ChartConfig;
  children: React.ComponentProps<
    typeof RechartsPrimitive.ResponsiveContainer
  >["children"];
}) {
  const style = Object.fromEntries(
    Object.entries(config).map(([key, value]) => [
      `--color-${key}`,
      value.color,
    ])
  ) as React.CSSProperties;

  return (
    <ChartContext.Provider value={{ config }}>
      <div
        data-slot="chart"
        className={cn("h-full w-full font-mono text-xs", className)}
        style={style}
        {...props}
      >
        <RechartsPrimitive.ResponsiveContainer>
          {children}
        </RechartsPrimitive.ResponsiveContainer>
      </div>
    </ChartContext.Provider>
  );
}

export function ChartTooltipContent({
  active,
  payload,
  label,
}: {
  active?: boolean;
  payload?: readonly { name?: string; value?: number | string; dataKey?: string; color?: string }[];
  label?: string;
}) {
  const { config } = useChart();
  if (!active || !payload || payload.length === 0) return null;

  return (
    <div className="border-2 border-foreground bg-popover px-3 py-2 text-popover-foreground shadow-brutal-sm">
      {label && (
        <div className="mb-1 text-xs font-bold tracking-wide uppercase">
          {label}
        </div>
      )}
      <div className="space-y-0.5">
        {payload.map((item, i) => {
          const key = item.dataKey ?? item.name ?? String(i);
          const entry = config[key as string];
          return (
            <div key={key} className="flex items-center gap-2 text-xs">
              <span
                className="size-2 border border-foreground"
                style={{ background: item.color }}
              />
              <span>{entry?.label ?? item.name}</span>
              <span className="ml-auto font-bold tabular-nums">
                {item.value}
              </span>
            </div>
          );
        })}
      </div>
    </div>
  );
}

export const ChartTooltip = RechartsPrimitive.Tooltip;
