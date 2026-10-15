import "@tanstack/react-start/client-only";
import { Bar, BarChart, ResponsiveContainer, Tooltip, XAxis } from "recharts";

export type MonthPoint = { month: string; count: number };

/** Isolated so Accueil can lazy-load recharts out of the main chunk. */
export function MonthlyChart({ data }: { data: MonthPoint[] }) {
  const longSeries = data.length > 24;
  const tickEvery = longSeries ? Math.max(1, Math.ceil(data.length / 6)) : 0;

  return (
    <ResponsiveContainer width="100%" height="100%">
      <BarChart data={data} margin={{ top: 2, right: 4, left: 0, bottom: 0 }}>
        <XAxis
          dataKey="month"
          tick={{ fontSize: 10, fill: "#6f685e" }}
          axisLine={false}
          tickLine={false}
          minTickGap={longSeries ? 28 : 8}
          interval={longSeries ? tickEvery - 1 : 0}
          tickFormatter={(value: string) => (longSeries ? value.slice(0, 4) : value)}
        />
        <Tooltip
          cursor={{ fill: "rgba(28,25,21,0.04)" }}
          contentStyle={{
            background: "#faf7f1",
            border: "1px solid #ddd4c4",
            borderRadius: 8,
            fontSize: 12,
          }}
          labelFormatter={(label) => String(label)}
        />
        <Bar dataKey="count" fill="#2c4a3e" maxBarSize={18} radius={[2, 2, 0, 0]} />
      </BarChart>
    </ResponsiveContainer>
  );
}
