import { Link, useRouter, useRouterState } from "@tanstack/react-router";
import { Trash2 } from "lucide-react";
import { BrandMark } from "./brand-mark";
import { AUTOMATION, MANAGE, NAV } from "./nav-config";
import { logoutOidc as logout } from "@/lib/auth/oidc";
import { useSessionIdentity } from "@/lib/auth/session-identity";
import { useDeleteSavedView } from "@/lib/paperless/use-delete-saved-view";
import { cn } from "@/lib/utils";
import type { Catalog, SavedView } from "@/lib/paperless/types";

function Item({
  to,
  label,
  icon: Icon,
  badge,
  exact,
  onNavigate,
}: {
  to: string;
  label: string;
  icon: (typeof NAV)[0]["icon"];
  badge?: number;
  exact?: boolean;
  onNavigate?: () => void;
}) {
  const router = useRouter();
  const pathname = useRouterState({ select: (s) => s.location.pathname });
  const pathOnly = to.split("?")[0] ?? to;
  const active = exact ? pathname === pathOnly : pathname === pathOnly || pathname.startsWith(`${pathOnly}/`);
  return (
    <a
      href={to}
      onClick={(e) => {
        e.preventDefault();
        onNavigate?.();
        router.history.push(to);
      }}
      className={cn(
        "flex h-10 items-center gap-2.5 rounded-md px-2.5 text-sm text-sidebar-muted transition-colors hover:bg-sidebar-hover hover:text-sidebar-fg",
        active && "bg-sidebar-active text-primary-fg hover:bg-sidebar-active hover:text-primary-fg",
      )}
    >
      <Icon className="size-4 shrink-0" strokeWidth={1.75} />
      <span className="flex-1 truncate">{label}</span>
      {typeof badge === "number" && badge > 0 ? (
        <span className="tabular min-w-5 rounded-full bg-primary-fg/15 px-1.5 text-center text-[11px]">
          {badge}
        </span>
      ) : null}
    </a>
  );
}

function SavedViewItem({
  view,
  onNavigate,
}: {
  view: SavedView;
  onNavigate?: () => void;
}) {
  const router = useRouter();
  const pathname = useRouterState({ select: (s) => s.location.pathname });
  const search = useRouterState({ select: (s) => s.location.search });
  const deleteView = useDeleteSavedView();
  const to = `/documents?view=${view.id}`;
  const searchView =
    search && typeof search === "object" && "view" in search
      ? String((search as { view?: string }).view ?? "")
      : "";
  const active = pathname === "/documents" && searchView === String(view.id);
  const Icon = NAV[2]!.icon;

  return (
    <div
      className={cn(
        "group flex h-10 items-center gap-0.5 rounded-md pr-0.5 text-sm text-sidebar-muted transition-colors hover:bg-sidebar-hover hover:text-sidebar-fg",
        active && "bg-sidebar-active text-primary-fg hover:bg-sidebar-active hover:text-primary-fg",
      )}
    >
      <a
        href={to}
        onClick={(e) => {
          e.preventDefault();
          onNavigate?.();
          router.history.push(to);
        }}
        className="flex min-w-0 flex-1 items-center gap-2.5 px-2.5"
      >
        <Icon className="size-4 shrink-0" strokeWidth={1.75} />
        <span className="truncate">{view.name}</span>
      </a>
      <button
        type="button"
        aria-label={`Supprimer la vue ${view.name}`}
        title={`Supprimer la vue ${view.name}`}
        className={cn(
          "shrink-0 rounded-md p-1.5 text-sidebar-muted opacity-100 hover:bg-sidebar-hover hover:text-sidebar-fg md:opacity-0 md:group-hover:opacity-100 md:group-focus-within:opacity-100",
          active && "text-primary-fg/80 hover:bg-primary-fg/10 hover:text-primary-fg",
        )}
        onClick={(e) => {
          e.preventDefault();
          e.stopPropagation();
          void deleteView({ id: view.id, name: view.name });
        }}
      >
        <Trash2 className="size-3.5" strokeWidth={1.75} />
      </button>
    </div>
  );
}

function IdentityFooter() {
  const { data: me, isPending } = useSessionIdentity();
  if (isPending) {
    return <p className="truncate text-sidebar-muted">…</p>;
  }
  if (me) {
    const label = me.name || me.email || "Connecté";
    return (
      <div className="flex items-start justify-between gap-2">
        <div className="min-w-0">
          <p className="truncate text-sidebar-fg">{label}</p>
          {me.name && me.email ? <p className="truncate">{me.email}</p> : null}
        </div>
        <button
          type="button"
          onClick={() => logout()}
          className="shrink-0 text-sidebar-muted hover:text-sidebar-fg hover:underline"
        >
          Déconnexion
        </button>
      </div>
    );
  }
  return (
    <div className="flex items-center justify-between gap-2">
      <span>Non connecté</span>
      <Link to="/login" className="text-sidebar-fg hover:underline">
        Connexion
      </Link>
    </div>
  );
}

export function SidebarNav({
  catalog,
  inbox,
  onNavigate,
}: {
  catalog?: Catalog;
  inbox?: number;
  onNavigate?: () => void;
}) {
  const views = catalog?.savedViews.filter((v) => v.showInSidebar) ?? [];
  return (
    <div className="flex h-full flex-col">
      <div className="flex items-center gap-2.5 px-4 pt-5 pb-4">
        <div className="flex size-8 items-center justify-center rounded-md bg-primary-fg/10 text-primary-fg">
          <BrandMark className="size-6" />
        </div>
        <div>
          <p className="font-display text-lg leading-none text-sidebar-fg">Paperrust</p>
          <p className="mt-0.5 text-[11px] tracking-wide text-sidebar-muted uppercase">Archive</p>
        </div>
      </div>
      <nav className="flex-1 space-y-5 overflow-y-auto px-3 pb-4">
        <div className="space-y-0.5">
          {NAV.map((item) => (
            <Item
              key={item.to}
              {...item}
              badge={item.badge === "inbox" ? inbox : undefined}
              onNavigate={onNavigate}
            />
          ))}
        </div>
        <div>
          <p className="px-2.5 pb-1.5 text-[11px] font-medium tracking-[0.14em] text-sidebar-muted uppercase">
            Classement
          </p>
          <div className="space-y-0.5">
            {MANAGE.map((item) => (
              <Item key={item.to} {...item} onNavigate={onNavigate} />
            ))}
          </div>
        </div>
        {views.length > 0 ? (
          <div>
            <p className="px-2.5 pb-1.5 text-[11px] font-medium tracking-[0.14em] text-sidebar-muted uppercase">
              Vues
            </p>
            <div className="space-y-0.5">
              {views.map((v) => (
                <SavedViewItem key={v.id} view={v} onNavigate={onNavigate} />
              ))}
            </div>
          </div>
        ) : null}
        <div>
          <p className="px-2.5 pb-1.5 text-[11px] font-medium tracking-[0.14em] text-sidebar-muted uppercase">
            Automatisation
          </p>
          <div className="space-y-0.5">
            {AUTOMATION.map((item) => (
              <Item key={item.to} {...item} onNavigate={onNavigate} />
            ))}
          </div>
        </div>
      </nav>
      <div className="border-t border-white/8 px-4 py-3 text-[11px] text-sidebar-muted">
        <IdentityFooter />
        <p className="mt-1.5">OCR LLM · Postgres</p>
      </div>
    </div>
  );
}
