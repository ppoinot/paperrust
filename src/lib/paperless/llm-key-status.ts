/**
 * Settings LLM key status — parse/merge helpers.
 *
 * Never persist, log, or return raw API key material. The server is the source
 * of truth (`hasKey` / empty `masked`). A last-known configured *flag* may live
 * in localStorage so Réglages still shows « clé configurée » after refresh
 * when GET omits `llmKeys`.
 */
import type { LlmProvider } from "./types";

/** Toast / error copy — provider names only (never model/product names). */
function providerNameForToast(provider?: string): string {
  const id = canonicalLlmProvider(provider);
  if (id === "xai") return "SpaceXAi";
  if (id === "openai") return "OpenAI";
  if (id === "anthropic") return "Anthropic";
  if (id === "selfhosted") return "Auto-hébergé";
  return "LLM";
}

/** Live paths — never `/api/settings/llm` (404 on beta). */
export const SETTINGS_PATH = "/api/settings";
export const LLM_KEYS_PATH = "/api/settings/llm-keys";

const CONFIGURED_KEY = "paperrust.llm.configured";

function browserStorage(): Storage | null {
  try {
    const ls = (globalThis as { localStorage?: Storage }).localStorage;
    return ls ?? null;
  } catch {
    return null;
  }
}

export type LlmKeyStatus = {
  provider: string;
  hasKey: boolean;
  masked: string;
};

export type LlmKeyPatchResult = LlmKeyStatus & {
  ok: boolean;
};

export type SettingsPayload = {
  ocrProvider: LlmProvider;
  ocrLanguage: string;
  autoMatch: boolean;
  aiAvailable: boolean;
  llmKeys: LlmKeyStatus[];
  /** False when the API omitted `llmKeys` / `llm_keys` — use last-known flags. */
  llmKeysFromApi: boolean;
  ocrBaseUrl?: string | null;
  trashRetentionDays?: number;
};

const SELECTABLE: readonly LlmProvider[] = ["xai", "openai", "anthropic", "selfhosted"];

const PROVIDER_ALIASES: Record<string, LlmProvider> = {
  xai: "xai",
  "x.ai": "xai",
  spacexai: "xai",
  "space-x-ai": "xai",
  openai: "openai",
  anthropic: "anthropic",
  selfhosted: "selfhosted",
  "openai-compatible": "selfhosted",
  autoheberge: "selfhosted",
  "auto-heberge": "selfhosted",
};

/** PATCH/POST body for llm-keys. `SpaceXAi` / `spacexai` → `xai`. Never logs the secret. */
export function buildLlmKeyRequestBody(data: {
  provider: string;
  apiKey?: string;
  clear?: boolean;
}): { provider: string; clear: boolean; apiKey?: string } {
  const provider = canonicalLlmProvider(data.provider) || data.provider;
  const clear = Boolean(data.clear);
  const body: { provider: string; clear: boolean; apiKey?: string } = { provider, clear };
  const trimmed = typeof data.apiKey === "string" ? data.apiKey.trim() : "";
  if (!clear && trimmed) body.apiKey = trimmed;
  return body;
}

export function llmKeyConfiguredBadge(): string {
  return "Clé configurée";
}

export function llmKeyConfiguredToast(provider: string): string {
  return `Clé API ${providerNameForToast(provider)} configurée.`;
}

export function canonicalLlmProvider(raw: string | null | undefined): string {
  const t = (raw ?? "").trim().toLowerCase();
  if (!t) return "";
  return PROVIDER_ALIASES[t] ?? t;
}

export function sameLlmProvider(a: string | null | undefined, b: string | null | undefined): boolean {
  const left = canonicalLlmProvider(a);
  const right = canonicalLlmProvider(b);
  return Boolean(left) && left === right;
}

function asRecord(raw: unknown): Record<string, unknown> | null {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return null;
  return raw as Record<string, unknown>;
}

function pick(raw: Record<string, unknown>, ...keys: string[]): unknown {
  for (const k of keys) {
    if (k in raw) return raw[k];
  }
  return undefined;
}

function asBool(v: unknown): boolean {
  if (v === true || v === 1) return true;
  if (v === false || v === 0 || v == null) return false;
  if (typeof v === "string") {
    const t = v.trim().toLowerCase();
    if (t === "true" || t === "1" || t === "yes") return true;
    if (t === "false" || t === "0" || t === "no" || t === "") return false;
  }
  return Boolean(v);
}

/**
 * Accept only already-masked hints. Anything that looks like key material
 * is dropped (UI then shows generic bullets).
 */
export function safeMasked(raw: unknown): string {
  if (typeof raw !== "string") return "";
  const t = raw.trim();
  if (!t) return "";
  if (t.length > 24) return "";
  if (/sk-|xai-|api[_-]?key|bearer\s/i.test(t)) return "";
  if (/^[•·*xX.]{3,}$/.test(t)) return "••••••••";
  if (/^[•·*.]{2,}\s*\S{1,4}$/.test(t)) return t;
  // Short trailing-hint style (e.g. "…wxyz") — never a full token.
  if (/^[.…•]{1,8}[A-Za-z0-9]{2,4}$/.test(t)) return t;
  return "";
}

export function normalizeLlmKeyStatus(raw: unknown): LlmKeyStatus | null {
  const rec = asRecord(raw);
  if (!rec) return null;
  const provider = String(pick(rec, "provider") ?? "").trim();
  if (!provider) return null;
  return {
    provider: canonicalLlmProvider(provider) || provider,
    hasKey: asBool(pick(rec, "hasKey", "has_key", "configured")),
    masked: safeMasked(pick(rec, "masked", "hint")),
  };
}

export function llmKeyStatusForProvider(
  keys: LlmKeyStatus[] | null | undefined,
  provider: string,
): LlmKeyStatus | undefined {
  if (!keys?.length) return undefined;
  return keys.find((k) => sameLlmProvider(k.provider, provider));
}

export function readConfiguredProviders(): string[] {
  const ls = browserStorage();
  if (!ls) return [];
  try {
    const raw = ls.getItem(CONFIGURED_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw) as unknown;
    if (!Array.isArray(parsed)) return [];
    return parsed
      .filter((p): p is string => typeof p === "string" && p.trim().length > 0)
      .map((p) => canonicalLlmProvider(p))
      .filter(Boolean);
  } catch {
    return [];
  }
}

export function writeConfiguredProvider(provider: string, configured: boolean): string[] {
  const id = canonicalLlmProvider(provider);
  const next = new Set(readConfiguredProviders());
  if (!id) return [...next];
  if (configured) next.add(id);
  else next.delete(id);
  const list = [...next];
  const ls = browserStorage();
  if (ls) {
    try {
      ls.setItem(CONFIGURED_KEY, JSON.stringify(list));
    } catch {
      /* ignore quota / private mode */
    }
  }
  return list;
}

/** When GET listed keys, align last-known flags with the server. */
export function syncConfiguredProvidersFromRemote(keys: LlmKeyStatus[]): string[] {
  const next = keys.filter((k) => k.hasKey).map((k) => canonicalLlmProvider(k.provider)).filter(Boolean);
  const ls = browserStorage();
  if (ls) {
    try {
      ls.setItem(CONFIGURED_KEY, JSON.stringify(next));
    } catch {
      /* ignore */
    }
  }
  return next;
}

/**
 * Visual « clé configurée »:
 * - If the API sent `llmKeys`, trust `hasKey` for that provider.
 * - If the API omitted the array, fall back to the last-known flag (no secret).
 */
export function resolveHasConfiguredKey(
  provider: string,
  keys: LlmKeyStatus[] | null | undefined,
  llmKeysFromApi: boolean,
): boolean {
  if (llmKeysFromApi) {
    return Boolean(llmKeyStatusForProvider(keys, provider)?.hasKey);
  }
  return readConfiguredProviders().includes(canonicalLlmProvider(provider));
}

function asProvider(raw: unknown, fallback: LlmProvider = "xai"): LlmProvider {
  const id = canonicalLlmProvider(typeof raw === "string" ? raw : "");
  if ((SELECTABLE as readonly string[]).includes(id)) return id as LlmProvider;
  return fallback;
}

export function emptySettingsPayload(): SettingsPayload {
  return {
    ocrProvider: "xai",
    ocrLanguage: "fr",
    autoMatch: true,
    aiAvailable: false,
    llmKeys: [],
    llmKeysFromApi: false,
    ocrBaseUrl: "",
    trashRetentionDays: 30,
  };
}

export function normalizeSettingsPayload(raw: unknown): SettingsPayload {
  const rec = asRecord(raw) ?? {};
  const listed = pick(rec, "llmKeys", "llm_keys");
  const fromApi = Array.isArray(listed);
  const llmKeys = fromApi
    ? (listed as unknown[]).map(normalizeLlmKeyStatus).filter((k): k is LlmKeyStatus => k != null)
    : [];

  const daysRaw = pick(rec, "trashRetentionDays", "trash_retention_days");
  const days = typeof daysRaw === "number" ? daysRaw : Number(daysRaw);

  const payload: SettingsPayload = {
    ocrProvider: asProvider(pick(rec, "ocrProvider", "ocr_provider")),
    ocrLanguage: String(pick(rec, "ocrLanguage", "ocr_language") ?? "fr") || "fr",
    autoMatch: asBool(pick(rec, "autoMatch", "auto_match") ?? true),
    aiAvailable: asBool(pick(rec, "aiAvailable", "ai_available")),
    llmKeys,
    llmKeysFromApi: fromApi,
    ocrBaseUrl: (() => {
      const u = pick(rec, "ocrBaseUrl", "ocr_base_url");
      if (typeof u === "string") return u;
      if (u == null) return "";
      return String(u);
    })(),
    trashRetentionDays: days === 60 || days === 90 ? days : 30,
  };

  if (fromApi) syncConfiguredProvidersFromRemote(llmKeys);
  return payload;
}

export function normalizeLlmKeyPatchResult(raw: unknown, fallbackProvider: string): LlmKeyPatchResult {
  const rec = asRecord(raw) ?? {};
  const status = normalizeLlmKeyStatus({
    provider: pick(rec, "provider") ?? fallbackProvider,
    hasKey: pick(rec, "hasKey", "has_key", "configured") ?? true,
    masked: pick(rec, "masked", "hint"),
  });
  return {
    ok: asBool(pick(rec, "ok") ?? true),
    provider: status?.provider || canonicalLlmProvider(fallbackProvider) || fallbackProvider,
    hasKey: status?.hasKey ?? true,
    masked: status?.masked ?? "",
  };
}

export function mergeLlmKeyIntoSettings(
  prev: SettingsPayload | undefined,
  status: LlmKeyStatus,
): SettingsPayload {
  const base = prev ?? emptySettingsPayload();
  const keys = [...base.llmKeys];
  const i = keys.findIndex((k) => sameLlmProvider(k.provider, status.provider));
  const row: LlmKeyStatus = {
    provider: canonicalLlmProvider(status.provider) || status.provider,
    hasKey: status.hasKey,
    masked: safeMasked(status.masked),
  };
  if (i >= 0) keys[i] = { ...keys[i], ...row };
  else keys.push(row);
  return { ...base, llmKeys: keys, llmKeysFromApi: true };
}

function looksLikeSecret(text: string): boolean {
  return /sk-|xai-|api[_-]?key\s*[:=]|bearer\s+[A-Za-z0-9._-]{8,}/i.test(text);
}

export function llmKeyMutationErrorMessage(err: unknown, provider?: string): string {
  const label = providerNameForToast(provider || "xai");
  const status =
    err && typeof err === "object" && "status" in err && typeof (err as { status?: unknown }).status === "number"
      ? (err as { status: number }).status
      : 0;
  const raw = err instanceof Error ? err.message : "";
  const msg = looksLikeSecret(raw) ? "" : raw.trim();

  if (status === 401 || status === 403) {
    return "Session expirée ou accès refusé — reconnecte-toi pour enregistrer la clé API.";
  }
  if (status === 404) {
    return `Impossible d'enregistrer la clé API ${label} (ressource introuvable).`;
  }
  if (status === 405) {
    return `Le serveur a refusé l'enregistrement de la clé API ${label}.`;
  }
  if (status === 429 || /rate limited/i.test(msg)) {
    return "Trop de tentatives. Réessaie dans un instant.";
  }
  if (status >= 500) {
    return `Impossible d'enregistrer la clé API ${label} (erreur serveur).`;
  }
  if (/request_failed|fournisseur inconnu|clé vide|clé trop longue|apiKey requis/i.test(msg)) {
    return `Impossible d'enregistrer la clé API ${label}.`;
  }
  if (msg && msg.length <= 180 && !msg.includes("<")) {
    return msg;
  }
  return `Impossible d'enregistrer la clé API ${label}.`;
}

export function httpStatusOf(err: unknown): number | undefined {
  if (err && typeof err === "object" && "status" in err) {
    const s = (err as { status?: unknown }).status;
    if (typeof s === "number") return s;
  }
  return undefined;
}
