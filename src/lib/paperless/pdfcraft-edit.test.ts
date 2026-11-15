import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  PDF_APPLY_PRIMARY,
  PDF_APPLY_SUCCESS,
  PDF_AUTH_REQUIRED,
  PDF_EDIT_HINT_NO_FILE,
  PDF_EDIT_LABEL,
  PDF_EDIT_NOT_READY,
  PDF_EDIT_SETTINGS_TOC_ID,
  PDF_READ_LABEL,
  PDF_SESSION_EXPIRED,
  bindPdfCraftSave,
  documentFileEditTarget,
  fileTokenResponseHeaders,
  isOfficeDocument,
  isPdfDocument,
  isPdfEditNotReadyStatus,
  isPdfEditSessionExpired,
  parsePdfEditSession,
  pdfEditApiPath,
  pdfEditCommitApiPath,
  pdfEditDisabledReason,
  pdfcraftEditorUrl,
  pdfcraftFrameProps,
  reasonFromHttpStatus,
  requestEditorSave,
  toastForPdfEditCommitFailure,
  toastForPdfEditFailure,
} from "./pdfcraft-edit.ts";

const SOURCE = "https://archive.example/api/edit/pdfcraft/TOKEN_REDACTED/file";
const CONFIGURED = "https://archive.example";

describe("pdfcraft editor save request", () => {
  it("calls file.save when the editor exposes it, and refuses a window without it", async () => {
    const calls: string[] = [];
    const win = {
      pdfcraft: {
        execute: (command: string) => {
          calls.push(command);
          return Promise.resolve(undefined);
        },
      },
    };
    assert.equal(requestEditorSave(win), true);
    assert.deepEqual(calls, ["file.save"]);
    assert.equal(requestEditorSave(null), false);
    assert.equal(requestEditorSave({}), false);
    assert.equal(requestEditorSave({ pdfcraft: {} }), false);
  });
});

describe("pdfcraft editor session", () => {
  it("opens ?file= on the Paperrust origin and never fileUrl", () => {
    const url = pdfcraftEditorUrl("https://archive.example", SOURCE);
    const parsed = new URL(url);
    assert.equal(parsed.origin, "https://archive.example");
    assert.equal(parsed.pathname, "/pdfcraft/");
    assert.equal(parsed.searchParams.get("file"), SOURCE);
    assert.equal(parsed.searchParams.get("host"), "parent");
    assert.equal(parsed.searchParams.has("control"), true);
    assert.equal(parsed.searchParams.get("fileUrl"), null);
    const tab = new URL(pdfcraftEditorUrl("https://archive.example", SOURCE, "tab"));
    assert.equal(tab.searchParams.get("file"), SOURCE);
    assert.equal(tab.searchParams.get("host"), null);
    assert.equal(tab.searchParams.has("control"), false);
    assert.equal(tab.searchParams.get("fileUrl"), null);
    assert.equal(url.includes("fileUrl"), false);
    const frame = pdfcraftFrameProps("https://archive.example", SOURCE, CONFIGURED);
    assert.equal(frame.referrerPolicy, "no-referrer");
    assert.match(frame.src, /\?file=/);
    assert.equal(frame.src.includes("fileUrl"), false);
    assert.equal(frame.src.startsWith("https://archive.example/pdfcraft/"), true);
  });

  it("file response CORS is the configured origin, not *, and sets no cookie", () => {
    const headers = fileTokenResponseHeaders(CONFIGURED, CONFIGURED);
    assert.equal(headers.get("access-control-allow-origin"), CONFIGURED);
    assert.notEqual(headers.get("access-control-allow-origin"), "*");
    assert.equal(headers.get("set-cookie"), null);
    assert.equal(headers.has("set-cookie"), false);
    assert.equal(headers.get("referrer-policy"), "no-referrer");
    const denied = fileTokenResponseHeaders(CONFIGURED, "https://evil.example");
    assert.equal(denied.get("access-control-allow-origin"), null);
    const star = fileTokenResponseHeaders("*", "*");
    assert.equal(star.get("access-control-allow-origin"), null);
  });

  it("pdfcraft:save calls the shipped commit with the event bytes", async () => {
    const bytes = new Uint8Array([0x25, 0x50, 0x44, 0x46, 0x2d, 0x31, 0x2e, 0x37]).buffer;
    const calls: { url: string; body: FormData }[] = [];
    const previous = globalThis.fetch;
    globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
      calls.push({ url: String(input), body: init?.body as FormData });
      return new Response(JSON.stringify({ ok: true, documentId: 7, ocr: { ok: true } }), {
        status: 200,
        headers: { "content-type": "application/json" },
      });
    }) as typeof fetch;
    try {
      const target = new EventTarget();
      bindPdfCraftSave(target, { documentId: 7, token: "commit-token" });
      target.dispatchEvent(
        new CustomEvent("pdfcraft:save", {
          detail: { name: "edited.pdf", bytes },
        }),
      );
      for (let i = 0; i < 20 && calls.length === 0; i += 1) {
        await new Promise((resolve) => setTimeout(resolve, 10));
      }
      assert.equal(calls.length, 1);
      assert.match(calls[0].url, /\/api\/documents\/7\/edit\/pdfcraft\/commit$/);
      const file = calls[0].body.get("file");
      assert.ok(file instanceof Blob);
      const saved = new Uint8Array(await file.arrayBuffer());
      assert.deepEqual(Array.from(saved), [0x25, 0x50, 0x44, 0x46, 0x2d, 0x31, 0x2e, 0x37]);
      assert.equal(calls[0].body.get("token"), "commit-token");
    } finally {
      globalThis.fetch = previous;
    }
  });

  it("same-origin pdfcraft:save commits, then answers saved", async () => {
    const bytes = new Uint8Array([0x25, 0x50, 0x44, 0x46, 0x2d, 0x31, 0x2e, 0x37]).buffer;
    const calls: string[] = [];
    const previous = globalThis.fetch;
    const previousLocation = (globalThis as { location?: { origin: string } }).location;
    Object.defineProperty(globalThis, "location", {
      value: { origin: "https://archive.example" },
      configurable: true,
    });
    const channel = new MessageChannel();
    const saved: unknown[] = [];
    channel.port2.onmessage = (event) => saved.push(event.data);
    globalThis.fetch = (async () => {
      calls.push("commit");
      return new Response(JSON.stringify({ ok: true, documentId: 7 }), {
        status: 200,
        headers: { "content-type": "application/json" },
      });
    }) as typeof fetch;
    try {
      const target = new EventTarget();
      bindPdfCraftSave(target, { documentId: 7, token: "commit-token" }, undefined, () => channel.port1);
      target.dispatchEvent(
        new MessageEvent("message", {
          data: { type: "pdfcraft:save", name: "edited.pdf", bytes, id: "save-1" },
          origin: "https://evil.example",
          source: channel.port1,
        }),
      );
      target.dispatchEvent(
        new MessageEvent("message", {
          data: { type: "pdfcraft:save", name: "edited.pdf", bytes, id: "save-1" },
          origin: "https://archive.example",
          source: channel.port2,
        }),
      );
      target.dispatchEvent(
        new MessageEvent("message", {
          data: { type: "pdfcraft:save", name: "edited.pdf", bytes, id: "save-1" },
          origin: "https://archive.example",
          source: channel.port1,
        }),
      );
      for (let i = 0; i < 20 && saved.length === 0; i += 1) {
        await new Promise((resolve) => setTimeout(resolve, 10));
      }
      assert.deepEqual(calls, ["commit"]);
      assert.equal(saved.length, 1);
      assert.deepEqual(saved[0], { type: "pdfcraft:saved", id: "save-1" });
    } finally {
      channel.port1.close();
      channel.port2.close();
      globalThis.fetch = previous;
      if (previousLocation) {
        Object.defineProperty(globalThis, "location", { value: previousLocation, configurable: true });
      } else {
        delete (globalThis as { location?: unknown }).location;
      }
    }
  });
});

function sampleSession(over: Record<string, unknown> = {}) {
  return {
    sessionId: "sid-1",
    token: "sid.exp.doc.uid.mac",
    editorUrl: "https://archive.example/pdfcraft/?file=https%3A%2F%2Fexample%2Ffile&host=parent",
    sourceFileUrl: "https://archive.example/api/edit/pdfcraft/tok/file",
    commitUrl: "https://archive.example/api/edit/pdfcraft/tok/commit",
    expiresAt: "2099-01-01T00:00:00.000Z",
    filename: "facture.pdf",
    documentId: 42,
    mimeType: "application/pdf",
    extraIgnored: true,
    ...over,
  };
}

describe("pdf edit session", () => {
  it("detects PDF versus Office and routes the editor", () => {
    assert.equal(isPdfDocument("application/pdf", "scan.png"), true);
    assert.equal(isPdfDocument("application/octet-stream", "facture.PDF"), true);
    assert.equal(isPdfDocument("image/png", "photo.png"), false);
    assert.equal(isOfficeDocument("application/octet-stream", "notes.docx"), true);
    assert.equal(documentFileEditTarget("application/pdf", "a.pdf"), "pdfcraft");
    assert.equal(documentFileEditTarget("", "contrat.docx"), "wordcraft");
    assert.equal(documentFileEditTarget("", "table.xlsx"), "m365");
    assert.equal(documentFileEditTarget("", "deck.pptx"), "m365");
    assert.equal(documentFileEditTarget("image/jpeg", "photo.jpg"), null);
    assert.equal(pdfEditApiPath(42), "/api/documents/42/edit/pdfcraft");
    assert.equal(pdfEditCommitApiPath(42), "/api/documents/42/edit/pdfcraft/commit");
    assert.equal(PDF_EDIT_SETTINGS_TOC_ID, "settings-pdf-edit");
  });

  it("parses the session JSON and ignores extra fields", () => {
    const session = parsePdfEditSession(sampleSession());
    assert.ok(session);
    assert.equal(session?.documentId, 42);
    assert.equal(Object.hasOwn(session ?? {}, "extraIgnored"), false);
    assert.equal(parsePdfEditSession({ editorUrl: "https://archive.example/pdfcraft/" }), null);
    assert.equal(parsePdfEditSession(sampleSession({ editorUrl: "javascript:alert(1)" })), null);
    const live = parsePdfEditSession(sampleSession())!;
    assert.equal(isPdfEditSessionExpired(live, Date.parse("2026-01-01T00:00:00Z")), false);
    const dead = parsePdfEditSession(sampleSession({ expiresAt: "2020-01-01T00:00:00.000Z" }))!;
    assert.equal(isPdfEditSessionExpired(dead, Date.parse("2026-01-01T00:00:00Z")), true);
  });

  it("maps French edit errors", () => {
    assert.equal(PDF_EDIT_LABEL, "Éditer");
    assert.equal(PDF_READ_LABEL, "Lecture");
    assert.equal(PDF_APPLY_PRIMARY, "Enregistrer");
    assert.equal(PDF_APPLY_SUCCESS, "PDF mis à jour, OCR relancé");
    assert.equal(PDF_SESSION_EXPIRED, "La session d’édition a expiré.");
    assert.equal(isPdfEditNotReadyStatus(404), true);
    assert.equal(isPdfEditNotReadyStatus(500), false);
    assert.equal(reasonFromHttpStatus(503), "not_ready");
    assert.equal(reasonFromHttpStatus(500), "unavailable");
    assert.equal(toastForPdfEditFailure("not_ready"), PDF_EDIT_NOT_READY);
    assert.equal(toastForPdfEditFailure("auth"), PDF_AUTH_REQUIRED);
    const auth = Object.assign(new Error("auth_required"), { name: "AuthRequired" });
    assert.equal(toastForPdfEditCommitFailure(auth), PDF_AUTH_REQUIRED);
    assert.equal(toastForPdfEditCommitFailure(new Error("boom")), "Impossible d’enregistrer le PDF.");
    assert.equal(pdfEditDisabledReason({ target: "pdfcraft", hasFile: false }), PDF_EDIT_HINT_NO_FILE);
    assert.equal(pdfEditDisabledReason({ target: "pdfcraft", hasFile: true }), null);
    assert.equal(pdfEditDisabledReason({ target: "m365", hasFile: true }), "Microsoft 365 n’est pas connecté.");
    assert.equal(pdfEditDisabledReason({ target: "wordcraft", hasFile: true }), null);
    assert.equal(pdfEditDisabledReason({ target: "wordcraft", hasFile: false }), PDF_EDIT_HINT_NO_FILE);
  });

  it("production dist does not inline dev http flags", (t) => {
    const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");
    const assets = path.join(root, "dist/assets");
    if (!existsSync(assets)) {
      t.skip("dist/assets is created by npm run build");
      return;
    }
    const files = readdirSync(assets).filter((name) => name.endsWith(".js"));
    assert.ok(files.length >= 1, "expected at least one JS chunk in dist/assets");
    for (const name of files) {
      const src = readFileSync(path.join(assets, name), "utf8");
      assert.equal(src.includes("import.meta.env"), false, name);
      assert.doesNotMatch(src, /allowHttp\s*:\s*(?:!0|true)\b/, name);
    }
  });
});
