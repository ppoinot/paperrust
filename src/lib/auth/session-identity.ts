/**
 * Product identity for the sidebar: GET /api/me only.
 * 404 / anything else → not signed in. Never DEV_USER, never OIDC userinfo,
 * never SPA tokens.
 */
import { useQuery } from "@tanstack/react-query";
import { apiUrl } from "@/lib/paperless/api";

export type SessionIdentity = {
  email?: string;
  name?: string;
};

function pickString(v: unknown): string | undefined {
  return typeof v === "string" && v.trim() ? v.trim() : undefined;
}

export async function fetchSessionIdentity(): Promise<SessionIdentity | null> {
  try {
    const res = await fetch(apiUrl("/api/me"), {
      headers: { accept: "application/json" },
      credentials: "same-origin",
    });
    if (!res.ok) {
      await res.text().catch(() => "");
      return null;
    }
    const ct = res.headers.get("content-type") ?? "";
    if (!ct.includes("json")) {
      await res.text().catch(() => "");
      return null;
    }
    const data = (await res.json()) as Record<string, unknown> | null;
    if (!data || typeof data !== "object") return null;
    const email = pickString(data.email);
    const name = pickString(data.name) ?? pickString(data.displayName);
    if (!email && !name) return null;
    return { email, name };
  } catch {
    return null;
  }
}

export function useSessionIdentity() {
  return useQuery({
    queryKey: ["session-identity"],
    queryFn: fetchSessionIdentity,
    staleTime: 30_000,
    retry: false,
  });
}
