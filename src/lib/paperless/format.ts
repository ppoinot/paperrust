import { format, parseISO } from "date-fns";
import { fr } from "date-fns/locale";
import type { OcrStatus } from "@/lib/paperless/types";

/** Always French-explicit (e.g. "3 sept. 2026") — never ambiguous US MM/DD. */
export function formatDate(value?: string | null, pattern = "d MMM yyyy"): string {
  if (!value) return "—";
  try {
    const iso = value.length <= 10 ? `${value}T00:00:00` : value.replace(" ", "T").replace(/([+-]\d{2})$/, "$1:00");
    return format(parseISO(iso), pattern, { locale: fr });
  } catch {
    return value;
  }
}

export function formatDateTime(value?: string | null): string {
  return formatDate(value, "d MMM yyyy, HH:mm");
}

/** Canonical OCR / job status wording (Accueil, liste, fiche, journal). */
export const OCR_STATUS_LABEL: Record<OcrStatus, string> = {
  pending: "OCR en attente",
  processing: "OCR en cours",
  done: "OCR terminé",
  skipped: "OCR ignoré",
  failed: "OCR échoué",
};

/**
 * Map document OCR status or consumption-job status to the same FR label.
 * Optional `message` disambiguates job `success` when OCR was skipped (no API key).
 */
export function ocrStatusLabel(status?: string | null, message?: string | null): string {
  if (!status) return "—";
  const msg = message ?? "";
  if (/aucune clé API|clé API pour ce fournisseur/i.test(msg)) {
    return OCR_STATUS_LABEL.skipped;
  }
  if (/clé API Vision invalide|clé API absente|incorrect api key|invalid api key/i.test(msg)) {
    return "OCR : clé API invalide";
  }
  if (status === "ocr_failed") return OCR_STATUS_LABEL.failed;
  if (status === "success") return OCR_STATUS_LABEL.done;
  if (status === "error") return OCR_STATUS_LABEL.failed;
  if (status in OCR_STATUS_LABEL) return OCR_STATUS_LABEL[status as OcrStatus];
  return status;
}

/** Toast / inline: never show raw OCR error codes from the API. */
export function humanizeOcrError(error?: string | null): string {
  const msg = (error ?? "").trim();
  if (!msg || /^ocr_failed$/i.test(msg)) return "OCR échoué.";
  if (/^ocr_auth_error$/i.test(msg)) {
    return "OCR impossible : clé API invalide ou refusée. Vérifiez la clé dans Réglages → OCR.";
  }
  if (/^ocr_rate_limited$/i.test(msg)) {
    return "OCR temporairement indisponible (quota). Réessayez plus tard.";
  }
  if (/^ocr_provider_error$/i.test(msg)) {
    return "OCR échoué : le fournisseur n'a pas pu traiter le document.";
  }
  if (/incorrect api key|invalid api key|invalid_api_key/i.test(msg)) {
    return "OCR impossible : clé API Vision invalide. Vérifiez la clé dans Réglages → OCR.";
  }
  return msg.length > 200 ? `${msg.slice(0, 197)}…` : msg;
}

export function asnLabel(asn?: number | null): string {
  if (!asn) return "—";
  return `#${String(asn).padStart(4, "0")}`;
}

export function dedupeTagsByName<T extends { id: number; name: string; isInboxTag?: boolean }>(tags: T[]): T[] {
  const ranked = [...tags].sort((a, b) => Number(Boolean(b.isInboxTag)) - Number(Boolean(a.isInboxTag)) || a.id - b.id);
  const seen = new Set<string>();
  const keep = new Set<number>();
  for (const t of ranked) {
    const key = t.name.trim().toLowerCase();
    if (seen.has(key)) continue;
    seen.add(key);
    keep.add(t.id);
  }
  return tags.filter((t) => keep.has(t.id));
}

/** French alphabetical order for tag names (base sensitivity). */
export function compareTagNamesFr(a: string, b: string): number {
  return a.localeCompare(b, "fr", { sensitivity: "base" });
}

/**
 * Tag grids / chips: active (checked / assigned) first, then inactive A→Z (fr).
 * Deactivating a tag moves it out of the active block into its alpha place among inactives.
 * With no `activeIds`, sorts the whole list alphabetically (display-only chips).
 */
export function sortTagsForGrid<T extends { id: number; name: string }>(
  tags: T[],
  activeIds?: Iterable<number> | null,
): T[] {
  const active = new Set(activeIds ?? []);
  return [...tags].sort((a, b) => {
    const aRank = active.has(a.id) ? 0 : 1;
    const bRank = active.has(b.id) ? 0 : 1;
    if (aRank !== bRank) return aRank - bRank;
    return compareTagNamesFr(a.name, b.name);
  });
}

export function pickByNameCI<T extends { id: number; name: string }>(items: T[], name: string): T | undefined {
  const key = name.trim().toLowerCase();
  if (!key) return undefined;
  const matches = items.filter((t) => t.name.trim().toLowerCase() === key);
  if (!matches.length) return undefined;
  return matches.reduce((a, b) => (a.id >= b.id ? a : b));
}

export function entityFromPost<T extends { id: number; name: string }>(
  raw: T | T[] | null | undefined,
  name?: string,
): T | undefined {
  if (raw == null) return undefined;
  const list = Array.isArray(raw) ? raw : [raw];
  const items = list.filter(
    (x): x is T => Boolean(x) && typeof x === "object" && typeof (x as T).id === "number",
  );
  if (!items.length) return undefined;
  if (name) return pickByNameCI(items, name) ?? items.reduce((a, b) => (a.id >= b.id ? a : b));
  return items.reduce((a, b) => (a.id >= b.id ? a : b));
}

export type InboxTagLike = {
  isInboxTag?: boolean;
  is_inbox_tag?: boolean;
  name?: string;
  slug?: string;
};

/** Inbox = flagged isInboxTag, or name/slug "inbox" (case-insensitive). */
export function isInboxTag(tag: InboxTagLike): boolean {
  if (tag.isInboxTag || tag.is_inbox_tag) return true;
  const tokens = [tag.slug, tag.name].filter((s): s is string => Boolean(s && String(s).trim()));
  return tokens.some((s) => /^inbox$/i.test(String(s).trim()));
}

