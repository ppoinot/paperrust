import type { LucideIcon } from "lucide-react";
import type { ReactNode } from "react";
import { cn } from "@/lib/utils";

/** Guided empty placeholder for catalog / ops pages. */
export function EmptyState({
  icon: Icon,
  title,
  description,
  action,
  className,
}: {
  icon: LucideIcon;
  title: string;
  description?: string;
  action?: ReactNode;
  className?: string;
}) {
  return (
    <div
      className={cn(
        "flex flex-col items-center justify-center gap-3 rounded-xl bg-surface px-6 py-14 text-center shadow-[var(--shadow-border)]",
        className,
      )}
    >
      <div className="flex size-12 items-center justify-center rounded-full bg-surface-2 text-muted">
        <Icon className="size-6" aria-hidden />
      </div>
      <div className="max-w-sm space-y-1">
        <p className="font-medium text-fg">{title}</p>
        {description ? <p className="text-sm text-muted">{description}</p> : null}
      </div>
      {action ? <div className="mt-1">{action}</div> : null}
    </div>
  );
}
