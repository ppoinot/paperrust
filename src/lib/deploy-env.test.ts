import { describe, it } from "node:test";
import assert from "node:assert/strict";
import {
  LOCAL_AXUM_ORIGIN,
  resolvePublicApiBase,
  trimOrigin,
  viteStirlingBaseUrl,
} from "./deploy-env.ts";

describe("VITE_API_URL / same-origin", () => {
  it("leaves the browser on same-origin /api when env is empty", () => {
    assert.equal(resolvePublicApiBase({ env: "", inBrowser: true }), "");
    assert.equal(resolvePublicApiBase({ env: "  ", inBrowser: true }), "");
  });

  it("uses local axum for SSR when env is empty", () => {
    assert.equal(resolvePublicApiBase({ env: "", inBrowser: false }), LOCAL_AXUM_ORIGIN);
  });

  it("honors an explicit API origin", () => {
    assert.equal(
      resolvePublicApiBase({ env: "https://archive.example/", inBrowser: true }),
      "https://archive.example",
    );
    assert.equal(trimOrigin("https://archive.example/"), "https://archive.example");
  });
});

describe("VITE_STIRLING_BASE_URL", () => {
  it("is a dev-only display fallback", () => {
    assert.equal(
      viteStirlingBaseUrl({ env: "https://stirling.example/", dev: true }),
      "https://stirling.example",
    );
    assert.equal(viteStirlingBaseUrl({ env: "https://stirling.example/", dev: false }), "");
    assert.equal(viteStirlingBaseUrl({ env: "", dev: true }), "");
  });
});
