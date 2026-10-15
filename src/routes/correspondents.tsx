import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { createFileRoute, Link } from "@tanstack/react-router";
import { Pencil, Plus, Trash2, Users } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import { AppShell } from "@/components/layout/app-shell";
import { EntityForm, type EntityDraft } from "@/components/manage/entity-form";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { EmptyState } from "@/components/ui/empty-state";
import { deleteCorrespondent, fetchCatalog, upsertCorrespondent } from "@/lib/paperless/actions";
import { MATCHING_LABELS, type Correspondent } from "@/lib/paperless/types";

export const Route = createFileRoute("/correspondents")({ component: Page });

function Page() {
  const qc = useQueryClient();
  const catalog = useQuery({ queryKey: ["catalog"], queryFn: () => fetchCatalog() });
  const [editing, setEditing] = useState<Correspondent | null | "new">(null);
  const save = useMutation({
    mutationFn: (draft: EntityDraft) =>
      upsertCorrespondent({
        data: {
          id: editing && editing !== "new" ? editing.id : undefined,
          name: draft.name,
          match: draft.match,
          matchingAlgorithm: draft.matchingAlgorithm,
          notes: draft.notes,
        },
      }),
    onSuccess: () => {
      setEditing(null);
      qc.invalidateQueries();
      toast.success("Correspondant enregistré");
    },
  });
  const remove = useMutation({
    mutationFn: (id: number) => deleteCorrespondent({ data: { id } }),
    onSuccess: () => qc.invalidateQueries(),
  });

  const items = catalog.data?.correspondents ?? [];

  return (
    <AppShell
      title="Correspondants"
      actions={
        <Button size="sm" onClick={() => setEditing("new")}>
          <Plus className="size-4" />
          Nouveau
        </Button>
      }
    >
      {items.length === 0 && !catalog.isLoading ? (
        <EmptyState
          icon={Users}
          title="Aucun correspondant"
          description="Ajoutez des expéditeurs ou destinataires pour classer plus vite vos documents."
          action={
            <Button size="sm" onClick={() => setEditing("new")}>
              <Plus className="size-4" />
              Nouveau correspondant
            </Button>
          }
        />
      ) : (
        <div className="overflow-hidden rounded-xl bg-surface shadow-[var(--shadow-border)]">
          <table className="w-full text-sm">
            <thead className="text-[11px] tracking-wide text-muted uppercase">
              <tr className="border-b border-border">
                <th className="px-4 py-2 text-left">Nom</th>
                <th className="px-4 py-2 text-left">Correspondance</th>
                <th className="px-4 py-2 text-left">Motif</th>
                <th className="w-24 px-4 py-2" />
              </tr>
            </thead>
            <tbody>
              {items.map((c) => (
                <tr key={c.id} className="border-b border-border/70 last:border-0">
                  <td className="px-4 py-2 font-medium">
                    <Link
                      to="/documents"
                      search={{ correspondent: String(c.id) }}
                      className="text-primary hover:underline"
                    >
                      {c.name}
                    </Link>
                  </td>
                  <td className="px-4 py-2 text-muted">{MATCHING_LABELS[c.matchingAlgorithm]}</td>
                  <td className="px-4 py-2 font-mono text-xs">{c.match || "—"}</td>
                  <td className="px-4 py-2 text-right">
                    <Button
                      variant="ghost"
                      size="icon-sm"
                      aria-label={`Modifier ${c.name}`}
                      title={`Modifier ${c.name}`}
                      onClick={() => setEditing(c)}
                    >
                      <Pencil className="size-4" aria-hidden />
                    </Button>
                    <Button
                      variant="ghost"
                      size="icon-sm"
                      aria-label={`Supprimer ${c.name}`}
                      title={`Supprimer ${c.name}`}
                      onClick={() => remove.mutate(c.id)}
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
            <DialogTitle>{editing === "new" ? "Nouveau correspondant" : "Modifier"}</DialogTitle>
          </DialogHeader>
          <EntityForm
            kind="correspondent"
            initial={
              editing && editing !== "new"
                ? { ...editing, notes: editing.notes ?? undefined }
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
