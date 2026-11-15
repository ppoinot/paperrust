import { useEffect, useMemo, useRef, type Ref } from "react";
import { toast } from "sonner";
import { commitWordcraft } from "@/lib/paperless/actions";
import { apiUrl } from "@/lib/paperless/api";
import {
  WORDCRAFT_EXPORT_DELAY_MS,
  WORD_OPEN_FAILED,
  WORD_SAVE_FAILED,
  WORD_SAVE_REJECTED,
  wordcraftFrameSrc,
  wordcraftSaveKind,
  type WordcraftCommitResult,
} from "@/lib/paperless/wordcraft-edit";

type WordcraftHostProps = {
  documentId: number;
  filename: string;
  author?: string | null;
  frameRef?: Ref<HTMLIFrameElement>;
  /** Export a PDF once after open when the archive has no rendition yet. */
  autoExport: boolean;
  onDirty?: (dirty: boolean) => void;
  onCommitted?: (result: WordcraftCommitResult) => void;
  onFailed?: () => void;
};

type WordcraftWindow = Window & {
  wordcraft?: {
    execute?: (command: string) => Promise<unknown>;
  };
};

type SavePayload = {
  name: string;
  bytes: ArrayBuffer;
};

function assignRef(ref: Ref<HTMLIFrameElement> | undefined, node: HTMLIFrameElement | null) {
  if (!ref) return;
  if (typeof ref === "function") ref(node);
  else ref.current = node;
}

function bytesOf(value: unknown): ArrayBuffer | null {
  if (value instanceof ArrayBuffer) return value;
  if (ArrayBuffer.isView(value)) {
    const view = value as ArrayBufferView;
    if (!(view.buffer instanceof ArrayBuffer)) return null;
    return view.buffer.slice(view.byteOffset, view.byteOffset + view.byteLength);
  }
  return null;
}

function requestPdfExport(frame: HTMLIFrameElement | null) {
  const win = frame?.contentWindow as WordcraftWindow | null;
  const api = win?.wordcraft;
  const execute = api?.execute;
  if (!api || typeof execute !== "function") return;
  try {
    void Promise.resolve(execute.call(api, "file.exportPdf")).catch(() => undefined);
  } catch {
    /* File › Export PDF posts the same wordcraft:save. */
  }
}

/** Same-origin WordCraft. Saves post `wordcraft:save` back to this page. */
export function WordcraftHost({
  documentId,
  filename,
  author,
  frameRef,
  autoExport,
  onDirty,
  onCommitted,
  onFailed,
}: WordcraftHostProps) {
  const iframeRef = useRef<HTMLIFrameElement>(null);
  const openedRef = useRef(false);
  const autoOnceRef = useRef(false);
  const busyRef = useRef(false);
  const queuedRef = useRef<SavePayload | null>(null);
  const dirtyRef = useRef(false);
  const timersRef = useRef<number[]>([]);
  const openRef = useRef<() => void>(() => {});
  const propsRef = useRef({ documentId, filename, autoExport, onDirty, onCommitted, onFailed });
  propsRef.current = { documentId, filename, autoExport, onDirty, onCommitted, onFailed };
  const src = useMemo(() => wordcraftFrameSrc(author), [author]);

  useEffect(() => {
    const onBeforeUnload = (event: BeforeUnloadEvent) => {
      if (!dirtyRef.current) return;
      event.preventDefault();
      event.returnValue = "";
    };
    window.addEventListener("beforeunload", onBeforeUnload);
    return () => {
      window.removeEventListener("beforeunload", onBeforeUnload);
      for (const id of timersRef.current) window.clearTimeout(id);
      timersRef.current = [];
    };
  }, []);

  useEffect(() => {
    const laterExport = () => {
      const id = window.setTimeout(() => requestPdfExport(iframeRef.current), WORDCRAFT_EXPORT_DELAY_MS);
      timersRef.current.push(id);
    };

    const commitOne = async (payload: SavePayload) => {
      const kind = wordcraftSaveKind(payload.name, new Uint8Array(payload.bytes));
      if (kind === "ignore") return;
      if (kind === "reject") {
        toast.error(WORD_SAVE_REJECTED);
        return;
      }
      const file = new File([payload.bytes], payload.name, {
        type: kind === "rendition" ? "application/pdf" : "application/octet-stream",
      });
      try {
        const result = await commitWordcraft(propsRef.current.documentId, file, payload.name);
        if (result.kind === "ignored") return;
        dirtyRef.current = false;
        propsRef.current.onDirty?.(false);
        propsRef.current.onCommitted?.(result);
        if (result.kind === "original") laterExport();
      } catch {
        toast.error(WORD_SAVE_FAILED);
        propsRef.current.onFailed?.();
      }
    };

    const pump = async (payload: SavePayload) => {
      if (busyRef.current) {
        queuedRef.current = payload;
        return;
      }
      busyRef.current = true;
      try {
        await commitOne(payload);
      } finally {
        busyRef.current = false;
        const next = queuedRef.current;
        queuedRef.current = null;
        if (next) void pump(next);
      }
    };

    const openDocument = async () => {
      if (openedRef.current) return;
      openedRef.current = true;
      const current = propsRef.current;
      try {
        const res = await fetch(apiUrl(`/api/documents/${current.documentId}/file`), {
          credentials: "same-origin",
        });
        if (res.status === 401) {
          window.location.assign("/login");
          return;
        }
        if (!res.ok) {
          openedRef.current = false;
          toast.error(WORD_OPEN_FAILED);
          current.onFailed?.();
          return;
        }
        const bytes = await res.arrayBuffer();
        const win = iframeRef.current?.contentWindow;
        if (!win) {
          openedRef.current = false;
          toast.error(WORD_OPEN_FAILED);
          return;
        }
        const name = current.filename.trim() || "document.docx";
        win.postMessage({ type: "wordcraft:open", name, bytes }, window.location.origin, [bytes]);
        if (current.autoExport && !autoOnceRef.current) {
          autoOnceRef.current = true;
          laterExport();
        }
      } catch {
        openedRef.current = false;
        toast.error(WORD_OPEN_FAILED);
        propsRef.current.onFailed?.();
      }
    };

    const onMessage = (event: MessageEvent) => {
      if (event.origin !== window.location.origin) return;
      if (event.source !== iframeRef.current?.contentWindow) return;
      const data = event.data as { type?: unknown; name?: unknown; bytes?: unknown; dirty?: unknown } | null;
      if (!data || typeof data !== "object") return;
      if (data.type === "wordcraft:ready") {
        void openDocument();
        return;
      }
      if (data.type === "wordcraft:failed") {
        toast.error(WORD_OPEN_FAILED);
        propsRef.current.onFailed?.();
        return;
      }
      if (data.type === "wordcraft:dirty") {
        const dirty = data.dirty !== false;
        dirtyRef.current = dirty;
        propsRef.current.onDirty?.(dirty);
        return;
      }
      if (data.type !== "wordcraft:save") return;
      const bytes = bytesOf(data.bytes);
      if (!bytes) return;
      const name = typeof data.name === "string" && data.name.trim() ? data.name.trim() : "document.docx";
      void pump({ name, bytes });
    };

    openRef.current = () => {
      void openDocument();
    };
    window.addEventListener("message", onMessage);
    const win = iframeRef.current?.contentWindow as WordcraftWindow | null;
    if (win?.wordcraft) void openDocument();
    return () => window.removeEventListener("message", onMessage);
  }, [documentId]);

  return (
    <iframe
      ref={(node) => {
        iframeRef.current = node;
        assignRef(frameRef, node);
      }}
      title="WordCraft"
      src={src}
      referrerPolicy="no-referrer"
      className="h-[80vh] w-full rounded-xl border-0 bg-surface shadow-[var(--shadow-border)]"
      allow="fullscreen; clipboard-read; clipboard-write"
      onLoad={() => {
        const win = iframeRef.current?.contentWindow as WordcraftWindow | null;
        if (win?.wordcraft) openRef.current();
      }}
    />
  );
}
