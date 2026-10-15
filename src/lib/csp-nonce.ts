/**
 * Consume the per-response CSP nonce already stamped
 * on the shell / `$tsr-stream-barrier` script. Never generate a nonce and
 * never set or rewrite Content-Security-Policy headers.
 */

export const TSR_CSP_NONCE_GLOBAL = "__TSR_CSP_NONCE";

/** Exact suffix TanStack Start emits on the hydration barrier script. */
export const BARRIER_SCRIPT_REMOVE = "document.currentScript.remove()";

/**
 * Class `$tsr` is not a valid unescaped CSS identifier (`script.$tsr` throws).
 * Attribute selector matches the barrier TanStack emits (`class="$tsr"`).
 */
export const TSR_SCRIPT_SELECTOR = 'script[class="$tsr"]';

/**
 * Same inline script: copy `script.nonce` onto `self.__TSR_CSP_NONCE`
 * and `meta[property="csp-nonce"]` (what TanStack `load-client` reads)
 * before the barrier removes itself. Not a second `<script>`.
 */
export const BARRIER_CONSUME_NONCE = `((s)=>{if(s&&s.nonce){self.${TSR_CSP_NONCE_GLOBAL}=s.nonce;let m=document.querySelector('meta[property="csp-nonce"]');if(!m){m=document.createElement('meta');m.setAttribute('property','csp-nonce');document.head.appendChild(m)}m.setAttribute('content',s.nonce)}s.remove()})(document.currentScript)`;

type NonceHolder = { [TSR_CSP_NONCE_GLOBAL]?: string };

type ScriptLike = {
  nonce?: string;
  tagName?: string;
  getAttribute?: (name: string) => string | null;
};

function isUsableNonce(value: unknown): value is string {
  return typeof value === "string" && /^[A-Za-z0-9+/=_-]+$/.test(value);
}

function safeQuerySelector(selector: string): ScriptLike | null {
  try {
    return document.querySelector(selector);
  } catch {
    return null;
  }
}

function scriptNonce(el: ScriptLike | null | undefined): string | undefined {
  if (!el) return undefined;
  if (el.tagName && el.tagName.toUpperCase() !== "SCRIPT") return undefined;
  // After parse, `getAttribute("nonce")` is empty in browsers; the IDL
  // `.nonce` property still holds the value stamped on the tag.
  const fromIdl = typeof el.nonce === "string" ? el.nonce : "";
  const fromAttr = el.getAttribute?.("nonce") ?? "";
  const n = fromIdl || fromAttr;
  return isUsableNonce(n) ? n : undefined;
}

function holder(): NonceHolder {
  return globalThis as typeof globalThis & NonceHolder;
}

/**
 * TanStack Start's client hydration overwrites `router.options.ssr` from
 * `meta[property="csp-nonce"]`. Publish there so `<Scripts />` keeps the
 * shell nonce — still not a CSP header.
 */
function publishTanStackNonceMeta(nonce: string): void {
  if (typeof document === "undefined") return;
  let meta = document.querySelector('meta[property="csp-nonce"]');
  if (!meta) {
    meta = document.createElement("meta");
    meta.setAttribute("property", "csp-nonce");
    document.head?.appendChild(meta);
  }
  if (meta.getAttribute("content") !== nonce) {
    meta.setAttribute("content", nonce);
  }
}

/** Persist a nonce already seen on the shell so later Scripts reuse it. */
export function rememberCspNonce(nonce: string | undefined): void {
  if (!isUsableNonce(nonce)) return;
  holder()[TSR_CSP_NONCE_GLOBAL] = nonce;
  publishTanStackNonceMeta(nonce);
}

export function clearRememberedCspNonce(): void {
  delete holder()[TSR_CSP_NONCE_GLOBAL];
}

/**
 * Read the nonce injected on the hydration shell.
 * Order: barrier stash (`self.__TSR_CSP_NONCE`) → `# $tsr-stream-barrier`
 * → `script.$tsr` → any remaining `script.nonce` (module entry).
 */
export function readShellCspNonce(): string | undefined {
  const cached = holder()[TSR_CSP_NONCE_GLOBAL];
  if (isUsableNonce(cached)) return cached;

  if (typeof document === "undefined") return undefined;

  const fromMeta = document
    .querySelector('meta[property="csp-nonce"]')
    ?.getAttribute("content");
  if (isUsableNonce(fromMeta)) {
    rememberCspNonce(fromMeta);
    return fromMeta;
  }

  const fromBarrier = scriptNonce(document.getElementById("$tsr-stream-barrier"));
  if (fromBarrier) {
    rememberCspNonce(fromBarrier);
    return fromBarrier;
  }

  const fromTsr = scriptNonce(safeQuerySelector(TSR_SCRIPT_SELECTOR));
  if (fromTsr) {
    rememberCspNonce(fromTsr);
    return fromTsr;
  }

  for (const el of document.querySelectorAll("script")) {
    const n = scriptNonce(el);
    if (n) {
      rememberCspNonce(n);
      return n;
    }
  }

  return undefined;
}

/** Rewrite the generated barrier so it consumes its own `nonce` attribute. */
export function rewriteBarrierToConsumeNonce(html: string): string {
  if (!html.includes(BARRIER_SCRIPT_REMOVE)) return html;
  if (html.includes(`self.${TSR_CSP_NONCE_GLOBAL}`)) return html;
  return html.split(BARRIER_SCRIPT_REMOVE).join(BARRIER_CONSUME_NONCE);
}
