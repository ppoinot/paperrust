import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { createFileRoute } from "@tanstack/react-router";
import { ListChecks, Pencil, Plus, Trash2 } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import { AppShell } from "@/components/layout/app-shell";
import { EntityForm, type EntityDraft } from "@/components/manage/entity-form";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { EmptyState } from "@/components/ui/empty-state";
import { deleteField, fetchCatalog, upsertField } from "@/lib/paperless/actions";
import type { CustomField } from "@/lib/paperless/types";

export const Route = createFileRoute("/fields")({ component: Page });

function Page() {
  const qc = useQueryClient();
  const catalog = useQuery({ queryKey: ["catalog"], queryFn: () => fetchCatalog() });
  const [editing, setEditing] = useState<CustomField | null | "new">(null);
  const save = useMutation({
    mutationFn: (draft: EntityDraft) =>
      upsertField({
        data: {
          id: editing && editing !== "new" ? editing.id : undefined,
          name: draft.name,
          dataType: draft.dataType ?? "string",
        },
      }),
    onSuccess: () => {
      setEditing(null);
      qc.invalidateQueries();
      toast.success("Champ enregistré");
    },
  });
  const remove = useMutation({
    mutationFn: (id: number) => deleteField({ data: { id } }),
    onSuccess: () => qc.invalidateQueries(),
  });

  const items = catalog.data?.customFields ?? [];

  return (
    <AppShell
      title="Champs personnalisés"
      actions={
        <Button size="sm" onClick={() => setEditing("new")}>
          <Plus className="size-4" />
          Nouveau
        </Button>
      }
    >
      {items.length === 0 && !catalog.isLoading ? (
        <EmptyState
          icon={ListChecks}
          title="Aucun champ personnalisé"
          description="Créez des champs (référence, montant, échéance…) pour enrichir vos fiches documents."
          action={
            <Button size="sm" onClick={() => setEditing("new")}>
              <Plus className="size-4" />
              Nouveau champ
            </Button>
          }
        />
      ) : (
        <div className="overflow-hidden rounded-xl bg-surface shadow-[var(--shadow-border)]">
          <table className="w-full text-sm">
            <thead className="text-[11px] tracking-wide text-muted uppercase">
              <tr className="border-b border-border">
                <th className="px-4 py-2 text-left">Nom</th>
                <th className="px-4 py-2 text-left">Type</th>
                <th className="w-24 px-4 py-2" />
              </tr>
            </thead>
            <tbody>
              {items.map((f) => (
                <tr key={f.id} className="border-b border-border/70 last:border-0">
                  <td className="px-4 py-2 font-medium">{f.name}</td>
                  <td className="px-4 py-2 font-mono text-xs text-muted">{f.dataType}</td>
                  <td className="px-4 py-2 text-right">
                    <Button
                      variant="ghost"
                      size="icon-sm"
                      aria-label={`Modifier ${f.name}`}
                      title={`Modifier ${f.name}`}
                      onClick={() => setEditing(f)}
                    >
                      <Pencil className="size-4" aria-hidden />
                    </Button>
                    <Button
                      variant="ghost"
                      size="icon-sm"
                      aria-label={`Supprimer ${f.name}`}
                      title={`Supprimer ${f.name}`}
                      onClick={() => remove.mutate(f.id)}
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
            <DialogTitle>{editing === "new" ? "Nouveau champ" : "Modifier"}</DialogTitle>
          </DialogHeader>
          <EntityForm
            kind="field"
            initial={
              editing && editing !== "new"
                ? { name: editing.name, dataType: editing.dataType, match: "", matchingAlgorithm: "none" }
                : undefined
            }
            pending={save.isPending}
            onCancel={() => setEditing(null)}
            onSubmit={(d) => save.mutate(d)}
          />
        </DialogContent>
      </Dialog>
    </AppShell>
  );
}
