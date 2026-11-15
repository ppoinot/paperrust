/**
 * Same-origin PdfCraft host for the Éditer button.
 *
 * The iframe URL is `{pageOrigin}/pdfcraft/?file=<signed file URL>&host=parent`.
 * Save arrives as `pdfcraft:save` (CustomEvent detail or parent postMessage)
 * and is committed with `commitPdfEdit` (archive + OCR).
 */
import { isWordDocument } from "./wordcraft-edit.ts";

/** Cross-origin atelier is off unless a caller passes an https origin. */
export const PDFCRAFT_ATELIER_ORIGIN = "";

export type PdfCraftFrameProps = {
  src: string;
  referrerPolicy: "no-referrer";
  title: string;
  allowOrigin: string | null;
};

export type PdfCraftSaveDetail = {
  name: string;
  bytes: ArrayBuffer;
};

/**
 * Editor URL on the Paperrust origin. Query key is `file`, never `fileUrl`.
 * `embedded` talks to this page (`host=parent` and `control`). `tab` is a
 * standalone editor: its own Save does not post back here.
 */
export function pdfcraftEditorUrl(
  pageOrigin: string,
  sourceFileUrl: string,
  mode: "embedded" | "tab" = "embedded",
): string {
  const origin = pageOrigin.trim().replace(/\/+$/, "");
  const url = new URL("/pdfcraft/", origin);
  url.searchParams.set("file", sourceFileUrl.trim());
  if (mode === "embedded") {
    url.searchParams.set("host", "parent");
    url.searchParams.set("control", "");
  }
  url.searchParams.delete("fileUrl");
  const href = url.href;
  if (href.includes("fileUrl")) {
    throw new Error("editor URL must not carry fileUrl");
  }
  return href;
}

/**
 * Headers for the signed file response. ACAO is the configured PdfCraft origin
 * and only when the request Origin is exactly that origin. Never `*`. No cookie.
 */
export function fileTokenResponseHeaders(
  configuredOrigin: string,
  requestOrigin: string | null | undefined,
): Headers {
  const headers = new Headers();
  headers.set("referrer-policy", "no-referrer");
  headers.delete("set-cookie");
  const configured = configuredOrigin.trim();
  const request = (requestOrigin ?? "").trim();
  if (!configured || configured === "*" || request !== configured) {
    return headers;
  }
  headers.set("access-control-allow-origin", configured);
  headers.set("vary", "Origin");
  return headers;
}

type EditorControlWindow = {
  pdfcraft?: {
    execute?: (command: string, params?: unknown) => Promise<unknown>;
  };
  focus?: () => void;
};

/**
 * Ask the same-origin editor to save. PdfCraft's `?control` channel exposes
 * `file.save`, which posts `pdfcraft:save` back to this page. Returns false
 * when that channel is absent (older build): the caller focuses the frame.
 */
export function requestEditorSave(win: EditorControlWindow | null | undefined): boolean {
  const api = win?.pdfcraft;
  const execute = api?.execute;
  if (!api || typeof execute !== "function") return false;
  try {
    void Promise.resolve(execute.call(api, "file.save")).catch(() => undefined);
  } catch {
    /* Channel exists. A synchronous throw stays inside the editor. */
  }
  return true;
}

/** What the Éditer iframe actually mounts. Navigation does not send Referer. */
export function pdfcraftFrameProps(
  pageOrigin: string,
  sourceFileUrl: string,
  configuredOrigin: string = PDFCRAFT_ATELIER_ORIGIN,
): PdfCraftFrameProps {
  const cors = fileTokenResponseHeaders(configuredOrigin, configuredOrigin);
  return {
    src: pdfcraftEditorUrl(pageOrigin, sourceFileUrl),
    referrerPolicy: "no-referrer",
    title: "PdfCraft",
    allowOrigin: cors.get("access-control-allow-origin"),
  };
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

function detailFromSave(data: unknown): (PdfCraftSaveDetail & { id?: string }) | null {
  if (!data || typeof data !== "object") return null;
  const record = data as { name?: unknown; bytes?: unknown; type?: unknown; id?: unknown };
  if (record.type != null && record.type !== "pdfcraft:save") return null;
  const bytes = bytesOf(record.bytes);
  if (!bytes) return null;
  const name = typeof record.name === "string" && record.name.trim() ? record.name.trim() : "document.pdf";
  const id = typeof record.id === "string" && record.id.trim() ? record.id.trim() : undefined;
  return { name, bytes, id };
}

/**
 * Listen for PdfCraft's save and pass the bytes to the shipped commit function.
 * Accepts a `pdfcraft:save` CustomEvent (`detail.bytes`) and a same-origin
 * `message` whose `data.type` is `pdfcraft:save`.
 */
function replySaved(source: MessageEventSource, id: string, origin: string): void {
  const poster = source as { postMessage?: (data: unknown, origin?: string) => void };
  if (typeof poster.postMessage !== "function") return;
  try {
    poster.postMessage({ type: "pdfcraft:saved", id }, origin);
  } catch {
    poster.postMessage({ type: "pdfcraft:saved", id });
  }
}

/** Multipart commit. A relative URL stays on this origin. */
async function postPdfCommit(
  documentId: number,
  file: File,
  token?: string,
  filename?: string,
): Promise<unknown> {
  const form = new FormData();
  form.append("file", file, filename?.trim() || file.name || "document.pdf");
  if (token?.trim()) form.append("token", token.trim());
  const fromEnv = (import.meta.env?.VITE_API_URL as string | undefined)?.trim();
  const base = fromEnv ? fromEnv.replace(/\/$/, "") : "";
  const res = await fetch(`${base}/api/documents/${documentId}/edit/pdfcraft/commit`, {
    method: "POST",
    body: form,
    credentials: "same-origin",
  });
  if (!res.ok) {
    const err = new Error("commit_failed") as Error & { status?: number };
    err.status = res.status;
    throw err;
  }
  const ct = res.headers.get("content-type") ?? "";
  if (!ct.includes("json")) return {};
  return res.json();
}

/**
 * Listen for PdfCraft's save and pass the bytes to the shipped commit function.
 * `pdfcraft:saved` is posted only after that commit resolves, and only when the
 * save carried an `id` (the app then clears its dirty flag).
 *
 * A `message` is accepted only from `frameWindow()` on this page's origin.
 * A `pdfcraft:save` CustomEvent is the same-document transport; it is ignored
 * when the listener is attached to `window` (the iframe path uses postMessage).
 */
export function bindPdfCraftSave(
  target: EventTarget,
  session: { documentId: number; token: string },
  onCommitted?: (result: unknown) => void,
  frameWindow?: () => MessageEventSource | null,
  onFailed?: (err: unknown) => void,
  commit: (
    documentId: number,
    file: File,
    token?: string,
    filename?: string,
  ) => Promise<unknown> = postPdfCommit,
): () => void {
  const run = async (detail: PdfCraftSaveDetail & { id?: string }, reply?: () => void) => {
    const file = new File([detail.bytes], detail.name, { type: "application/pdf" });
    try {
      const result = await commit(session.documentId, file, session.token, detail.name);
      reply?.();
      onCommitted?.(result);
    } catch (err) {
      onFailed?.(err);
    }
  };
  const onCustom = (event: Event) => {
    if (typeof window !== "undefined" && (event.currentTarget === window || event.target === window)) return;
    const detail = detailFromSave((event as CustomEvent).detail);
    if (detail) void run(detail);
  };
  const onMessage = (event: Event) => {
    const message = event as MessageEvent;
    const pageOrigin = typeof location === "undefined" ? "" : location.origin;
    if (!pageOrigin || message.origin !== pageOrigin) return;
    const expected = frameWindow?.() ?? null;
    if (!expected || message.source !== expected) return;
    const data = message.data as { type?: unknown } | null;
    if (!data || data.type !== "pdfcraft:save") return;
    const detail = detailFromSave(data);
    if (!detail) return;
    const source = message.source;
    void run(detail, () => {
      if (detail.id && source) replySaved(source, detail.id, pageOrigin);
    });
  };
  target.addEventListener("pdfcraft:save", onCustom);
  target.addEventListener("message", onMessage);
  return () => {
    target.removeEventListener("pdfcraft:save", onCustom);
    target.removeEventListener("message", onMessage);
  };
}

export type PdfEditSession = {
  sessionId: string;
  token: string;
  editorUrl: string;
  sourceFileUrl: string;
  commitUrl: string;
  expiresAt: string;
  filename: string;
  documentId: number;
  mimeType: string;
};

export type PdfEditCommitResult = {
  ok: boolean;
  documentId: number;
  ocr: { ok?: boolean; error?: string };
};

export type DocumentFileEditTarget = "pdfcraft" | "wordcraft" | "m365" | null;

export type PdfEditFailureReason = "not_ready" | "unavailable" | "auth";

export const PDF_EDIT_LABEL = "Éditer";
export const PDF_READ_LABEL = "Lecture";
export const PDF_EDIT_HINT_NOT_PDF = "L’édition n’est disponible que pour les PDF.";
export const PDF_EDIT_HINT_NO_FILE = "Fichier original indisponible.";
export const PDF_EDIT_HINT_BLOCKED = "Impossible d’ouvrir l’édition.";
export const PDF_EDIT_NOT_READY = "L’édition n’est pas encore disponible.";
export const PDF_EDIT_UNAVAILABLE = "Impossible d’ouvrir l’édition.";
export const PDF_APPLY_BANNER_TITLE = "Édition en cours";
export const PDF_APPLY_BANNER_HELP =
  "Enregistrer ici renvoie le PDF dans Paperrust. L’onglet séparé ne le renvoie pas.";
export const PDF_APPLY_OPEN_FILE = "Ouvrir l’édition dans un nouvel onglet";
export const PDF_OPEN_FILE_FAILED = "Impossible d’ouvrir l’édition dans un nouvel onglet.";
export const PDF_APPLY_PRIMARY = "Enregistrer";
export const PDF_APPLY_CANCEL = "Annuler";
export const PDF_APPLY_SUCCESS = "PDF mis à jour, OCR relancé";
export const PDF_APPLY_UNAVAILABLE = "Impossible d’enregistrer le PDF.";
export const PDF_AUTH_REQUIRED = "Authentification requise.";
export const PDF_SESSION_EXPIRED = "La session d’édition a expiré.";
export const M365_EDIT_HINT_DISCONNECTED = "Microsoft 365 n’est pas connecté.";

export const PDF_EDIT_SETTINGS_TOC_ID = "settings-pdf-edit";
export const PDF_EDIT_SETTINGS_HELP =
  "L’édition se fait dans la page. Enregistrer renvoie le PDF dans Paperrust. Aucun jeton n’est stocké dans le navigateur.";

const OFFICE_MIME = new Set([
  "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
  "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
  "application/vnd.openxmlformats-officedocument.presentationml.presentation",
  "application/msword",
  "application/vnd.ms-excel",
  "application/vnd.ms-powerpoint",
  "application/vnd.ms-word",
  "application/vnd.openxmlformats-officedocument.wordprocessingml.template",
  "application/vnd.openxmlformats-officedocument.spreadsheetml.template",
  "application/vnd.openxmlformats-officedocument.presentationml.template",
]);

const OFFICE_EXT = new Set([".docx", ".xlsx", ".pptx", ".doc", ".xls", ".ppt", ".dotx", ".xltx", ".potx"]);

function filenameExt(filename?: string | null): string {
  const name = (filename ?? "").trim().split(/[\\/]/).pop() ?? "";
  const dot = name.lastIndexOf(".");
  if (dot < 0) return "";
  return name.slice(dot).toLowerCase();
}

function asTrimmedString(value: unknown): string | null {
  if (typeof value !== "string") return null;
  const trimmed = value.trim();
  return trimmed ? trimmed : null;
}

export function isPdfDocument(mimeType?: string | null, filename?: string | null): boolean {
  const mime = (mimeType ?? "").trim().toLowerCase();
  if (mime === "application/pdf" || mime.endsWith("/pdf") || mime.includes("pdf")) return true;
  return filenameExt(filename) === ".pdf";
}

export function isOfficeDocument(mimeType?: string | null, filename?: string | null): boolean {
  const mime = (mimeType ?? "").split(";")[0]?.trim().toLowerCase() ?? "";
  if (OFFICE_MIME.has(mime)) return true;
  return OFFICE_EXT.has(filenameExt(filename));
}

export function documentFileEditTarget(
  mimeType?: string | null,
  filename?: string | null,
): DocumentFileEditTarget {
  if (isPdfDocument(mimeType, filename)) return "pdfcraft";
  if (isWordDocument(mimeType, filename)) return "wordcraft";
  if (isOfficeDocument(mimeType, filename)) return "m365";
  return null;
}

export function pdfEditApiPath(documentId: number): string {
  return `/api/documents/${documentId}/edit/pdfcraft`;
}

export function pdfEditCommitApiPath(documentId: number): string {
  return `/api/documents/${documentId}/edit/pdfcraft/commit`;
}

export function isOpenableEditorUrl(raw: string | null | undefined): boolean {
  const value = (raw ?? "").trim();
  if (!value) return false;
  try {
    const url = new URL(value);
    return url.protocol === "https:" || url.protocol === "http:";
  } catch {
    return false;
  }
}

/** Accepts the start-session JSON. Extra fields are ignored. */
export function parsePdfEditSession(data: unknown): PdfEditSession | null {
  if (!data || typeof data !== "object") return null;
  const o = data as Record<string, unknown>;
  const sessionId = asTrimmedString(o.sessionId);
  const token = asTrimmedString(o.token);
  const editorUrl = asTrimmedString(o.editorUrl);
  const sourceFileUrl = asTrimmedString(o.sourceFileUrl);
  const commitUrl = asTrimmedString(o.commitUrl);
  const expiresAt = asTrimmedString(o.expiresAt);
  const filename = asTrimmedString(o.filename);
  const mimeType = asTrimmedString(o.mimeType);
  const documentId =
    typeof o.documentId === "number" && Number.isFinite(o.documentId) ? o.documentId : null;
  if (
    !sessionId ||
    !token ||
    !editorUrl ||
    !sourceFileUrl ||
    !commitUrl ||
    !expiresAt ||
    !filename ||
    documentId == null ||
    !mimeType
  ) {
    return null;
  }
  if (!isOpenableEditorUrl(editorUrl)) return null;
  return {
    sessionId,
    token,
    editorUrl,
    sourceFileUrl,
    commitUrl,
    expiresAt,
    filename,
    documentId,
    mimeType,
  };
}

export function isPdfEditSessionExpired(
  session: Pick<PdfEditSession, "expiresAt">,
  now = Date.now(),
): boolean {
  const t = Date.parse(session.expiresAt);
  return Number.isFinite(t) && t <= now;
}

export function errorStatus(err: unknown): number {
  if (err && typeof err === "object" && "status" in err) {
    return Number((err as { status?: number }).status) || 0;
  }
  return 0;
}

export function isAuthError(err: unknown): boolean {
  return err instanceof Error && err.name === "AuthRequired";
}

export function isPdfEditNotReadyStatus(status: number): boolean {
  return status === 404 || status === 501 || status === 503;
}

export function toastForPdfEditFailure(reason: PdfEditFailureReason): string {
  if (reason === "auth") return PDF_AUTH_REQUIRED;
  return reason === "not_ready" ? PDF_EDIT_NOT_READY : PDF_EDIT_UNAVAILABLE;
}

export function toastForPdfEditCommitFailure(err: unknown): string {
  if (isAuthError(err)) return PDF_AUTH_REQUIRED;
  const status = errorStatus(err);
  if (isPdfEditNotReadyStatus(status)) return PDF_EDIT_NOT_READY;
  return PDF_APPLY_UNAVAILABLE;
}

export function reasonFromHttpStatus(status: number): Exclude<PdfEditFailureReason, "auth"> {
  return isPdfEditNotReadyStatus(status) ? "not_ready" : "unavailable";
}

export function buildM365EditUrl(_params: { documentId: number }): string | null {
  return null;
}

export function isM365Connected(): boolean {
  return false;
}

export function pdfEditDisabledReason(opts: {
  target: DocumentFileEditTarget;
  hasFile: boolean;
}): string | null {
  if (opts.target === "m365") {
    return isM365Connected() ? null : M365_EDIT_HINT_DISCONNECTED;
  }
  if (opts.target === "wordcraft") {
    return opts.hasFile ? null : PDF_EDIT_HINT_NO_FILE;
  }
  if (opts.target !== "pdfcraft") return PDF_EDIT_HINT_NOT_PDF;
  if (!opts.hasFile) return PDF_EDIT_HINT_NO_FILE;
  return null;
}

/** Programmatic `<a target=_blank rel="noopener noreferrer">` — success without a Window. */
export function openUrlViaAnchor(url: string): boolean {
  if (!isOpenableEditorUrl(url) || typeof document === "undefined") return false;
  const a = document.createElement("a");
  a.target = "_blank";
  a.rel = "noopener noreferrer";
  a.href = url;
  if (typeof document.body?.appendChild === "function") {
    document.body.appendChild(a);
  }
  a.click();
  a.remove();
  return true;
}
