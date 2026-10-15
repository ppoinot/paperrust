import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { createFileRoute } from "@tanstack/react-router";
import { Pencil, Plus, Trash2 } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import { AppShell } from "@/components/layout/app-shell";
import { EntityForm, type EntityDraft } from "@/components/manage/entity-form";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { deleteType, fetchCatalog, upsertType } from "@/lib/paperless/actions";
import { MATCHING_LABELS, type DocumentType } from "@/lib/paperless/types";

export const Route = createFileRoute("/types")({ component: Page });

function Page() {
  const qc = useQueryClient();
  const catalog = useQuery({ queryKey: ["catalog"], queryFn: () => fetchCatalog() });
  const [editing, setEditing] = useState<DocumentType | null | "new">(null);
  const save = useMutation({
    mutationFn: (draft: EntityDraft) =>
      upsertType({
        data: {
          id: editing && editing !== "new" ? editing.id : undefined,
          name: draft.name,
          match: draft.match,
          matchingAlgorithm: draft.matchingAlgorithm,
        },
      }),
    onSuccess: () => {
      setEditing(null);
      qc.invalidateQueries();
      toast.success("Type enregistré");
    },
  });
  const remove = useMutation({
    mutationFn: (id: number) => deleteType({ data: { id } }),
    onSuccess: () => qc.invalidateQueries(),
  });

  return (
    <AppShell
      title="Types de documents"
      actions={
        <Button size="sm" onClick={() => setEditing("new")}>
          <Plus className="size-4" />
          Ajouter
        </Button>
      }
    >
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
            {(catalog.data?.documentTypes ?? []).map((t) => (
              <tr key={t.id} className="border-b border-border/70 last:border-0">
                <td className="px-4 py-2 font-medium">{t.name}</td>
                <td className="px-4 py-2 text-muted">{MATCHING_LABELS[t.matchingAlgorithm]}</td>
                <td className="px-4 py-2 font-mono text-xs">{t.match || "—"}</td>
                <td className="px-4 py-2 text-right">
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    aria-label={`Modifier le type ${t.name}`}
                    title={`Modifier le type ${t.name}`}
                    onClick={() => setEditing(t)}
                  >
                    <Pencil className="size-4" aria-hidden />
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    aria-label={`Supprimer le type ${t.name}`}
                    title={`Supprimer le type ${t.name}`}
                    onClick={() => remove.mutate(t.id)}
                  >
                    <Trash2 className="size-4" aria-hidden />
                  </Button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <Dialog open={editing !== null} onOpenChange={(o) => !o && setEditing(null)}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{editing === "new" ? "Nouveau type" : "Modifier"}</DialogTitle>
          </DialogHeader>
          <EntityForm
            kind="type"
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
