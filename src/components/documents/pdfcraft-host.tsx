import { useEffect, useRef, type Ref } from "react";
import { commitPdfEdit } from "@/lib/paperless/actions";
import { bindPdfCraftSave, pdfcraftFrameProps } from "@/lib/paperless/pdfcraft-edit";
import type { PdfEditSession } from "@/lib/paperless/pdfcraft-edit";

type PdfCraftHostProps = {
  session: Pick<PdfEditSession, "documentId" | "token" | "sourceFileUrl">;
  pageOrigin: string;
  frameRef?: Ref<HTMLIFrameElement>;
  onCommitted?: (result: unknown) => void;
  onFailed?: (err: unknown) => void;
};

function assignRef(ref: Ref<HTMLIFrameElement> | undefined, node: HTMLIFrameElement | null) {
  if (!ref) return;
  if (typeof ref === "function") ref(node);
  else ref.current = node;
}

/** Same-origin PdfCraft. Enregistrer posts pdfcraft:save back to this page. */
export function PdfCraftHost({ session, pageOrigin, frameRef, onCommitted, onFailed }: PdfCraftHostProps) {
  const frame = pageOrigin ? pdfcraftFrameProps(pageOrigin, session.sourceFileUrl) : null;
  const iframeRef = useRef<HTMLIFrameElement>(null);
  useEffect(() => {
    if (typeof window === "undefined" || !pageOrigin) return;
    return bindPdfCraftSave(
      window,
      { documentId: session.documentId, token: session.token },
      onCommitted,
      () => iframeRef.current?.contentWindow ?? null,
      onFailed,
      commitPdfEdit,
    );
  }, [pageOrigin, session.documentId, session.token, onCommitted, onFailed]);
  if (!frame) return null;
  return (
    <iframe
      ref={(node) => {
        iframeRef.current = node;
        assignRef(frameRef, node);
      }}
      title={frame.title}
      src={frame.src}
      referrerPolicy={frame.referrerPolicy}
      data-pdfcraft-origin={frame.allowOrigin ?? ""}
      className="h-[80vh] w-full rounded-xl border-0 bg-surface shadow-[var(--shadow-border)]"
      allow="fullscreen; clipboard-read; clipboard-write"
    />
  );
}
