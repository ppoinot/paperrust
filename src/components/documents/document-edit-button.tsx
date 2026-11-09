import { useMutation } from "@tanstack/react-query";
import { Loader2, Pencil } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Tooltip } from "@/components/ui/tooltip";
import { isAuthRequired } from "@/lib/auth/auth-required";
import { startPdfStirlingEdit } from "@/lib/paperless/actions";
import {
  STIRLING_CONFIG_CHECKING,
  STIRLING_EDIT_HINT_BLOCKED,
  STIRLING_EDIT_LABEL,
  STIRLING_EDIT_STARTED_NO_FILEURL,
  STIRLING_SESSION_EXPIRED,
  configuredStirlingOpenHref,
  documentFileEditTarget,
  errorStatus,
  isM365Connected,
  isStirlingSessionExpired,
  openStirlingTab,
  parseStirlingEditSession,
  loadStirlingBaseUrl,
  reasonFromHttpStatus,
  stirlingConfiguredFrom,
  stirlingEditDisabledReason,
  stirlingPublicHref,
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
    /* Office edit is not available yet. */
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
  const [stirlingConfigured, setStirlingConfigured] = useState<boolean | null>(null);
  useEffect(() => {
    if (target !== "stirling") return;
    let cancelled = false;
    void loadStirlingBaseUrl().then((url) => {
      if (!cancelled) setStirlingConfigured(stirlingConfiguredFrom(url));
    });
    return () => {
      cancelled = true;
    };
  }, [target]);
  const hint =
    target === "stirling" && stirlingConfigured === null
      ? STIRLING_CONFIG_CHECKING
      : stirlingEditDisabledReason({
          target,
          hasFile,
          stirlingConfigured: stirlingConfigured === false ? false : undefined,
        });
  const disabled = target === null || hint !== null;
  const openedRef = useRef(false);

  const stirling = useMutation({
    mutationFn: () => startPdfStirlingEdit(documentId),
    onSuccess: (raw) => {
      const session = parseStirlingEditSession(raw);
      if (!session) {
        toast.error(toastForStirlingEditFailure("not_ready"));
        return;
      }
      if (isStirlingSessionExpired(session)) {
        toast.error(STIRLING_SESSION_EXPIRED);
        return;
      }
      if (openedRef.current) {
        const href = stirlingPublicHref(session.editorUrl, [session.token, session.sessionId]);
        toast.success(
          href
            ? toastForStirlingEditStarted(session.stirlingUpload, href, false)
            : STIRLING_EDIT_STARTED_NO_FILEURL,
        );
      }
      onSessionStarted?.(session);
    },
    onError: (err) => {
      if (isAuthRequired(err)) {
        toast.error(toastForStirlingEditFailure("auth"));
        return;
      }
      toast.error(toastForStirlingEditFailure(reasonFromHttpStatus(errorStatus(err))));
    },
  });

  const onStirlingClick = () => {
    if (stirling.isPending) return;
    const href = configuredStirlingOpenHref();
    openedRef.current = Boolean(href && openStirlingTab(href));
    if (!openedRef.current) {
      toast.error(STIRLING_EDIT_HINT_BLOCKED);
    }
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
