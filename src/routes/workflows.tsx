import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { createFileRoute } from "@tanstack/react-router";
import { GitBranch, Plus, Trash2 } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import { AppShell } from "@/components/layout/app-shell";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { EmptyState } from "@/components/ui/empty-state";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { deleteWorkflow, fetchCatalog, upsertWorkflow } from "@/lib/paperless/actions";

export const Route = createFileRoute("/workflows")({ component: Page });

function Page() {
  const qc = useQueryClient();
  const catalog = useQuery({ queryKey: ["catalog"], queryFn: () => fetchCatalog() });
  const [open, setOpen] = useState(false);
  const [name, setName] = useState("");
  const [trigger, setTrigger] = useState("document_added");
  const [contains, setContains] = useState("");
  const [action, setAction] = useState("add_tag");
  const [tagId, setTagId] = useState("");

  const save = useMutation({
    mutationFn: () =>
      upsertWorkflow({
        data: {
          name,
          enabled: true,
          triggerType: trigger,
          filters: contains ? { contentContains: contains } : {},
          actions: tagId ? [{ type: action, tagId: Number(tagId) }] : [],
        },
      }),
    onSuccess: () => {
      setOpen(false);
      setName("");
      qc.invalidateQueries();
      toast.success("Workflow enregistré");
    },
  });
  const remove = useMutation({
    mutationFn: (id: number) => deleteWorkflow({ data: { id } }),
    onSuccess: () => qc.invalidateQueries(),
  });
  const toggle = useMutation({
    mutationFn: (wf: {
      id: number;
      name: string;
      enabled: boolean;
      triggerType: string;
      filters: { contentContains?: string; hasCorrespondent?: boolean };
      actions: unknown[];
    }) =>
      upsertWorkflow({
        data: {
          id: wf.id,
          name: wf.name,
          enabled: !wf.enabled,
          triggerType: wf.triggerType,
          filters: wf.filters,
          actions: wf.actions,
        },
      }),
    onSuccess: () => qc.invalidateQueries(),
  });

  const items = catalog.data?.workflows ?? [];

  return (
    <AppShell
      title="Workflows"
      actions={
        <Button size="sm" onClick={() => setOpen(true)}>
          <Plus className="size-4" />
          Nouveau
        </Button>
      }
    >
      <p className="mb-4 max-w-2xl text-sm text-muted">
        Déclenchés à la consommation ou à la mise à jour : ajouter / retirer une étiquette, assigner un correspondant.
      </p>
      {items.length === 0 && !catalog.isLoading ? (
        <EmptyState
          icon={GitBranch}
          title="Aucun workflow"
          description="Automatisez le classement : à l'ajout d'un document, appliquez des étiquettes selon le contenu."
          action={
            <Button size="sm" onClick={() => setOpen(true)}>
              <Plus className="size-4" />
              Créer un workflow
            </Button>
          }
        />
      ) : (
        <div className="space-y-3">
          {items.map((wf) => (
            <div key={wf.id} className="flex items-center gap-3 rounded-xl bg-surface p-4 shadow-[var(--shadow-border)]">
              <Switch checked={wf.enabled} onCheckedChange={() => toggle.mutate(wf)} />
              <div className="min-w-0 flex-1">
                <p className="font-medium">{wf.name}</p>
                <p className="truncate text-xs text-muted">
                  {wf.triggerType} · {wf.actions.length} action{wf.actions.length > 1 ? "s" : ""}
                </p>
              </div>
              <Button
                variant="ghost"
                size="icon-sm"
                aria-label={`Supprimer le workflow ${wf.name}`}
                title={`Supprimer le workflow ${wf.name}`}
                onClick={() => remove.mutate(wf.id)}
              >
                <Trash2 className="size-4" aria-hidden />
              </Button>
            </div>
          ))}
        </div>
      )}
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Nouveau workflow</DialogTitle>
          </DialogHeader>
          <div className="space-y-3">
            <div>
              <Label>Nom</Label>
              <Input className="mt-1" value={name} onChange={(e) => setName(e.target.value)} />
            </div>
            <div>
              <Label>Déclencheur</Label>
              <Select value={trigger} onValueChange={setTrigger}>
                <SelectTrigger className="mt-1">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="document_added">Document ajouté</SelectItem>
                  <SelectItem value="document_updated">Document mis à jour</SelectItem>
                  <SelectItem value="consumption">Consommation</SelectItem>
                </SelectContent>
              </Select>
            </div>
            <div>
              <Label>Le contenu contient</Label>
              <Input className="mt-1" value={contains} onChange={(e) => setContains(e.target.value)} />
            </div>
            <div>
              <Label>Action</Label>
              <Select value={action} onValueChange={setAction}>
                <SelectTrigger className="mt-1">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="add_tag">Ajouter une étiquette</SelectItem>
                  <SelectItem value="remove_tag">Retirer une étiquette</SelectItem>
                </SelectContent>
              </Select>
            </div>
            <div>
              <Label>Étiquette</Label>
              <Select value={tagId} onValueChange={setTagId}>
                <SelectTrigger className="mt-1">
                  <SelectValue placeholder="Choisir" />
                </SelectTrigger>
                <SelectContent>
                  {(catalog.data?.tags ?? []).map((t) => (
                    <SelectItem key={t.id} value={String(t.id)}>
                      {t.name}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
            <div className="flex justify-end gap-2">
              <Button variant="ghost" onClick={() => setOpen(false)}>
                Annuler
              </Button>
              <Button disabled={!name.trim() || save.isPending} onClick={() => save.mutate()}>
                Créer
              </Button>
            </div>
          </div>
        </DialogContent>
      </Dialog>
    </AppShell>
  );
}
