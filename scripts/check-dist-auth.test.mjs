import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { promisify } from "node:util";
import {
  checkDistAuth,
  disabledAuthEnv,
  filesContainingAuthFlag,
  parseDotEnv,
  resolveBuildEnv,
} from "./check-dist-auth.mjs";
import { projectRoot } from "./with-app-env.mjs";

const execFileAsync = promisify(execFile);
const SCRIPT = join(projectRoot(), "scripts/check-dist-auth.mjs");

function fixture({ appEnv, files = {}, distFiles = null } = {}) {
  const root = mkdtempSync(join(tmpdir(), "dist-auth-"));
  if (appEnv !== undefined) {
    mkdirSync(join(root, ".grok"), { recursive: true });
    writeFileSync(join(root, ".grok/app-env.json"), appEnv);
  }
  for (const [name, body] of Object.entries(files)) {
    const path = join(root, name);
    mkdirSync(dirname(path), { recursive: true });
    writeFileSync(path, body);
  }
  const dist = join(root, "dist");
  if (distFiles !== null) {
    mkdirSync(dist, { recursive: true });
    for (const [name, body] of Object.entries(distFiles)) {
      const path = join(dist, name);
      mkdirSync(dirname(path), { recursive: true });
      writeFileSync(path, body);
    }
  }
  return { root, dist };
}

function problems(options) {
  return checkDistAuth(options).problems;
}

test("dotenv keeps quoted, exported, and empty auth values", () => {
  assert.deepEqual(
    parseDotEnv(`
# comment
export VITE_AUTH_ENABLED="false"
VITE_FOO_AUTH=0
VITE_AUTH_MODE=
VITE_AUTH_NOTE='ok' # not a comment
VITE_OTHER=false # comment
`),
    {
      VITE_AUTH_ENABLED: "false",
      VITE_FOO_AUTH: "0",
      VITE_AUTH_MODE: "",
      VITE_AUTH_NOTE: "ok",
      VITE_OTHER: "false",
    },
  );
});

test("production VITE_*AUTH* values of false, 0, or empty fail", () => {
  for (const [key, value] of [
    ["VITE_AUTH_ENABLED", "false"],
    ["VITE_FOO_AUTH_BAR", "0"],
    ["VITE_AUTH_MODE", ""],
  ]) {
    const { root, dist } = fixture({
      appEnv: JSON.stringify({ [key]: value }),
      distFiles: { "app.js": "clean" },
    });
    const found = problems({ root, distDir: dist, processEnv: {} });
    assert.equal(found.length, 1, key);
    assert.match(found[0], new RegExp(key));
  }
});

test("other values and non-auth keys do not fail", () => {
  const { root, dist } = fixture({
    appEnv: JSON.stringify({
      VITE_AUTH_ENABLED: "true",
      VITE_OIDC_CLIENT_ID: "false",
      VITE_OTHER: "0",
    }),
    distFiles: { "app.js": "clean" },
  });
  assert.deepEqual(disabledAuthEnv(resolveBuildEnv(root, {}, "production")), []);
  assert.deepEqual(problems({ root, distDir: dist, processEnv: {} }), []);
});

test("process env wins over app-env, which wins over dotenv files", () => {
  const { root } = fixture({
    appEnv: '{"VITE_AUTH_ENABLED":"false","VITE_SESSION_AUTH":"true"}',
    files: {
      ".env": "VITE_AUTH_ENABLED=true\nVITE_SESSION_AUTH=false\n",
      ".env.development": "VITE_DEV_AUTH=false\n",
      ".env.production": "VITE_BUILD_AUTH=0\nVITE_SESSION_AUTH=0\n",
    },
  });
  const env = resolveBuildEnv(root, { VITE_AUTH_ENABLED: "true" }, "production");
  assert.equal(env.VITE_AUTH_ENABLED, "true");
  assert.equal(env.VITE_SESSION_AUTH, "true");
  assert.equal(env.VITE_BUILD_AUTH, "0");
  assert.equal(env.VITE_DEV_AUTH, undefined);
  assert.deepEqual(
    disabledAuthEnv(env).map((hit) => hit.key),
    ["VITE_BUILD_AUTH"],
  );
});

test("a later production env file overrides an earlier one", () => {
  const { root, dist } = fixture({
    files: {
      ".env.production": "VITE_AUTH_ENABLED=false\n",
      ".env.production.local": "VITE_AUTH_ENABLED=true\n",
    },
    distFiles: {},
  });
  assert.deepEqual(problems({ root, distDir: dist, processEnv: {} }), []);
});

test("development mode does not apply the production disable rule", () => {
  const { root, dist } = fixture({
    appEnv: '{"VITE_AUTH_ENABLED":"false"}',
    distFiles: { "app.js": "clean" },
  });
  assert.deepEqual(
    problems({ root, distDir: dist, processEnv: {}, mode: "development" }),
    [],
  );
});

test("dist files containing the flag name fail, in any form", () => {
  const { root, dist } = fixture({
    distFiles: {
      "assets/app.js": 'const flag = "preVITE_AUTH_ENABLEDpost";',
      "ok.js": "no flag here",
    },
  });
  const hits = filesContainingAuthFlag(dist);
  assert.deepEqual(hits, ["assets/app.js"]);
  const found = problems({ root, distDir: dist, processEnv: {} });
  assert.equal(found.length, 1);
  assert.match(found[0], /assets\/app\.js contains VITE_AUTH_ENABLED/);
});

test("a missing dist directory fails", () => {
  const { root, dist } = fixture();
  const result = checkDistAuth({ root, distDir: dist, processEnv: {} });
  assert.equal(result.ok, false);
  assert.match(result.problems[0], /dist directory not found/);
});

test("--dist-only ignores a disabling env and still scans dist", () => {
  const badEnv = fixture({
    appEnv: '{"VITE_AUTH_ENABLED":"false"}',
    distFiles: { "app.js": "clean" },
  });
  assert.deepEqual(
    problems({ root: badEnv.root, distDir: badEnv.dist, processEnv: {}, distOnly: true }),
    [],
  );
  const badDist = fixture({
    appEnv: '{"VITE_AUTH_ENABLED":"true"}',
    distFiles: { "nested/chunk.js": "VITE_AUTH_ENABLED" },
  });
  const found = problems({
    root: badDist.root,
    distDir: badDist.dist,
    processEnv: {},
    distOnly: true,
  });
  assert.equal(found.length, 1);
  assert.match(found[0], /nested\/chunk\.js/);
});

test("the CLI exits non-zero for a disabling production env or a marked dist", async () => {
  const disabled = fixture({
    appEnv: '{"VITE_AUTH_ENABLED":""}',
    distFiles: { "app.js": "clean" },
  });
  const disabledError = await execFileAsync(process.execPath, [
    SCRIPT,
    "--root",
    disabled.root,
    "--dist",
    disabled.dist,
  ], { env: { PATH: process.env.PATH } }).catch((err) => err);
  assert.equal(disabledError.code, 1);
  assert.match(disabledError.stderr, /VITE_AUTH_ENABLED/);

  const marked = fixture({ distFiles: { "app.js": "see VITE_AUTH_ENABLED" } });
  const markedError = await execFileAsync(
    process.execPath,
    [SCRIPT, "--dist-only", "--root", marked.root, "--dist", marked.dist],
    { env: { PATH: process.env.PATH, VITE_AUTH_ENABLED: "false" } },
  ).catch((err) => err);
  assert.equal(markedError.code, 1);
  assert.match(markedError.stderr, /app\.js contains VITE_AUTH_ENABLED/);
  assert.doesNotMatch(markedError.stderr, /resolves to/);
});

test("the CLI accepts a clean dist and rejects unknown arguments", async () => {
  const clean = fixture({
    appEnv: '{"VITE_OIDC_CLIENT_ID":"paperrust"}',
    distFiles: { "app.js": "ok" },
  });
  const { stdout } = await execFileAsync(
    process.execPath,
    [SCRIPT, "--root", clean.root, "--dist", clean.dist],
    { env: { PATH: process.env.PATH } },
  );
  assert.match(stdout, /\[dist-auth\] ok/);

  const usage = await execFileAsync(process.execPath, [SCRIPT, "--nope"]).catch((err) => err);
  assert.equal(usage.code, 2);
});
