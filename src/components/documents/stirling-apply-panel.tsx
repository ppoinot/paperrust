import { FileUp, Loader2 } from "lucide-react";
import { useCallback, useId, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  STIRLING_APPLY_BANNER_TITLE,
  STIRLING_APPLY_CANCEL,
  STIRLING_APPLY_DROP_HELP,
  STIRLING_APPLY_NOT_PDF,
  STIRLING_APPLY_OPEN_FILE,
  STIRLING_APPLY_OPEN_FILE_HELP,
  STIRLING_APPLY_PRIMARY,
  STIRLING_APPLY_PULL,
  STIRLING_APPLY_REOPEN,
  STIRLING_APPLY_SUBMIT,
  bannerHelpForStirlingSession,
  isPdfFile,
  type StirlingEditSession,
} from "@/lib/paperless/stirling-edit";
import { cn } from "@/lib/utils";

type Props = {
  session: StirlingEditSession;
  pending: boolean;
  onApplyFile: (file: File) => void;
  onPull?: () => void;
  onReopen: () => void;
  onOpenFile: () => void;
  onCancel: () => void;
};

function PdfDropzone({
  disabled,
  onFile,
  compact,
}: {
  disabled: boolean;
  onFile: (file: File) => void;
  compact?: boolean;
}) {
  const inputId = useId();
  const inputRef = useRef<HTMLInputElement>(null);
  const [drag, setDrag] = useState(false);
  const [reject, setReject] = useState<string | null>(null);

  const take = useCallback(
    (file: File | undefined) => {
      if (!file || disabled) return;
      if (!isPdfFile(file)) {
        setReject(STIRLING_APPLY_NOT_PDF);
        return;
      }
      setReject(null);
      onFile(file);
    },
    [disabled, onFile],
  );

  return (
    <div className="space-y-2">
      <label
        htmlFor={inputId}
        className={cn(
          "flex cursor-pointer flex-col items-center justify-center gap-2 rounded-xl border border-dashed border-border bg-surface-2/50 px-4 text-center transition-colors",
          compact ? "py-5" : "py-8",
          drag && "border-primary bg-surface-2",
          disabled && "pointer-events-none opacity-50",
        )}
        onDragEnter={(e) => {
          e.preventDefault();
          setDrag(true);
        }}
        onDragOver={(e) => {
          e.preventDefault();
          setDrag(true);
        }}
        onDragLeave={() => setDrag(false)}
        onDrop={(e) => {
          e.preventDefault();
          setDrag(false);
          take(e.dataTransfer.files[0]);
        }}
      >
        <FileUp className="size-5 text-muted" aria-hidden />
        <span className="text-sm text-fg">{STIRLING_APPLY_DROP_HELP}</span>
        <span className="text-xs text-faint">PDF uniquement</span>
        <input
          id={inputId}
          ref={inputRef}
          type="file"
          accept="application/pdf,.pdf"
          className="sr-only"
          disabled={disabled}
          onChange={(e) => {
            take(e.target.files?.[0]);
            e.target.value = "";
          }}
        />
      </label>
      {reject ? (
        <p className="text-sm text-danger" role="alert">
          {reject}
        </p>
      ) : null}
    </div>
  );
}

export function StirlingApplyPanel({
  session,
  pending,
  onApplyFile,
  onPull,
  onReopen,
  onOpenFile,
  onCancel,
}: Props) {
  const [modalOpen, setModalOpen] = useState(false);
  const [picked, setPicked] = useState<File | null>(null);
  const canPull = session.stirlingUpload === "ok" && onPull;
  const bannerHelp = bannerHelpForStirlingSession(session.stirlingUpload);

  const openModal = () => {
    setPicked(null);
    setModalOpen(true);
  };

  return (
    <section
      className="rounded-xl bg-surface p-4 shadow-[var(--shadow-border)]"
      aria-labelledby="stirling-apply-title"
    >
      <div className="space-y-3">
        <div>
          <h2 id="stirling-apply-title" className="font-display text-lg font-medium text-fg">
            {STIRLING_APPLY_BANNER_TITLE}
          </h2>
          <p className="mt-1 text-sm text-muted">{bannerHelp}</p>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <Button type="button" size="sm" onClick={openModal} disabled={pending}>
            {pending ? <Loader2 className="size-4 animate-spin" /> : null}
            {STIRLING_APPLY_PRIMARY}
          </Button>
          <Button type="button" variant="outline" size="sm" onClick={onOpenFile} disabled={pending}>
            {STIRLING_APPLY_OPEN_FILE}
          </Button>
          <Button type="button" variant="outline" size="sm" onClick={onReopen} disabled={pending}>
            {STIRLING_APPLY_REOPEN}
          </Button>
          {canPull ? (
            <Button type="button" variant="outline" size="sm" onClick={onPull} disabled={pending}>
              {pending ? <Loader2 className="size-4 animate-spin" /> : null}
              {STIRLING_APPLY_PULL}
            </Button>
          ) : null}
          <Button type="button" variant="ghost" size="sm" onClick={onCancel} disabled={pending}>
            {STIRLING_APPLY_CANCEL}
          </Button>
        </div>
        <p className="text-xs text-muted">{STIRLING_APPLY_OPEN_FILE_HELP}</p>
        <PdfDropzone compact disabled={pending} onFile={onApplyFile} />
      </div>

      <Dialog open={modalOpen} onOpenChange={setModalOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{STIRLING_APPLY_PRIMARY}</DialogTitle>
            <DialogDescription>{bannerHelp}</DialogDescription>
          </DialogHeader>
          <PdfDropzone
            disabled={pending}
            onFile={(file) => setPicked(file)}
          />
          {picked ? (
            <p className="mt-2 truncate text-sm text-fg" title={picked.name}>
              {picked.name}
            </p>
          ) : null}
          <div className="mt-4 flex flex-wrap justify-end gap-2">
            <Button type="button" variant="ghost" onClick={() => setModalOpen(false)} disabled={pending}>
              {STIRLING_APPLY_CANCEL}
            </Button>
            <Button
              type="button"
              disabled={!picked || pending}
              onClick={() => {
                if (!picked) return;
                onApplyFile(picked);
                setModalOpen(false);
              }}
            >
              {pending ? <Loader2 className="size-4 animate-spin" /> : null}
              {STIRLING_APPLY_SUBMIT}
            </Button>
          </div>
        </DialogContent>
      </Dialog>
    </section>
  );
}
