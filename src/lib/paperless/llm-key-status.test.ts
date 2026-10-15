import { describe, it, beforeEach } from "node:test";
import assert from "node:assert/strict";
import {
  buildLlmKeyRequestBody,
  canonicalLlmProvider,
  emptySettingsPayload,
  LLM_KEYS_PATH,
  llmKeyConfiguredBadge,
  llmKeyConfiguredToast,
  llmKeyMutationErrorMessage,
  llmKeyStatusForProvider,
  mergeLlmKeyIntoSettings,
  normalizeLlmKeyPatchResult,
  normalizeLlmKeyStatus,
  normalizeSettingsPayload,
  readConfiguredProviders,
  resolveHasConfiguredKey,
  safeMasked,
  sameLlmProvider,
  SETTINGS_PATH,
  writeConfiguredProvider,
} from "./llm-key-status.ts";
import { keyPlaceholder } from "./llm-settings.ts";

/** Fixtures use redacted placeholders only — never a real secret. */
const REDACTED = "***";
const LEAK_SK = "sk-REDACTED";
const LEAK_XAI = "xai-REDACTED";

function mockLocalStorage() {
  const memory = new Map<string, string>();
  (globalThis as { localStorage?: Storage }).localStorage = {
    getItem: (k: string) => memory.get(k) ?? null,
    setItem: (k: string, v: string) => {
      memory.set(k, v);
    },
    removeItem: (k: string) => {
      memory.delete(k);
    },
    clear: () => memory.clear(),
    key: () => null,
    get length() {
      return memory.size;
    },
  } as Storage;
  return memory;
}

describe("llm-keys paths and POST/PATCH body", () => {
  it("uses /api/settings and /api/settings/llm-keys — never /api/settings/llm", () => {
    assert.equal(SETTINGS_PATH, "/api/settings");
    assert.equal(LLM_KEYS_PATH, "/api/settings/llm-keys");
    assert.notEqual(LLM_KEYS_PATH, "/api/settings/llm");
    assert.doesNotMatch(LLM_KEYS_PATH, /\/api\/settings\/llm$/);
  });

  it("sends provider=xai (not spacexai/openai) and field apiKey", () => {
    const body = buildLlmKeyRequestBody({ provider: "SpaceXAi", apiKey: REDACTED });
    assert.equal(body.provider, "xai");
    assert.equal(body.clear, false);
    assert.equal(body.apiKey, REDACTED);
    assert.deepEqual(Object.keys(body).sort(), ["apiKey", "clear", "provider"]);
    assert.equal(buildLlmKeyRequestBody({ provider: "spacexai", apiKey: REDACTED }).provider, "xai");
    assert.notEqual(buildLlmKeyRequestBody({ provider: "openai", apiKey: REDACTED }).provider, "xai");
  });

  it("omits apiKey when clearing — never required on clear", () => {
    const body = buildLlmKeyRequestBody({ provider: "xai", clear: true, apiKey: REDACTED });
    assert.equal(body.provider, "xai");
    assert.equal(body.clear, true);
    assert.equal("apiKey" in body, false);
  });

  it("success copy is FR « clé configurée » with SpaceXAi — no model names, no secret", () => {
    assert.equal(llmKeyConfiguredBadge(), "Clé configurée");
    const toast = llmKeyConfiguredToast("xai");
    assert.match(toast, /Clé API SpaceXAi configurée/);
    assert.doesNotMatch(toast, /Grok|ChatGPT|Claude|GPT-/i);
    assert.doesNotMatch(toast, /sk-|REDACTED/);
  });

  it("uses masked placeholder •••••••• when a key is configured", () => {
    assert.equal(keyPlaceholder("xai", true), "••••••••");
    assert.equal(keyPlaceholder("xai", false), "Nouvelle clé…");
  });
});

describe("canonical LLM provider ids", () => {
  it("maps SpaceXAi aliases to xai without using model names", () => {
    assert.equal(canonicalLlmProvider("xai"), "xai");
    assert.equal(canonicalLlmProvider("XAI"), "xai");
    assert.equal(canonicalLlmProvider("SpaceXAi"), "xai");
    assert.equal(canonicalLlmProvider("spacexai"), "xai");
    assert.equal(sameLlmProvider("SpaceXAi", "xai"), true);
    assert.equal(sameLlmProvider("openai", "anthropic"), false);
  });
});

describe("safeMasked never keeps key material", () => {
  it("accepts bullets and drops leaked-looking values", () => {
    assert.equal(safeMasked(""), "");
    assert.equal(safeMasked("••••••••"), "••••••••");
    assert.equal(safeMasked("…wxyz"), "…wxyz");
    assert.equal(safeMasked(LEAK_SK), "");
    assert.equal(safeMasked(LEAK_XAI), "");
    assert.equal(safeMasked("api_key=***"), "");
    assert.equal(safeMasked("xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"), "");
  });
});

describe("normalize GET /api/settings (camel + snake)", () => {
  it("reads llmKeys.hasKey (camelCase)", () => {
    const p = normalizeSettingsPayload({
      ocrProvider: "xai",
      ocrLanguage: "fr",
      autoMatch: true,
      aiAvailable: true,
      llmKeys: [{ provider: "xai", hasKey: true, masked: "" }],
    });
    assert.equal(p.llmKeysFromApi, true);
    assert.equal(p.ocrProvider, "xai");
    const row = llmKeyStatusForProvider(p.llmKeys, "xai");
    assert.equal(row?.hasKey, true);
    assert.equal(row?.masked, "");
    assert.equal(resolveHasConfiguredKey("xai", p.llmKeys, p.llmKeysFromApi), true);
  });

  it("reads llm_keys.has_key (snake_case)", () => {
    const p = normalizeSettingsPayload({
      ocr_provider: "xai",
      ocr_language: "fr",
      auto_match: true,
      ai_available: false,
      llm_keys: [{ provider: "xai", has_key: true, masked: REDACTED }],
    });
    assert.equal(p.llmKeysFromApi, true);
    assert.equal(llmKeyStatusForProvider(p.llmKeys, "SpaceXAi")?.hasKey, true);
  });

  it("treats omitted llmKeys as unknown (local flag fallback)", () => {
    const memory = mockLocalStorage();
    const p = normalizeSettingsPayload({
      ocrProvider: "xai",
      ocrLanguage: "fr",
      autoMatch: true,
      aiAvailable: false,
    });
    assert.equal(p.llmKeysFromApi, false);
    assert.deepEqual(p.llmKeys, []);
    assert.equal(resolveHasConfiguredKey("xai", p.llmKeys, p.llmKeysFromApi), false);
    writeConfiguredProvider("xai", true);
    assert.equal(resolveHasConfiguredKey("xai", p.llmKeys, false), true);
    const stored = memory.get("paperrust.llm.configured");
    assert.ok(stored);
    assert.doesNotMatch(stored, /sk-|xai-|apiKey|api_key|REDACTED/i);
    assert.deepEqual(JSON.parse(stored as string), ["xai"]);
  });

  it("never copies a leaked apiKey field from a GET body", () => {
    const p = normalizeSettingsPayload({
      ocrProvider: "xai",
      llmKeys: [
        {
          provider: "xai",
          hasKey: true,
          masked: LEAK_XAI,
          apiKey: LEAK_SK,
          api_key: LEAK_SK,
        },
      ],
    });
    const json = JSON.stringify(p);
    assert.doesNotMatch(json, /sk-REDACTED|xai-REDACTED|apiKey|api_key/);
    assert.equal(p.llmKeys[0]?.masked, "");
    assert.equal(p.llmKeys[0]?.hasKey, true);
  });
});

describe("PATCH /api/settings/llm-keys result", () => {
  it("normalizes hasKey and ignores leaked masked material", () => {
    const r = normalizeLlmKeyPatchResult(
      { ok: true, provider: "xai", hasKey: true, masked: LEAK_XAI },
      "xai",
    );
    assert.equal(r.ok, true);
    assert.equal(r.provider, "xai");
    assert.equal(r.hasKey, true);
    assert.equal(r.masked, "");
  });

  it("merges the patch into settings cache without wiping other providers", () => {
    const prev = normalizeSettingsPayload({
      ocrProvider: "xai",
      llmKeys: [
        { provider: "xai", hasKey: false, masked: "" },
        { provider: "openai", hasKey: true, masked: "••••••••" },
      ],
    });
    const next = mergeLlmKeyIntoSettings(prev, {
      provider: "xai",
      hasKey: true,
      masked: "",
    });
    assert.equal(llmKeyStatusForProvider(next.llmKeys, "xai")?.hasKey, true);
    assert.equal(llmKeyStatusForProvider(next.llmKeys, "openai")?.hasKey, true);
    assert.equal(next.llmKeysFromApi, true);
  });

  it("can seed cache when GET never returned settings", () => {
    const next = mergeLlmKeyIntoSettings(undefined, {
      provider: "xai",
      hasKey: true,
      masked: "",
    });
    assert.equal(resolveHasConfiguredKey("xai", next.llmKeys, next.llmKeysFromApi), true);
    assert.equal(next.ocrProvider, emptySettingsPayload().ocrProvider);
  });
});

describe("configured-provider flag (never a secret)", () => {
  beforeEach(() => {
    mockLocalStorage();
  });

  it("round-trips provider ids only", () => {
    assert.deepEqual(readConfiguredProviders(), []);
    writeConfiguredProvider("SpaceXAi", true);
    assert.deepEqual(readConfiguredProviders(), ["xai"]);
    writeConfiguredProvider("openai", true);
    assert.deepEqual(readConfiguredProviders().sort(), ["openai", "xai"]);
    writeConfiguredProvider("xai", false);
    assert.deepEqual(readConfiguredProviders(), ["openai"]);
  });

  it("server list wins over a stale local flag", () => {
    writeConfiguredProvider("xai", true);
    const keys = [normalizeLlmKeyStatus({ provider: "xai", hasKey: false, masked: "" })!];
    assert.equal(resolveHasConfiguredKey("xai", keys, true), false);
    assert.equal(resolveHasConfiguredKey("xai", [], false), true);
  });
});

describe("FR error toasts (no secrets, provider names only)", () => {
  it("maps generic API failures to a SpaceXAi FR sentence", () => {
    const err = Object.assign(new Error("request_failed"), { status: 400 });
    const msg = llmKeyMutationErrorMessage(err, "xai");
    assert.match(msg, /clé API SpaceXAi/);
    assert.doesNotMatch(msg, /Grok|ChatGPT|Claude|GPT-/i);
    assert.doesNotMatch(msg, /sk-|xai-REDACTED|request_failed/);
  });

  it("scrubs an error that accidentally includes key material", () => {
    const err = new Error(`échec ${LEAK_SK}`);
    const msg = llmKeyMutationErrorMessage(err, "xai");
    assert.doesNotMatch(msg, /sk-REDACTED/);
    assert.match(msg, /clé API SpaceXAi|Impossible/);
  });

  it("keeps a short FR server message when safe", () => {
    const err = new Error("Format de clé refusé.");
    assert.equal(llmKeyMutationErrorMessage(err, "openai"), "Format de clé refusé.");
  });
});
