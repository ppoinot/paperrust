import { Button } from "@/components/ui/button";
import {
  PDF_APPLY_BANNER_TITLE,
  PDF_APPLY_CANCEL,
  PDF_APPLY_OPEN_FILE,
  PDF_APPLY_PRIMARY,
} from "@/lib/paperless/pdfcraft-edit";
import { WORD_APPLY_HELP } from "@/lib/paperless/wordcraft-edit";

type Props = {
  documentId: number;
  onSave: () => void;
  onOpenEditor: () => void;
  onCancel: () => void;
};

/** Same actions as the PDF banner: save, open a tab, cancel. */
export function WordEditApplyPanel({ documentId, onSave, onOpenEditor, onCancel }: Props) {
  return (
    <section
      className="rounded-xl bg-surface p-4 shadow-[var(--shadow-border)]"
      aria-labelledby="word-edit-session-title"
      data-document-id={documentId}
    >
      <div className="space-y-3">
        <div>
          <h2 id="word-edit-session-title" className="font-display text-lg font-medium text-fg">
            {PDF_APPLY_BANNER_TITLE}
          </h2>
          <p className="mt-1 text-sm text-muted">{WORD_APPLY_HELP}</p>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <Button type="button" size="sm" onClick={onSave}>
            {PDF_APPLY_PRIMARY}
          </Button>
          <Button type="button" variant="outline" size="sm" onClick={onOpenEditor}>
            {PDF_APPLY_OPEN_FILE}
          </Button>
          <Button type="button" variant="ghost" size="sm" onClick={onCancel}>
            {PDF_APPLY_CANCEL}
          </Button>
        </div>
      </div>
    </section>
  );
}
