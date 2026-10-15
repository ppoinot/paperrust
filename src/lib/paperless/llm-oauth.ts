/**
 * Future "Se connecter avec …" OAuth for OCR (user subscription / account).
 *
 * Today: all providers disabled. Official path remains per-user API keys
 * (SpaceXAi / OpenAI / Anthropic stored server-side). First-party OAuth clients
 * need partnership / allowlisted registration per vendor.
 *
 * Flip a provider's `*_OAUTH_OCR_AVAILABLE` flag (or the shared master) to true,
 * then wire authorize / token / refresh for that vendor.
 */

export type OAuthLlmProvider = "xai" | "openai" | "anthropic" | "kimi" | "mistral";

/** Master kill-switch — keep false until any partnership is live. */
export const LLM_OAUTH_OCR_AVAILABLE = false;

export const XAI_OAUTH_OCR_AVAILABLE = false;
export const OPENAI_OAUTH_OCR_AVAILABLE = false;
export const ANTHROPIC_OAUTH_OCR_AVAILABLE = false;
export const KIMI_OAUTH_OCR_AVAILABLE = false;
export const MISTRAL_OAUTH_OCR_AVAILABLE = false;

const PROVIDER_FLAGS: Record<OAuthLlmProvider, boolean> = {
  xai: XAI_OAUTH_OCR_AVAILABLE,
  openai: OPENAI_OAUTH_OCR_AVAILABLE,
  anthropic: ANTHROPIC_OAUTH_OCR_AVAILABLE,
  kimi: KIMI_OAUTH_OCR_AVAILABLE,
  mistral: MISTRAL_OAUTH_OCR_AVAILABLE,
};

export type LlmOAuthProviderMeta = {
  id: OAuthLlmProvider;
  /** Short brand for the button label after "Se connecter avec ". */
  buttonLabel: string;
  /** Section title next to the button. */
  title: string;
};

/** Uniform OAuth-disabled copy for every row (incl. Kimi / Mistral). */
export const OAUTH_SOON_FR = "OAuth bientôt — utilisez votre clé API personnelle.";

export const LLM_OAUTH_PROVIDERS: readonly LlmOAuthProviderMeta[] = [
  { id: "xai", buttonLabel: "SpaceXAi", title: "SpaceXAi" },
  { id: "openai", buttonLabel: "OpenAI", title: "OpenAI" },
  { id: "anthropic", buttonLabel: "Anthropic", title: "Anthropic" },
  { id: "kimi", buttonLabel: "Kimi", title: "Kimi" },
  { id: "mistral", buttonLabel: "Mistral", title: "Mistral" },
] as const;

export type LlmOAuthConnection = {
  connected: boolean;
  /** Email or handle once OAuth is live; empty while disabled. */
  label: string;
};

export function isLlmOAuthAvailable(provider: OAuthLlmProvider): boolean {
  return LLM_OAUTH_OCR_AVAILABLE && PROVIDER_FLAGS[provider];
}

export function readLlmOAuthConnection(_provider: OAuthLlmProvider): LlmOAuthConnection {
  return { connected: false, label: "" };
}

/** No-op until a registered OAuth client exists for that provider. */
export function startLlmOAuthLogin(provider: OAuthLlmProvider): void {
  if (!isLlmOAuthAvailable(provider)) return;
  // Future: window.location.assign(`/api/auth/${provider}/ocr/login`) or device-code UI.
}

export function disconnectLlmOAuth(provider: OAuthLlmProvider): void {
  if (!isLlmOAuthAvailable(provider)) return;
  // Future: clear server-stored tokens + local connection marker.
}

/** @deprecated Prefer isLlmOAuthAvailable("xai") — kept for callers of the SpaceXAi OAuth stub. */
export function readXaiOAuthConnection(): LlmOAuthConnection {
  return readLlmOAuthConnection("xai");
}

/** @deprecated Prefer startLlmOAuthLogin("xai"). */
export function startXaiOAuthLogin(): void {
  startLlmOAuthLogin("xai");
}

/** @deprecated Prefer disconnectLlmOAuth("xai"). */
export function disconnectXaiOAuth(): void {
  disconnectLlmOAuth("xai");
}
