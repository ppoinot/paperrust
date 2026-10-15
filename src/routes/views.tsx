import { useQuery } from "@tanstack/react-query";
import { createFileRoute, Link } from "@tanstack/react-router";
import { Trash2 } from "lucide-react";
import { AppShell } from "@/components/layout/app-shell";
import { Button } from "@/components/ui/button";
import { fetchCatalog } from "@/lib/paperless/actions";
import { useDeleteSavedView } from "@/lib/paperless/use-delete-saved-view";

export const Route = createFileRoute("/views")({ component: Page });

function Page() {
  const catalog = useQuery({ queryKey: ["catalog"], queryFn: () => fetchCatalog() });
  const deleteSavedView = useDeleteSavedView();

  return (
    <AppShell title="Vues enregistrées">
      <p className="mb-4 max-w-2xl text-sm text-muted">
        Une vue fige filtres et tri. Créez-en une depuis la liste des documents (« Sauver la vue »).
      </p>
      <div className="grid gap-3 md:grid-cols-2">
        {(catalog.data?.savedViews ?? []).map((v) => (
          <div key={v.id} className="rounded-xl bg-surface p-4 shadow-[var(--shadow-border)]">
            <div className="flex items-start justify-between gap-3">
              <div>
                <p className="font-medium">{v.name}</p>
                <p className="mt-1 text-xs text-muted">
                  Tri {v.sortField} {v.sortReverse ? "↓" : "↑"}
                  {v.showInSidebar ? " · barre latérale" : ""}
                  {v.showOnDashboard ? " · tableau de bord" : ""}
                </p>
              </div>
              <Button
                variant="ghost"
                size="icon-sm"
                aria-label={`Supprimer la vue ${v.name}`}
                title={`Supprimer la vue ${v.name}`}
                onClick={() => void deleteSavedView({ id: v.id, name: v.name })}
              >
                <Trash2 className="size-4" />
              </Button>
            </div>
            <Link
              to="/documents"
              search={{ view: String(v.id) }}
              className="mt-3 inline-block text-sm text-primary hover:underline"
            >
              Ouvrir
            </Link>
          </div>
        ))}
      </div>
    </AppShell>
  );
}
