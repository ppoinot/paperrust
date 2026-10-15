/**
 * HTTP client for the live axum API (pgrust). Never talks to PGLite.
 *
 * Browser: same-origin `/api/*` so Vite (or a reverse proxy) forwards to :8788
 * without CORS. SSR: hit 127.0.0.1:8788 directly.
 *
 * Auth is cookies if the API sets them. Do not attach SPA tokens.
 * 401: do not parse the body as documents. Throw AuthRequired and send the
 * browser to /login after clearing catalog/document queries.
 */
import { AuthRequired } from "@/lib/auth/auth-required";
import { friendlyHttpError } from "@/lib/http-error";
import { clearDocumentQueries } from "@/lib/query-client";

export { AuthRequired, isAuthRequired } from "@/lib/auth/auth-required";

async function throwHttpError(res: Response): Promise<never> {
  const text = await res.text().catch(() => "");
  const ct = res.headers.get("content-type");
  const err = new Error(friendlyHttpError(res.status, text, ct)) as Error & { status?: number };
  err.status = res.status;
  throw err;
}

function apiBase(): string {
  const fromEnv = (import.meta.env.VITE_API_URL as string | undefined)?.trim();
  if (fromEnv) return fromEnv.replace(/\/$/, "");
  if (typeof window === "undefined") return "http://127.0.0.1:8788";
  return "";
}

export function apiUrl(path: string): string {
  const p = path.startsWith("/") ? path : `/${path}`;
  return `${apiBase()}${p}`;
}

function redirectToLogin(): void {
  if (typeof window === "undefined") return;
  const path = window.location.pathname;
  if (path === "/login" || path.startsWith("/login/")) return;
  clearDocumentQueries();
  window.location.assign("/login");
}

async function rejectIfUnauthorized(res: Response): Promise<void> {
  if (res.status !== 401) return;
  await res.text().catch(() => "");
  redirectToLogin();
  throw new AuthRequired();
}

export async function api<T>(path: string, init?: RequestInit): Promise<T> {
  const headers = new Headers(init?.headers);
  if (init?.body && !headers.has("content-type")) {
    headers.set("content-type", "application/json");
  }
  const res = await fetch(apiUrl(path), { ...init, headers, credentials: "same-origin" });
  await rejectIfUnauthorized(res);
  if (!res.ok) await throwHttpError(res);
  if (res.status === 204) return undefined as T;
  const ct = res.headers.get("content-type") ?? "";
  if (!ct.includes("json")) {
    const text = await res.text().catch(() => "");
    throw new Error(friendlyHttpError(res.status || 0, text, ct));
  }
  return (await res.json()) as T;
}

/** GET /api/documents/:id — 404 `{error:"not_found"}` becomes null (Introuvable), not a thrown banner. */
export async function getDocument<T = unknown>(id: number): Promise<T | null> {
  const path = `/api/documents/${id}`;
  const res = await fetch(apiUrl(path), {
    headers: { accept: "application/json" },
    credentials: "same-origin",
  });
  await rejectIfUnauthorized(res);
  if (res.status === 404) {
    await res.text().catch(() => "");
    return null;
  }
  if (!res.ok) await throwHttpError(res);
  if (res.status === 204) return null;
  const ct = res.headers.get("content-type") ?? "";
  if (!ct.includes("json")) {
    const text = await res.text().catch(() => "");
    throw new Error(friendlyHttpError(res.status || 0, text, ct));
  }
  const data = (await res.json()) as T | null;
  return data ?? null;
}

export async function apiDelete<T>(path: string): Promise<T> {
  return api<T>(path, { method: "DELETE" });
}

export async function apiPost<T>(path: string, body?: unknown): Promise<T> {
  return api<T>(path, {
    method: "POST",
    body: body === undefined ? undefined : JSON.stringify(body),
  });
}

export async function apiPatch<T>(path: string, body?: unknown): Promise<T> {
  return api<T>(path, {
    method: "PATCH",
    body: body === undefined ? undefined : JSON.stringify(body),
  });
}

/** POST multipart. Do not set Content-Type — the browser must add the boundary. */
export async function apiForm<T>(path: string, form: FormData): Promise<T> {
  const res = await fetch(apiUrl(path), {
    method: "POST",
    body: form,
    credentials: "same-origin",
  });
  await rejectIfUnauthorized(res);
  if (!res.ok) await throwHttpError(res);
  if (res.status === 204) return undefined as T;
  const ct = res.headers.get("content-type") ?? "";
  if (!ct.includes("json")) {
    const text = await res.text().catch(() => "");
    throw new Error(friendlyHttpError(res.status || 0, text, ct));
  }
  return (await res.json()) as T;
}

/** HEAD (GET fallback) — true when the original file bytes exist. */
export async function documentOriginalExists(id: number): Promise<boolean> {
  const url = apiUrl(`/api/documents/${id}/file`);
  try {
    const head = await fetch(url, { method: "HEAD", credentials: "same-origin" });
    if (head.status === 401) {
      await head.text().catch(() => "");
      redirectToLogin();
      throw new AuthRequired();
    }
    if (head.ok) return true;
  } catch (err) {
    if (err instanceof AuthRequired) throw err;
    /* fall through to GET */
  }
  try {
    const get = await fetch(url, {
      method: "GET",
      headers: { Range: "bytes=0-0" },
      credentials: "same-origin",
    });
    if (get.status === 401) {
      await get.text().catch(() => "");
      redirectToLogin();
      throw new AuthRequired();
    }
    if (get.status === 404) return false;
    return get.ok || get.status === 206;
  } catch (err) {
    if (err instanceof AuthRequired) throw err;
    return false;
  }
}
