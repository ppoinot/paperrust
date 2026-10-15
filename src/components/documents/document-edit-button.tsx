import { useMutation } from "@tanstack/react-query";
import { Loader2, Pencil } from "lucide-react";
import { useRef } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Tooltip } from "@/components/ui/tooltip";
import { isAuthRequired } from "@/lib/auth/auth-required";
import { startPdfStirlingEdit } from "@/lib/paperless/actions";
import {
  STIRLING_EDIT_HINT_BLOCKED,
  STIRLING_EDIT_LABEL,
  STIRLING_SESSION_EXPIRED,
  closeStirlingPopup,
  documentFileEditTarget,
  errorStatus,
  isM365Connected,
  isStirlingSessionExpired,
  navigateStirlingEditorSession,
  openStirlingGesturePopup,
  parseStirlingEditSession,
  reasonFromHttpStatus,
  resolveStirlingEditHandoff,
  stirlingEditDisabledReason,
  toastForStirlingEditFailure,
  toastForStirlingEditStarted,
  type DocumentFileEditTarget,
  type StirlingEditSession,
} from "@/lib/paperless/stirling-edit";

type DocumentEditButtonProps = {
  documentId: number;
  mimeType?: string | null;
  filename?: string | null;
  hasFile: boolean;
  onSessionStarted?: (session: StirlingEditSession) => void;
};

function EditButtonShell({
  disabled,
  hint,
  pending,
  onClick,
}: {
  disabled: boolean;
  hint: string | null;
  pending?: boolean;
  onClick?: () => void;
}) {
  const button = (
    <Button
      type="button"
      variant="outline"
      size="sm"
      disabled={disabled || pending}
      onClick={onClick}
      aria-label={STIRLING_EDIT_LABEL}
    >
      {pending ? <Loader2 className="size-4 animate-spin" /> : <Pencil className="size-4" />}
      {STIRLING_EDIT_LABEL}
    </Button>
  );
  if (!hint) return button;
  return (
    <Tooltip label={hint}>
      <span className="inline-flex">{button}</span>
    </Tooltip>
  );
}

function onM365Click() {
  if (isM365Connected()) {
    /* Office session — not ready. */
    return;
  }
}

export function DocumentEditButton({
  documentId,
  mimeType,
  filename,
  hasFile,
  onSessionStarted,
}: DocumentEditButtonProps) {
  const target: DocumentFileEditTarget = documentFileEditTarget(mimeType, filename);
  const hint = stirlingEditDisabledReason({ target, hasFile });
  const disabled = target === null || hint !== null;
  const gestureRef = useRef<Window | null>(null);

  const abandonGesturePopup = () => {
    closeStirlingPopup(gestureRef.current);
    gestureRef.current = null;
  };

  const stirling = useMutation({
    mutationFn: () => startPdfStirlingEdit(documentId),
    onSuccess: (raw) => {
      const session = parseStirlingEditSession(raw);
      if (!session) {
        abandonGesturePopup();
        toast.error(toastForStirlingEditFailure("not_ready"));
        return;
      }
      if (isStirlingSessionExpired(session)) {
        abandonGesturePopup();
        toast.error(STIRLING_SESSION_EXPIRED);
        return;
      }
      const editorPopup = gestureRef.current;
      gestureRef.current = null;
      if (editorPopup) {
        if (!resolveStirlingEditHandoff(session)) {
          closeStirlingPopup(editorPopup);
        } else if (!navigateStirlingEditorSession(editorPopup, session)) {
          toast.error(STIRLING_EDIT_HINT_BLOCKED);
        } else {
          toast.success(toastForStirlingEditStarted(session.stirlingUpload, undefined, true));
        }
      }
      onSessionStarted?.(session);
    },
    onError: (err) => {
      abandonGesturePopup();
      if (isAuthRequired(err)) {
        toast.error(toastForStirlingEditFailure("auth"));
        return;
      }
      toast.error(toastForStirlingEditFailure(reasonFromHttpStatus(errorStatus(err))));
    },
  });

  const onStirlingClick = () => {
    if (stirling.isPending) return;
    const editorPopup = openStirlingGesturePopup("editor");
    if (!editorPopup) {
      toast.error(STIRLING_EDIT_HINT_BLOCKED);
    }
    gestureRef.current = editorPopup;
    stirling.mutate();
  };

  if (target === null) return null;

  if (target === "m365") {
    return <EditButtonShell disabled={disabled} hint={hint} onClick={onM365Click} />;
  }

  return (
    <EditButtonShell
      disabled={disabled}
      hint={hint}
      pending={stirling.isPending}
      onClick={disabled ? undefined : onStirlingClick}
    />
  );
}
