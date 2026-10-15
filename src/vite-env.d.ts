/// <reference types="vite/client" />

/**
 * Build-time public env (no secrets). See `src/lib/deploy-env.ts`.
 *
 * OIDC stays same-origin on whichever host is served. Stirling's base URL
 * comes from the API at runtime; `VITE_STIRLING_BASE_URL` is dev-only.
 */
interface ImportMetaEnv {
  /** Unset/empty = same-origin `/api`. Local Vite → axum: `http://127.0.0.1:8788`. */
  readonly VITE_API_URL?: string;
  /** Display/default Stirling origin. Open URL comes from POST …/stirling-edit. */
  readonly VITE_STIRLING_BASE_URL?: string;
  readonly VITE_OIDC_CLIENT_ID?: string;
  readonly VITE_AUTH_ENABLED?: string;
}
