/** Upload-only error UX. Never surface raw nginx/HTML to the user. */

/** Hard cap — keep in sync with API `consume::MAX_UPLOAD_BYTES` and Ingress `proxy-body-size`. */
export const MAX_UPLOAD_BYTES = 300 * 1024 * 1024;

export const UPLOAD_TOO_LARGE_FR = "Fichier trop volumineux (max. 300 Mo).";

export function isHtmlErrorBody(text: string): boolean {
  const t = text.trimStart().slice(0, 512).toLowerCase();
  if (!t) return false;
  return (
    t.startsWith("<!doctype") ||
    t.startsWith("<html") ||
    t.includes("<center>") ||
    t.includes("<title>") ||
    /\bnginx\b/i.test(t) ||
    t.includes("413 request entity too large") ||
    t.includes("request entity too large")
  );
}

function statusFromUnknown(err: unknown): number {
  if (err && typeof err === "object") {
    const o = err as { status?: unknown; statusCode?: unknown };
    if (typeof o.status === "number") return o.status;
    if (typeof o.statusCode === "number") return o.statusCode;
  }
  const msg = err instanceof Error ? err.message : String(err ?? "");
  const title = msg.match(/<title>\s*(\d{3})\b/i);
  if (title) return Number(title[1]);
  const code = msg.match(/\b(413|401|403|404|502|504|500|503)\b/);
  if (code) return Number(code[1]);
  return 0;
}

function mapStatus(status: number): string | null {
  if (status === 413) return UPLOAD_TOO_LARGE_FR;
  if (status === 401) return "Session expirée — reconnectez-vous.";
  if (status === 403) return "Accès refusé.";
  if (status === 404) return "Service d’import introuvable.";
  if (status === 502 || status === 504) return "Serveur indisponible — réessayez.";
  if (status >= 500) return "Erreur serveur — réessayez.";
  return null;
}

function isNetworkFailure(err: unknown, msg: string): boolean {
  if (err instanceof TypeError) return true;
  const m = msg.toLowerCase();
  return (
    m.includes("failed to fetch") ||
    m.includes("networkerror") ||
    m.includes("network request failed") ||
    m.includes("load failed") ||
    m.includes("connexion") && m.includes("impossible")
  );
}

/**
 * Map any upload failure to a short French string.
 * Never returns HTML / nginx page bodies.
 */
export function humanizeUploadError(err: unknown): string {
  const msg = err instanceof Error ? err.message : String(err ?? "");
  const status = statusFromUnknown(err);

  if (isHtmlErrorBody(msg)) {
    return mapStatus(status) ?? "Échec de l’import.";
  }

  const byStatus = mapStatus(status);
  if (byStatus) return byStatus;

  if (isNetworkFailure(err, msg)) return "Connexion impossible.";

  const trimmed = msg.trim();
  if (trimmed && !isHtmlErrorBody(trimmed) && trimmed.length <= 160 && !trimmed.includes("<")) {
    return trimmed;
  }

  return "Échec de l’import.";
}
