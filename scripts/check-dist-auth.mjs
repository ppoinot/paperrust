#!/usr/bin/env node
/**
 * Fail a production build that would ship auth turned off, and fail if the
 * removed `VITE_AUTH_ENABLED` name is still sitting in `dist/`.
 *
 * (a) Resolve the client env the same way `vite build` does in production
 *     mode — `.env*` files, then `.grok/app-env.json`, then the real process
 *     environment — and reject any `VITE_*AUTH*` value that is `"false"`,
 *     `"0"`, or `""`.
 * (b) Reject any file under `dist/` whose bytes contain `VITE_AUTH_ENABLED`.
 *
 * `npm run build` runs this after `scripts/copy-ui-dist.mjs`. CI runs `--dist-only`
 * against the committed `dist/` and skips (a).
 *
 * Do not import Vite here. `loadEnv` pulls in rolldown native bindings that
 * SIGSEGV the test worker under qemu-user.
 */
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative, sep } from "node:path";
import { isMainModule, mergeAppEnv, projectRoot, readAppEnv } from "./with-app-env.mjs";

export const AUTH_ENV_KEY = /^VITE_.*AUTH/;
export const DISABLED_AUTH_VALUES = new Set(["false", "0", ""]);
export const AUTH_FLAG_MARK = "VITE_AUTH_ENABLED";

const VITE_PREFIX = "VITE_";

/** `.env` files Vite loads for `mode`, later entries overriding earlier ones. */
export function envFilesForMode(mode) {
  return [".env", ".env.local", `.env.${mode}`, `.env.${mode}.local`];
}

/**
 * A small dotenv subset: `export` prefix, quotes, and unquoted inline comments.
 * Enough to see the literal values a production build would inline.
 */
export function parseDotEnv(text) {
  const env = {};
  const lines = String(text).replace(/^\uFEFF/, "").split(/\r?\n/);
  for (const line of lines) {
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith("#")) continue;
    const match = /^(?:export\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*)$/.exec(trimmed);
    if (!match) continue;
    env[match[1]] = unquoteEnvValue(match[2]);
  }
  return env;
}

function unquoteEnvValue(raw) {
  const value = raw.trim();
  if (value.startsWith('"') || value.startsWith("'")) {
    const quote = value[0];
    let inner = "";
    for (let i = 1; i < value.length; i += 1) {
      const ch = value[i];
      if (quote === '"' && ch === "\\" && i + 1 < value.length) {
        const next = value[i + 1];
        if (next === "n") inner += "\n";
        else if (next === "r") inner += "\r";
        else if (next === '"') inner += '"';
        else if (next === "\\") inner += "\\";
        else inner += next;
        i += 1;
        continue;
      }
      if (ch === quote) return inner;
      inner += ch;
    }
  }
  const comment = value.search(/\s#/);
  return (comment === -1 ? value : value.slice(0, comment)).trim();
}

function readDotEnvFile(root, name) {
  const path = join(root, name);
  if (!existsSync(path)) return {};
  return parseDotEnv(readFileSync(path, "utf8"));
}

/**
 * Client env a `vite build --mode <mode>` sees after `with-app-env`.
 * Precedence, highest last: mode env files, app-env.json, `processEnv`.
 */
export function resolveBuildEnv(root, processEnv = process.env, mode = "production") {
  const fromFiles = {};
  for (const name of envFilesForMode(mode)) {
    Object.assign(fromFiles, readDotEnvFile(root, name));
  }
  const resolved = {};
  for (const [key, value] of Object.entries(fromFiles)) {
    if (key.startsWith(VITE_PREFIX) && typeof value === "string") resolved[key] = value;
  }
  const injected = mergeAppEnv(readAppEnv(root), processEnv);
  for (const [key, value] of Object.entries(injected)) {
    if (key.startsWith(VITE_PREFIX) && typeof value === "string") resolved[key] = value;
  }
  return resolved;
}

/** `VITE_*AUTH*` keys whose production value turns auth off. */
export function disabledAuthEnv(env) {
  const hits = [];
  for (const [key, value] of Object.entries(env)) {
    if (!AUTH_ENV_KEY.test(key)) continue;
    if (DISABLED_AUTH_VALUES.has(value)) hits.push({ key, value });
  }
  return hits;
}

/** Relative paths under `distDir` that contain `VITE_AUTH_ENABLED` in any form. */
export function filesContainingAuthFlag(distDir) {
  let entries;
  try {
    entries = readdirSync(distDir, { withFileTypes: true });
  } catch (err) {
    if (err && err.code === "ENOENT") {
      const missing = new Error(`dist directory not found: ${distDir}`);
      missing.code = "ENOENT";
      throw missing;
    }
    throw err;
  }
  const needle = Buffer.from(AUTH_FLAG_MARK);
  const hits = [];
  const visit = (dir, dirEntries) => {
    for (const entry of dirEntries) {
      const full = join(dir, entry.name);
      if (entry.isDirectory()) {
        visit(full, readdirSync(full, { withFileTypes: true }));
        continue;
      }
      if (entry.isSymbolicLink()) {
        try {
          if (!statSync(full).isFile()) continue;
        } catch {
          continue;
        }
      } else if (!entry.isFile()) {
        continue;
      }
      if (readFileSync(full).includes(needle)) {
        hits.push(relative(distDir, full).split(sep).join("/"));
      }
    }
  };
  visit(distDir, entries);
  return hits;
}

/**
 * @param {{
 *   root: string,
 *   distDir: string,
 *   processEnv?: NodeJS.ProcessEnv,
 *   distOnly?: boolean,
 *   mode?: string,
 * }} options
 */
export function checkDistAuth({
  root,
  distDir,
  processEnv = process.env,
  distOnly = false,
  mode = "production",
}) {
  const problems = [];
  if (!distOnly && mode === "production") {
    const env = resolveBuildEnv(root, processEnv, mode);
    for (const { key, value } of disabledAuthEnv(env)) {
      problems.push(
        `[dist-auth] production ${key} resolves to ${JSON.stringify(value)}; ` +
          'a VITE_*AUTH* value must not be "false", "0", or empty',
      );
    }
  }
  let hits;
  try {
    hits = filesContainingAuthFlag(distDir);
  } catch (err) {
    if (err && err.code === "ENOENT") {
      problems.push(`[dist-auth] ${err.message}`);
      return { ok: false, problems };
    }
    throw err;
  }
  for (const file of hits) {
    problems.push(`[dist-auth] ${file} contains ${AUTH_FLAG_MARK}`);
  }
  return { ok: problems.length === 0, problems };
}

export function parseDistAuthArgs(argv) {
  const usage = "usage: node scripts/check-dist-auth.mjs [--dist-only] [--root <dir>] [--dist <dir>]";
  let distOnly = false;
  let root = null;
  let dist = null;
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === "--dist-only") {
      distOnly = true;
    } else if (arg === "--root") {
      root = argv[(i += 1)];
      if (!root) return { error: `--root needs a directory — ${usage}` };
    } else if (arg === "--dist") {
      dist = argv[(i += 1)];
      if (!dist) return { error: `--dist needs a directory — ${usage}` };
    } else {
      return { error: `unexpected argument: ${arg} — ${usage}` };
    }
  }
  return { distOnly, root, dist };
}

function main(argv) {
  const args = parseDistAuthArgs(argv);
  if (args.error) {
    console.error(args.error);
    process.exit(2);
  }
  const root = args.root ?? projectRoot();
  const distDir = args.dist ?? join(root, "dist");
  const result = checkDistAuth({
    root,
    distDir,
    processEnv: process.env,
    distOnly: args.distOnly,
  });
  if (!result.ok) {
    for (const problem of result.problems) console.error(problem);
    process.exit(1);
  }
  console.log("[dist-auth] ok");
}

if (isMainModule(import.meta.url)) {
  main(process.argv.slice(2));
}
