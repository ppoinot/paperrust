import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { createFileRoute } from "@tanstack/react-router";
import { FolderTree, Pencil, Plus, Trash2 } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import { AppShell } from "@/components/layout/app-shell";
import { EntityForm, type EntityDraft } from "@/components/manage/entity-form";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { EmptyState } from "@/components/ui/empty-state";
import { deletePath, fetchCatalog, upsertPath } from "@/lib/paperless/actions";
import { MATCHING_LABELS, type StoragePath } from "@/lib/paperless/types";

export const Route = createFileRoute("/storage-paths")({ component: Page });

function Page() {
  const qc = useQueryClient();
  const catalog = useQuery({ queryKey: ["catalog"], queryFn: () => fetchCatalog() });
  const [editing, setEditing] = useState<StoragePath | null | "new">(null);
  const save = useMutation({
    mutationFn: (draft: EntityDraft) =>
      upsertPath({
        data: {
          id: editing && editing !== "new" ? editing.id : undefined,
          name: draft.name,
          path: draft.path ?? draft.name,
          match: draft.match,
          matchingAlgorithm: draft.matchingAlgorithm,
        },
      }),
    onSuccess: () => {
      setEditing(null);
      qc.invalidateQueries();
      toast.success("Chemin enregistré");
    },
  });
  const remove = useMutation({
    mutationFn: (id: number) => deletePath({ data: { id } }),
    onSuccess: () => qc.invalidateQueries(),
  });

  const items = catalog.data?.storagePaths ?? [];

  return (
    <AppShell
      title="Chemins de stockage"
      actions={
        <Button size="sm" onClick={() => setEditing("new")}>
          <Plus className="size-4" />
          Nouveau
        </Button>
      }
    >
      <p className="mb-4 max-w-2xl text-sm text-muted">
        Modèles du type <code className="font-mono text-xs">{"{created_year}/{correspondent}/{title}"}</code> — comme dans Paperless.
      </p>
      {items.length === 0 && !catalog.isLoading ? (
        <EmptyState
          icon={FolderTree}
          title="Aucun chemin de stockage"
          description="Définissez des modèles de chemins pour organiser l'archivage logique des documents."
          action={
            <Button size="sm" onClick={() => setEditing("new")}>
              <Plus className="size-4" />
              Nouveau chemin
            </Button>
          }
        />
      ) : (
        <div className="overflow-hidden rounded-xl bg-surface shadow-[var(--shadow-border)]">
          <table className="w-full text-sm">
            <thead className="text-[11px] tracking-wide text-muted uppercase">
              <tr className="border-b border-border">
                <th className="px-4 py-2 text-left">Nom</th>
                <th className="px-4 py-2 text-left">Modèle</th>
                <th className="px-4 py-2 text-left">Correspondance</th>
                <th className="w-24 px-4 py-2" />
              </tr>
            </thead>
            <tbody>
              {items.map((p) => (
                <tr key={p.id} className="border-b border-border/70 last:border-0">
                  <td className="px-4 py-2 font-medium">{p.name}</td>
                  <td className="px-4 py-2 font-mono text-xs">{p.path}</td>
                  <td className="px-4 py-2 text-muted">{MATCHING_LABELS[p.matchingAlgorithm]}</td>
                  <td className="px-4 py-2 text-right">
                    <Button
                      variant="ghost"
                      size="icon-sm"
                      aria-label={`Modifier ${p.name}`}
                      title={`Modifier ${p.name}`}
                      onClick={() => setEditing(p)}
                    >
                      <Pencil className="size-4" aria-hidden />
                    </Button>
                    <Button
                      variant="ghost"
                      size="icon-sm"
                      aria-label={`Supprimer ${p.name}`}
                      title={`Supprimer ${p.name}`}
                      onClick={() => remove.mutate(p.id)}
                    >
                      <Trash2 className="size-4" aria-hidden />
                    </Button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      <Dialog open={editing !== null} onOpenChange={(o) => !o && setEditing(null)}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{editing === "new" ? "Nouveau chemin" : "Modifier"}</DialogTitle>
          </DialogHeader>
          <EntityForm
            kind="path"
            initial={editing && editing !== "new" ? editing : undefined}
            pending={save.isPending}
            onCancel={() => setEditing(null)}
            onSubmit={(d) => save.mutate(d)}
          />
        </DialogContent>
      </Dialog>
    </AppShell>
  );
}
