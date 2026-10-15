import { describe, it, beforeEach } from "node:test";
import assert from "node:assert/strict";
import {
  BARRIER_CONSUME_NONCE,
  BARRIER_SCRIPT_REMOVE,
  TSR_CSP_NONCE_GLOBAL,
  TSR_SCRIPT_SELECTOR,
  clearRememberedCspNonce,
  readShellCspNonce,
  rememberCspNonce,
  rewriteBarrierToConsumeNonce,
} from "./csp-nonce.ts";

type ScriptStub = {
  tagName: string;
  nonce: string;
  getAttribute: (name: string) => string | null;
};

function scriptStub(nonce: string, attr = nonce): ScriptStub {
  return {
    tagName: "SCRIPT",
    nonce,
    getAttribute: (name) => (name === "nonce" ? attr : null),
  };
}

function withDocument<T>(
  doc: {
    getElementById?: (id: string) => ScriptStub | null;
    querySelector?: (sel: string) => { getAttribute: (n: string) => string | null } | ScriptStub | null;
    querySelectorAll?: (sel: string) => ScriptStub[];
    createElement?: (tag: string) => { setAttribute: (n: string, v: string) => void; getAttribute: (n: string) => string | null };
    head?: { appendChild: (el: unknown) => void };
  },
  fn: () => T,
): T {
  const prev = (globalThis as { document?: unknown }).document;
  const metas: Array<{ property?: string; content?: string }> = [];
  const created = {
    setAttribute(name: string, value: string) {
      if (name === "property") created.property = value;
      if (name === "content") created.content = value;
    },
    getAttribute(name: string) {
      if (name === "property") return created.property ?? null;
      if (name === "content") return created.content ?? null;
      return null;
    },
    property: undefined as string | undefined,
    content: undefined as string | undefined,
  };
  const userQS = doc.querySelector;
  (globalThis as { document?: unknown }).document = {
    getElementById: () => null,
    querySelectorAll: () => [],
    createElement: (tag: string) => {
      if (tag !== "meta") throw new Error(`unexpected createElement(${tag})`);
      return created;
    },
    head: {
      appendChild: (el: unknown) => {
        metas.push(el as { property?: string; content?: string });
      },
    },
    ...doc,
    querySelector: (sel: string) => {
      if (sel === 'meta[property="csp-nonce"]') {
        return metas[0]
          ? {
              getAttribute: (n: string) =>
                n === "content" ? (metas[0]?.content ?? null) : n === "property" ? "csp-nonce" : null,
              setAttribute: (n: string, v: string) => {
                if (n === "content" && metas[0]) metas[0].content = v;
              },
            }
          : null;
      }
      return userQS?.(sel) ?? null;
    },
  };
  try {
    return fn();
  } finally {
    if (prev === undefined) {
      delete (globalThis as { document?: unknown }).document;
    } else {
      (globalThis as { document?: unknown }).document = prev;
    }
  }
}

describe("csp-nonce", () => {
  beforeEach(() => {
    clearRememberedCspNonce();
  });

  it("returns undefined without a document or stamped script", () => {
    assert.equal(readShellCspNonce(), undefined);
    withDocument({}, () => {
      assert.equal(readShellCspNonce(), undefined);
    });
  });

  it("reads the barrier script nonce and remembers it", () => {
    const barrier = scriptStub("abc123XYZ");
    withDocument(
      {
        getElementById: (id) => (id === "$tsr-stream-barrier" ? barrier : null),
      },
      () => {
        assert.equal(readShellCspNonce(), "abc123XYZ");
      },
    );
    assert.equal(readShellCspNonce(), "abc123XYZ");
  });

  it("prefers the IDL nonce when getAttribute is empty (browser quirk)", () => {
    const barrier = scriptStub("idlOnlyNonce", "");
    withDocument(
      {
        getElementById: (id) => (id === "$tsr-stream-barrier" ? barrier : null),
      },
      () => {
        assert.equal(readShellCspNonce(), "idlOnlyNonce");
      },
    );
  });

  it("uses an attribute selector so class $tsr does not throw in querySelector", () => {
    assert.equal(TSR_SCRIPT_SELECTOR, 'script[class="$tsr"]');
    assert.equal(TSR_SCRIPT_SELECTOR.includes("script.$tsr"), false);
  });

  it("falls back to script.$tsr then any script.nonce", () => {
    const tsr = scriptStub("fromTsr");
    withDocument(
      {
        querySelector: (sel) => (sel === TSR_SCRIPT_SELECTOR ? tsr : null),
      },
      () => {
        assert.equal(readShellCspNonce(), "fromTsr");
      },
    );

    clearRememberedCspNonce();
    const moduleScript = scriptStub("fromModule");
    withDocument(
      {
        querySelectorAll: () => [moduleScript],
      },
      () => {
        assert.equal(readShellCspNonce(), "fromModule");
      },
    );
  });

  it("rejects nonces that could break a script attribute", () => {
    rememberCspNonce(`bad'nonce`);
    assert.equal(readShellCspNonce(), undefined);
    rememberCspNonce("ok_nonce-1+/=");
    assert.equal(readShellCspNonce(), "ok_nonce-1+/=");
  });

  it("rewrites the barrier body in place — no second script tag", () => {
    const html = `<script class="$tsr" id="$tsr-stream-barrier">$_TSR.e();${BARRIER_SCRIPT_REMOVE}</script><script type="module" src="/assets/x.js"></script>`;
    const next = rewriteBarrierToConsumeNonce(html);
    assert.ok(next.includes(BARRIER_CONSUME_NONCE));
    assert.ok(!next.includes(BARRIER_SCRIPT_REMOVE));
    assert.equal((next.match(/<script/g) ?? []).length, 2);
    assert.equal(rewriteBarrierToConsumeNonce(next), next);
  });

  it("leaves HTML untouched when there is no barrier remove()", () => {
    const html = `<script type="module" src="/a.js"></script>`;
    assert.equal(rewriteBarrierToConsumeNonce(html), html);
  });

  it("stashes onto the same global the reader consumes", () => {
    assert.ok(BARRIER_CONSUME_NONCE.includes(`self.${TSR_CSP_NONCE_GLOBAL}`));
    assert.ok(BARRIER_CONSUME_NONCE.includes('meta[property="csp-nonce"]'));
    (globalThis as Record<string, unknown>)[TSR_CSP_NONCE_GLOBAL] = "stashedFromBarrier";
    assert.equal(readShellCspNonce(), "stashedFromBarrier");
  });

  it("publishes meta[property=csp-nonce] so TanStack load-client can read it", () => {
    withDocument({}, () => {
      rememberCspNonce("metaNonce99");
      const meta = document.querySelector('meta[property="csp-nonce"]');
      assert.equal(meta?.getAttribute("content"), "metaNonce99");
    });
  });
});
