import { describe, it, beforeEach } from "node:test";
import assert from "node:assert/strict";
import {
  acceptPaperlessJobId,
  buildPaperlessImportBody,
  isPaperlessImportTerminal,
  isSafePaperlessJobId,
  normalizePaperlessImportJob,
  paperlessImportPhaseLabel,
  paperlessImportPollIntervalMs,
  paperlessImportStateLabel,
  paperlessImportUiState,
  PAPERLESS_IMPORT_MODAL_HELP,
  PAPERLESS_IMPORT_POLL_MAX_MS,
  PAPERLESS_IMPORT_SECTION_HELP,
  PAPERLESS_IMPORT_SECTION_HINT,
  PAPERLESS_IMPORT_TOC_ID,
  PAPERLESS_IMPORT_TOKEN_HINT,
  readActivePaperlessImportJobId,
  validatePaperlessHttpsUrl,
  writeActivePaperlessImportJobId,
} from "./paperless-import.ts";

describe("paperless-import HTTPS URL", () => {
  it("accepts a public https URL", () => {
    assert.equal(validatePaperlessHttpsUrl("https://paperless.example"), null);
    assert.equal(validatePaperlessHttpsUrl("  https://paperless.example/  "), null);
  });

  it("rejects empty, http, and invalid URLs in FR", () => {
    assert.match(validatePaperlessHttpsUrl("") ?? "", /requise/);
    assert.match(validatePaperlessHttpsUrl("http://paperless.example") ?? "", /https:\/\//);
    assert.match(validatePaperlessHttpsUrl("paperless.example") ?? "", /https:\/\//);
    assert.match(validatePaperlessHttpsUrl("https://") ?? "", /invalide|https/);
  });

  it("rejects userinfo (user:pass@) in FR", () => {
    assert.match(
      validatePaperlessHttpsUrl("https://user:pass@paperless.example/api") ?? "",
      /identifiants|user:pass/,
    );
    assert.match(validatePaperlessHttpsUrl("https://admin@paperless.example") ?? "", /identifiants|user:pass/);
    assert.equal(validatePaperlessHttpsUrl("https://paperless.example/path?x=1"), null);
  });

  it("does not client-block private or LAN HTTPS hosts (SSRF is server-side)", () => {
    assert.equal(validatePaperlessHttpsUrl("https://paperless.internal.example"), null);
    assert.equal(validatePaperlessHttpsUrl("https://paperless.internal.example/api"), null);
    assert.equal(validatePaperlessHttpsUrl("https://192.0.2.10"), null);
    assert.equal(validatePaperlessHttpsUrl("https://192.168.1.10"), null);
    assert.equal(validatePaperlessHttpsUrl("https://172.16.0.5:8443"), null);
    const blocked = [
      validatePaperlessHttpsUrl("https://paperless.internal.example") ?? "",
      validatePaperlessHttpsUrl("https://192.0.2.10") ?? "",
    ];
    for (const msg of blocked) {
      assert.doesNotMatch(msg, /hôte privé|interdit|loopback|allowlist|liste blanche/i);
    }
  });
});

describe("paperless-import FR copy (any public HTTPS)", () => {
  const copy = [
    PAPERLESS_IMPORT_SECTION_HELP,
    PAPERLESS_IMPORT_SECTION_HINT,
    PAPERLESS_IMPORT_MODAL_HELP,
    PAPERLESS_IMPORT_TOKEN_HINT,
  ];

  it("supports any HTTPS instance and never implies allowlist-only", () => {
    for (const s of copy) {
      assert.doesNotMatch(s, /allowlist|liste blanche|hôtes? autoris|uniquement les hôtes/i);
    }
    assert.match(PAPERLESS_IMPORT_SECTION_HELP, /n'importe quelle instance/i);
    assert.match(PAPERLESS_IMPORT_SECTION_HELP, /HTTPS/i);
    assert.match(PAPERLESS_IMPORT_MODAL_HELP, /Toute instance Paperless-ngx en HTTPS/);
    assert.match(PAPERLESS_IMPORT_SECTION_HINT, /HTTPS publique/);
    assert.match(PAPERLESS_IMPORT_SECTION_HINT, /privé ou LAN/);
    assert.match(PAPERLESS_IMPORT_SECTION_HINT, /côté serveur/);
  });
});

describe("paperless-import poll + terminal", () => {
  it("backs off then caps", () => {
    assert.equal(paperlessImportPollIntervalMs(0), 1200);
    assert.equal(paperlessImportPollIntervalMs(1), 1800);
    assert.equal(paperlessImportPollIntervalMs(4), 6000);
    assert.equal(paperlessImportPollIntervalMs(9), PAPERLESS_IMPORT_POLL_MAX_MS);
  });

  it("stops polling on terminal statuses only", () => {
    assert.equal(isPaperlessImportTerminal("queued"), false);
    assert.equal(isPaperlessImportTerminal("running"), false);
    assert.equal(isPaperlessImportTerminal("done"), true);
    assert.equal(isPaperlessImportTerminal("cancelled"), true);
    assert.equal(isPaperlessImportTerminal("failed"), true);
    assert.equal(isPaperlessImportTerminal(null), false);
  });
});

describe("paperless-import UI states FR", () => {
  it("maps job status to idle/running/done/partial/failed/cancelled", () => {
    assert.equal(paperlessImportUiState(null), "idle");
    assert.equal(
      paperlessImportUiState({
        jobId: "j",
        status: "running",
        phase: "importing",
        done: 1,
        total: 2,
        errors: 0,
        errorSamples: [],
        message: "",
      }),
      "running",
    );
    assert.equal(
      paperlessImportUiState({
        jobId: "j",
        status: "done",
        phase: null,
        done: 2,
        total: 2,
        errors: 0,
        errorSamples: [],
        message: "",
      }),
      "done",
    );
    assert.equal(
      paperlessImportUiState({
        jobId: "j",
        status: "done",
        phase: null,
        done: 2,
        total: 2,
        errors: 3,
        errorSamples: [],
        message: "",
      }),
      "partial",
    );
    assert.equal(
      paperlessImportUiState({
        jobId: "j",
        status: "failed",
        phase: null,
        done: 0,
        total: 0,
        errors: 1,
        errorSamples: [],
        message: "boom",
      }),
      "failed",
    );
    assert.equal(
      paperlessImportUiState({
        jobId: "j",
        status: "cancelled",
        phase: null,
        done: 1,
        total: 4,
        errors: 0,
        errorSamples: [],
        message: "",
      }),
      "cancelled",
    );
  });

  it("exposes French labels and ToC id", () => {
    assert.equal(PAPERLESS_IMPORT_TOC_ID, "settings-paperless-import");
    assert.equal(paperlessImportStateLabel("idle"), "Aucun import en cours");
    assert.equal(paperlessImportStateLabel("running"), "Import en cours");
    assert.equal(paperlessImportStateLabel("done"), "Import terminé");
    assert.equal(paperlessImportStateLabel("partial"), "Import terminé avec des erreurs");
    assert.equal(paperlessImportStateLabel("failed"), "Échec de l'import");
    assert.equal(paperlessImportStateLabel("cancelled"), "Import annulé");
    assert.equal(paperlessImportPhaseLabel("listing"), "Inventaire des documents…");
    assert.equal(paperlessImportPhaseLabel(null, "queued"), "En file d'attente…");
  });
});

describe("paperless-import body + job normalize", () => {
  it("applies contract defaults when omitted", () => {
    const body = buildPaperlessImportBody({
      baseUrl: " https://paperless.example/ ",
      token: "secret-token",
    });
    assert.deepEqual(body, {
      baseUrl: "https://paperless.example/",
      token: "secret-token",
      skipOcrIfPresent: true,
      preserveTags: true,
      dryRun: false,
    });
  });

  it("normalizes a sparse status payload", () => {
    const job = normalizePaperlessImportJob(
      {
        jobId: "abc",
        status: "running",
        phase: "listing",
        done: "12",
        total: 100,
        errors: 2,
        errorSamples: [{ externalId: "42", message: "timeout" }, { junk: true }],
        message: "ok",
      },
      "fallback",
    );
    assert.equal(job.done, 12);
    assert.equal(job.total, 100);
    assert.equal(job.errors, 2);
    assert.deepEqual(job.errorSamples, [{ externalId: "42", message: "timeout" }]);
    assert.equal(job.phase, "listing");
  });
});

describe("paperless-import job id persistence (never the token)", () => {
  const memory = new Map<string, string>();

  beforeEach(() => {
    memory.clear();
    (globalThis as { sessionStorage?: Storage }).sessionStorage = {
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
  });

  it("stores only a safe jobId in sessionStorage", () => {
    writeActivePaperlessImportJobId("job-1");
    assert.equal(readActivePaperlessImportJobId(), "job-1");
    assert.equal([...memory.keys()].join(","), "paperrust.paperless-import.jobId");
    assert.equal([...memory.values()].join(","), "job-1");
    writeActivePaperlessImportJobId("not a token please /../../x");
    assert.equal(readActivePaperlessImportJobId(), null);
    assert.equal(memory.size, 0);
  });

  it("rejects unsafe job ids", () => {
    assert.equal(isSafePaperlessJobId("ok_job-1:2"), true);
    assert.equal(isSafePaperlessJobId("../etc/passwd"), false);
    assert.equal(isSafePaperlessJobId("has space"), false);
    assert.equal(isSafePaperlessJobId(""), false);
    assert.equal(acceptPaperlessJobId("job-ok"), "job-ok");
    assert.equal(acceptPaperlessJobId("  job-ok  "), "job-ok");
    assert.equal(acceptPaperlessJobId("../x"), null);
    assert.equal(acceptPaperlessJobId(""), null);
    assert.equal(acceptPaperlessJobId(undefined), null);
  });
});
