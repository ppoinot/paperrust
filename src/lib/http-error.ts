/**
 * File cap — keep in sync with `limits::MAX_UPLOAD_BYTES`.
 * Cloudflare Free/Pro refuses a proxied body above 100 MB. The byte cap leaves
 * 64 KiB for the multipart wrapper (`MULTIPART_OVERHEAD_BYTES`).
 */
export const MAX_UPLOAD_BYTES = 100_000_000 - 64 * 1024;
export const MAX_UPLOAD_LABEL = "100 Mo";

export function looksLikeHtml(body: string, contentType?: string | null): boolean {
  const ct = (contentType ?? "").toLowerCase();
  if (ct.includes("text/html")) return true;
  const head = body.trimStart().slice(0, 256).toLowerCase();
  return (
    head.startsWith("<!doctype html") ||
    head.startsWith("<html") ||
    head.includes("<title>") ||
    /nginx/i.test(head)
  );
}

/**
 * Map HTTP failures (incl. nginx HTML pages) to short French copy.
 * Never returns raw HTML suitable for a toast.
 */
export function friendlyHttpError(
  status: number,
  body = "",
  contentType?: string | null,
): string {
  if (status === 413) return `Fichier trop volumineux (limite ${MAX_UPLOAD_LABEL}).`;
  if (status === 502 || status === 504) return "Serveur indisponible, réessaie dans un instant.";
  if (status === 404) return "Ressource introuvable.";
  if (status === 401 || status === 403) return "Session expirée ou accès refusé — reconnecte-toi.";
  if (status >= 500) return `Erreur serveur (code ${status}).`;

  const trimmed = body.trim();
  if (trimmed && !looksLikeHtml(trimmed, contentType)) {
    try {
      const j = JSON.parse(trimmed) as { error?: unknown; message?: unknown };
      const errText = typeof j.error === "string" ? j.error.trim() : "";
      const msgText = typeof j.message === "string" ? j.message.trim() : "";
      const text = errText || msgText;
      if (text === "request_failed") return "La requête a échoué.";
      if (text) return text;
    } catch {
      /* plain text */
    }
    if (trimmed === "request_failed") return "La requête a échoué.";
    if (trimmed.length <= 180 && !trimmed.includes("<")) return trimmed;
  }

  if (status > 0) return `Erreur réseau (code ${status}).`;
  return "Erreur réseau.";
}

/** Sanitize any thrown Error / unknown (e.g. already-stringified HTML body). */
export function errorMessageFromUnknown(err: unknown): string {
  if (!(err instanceof Error)) return "Erreur réseau.";
  const msg = err.message ?? "";
  if (!msg.trim()) return "Erreur réseau.";
  if (looksLikeHtml(msg)) {
    const fromTitle = msg.match(/<title>\s*(\d{3})\b/i);
    const fromText = msg.match(/\b(413|502|504|401|403|404|500|503)\b/);
    const code = Number(fromTitle?.[1] ?? fromText?.[1] ?? 0);
    return friendlyHttpError(code, msg, "text/html");
  }
  return msg;
}

export function uploadTooLargeMessage(fileName?: string): string {
  const who = fileName?.trim() ? `« ${fileName.trim()} »` : "Ce fichier";
  return `${who} dépasse la limite de ${MAX_UPLOAD_LABEL}.`;
}
