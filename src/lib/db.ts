/**
 * PGLite is disabled. Documents live in pgrust via the axum API (:8788).
 * Better Auth's preview dialect still imports getPglite(); it throws if ever
 * called — real SSO is OIDC on the Rust API. Do not re-enable PGLite.
 */

export type DbSource = "api";
export const dbSource: DbSource = "api";

export interface Sql {
  <T = Record<string, unknown>>(
    strings: TemplateStringsArray,
    ...values: unknown[]
  ): Promise<T[]>;
  query<T = Record<string, unknown>>(
    text: string,
    params?: unknown[]
  ): Promise<T[]>;
}

const DISABLED = "PGLite is disabled. Use the Rust API at http://127.0.0.1:8788.";

export function getSql(): Promise<Sql> {
  return Promise.reject(new Error(DISABLED));
}

export async function getPglite(): Promise<never> {
  throw new Error(DISABLED);
}

/** No-op: Vite used to await this and hang the UI on WASM Postgres. */
export function ensureDbReady(): Promise<void> {
  return Promise.resolve();
}
