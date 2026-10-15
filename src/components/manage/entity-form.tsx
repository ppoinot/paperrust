import { MATCHING_LABELS, type MatchingAlgorithm } from "@/lib/paperless/types";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import { useState } from "react";

export type EntityDraft = {
  name: string;
  match: string;
  matchingAlgorithm: MatchingAlgorithm;
  color?: string;
  isInboxTag?: boolean;
  path?: string;
  notes?: string;
  dataType?: string;
};

export function EntityForm({
  initial,
  onSubmit,
  onCancel,
  kind,
  pending,
}: {
  initial?: Partial<EntityDraft>;
  onSubmit: (draft: EntityDraft) => void;
  onCancel: () => void;
  kind: "tag" | "correspondent" | "type" | "path" | "field";
  pending?: boolean;
}) {
  const [draft, setDraft] = useState<EntityDraft>({
    name: initial?.name ?? "",
    match: initial?.match ?? "",
    matchingAlgorithm: (initial?.matchingAlgorithm as MatchingAlgorithm) ?? "auto",
    color: initial?.color ?? "#3d5a4c",
    isInboxTag: initial?.isInboxTag ?? false,
    path: initial?.path ?? "{created_year}/{correspondent}/{title}",
    notes: initial?.notes ?? "",
    dataType: initial?.dataType ?? "string",
  });

  return (
    <form
      className="space-y-3"
      onSubmit={(e) => {
        e.preventDefault();
        onSubmit(draft);
      }}
    >
      <div>
        <Label>Nom</Label>
        <Input className="mt-1" value={draft.name} onChange={(e) => setDraft({ ...draft, name: e.target.value })} required />
      </div>
      {kind === "tag" ? (
        <div className="flex items-center gap-4">
          <div>
            <Label>Couleur</Label>
            <input
              type="color"
              className="mt-1 h-10 w-14 cursor-pointer rounded-md bg-transparent"
              value={draft.color}
              onChange={(e) => setDraft({ ...draft, color: e.target.value })}
            />
          </div>
          <label className="mt-5 flex items-center gap-2 text-sm">
            <Switch checked={draft.isInboxTag} onCheckedChange={(v) => setDraft({ ...draft, isInboxTag: v })} />
            Étiquette Inbox
          </label>
        </div>
      ) : null}
      {kind === "path" ? (
        <div>
          <Label>Modèle de chemin</Label>
          <Input className="mt-1 font-mono text-[13px]" value={draft.path} onChange={(e) => setDraft({ ...draft, path: e.target.value })} />
        </div>
      ) : null}
      {kind === "field" ? (
        <div>
          <Label>Type de données</Label>
          <Select value={draft.dataType} onValueChange={(v) => setDraft({ ...draft, dataType: v })}>
            <SelectTrigger className="mt-1">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {["string", "boolean", "date", "integer", "float", "monetary", "url"].map((t) => (
                <SelectItem key={t} value={t}>
                  {t}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
      ) : null}
      {kind !== "field" ? (
        <>
          <div>
            <Label>Algorithme de correspondance</Label>
            <Select
              value={draft.matchingAlgorithm}
              onValueChange={(v) => setDraft({ ...draft, matchingAlgorithm: v as MatchingAlgorithm })}
            >
              <SelectTrigger className="mt-1">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {Object.entries(MATCHING_LABELS).map(([k, v]) => (
                  <SelectItem key={k} value={k}>
                    {v}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          <div>
            <Label>Motif</Label>
            <Input className="mt-1" value={draft.match} onChange={(e) => setDraft({ ...draft, match: e.target.value })} />
          </div>
        </>
      ) : null}
      {kind === "correspondent" ? (
        <div>
          <Label>Notes</Label>
          <Textarea className="mt-1" value={draft.notes} onChange={(e) => setDraft({ ...draft, notes: e.target.value })} />
        </div>
      ) : null}
      <div className="flex justify-end gap-2 pt-2">
        <Button type="button" variant="ghost" onClick={onCancel}>
          Annuler
        </Button>
        <Button type="submit" disabled={pending || !draft.name.trim()}>
          Enregistrer
        </Button>
      </div>
    </form>
  );
}
