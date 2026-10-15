import { getDocument, GlobalWorkerOptions, TextLayer } from "pdfjs-dist";
import type {
  PDFDocumentLoadingTask,
  PDFDocumentProxy,
  PDFPageProxy,
  RenderTask,
} from "pdfjs-dist";
import workerSrc from "pdfjs-dist/build/pdf.worker.min.mjs?url";
import { useEffect, useRef, useState } from "react";
import { bindTextLayerSelection } from "@/components/documents/pdf-text-layer-selection";

GlobalWorkerOptions.workerSrc = workerSrc;

/**
 * Hard cap for SPA PDF.js page renders (front).
 * Align with backend `preview::MAX_PAGES` (raster / OCR). That backend limit does
 * **not** apply to `/api/documents/:id/file` + pdf.js — the client must enforce.
 */
export const PDF_VIEWER_MAX_PAGES = 50;

/** Wall-clock budget for `getDocument` (network + parse). */
export const PDF_LOAD_TIMEOUT_MS = 45_000;

/** Wall-clock budget for a single page raster + text layer. */
export const PDF_PAGE_RENDER_TIMEOUT_MS = 20_000;

/** Max fully-rendered page canvases / text layers retained in the DOM. */
export const PDF_MAX_LIVE_PAGES = 8;

/**
 * Cap one page's backing-store pixels (MediaBox × scale × DPR) and pdf.js
 * `maxImageSize`. 16 Mpx matches typical Safari canvas limits so a single
 * giant page cannot allocate an unbounded canvas.
 */
export const PDF_MAX_CANVAS_PIXELS = 16_777_216;

class PdfTimeoutError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "PdfTimeoutError";
  }
}

function isAbortLike(err: unknown): boolean {
  if (!err || typeof err !== "object") return false;
  const e = err as { name?: string; message?: string };
  if (e.name === "AbortError" || e.name === "RenderingCancelledException") return true;
  const msg = (e.message ?? "").toLowerCase();
  return msg.includes("abort") || msg.includes("cancel");
}

function safeCancel(fn?: () => void) {
  try {
    fn?.();
  } catch {
    /* ignore */
  }
}

/** Drop a page proxy once it arrives — used when getPage outlives withTimeout. */
function cancelPageWhenReady(pagePromise: Promise<PDFPageProxy>) {
  void pagePromise
    .then((page) => {
      try {
        page.cleanup();
      } catch {
        /* ignore */
      }
    })
    .catch(() => {});
}

function withTimeout<T>(
  promise: Promise<T>,
  ms: number,
  label: string,
  signal?: AbortSignal,
  cancel?: () => void,
): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    let settled = false;
    let timer = 0;
    const onAbort = () => {
      abortWith(new DOMException("Aborted", "AbortError"));
    };
    const finish = (fn: () => void) => {
      if (settled) return;
      settled = true;
      window.clearTimeout(timer);
      signal?.removeEventListener("abort", onAbort);
      fn();
    };
    const abortWith = (err: Error) => {
      finish(() => {
        safeCancel(cancel);
        reject(err);
      });
    };

    timer = window.setTimeout(() => {
      abortWith(new PdfTimeoutError(`${label} (délai dépassé)`));
    }, ms);

    if (signal?.aborted) {
      abortWith(new DOMException("Aborted", "AbortError"));
      return;
    }
    signal?.addEventListener("abort", onAbort, { once: true });
    promise.then(
      (value) => finish(() => resolve(value)),
      (err) => finish(() => reject(err)),
    );
  });
}

/**
 * Fit the page to the host width, then shrink so width×height×DPR² stays
 * under {@link PDF_MAX_CANVAS_PIXELS}.
 */
export function clampPageViewport(page: PDFPageProxy, cssWidth: number, dpr: number) {
  const unscaled = page.getViewport({ scale: 1 });
  const mediaW = Math.max(unscaled.width, 1);
  const mediaH = Math.max(unscaled.height, 1);
  const fitScale = cssWidth / mediaW;
  const areaAtScale1 = mediaW * mediaH * dpr * dpr;
  const capScale =
    areaAtScale1 > 0 ? Math.sqrt(PDF_MAX_CANVAS_PIXELS / areaAtScale1) : 2.25;
  // Cap always wins — do not floor scale (a 0.05 min would blow 16 Mpx on a
  // poster MediaBox).
  const scale = Math.min(2.25, fitScale, capScale);
  return page.getViewport({ scale: Number.isFinite(scale) && scale > 0 ? scale : 0.01 });
}

type LivePage = {
  pageNum: number;
  pageEl: HTMLDivElement;
  canvas: HTMLCanvasElement;
  textLayerDiv: HTMLDivElement;
  unbind?: () => void;
  height: number;
};

/**
 * PDF Aperçu with a real PDF.js text layer so mouse select + copy works
 * on embedded PDF text (not OCR coordinates / JPEG pixels).
 *
 * capped page count, load/render timeouts, destroy on unmount,
 * live-page budget, MediaBox megapixel clamp, disableAutoFetch.
 */
export function DocumentPdfViewer({
  docId,
  title,
  fileRevision,
  onError,
}: {
  docId: number;
  title: string;
  fileRevision?: number;
  onError?: () => void;
}) {
  const hostRef = useRef<HTMLDivElement>(null);
  const [loading, setLoading] = useState(true);
  const [hint, setHint] = useState<string | null>(null);

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;

    const ac = new AbortController();
    const { signal } = ac;
    let loadingTask: PDFDocumentLoadingTask | null = null;
    let pdf: PDFDocumentProxy | null = null;
    const renderTasks = new Set<RenderTask>();
    const textLayers = new Set<TextLayer>();
    const live = new Map<number, LivePage>();
    const placeholders = new Map<number, HTMLDivElement>();
    let io: IntersectionObserver | null = null;
    let renderChain: Promise<void> = Promise.resolve();

    setLoading(true);
    setHint(null);
    host.replaceChildren();

    const cancelRenders = () => {
      for (const task of renderTasks) {
        safeCancel(() => task.cancel());
      }
      renderTasks.clear();
      for (const layer of textLayers) {
        safeCancel(() => layer.cancel());
      }
      textLayers.clear();
    };

    const unloadPage = (pageNum: number) => {
      const livePage = live.get(pageNum);
      if (!livePage) return;
      livePage.unbind?.();
      livePage.unbind = undefined;
      const ctx = livePage.canvas.getContext("2d");
      if (ctx) {
        ctx.clearRect(0, 0, livePage.canvas.width, livePage.canvas.height);
      }
      livePage.canvas.width = 0;
      livePage.canvas.height = 0;
      livePage.pageEl.replaceChildren();
      livePage.pageEl.style.height = `${livePage.height}px`;
      livePage.pageEl.dataset.live = "0";
      live.delete(pageNum);
    };

    const evictIfNeeded = (keepPage: number) => {
      while (live.size >= PDF_MAX_LIVE_PAGES) {
        let farthest: number | null = null;
        let farthestDist = -1;
        for (const n of live.keys()) {
          if (n === keepPage) continue;
          const dist = Math.abs(n - keepPage);
          if (dist > farthestDist) {
            farthestDist = dist;
            farthest = n;
          }
        }
        if (farthest == null) break;
        unloadPage(farthest);
      }
    };

    const renderPage = async (pageNum: number) => {
      if (signal.aborted || !pdf || live.has(pageNum)) return;
      const shell = placeholders.get(pageNum);
      if (!shell) return;

      evictIfNeeded(pageNum);

      let page: PDFPageProxy | undefined;
      let layer: TextLayer | undefined;
      let committed = false;
      const pagePromise = pdf.getPage(pageNum);

      try {
        page = await withTimeout(
          pagePromise,
          PDF_PAGE_RENDER_TIMEOUT_MS,
          `Page ${pageNum}`,
          signal,
          () => cancelPageWhenReady(pagePromise),
        );
        if (signal.aborted) return;

        const width = Math.max(host.clientWidth || 720, 320);
        const dpr = window.devicePixelRatio || 1;
        const viewport = clampPageViewport(page, width, dpr);

        shell.style.width = `${viewport.width}px`;
        shell.style.height = `${viewport.height}px`;
        shell.replaceChildren();

        const canvas = document.createElement("canvas");
        const ctx = canvas.getContext("2d");
        if (!ctx) throw new Error("Canvas indisponible");
        canvas.width = Math.floor(viewport.width * dpr);
        canvas.height = Math.floor(viewport.height * dpr);
        canvas.style.width = `${viewport.width}px`;
        canvas.style.height = `${viewport.height}px`;
        canvas.className = "pdf-page-canvas block max-w-full";
        shell.appendChild(canvas);

        const textLayerDiv = document.createElement("div");
        textLayerDiv.className = "textLayer";
        shell.appendChild(textLayerDiv);

        // Count against the live budget as soon as the canvas exists so a
        // failed raster cannot leak an untracked backing store.
        live.set(pageNum, {
          pageNum,
          pageEl: shell,
          canvas,
          textLayerDiv,
          height: viewport.height,
        });

        const transform = dpr !== 1 ? [dpr, 0, 0, dpr, 0, 0] : undefined;
        const renderTask = page.render({
          canvasContext: ctx,
          viewport,
          transform,
        });
        renderTasks.add(renderTask);
        try {
          await withTimeout(
            renderTask.promise,
            PDF_PAGE_RENDER_TIMEOUT_MS,
            `Rendu page ${pageNum}`,
            signal,
            () => renderTask.cancel(),
          );
        } finally {
          renderTasks.delete(renderTask);
        }
        if (signal.aborted) return;

        const textContent = await withTimeout(
          page.getTextContent({
            includeMarkedContent: true,
            disableNormalization: true,
          }),
          PDF_PAGE_RENDER_TIMEOUT_MS,
          `Texte page ${pageNum}`,
          signal,
          () => {
            try {
              page?.cleanup();
            } catch {
              /* ignore */
            }
          },
        );
        if (signal.aborted) return;

        layer = new TextLayer({
          textContentSource: textContent,
          container: textLayerDiv,
          viewport,
        });
        textLayers.add(layer);
        try {
          await withTimeout(
            layer.render(),
            PDF_PAGE_RENDER_TIMEOUT_MS,
            `Calque page ${pageNum}`,
            signal,
            () => layer?.cancel(),
          );
        } finally {
          textLayers.delete(layer);
        }
        if (signal.aborted) return;

        const livePage = live.get(pageNum);
        if (livePage) {
          livePage.unbind = bindTextLayerSelection(textLayerDiv);
        }
        shell.dataset.live = "1";
        committed = true;
      } catch (err) {
        safeCancel(() => layer?.cancel());
        throw err;
      } finally {
        try {
          page?.cleanup();
        } catch {
          /* ignore */
        }
        if (!committed) {
          unloadPage(pageNum);
        }
      }
    };

    const enqueueRender = (pageNum: number) => {
      renderChain = renderChain
        .then(() => renderPage(pageNum))
        .catch((err) => {
          if (signal.aborted || isAbortLike(err)) return;
          console.warn("[pdf] page render failed", pageNum, err);
        });
    };

    void (async () => {
      try {
        loadingTask = getDocument({
          url:
            fileRevision && fileRevision > 0
              ? `/api/documents/${docId}/file?v=${fileRevision}`
              : `/api/documents/${docId}/file`,
          withCredentials: true,
          isEvalSupported: false,
          // pdf.js: disableAutoFetch only stops unbounded range prefetch
          // when streaming is also off.
          disableStream: true,
          disableAutoFetch: true,
          maxImageSize: PDF_MAX_CANVAS_PIXELS,
          cMapUrl: "/pdfjs/cmaps/",
          cMapPacked: true,
          standardFontDataUrl: "/pdfjs/standard_fonts/",
        });

        const doc = await withTimeout(
          loadingTask.promise,
          PDF_LOAD_TIMEOUT_MS,
          "Chargement du PDF",
          signal,
          () => {
            void loadingTask?.destroy();
          },
        );
        if (signal.aborted) {
          await doc.destroy();
          return;
        }
        pdf = doc;

        const totalPages = doc.numPages;
        const pageLimit = Math.min(totalPages, PDF_VIEWER_MAX_PAGES);
        if (totalPages > PDF_VIEWER_MAX_PAGES && !signal.aborted) {
          setHint(
            `Aperçu limité aux ${PDF_VIEWER_MAX_PAGES} premières pages sur ${totalPages}.`,
          );
        }

        const width = Math.max(host.clientWidth || 720, 320);
        const dpr = window.devicePixelRatio || 1;
        // Size shells from page 1 so we don't open every page up front.
        const firstPromise = doc.getPage(1);
        const first = await withTimeout(
          firstPromise,
          PDF_PAGE_RENDER_TIMEOUT_MS,
          "Page 1",
          signal,
          () => cancelPageWhenReady(firstPromise),
        );
        const viewport = clampPageViewport(first, width, dpr);
        first.cleanup();

        for (let pageNum = 1; pageNum <= pageLimit; pageNum++) {
          if (signal.aborted) break;
          const pageEl = document.createElement("div");
          pageEl.className =
            "pdf-page mx-auto w-full max-w-full rounded-lg bg-white shadow-[var(--shadow-border)]";
          pageEl.style.width = `${viewport.width}px`;
          pageEl.style.height = `${viewport.height}px`;
          pageEl.dataset.page = String(pageNum);
          pageEl.dataset.live = "0";
          pageEl.setAttribute("aria-label", `${title} — page ${pageNum}`);
          host.appendChild(pageEl);
          placeholders.set(pageNum, pageEl);
        }

        if (signal.aborted) return;

        io = new IntersectionObserver(
          (entries) => {
            if (signal.aborted) return;
            for (const entry of entries) {
              const pageNum = Number((entry.target as HTMLElement).dataset.page);
              if (!Number.isFinite(pageNum) || pageNum < 1) continue;
              if (entry.isIntersecting) {
                enqueueRender(pageNum);
              } else if (live.has(pageNum) && live.size > PDF_MAX_LIVE_PAGES) {
                unloadPage(pageNum);
              }
            }
          },
          { root: null, rootMargin: "200px 0px", threshold: 0.01 },
        );

        for (const el of placeholders.values()) {
          io.observe(el);
        }

        // Eagerly warm the first pages so the aperçu isn't blank.
        for (let pageNum = 1; pageNum <= Math.min(2, pageLimit); pageNum++) {
          enqueueRender(pageNum);
        }

        if (!signal.aborted) setLoading(false);
      } catch (err) {
        if (signal.aborted || isAbortLike(err)) return;
        console.warn("[pdf] render failed", err);
        setLoading(false);
        if (err instanceof PdfTimeoutError && !signal.aborted) {
          setHint("Chargement du PDF trop long — bascule sur l’aperçu image.");
        }
        // Abort in-flight getDocument / worker work before falling back.
        try {
          await loadingTask?.destroy();
        } catch {
          /* ignore */
        }
        loadingTask = null;
        try {
          await pdf?.destroy();
        } catch {
          /* ignore */
        }
        pdf = null;
        onError?.();
      }
    })();

    return () => {
      ac.abort();
      io?.disconnect();
      io = null;
      cancelRenders();
      for (const livePage of live.values()) {
        livePage.unbind?.();
      }
      live.clear();
      placeholders.clear();
      host.replaceChildren();
      const task = loadingTask;
      loadingTask = null;
      const doc = pdf;
      pdf = null;
      // Destroy loading task first (aborts network); then drop the doc proxy.
      // Do not terminate GlobalWorkerOptions — it is shared across the SPA.
      void (async () => {
        try {
          await task?.destroy();
        } catch {
          /* ignore */
        }
        try {
          await doc?.destroy();
        } catch {
          /* ignore */
        }
      })();
    };
  }, [docId, title, onError, fileRevision]);

  return (
    <div className="flex flex-col gap-3">
      {loading ? (
        <p className="px-1 text-center text-sm text-muted" role="status">
          Chargement du PDF…
        </p>
      ) : null}
      {hint ? (
        <p className="px-1 text-center text-sm text-muted" role="status">
          {hint}
        </p>
      ) : null}
      <div ref={hostRef} className="flex flex-col items-center gap-3" />
    </div>
  );
}

export function isPdfMime(mimeType?: string | null): boolean {
  const m = (mimeType ?? "").toLowerCase();
  return m === "application/pdf" || m.includes("pdf");
}
