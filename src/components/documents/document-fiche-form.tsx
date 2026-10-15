import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { Loader2, Plus, Trash2 } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { toast } from "sonner";
import { CorrespondentCombobox } from "@/components/documents/correspondent-combobox";
import { TagChip } from "@/components/documents/tag-chip";
import { TypeCombobox } from "@/components/documents/type-combobox";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Textarea } from "@/components/ui/textarea";
import {
  addNote,
  bulkUpdateDocuments,
  createNamedCorrespondent,
  createNamedTag,
  createNamedType,
  updateDocumentMeta,
} from "@/lib/paperless/actions";
import {
  asnLabel,
  dedupeTagsByName,
  sortTagsForGrid,
  entityFromPost,
  formatDate,
  formatDateTime,
  ocrStatusLabel,
  pickByNameCI,
} from "@/lib/paperless/format";
import { providerLabel } from "@/lib/paperless/llm-settings";
import type { Catalog, Correspondent, DocumentDetail, DocumentType } from "@/lib/paperless/types";
import { useCatalog } from "@/lib/paperless/use-catalog";

const EMPTY_TYPES: DocumentType[] = [];
const EMPTY_CORRESPONDENTS: Correspondent[] = [];

/** Typing debounce — brief ~400–800 ms. */
const DEBOUNCE_TEXT_MS = 600;
/** Selects / combobox picks — short delay so rapid picks coalesce. */
const DEBOUNCE_PICK_MS = 120;

export const DOCUMENT_FICHE_FORM_ID = "document-fiche-form";

export type FicheSaveStatus = "idle" | "saving" | "saved" | "error";

type MetaDraft = {
  title: string;
  content: string;
  correspondentId: string;
  pendingCorrespondentName: string | null;
  typeId: string;
  pathId: string;
  createdDate: string;
  asn: string;
  tagIds: number[];
  custom: Record<number, string>;
};

function metaFingerprint(d: MetaDraft): string {
  return JSON.stringify({
    title: d.title,
    content: d.content,
    correspondentId: d.correspondentId,
    pendingCorrespondentName: d.pendingCorrespondentName,
    typeId: d.typeId,
    pathId: d.pathId,
    createdDate: d.createdDate,
    asn: d.asn,
    custom: d.custom,
  });
}

function draftFromDoc(doc: DocumentDetail): MetaDraft {
  return {
    title: doc.title,
    content: doc.content,
    correspondentId: doc.correspondentId ? String(doc.correspondentId) : "none",
    pendingCorrespondentName: null,
    typeId: doc.documentTypeId ? String(doc.documentTypeId) : "none",
    pathId: doc.storagePathId ? String(doc.storagePathId) : "none",
    createdDate: doc.createdDate ?? "",
    asn: doc.asn ? String(doc.asn) : "",
    tagIds: doc.tags.map((t) => t.id),
    custom: Object.fromEntries(doc.customValues.map((c) => [c.fieldId, c.value ?? ""])),
  };
}

type Props = {
  docId: number;
  doc: DocumentDetail;
  onSaveStatusChange: (status: FicheSaveStatus) => void;
};

/** Fiche form hoist — draft state lives here so preview (sibling) skips re-render. */
export function DocumentFicheForm({ docId, doc, onSaveStatusChange }: Props) {
  const navigate = useNavigate();
  const qc = useQueryClient();
  const catalog = useCatalog();

  const [boot] = useState(() => draftFromDoc(doc));
  const [title, setTitle] = useState(boot.title);
  const [content, setContent] = useState(boot.content);
  const [correspondentId, setCorrespondentId] = useState(boot.correspondentId);
  /** Unknown typed name — created + linked on fiche autosave. */
  const [pendingCorrespondentName, setPendingCorrespondentName] = useState<string | null>(
    boot.pendingCorrespondentName,
  );
  const [typeId, setTypeId] = useState(boot.typeId);
  const [pathId, setPathId] = useState(boot.pathId);
  const [createdDate, setCreatedDate] = useState(boot.createdDate);
  const [asn, setAsn] = useState(boot.asn);
  const [tagIds, setTagIds] = useState(boot.tagIds);
  const [note, setNote] = useState("");
  const [custom, setCustom] = useState(boot.custom);
  const [addingTag, setAddingTag] = useState(false);
  const [newTagName, setNewTagName] = useState("");
  const [creatingTag, setCreatingTag] = useState(false);
  const [saving, setSaving] = useState(false);

  const draftRef = useRef<MetaDraft>(boot);
  const lastSavedFpRef = useRef(metaFingerprint(boot));
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const savingRef = useRef(false);
  const queuedRef = useRef(false);
  const skipDebounceRef = useRef(true);
  const onStatusRef = useRef(onSaveStatusChange);
  onStatusRef.current = onSaveStatusChange;
  const catalogRef = useRef(catalog.data);
  catalogRef.current = catalog.data;
  const qcRef = useRef(qc);
  qcRef.current = qc;

  draftRef.current = {
    title,
    content,
    correspondentId,
    pendingCorrespondentName,
    typeId,
    pathId,
    createdDate,
    asn,
    tagIds,
    custom,
  };

  const applyDraft = useCallback((d: MetaDraft) => {
    setTitle(d.title);
    setContent(d.content);
    setCorrespondentId(d.correspondentId);
    setPendingCorrespondentName(d.pendingCorrespondentName);
    setTypeId(d.typeId);
    setPathId(d.pathId);
    setCreatedDate(d.createdDate);
    setAsn(d.asn);
    setTagIds(d.tagIds);
    setCustom(d.custom);
  }, []);

  useEffect(() => {
    if (!doc) return;
    // Avoid clobbering in-flight user edits while a save invalidate refreshes `doc`.
    if (savingRef.current || timerRef.current || queuedRef.current) return;
    if (metaFingerprint(draftRef.current) !== lastSavedFpRef.current) return;
    const next = draftFromDoc(doc);
    const nextFp = metaFingerprint(next);
    // Same meta as draft — keep « Enregistré »; only re-hydrate when server diverges.
    if (nextFp === metaFingerprint(draftRef.current)) {
      lastSavedFpRef.current = nextFp;
      return;
    }
    skipDebounceRef.current = true;
    applyDraft(next);
    lastSavedFpRef.current = nextFp;
  }, [doc, applyDraft]);

  const persistMeta = useCallback(async (): Promise<boolean> => {
    const draft = draftRef.current;
    const fp = metaFingerprint(draft);
    if (fp === lastSavedFpRef.current) return true;

    if (savingRef.current) {
      queuedRef.current = true;
      return false;
    }

    savingRef.current = true;
    setSaving(true);
    onStatusRef.current("saving");

    let resolvedCorrespondentId: number | null =
      draft.correspondentId === "none" ? null : Number(draft.correspondentId);
    let nextCorrespondentId = draft.correspondentId;
    let nextPending: string | null = draft.pendingCorrespondentName;

    try {
      if (draft.pendingCorrespondentName) {
        const name = draft.pendingCorrespondentName.trim();
        const existing = pickByNameCI(catalogRef.current?.correspondents ?? EMPTY_CORRESPONDENTS, name);
        let created = existing;
        if (!created) {
          created = await createNamedCorrespondent(name);
          qcRef.current.setQueryData(["catalog"], (prev: Catalog | undefined) => {
            if (!prev) return prev;
            const next = prev.correspondents.some((c) => c.id === created!.id)
              ? prev.correspondents.map((c) => (c.id === created!.id ? created! : c))
              : [...prev.correspondents, created!];
            return { ...prev, correspondents: next };
          });
        }
        resolvedCorrespondentId = created.id;
        nextCorrespondentId = String(created.id);
        nextPending = null;
        setCorrespondentId(nextCorrespondentId);
        setPendingCorrespondentName(null);
      }

      await updateDocumentMeta({
        data: {
          id: docId,
          title: draft.title,
          content: draft.content,
          correspondentId: resolvedCorrespondentId,
          documentTypeId: draft.typeId === "none" ? null : Number(draft.typeId),
          storagePathId: draft.pathId === "none" ? null : Number(draft.pathId),
          createdDate: draft.createdDate || null,
          asn: draft.asn ? Number(draft.asn) : null,
          tagIds: draft.tagIds,
          customValues: Object.entries(draft.custom).map(([fieldId, value]) => ({
            fieldId: Number(fieldId),
            value: value || null,
          })),
        },
      });

      const savedDraft: MetaDraft = {
        ...draft,
        correspondentId: nextCorrespondentId,
        pendingCorrespondentName: nextPending,
      };
      lastSavedFpRef.current = metaFingerprint(savedDraft);
      onStatusRef.current("saved");
      void qcRef.current.invalidateQueries({ queryKey: ["document", docId] });
      void qcRef.current.invalidateQueries({ queryKey: ["documents"] });
      void qcRef.current.invalidateQueries({ queryKey: ["dashboard"] });
      void qcRef.current.invalidateQueries({ queryKey: ["catalog"] });
      return true;
    } catch (e) {
      onStatusRef.current("error");
      toast.error((e as Error).message);
      return false;
    } finally {
      savingRef.current = false;
      setSaving(false);
      if (queuedRef.current) {
        queuedRef.current = false;
        void persistMeta();
      }
    }
  }, [docId]);

  const scheduleSave = useCallback(
    (delayMs: number) => {
      if (timerRef.current) clearTimeout(timerRef.current);
      timerRef.current = setTimeout(() => {
        timerRef.current = null;
        void persistMeta();
      }, delayMs);
    },
    [persistMeta],
  );

  const flushSave = useCallback(() => {
    if (timerRef.current) {
      clearTimeout(timerRef.current);
      timerRef.current = null;
    }
    void persistMeta();
  }, [persistMeta]);

  // Debounced autosave for text-ish fields.
  useEffect(() => {
    if (skipDebounceRef.current) {
      skipDebounceRef.current = false;
      return;
    }
    if (metaFingerprint(draftRef.current) === lastSavedFpRef.current) return;
    scheduleSave(DEBOUNCE_TEXT_MS);
  }, [title, content, asn, createdDate, custom, scheduleSave]);

  // Short debounce for picks (correspondent / path). Type persists on its own path.
  useEffect(() => {
    if (skipDebounceRef.current) return;
    if (metaFingerprint(draftRef.current) === lastSavedFpRef.current) return;
    scheduleSave(DEBOUNCE_PICK_MS);
  }, [correspondentId, pendingCorrespondentName, pathId, scheduleSave]);

  // Flush pending edits on leave / unmount.
  useEffect(() => {
    const onBeforeUnload = () => {
      if (metaFingerprint(draftRef.current) !== lastSavedFpRef.current) {
        void persistMeta();
      }
    };
    window.addEventListener("beforeunload", onBeforeUnload);
    return () => {
      window.removeEventListener("beforeunload", onBeforeUnload);
      if (timerRef.current) {
        clearTimeout(timerRef.current);
        timerRef.current = null;
      }
      if (metaFingerprint(draftRef.current) !== lastSavedFpRef.current) {
        void persistMeta();
      }
    };
  }, [persistMeta]);

  // Optional Cmd/Ctrl+S → flush.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.metaKey || e.ctrlKey) || e.key.toLowerCase() !== "s") return;
      e.preventDefault();
      flushSave();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [flushSave]);

  const noteMut = useMutation({
    mutationFn: () => addNote({ data: { documentId: docId, body: note } }),
    onSuccess: () => {
      setNote("");
      qc.invalidateQueries({ queryKey: ["document", docId] });
    },
  });

  const persistTagIds = (next: number[], prev: number[]) => {
    setTagIds(next);
    void updateDocumentMeta({ data: { id: docId, tagIds: next } })
      .then(() => {
        void qc.invalidateQueries({ queryKey: ["documents"] });
        void qc.invalidateQueries({ queryKey: ["dashboard"] });
      })
      .catch((e: Error) => {
        toast.error(e.message);
        setTagIds(prev);
      });
  };

  const createAndAttachTag = async () => {
    const name = newTagName.trim();
    if (!name || creatingTag) return;
    const existing = pickByNameCI(catalog.data?.tags ?? [], name);
    if (existing) {
      if (!tagIds.includes(existing.id)) persistTagIds([...tagIds, existing.id], tagIds);
      setAddingTag(false);
      setNewTagName("");
      return;
    }
    setCreatingTag(true);
    try {
      const created = entityFromPost(await createNamedTag(name), name);
      if (!created?.id) throw new Error("Étiquette créée introuvable");
      const next = tagIds.includes(created.id) ? tagIds : [...tagIds, created.id];
      setTagIds(next);
      qc.setQueryData(["catalog"], (prev: Catalog | undefined) => {
        if (!prev) return prev;
        const tags = prev.tags.some((t) => t.id === created.id)
          ? prev.tags.map((t) => (t.id === created.id ? created : t))
          : [...prev.tags, created];
        return { ...prev, tags };
      });
      await updateDocumentMeta({ data: { id: docId, tagIds: next } });
      void qc.invalidateQueries({ queryKey: ["catalog"] });
      void qc.invalidateQueries({ queryKey: ["documents"] });
      void qc.invalidateQueries({ queryKey: ["dashboard"] });
      void qc.invalidateQueries({ queryKey: ["document", docId] });
      setAddingTag(false);
      setNewTagName("");
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setCreatingTag(false);
    }
  };

  const documentTypes = catalog.data?.documentTypes;
  const types = useMemo(() => documentTypes ?? EMPTY_TYPES, [documentTypes]);

  const commitType = useCallback(
    async (name: string) => {
      const trimmed = name.trim();
      try {
        onStatusRef.current("saving");
        if (!trimmed) {
          setTypeId("none");
          await updateDocumentMeta({ data: { id: docId, documentTypeId: null } });
          draftRef.current = { ...draftRef.current, typeId: "none" };
          lastSavedFpRef.current = metaFingerprint({
            ...draftRef.current,
            typeId: "none",
          });
          onStatusRef.current("saved");
          void qc.invalidateQueries({ queryKey: ["documents"] });
          void qc.invalidateQueries({ queryKey: ["document", docId] });
          return;
        }
        const existing = pickByNameCI(documentTypes ?? EMPTY_TYPES, trimmed);
        let id = existing?.id;
        if (id == null) {
          const created = await createNamedType(trimmed);
          if (!created?.id) throw new Error("Type créé introuvable");
          id = created.id;
          qc.setQueryData(["catalog"], (prev: Catalog | undefined) => {
            if (!prev) return prev;
            const nextTypes = prev.documentTypes.some((t) => t.id === created.id)
              ? prev.documentTypes.map((t) => (t.id === created.id ? created : t))
              : [...prev.documentTypes, created];
            return { ...prev, documentTypes: nextTypes };
          });
        }
        setTypeId(String(id));
        await updateDocumentMeta({ data: { id: docId, documentTypeId: id } });
        draftRef.current = { ...draftRef.current, typeId: String(id) };
        lastSavedFpRef.current = metaFingerprint({
          ...draftRef.current,
          typeId: String(id),
        });
        onStatusRef.current("saved");
        void qc.invalidateQueries({ queryKey: ["catalog"] });
        void qc.invalidateQueries({ queryKey: ["documents"] });
        void qc.invalidateQueries({ queryKey: ["document", docId] });
      } catch (e) {
        onStatusRef.current("error");
        toast.error((e as Error).message);
        throw e;
      }
    },
    [docId, documentTypes, qc],
  );

  const correspondents = useMemo(
    () => catalog.data?.correspondents ?? EMPTY_CORRESPONDENTS,
    [catalog.data?.correspondents],
  );

  const commitCorrespondent = useCallback((name: string) => {
    const trimmed = name.trim();
    if (!trimmed) {
      setCorrespondentId("none");
      setPendingCorrespondentName(null);
      return;
    }
    const existing = pickByNameCI(correspondents, trimmed);
    if (existing) {
      setCorrespondentId(String(existing.id));
      setPendingCorrespondentName(null);
      return;
    }
    setCorrespondentId("none");
    setPendingCorrespondentName(trimmed);
  }, [correspondents]);

  const tagGrid = useMemo(
    () => sortTagsForGrid(dedupeTagsByName(catalog.data?.tags ?? []), tagIds),
    [catalog.data?.tags, tagIds],
  );

  return (
    <form
      id={DOCUMENT_FICHE_FORM_ID}
      onSubmit={(e) => {
        e.preventDefault();
        flushSave();
      }}
    >
      <Tabs defaultValue="meta">
        <TabsList>
          <TabsTrigger value="meta">Métadonnées</TabsTrigger>
          <TabsTrigger value="content">Contenu</TabsTrigger>
          <TabsTrigger value="notes">Notes</TabsTrigger>
          <TabsTrigger value="history">Historique</TabsTrigger>
        </TabsList>
        <TabsContent value="meta" className="space-y-3 rounded-xl bg-surface p-4 shadow-[var(--shadow-border)]">
          <div>
            <Label>Titre</Label>
            <Input
              className="mt-1"
              value={title}
              onChange={(e) => setTitle(e.target.value)}
              onBlur={() => flushSave()}
            />
          </div>
          <div className="grid grid-cols-2 gap-3">
            <div>
              <Label htmlFor="doc-created-date">Date du document</Label>
              {/*
                Browsers with US locale paint type=date as MM/DD/YYYY (e.g. 09/03/2026).
                Keep ISO in the input; overlay the FR label so that ambiguous string is never visible.
              */}
              <div className="relative mt-1">
                <p
                  id="doc-created-date-fr"
                  className="pointer-events-none absolute inset-y-0 left-3 z-10 flex items-center text-sm font-medium text-fg"
                  aria-live="polite"
                >
                  {createdDate ? formatDate(createdDate) : "Non renseignée"}
                </p>
                <Input
                  id="doc-created-date"
                  className="relative z-0 cursor-pointer text-transparent caret-transparent [-webkit-text-fill-color:transparent] [&::-webkit-calendar-picker-indicator]:cursor-pointer [&::-webkit-calendar-picker-indicator]:opacity-100 [&::-webkit-datetime-edit]:text-transparent [&::-webkit-datetime-edit-fields-wrapper]:text-transparent [&::-webkit-datetime-edit-text]:text-transparent [&::-webkit-datetime-edit-month-field]:text-transparent [&::-webkit-datetime-edit-day-field]:text-transparent [&::-webkit-datetime-edit-year-field]:text-transparent"
                  type="date"
                  lang="fr-FR"
                  value={createdDate}
                  onChange={(e) => setCreatedDate(e.target.value)}
                  onBlur={() => flushSave()}
                  aria-describedby="doc-created-date-fr"
                  aria-label={
                    createdDate
                      ? `Date du document : ${formatDate(createdDate)}`
                      : "Date du document, non renseignée"
                  }
                  title={createdDate ? formatDate(createdDate) : "Choisir une date"}
                />
              </div>
            </div>
            <div>
              <Label>ASN</Label>
              <Input
                className="mt-1 tabular"
                value={asn}
                onChange={(e) => setAsn(e.target.value)}
                onBlur={() => flushSave()}
              />
            </div>
          </div>
          <div>
            <Label>Correspondant</Label>
            <CorrespondentCombobox
              correspondents={correspondents}
              valueId={
                pendingCorrespondentName
                  ? null
                  : correspondentId === "none"
                    ? null
                    : Number(correspondentId)
              }
              draftName={pendingCorrespondentName}
              disabled={saving}
              onCommit={commitCorrespondent}
            />
          </div>
          <div>
            <Label>Type</Label>
            <TypeCombobox
              types={types}
              valueId={typeId === "none" ? null : Number(typeId)}
              onCommit={commitType}
            />
          </div>
          <div>
            <Label>Chemin de stockage</Label>
            <Select value={pathId} onValueChange={setPathId}>
              <SelectTrigger className="mt-1">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="none">Aucun</SelectItem>
                {(catalog.data?.storagePaths ?? []).map((p) => (
                  <SelectItem key={p.id} value={String(p.id)}>
                    {p.name}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          <div>
            <Label>Étiquettes</Label>
            <div className="mt-2 flex flex-wrap items-center gap-1.5">
              {tagGrid.map((t) => {
                const id = Number(t.id);
                return (
                  <TagChip
                    key={id}
                    tag={t}
                    active={tagIds.includes(id)}
                    onClick={() => {
                      const prev = tagIds;
                      const next = prev.includes(id) ? prev.filter((x) => x !== id) : [...prev, id];
                      persistTagIds(next, prev);
                    }}
                  />
                );
              })}
              {addingTag ? (
                <form
                  className="flex items-center gap-1"
                  onSubmit={(e) => {
                    e.preventDefault();
                    void createAndAttachTag();
                  }}
                >
                  <Input
                    autoFocus
                    className="h-6 w-36 px-2 text-[11px]"
                    value={newTagName}
                    placeholder="Nom"
                    disabled={creatingTag}
                    onChange={(e) => setNewTagName(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === "Escape") {
                        setAddingTag(false);
                        setNewTagName("");
                      }
                    }}
                  />
                  <Button
                    type="submit"
                    size="sm"
                    className="h-6 px-2 text-[11px]"
                    disabled={creatingTag || !newTagName.trim()}
                  >
                    {creatingTag ? <Loader2 className="size-3 animate-spin" /> : "Ajouter"}
                  </Button>
                </form>
              ) : (
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  className="h-6 rounded-full px-2 text-[11px]"
                  onClick={() => setAddingTag(true)}
                >
                  <Plus className="size-3" />
                  Ajouter
                </Button>
              )}
            </div>
          </div>
          {(doc.customValues ?? []).map((field) => (
            <div key={field.fieldId}>
              <Label>{field.name}</Label>
              <Input
                className="mt-1"
                value={custom[field.fieldId] ?? ""}
                onChange={(e) => setCustom((c) => ({ ...c, [field.fieldId]: e.target.value }))}
                onBlur={() => flushSave()}
              />
            </div>
          ))}
          <p className="text-xs text-faint">
            {asnLabel(doc.asn)} · {ocrStatusLabel(doc.ocrStatus, doc.summary)}
            {doc.ocrProvider ? ` · ${providerLabel(doc.ocrProvider)}` : ""} · {doc.pageCount} p.
          </p>
          <p className="text-xs text-faint">
            Ajouté {formatDateTime(doc.addedAt)} · Modifié {formatDateTime(doc.modifiedAt)}
          </p>
          <Button
            variant="danger"
            size="sm"
            onClick={() => {
              if (!confirm("Mettre ce document à la corbeille ?")) return;
              void bulkUpdateDocuments({ data: { ids: [docId], deleteSelected: true } }).then(() => {
                void qc.invalidateQueries({ queryKey: ["documents"] });
                void qc.invalidateQueries({ queryKey: ["dashboard"] });
                void qc.invalidateQueries({ queryKey: ["catalog"] });
                void navigate({ to: "/trash" });
              });
            }}
          >
            <Trash2 className="size-4" />
            Supprimer
          </Button>
        </TabsContent>
        <TabsContent value="content" className="rounded-xl bg-surface p-4 shadow-[var(--shadow-border)]">
          {doc.summary ? <p className="mb-3 text-sm text-muted">{doc.summary}</p> : null}
          <Textarea
            className="min-h-80 font-mono text-[13px]"
            value={content}
            onChange={(e) => setContent(e.target.value)}
            onBlur={() => flushSave()}
          />
        </TabsContent>
        <TabsContent value="notes" className="space-y-3 rounded-xl bg-surface p-4 shadow-[var(--shadow-border)]">
          <Textarea value={note} onChange={(e) => setNote(e.target.value)} placeholder="Ajouter une note…" />
          <Button size="sm" disabled={!note.trim() || noteMut.isPending} onClick={() => noteMut.mutate()}>
            Ajouter
          </Button>
          <ul className="space-y-3">
            {doc.notes.map((n) => (
              <li key={n.id} className="rounded-lg bg-surface-2 p-3 text-sm">
                <p>{n.body}</p>
                <p className="mt-1 text-xs text-faint">{formatDateTime(n.createdAt)}</p>
              </li>
            ))}
          </ul>
        </TabsContent>
        <TabsContent value="history" className="rounded-xl bg-surface p-4 shadow-[var(--shadow-border)]">
          <ol className="space-y-3">
            {doc.history.map((h) => (
              <li key={h.id} className="border-l-2 border-border pl-3 text-sm">
                <p className="font-medium">{h.action}</p>
                {h.detail ? <p className="text-muted">{h.detail}</p> : null}
                <p className="text-xs text-faint">{formatDateTime(h.createdAt)}</p>
              </li>
            ))}
          </ol>
        </TabsContent>
      </Tabs>
    </form>
  );
}
