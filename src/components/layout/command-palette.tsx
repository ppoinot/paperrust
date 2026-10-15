import { useQuery } from "@tanstack/react-query";
import { useRouter } from "@tanstack/react-router";
import { FileText, Search } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { AUTOMATION, MANAGE, NAV } from "./nav-config";
import { searchDocuments } from "@/lib/paperless/actions";

export function CommandPalette() {
  const [open, setOpen] = useState(false);
  const [q, setQ] = useState("");
  const [debouncedQ, setDebouncedQ] = useState("");
  const router = useRouter();

  useEffect(() => {
    const t = window.setTimeout(() => setDebouncedQ(q.trim()), 250);
    return () => window.clearTimeout(t);
  }, [q]);

  const search = useQuery({
    queryKey: ["search", debouncedQ],
    queryFn: () => searchDocuments({ data: { q: debouncedQ } }),
    enabled: open && debouncedQ.length > 1,
  });

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setOpen((v) => !v);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const pages = useMemo(
    () =>
      [...NAV, ...MANAGE, ...AUTOMATION].filter((p) =>
        p.label.toLowerCase().includes(q.toLowerCase()),
      ),
    [q],
  );

  function go(to: string) {
    setOpen(false);
    setQ("");
    router.history.push(to);
  }

  return (
    <>
      <button
        type="button"
        onClick={() => setOpen(true)}
        className="hidden h-10 min-w-52 items-center gap-2 rounded-md bg-surface px-3 text-sm text-muted shadow-[var(--shadow-border)] md:flex"
      >
        <Search className="size-4" />
        <span className="flex-1 text-left">Rechercher</span>
        <kbd className="rounded bg-surface-2 px-1.5 py-0.5 font-mono text-[10px] text-faint">
          ⌘K
        </kbd>
      </button>
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent className="max-w-xl p-0">
          <DialogTitle className="sr-only">Recherche</DialogTitle>
          <div className="flex items-center gap-2 border-b border-border px-3">
            <Search className="size-4 text-muted" />
            <Input
              autoFocus
              value={q}
              onChange={(e) => setQ(e.target.value)}
              placeholder="Documents, pages, correspondants…"
              className="h-12 border-0 shadow-none focus-visible:ring-0"
            />
          </div>
          <div className="max-h-80 overflow-auto p-2">
            {search.isError ? (
              <p className="px-2 py-2 text-sm text-danger">
                Recherche impossible : {(search.error as Error).message}
              </p>
            ) : null}
            {search.data && search.data.length > 0 ? (
              <div className="mb-2">
                <p className="px-2 py-1 text-[11px] font-medium tracking-wide text-faint uppercase">
                  Documents
                </p>
                {search.data.map((d) => (
                  <button
                    key={d.id}
                    type="button"
                    onClick={() => go(`/documents/${d.id}`)}
                    className="flex w-full items-center gap-2 rounded-md px-2 py-2 text-left text-sm hover:bg-surface-2"
                  >
                    <FileText className="size-4 text-muted" />
                    <span className="flex-1 truncate">{d.title}</span>
                    <span className="text-xs text-faint">{d.correspondentName}</span>
                  </button>
                ))}
              </div>
            ) : null}
            <p className="px-2 py-1 text-[11px] font-medium tracking-wide text-faint uppercase">
              Navigation
            </p>
            {pages.map((p) => (
              <button
                key={p.to}
                type="button"
                onClick={() => go(p.to)}
                className="flex w-full items-center gap-2 rounded-md px-2 py-2 text-left text-sm hover:bg-surface-2"
              >
                <p.icon className="size-4 text-muted" />
                {p.label}
              </button>
            ))}
          </div>
        </DialogContent>
      </Dialog>
    </>
  );
}
