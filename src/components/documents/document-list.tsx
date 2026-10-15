import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate } from "@tanstack/react-router";
import {
  Bookmark,
  LayoutGrid,
  List as ListIcon,
  MoreHorizontal,
  RotateCcw,
  Trash2,
} from "lucide-react";
import { useMemo, useState } from "react";
import { toast } from "sonner";
import { TagChip } from "./tag-chip";
import { DocThumb } from "./thumb";
import { UploadDropzone } from "./upload-dropzone";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { bulkUpdateDocuments, fetchCatalog, fetchDocuments, upsertView } from "@/lib/paperless/actions";
import { isAuthRequired } from "@/lib/auth/auth-required";
import { asnLabel, dedupeTagsByName, formatDate, ocrStatusLabel, sortTagsForGrid } from "@/lib/paperless/format";
import type { DocumentFilter } from "@/lib/paperless/types";
import { cn } from "@/lib/utils";

export function DocumentList({ initialFilter }: { initialFilter?: DocumentFilter }) {
  const [filter, setFilter] = useState<DocumentFilter>({
    sort: "created_date",
    sortReverse: true,
    ...initialFilter,
  });
  const [q, setQ] = useState(initialFilter?.q ?? "");
  const [mode, setMode] = useState<"table" | "cards">("table");
  const [selected, setSelected] = useState<number[]>([]);
  const [saveOpen, setSaveOpen] = useState(false);
  const [viewName, setViewName] = useState("");
  const navigate = useNavigate();
  const qc = useQueryClient();
  const trashMode = filter.trashOnly === true;

  const catalog = useQuery({ queryKey: ["catalog"], queryFn: () => fetchCatalog() });
  const docs = useQuery({
    queryKey: ["documents", filter],
    queryFn: () => fetchDocuments({ data: filter }),
  });

  const items = docs.data ?? [];
  const allIds = items.map((d) => d.id);
  const allSelected = allIds.length > 0 && allIds.every((id) => selected.includes(id));

  const bulk = useMutation({
    mutationFn: (input: Parameters<typeof bulkUpdateDocuments>[0]["data"]) =>
      bulkUpdateDocuments({ data: input }),
    onSuccess: () => {
      setSelected([]);
      qc.invalidateQueries();
      toast.success("Modification enregistrée");
    },
  });

  const saveView = useMutation({
    mutationFn: () =>
      upsertView({
        data: {
          name: viewName,
          showOnDashboard: false,
          showInSidebar: true,
          sortField: filter.sort ?? "created_date",
          sortReverse: filter.sortReverse !== false,
          filter,
        },
      }),
    onSuccess: () => {
      setSaveOpen(false);
      qc.invalidateQueries({ queryKey: ["catalog"] });
      toast.success("Vue enregistrée");
    },
  });

  const tags = useMemo(
    () => sortTagsForGrid(dedupeTagsByName(catalog.data?.tags ?? []), filter.tagIds),
    [catalog.data?.tags, filter.tagIds],
  );
  const correspondents = catalog.data?.correspondents ?? [];
  const types = catalog.data?.documentTypes ?? [];

  const activeTag = useMemo(
    () => tags.filter((t) => filter.tagIds?.includes(t.id)),
    [tags, filter.tagIds],
  );

  function toggleTag(id: number) {
    const current = filter.tagIds ?? [];
    const next = current.includes(id) ? current.filter((x) => x !== id) : [...current, id];
    setFilter({ ...filter, tagIds: next });
  }

  return (
    <div className="min-w-0 space-y-4">
      {trashMode ? null : <UploadDropzone compact />}
      <div className="flex min-w-0 flex-col flex-wrap gap-3 xl:flex-row">
        <Input
          value={q}
          placeholder="Titre, contenu, correspondant…"
          onChange={(e) => setQ(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") setFilter({ ...filter, q });
          }}
          className="lg:max-w-sm"
        />
        <Select
          value={filter.correspondentId ? String(filter.correspondentId) : "all"}
          onValueChange={(v) =>
            setFilter({ ...filter, correspondentId: v === "all" ? undefined : Number(v) })
          }
        >
          <SelectTrigger className="lg:w-52">
            <SelectValue placeholder="Correspondant" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="all">Tous les correspondants</SelectItem>
            {correspondents.map((c) => (
              <SelectItem key={c.id} value={String(c.id)}>
                {c.name}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <Select
          value={filter.documentTypeId ? String(filter.documentTypeId) : "all"}
          onValueChange={(v) =>
            setFilter({ ...filter, documentTypeId: v === "all" ? undefined : Number(v) })
          }
        >
          <SelectTrigger className="lg:w-44">
            <SelectValue placeholder="Type" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="all">Tous les types</SelectItem>
            {types.map((t) => (
              <SelectItem key={t.id} value={String(t.id)}>
                {t.name}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <Select
          value={filter.sort ?? "created_date"}
          onValueChange={(v) => setFilter({ ...filter, sort: v })}
        >
          <SelectTrigger className="lg:w-40">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="created_date">Date du document</SelectItem>
            <SelectItem value="added_at">Date d'ajout</SelectItem>
            <SelectItem value="title">Titre</SelectItem>
            <SelectItem value="correspondent">Correspondant</SelectItem>
            <SelectItem value="asn">ASN</SelectItem>
          </SelectContent>
        </Select>
        <div className="ml-auto flex items-center gap-1">
          <Button
            variant={mode === "table" ? "secondary" : "ghost"}
            size="icon"
            aria-label="Vue liste"
            title="Vue liste"
            aria-pressed={mode === "table"}
            onClick={() => setMode("table")}
          >
            <ListIcon className="size-4" aria-hidden />
          </Button>
          <Button
            variant={mode === "cards" ? "secondary" : "ghost"}
            size="icon"
            aria-label="Vue grille"
            title="Vue grille"
            aria-pressed={mode === "cards"}
            onClick={() => setMode("cards")}
          >
            <LayoutGrid className="size-4" aria-hidden />
          </Button>
          {trashMode ? null : (
            <Button variant="outline" size="sm" onClick={() => setSaveOpen(true)}>
            <Bookmark className="size-4" />
            Sauver la vue
          </Button>
          )}
        </div>
      </div>
      <div className="flex flex-wrap gap-1.5">
        {tags.map((t) => (
          <TagChip
            key={t.id}
            tag={t}
            active={filter.tagIds?.includes(t.id)}
            onClick={() => toggleTag(t.id)}
          />
        ))}
      </div>
      {selected.length > 0 ? (
        <div className="flex flex-wrap items-center gap-2 rounded-lg bg-surface px-3 py-2 shadow-[var(--shadow-border)]">
          <span className="tabular text-sm">{selected.length} sélectionné{selected.length > 1 ? "s" : ""}</span>
          <Select
            onValueChange={(v) => bulk.mutate({ ids: selected, addTagIds: [Number(v)] })}
          >
            <SelectTrigger className="h-8 w-40">
              <SelectValue placeholder="Ajouter étiquette" />
            </SelectTrigger>
            <SelectContent>
              {tags.map((t) => (
                <SelectItem key={t.id} value={String(t.id)}>
                  {t.name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <Select
            onValueChange={(v) =>
              bulk.mutate({ ids: selected, correspondentId: v === "none" ? null : Number(v) })
            }
          >
            <SelectTrigger className="h-8 w-44">
              <SelectValue placeholder="Correspondant" />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="none">Aucun</SelectItem>
              {correspondents.map((c) => (
                <SelectItem key={c.id} value={String(c.id)}>
                  {c.name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          {trashMode ? (
            <>
              <Button
                variant="secondary"
                size="sm"
                onClick={() => {
                  if (confirm("Restaurer les documents sélectionnés ?")) {
                    bulk.mutate({ ids: selected, restoreSelected: true });
                  }
                }}
              >
                <RotateCcw className="size-4" />
                Restaurer
              </Button>
              <Button
                variant="danger"
                size="sm"
                onClick={() => {
                  if (
                    confirm(
                      "Supprimer définitivement les documents sélectionnés ? Cette action est irréversible.",
                    )
                  ) {
                    bulk.mutate({ ids: selected, purgeSelected: true });
                  }
                }}
              >
                <Trash2 className="size-4" />
                Supprimer définitivement
              </Button>
            </>
          ) : (
            <Button
              variant="danger"
              size="sm"
              onClick={() => {
                if (confirm("Mettre les documents sélectionnés à la corbeille ?")) {
                  bulk.mutate({ ids: selected, deleteSelected: true });
                }
              }}
            >
              <Trash2 className="size-4" />
              Supprimer
            </Button>
          )}
        </div>
      ) : null}

      {docs.isError && !isAuthRequired(docs.error) ? (
        <div className="rounded-xl bg-surface p-10 text-center text-sm text-danger shadow-[var(--shadow-border)]">
          Impossible de charger les documents : {(docs.error as Error).message}
        </div>
      ) : docs.isError && isAuthRequired(docs.error) ? (
        <div className="rounded-xl bg-surface p-10 text-center text-sm text-muted shadow-[var(--shadow-border)]">
          Authentification requise.
        </div>
      ) : docs.isLoading ? (
        <div className="rounded-xl bg-surface p-10 text-center text-sm text-muted shadow-[var(--shadow-border)]">
          Chargement de l'archive…
        </div>
      ) : items.length === 0 ? (
        <div className="rounded-xl bg-surface p-10 text-center shadow-[var(--shadow-border)]">
          <p className="font-display text-xl">{trashMode ? "Corbeille vide" : "Aucun document"}</p>
          <p className="mt-1 text-sm text-muted">{trashMode ? "Les documents supprimés apparaîtront ici." : "Ajustez les filtres ou importez un fichier."}</p>
        </div>
      ) : mode === "cards" ? (
        <div className="grid grid-cols-2 gap-3 md:grid-cols-3 xl:grid-cols-4">
          {items.map((d) => (
            <Link
              key={d.id}
              to="/documents/$id"
              params={{ id: String(d.id) }}
              className="group overflow-hidden rounded-xl bg-surface shadow-[var(--shadow-border)]"
            >
              <DocThumb documentId={d.id} svg={d.thumbnailSvg} mimeType={d.mimeType} alt={d.title} className="aspect-[3/4] w-full" />
              <div className="space-y-1 p-3">
                <p className="truncate text-sm font-medium">{d.title}</p>
                <p className="truncate text-xs text-muted">{d.correspondentName ?? "—"}</p>
                <div className="flex flex-wrap gap-1">
                  {sortTagsForGrid(d.tags).slice(0, 3).map((t) => (
                    <TagChip key={t.id} tag={t} />
                  ))}
                </div>
              </div>
            </Link>
          ))}
        </div>
      ) : (
        <div className="max-w-full min-w-0 overflow-x-auto rounded-xl bg-surface shadow-[var(--shadow-border)]">
          <table className="w-full text-left text-sm">
            <thead className="text-[11px] tracking-wide text-muted uppercase">
              <tr className="border-b border-border">
                <th className="w-10 px-3 py-2">
                  <Checkbox
                    checked={allSelected}
                    onCheckedChange={() => setSelected(allSelected ? [] : allIds)}
                    aria-label="Tout sélectionner"
                  />
                </th>
                <th className="w-16 px-2 py-2">Aperçu</th>
                <th className="px-2 py-2">ASN</th>
                <th className="px-2 py-2">Correspondant</th>
                <th className="px-2 py-2">Titre</th>
                <th className="px-2 py-2">Étiquettes</th>
                <th className="hidden px-2 py-2 xl:table-cell">Type</th>
                <th className="hidden px-2 py-2 xl:table-cell">Date</th>
                <th className="hidden w-10 px-2 py-2 xl:table-cell" />
              </tr>
            </thead>
            <tbody>
              {items.map((d) => (
                <tr
                  key={d.id}
                  className={cn(
                    "border-b border-border/70 last:border-0 hover:bg-surface-2/60",
                    selected.includes(d.id) && "bg-surface-2/80",
                  )}
                >
                  <td className="px-3 py-2">
                    <Checkbox
                      checked={selected.includes(d.id)}
                      onCheckedChange={(v) =>
                        setSelected((s) => (v ? [...s, d.id] : s.filter((x) => x !== d.id)))
                      }
                      aria-label={`Sélectionner ${d.title}`}
                    />
                  </td>
                  <td className="px-2 py-2">
                    <DocThumb
                      documentId={d.id}
                      svg={d.thumbnailSvg}
                      mimeType={d.mimeType}
                      alt=""
                      className="h-12 w-9 rounded-sm"
                    />
                  </td>
                  <td className="tabular px-2 py-2 text-muted">{asnLabel(d.asn)}</td>
                  <td className="max-w-36 truncate px-2 py-2">{d.correspondentName ?? "—"}</td>
                  <td className="px-2 py-2">
                    <Link
                      to="/documents/$id"
                      params={{ id: String(d.id) }}
                      title={d.title}
                      className="block max-w-[14rem] truncate font-medium hover:underline md:max-w-xs"
                    >
                      {d.title}
                    </Link>
                    {d.ocrStatus !== "done" ? (
                      <span className="ml-2 text-[11px] text-warn">{ocrStatusLabel(d.ocrStatus)}</span>
                    ) : null}
                  </td>
                  <td className="px-2 py-2">
                    <div className="flex max-w-56 flex-wrap gap-1">
                      {sortTagsForGrid(d.tags).map((t) => (
                        <TagChip key={t.id} tag={t} />
                      ))}
                    </div>
                  </td>
                  <td className="hidden px-2 py-2 text-muted xl:table-cell">{d.documentTypeName ?? "—"}</td>
                  <td className="hidden px-2 py-2 text-muted xl:table-cell">{formatDate(d.createdDate)}</td>
                  <td className="hidden px-2 py-2 xl:table-cell">
                    <DropdownMenu>
                      <DropdownMenuTrigger asChild>
                        <Button variant="ghost" size="icon-sm" aria-label="Actions">
                          <MoreHorizontal className="size-4" />
                        </Button>
                      </DropdownMenuTrigger>
                      <DropdownMenuContent>
                        <DropdownMenuItem onSelect={() => navigate({ to: "/documents/$id", params: { id: String(d.id) } })}>
                          Ouvrir
                        </DropdownMenuItem>
                        <DropdownMenuSeparator />
                        {trashMode ? (
                          <>
                            <DropdownMenuItem
                              onSelect={() => {
                                if (confirm(`Restaurer « ${d.title} » ?`)) {
                                  bulk.mutate({ ids: [d.id], restoreSelected: true });
                                }
                              }}
                            >
                              Restaurer
                            </DropdownMenuItem>
                            <DropdownMenuItem
                              onSelect={() => {
                                if (
                                  confirm(
                                    `Supprimer définitivement « ${d.title} » ? Cette action est irréversible.`,
                                  )
                                ) {
                                  bulk.mutate({ ids: [d.id], purgeSelected: true });
                                }
                              }}
                            >
                              Supprimer définitivement
                            </DropdownMenuItem>
                          </>
                        ) : (
                          <DropdownMenuItem
                            onSelect={() => {
                              if (confirm(`Mettre « ${d.title} » à la corbeille ?`)) {
                                bulk.mutate({ ids: [d.id], deleteSelected: true });
                              }
                            }}
                          >
                            Supprimer
                          </DropdownMenuItem>
                        )}
                      </DropdownMenuContent>
                    </DropdownMenu>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      <p className="text-xs text-muted">
        {items.length} document{items.length > 1 ? "s" : ""}
        {activeTag.length ? ` · filtres étiquettes` : ""}
      </p>

      <Dialog open={saveOpen} onOpenChange={setSaveOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Enregistrer la vue</DialogTitle>
            <DialogDescription>
              Les filtres actuels deviendront une vue dans la barre latérale.
            </DialogDescription>
          </DialogHeader>
          <Label htmlFor="view-name">Nom</Label>
          <Input id="view-name" value={viewName} onChange={(e) => setViewName(e.target.value)} className="mt-1" />
          <div className="mt-4 flex justify-end gap-2">
            <Button variant="ghost" onClick={() => setSaveOpen(false)}>
              Annuler
            </Button>
            <Button disabled={!viewName.trim() || saveView.isPending} onClick={() => saveView.mutate()}>
              Enregistrer
            </Button>
          </div>
        </DialogContent>
      </Dialog>
    </div>
  );
}
