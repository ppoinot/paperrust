/**
 * Paperless-ngx import — client contract + FR copy.
 * The API token is request-only: never localStorage / sessionStorage / logs.
 */

export const PAPERLESS_IMPORT_TOC_ID = "settings-paperless-import";

/** Settings section + modal FR — any public HTTPS Paperless; no allowlist-only implication. */
export const PAPERLESS_IMPORT_SECTION_HELP =
  "Importez les documents de n'importe quelle instance Paperless-ngx (URL HTTPS + jeton API). Le jeton n'est jamais enregistré dans le navigateur — uniquement envoyé au serveur pour ce travail.";

export const PAPERLESS_IMPORT_SECTION_HINT =
  "Une instance HTTPS publique fonctionne telle quelle. Un hôte privé ou LAN peut nécessiter une configuration côté serveur.";

export const PAPERLESS_IMPORT_MODAL_HELP =
  "Toute instance Paperless-ngx en HTTPS. Le jeton API n'est pas mémorisé sur cet appareil.";

export const PAPERLESS_IMPORT_TOKEN_HINT =
  "Saisi pour cet import seulement — jamais stocké dans le navigateur.";

export type PaperlessImportStatus = "queued" | "running" | "done" | "cancelled" | "failed";
export type PaperlessImportPhase = "listing" | "importing" | "finalizing";
export type PaperlessImportUiState = "idle" | "running" | "done" | "partial" | "failed" | "cancelled";

export type PaperlessImportErrorSample = {
  externalId: string;
  message: string;
};

export type PaperlessImportStartBody = {
  baseUrl: string;
  token: string;
  skipOcrIfPresent: boolean;
  preserveTags: boolean;
  dryRun: boolean;
};

export type PaperlessImportStartResponse = {
  jobId: string;
};

export type PaperlessImportJob = {
  jobId: string;
  status: PaperlessImportStatus;
  phase: PaperlessImportPhase | null;
  done: number;
  total: number;
  errors: number;
  errorSamples: PaperlessImportErrorSample[];
  message: string;
};

export const PAPERLESS_IMPORT_DEFAULTS = {
  skipOcrIfPresent: true,
  preserveTags: true,
  dryRun: false,
} as const;

const SESSION_JOB_KEY = "paperrust.paperless-import.jobId";

const POLL_STEPS_MS = [1200, 1800, 2500, 4000, 6000] as const;
export const PAPERLESS_IMPORT_POLL_MAX_MS = 8000;

export function isSafePaperlessJobId(id: string): boolean {
  return /^[A-Za-z0-9._:-]{1,128}$/.test(id);
}

export function isPaperlessImportTerminal(
  status: PaperlessImportStatus | undefined | null,
): boolean {
  return status === "done" || status === "cancelled" || status === "failed";
}

/** Backoff: ~1.2s → 8s. `tick` is the number of successful status fetches so far. */
export function paperlessImportPollIntervalMs(tick: number): number {
  if (tick < 0) return POLL_STEPS_MS[0];
  if (tick < POLL_STEPS_MS.length) return POLL_STEPS_MS[tick]!;
  return PAPERLESS_IMPORT_POLL_MAX_MS;
}

export function paperlessImportUiState(
  job: PaperlessImportJob | null | undefined,
): PaperlessImportUiState {
  if (!job) return "idle";
  if (job.status === "queued" || job.status === "running") return "running";
  if (job.status === "cancelled") return "cancelled";
  if (job.status === "failed") return "failed";
  if (job.status === "done" && (job.errors ?? 0) > 0) return "partial";
  if (job.status === "done") return "done";
  return "idle";
}

export function paperlessImportStateLabel(state: PaperlessImportUiState): string {
  switch (state) {
    case "idle":
      return "Aucun import en cours";
    case "running":
      return "Import en cours";
    case "done":
      return "Import terminé";
    case "partial":
      return "Import terminé avec des erreurs";
    case "failed":
      return "Échec de l'import";
    case "cancelled":
      return "Import annulé";
  }
}

export function paperlessImportPhaseLabel(
  phase: PaperlessImportPhase | null | undefined,
  status?: PaperlessImportStatus | null,
): string | null {
  if (phase === "listing") return "Inventaire des documents…";
  if (phase === "importing") return "Import des documents…";
  if (phase === "finalizing") return "Finalisation…";
  if (status === "queued") return "En file d'attente…";
  return null;
}

/**
 * FR error if the URL is missing or not HTTPS. `null` when the URL is acceptable.
 * Does **not** reject private / LAN hostnames (`paperless.lab.loc`, RFC1918) —
 * SSRF and LAN policy are server-side only (leave OCR selfhosted rules alone).
 */
export function validatePaperlessHttpsUrl(raw: string): string | null {
  const trimmed = raw.trim();
  if (!trimmed) return "L'URL du serveur Paperless-ngx est requise.";
  if (!/^https:\/\//i.test(trimmed)) {
    return "L'URL du serveur doit commencer par https:// (HTTP en clair refusé).";
  }
  try {
    const u = new URL(trimmed);
    if (u.protocol !== "https:") {
      return "L'URL du serveur doit commencer par https:// (HTTP en clair refusé).";
    }
    if (!u.hostname) {
      return "URL Paperless-ngx invalide — utilisez une URL https:// complète.";
    }
    if (u.username || u.password) {
      return "L'URL du serveur ne doit pas contenir d'identifiants (user:pass@).";
    }
  } catch {
    return "URL Paperless-ngx invalide — utilisez une URL https:// complète.";
  }
  return null;
}

/** Accept only a safe job id; ignore anything else (path traversal, spaces, empty). */
export function acceptPaperlessJobId(id: string | null | undefined): string | null {
  const t = (id ?? "").trim();
  return isSafePaperlessJobId(t) ? t : null;
}

export function buildPaperlessImportBody(input: {
  baseUrl: string;
  token: string;
  skipOcrIfPresent?: boolean;
  preserveTags?: boolean;
  dryRun?: boolean;
}): PaperlessImportStartBody {
  return {
    baseUrl: input.baseUrl.trim(),
    token: input.token,
    skipOcrIfPresent: input.skipOcrIfPresent ?? PAPERLESS_IMPORT_DEFAULTS.skipOcrIfPresent,
    preserveTags: input.preserveTags ?? PAPERLESS_IMPORT_DEFAULTS.preserveTags,
    dryRun: input.dryRun ?? PAPERLESS_IMPORT_DEFAULTS.dryRun,
  };
}

export function readActivePaperlessImportJobId(): string | null {
  if (typeof sessionStorage === "undefined") return null;
  try {
    const id = sessionStorage.getItem(SESSION_JOB_KEY)?.trim() ?? "";
    return isSafePaperlessJobId(id) ? id : null;
  } catch {
    return null;
  }
}

/** Persist job id only (never the API token). Session-scoped so a refresh can resume polling. */
export function writeActivePaperlessImportJobId(jobId: string | null): void {
  if (typeof sessionStorage === "undefined") return;
  try {
    if (!jobId || !isSafePaperlessJobId(jobId)) {
      sessionStorage.removeItem(SESSION_JOB_KEY);
      return;
    }
    sessionStorage.setItem(SESSION_JOB_KEY, jobId);
  } catch {
    /* private mode / quota */
  }
}

export function normalizePaperlessImportJob(raw: unknown, fallbackId: string): PaperlessImportJob {
  const o = raw && typeof raw === "object" ? (raw as Record<string, unknown>) : {};
  const statusRaw = typeof o.status === "string" ? o.status : "";
  const status: PaperlessImportStatus = isPaperlessImportStatus(statusRaw) ? statusRaw : "running";
  const phaseRaw = o.phase;
  const phase: PaperlessImportPhase | null =
    phaseRaw === "listing" || phaseRaw === "importing" || phaseRaw === "finalizing"
      ? phaseRaw
      : null;
  const samplesIn = Array.isArray(o.errorSamples) ? o.errorSamples : [];
  const errorSamples: PaperlessImportErrorSample[] = samplesIn
    .filter((s): s is Record<string, unknown> => Boolean(s) && typeof s === "object")
    .map((s) => ({
      externalId: typeof s.externalId === "string" ? s.externalId : "",
      message: typeof s.message === "string" ? s.message : "",
    }))
    .filter((s) => s.externalId || s.message)
    .slice(0, 8);

  return {
    jobId: typeof o.jobId === "string" && o.jobId.trim() ? o.jobId.trim() : fallbackId,
    status,
    phase,
    done: asNonNegInt(o.done),
    total: asNonNegInt(o.total),
    errors: asNonNegInt(o.errors),
    errorSamples,
    message: typeof o.message === "string" ? o.message : "",
  };
}

function isPaperlessImportStatus(v: string): v is PaperlessImportStatus {
  return v === "queued" || v === "running" || v === "done" || v === "cancelled" || v === "failed";
}

function asNonNegInt(v: unknown): number {
  const n = typeof v === "number" ? v : Number(v);
  if (!Number.isFinite(n) || n < 0) return 0;
  return Math.floor(n);
}
