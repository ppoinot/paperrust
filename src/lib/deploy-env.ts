/**
 * Public build-time env. Hostnames are not compiled in.
 *
 * Stirling origins come from `GET /api/config` at runtime
 * (`PAPERRUST_STIRLING_URL` / `PAPERRUST_STIRLING_URL_LAN` on the API).
 * `VITE_STIRLING_BASE_URL` is a dev-only display fallback.
 *
 * Vite (client):
 * - `VITE_API_URL` — optional. Unset/empty → same-origin `/api`. Set only for
 *   local Vite → API, or a split SPA/API origin. OIDC login/callback are
 *   same-origin (`/api/auth/*`) and follow the page host.
 * - `VITE_STIRLING_BASE_URL` — dev display/default only. The open URL is the
 *   API's `editorUrl`.
 */

/** Local Vite / SSR fallback when `VITE_API_URL` is unset (not a public host). */
export const LOCAL_AXUM_ORIGIN = "http://127.0.0.1:8788";

function readVite(name: "VITE_API_URL" | "VITE_STIRLING_BASE_URL"): string {
  try {
    const env = (import.meta as ImportMeta & { env?: Record<string, string | undefined> }).env;
    const raw = env?.[name];
    return typeof raw === "string" ? raw.trim() : "";
  } catch {
    return "";
  }
}

export function trimOrigin(raw: string | null | undefined): string {
  return (raw ?? "").trim().replace(/\/+$/, "");
}

/**
 * Browser: same-origin `""` unless `VITE_API_URL` is set.
 * SSR / Node: local axum if env is empty (dev proxy target).
 */
export function resolvePublicApiBase(opts?: { env?: string | null; inBrowser?: boolean }): string {
  const fromEnv = trimOrigin(opts?.env ?? readVite("VITE_API_URL"));
  if (fromEnv) return fromEnv;
  const inBrowser = opts?.inBrowser ?? typeof window !== "undefined";
  return inBrowser ? "" : LOCAL_AXUM_ORIGIN;
}

/** Dev-only. Production builds ignore this and use `GET /api/config`. */
export function viteStirlingBaseUrl(opts?: { env?: string | null; dev?: boolean }): string {
  const dev = opts?.dev ?? import.meta.env.DEV;
  if (!dev) return "";
  return trimOrigin(opts?.env ?? readVite("VITE_STIRLING_BASE_URL"));
}
