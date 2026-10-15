/**
 * PDF Edit → Stirling.
 *
 * Start: POST `/api/documents/:id/edit/stirling` → `StirlingEditSession`.
 * Commit: cookie multipart (or `{pull:true}` when `stirlingUpload === "ok"`).
 * Stirling does not POST back — the return path is the Paperrust fiche only.
 *
 * Happy path: one Stirling tab at `editorUrl` (the API sets
 * `?fileUrl=` so Stirling loads the PDF). `sourceFileUrl` is banner
 * fallback only — never the primary Éditer navigation.
 */

import { trimOrigin, viteStirlingBaseUrl } from "../deploy-env.ts";

export type StirlingUploadStatus = "skipped" | "ok" | "failed";

export type StirlingEditSession = {
  sessionId: string;
  token: string;
  editorUrl: string;
  sourceFileUrl: string;
  commitUrl: string;
  expiresAt: string;
  filename: string;
  documentId: number;
  mimeType: string;
  stirlingUpload: StirlingUploadStatus;
};

export type StirlingCommitResult = {
  ok: boolean;
  documentId: number;
  ocr: { ok?: boolean; error?: string };
};

export const STIRLING_EDIT_LABEL = "Éditer";
export const STIRLING_EDIT_HINT_NOT_PDF = "L’édition Stirling n’est disponible que pour les PDF.";
export const STIRLING_EDIT_HINT_NO_FILE = "Fichier original indisponible.";
export const STIRLING_EDIT_HINT_BLOCKED = "Impossible d’ouvrir Stirling PDF.";
export const STIRLING_EDIT_NOT_READY = "L’édition Stirling n’est pas encore disponible.";
export const STIRLING_EDIT_UNAVAILABLE = "Impossible d’ouvrir l’édition Stirling.";
export const STIRLING_EDIT_STARTED =
  "Stirling ouvert — le PDF s’ouvre dans un onglet ; déposez-le dans Stirling si besoin.";
export const STIRLING_EDIT_STARTED_LOADED =
  "Stirling ouvert — le PDF est chargé dans l’éditeur.";
export const STIRLING_EDIT_STARTED_UPLOADED =
  "Stirling ouvert — le fichier peut déjà être dans Stirling. Sinon, utilisez « Ouvrir le fichier ».";
export const STIRLING_EDIT_STARTED_NO_FILEURL =
  "Stirling ouvert — le PDF n’est pas chargé automatiquement. Utilisez « Ouvrir le fichier ».";
export const STIRLING_APPLY_BANNER_TITLE = "Édition Stirling en cours";
export const STIRLING_APPLY_BANNER_HELP =
  "Enregistrez le PDF dans Stirling, puis appliquez-le ici.";
export const STIRLING_APPLY_BANNER_HELP_UPLOADED =
  "Le fichier peut déjà être dans Stirling. Sinon, ouvrez-le et déposez-le, puis appliquez-le ici.";
export const STIRLING_APPLY_OPEN_FILE = "Ouvrir le fichier";
export const STIRLING_APPLY_OPEN_FILE_HELP =
  "Le PDF s’ouvre dans un onglet : déposez-le dans Stirling si besoin.";
export const STIRLING_OPEN_FILE_FAILED = "Impossible d’ouvrir le fichier PDF.";
export const STIRLING_APPLY_PRIMARY = "Appliquer dans Paperrust";
export const STIRLING_APPLY_REOPEN = "Rouvrir Stirling";
export const STIRLING_APPLY_CANCEL = "Annuler";
export const STIRLING_APPLY_SUBMIT = "Appliquer";
export const STIRLING_APPLY_PULL = "Récupérer depuis Stirling";
export const STIRLING_APPLY_DROP_HELP = "Déposez le PDF enregistré, ou parcourez.";
export const STIRLING_APPLY_NOT_PDF = "Seuls les fichiers PDF sont acceptés.";
export const STIRLING_APPLY_SUCCESS = "PDF mis à jour, OCR relancé";
export const STIRLING_APPLY_UNAVAILABLE = "Impossible d’appliquer le PDF.";
export const STIRLING_AUTH_REQUIRED = "Authentification requise.";
export const STIRLING_SESSION_EXPIRED = "La session d’édition a expiré.";
export const M365_EDIT_HINT_DISCONNECTED = "Microsoft 365 n’est pas connecté.";

/** localStorage override (Réglages). Empty → runtime `/api/config`, then dev Vite. */
const STIRLING_BASE_STORAGE_KEY = "paperrust.stirlingBaseUrl";

export const STIRLING_SETTINGS_TOC_ID = "settings-stirling";
export const STIRLING_SETTINGS_HELP =
  "Adresse de Stirling PDF. Laissez vide pour utiliser l'adresse servie par cette archive. L'ouverture passe par le serveur ; aucun jeton n'est stocké ici.";

let runtimeStirlingUrl = "";

/** Stirling base from `GET /api/config` (`PAPERRUST_STIRLING_URL*`). */
export async function loadStirlingBaseUrl(): Promise<string> {
  try {
    const res = await fetch("/api/config", { credentials: "same-origin" });
    if (!res.ok) return runtimeStirlingUrl;
    const body = (await res.json()) as { stirlingUrl?: string | null };
    runtimeStirlingUrl = trimOrigin(body.stirlingUrl ?? "");
    return runtimeStirlingUrl;
  } catch {
    return runtimeStirlingUrl;
  }
}

export function cachedStirlingBaseUrl(): string {
  return runtimeStirlingUrl;
}


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
const UPLOAD_STATUSES: readonly StirlingUploadStatus[] = ["skipped", "ok", "failed"];

export type DocumentFileEditTarget = "stirling" | "m365" | null;

export type StirlingFailureReason = "not_ready" | "unavailable" | "auth";

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

export function isPdfFile(file: File): boolean {
  return isPdfDocument(file.type, file.name);
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
  if (isPdfDocument(mimeType, filename)) return "stirling";
  if (isOfficeDocument(mimeType, filename)) return "m365";
  return null;
}


export function readStirlingBaseUrlOverride(): string {
  if (typeof localStorage === "undefined") return "";
  try {
    return (localStorage.getItem(STIRLING_BASE_STORAGE_KEY) ?? "").trim();
  } catch {
    return "";
  }
}

export function writeStirlingBaseUrlOverride(value: string): void {
  if (typeof localStorage === "undefined") return;
  try {
    const trimmed = value.trim();
    if (!trimmed) localStorage.removeItem(STIRLING_BASE_STORAGE_KEY);
    else localStorage.setItem(STIRLING_BASE_STORAGE_KEY, trimmed);
  } catch {
    /* ignore quota / private mode */
  }
}

/** Display / default only — the open URL always comes from `editorUrl`. */
export function resolveStirlingBaseUrl(): string {
  const override = readStirlingBaseUrlOverride();
  if (override) return override.replace(/\/+$/, "");
  if (runtimeStirlingUrl) return runtimeStirlingUrl;
  return viteStirlingBaseUrl();
}

export function stirlingEditApiPath(documentId: number): string {
  return `/api/documents/${documentId}/edit/stirling`;
}

export function stirlingCommitApiPath(documentId: number): string {
  return `/api/documents/${documentId}/edit/stirling/commit`;
}

export function isStirlingEditNotReadyStatus(status: number): boolean {
  return status === 404 || status === 501 || status === 503;
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

function fileUrlFromHash(hash: string): string {
  const raw = hash.startsWith("#") ? hash.slice(1) : hash;
  if (!raw.startsWith("fileUrl=")) return "";
  try {
    return decodeURIComponent(raw.slice("fileUrl=".length)).trim();
  } catch {
    return raw.slice("fileUrl=".length).trim();
  }
}

/** True when the URL has a non-empty `fileUrl` in search or `#fileUrl=`. */
export function hasFileUrl(raw: string | null | undefined): boolean {
  const value = (raw ?? "").trim();
  if (!value) return false;
  try {
    const url = new URL(value);
    if (url.protocol !== "https:" && url.protocol !== "http:") return false;
    const fromSearch = url.searchParams.get("fileUrl");
    if (typeof fromSearch === "string" && fromSearch.trim().length > 0) return true;
    return fileUrlFromHash(url.hash).length > 0;
  } catch {
    return false;
  }
}

/** Alias — search `fileUrl` only (legacy callers / tests). */
export function editorUrlHasFileUrl(raw: string | null | undefined): boolean {
  const value = (raw ?? "").trim();
  if (!value) return false;
  try {
    const url = new URL(value);
    if (url.protocol !== "https:" && url.protocol !== "http:") return false;
    const fileUrl = url.searchParams.get("fileUrl");
    return typeof fileUrl === "string" && fileUrl.trim().length > 0;
  } catch {
    return false;
  }
}

/** Root Stirling landing with empty search and no fileUrl — must not be opened. */
export function isBareStirlingLanding(raw: string | null | undefined): boolean {
  const value = (raw ?? "").trim();
  if (!isOpenableEditorUrl(value) || hasFileUrl(value)) return false;
  try {
    const url = new URL(value);
    const root = url.pathname === "/" || url.pathname === "";
    const emptySearch = url.search === "" || url.search === "?";
    return root && emptySearch;
  } catch {
    return false;
  }
}

function resolvedFileUrlValue(
  editorUrl: string,
  sourceFileUrl?: string | null,
): string | null {
  try {
    const fromSearch = new URL(editorUrl).searchParams.get("fileUrl");
    if (typeof fromSearch === "string" && fromSearch.trim()) return fromSearch.trim();
  } catch {
    /* keep looking */
  }
  const source = (sourceFileUrl ?? "").trim();
  return isOpenableEditorUrl(source) ? source : null;
}

/**
 * Stirling href is origin+pathname only (empty search/hash). Primary carrier
 * is `window.name` (`paperrustEdit=` JSON). Never put fileUrl or commitUrl
 * on the Stirling query or hash.
 */
export function resolveEditorNavigateUrl(
  editorUrl: string,
  sourceFileUrl?: string | null,
): string {
  const editor = (editorUrl ?? "").trim();
  if (!isOpenableEditorUrl(editor)) return editor;

  const value = resolvedFileUrlValue(editor, sourceFileUrl);
  if (!value) return editor;

  try {
    const url = new URL(editor);
    return `${url.origin}${url.pathname || "/"}`;
  } catch {
    return editor;
  }
}

/** fileUrl value from API `editorUrl` search, else `sourceFileUrl` if http(s). */
export function resolveStirlingFileUrlValue(
  editorUrl: string,
  sourceFileUrl?: string | null,
): string | null {
  return resolvedFileUrlValue(editorUrl, sourceFileUrl);
}

export type StirlingEditHandoff = {
  fileUrl: string;
  commitUrl: string;
};

export function resolveStirlingEditHandoff(
  session: Pick<StirlingEditSession, "editorUrl" | "sourceFileUrl" | "commitUrl">,
): StirlingEditHandoff | null {
  const fileUrl = resolveStirlingFileUrlValue(session.editorUrl, session.sourceFileUrl);
  const commitUrl = (session.commitUrl ?? "").trim();
  if (!fileUrl || !isOpenableEditorUrl(commitUrl)) return null;
  return { fileUrl, commitUrl };
}

/**
 * Absolute http(s) as-is. Relative paths resolve against the page origin only
 * (no invented host). Rejects javascript:/data: and relatives without a window.
 */
export function resolveSourceFileUrl(raw: string | null | undefined): string | null {
  const value = (raw ?? "").trim();
  if (!value) return null;
  if (isOpenableEditorUrl(value)) {
    try {
      return new URL(value).href;
    } catch {
      return null;
    }
  }
  if (typeof window === "undefined") return null;
  const origin = window.location?.origin;
  if (!origin || origin === "null") return null;
  try {
    const resolved = new URL(value, origin);
    return isOpenableEditorUrl(resolved.href) ? resolved.href : null;
  } catch {
    return null;
  }
}

export function parseStirlingEditSession(data: unknown): StirlingEditSession | null {
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
  const stirlingUpload = UPLOAD_STATUSES.includes(o.stirlingUpload as StirlingUploadStatus)
    ? (o.stirlingUpload as StirlingUploadStatus)
    : null;
  if (
    !sessionId ||
    !token ||
    !editorUrl ||
    !sourceFileUrl ||
    !commitUrl ||
    !expiresAt ||
    !filename ||
    documentId == null ||
    !mimeType ||
    !stirlingUpload
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
    stirlingUpload,
  };
}

export function isStirlingSessionExpired(
  session: Pick<StirlingEditSession, "expiresAt">,
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

export function toastForStirlingEditFailure(reason: StirlingFailureReason): string {
  if (reason === "auth") return STIRLING_AUTH_REQUIRED;
  return reason === "not_ready" ? STIRLING_EDIT_NOT_READY : STIRLING_EDIT_UNAVAILABLE;
}

export function toastForStirlingEditStarted(
  upload: StirlingUploadStatus,
  editorUrl?: string,
  handedOff?: boolean,
): string {
  // Gate on handoff success or navigated URL (search / #fileUrl=). Origin+pathname
  // navigate alone has no fileUrl in href — callers pass handedOff after name stamp.
  if (handedOff || (editorUrl != null && hasFileUrl(editorUrl))) {
    return upload === "ok" ? STIRLING_EDIT_STARTED_UPLOADED : STIRLING_EDIT_STARTED_LOADED;
  }
  if (editorUrl != null) {
    return STIRLING_EDIT_STARTED_NO_FILEURL;
  }
  return upload === "ok" ? STIRLING_EDIT_STARTED_UPLOADED : STIRLING_EDIT_STARTED;
}

export function bannerHelpForStirlingSession(upload: StirlingUploadStatus): string {
  return upload === "ok" ? STIRLING_APPLY_BANNER_HELP_UPLOADED : STIRLING_APPLY_BANNER_HELP;
}

export function toastForStirlingCommitFailure(err: unknown): string {
  if (isAuthError(err)) return STIRLING_AUTH_REQUIRED;
  const status = errorStatus(err);
  if (isStirlingEditNotReadyStatus(status)) return STIRLING_EDIT_NOT_READY;
  return STIRLING_APPLY_UNAVAILABLE;
}

export function reasonFromHttpStatus(status: number): Exclude<StirlingFailureReason, "auth"> {
  return isStirlingEditNotReadyStatus(status) ? "not_ready" : "unavailable";
}

/** M365 edit for Office types — session not ready. */
export function buildM365EditUrl(_params: { documentId: number }): string | null {
  return null;
}

export function isM365Connected(): boolean {
  return false;
}

export function stirlingEditDisabledReason(opts: {
  target: DocumentFileEditTarget;
  hasFile: boolean;
}): string | null {
  if (opts.target === "m365") {
    return isM365Connected() ? null : M365_EDIT_HINT_DISCONNECTED;
  }
  if (opts.target !== "stirling") return STIRLING_EDIT_HINT_NOT_PDF;
  if (!opts.hasFile) return STIRLING_EDIT_HINT_NO_FILE;
  return null;
}

export function closeStirlingPopup(popup: Window | null | undefined): void {
  if (!popup) return;
  try {
    popup.close();
  } catch {
    /* ignore */
  }
}

/** User-gesture `about:blank` with `opener` detached (Éditer / « Ouvrir le fichier »). */
export function openBlankEditorWindow(): Window | null {
  if (typeof window === "undefined") return null;
  const popup = window.open("about:blank", "_blank");
  if (!popup) return null;
  try {
    popup.opener = null;
  } catch {
    /* ignore */
  }
  return popup;
}

/**
 * Gesture-time `about:blank` (!11). Editor detaches `opener`. A leftover
 * file-kind blank keeps opener for tests; Éditer never opens that second tab.
 */
export function openStirlingGesturePopup(kind: "editor" | "file"): Window | null {
  if (kind === "editor") return openBlankEditorWindow();
  if (typeof window === "undefined") return null;
  return window.open("about:blank", "_blank");
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

/** `window.name` prefix; payload is JSON `{ fileUrl, commitUrl }`. */
export const PAPERRUST_EDIT_WINDOW_NAME_PREFIX = "paperrustEdit=";

/** @deprecated prefix — kept so older handoffs still resolve. */
export const PAPERRUST_FILEURL_WINDOW_NAME_PREFIX = "paperrustFileUrl=";

/** Extract `fileUrl` from a navigate href (search first, then `#fileUrl=`). */
export function fileUrlFromNavigateUrl(url: string): string | null {
  try {
    const parsed = new URL(url);
    const fromSearch = parsed.searchParams.get("fileUrl");
    if (typeof fromSearch === "string" && fromSearch.trim()) return fromSearch.trim();
    const fromHash = fileUrlFromHash(parsed.hash);
    return fromHash.length > 0 ? fromHash : null;
  } catch {
    return null;
  }
}

/**
 * The editor reads `window.name` prefix `paperrustEdit=` → JSON
 * `{ fileUrl, commitUrl }` (both http(s)). Token stays inside commitUrl path.
 * Never put commitUrl on the Stirling query or hash.
 */
export function stirlingWindowNameForEdit(handoff: StirlingEditHandoff): string {
  return `${PAPERRUST_EDIT_WINDOW_NAME_PREFIX}${encodeURIComponent(
    JSON.stringify({ fileUrl: handoff.fileUrl, commitUrl: handoff.commitUrl }),
  )}`;
}

export function stampPaperrustEditName(popup: Window, handoff: StirlingEditHandoff): boolean {
  if (!isOpenableEditorUrl(handoff.fileUrl) || !isOpenableEditorUrl(handoff.commitUrl)) return false;
  try {
    popup.name = stirlingWindowNameForEdit(handoff);
    return popup.name.startsWith(PAPERRUST_EDIT_WINDOW_NAME_PREFIX);
  } catch {
    return false;
  }
}

/**
 * After await: `location.replace(fullUrl)` on a gesture-opened blank.
 * Full resolved string only — not `location =`, not pathname/search assigned
 * separately. Editor name stamp lives in `navigateStirlingEditorSession`.
 */
export function navigateStirlingPopup(popup: Window, url: string): boolean {
  if (!isOpenableEditorUrl(url)) {
    closeStirlingPopup(popup);
    return false;
  }
  try {
    popup.location.replace(url);
    return true;
  } catch {
    closeStirlingPopup(popup);
    return false;
  }
}

/**
 * Name handoff then `location.replace(origin+pathname)`.
 * If fileUrl or commitUrl is missing: close popup (no fake success).
 */
export function navigateStirlingEditorSession(
  popup: Window,
  session: Pick<StirlingEditSession, "editorUrl" | "sourceFileUrl" | "commitUrl">,
): boolean {
  const handoff = resolveStirlingEditHandoff(session);
  if (!handoff) {
    closeStirlingPopup(popup);
    return false;
  }
  const replaceHref = resolveEditorNavigateUrl(session.editorUrl, session.sourceFileUrl);
  if (
    !isOpenableEditorUrl(replaceHref) ||
    /[?#].*commitUrl/.test(replaceHref) ||
    !stampPaperrustEditName(popup, handoff)
  ) {
    closeStirlingPopup(popup);
    return false;
  }
  try {
    console.info("[paperrust-edit]", {
      replaceHref,
      nameSet: true,
      namePrefix: "paperrustEdit=",
      hasCommitUrl: true,
    });
  } catch {
    /* ignore */
  }
  return navigateStirlingPopup(popup, replaceHref);
}

/** Resolved navigate URL for toasts / callers (same as navigate path). */
export function editorNavigateUrlForSession(
  session: Pick<StirlingEditSession, "editorUrl" | "sourceFileUrl" | "commitUrl">,
): string {
  return resolveEditorNavigateUrl(session.editorUrl, session.sourceFileUrl);
}

/**
 * Open Stirling without leaking `window.opener`:
 * sync `about:blank` → `opener = null` → name handoff → `location.replace`.
 * Requires fileUrl + commitUrl.
 */
export function openStirlingEditor(
  session: Pick<StirlingEditSession, "editorUrl" | "sourceFileUrl" | "commitUrl">,
): Window | null {
  if (!resolveStirlingEditHandoff(session)) return null;
  const popup = openStirlingGesturePopup("editor");
  if (!popup) return null;
  if (!navigateStirlingEditorSession(popup, session)) return null;
  return popup;
}

/**
 * Banner fallback: open `sourceFileUrl` on a user click (« Ouvrir le fichier »).
 * Returns whether the open was attempted successfully.
 * Never treats `window.open(..., "noopener")` null as failure.
 */
export function openStirlingSourceFile(
  sourceFileUrl: string,
  popup?: Window | null,
): boolean {
  const url = resolveSourceFileUrl(sourceFileUrl);
  if (!url || !isOpenableEditorUrl(url)) return false;
  if (popup) {
    return navigateStirlingPopup(popup, url);
  }
  const blank = openBlankEditorWindow();
  if (blank) {
    return navigateStirlingPopup(blank, url);
  }
  return openUrlViaAnchor(url);
}

export const readStoredStirlingBaseUrl = readStirlingBaseUrlOverride;
export const writeStoredStirlingBaseUrl = writeStirlingBaseUrlOverride;
