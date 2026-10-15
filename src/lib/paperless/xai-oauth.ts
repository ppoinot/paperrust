/**
 * Re-exports SpaceXAi OCR OAuth stubs (internal id `xai`).
 * Canonical module: `@/lib/paperless/llm-oauth`.
 */
export {
  XAI_OAUTH_OCR_AVAILABLE,
  type LlmOAuthConnection as XaiOAuthConnection,
  readXaiOAuthConnection,
  startXaiOAuthLogin,
  disconnectXaiOAuth,
} from "./llm-oauth";
