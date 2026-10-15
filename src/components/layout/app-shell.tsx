import { useQuery } from "@tanstack/react-query";
import { Menu } from "lucide-react";
import { type ReactNode, useState } from "react";
import { CommandPalette } from "./command-palette";
import { SidebarNav } from "./sidebar";
import { Button } from "@/components/ui/button";
import { Sheet, SheetContent } from "@/components/ui/sheet";
import { fetchDashboard } from "@/lib/paperless/actions";
import { useCatalog } from "@/lib/paperless/use-catalog";

export function AppShell({ children, title, actions }: { children: ReactNode; title?: string; actions?: ReactNode }) {
  const [open, setOpen] = useState(false);
  const catalog = useCatalog();
  const dash = useQuery({ queryKey: ["dashboard"], queryFn: () => fetchDashboard() });
  const inbox = dash.data?.inbox ?? 0;

  return (
    <div className="flex min-h-dvh bg-bg text-fg">
      <aside className="sticky top-0 hidden h-dvh w-60 shrink-0 bg-sidebar md:block">
        <SidebarNav catalog={catalog.data} inbox={inbox} />
      </aside>
      <Sheet open={open} onOpenChange={setOpen}>
        <SheetContent side="left" className="p-0">
          <SidebarNav catalog={catalog.data} inbox={inbox} onNavigate={() => setOpen(false)} />
        </SheetContent>
      </Sheet>
      <div className="flex min-w-0 flex-1 flex-col">
        <header className="sticky top-0 z-30 flex h-14 items-center gap-3 border-b border-border bg-bg/90 px-4 backdrop-blur-sm">
          <Button
            variant="ghost"
            size="icon"
            className="md:hidden"
            onClick={() => setOpen(true)}
            aria-label="Menu"
          >
            <Menu className="size-5" />
          </Button>
          <div className="min-w-0 flex-1">
            {title ? (
              <h1 title={title} className="truncate font-display text-lg font-medium md:text-xl">{title}</h1>
            ) : (
              <span className="font-display text-lg md:hidden">Paperrust</span>
            )}
          </div>
          <CommandPalette />
          {actions}
        </header>
        <main className="mx-auto w-full min-w-0 max-w-[1400px] flex-1 overflow-x-hidden px-4 py-5 md:px-6 md:py-6">{children}</main>
      </div>
    </div>
  );
}
