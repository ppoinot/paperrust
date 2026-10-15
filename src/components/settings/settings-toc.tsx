import { useEffect, useState } from "react";
import { cn } from "@/lib/utils";

export type SettingsTocItem = {
  id: string;
  label: string;
};

type SettingsTocProps = {
  items: readonly SettingsTocItem[];
};

function scrollToId(id: string) {
  const el = document.getElementById(id);
  if (!el) return;
  const reduce =
    typeof window !== "undefined" &&
    window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  el.scrollIntoView({ behavior: reduce ? "auto" : "smooth", block: "start" });
  // Keep URL hash in sync without jumping again.
  if (typeof history !== "undefined") {
    history.replaceState(null, "", `#${id}`);
  }
}

export function SettingsToc({ items }: SettingsTocProps) {
  const [activeId, setActiveId] = useState(items[0]?.id ?? "");

  useEffect(() => {
    const nodes = items
      .map((item) => document.getElementById(item.id))
      .filter((n): n is HTMLElement => Boolean(n));
    if (nodes.length === 0) return;

    const ratios = new Map<string, number>();
    const observer = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          ratios.set(entry.target.id, entry.isIntersecting ? entry.intersectionRatio : 0);
        }
        let bestId = items[0]?.id ?? "";
        let bestRatio = -1;
        for (const item of items) {
          const r = ratios.get(item.id) ?? 0;
          if (r > bestRatio) {
            bestRatio = r;
            bestId = item.id;
          }
        }
        // Prefer the topmost intersecting section when ratios are equal.
        if (bestRatio <= 0) {
          const top = nodes.find((n) => {
            const rect = n.getBoundingClientRect();
            return rect.top < window.innerHeight * 0.45 && rect.bottom > 80;
          });
          if (top) bestId = top.id;
        }
        setActiveId(bestId);
      },
      {
        root: null,
        // Bias toward the section near the upper third of the viewport.
        rootMargin: "-12% 0px -55% 0px",
        threshold: [0, 0.1, 0.25, 0.5, 0.75, 1],
      },
    );

    for (const node of nodes) observer.observe(node);
    return () => observer.disconnect();
  }, [items]);

  return (
    <>
      {/* Narrow: sticky horizontal jump links — not a second app sidebar */}
      <nav
        aria-label="Table des matières"
        className="sticky top-14 z-20 -mx-1 border-b border-border/70 bg-bg/95 px-1 py-2 backdrop-blur-sm lg:hidden"
      >
        <p className="mb-1.5 text-[11px] font-medium tracking-wide text-faint uppercase">
          Table des matières
        </p>
        <ul className="flex gap-1.5 overflow-x-auto pb-0.5 [-ms-overflow-style:none] [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">
          {items.map((item) => {
            const active = item.id === activeId;
            return (
              <li key={item.id} className="shrink-0">
                <a
                  href={`#${item.id}`}
                  onClick={(e) => {
                    e.preventDefault();
                    scrollToId(item.id);
                  }}
                  className={cn(
                    "inline-flex rounded-full px-2.5 py-1 text-xs transition-colors",
                    active
                      ? "bg-primary/15 font-medium text-primary"
                      : "bg-surface-2/80 text-muted hover:bg-surface-2 hover:text-fg",
                  )}
                  aria-current={active ? "location" : undefined}
                >
                  {item.label}
                </a>
              </li>
            );
          })}
        </ul>
      </nav>

      {/* Desktop: sticky vertical ToC in the empty column left of the cards */}
      <nav
        aria-label="Table des matières"
        className="sticky top-20 hidden w-44 shrink-0 self-start lg:block xl:w-48"
      >
        <p className="font-display text-sm text-fg">Table des matières</p>
        <ul className="mt-3 space-y-0.5 border-l border-border">
          {items.map((item) => {
            const active = item.id === activeId;
            return (
              <li key={item.id}>
                <a
                  href={`#${item.id}`}
                  onClick={(e) => {
                    e.preventDefault();
                    scrollToId(item.id);
                  }}
                  className={cn(
                    "-ml-px block border-l-2 py-1.5 pr-2 pl-3 text-sm transition-colors",
                    active
                      ? "border-primary font-medium text-primary"
                      : "border-transparent text-muted hover:border-border hover:text-fg",
                  )}
                  aria-current={active ? "location" : undefined}
                >
                  {item.label}
                </a>
              </li>
            );
          })}
        </ul>
      </nav>
    </>
  );
}
