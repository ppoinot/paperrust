import type { LlmProvider } from "./types";

const KEY = "paperrust.llm";

/**
 * Providers that accept a pasted API key in Settings (server-stored per user).
 * `selfhosted` key is optional — base URL is required instead.
 */
export const API_KEY_PROVIDERS: readonly LlmProvider[] = [
  "xai",
  "openai",
  "anthropic",
  "selfhosted",
];

export type ClientLlmSettings = {
  provider: LlmProvider;
  /** Draft only — never the source of truth; keys live on the server per user. */
  openaiKey: string;
  anthropicKey: string;
  xaiKey: string;
  selfhostedKey: string;
  /** OpenAI-compatible base URL for provider `selfhosted` (e.g. https://llm.exemple.loc/v1). */
  selfhostedBaseUrl: string;
};

const defaults: ClientLlmSettings = {
  provider: "xai",
  openaiKey: "",
  anthropicKey: "",
  xaiKey: "",
  selfhostedKey: "",
  selfhostedBaseUrl: "",
};

function legacyLlmKey(): string {
  return ["vel", "lum.llm"].join("");
}

function parseSettings(raw: string): ClientLlmSettings {
  return { ...defaults, ...(JSON.parse(raw) as Partial<ClientLlmSettings>) };
}

export function readLlmSettings(): ClientLlmSettings {
  if (typeof window === "undefined") return defaults;
  try {
    const current = localStorage.getItem(KEY);
    if (current) return parseSettings(current);
    const legacy = localStorage.getItem(legacyLlmKey());
    if (legacy) {
      localStorage.setItem(KEY, legacy);
      localStorage.removeItem(legacyLlmKey());
      return parseSettings(legacy);
    }
    return defaults;
  } catch {
    return defaults;
  }
}

export function writeLlmSettings(next: ClientLlmSettings) {
  // Persist provider + selfhosted URL locally for upload UX; API keys are drafts only.
  localStorage.setItem(
    KEY,
    JSON.stringify({
      provider: next.provider,
      openaiKey: "",
      anthropicKey: "",
      xaiKey: "",
      selfhostedKey: "",
      selfhostedBaseUrl: next.selfhostedBaseUrl.trim(),
    }),
  );
  try {
    localStorage.removeItem(legacyLlmKey());
  } catch {
    /* ignore */
  }
  if (typeof window !== "undefined") {
    window.dispatchEvent(new CustomEvent("paperrust:llm-settings", { detail: next.provider }));
  }
}

/** @deprecated Server loads the per-user key; client no longer sends keys in prod. */
export function clientApiKey(_settings: ClientLlmSettings): string | undefined {
  return undefined;
}

export function draftKeyForProvider(settings: ClientLlmSettings, provider: LlmProvider): string {
  if (provider === "openai") return settings.openaiKey;
  if (provider === "anthropic") return settings.anthropicKey;
  if (provider === "xai") return settings.xaiKey;
  if (provider === "selfhosted") return settings.selfhostedKey;
  return "";
}

export function withDraftKey(
  settings: ClientLlmSettings,
  provider: LlmProvider,
  value: string,
): ClientLlmSettings {
  if (provider === "openai") return { ...settings, openaiKey: value };
  if (provider === "anthropic") return { ...settings, anthropicKey: value };
  if (provider === "xai") return { ...settings, xaiKey: value };
  if (provider === "selfhosted") return { ...settings, selfhostedKey: value };
  return settings;
}

/**
 * User-facing brand label. Internal id `xai` stays for API/env;
 * UI must show SpaceXAi (exact casing), never bare « xAI » / « xai ».
 */
export function providerLabel(provider: string | null | undefined): string {
  const p = (provider ?? "").trim().toLowerCase();
  if (p === "xai") return "SpaceXAi";
  if (p === "openai") return "OpenAI";
  if (p === "anthropic") return "Anthropic";
  if (p === "selfhosted" || p === "openai-compatible" || p === "autoheberge") {
    return "Auto-hébergé";
  }
  if (!p) return "LLM";
  return provider!.trim();
}

/** Longer label for Settings select — provider name only (never model names). */
export function providerSelectLabel(provider: LlmProvider): string {
  return providerLabel(provider);
}

/** Dropzone / status line: `OCR via SpaceXAi · max. 300 Mo` */
export function ocrViaHint(provider: LlmProvider): string {
  return `OCR via ${providerLabel(provider)} · max. 300 Mo`;
}

export function keyPlaceholder(_provider: LlmProvider, hasKey: boolean): string {
  if (hasKey) return "••••••••";
  return "Nouvelle clé…";
}

export function keyLabel(provider: LlmProvider): string {
  if (provider === "openai") return "Clé API OpenAI";
  if (provider === "anthropic") return "Clé API Anthropic";
  if (provider === "xai") return "Clé API SpaceXAi";
  if (provider === "selfhosted") return "Clé API (optionnelle)";
  return "Clé API";
}
