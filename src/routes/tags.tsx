import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { createFileRoute } from "@tanstack/react-router";
import { Pencil, Plus, Trash2 } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import { AppShell } from "@/components/layout/app-shell";
import { EntityForm, type EntityDraft } from "@/components/manage/entity-form";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { deleteTag, fetchCatalog, upsertTag } from "@/lib/paperless/actions";
import { dedupeTagsByName, pickByNameCI, sortTagsForGrid } from "@/lib/paperless/format";
import { MATCHING_LABELS, type Tag } from "@/lib/paperless/types";

export const Route = createFileRoute("/tags")({ component: TagsPage });

function TagsPage() {
  const qc = useQueryClient();
  const catalog = useQuery({ queryKey: ["catalog"], queryFn: () => fetchCatalog() });
  const [editing, setEditing] = useState<Tag | null | "new">(null);
  const save = useMutation({
    mutationFn: (draft: EntityDraft) => {
      const currentId = editing && editing !== "new" ? editing.id : undefined;
      const dup = pickByNameCI(catalog.data?.tags ?? [], draft.name);
      if (dup && dup.id !== currentId) {
        throw new Error("Cette étiquette existe déjà");
      }
      return upsertTag({
        data: {
          id: currentId,
          name: draft.name,
          color: draft.color ?? "#57534e",
          isInboxTag: Boolean(draft.isInboxTag),
          match: draft.match,
          matchingAlgorithm: draft.matchingAlgorithm,
        },
      });
    },
    onSuccess: () => {
      setEditing(null);
      qc.invalidateQueries();
      toast.success("Étiquette enregistrée");
    },
    onError: (e: Error) => toast.error(e.message),
  });
  const remove = useMutation({
    mutationFn: (id: number) => deleteTag({ data: { id } }),
    onSuccess: () => {
      qc.invalidateQueries();
      toast.success("Supprimée");
    },
  });

  return (
    <AppShell
      title="Étiquettes"
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
            {sortTagsForGrid(dedupeTagsByName(catalog.data?.tags ?? [])).map((t) => (
              <tr key={t.id} className="border-b border-border/70 last:border-0">
                <td className="px-4 py-2">
                  <Badge color={t.color}>{t.name}</Badge>
                  {t.isInboxTag && t.name.trim().toLowerCase() !== "inbox" ? (
                    <span className="ml-2 text-[11px] text-muted">inbox</span>
                  ) : null}
                </td>
                <td className="px-4 py-2 text-muted">{MATCHING_LABELS[t.matchingAlgorithm]}</td>
                <td className="px-4 py-2 font-mono text-xs">{t.match || "—"}</td>
                <td className="px-4 py-2 text-right">
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    aria-label={`Modifier l'étiquette ${t.name}`}
                    title={`Modifier l'étiquette ${t.name}`}
                    onClick={() => setEditing(t)}
                  >
                    <Pencil className="size-4" aria-hidden />
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    aria-label={`Supprimer l'étiquette ${t.name}`}
                    title={`Supprimer l'étiquette ${t.name}`}
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
            <DialogTitle>{editing === "new" ? "Nouvelle étiquette" : "Modifier l'étiquette"}</DialogTitle>
          </DialogHeader>
          <EntityForm
            kind="tag"
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
