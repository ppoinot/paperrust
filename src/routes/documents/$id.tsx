import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { createFileRoute, Link } from "@tanstack/react-router";
import { ArrowLeft, Download, Loader2, ScanText } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { toast } from "sonner";
import { DocumentEditButton } from "@/components/documents/document-edit-button";
import { PdfCraftHost } from "@/components/documents/pdfcraft-host";
import { PdfEditApplyPanel } from "@/components/documents/pdf-edit-apply-panel";
import { WordEditApplyPanel } from "@/components/documents/word-edit-apply-panel";
import { WordcraftHost } from "@/components/documents/wordcraft-host";
import {
  DocumentFicheForm,
  type FicheSaveStatus,
} from "@/components/documents/document-fiche-form";
import { DocumentPages } from "@/components/documents/document-pages";
import { AppShell } from "@/components/layout/app-shell";
import { Button } from "@/components/ui/button";
import { fetchDocument, rerunOcr } from "@/lib/paperless/actions";
import { isAuthRequired } from "@/lib/auth/auth-required";
import { useSessionIdentity } from "@/lib/auth/session-identity";
import { humanizeOcrError } from "@/lib/paperless/format";
import { readLlmSettings } from "@/lib/paperless/llm-settings";
import {
  PDF_APPLY_BANNER_HELP,
  PDF_APPLY_SUCCESS,
  PDF_OPEN_FILE_FAILED,
  PDF_SESSION_EXPIRED,
  documentFileEditTarget,
  isPdfEditSessionExpired,
  openUrlViaAnchor,
  pdfcraftEditorUrl,
  requestEditorSave,
  toastForPdfEditCommitFailure,
  type PdfEditSession,
} from "@/lib/paperless/pdfcraft-edit";
import {
  WORD_APPLY_HELP,
  WORD_DIRTY_LEAVE,
  WORD_ORIGINAL_SAVED,
  WORD_RENDITION_SAVED,
  requestWordSave,
  wordEditionTabUrl,
} from "@/lib/paperless/wordcraft-edit";

export const Route = createFileRoute("/documents/$id")({ component: DocumentDetail });

function saveStatusLabel(status: FicheSaveStatus): string | null {
  switch (status) {
    case "saving":
      return "Enregistrement…";
    case "saved":
      return "Enregistré";
    case "error":
      return "Erreur d’enregistrement";
    default:
      return null;
  }
}

function DocumentDetail() {
  const { id } = Route.useParams();
  const docId = Number(id);
  const qc = useQueryClient();
  const query = useQuery({
    queryKey: ["document", docId],
    queryFn: () => fetchDocument({ data: { id: docId } }),
  });
  const doc = query.data;
  const [saveStatus, setSaveStatus] = useState<FicheSaveStatus>("idle");
  const [viewerEpoch, setViewerEpoch] = useState(0);
  const [editSession, setEditSession] = useState<PdfEditSession | null>(null);
  const [editing, setEditing] = useState(false);
  const [wordOpen, setWordOpen] = useState(false);
  const [wordClosed, setWordClosed] = useState(false);
  const [wordDirty, setWordDirty] = useState(false);
  const editorFrameRef = useRef<HTMLIFrameElement>(null);
  const wordFrameRef = useRef<HTMLIFrameElement>(null);
  const identity = useSessionIdentity();
  const onSaveStatusChange = useCallback((status: FicheSaveStatus) => {
    setSaveStatus(status);
  }, []);

  const wordDoc = doc
    ? documentFileEditTarget(doc.mimeType, doc.originalFilename) === "wordcraft"
    : false;
  const showWordEditor = Boolean(doc) && wordDoc && !wordClosed && wordOpen;
  const author = identity.isPending ? null : (identity.data?.name ?? "");

  const hideWordEditor = useCallback(() => {
    setWordClosed(true);
    setWordOpen(false);
    if (typeof window === "undefined" || window.location.hash !== "#edition") return;
    const next = `${window.location.pathname}${window.location.search}`;
    window.history.replaceState(window.history.state, "", next);
  }, []);

  const clearEditSession = useCallback(() => {
    setEditSession(null);
    setEditing(false);
  }, []);

  const refreshAfterCommit = useCallback(() => {
    void qc.invalidateQueries({ queryKey: ["document", docId] });
    void qc.invalidateQueries({ queryKey: ["documents"] });
    void qc.invalidateQueries({ queryKey: ["dashboard"] });
    setViewerEpoch((n) => n + 1);
    clearEditSession();
  }, [qc, docId, clearEditSession]);

  const refreshWord = useCallback(() => {
    void qc.invalidateQueries({ queryKey: ["document", docId] });
    void qc.invalidateQueries({ queryKey: ["documents"] });
    void qc.invalidateQueries({ queryKey: ["dashboard"] });
    setViewerEpoch((n) => n + 1);
  }, [qc, docId]);

  useEffect(() => {
    setWordOpen(false);
    setWordClosed(false);
    setWordDirty(false);
    clearEditSession();
  }, [docId, clearEditSession]);

  useEffect(() => {
    if (wordDoc && !wordClosed) setWordOpen(true);
  }, [wordDoc, wordClosed]);

  useEffect(() => {
    if (typeof window === "undefined" || window.location.hash !== "#edition") return;
    if (!wordDoc) return;
    setWordClosed(false);
    setWordOpen(true);
  }, [docId, wordDoc]);

  useEffect(() => {
    if (!editSession) return;
    if (isPdfEditSessionExpired(editSession)) {
      toast.error(PDF_SESSION_EXPIRED);
      clearEditSession();
      return;
    }
    const expires = Date.parse(editSession.expiresAt);
    const ms = expires - Date.now();
    if (!Number.isFinite(ms) || ms <= 0) return;
    const timer = window.setTimeout(() => {
      toast.error(PDF_SESSION_EXPIRED);
      clearEditSession();
    }, ms);
    return () => window.clearTimeout(timer);
  }, [editSession, clearEditSession]);

  const onSessionStarted = useCallback((session: PdfEditSession) => {
    if (isPdfEditSessionExpired(session)) {
      toast.error(PDF_SESSION_EXPIRED);
      return;
    }
    setEditSession(session);
    setEditing(true);
  }, []);

  const ocr = useMutation({
    mutationFn: () => {
      const llm = readLlmSettings();
      return rerunOcr({
        data: { id: docId, provider: llm.provider },
      });
    },
    onSuccess: (res) => {
      if (!res.ok) toast.error(humanizeOcrError(res.error));
      else toast.success("OCR mis à jour");
      void qc.invalidateQueries({ queryKey: ["document", docId] });
      void qc.invalidateQueries({ queryKey: ["documents"] });
      void qc.invalidateQueries({ queryKey: ["dashboard"] });
    },
  });

  if (query.isLoading) {
    return (
      <AppShell title="Document">
        <p className="text-sm text-muted">Chargement…</p>
      </AppShell>
    );
  }
  if (query.isError) {
    if (isAuthRequired(query.error)) {
      return (
        <AppShell title="Document">
          <p className="text-sm text-muted">Authentification requise.</p>
        </AppShell>
      );
    }
    return (
      <AppShell title="Document">
        <p className="text-sm text-danger">
          Impossible d&apos;ouvrir le document : {(query.error as Error).message}
        </p>
      </AppShell>
    );
  }
  if (!doc) {
    return (
      <AppShell title="Document">
        <p>Introuvable.</p>
      </AppShell>
    );
  }

  const fileHref = `/api/documents/${docId}/file`;
  const canDownload = Boolean(doc.hasOriginal) || doc.fileSize > 0;
  const statusLabel = saveStatusLabel(saveStatus);
  const liveSession =
    editSession && editSession.documentId === docId && !isPdfEditSessionExpired(editSession)
      ? editSession
      : null;

  return (
    <AppShell
      title={doc.title}
      actions={
        <div className="flex items-center gap-2">
          {statusLabel ? (
            <span
              className={saveStatus === "error" ? "text-sm text-danger" : "text-sm text-muted"}
              aria-live="polite"
            >
              {statusLabel}
            </span>
          ) : null}
          <DocumentEditButton
            documentId={docId}
            mimeType={doc.mimeType}
            filename={doc.originalFilename}
            hasFile={canDownload}
            editing={wordDoc ? showWordEditor : Boolean(liveSession) && editing}
            sessionOpen={wordDoc ? true : Boolean(liveSession)}
            onShowReading={() => {
              if (wordDoc) {
                hideWordEditor();
                return;
              }
              setEditing(false);
            }}
            onShowEditing={() => {
              if (wordDoc) {
                setWordClosed(false);
                setWordOpen(true);
                return;
              }
              setEditing(true);
            }}
            onSessionStarted={onSessionStarted}
          />
          {canDownload ? (
            <Button variant="outline" size="sm" asChild>
              <a href={fileHref} download={doc.originalFilename ?? "document"}>
                <Download className="size-4" />
                Télécharger
              </a>
            </Button>
          ) : null}
          <Button variant="outline" size="sm" onClick={() => ocr.mutate()} disabled={ocr.isPending}>
            {ocr.isPending ? <Loader2 className="size-4 animate-spin" /> : <ScanText className="size-4" />}
            OCR
          </Button>
        </div>
      }
    >
      <div className="mb-4">
        <Link
          to="/documents"
          search={{}}
          className="inline-flex items-center gap-1 text-sm text-muted hover:text-fg"
          onClick={(event) => {
            if (!wordDirty) return;
            if (!window.confirm(WORD_DIRTY_LEAVE)) event.preventDefault();
          }}
        >
          <ArrowLeft className="size-4" />
          Documents
        </Link>
      </div>
      <div className="grid gap-5 lg:grid-cols-[minmax(0,1.1fr)_minmax(18rem,0.9fr)] lg:items-start">
        <div className="space-y-4">
          {wordDoc && author !== null ? (
            <div
              className={
                showWordEditor
                  ? "space-y-4"
                  : "pointer-events-none invisible fixed top-0 left-0 -z-10 h-[80vh] w-[min(100%,72rem)] overflow-hidden"
              }
              aria-hidden={showWordEditor ? undefined : true}
              inert={showWordEditor ? undefined : true}
            >
              {showWordEditor ? (
                <WordEditApplyPanel
                  documentId={docId}
                  onSave={() => {
                    const frame = wordFrameRef.current;
                    const saved = requestWordSave(frame?.contentWindow ?? null);
                    if (!saved) {
                      frame?.focus();
                      toast.message(WORD_APPLY_HELP);
                    }
                  }}
                  onOpenEditor={() => {
                    const origin = typeof window === "undefined" ? "" : window.location.origin;
                    let opened = false;
                    try {
                      opened = origin ? openUrlViaAnchor(wordEditionTabUrl(origin, docId)) : false;
                    } catch {
                      opened = false;
                    }
                    if (!opened) toast.error(PDF_OPEN_FILE_FAILED);
                  }}
                  onCancel={hideWordEditor}
                />
              ) : null}
              <WordcraftHost
                key={docId}
                documentId={docId}
                filename={doc.originalFilename || "document.docx"}
                author={author}
                frameRef={wordFrameRef}
                autoExport
                onDirty={setWordDirty}
                onCommitted={(result) => {
                  if (result.kind === "rendition") toast.success(WORD_RENDITION_SAVED);
                  else if (result.kind === "original") toast.success(WORD_ORIGINAL_SAVED);
                  refreshWord();
                }}
              />
            </div>
          ) : null}
          {liveSession ? (
            <div
              className={
                editing
                  ? "space-y-4"
                  : "pointer-events-none invisible fixed top-0 left-0 -z-10 h-[80vh] w-[min(100%,72rem)] overflow-hidden"
              }
              aria-hidden={editing ? undefined : true}
              inert={editing ? undefined : true}
            >
              <PdfCraftHost
                session={liveSession}
                pageOrigin={typeof window === "undefined" ? "" : window.location.origin}
                frameRef={editorFrameRef}
                onCommitted={() => {
                  toast.success(PDF_APPLY_SUCCESS);
                  refreshAfterCommit();
                }}
                onFailed={(err) => toast.error(toastForPdfEditCommitFailure(err))}
              />
              {editing ? (
                <PdfEditApplyPanel
                  session={liveSession}
                  pending={false}
                  onSave={() => {
                    const frame = editorFrameRef.current;
                    const saved = requestEditorSave(frame?.contentWindow ?? null);
                    if (!saved) {
                      frame?.focus();
                      toast.message(PDF_APPLY_BANNER_HELP);
                    }
                  }}
                  onOpenEditor={() => {
                    const origin = typeof window === "undefined" ? "" : window.location.origin;
                    const opened = origin
                      ? openUrlViaAnchor(pdfcraftEditorUrl(origin, liveSession.sourceFileUrl, "tab"))
                      : false;
                    if (!opened) toast.error(PDF_OPEN_FILE_FAILED);
                  }}
                  onCancel={clearEditSession}
                />
              ) : null}
            </div>
          ) : null}
          {showWordEditor || (editing && liveSession) ? null : wordDoc ? (
            <div className="rounded-xl bg-surface p-4 text-sm text-muted shadow-[var(--shadow-border)]">
              L’aperçu PDF n’est pas encore prêt. Éditer rouvre le document.
            </div>
          ) : (
            <div className="max-h-[80vh] overflow-y-auto rounded-xl bg-surface p-3 shadow-[var(--shadow-border)]">
              <DocumentPages
                key={`${docId}-${viewerEpoch}`}
                docId={docId}
                title={doc.title}
                svg={doc.thumbnailSvg}
                fileData={doc.fileData}
                mimeType={doc.mimeType}
                fileRevision={viewerEpoch}
              />
            </div>
          )}
        </div>
        <div className="space-y-4">
          <DocumentFicheForm docId={docId} doc={doc} onSaveStatusChange={onSaveStatusChange} />
        </div>
      </div>
    </AppShell>
  );
}
