import { Check } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { isInboxTag } from "@/lib/paperless/format";
import type { Tag } from "@/lib/paperless/types";
import { cn } from "@/lib/utils";

const INBOX_RED = "#9a4a28";

export function TagChip({
  tag,
  onClick,
  active,
}: {
  tag: Tag;
  onClick?: () => void;
  active?: boolean;
}) {
  const interactive = typeof onClick === "function";
  const selected = interactive ? Boolean(active) : true;
  const inbox = isInboxTag(tag);
  const color = inbox ? tag.color || INBOX_RED : tag.color;

  const badge = (
    <Badge
      color={color}
      variant={selected ? "solid" : "outline"}
      className={cn(
        "pointer-events-none gap-0.5",
        inbox && "uppercase tracking-wide",
        interactive && !selected && "opacity-55",
      )}
    >
      {interactive && selected ? <Check className="size-3 shrink-0" strokeWidth={2.5} aria-hidden /> : null}
      {tag.name}
    </Badge>
  );

  if (onClick) {
    const toggle = onClick;
    return (
      <button
        type="button"
        onClick={(e) => {
          e.preventDefault();
          e.stopPropagation();
          toggle();
        }}
        aria-pressed={selected}
        className="relative z-[1] max-w-[11rem] cursor-pointer rounded-full focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
      >
        {badge}
      </button>
    );
  }

  return <span className="max-w-[11rem]">{badge}</span>;
}
