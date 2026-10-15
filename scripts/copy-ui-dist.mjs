#!/usr/bin/env node
/**
 * Copy the Nitro prerender (`.vercel/output/static`) into `dist/`
 * so the API can serve it (`PAPERRUST_UI_DIR`, Containerfile `COPY dist/`).
 */
import { cpSync, existsSync, mkdirSync, rmSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const src = path.join(root, ".vercel/output/static");
const dest = path.join(root, "dist");

if (!existsSync(src)) {
  console.error("[copy-ui-dist] missing .vercel/output/static — run vite build first");
  process.exit(1);
}

rmSync(dest, { recursive: true, force: true });
mkdirSync(dest, { recursive: true });
cpSync(src, dest, { recursive: true });

const index = path.join(dest, "index.html");
const shell = path.join(dest, "_shell.html");
if (!existsSync(index) && existsSync(shell)) {
  cpSync(shell, index);
}

console.log(`[copy-ui-dist] ${path.relative(root, src)} → dist/`);
