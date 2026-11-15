import { useMutation } from "@tanstack/react-query";
import { Loader2, Pencil } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Tooltip } from "@/components/ui/tooltip";
import { isAuthRequired } from "@/lib/auth/auth-required";
import { startPdfEdit } from "@/lib/paperless/actions";
import {
  PDF_EDIT_LABEL,
  PDF_READ_LABEL,
  PDF_SESSION_EXPIRED,
  documentFileEditTarget,
  errorStatus,
  isPdfEditSessionExpired,
  parsePdfEditSession,
  pdfcraftEditorUrl,
  pdfEditDisabledReason,
  reasonFromHttpStatus,
  toastForPdfEditFailure,
  type DocumentFileEditTarget,
  type PdfEditSession,
} from "@/lib/paperless/pdfcraft-edit";

type DocumentEditButtonProps = {
  documentId: number;
  mimeType?: string | null;
  filename?: string | null;
  hasFile: boolean;
  editing?: boolean;
  sessionOpen?: boolean;
  onShowReading?: () => void;
  onShowEditing?: () => void;
  onSessionStarted?: (session: PdfEditSession) => void;
};

function EditButtonShell({
  disabled,
  hint,
  pending,
  pressed,
  label,
  onClick,
}: {
  disabled: boolean;
  hint: string | null;
  pending?: boolean;
  pressed?: boolean;
  label: string;
  onClick?: () => void;
}) {
  const button = (
    <Button
      type="button"
      variant="outline"
      size="sm"
      disabled={disabled || pending}
      onClick={onClick}
      aria-label={label}
      aria-pressed={pressed}
    >
      {pending ? <Loader2 className="size-4 animate-spin" /> : <Pencil className="size-4" />}
      {label}
    </Button>
  );
  if (!hint) return button;
  return (
    <Tooltip label={hint}>
      <span className="inline-flex">{button}</span>
    </Tooltip>
  );
}

export function DocumentEditButton({
  documentId,
  mimeType,
  filename,
  hasFile,
  editing = false,
  sessionOpen = false,
  onShowReading,
  onShowEditing,
  onSessionStarted,
}: DocumentEditButtonProps) {
  const target: DocumentFileEditTarget = documentFileEditTarget(mimeType, filename);
  const hint = pdfEditDisabledReason({ target, hasFile });
  const disabled = target === null || hint !== null;

  const edit = useMutation({
    mutationFn: () => startPdfEdit(documentId),
    onSuccess: (raw) => {
      const session = parsePdfEditSession(raw);
      if (!session) {
        toast.error(toastForPdfEditFailure("not_ready"));
        return;
      }
      if (isPdfEditSessionExpired(session)) {
        toast.error(PDF_SESSION_EXPIRED);
        return;
      }
      const pageOrigin = typeof window !== "undefined" ? window.location.origin : "";
      if (pageOrigin) {
        const opened = pdfcraftEditorUrl(pageOrigin, session.sourceFileUrl);
        if (!opened.includes("?file=") || opened.includes("fileUrl")) {
          toast.error(toastForPdfEditFailure("unavailable"));
          return;
        }
      }
      onSessionStarted?.(session);
    },
    onError: (err) => {
      if (isAuthRequired(err)) {
        toast.error(toastForPdfEditFailure("auth"));
        return;
      }
      toast.error(toastForPdfEditFailure(reasonFromHttpStatus(errorStatus(err))));
    },
  });

  const onEditClick = () => {
    if (edit.isPending) return;
    if (editing) {
      onShowReading?.();
      return;
    }
    if (sessionOpen) {
      onShowEditing?.();
      return;
    }
    edit.mutate();
  };

  if (target === null || target === "m365") return null;

  const label = editing ? PDF_READ_LABEL : PDF_EDIT_LABEL;

  if (target === "wordcraft") {
    const onWordClick = () => {
      if (editing) onShowReading?.();
      else onShowEditing?.();
    };
    return (
      <EditButtonShell
        disabled={disabled}
        hint={editing ? null : hint}
        pressed={editing}
        label={label}
        onClick={disabled ? undefined : onWordClick}
      />
    );
  }

  return (
    <EditButtonShell
      disabled={disabled}
      hint={editing ? null : hint}
      pending={edit.isPending}
      pressed={editing}
      label={label}
      onClick={disabled ? undefined : onEditClick}
    />
  );
}
