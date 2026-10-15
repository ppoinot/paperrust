import { describe, it } from "node:test";
import assert from "node:assert/strict";
import {
  M365_EDIT_HINT_DISCONNECTED,
  STIRLING_APPLY_BANNER_HELP,
  STIRLING_APPLY_BANNER_HELP_UPLOADED,
  STIRLING_APPLY_BANNER_TITLE,
  STIRLING_APPLY_OPEN_FILE,
  STIRLING_APPLY_OPEN_FILE_HELP,
  STIRLING_APPLY_PRIMARY,
  STIRLING_APPLY_PULL,
  STIRLING_APPLY_SUCCESS,
  STIRLING_AUTH_REQUIRED,
  STIRLING_EDIT_HINT_NO_FILE,
  STIRLING_EDIT_HINT_NOT_PDF,
  STIRLING_EDIT_LABEL,
  STIRLING_EDIT_NOT_READY,
  STIRLING_EDIT_STARTED,
  STIRLING_EDIT_STARTED_LOADED,
  STIRLING_EDIT_STARTED_NO_FILEURL,
  STIRLING_EDIT_STARTED_UPLOADED,
  STIRLING_EDIT_UNAVAILABLE,
  STIRLING_OPEN_FILE_FAILED,
  STIRLING_SESSION_EXPIRED,
  bannerHelpForStirlingSession,
  documentFileEditTarget,
  editorNavigateUrlForSession,
  editorUrlHasFileUrl,
  hasFileUrl,
  isBareStirlingLanding,
  isOfficeDocument,
  isOpenableEditorUrl,
  isPdfDocument,
  isStirlingEditNotReadyStatus,
  isStirlingSessionExpired,
  navigateStirlingEditorSession,
  navigateStirlingPopup,
  PAPERRUST_EDIT_WINDOW_NAME_PREFIX,
  PAPERRUST_FILEURL_WINDOW_NAME_PREFIX,
  fileUrlFromNavigateUrl,
  stampPaperrustEditName,
  openBlankEditorWindow,
  openStirlingEditor,
  openStirlingGesturePopup,
  openStirlingSourceFile,
  openUrlViaAnchor,
  parseStirlingEditSession,
  reasonFromHttpStatus,
  resolveEditorNavigateUrl,
  resolveSourceFileUrl,
  resolveStirlingEditHandoff,
  stirlingWindowNameForEdit,
  stirlingCommitApiPath,
  stirlingEditApiPath,
  stirlingEditDisabledReason,
  toastForStirlingCommitFailure,
  toastForStirlingEditFailure,
  toastForStirlingEditStarted,
} from "./stirling-edit.ts";

function sampleSession(over: Record<string, unknown> = {}) {
  return {
    sessionId: "sid-1",
    token: "sid.exp.doc.uid.mac",
    editorUrl: "https://stirling.example",
    sourceFileUrl: "https://paperrust.example/api/edit/stirling/tok/file",
    commitUrl: "https://paperrust.example/api/edit/stirling/tok/commit",
    expiresAt: "2099-01-01T00:00:00.000Z",
    filename: "facture.pdf",
    documentId: 42,
    mimeType: "application/pdf",
    stirlingUpload: "skipped",
    ...over,
  };
}

function fileUrlEditor(sourceFileUrl = "https://paperrust.example/api/edit/stirling/tok/file"): string {
  return `https://stirling.example/?fileUrl=${encodeURIComponent(sourceFileUrl)}`;
}

function expectedNavigateUrl(_fileUrl?: string): string {
  return "https://stirling.example/";
}

function expectedEditName(fileUrl: string, commitUrl: string): string {
  return stirlingWindowNameForEdit({ fileUrl, commitUrl });
}

describe("document kind (PDF vs Office)", () => {
  it("detects PDF from mime or extension", () => {
    assert.equal(isPdfDocument("application/pdf", "scan.png"), true);
    assert.equal(isPdfDocument("application/x-pdf", "x"), true);
    assert.equal(isPdfDocument("application/octet-stream", "facture.PDF"), true);
    assert.equal(isPdfDocument("image/png", "photo.png"), false);
  });

  it("detects Office OOXML from mime or extension", () => {
    assert.equal(
      isOfficeDocument(
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "x",
      ),
      true,
    );
    assert.equal(isOfficeDocument("application/octet-stream", "notes.docx"), true);
    assert.equal(documentFileEditTarget("application/pdf", "a.pdf"), "stirling");
    assert.equal(documentFileEditTarget("", "contrat.docx"), "m365");
    assert.equal(documentFileEditTarget("image/jpeg", "photo.jpg"), null);
  });
});

describe("frozen Stirling edit routes", () => {
  it("uses /edit/stirling — not the old /stirling-edit path", () => {
    assert.equal(stirlingEditApiPath(42), "/api/documents/42/edit/stirling");
    assert.equal(stirlingCommitApiPath(42), "/api/documents/42/edit/stirling/commit");
    assert.doesNotMatch(stirlingEditApiPath(1), /\/stirling-edit$/);
  });
});

describe("StirlingEditSession camelCase", () => {
  it("parses a live session and rejects invented aliases", () => {
    const session = parseStirlingEditSession(sampleSession());
    assert.ok(session);
    assert.equal(session?.documentId, 42);
    assert.equal(session?.stirlingUpload, "skipped");
    assert.equal(parseStirlingEditSession({ editorUrl: "https://stirling.example" }), null);
    assert.equal(parseStirlingEditSession(sampleSession({ editorUrl: "javascript:alert(1)" })), null);
    assert.equal(parseStirlingEditSession(sampleSession({ stirlingUpload: "maybe" })), null);
    assert.equal(isOpenableEditorUrl("https://stirling.example/x"), true);
    assert.equal(isOpenableEditorUrl("data:text/html,x"), false);
  });

  it("treats expiresAt in the past as expired", () => {
    const live = parseStirlingEditSession(sampleSession())!;
    assert.equal(isStirlingSessionExpired(live, Date.parse("2026-01-01T00:00:00Z")), false);
    const dead = parseStirlingEditSession(sampleSession({ expiresAt: "2020-01-01T00:00:00.000Z" }))!;
    assert.equal(isStirlingSessionExpired(dead, Date.parse("2026-01-01T00:00:00Z")), true);
  });
});

describe("FR error mapping", () => {
  it("maps 404/501/503 to not ready", () => {
    assert.equal(isStirlingEditNotReadyStatus(404), true);
    assert.equal(isStirlingEditNotReadyStatus(501), true);
    assert.equal(reasonFromHttpStatus(404), "not_ready");
    assert.equal(reasonFromHttpStatus(500), "unavailable");
    assert.equal(toastForStirlingEditFailure("not_ready"), STIRLING_EDIT_NOT_READY);
    assert.equal(toastForStirlingEditFailure("unavailable"), STIRLING_EDIT_UNAVAILABLE);
    assert.equal(toastForStirlingEditFailure("auth"), STIRLING_AUTH_REQUIRED);
    const notReady = Object.assign(new Error("gone"), { status: 404 });
    assert.equal(toastForStirlingCommitFailure(notReady), STIRLING_EDIT_NOT_READY);
    const auth = Object.assign(new Error("auth_required"), { name: "AuthRequired" });
    assert.equal(toastForStirlingCommitFailure(auth), STIRLING_AUTH_REQUIRED);
    assert.equal(toastForStirlingCommitFailure(new Error("boom")), "Impossible d’appliquer le PDF.");
  });
});

describe("Éditer / apply chrome (FR only)", () => {
  it("uses French copy and no product-model names", () => {
    const chrome = [
      STIRLING_EDIT_LABEL,
      STIRLING_APPLY_BANNER_TITLE,
      STIRLING_APPLY_BANNER_HELP,
      STIRLING_APPLY_BANNER_HELP_UPLOADED,
      STIRLING_APPLY_OPEN_FILE,
      STIRLING_APPLY_OPEN_FILE_HELP,
      STIRLING_OPEN_FILE_FAILED,
      STIRLING_APPLY_PRIMARY,
      STIRLING_APPLY_PULL,
      STIRLING_APPLY_SUCCESS,
      STIRLING_EDIT_STARTED,
      STIRLING_EDIT_STARTED_LOADED,
      STIRLING_EDIT_STARTED_UPLOADED,
      STIRLING_EDIT_STARTED_NO_FILEURL,
      STIRLING_SESSION_EXPIRED,
      STIRLING_EDIT_HINT_NOT_PDF,
    ].join(" ");
    assert.equal(STIRLING_EDIT_LABEL, "Éditer");
    assert.equal(STIRLING_APPLY_PRIMARY, "Appliquer dans Paperrust");
    assert.equal(STIRLING_APPLY_OPEN_FILE, "Ouvrir le fichier");
    assert.equal(STIRLING_OPEN_FILE_FAILED, "Impossible d’ouvrir le fichier PDF.");
    assert.notEqual(STIRLING_OPEN_FILE_FAILED, "Impossible d’ouvrir Stirling PDF.");
    assert.equal(toastForStirlingEditStarted("skipped"), STIRLING_EDIT_STARTED);
    assert.equal(toastForStirlingEditStarted("failed"), STIRLING_EDIT_STARTED);
    assert.equal(toastForStirlingEditStarted("ok"), STIRLING_EDIT_STARTED_UPLOADED);
    assert.equal(bannerHelpForStirlingSession("skipped"), STIRLING_APPLY_BANNER_HELP);
    assert.equal(bannerHelpForStirlingSession("ok"), STIRLING_APPLY_BANNER_HELP_UPLOADED);
    assert.doesNotMatch(chrome, /Grok|ChatGPT|Claude|not ready|unavailable/i);
    assert.equal(
      stirlingEditDisabledReason({ target: "stirling", hasFile: false }),
      STIRLING_EDIT_HINT_NO_FILE,
    );
    assert.equal(
      stirlingEditDisabledReason({ target: "m365", hasFile: true }),
      M365_EDIT_HINT_DISCONNECTED,
    );
  });
});

describe("editorUrl fileUrl", () => {
  it("detects fileUrl= on editorUrl and rejects empty or missing query", () => {
    const source = "https://paperrust.example/api/edit/stirling/tok/file";
    const withFile = fileUrlEditor(source);
    assert.equal(editorUrlHasFileUrl(withFile), true);
    assert.equal(hasFileUrl(withFile), true);
    assert.equal(editorUrlHasFileUrl("https://stirling.example"), false);
    assert.equal(editorUrlHasFileUrl("https://stirling.example/?fileUrl="), false);
    assert.equal(editorUrlHasFileUrl("https://stirling.example/?fileurl=" + encodeURIComponent(source)), false);
    assert.equal(editorUrlHasFileUrl("javascript:alert(1)?fileUrl=" + encodeURIComponent(source)), false);
    assert.equal(editorUrlHasFileUrl(""), false);
  });

  it("happy-path toast when fileUrl is present; fallback toast when absent", () => {
    const withFile = fileUrlEditor();
    assert.equal(toastForStirlingEditStarted("skipped", withFile), STIRLING_EDIT_STARTED_LOADED);
    assert.equal(toastForStirlingEditStarted("failed", withFile), STIRLING_EDIT_STARTED_LOADED);
    assert.equal(toastForStirlingEditStarted("ok", withFile), STIRLING_EDIT_STARTED_UPLOADED);
    assert.equal(
      toastForStirlingEditStarted("skipped", "https://stirling.example"),
      STIRLING_EDIT_STARTED_NO_FILEURL,
    );
    assert.equal(
      toastForStirlingEditStarted("ok", "https://stirling.example"),
      STIRLING_EDIT_STARTED_NO_FILEURL,
    );
    assert.match(STIRLING_EDIT_STARTED_NO_FILEURL, /Ouvrir le fichier/);
    assert.match(STIRLING_EDIT_STARTED, /s’ouvre dans un onglet/);
    assert.match(STIRLING_EDIT_STARTED_LOADED, /chargé/);
  });
});

describe("resolveEditorNavigateUrl + paperrustEdit handoff", () => {
  const source = "https://paperrust.example/api/edit/stirling/tok/file";
  const beta = "https://archive.example/api/edit/stirling/tok/file";
  const commit = "https://paperrust.example/api/edit/stirling/tok/commit";

  it("navigate href is origin+pathname only — no fileUrl/commitUrl on query/hash", () => {
    const editor = `https://stirling.example/?fileUrl=${encodeURIComponent(beta)}`;
    const navigated = resolveEditorNavigateUrl(editor, "https://other.example/file");
    const parsed = new URL(navigated);
    assert.equal(parsed.origin, "https://stirling.example");
    assert.equal(parsed.pathname, "/");
    assert.equal(parsed.search, "");
    assert.equal(parsed.hash, "");
    assert.equal(parsed.searchParams.get("fileUrl"), null);
    assert.equal(navigated, expectedNavigateUrl(beta));
    assert.doesNotMatch(navigated, /[?#]/);
    assert.equal(hasFileUrl(navigated), false);
  });

  it("uses origin+pathname when search fileUrl missing but source is http(s)", () => {
    const expected = expectedNavigateUrl(source);
    assert.equal(resolveEditorNavigateUrl("https://stirling.example/?", source), expected);
    assert.equal(resolveEditorNavigateUrl("https://stirling.example", source), expected);
    assert.equal(resolveEditorNavigateUrl("https://stirling.example/", source), expected);
    assert.equal(hasFileUrl("https://stirling.example/?"), false);
    assert.equal(isBareStirlingLanding("https://stirling.example/?"), true);
    assert.equal(isBareStirlingLanding("https://stirling.example"), true);
    // origin+pathname alone is bare landing for hasFileUrl purposes
    assert.equal(isBareStirlingLanding(expected), true);
  });

  it("stamps window.name as paperrustEdit= + JSON { fileUrl, commitUrl }", () => {
    assert.equal(PAPERRUST_EDIT_WINDOW_NAME_PREFIX, "paperrustEdit=");
    const handoff = { fileUrl: beta, commitUrl: commit };
    assert.deepEqual(
      resolveStirlingEditHandoff({
        editorUrl: `https://stirling.example/?fileUrl=${encodeURIComponent(beta)}`,
        sourceFileUrl: source,
        commitUrl: commit,
      }),
      handoff,
    );
    assert.equal(
      resolveStirlingEditHandoff({
        editorUrl: "https://stirling.example/?",
        sourceFileUrl: source,
        commitUrl: "",
      }),
      null,
    );
    const name = stirlingWindowNameForEdit(handoff);
    assert.ok(name.startsWith("paperrustEdit="));
    const parsed = JSON.parse(decodeURIComponent(name.slice("paperrustEdit=".length)));
    assert.deepEqual(parsed, handoff);
    assert.equal(name.includes(encodeURIComponent(encodeURIComponent(beta))), false);
    const popup = fakePopup();
    assert.equal(stampPaperrustEditName(popup as unknown as Window, handoff), true);
    assert.equal(popup.name, name);
    assert.equal(
      stampPaperrustEditName(popup as unknown as Window, {
        fileUrl: beta,
        commitUrl: "javascript:alert(1)",
      }),
      false,
    );
  });

  it("falls back to editorUrl when source is missing or not http(s)", () => {
    assert.equal(resolveEditorNavigateUrl("https://stirling.example/?", ""), "https://stirling.example/?");
    assert.equal(resolveEditorNavigateUrl("https://stirling.example/?", "   "), "https://stirling.example/?");
    assert.equal(resolveEditorNavigateUrl("https://stirling.example/?", "/api/file"), "https://stirling.example/?");
    assert.equal(
      resolveEditorNavigateUrl("https://stirling.example/?", "javascript:alert(1)"),
      "https://stirling.example/?",
    );
    assert.equal(hasFileUrl("javascript:alert(1)?fileUrl=" + encodeURIComponent(source)), false);
    assert.equal(isBareStirlingLanding("https://stirling.example/edit"), false);
  });

  it("gates chargé toast on handedOff or hash URL; origin+pathname alone is NO_FILEURL", () => {
    const bare = "https://stirling.example/?";
    const navigated = resolveEditorNavigateUrl(bare, source);
    assert.equal(toastForStirlingEditStarted("skipped", bare), STIRLING_EDIT_STARTED_NO_FILEURL);
    assert.equal(toastForStirlingEditStarted("ok", bare), STIRLING_EDIT_STARTED_NO_FILEURL);
    assert.equal(toastForStirlingEditStarted("skipped", navigated), STIRLING_EDIT_STARTED_NO_FILEURL);
    assert.equal(toastForStirlingEditStarted("skipped", undefined, true), STIRLING_EDIT_STARTED_LOADED);
    assert.equal(toastForStirlingEditStarted("failed", undefined, true), STIRLING_EDIT_STARTED_LOADED);
    assert.equal(toastForStirlingEditStarted("ok", undefined, true), STIRLING_EDIT_STARTED_UPLOADED);
    const hashHref = `https://stirling.example/#fileUrl=${encodeURIComponent(source)}`;
    assert.equal(toastForStirlingEditStarted("skipped", hashHref), STIRLING_EDIT_STARTED_LOADED);
  });

  it("sets paperrustEdit name then replace(origin+pathname); refuses incomplete session", () => {
    const source = "https://paperrust.example/api/edit/stirling/tok/file";
    const commit = "https://paperrust.example/api/edit/stirling/tok/commit";
    const session = parseStirlingEditSession(
      sampleSession({ editorUrl: fileUrlEditor(source), sourceFileUrl: source, commitUrl: commit }),
    )!;
    const expected = editorNavigateUrlForSession(session);
    const editor = fakePopup();
    assert.equal(navigateStirlingEditorSession(editor as unknown as Window, session), true);
    assert.equal(editor.replacedWith, expected);
    assert.equal(editor.location.href, expected);
    assert.equal(editor.name, expectedEditName(source, commit));
    assert.doesNotMatch(editor.location.href, /[?#]/);
    assert.notEqual(editor.location.href, session.sourceFileUrl);

    const noCommit = fakePopup();
    assert.equal(
      navigateStirlingEditorSession(noCommit as unknown as Window, {
        editorUrl: "https://stirling.example/?",
        sourceFileUrl: source,
        commitUrl: "",
      }),
      false,
    );
    assert.equal(noCommit.closed, true);
    assert.equal(noCommit.replacedWith, null);

    const noFile = fakePopup();
    assert.equal(
      navigateStirlingEditorSession(noFile as unknown as Window, {
        editorUrl: "https://stirling.example/?",
        sourceFileUrl: "",
        commitUrl: commit,
      }),
      false,
    );
    assert.equal(noFile.closed, true);
  });
});

type FakeLocation = {
  href: string;
  replace: (next: string) => void;
};

type FakePopup = {
  opener: unknown;
  name: string;
  location: FakeLocation;
  closed: boolean;
  close: () => void;
  replacedWith: string | null;
};

function fakePopup(href = "about:blank"): FakePopup {
  const popup: FakePopup = {
    opener: "parent",
    name: "_blank",
    replacedWith: null,
    location: {
      href,
      replace(next: string) {
        popup.replacedWith = next;
        popup.location.href = next;
      },
    },
    closed: false,
    close() {
      popup.closed = true;
    },
  };
  return popup;
}

type FakeAnchor = {
  target: string;
  rel: string;
  href: string;
  click: () => void;
  remove: () => void;
};

type OpenFn = (url: string, target?: string, features?: string) => FakePopup | null;

function withWindow(
  open: OpenFn,
  fn: () => void,
  extras?: { origin?: string; document?: { createElement: (tag: string) => FakeAnchor } },
) {
  const prevWindow = (globalThis as { window?: unknown }).window;
  const prevDocument = (globalThis as { document?: unknown }).document;
  (globalThis as {
    window: {
      open: OpenFn;
      location: { origin: string };
    };
  }).window = {
    open,
    location: { origin: extras?.origin ?? "https://paperrust.example" },
  };
  if (extras?.document) {
    (globalThis as { document: unknown }).document = extras.document;
  }
  try {
    fn();
  } finally {
    if (prevWindow === undefined) {
      delete (globalThis as { window?: unknown }).window;
    } else {
      (globalThis as { window: unknown }).window = prevWindow;
    }
    if (prevDocument === undefined) {
      delete (globalThis as { document?: unknown }).document;
    } else {
      (globalThis as { document: unknown }).document = prevDocument;
    }
  }
}

describe("Stirling gesture popups", () => {
  it("opens editor about:blank with opener=null; file blank keeps opener", () => {
    const opened: FakePopup[] = [];
    withWindow((url) => {
      assert.equal(url, "about:blank");
      const popup = fakePopup(url);
      opened.push(popup);
      return popup;
    }, () => {
      const editor = openStirlingGesturePopup("editor") as unknown as FakePopup;
      const file = openStirlingGesturePopup("file") as unknown as FakePopup;
      assert.equal(opened.length, 2);
      assert.equal(editor.opener, null);
      assert.equal(file.opener, "parent");
      assert.equal(editor.location.href, "about:blank");
      assert.equal(file.location.href, "about:blank");
    });
  });

  it("Éditer gesture opens a single editor blank, then fullUrl via location.replace", () => {
    const opened: FakePopup[] = [];
    const source = "https://paperrust.example/api/edit/stirling/tok/file";
    const commit = "https://paperrust.example/api/edit/stirling/tok/commit";
    const session = parseStirlingEditSession(
      sampleSession({ editorUrl: fileUrlEditor(source), sourceFileUrl: source, commitUrl: commit }),
    )!;
    const expected = editorNavigateUrlForSession(session);
    withWindow((url) => {
      const popup = fakePopup(url);
      opened.push(popup);
      return popup;
    }, () => {
      const editor = openStirlingGesturePopup("editor") as unknown as FakePopup;
      assert.equal(opened.length, 1);
      assert.equal(editor.opener, null);
      assert.equal(navigateStirlingEditorSession(editor as unknown as Window, session), true);
      assert.equal(editor.replacedWith, expected);
      assert.equal(editor.location.href, expected);
      assert.equal(editor.name, expectedEditName(source, commit));
      assert.notEqual(editor.location.href, session.sourceFileUrl);
    });
  });

  it("navigates a gesture blank after await via location.replace(fullUrl)", () => {
    const editor = fakePopup();
    const file = fakePopup();
    assert.equal(navigateStirlingPopup(editor as unknown as Window, "https://stirling.example/edit"), true);
    assert.equal(editor.replacedWith, "https://stirling.example/edit");
    assert.equal(editor.location.href, "https://stirling.example/edit");
    assert.equal(openStirlingSourceFile("https://paperrust.example/file.pdf", file as unknown as Window), true);
    assert.equal(file.replacedWith, "https://paperrust.example/file.pdf");
    assert.equal(file.location.href, "https://paperrust.example/file.pdf");
    const bad = fakePopup();
    assert.equal(navigateStirlingPopup(bad as unknown as Window, "javascript:alert(1)"), false);
    assert.equal(bad.closed, true);
  });

  it("does not use post-await window.open when a file blank already exists", () => {
    let openCalls = 0;
    const file = fakePopup();
    withWindow((url) => {
      openCalls += 1;
      return fakePopup(url);
    }, () => {
      assert.equal(openStirlingSourceFile("https://paperrust.example/file.pdf", file as unknown as Window), true);
      assert.equal(openCalls, 0);
      assert.equal(file.location.href, "https://paperrust.example/file.pdf");
    });
  });

  it("click path opens about:blank, nulls opener, then navigates", () => {
    let openedUrl = "";
    let openedFeatures: string | undefined;
    withWindow((url, _target, features) => {
      openedUrl = url;
      openedFeatures = features;
      return fakePopup(url);
    }, () => {
      const ok = openStirlingSourceFile("https://paperrust.example/file.pdf");
      assert.equal(ok, true);
      assert.equal(openedUrl, "about:blank");
      assert.equal(openedFeatures, undefined);
      const blank = openBlankEditorWindow() as unknown as FakePopup;
      assert.equal(blank.opener, null);
      assert.equal(navigateStirlingPopup(blank as unknown as Window, "https://paperrust.example/file.pdf"), true);
      assert.equal(blank.replacedWith, "https://paperrust.example/file.pdf");
      assert.equal(blank.location.href, "https://paperrust.example/file.pdf");
    });
  });

  it("does not treat noopener window.open null as failure", () => {
    const clicks: FakeAnchor[] = [];
    withWindow((url, _target, features) => {
      if (features?.includes("noopener")) return null;
      return fakePopup(url);
    }, () => {
      assert.equal(openStirlingSourceFile("https://paperrust.example/file.pdf"), true);
    });
    withWindow(
      (_url, _target, features) => (features?.includes("noopener") ? null : null),
      () => {
        assert.equal(openStirlingSourceFile("https://paperrust.example/file.pdf"), true);
        assert.equal(clicks.length, 1);
        assert.equal(clicks[0]?.target, "_blank");
        assert.equal(clicks[0]?.rel, "noopener noreferrer");
        assert.equal(clicks[0]?.href, "https://paperrust.example/file.pdf");
      },
      {
        document: {
          createElement(tag: string) {
            assert.equal(tag, "a");
            const el: FakeAnchor = {
              target: "",
              rel: "",
              href: "",
              click() {
                clicks.push(el);
              },
              remove() {},
            };
            return el;
          },
        },
      },
    );
  });

  it("falls back to a target=_blank anchor and treats the click as success", () => {
    const clicks: FakeAnchor[] = [];
    withWindow(
      () => null,
      () => {
        assert.equal(openStirlingSourceFile("https://paperrust.example/file.pdf"), true);
        assert.equal(clicks.length, 1);
        assert.equal(clicks[0]?.target, "_blank");
        assert.equal(clicks[0]?.rel, "noopener noreferrer");
        assert.equal(openUrlViaAnchor("javascript:alert(1)"), false);
      },
      {
        document: {
          createElement(tag: string) {
            assert.equal(tag, "a");
            const el: FakeAnchor = {
              target: "",
              rel: "",
              href: "",
              click() {
                clicks.push(el);
              },
              remove() {},
            };
            return el;
          },
        },
      },
    );
  });

  it("resolves a relative sourceFileUrl against the page origin", () => {
    let navigated = "";
    withWindow((url) => {
      const popup = fakePopup(url);
      const originalReplace = popup.location.replace.bind(popup.location);
      popup.location.replace = (next: string) => {
        navigated = next;
        originalReplace(next);
      };
      return popup;
    }, () => {
      assert.equal(resolveSourceFileUrl("/api/edit/stirling/tok/file"), "https://paperrust.example/api/edit/stirling/tok/file");
      assert.equal(isOpenableEditorUrl("/api/edit/stirling/tok/file"), false);
      assert.equal(isOpenableEditorUrl("https://paperrust.example/api/edit/stirling/tok/file"), true);
      assert.equal(openStirlingSourceFile("/api/edit/stirling/tok/file"), true);
      assert.equal(navigated, "https://paperrust.example/api/edit/stirling/tok/file");
    }, { origin: "https://paperrust.example" });
    assert.equal(resolveSourceFileUrl("/api/file"), null);
    assert.equal(openStirlingSourceFile("/api/file"), false);
    assert.equal(openStirlingSourceFile("javascript:alert(1)"), false);
    assert.equal(openStirlingSourceFile("data:text/html,x"), false);
  });

  it("openStirlingEditor blank → replace(origin+pathname); refuses incomplete session", () => {
    let openedUrl = "";
    withWindow((url) => {
      openedUrl = url;
      return fakePopup(url);
    }, () => {
      const commit = "https://paperrust.example/api/edit/stirling/tok/commit";
      const source = "https://paperrust.example/api/edit/stirling/tok/file";
      assert.equal(
        openStirlingEditor({
          editorUrl: "https://stirling.example",
          sourceFileUrl: "",
          commitUrl: commit,
        }),
        null,
      );
      assert.equal(
        openStirlingEditor({
          editorUrl: "https://stirling.example/?",
          sourceFileUrl: source,
          commitUrl: "",
        }),
        null,
      );
      const popup = openStirlingEditor({
        editorUrl: "https://stirling.example/?",
        sourceFileUrl: source,
        commitUrl: commit,
      }) as unknown as FakePopup;
      assert.equal(openedUrl, "about:blank");
      assert.equal(popup.opener, null);
      assert.equal(popup.replacedWith, expectedNavigateUrl(source));
      assert.equal(popup.location.href, expectedNavigateUrl(source));
      assert.equal(popup.name, expectedEditName(source, commit));
      assert.doesNotMatch(popup.replacedWith ?? "", /commitUrl|[?#]/);
    });
  });
});

describe("popup.name paperrustEdit handoff", () => {
  it("fileUrlFromNavigateUrl still reads hash-only href (legacy)", () => {
    const source = "https://paperrust.example/api/edit/stirling/tok/file";
    const href = `https://stirling.example/#fileUrl=${encodeURIComponent(source)}`;
    assert.equal(fileUrlFromNavigateUrl(href), source);
  });

  it("stampPaperrustEditName sets paperrustEdit= JSON prefix", () => {
    const popup = fakePopup();
    const source = "https://paperrust.example/api/edit/stirling/tok/file";
    const commit = "https://paperrust.example/api/edit/stirling/tok/commit";
    assert.equal(
      stampPaperrustEditName(popup as unknown as Window, { fileUrl: source, commitUrl: commit }),
      true,
    );
    assert.equal(popup.name, expectedEditName(source, commit));
    assert.ok(popup.name.startsWith(PAPERRUST_EDIT_WINDOW_NAME_PREFIX));
    assert.equal(PAPERRUST_FILEURL_WINDOW_NAME_PREFIX, "paperrustFileUrl=");
  });

  it("navigateStirlingPopup does not stamp name (editor stamp is session path)", () => {
    const popup = fakePopup();
    popup.name = "_blank";
    const source = "https://paperrust.example/api/edit/stirling/tok/file";
    const href = `https://stirling.example/#fileUrl=${encodeURIComponent(source)}`;
    assert.equal(navigateStirlingPopup(popup as unknown as Window, href), true);
    assert.equal(popup.replacedWith, href);
    assert.equal(popup.name, "_blank");
  });

  it("navigateStirlingPopup leaves name alone when no fileUrl in href", () => {
    const popup = fakePopup();
    popup.name = "_blank";
    assert.equal(
      navigateStirlingPopup(popup as unknown as Window, "https://stirling.example/edit"),
      true,
    );
    assert.equal(popup.name, "_blank");
  });
});
