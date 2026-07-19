import { Bar, BarChart, CartesianGrid, XAxis, YAxis } from "recharts";
import {
  ChartContainer,
  ChartTooltip,
  ChartTooltipContent,
  type ChartConfig,
} from "@/shared/components/ui/chart";

export interface BarDatum {
  label: string;
  value: number;
}

const CHART_COLORS = [
  "var(--chart-1)",
  "var(--chart-2)",
  "var(--chart-3)",
  "var(--chart-4)",
  "var(--chart-5)",
];

/** Horizontal or vertical brutalist bar chart for one series of counts. */
export function BarChartCard({
  data,
  seriesLabel,
  layout = "vertical",
  height = 260,
  emptyLabel = "Nothing to chart yet.",
}: {
  data: BarDatum[];
  seriesLabel: string;
  layout?: "vertical" | "horizontal";
  height?: number;
  emptyLabel?: string;
}) {
  if (data.length === 0) {
    return (
      <p className="border-2 border-dashed border-border p-6 text-center text-sm text-muted-foreground">
        {emptyLabel}
      </p>
    );
  }

  const config: ChartConfig = {
    value: { label: seriesLabel, color: CHART_COLORS[0] },
  };

  const isHorizontal = layout === "horizontal";

  return (
    <div
      className="border-2 border-foreground bg-card p-3 shadow-brutal-sm"
      style={{ height }}
    >
      <ChartContainer config={config}>
        <BarChart
          data={data}
          layout={isHorizontal ? "vertical" : "horizontal"}
          margin={{ top: 4, right: 12, left: 4, bottom: 4 }}
        >
          <CartesianGrid stroke="var(--border)" strokeDasharray="4 4" />
          {isHorizontal ? (
            <>
              <XAxis type="number" stroke="var(--foreground)" allowDecimals={false} />
              <YAxis
                type="category"
                dataKey="label"
                stroke="var(--foreground)"
                width={110}
                tick={{ fontSize: 11 }}
              />
            </>
          ) : (
            <>
              <XAxis
                dataKey="label"
                stroke="var(--foreground)"
                tick={{ fontSize: 11 }}
                interval={0}
                angle={-25}
                textAnchor="end"
                height={50}
              />
              <YAxis stroke="var(--foreground)" allowDecimals={false} />
            </>
          )}
          <ChartTooltip content={<ChartTooltipContent />} />
          <Bar
            dataKey="value"
            fill="var(--color-value)"
            stroke="var(--border)"
            strokeWidth={2}
          />
        </BarChart>
      </ChartContainer>
    </div>
  );
}
