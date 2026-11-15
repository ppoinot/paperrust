import { Loader2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  PDF_APPLY_BANNER_HELP,
  PDF_APPLY_BANNER_TITLE,
  PDF_APPLY_CANCEL,
  PDF_APPLY_OPEN_FILE,
  PDF_APPLY_PRIMARY,
  type PdfEditSession,
} from "@/lib/paperless/pdfcraft-edit";

type Props = {
  session: PdfEditSession;
  pending: boolean;
  onSave: () => void;
  onOpenEditor: () => void;
  onCancel: () => void;
};

export function PdfEditApplyPanel({ session, pending, onSave, onOpenEditor, onCancel }: Props) {
  return (
    <section
      className="rounded-xl bg-surface p-4 shadow-[var(--shadow-border)]"
      aria-labelledby="edit-session-title"
      data-document-id={session.documentId}
    >
      <div className="space-y-3">
        <div>
          <h2 id="edit-session-title" className="font-display text-lg font-medium text-fg">
            {PDF_APPLY_BANNER_TITLE}
          </h2>
          <p className="mt-1 text-sm text-muted">{PDF_APPLY_BANNER_HELP}</p>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <Button type="button" size="sm" onClick={onSave} disabled={pending}>
            {pending ? <Loader2 className="size-4 animate-spin" /> : null}
            {PDF_APPLY_PRIMARY}
          </Button>
          <Button type="button" variant="outline" size="sm" onClick={onOpenEditor} disabled={pending}>
            {PDF_APPLY_OPEN_FILE}
          </Button>
          <Button type="button" variant="ghost" size="sm" onClick={onCancel} disabled={pending}>
            {PDF_APPLY_CANCEL}
          </Button>
        </div>
      </div>
    </section>
  );
}
