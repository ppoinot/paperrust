import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { createFileRoute, Link } from "@tanstack/react-router";
import { ArrowLeft, Download, Loader2, ScanText } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { toast } from "sonner";
import { DocumentEditButton } from "@/components/documents/document-edit-button";
import { StirlingApplyPanel } from "@/components/documents/stirling-apply-panel";
import {
  DocumentFicheForm,
  type FicheSaveStatus,
} from "@/components/documents/document-fiche-form";
import { DocumentPages } from "@/components/documents/document-pages";
import { AppShell } from "@/components/layout/app-shell";
import { Button } from "@/components/ui/button";
import { commitPdfStirlingEdit, fetchDocument, pullPdfStirlingEdit, rerunOcr } from "@/lib/paperless/actions";
import { isAuthRequired } from "@/lib/auth/auth-required";
import { humanizeOcrError } from "@/lib/paperless/format";
import { readLlmSettings } from "@/lib/paperless/llm-settings";
import {
  STIRLING_APPLY_SUCCESS,
  STIRLING_EDIT_HINT_BLOCKED,
  STIRLING_OPEN_FILE_FAILED,
  STIRLING_SESSION_EXPIRED,
  isStirlingSessionExpired,
  openStirlingEditor,
  openStirlingSourceFile,
  resolveStirlingEditHandoff,
  toastForStirlingCommitFailure,
  type StirlingEditSession,
} from "@/lib/paperless/stirling-edit";

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
  const [stirlingSession, setStirlingSession] = useState<StirlingEditSession | null>(null);
  const onSaveStatusChange = useCallback((status: FicheSaveStatus) => {
    setSaveStatus(status);
  }, []);

  const clearStirlingSession = useCallback(() => {
    setStirlingSession(null);
  }, []);

  const refreshAfterCommit = useCallback(() => {
    void qc.invalidateQueries({ queryKey: ["document", docId] });
    void qc.invalidateQueries({ queryKey: ["documents"] });
    void qc.invalidateQueries({ queryKey: ["dashboard"] });
    setViewerEpoch((n) => n + 1);
    clearStirlingSession();
  }, [qc, docId, clearStirlingSession]);

  useEffect(() => {
    if (!stirlingSession) return;
    if (isStirlingSessionExpired(stirlingSession)) {
      toast.error(STIRLING_SESSION_EXPIRED);
      clearStirlingSession();
      return;
    }
    const t = Date.parse(stirlingSession.expiresAt);
    const ms = t - Date.now();
    if (!Number.isFinite(ms) || ms <= 0) return;
    const id = window.setTimeout(() => {
      toast.error(STIRLING_SESSION_EXPIRED);
      clearStirlingSession();
    }, ms);
    return () => window.clearTimeout(id);
  }, [stirlingSession, clearStirlingSession]);

  const onSessionStarted = useCallback((session: StirlingEditSession) => {
    if (isStirlingSessionExpired(session)) {
      toast.error(STIRLING_SESSION_EXPIRED);
      return;
    }
    setStirlingSession(session);
  }, []);

  const commit = useMutation({
    mutationFn: (file: File) => {
      if (!stirlingSession || isStirlingSessionExpired(stirlingSession)) {
        throw Object.assign(new Error(STIRLING_SESSION_EXPIRED), { status: 410 });
      }
      return commitPdfStirlingEdit(docId, file, stirlingSession.token, file.name);
    },
    onSuccess: () => {
      toast.success(STIRLING_APPLY_SUCCESS);
      refreshAfterCommit();
    },
    onError: (err) => {
      if (err instanceof Error && err.message === STIRLING_SESSION_EXPIRED) {
        toast.error(STIRLING_SESSION_EXPIRED);
        clearStirlingSession();
        return;
      }
      if (isAuthRequired(err)) {
        toast.error(toastForStirlingCommitFailure(err));
        return;
      }
      toast.error(toastForStirlingCommitFailure(err));
    },
  });

  const pull = useMutation({
    mutationFn: () => {
      if (!stirlingSession || isStirlingSessionExpired(stirlingSession)) {
        throw Object.assign(new Error(STIRLING_SESSION_EXPIRED), { status: 410 });
      }
      return pullPdfStirlingEdit(docId, stirlingSession.token);
    },
    onSuccess: () => {
      toast.success(STIRLING_APPLY_SUCCESS);
      refreshAfterCommit();
    },
    onError: (err) => {
      if (err instanceof Error && err.message === STIRLING_SESSION_EXPIRED) {
        toast.error(STIRLING_SESSION_EXPIRED);
        clearStirlingSession();
        return;
      }
      toast.error(toastForStirlingCommitFailure(err));
    },
  });

  const applyPending = commit.isPending || pull.isPending;

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
    stirlingSession && stirlingSession.documentId === docId && !isStirlingSessionExpired(stirlingSession)
      ? stirlingSession
      : null;

  return (
    <AppShell
      title={doc.title}
      actions={
        <div className="flex items-center gap-2">
          {statusLabel ? (
            <span
              className={
                saveStatus === "error"
                  ? "text-sm text-danger"
                  : "text-sm text-muted"
              }
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
        <Link to="/documents" search={{}} className="inline-flex items-center gap-1 text-sm text-muted hover:text-fg">
          <ArrowLeft className="size-4" />
          Documents
        </Link>
      </div>
      <div className="grid gap-5 lg:grid-cols-[minmax(0,1.1fr)_minmax(18rem,0.9fr)] lg:items-start">
        <div className="space-y-4">
          {liveSession ? (
            <StirlingApplyPanel
              session={liveSession}
              pending={applyPending}
              onApplyFile={(file) => commit.mutate(file)}
              onPull={liveSession.stirlingUpload === "ok" ? () => pull.mutate() : undefined}
              onReopen={() => {
                if (!resolveStirlingEditHandoff(liveSession)) return;
                const opened = openStirlingEditor(liveSession);
                if (!opened) toast.error(STIRLING_EDIT_HINT_BLOCKED);
              }}
              onOpenFile={() => {
                const opened = openStirlingSourceFile(liveSession.sourceFileUrl);
                if (!opened) toast.error(STIRLING_OPEN_FILE_FAILED);
              }}
              onCancel={clearStirlingSession}
            />
          ) : null}
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
        </div>
        <div className="space-y-4">
          <DocumentFicheForm docId={docId} doc={doc} onSaveStatusChange={onSaveStatusChange} />
        </div>
      </div>
    </AppShell>
  );
}
