import { existsSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import type { Plugin } from "vite";
import { defineConfig } from "vite";
import { tanstackStart } from "@tanstack/react-start/plugin/vite";
import viteReact from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { nitro } from "nitro/vite";
import { rewriteBarrierToConsumeNonce } from "./src/lib/csp-nonce.ts";
// @ts-expect-error JS plugin alongside the TS vite config
import { appEnvPlugin } from "./scripts/app-env-plugin.mjs";

const API_ORIGIN = process.env.VITE_API_URL?.trim() || "http://127.0.0.1:8788";

/** Proxy /api (except Better Auth) and /health to the live axum API. */
function apiProxy() {
  return {
    target: API_ORIGIN,
    changeOrigin: true,
    bypass(req: { url?: string }) {
      const url = req.url ?? "";
      if (url.startsWith("/api/auth")) return url;
    },
  };
}

/**
 * Rewrite the existing `$tsr-stream-barrier` body so it copies
 * `document.currentScript.nonce` onto `self.__TSR_CSP_NONCE`
 * before it removes itself. No new `<script>` tag.
 */
function cspNonceBarrierPlugin(): Plugin {
  const rewriteHtmlAssets = (
    bundle: Record<string, { type: string; fileName: string; source?: string | Uint8Array }>,
  ) => {
    for (const chunk of Object.values(bundle)) {
      if (chunk.type !== "asset" || typeof chunk.source !== "string") continue;
      if (!chunk.fileName.endsWith(".html")) continue;
      chunk.source = rewriteBarrierToConsumeNonce(chunk.source);
    }
  };

  const rewriteHtmlFile = (file: string) => {
    if (!existsSync(file)) return;
    const html = readFileSync(file, "utf8");
    const next = rewriteBarrierToConsumeNonce(html);
    if (next !== html) writeFileSync(file, next);
  };

  const rewriteShellsIn = (dir: string) => {
    if (!existsSync(dir)) return;
    rewriteHtmlFile(path.join(dir, "index.html"));
    rewriteHtmlFile(path.join(dir, "_shell.html"));
  };

  return {
    name: "paperrust-csp-nonce-barrier",
    transformIndexHtml: {
      order: "post",
      handler(html) {
        return rewriteBarrierToConsumeNonce(html);
      },
    },
    generateBundle(_opts, bundle) {
      rewriteHtmlAssets(bundle);
    },
    closeBundle() {
      // Prerendered shells are finalized after this hook — the dist pose script
      // rewrites them again. This still covers transformIndexHtml / generateBundle.
      rewriteShellsIn(path.resolve("dist"));
      rewriteShellsIn(path.resolve(".vercel/output/static"));
    },
  };
}

/**
 * Live-preview OAuth popup — handled HERE so the agent never has to create a
 * `/auth/popup` route (and cannot break it by scaffolding a React page that
 * paints the full app shell in the popup).
 *
 * `signIn` (client.ts) opens `/auth/popup?providerId=…` in a top-level window.
 * This middleware runs before TanStack Start, calls `handleAuthPopupRequest`,
 * and returns the 302 / completion HTML. Deployed apps do not use the popup
 * (full-page OAuth redirect), so `apply: "serve"` is enough.
 */
function authPopupPlugin(): Plugin {
  return {
    name: "app-builder:auth-popup",
    apply: "serve",
    configureServer(server) {
      // Register immediately (not in a returned post-hook) so we run BEFORE
      // TanStack Start / the SPA HTML fallback. A model-authored
      // `src/routes/auth/popup.tsx` React page must never win this path.
      server.middlewares.use(async (req, res, next) => {
        try {
          const rawUrl = req.url ?? "";
          const pathOnly = rawUrl.split("?", 1)[0] ?? "";
          if (pathOnly !== "/auth/popup") {
            next();
            return;
          }
          if ((req.method ?? "GET").toUpperCase() !== "GET") {
            res.statusCode = 405;
            res.setHeader("content-type", "text/plain; charset=utf-8");
            res.end("Method Not Allowed");
            return;
          }

          const host = String(
            req.headers["x-forwarded-host"] ?? req.headers.host ?? "localhost:8080",
          );
          const proto = String(
            req.headers["x-forwarded-proto"] ??
              ((req.socket as { encrypted?: boolean } | undefined)?.encrypted ? "https" : "http"),
          );
          const requestHeaders = new Headers();
          for (const [key, value] of Object.entries(req.headers)) {
            if (value === undefined) continue;
            if (Array.isArray(value)) {
              for (const v of value) requestHeaders.append(key, v);
            } else {
              requestHeaders.set(key, value);
            }
          }
          // Ensure Host is the public preview host so Better Auth's dynamic
          // baseURL / redirect_uri match the popup origin.
          if (!requestHeaders.has("host")) requestHeaders.set("host", host);

          const request = new Request(`${proto}://${host}${rawUrl}`, {
            method: "GET",
            headers: requestHeaders,
          });

          const mod = (await server.ssrLoadModule("/src/lib/auth/popup.server.ts")) as {
            handleAuthPopupRequest: (req: Request) => Promise<Response>;
          };
          const response = await mod.handleAuthPopupRequest(request);

          res.statusCode = response.status;
          // Preserve multiple Set-Cookie headers (OAuth state + session).
          const setCookies =
            typeof response.headers.getSetCookie === "function"
              ? response.headers.getSetCookie()
              : [];
          response.headers.forEach((value, key) => {
            if (key.toLowerCase() === "set-cookie") return;
            res.setHeader(key, value);
          });
          for (const cookie of setCookies) {
            res.appendHeader("set-cookie", cookie);
          }
          const body = Buffer.from(await response.arrayBuffer());
          res.end(body);
        } catch (err) {
          console.error("[app-builder] /auth/popup handler failed:", err);
          if (!res.headersSent) {
            res.statusCode = 500;
            res.setHeader("content-type", "text/plain; charset=utf-8");
            res.end("auth popup failed");
          }
        }
      });
    },
  };
}

// `0.0.0.0:8080` is the dev-server contract — don't change host/port.
// The dev server starts once `src/router.tsx` and `src/routes/` exist.
export default defineConfig(({ command, isPreview }) => ({
  server: {
    host: "0.0.0.0",
    port: 8080,
    strictPort: true,
    proxy: {
      "/api": apiProxy(),
      "/health": apiProxy(),
    },
  },
  preview: {
    host: "127.0.0.1",
    port: 8081,
    strictPort: true,
    proxy: {
      "/api": apiProxy(),
      "/health": apiProxy(),
    },
  },
  resolve: { tsconfigPaths: true },
  plugins: [
    // Before tanstackStart so /auth/popup never falls through to the SPA.
    authPopupPlugin(),
    // Dev-only /__app-env, read by scripts/check-auth-invariant.mjs.
    appEnvPlugin(),
    tailwindcss(),
    tanstackStart({
      spa: { enabled: true },
    }),
    ...(command === "build" || isPreview
      ? [
          nitro({
            preset: "vercel",
          }),
        ]
      : []),
    viteReact(),
    // Last: rewrite the prerendered `$tsr-stream-barrier` after Start writes it.
    cspNonceBarrierPlugin(),
  ],
}));
