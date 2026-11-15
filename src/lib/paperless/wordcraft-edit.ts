/**
 * Same-origin WordCraft helpers. The iframe stays on `/wordcraft/`.
 * This module does not import the PdfCraft editor.
 */

export const WORD_UPLOAD_REFUSAL =
  "Type de fichier non supporté (PDF, PNG, JPEG ou document Word).";

export const WORD_SAVE_REJECTED = "Ce format n’est pas enregistré dans l’archive.";
export const WORD_SAVE_FAILED = "Impossible d’enregistrer le document.";
export const WORD_OPEN_FAILED = "Impossible d’ouvrir le fichier.";
export const WORD_RENDITION_SAVED = "Aperçu PDF enregistré";
export const WORD_ORIGINAL_SAVED = "Document Word enregistré";
export const WORD_DIRTY_LEAVE =
  "Des modifications ne sont pas enregistrées. Quitter quand même ?";

export const WORD_APPLY_HELP =
  "Enregistrer ici renvoie le document dans Paperrust. L’onglet ouvre le même éditeur.";

/** Wait after `wordcraft:open` before `file.exportPdf`. Same-frame export is blank. */
export const WORDCRAFT_EXPORT_DELAY_MS = 1800;

export const WORD_UPLOAD_ACCEPT = [
  ".pdf",
  ".png",
  ".jpg",
  ".jpeg",
  "application/pdf",
  "image/png",
  "image/jpeg",
  ".docx",
  ".doc",
  ".docm",
  ".dotx",
  ".dotm",
  ".odt",
  ".ott",
  ".fodt",
  ".rtf",
  ".dot",
  "application/msword",
  "application/rtf",
  "application/vnd.oasis.opendocument.text",
].join(",");

const WORD_MIME = new Set([
  "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
  "application/vnd.openxmlformats-officedocument.wordprocessingml.template",
  "application/vnd.ms-word.document.macroenabled.12",
  "application/vnd.ms-word.template.macroenabled.12",
  "application/vnd.oasis.opendocument.text",
  "application/vnd.oasis.opendocument.text-template",
  "application/rtf",
  "text/rtf",
  "application/msword",
]);

const WORD_EXT = new Set([
  ".docx",
  ".docm",
  ".dotx",
  ".dotm",
  ".odt",
  ".ott",
  ".fodt",
  ".rtf",
  ".doc",
  ".dot",
]);

const ORIGINAL_EXT = new Set([
  ".docx",
  ".docm",
  ".dotx",
  ".dotm",
  ".odt",
  ".ott",
  ".fodt",
  ".rtf",
]);

const REJECT_EXT = new Set([
  ".html",
  ".htm",
  ".md",
  ".txt",
  ".tex",
  ".json",
  ".xml",
  ".xlsx",
  ".xls",
  ".pptx",
  ".ppt",
  ".doc",
  ".dot",
]);

const UPLOAD_EXT = new Set([".pdf", ".png", ".jpg", ".jpeg", ...WORD_EXT]);

const UPLOAD_MIME = new Set([
  "application/pdf",
  "image/png",
  "image/jpeg",
  "image/jpg",
  ...WORD_MIME,
]);

export type WordcraftSaveKind = "rendition" | "original" | "ignore" | "reject";

export type WordcraftCommitResult = {
  ok: boolean;
  kind: "original" | "rendition" | "ignored";
  documentId: number;
};

function filenameExt(filename?: string | null): string {
  const name = (filename ?? "").trim().split(/[\\/]/).pop() ?? "";
  const dot = name.lastIndexOf(".");
  if (dot < 0) return "";
  return name.slice(dot).toLowerCase();
}

function mimeOf(value?: string | null): string {
  return (value ?? "").split(";")[0]?.trim().toLowerCase() ?? "";
}

export function isWordDocument(mimeType?: string | null, filename?: string | null): boolean {
  if (WORD_MIME.has(mimeOf(mimeType))) return true;
  return WORD_EXT.has(filenameExt(filename));
}

type WordSaveWindow = {
  wordcraft?: {
    execute?: (command: string) => Promise<unknown>;
  };
  focus?: () => void;
};

/** Ask the editor to save. The bytes come back as `wordcraft:save`, not from this promise. */
export function requestWordSave(win: WordSaveWindow | null | undefined): boolean {
  const api = win?.wordcraft;
  const execute = api?.execute;
  if (!api || typeof execute !== "function") return false;
  try {
    void Promise.resolve(execute.call(api, "file.save")).catch(() => undefined);
  } catch {
    /* The channel exists. A synchronous throw stays inside the editor. */
  }
  return true;
}

/** Full editor on this archive. The hash forces edit mode in the new tab. */
export function wordEditionTabUrl(pageOrigin: string, documentId: number): string {
  const origin = pageOrigin.trim().replace(/\/+$/, "");
  const url = new URL(`/documents/${documentId}`, origin);
  url.hash = "edition";
  if (url.pathname.startsWith("/wordcraft")) {
    throw new Error("word edition tab stays on the document page");
  }
  return url.href;
}

/** Relative iframe URL. The query stays in the browser and is not proxied upstream. */
export function wordcraftFrameSrc(author?: string | null): string {
  const params = new URLSearchParams();
  params.set("host", "parent");
  const name = (author ?? "").trim().slice(0, 200);
  if (name) params.set("author", name);
  return `/wordcraft/?${params.toString()}`;
}

/** French sentence shown before a file enters the upload queue. Null means allowed. */
export function clientUploadRefusal(file: {
  name?: string | null;
  type?: string | null;
}): string | null {
  const ext = filenameExt(file.name);
  const mime = mimeOf(file.type);
  if (ext === ".svg" || mime === "image/svg+xml") return WORD_UPLOAD_REFUSAL;
  if (UPLOAD_EXT.has(ext) || UPLOAD_MIME.has(mime)) return null;
  return WORD_UPLOAD_REFUSAL;
}

function startsWith(bytes: Uint8Array, magic: number[]): boolean {
  if (bytes.length < magic.length) return false;
  for (let i = 0; i < magic.length; i += 1) {
    if (bytes[i] !== magic[i]) return false;
  }
  return true;
}

/** Client pre-filter. The server still classifies the bytes. */
export function wordcraftSaveKind(name: string, bytes: Uint8Array): WordcraftSaveKind {
  if (startsWith(bytes, [0x89, 0x50, 0x4e, 0x47])) return "ignore";
  if (startsWith(bytes, [0xff, 0xd8, 0xff])) return "ignore";
  if (startsWith(bytes, [0x25, 0x50, 0x44, 0x46])) return "rendition";
  const ext = filenameExt(name);
  if (REJECT_EXT.has(ext)) return "reject";
  if (ORIGINAL_EXT.has(ext)) return "original";
  return "reject";
}
