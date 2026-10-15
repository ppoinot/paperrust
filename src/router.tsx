import { createRouter } from "@tanstack/react-router";
import { readShellCspNonce } from "@/lib/csp-nonce";
import { AppErrorComponent } from "@/lib/error-component";
import { routeTree } from "./routeTree.gen";

export function getRouter() {
  // TanStack `<Scripts />` stamps `router.options.ssr.nonce` onto the
  // `$tsr-stream-barrier` / hydration tags it emits. Consume the
  // already-injected attribute — do not mint a nonce or set CSP here.
  const nonce = readShellCspNonce();
  return createRouter({
    routeTree,
    defaultErrorComponent: AppErrorComponent,
    ssr: { nonce },
  });
}
