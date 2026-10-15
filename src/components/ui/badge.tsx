import { cn } from "@/lib/utils";
import type { CSSProperties, HTMLAttributes } from "react";

function parseHex(color: string): [number, number, number] | null {
  let h = color.trim().replace(/^#/, "");
  if (h.length === 3) h = [...h].map((c) => c + c).join("");
  if (!/^[0-9a-fA-F]{6}$/.test(h)) return null;
  return [parseInt(h.slice(0, 2), 16), parseInt(h.slice(2, 4), 16), parseInt(h.slice(4, 6), 16)];
}

function contrastFg(color: string): string {
  const rgb = parseHex(color);
  if (!rgb) return "#fffaf7";
  const [r, g, b] = rgb;
  const l = (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255;
  return l > 0.62 ? "#1c1917" : "#fffaf7";
}

export function Badge({
  className,
  color,
  variant = "soft",
  ...props
}: HTMLAttributes<HTMLSpanElement> & {
  color?: string;
  variant?: "soft" | "solid" | "outline";
}) {
  let style: CSSProperties | undefined;
  if (color) {
    if (variant === "solid") {
      style = { background: color, color: contrastFg(color), boxShadow: "none" };
    } else if (variant === "outline") {
      style = {
        background: `${color}1a`,
        color,
        boxShadow: `inset 0 0 0 1px ${color}99`,
      };
    } else {
      style = { background: `${color}22`, color, boxShadow: `inset 0 0 0 1px ${color}55` };
    }
  }
  return (
    <span
      {...props}
      className={cn(
        "inline-flex max-w-full items-center truncate rounded-full px-2 py-0.5 text-[11px] font-medium",
        className,
      )}
      style={style}
    />
  );
}
