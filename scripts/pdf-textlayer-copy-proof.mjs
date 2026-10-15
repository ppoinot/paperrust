#!/usr/bin/env node
/**
 * Proof: absolute TextLayer spans + endOfContent lifecycle.
 * Sequence: drag FOUCAULT→KEVIN → pointerup → assert .selecting kept → Ctrl+C
 * → clipboard === "FOUCAULT KEVIN".
 *
 * Also runs a "broken" control that tears down on pointerup (PR#7 behaviour)
 * to show native selection collapse.
 *
 * podman run --rm -v "$PWD:/work:z" -w /work \
 *   -e PLAYWRIGHT_BROWSERS_PATH=/ms-playwright \
 *   mcr.microsoft.com/playwright:v1.62.1-jammy \
 *   node scripts/pdf-textlayer-copy-proof.mjs
 */
import { mkdirSync, writeFileSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright";

const __dirname = dirname(fileURLToPath(import.meta.url));
const root = join(__dirname, "..");
const outDir = join(root, "scripts/.tmp-textlayer-proof");
mkdirSync(outDir, { recursive: true });

const css = readFileSync(join(root, "src/components/documents/pdf-text-layer.css"), "utf8");

/** Fixed implementation (keep .selecting after pointerup + capture copy). */
const fixedJs = `
(() => {
  const textLayers = new Map();
  const dragSnapshots = new WeakMap();
  let selectionAbort = null;
  let prevRange = null;
  let pointerDown = false;
  let dragOrigin = null;

  function resetLayer(end, layer) {
    layer.append(end);
    end.style.width = "";
    end.style.height = "";
    layer.classList.remove("selecting");
  }
  function layerSpans(layer) {
    return [...layer.querySelectorAll(":scope > span")].filter(
      (el) => el instanceof HTMLElement && !el.classList.contains("endOfContent"),
    );
  }
  function rectsOverlap(a, b, pad = 1) {
    return !(a.right < b.left - pad || a.left > b.right + pad || a.bottom < b.top - pad || a.top > b.bottom + pad);
  }
  function joinSpanTexts(pieces) {
    if (!pieces.length) return "";
    const sorted = [...pieces].sort((a, b) => Math.abs(a.top - b.top) > 4 ? a.top - b.top : a.left - b.left);
    let out = sorted[0].text;
    for (let i = 1; i < sorted.length; i++) {
      const prev = sorted[i - 1], cur = sorted[i];
      const sameLine = Math.abs(cur.top - prev.top) <= 4;
      const needsSpace = sameLine && !/\\s$/.test(out) && !/^\\s/.test(cur.text);
      const needsNewline = !sameLine && !/\\n$/.test(out);
      out += needsNewline ? "\\n" : needsSpace ? " " : "";
      out += cur.text;
    }
    return out;
  }
  function spansInDragRect(layer, x0, y0, x1, y1) {
    const left = Math.min(x0, x1), right = Math.max(x0, x1), top = Math.min(y0, y1), bottom = Math.max(y0, y1);
    const pad = right - left < 2 && bottom - top < 2 ? 4 : 0;
    const drag = new DOMRect(left - pad, top - pad, right - left + pad * 2, bottom - top + pad * 2);
    return layerSpans(layer).filter((span) => {
      const r = span.getBoundingClientRect();
      return r.width > 0 && r.height > 0 && rectsOverlap(drag, r);
    });
  }
  function textFromSpans(spans) {
    return joinSpanTexts(spans.map((span) => {
      const rect = span.getBoundingClientRect();
      return { text: span.textContent || "", top: rect.top, left: rect.left };
    }).filter((p) => p.text));
  }
  function expandSelectionToDrag(layer, origin, endX, endY) {
    const spans = spansInDragRect(layer, origin.x, origin.y, endX, endY);
    const snapshot = textFromSpans(spans);
    if (snapshot) dragSnapshots.set(layer, snapshot);
    if (!spans.length) return;
    const ordered = [...spans].sort((a, b) => {
      const ar = a.getBoundingClientRect(), br = b.getBoundingClientRect();
      return Math.abs(ar.top - br.top) > 4 ? ar.top - br.top : ar.left - br.left;
    });
    const startNode = ordered[0].firstChild, endNode = ordered[ordered.length - 1].firstChild;
    if (!startNode || !endNode) return;
    try {
      const range = document.createRange();
      range.setStart(startNode, 0);
      range.setEnd(endNode, endNode.textContent?.length || 0);
      const sel = document.getSelection();
      if (!sel) return;
      sel.removeAllRanges();
      sel.addRange(range);
    } catch (_) {}
  }
  function selectionTextFromLayer(selection, layer) {
    if (!selection.rangeCount) return null;
    const range = selection.getRangeAt(0);
    if (!range.intersectsNode(layer)) return null;
    const pieces = [];
    for (const span of layerSpans(layer)) {
      if (!selection.containsNode(span, true)) continue;
      const rect = span.getBoundingClientRect();
      pieces.push({ text: span.textContent || "", top: rect.top, left: rect.left });
    }
    if (!pieces.length) return selection.toString() || dragSnapshots.get(layer) || null;
    return joinSpanTexts(pieces);
  }
  function enable() {
    if (selectionAbort) return;
    selectionAbort = new AbortController();
    const { signal } = selectionAbort;
    document.addEventListener("pointerdown", (e) => {
      pointerDown = true;
      const layer = e.target?.closest?.(".textLayer");
      if (layer && textLayers.has(layer)) {
        dragOrigin = { x: e.clientX, y: e.clientY, layer };
        layer.classList.add("selecting");
      } else dragOrigin = null;
    }, { signal });
    document.addEventListener("pointerup", (e) => {
      const origin = dragOrigin;
      dragOrigin = null;
      pointerDown = false;
      if (origin && textLayers.has(origin.layer)) {
        expandSelectionToDrag(origin.layer, origin, e.clientX, e.clientY);
        origin.layer.classList.add("selecting"); // keep bridge — do NOT resetLayer
        return;
      }
    }, { signal });
    document.addEventListener("copy", (e) => {
      const selection = document.getSelection();
      if (!selection || !selection.rangeCount) return;
      for (const layer of textLayers.keys()) {
        if (!layer.contains(selection.anchorNode) && !layer.contains(selection.focusNode)) continue;
        let text = selectionTextFromLayer(selection, layer);
        const snap = dragSnapshots.get(layer);
        if (snap && (!text || snap.length > text.length)) text = snap;
        if (!text) continue;
        e.clipboardData?.setData("text/plain", text);
        e.preventDefault();
        e.stopPropagation();
        return;
      }
    }, { capture: true, signal });
    document.addEventListener("selectionchange", () => {
      const selection = document.getSelection();
      if (!selection || !selection.rangeCount || selection.isCollapsed) {
        textLayers.forEach(resetLayer);
        prevRange = null;
        return;
      }
      for (const [layer, end] of textLayers) {
        let active = false;
        for (let i = 0; i < selection.rangeCount; i++) {
          if (selection.getRangeAt(i).intersectsNode(layer)) active = true;
        }
        if (active) {
          layer.classList.add("selecting");
          const range = selection.getRangeAt(0);
          let anchor = range.endContainer;
          if (anchor.nodeType === Node.TEXT_NODE) anchor = anchor.parentNode;
          const parent = anchor?.parentElement;
          if (parent && parent.closest(".textLayer") === layer) {
            end.style.width = layer.clientWidth + "px";
            end.style.height = layer.clientHeight + "px";
            parent.insertBefore(end, anchor.nextSibling);
          }
        } else resetLayer(end, layer);
      }
      prevRange = selection.getRangeAt(0).cloneRange();
    }, { signal });
  }
  function bindTextLayerSelection(layer) {
    layer.querySelector(".endOfContent")?.remove();
    const end = document.createElement("div");
    end.className = "endOfContent";
    layer.append(end);
    layer.addEventListener("mousedown", () => layer.classList.add("selecting"));
    textLayers.set(layer, end);
    enable();
    return () => { textLayers.delete(layer); end.remove(); };
  }
  window.PdfTextLayerSel = { bindTextLayerSelection, selectionTextFromLayer };
})();
`;

/** Control: PR#7-style — resetLayer on every pointerup (collapses Chromium). */
const brokenJs = `
(() => {
  const textLayers = new Map();
  let selectionAbort = null;
  function resetLayer(end, layer) {
    layer.append(end);
    end.style.width = "";
    end.style.height = "";
    layer.classList.remove("selecting");
  }
  function enable() {
    if (selectionAbort) return;
    selectionAbort = new AbortController();
    const { signal } = selectionAbort;
    document.addEventListener("pointerdown", (e) => {
      const layer = e.target?.closest?.(".textLayer");
      if (layer && textLayers.has(layer)) layer.classList.add("selecting");
    }, { signal });
    document.addEventListener("pointerup", () => {
      textLayers.forEach(resetLayer); // BUG: tear down before Ctrl+C
    }, { signal });
    document.addEventListener("selectionchange", () => {
      const selection = document.getSelection();
      if (!selection || !selection.rangeCount) { textLayers.forEach(resetLayer); return; }
      for (const [layer, end] of textLayers) {
        let active = false;
        for (let i = 0; i < selection.rangeCount; i++) {
          if (selection.getRangeAt(i).intersectsNode(layer)) active = true;
        }
        if (active) {
          layer.classList.add("selecting");
          const range = selection.getRangeAt(0);
          let anchor = range.endContainer;
          if (anchor.nodeType === Node.TEXT_NODE) anchor = anchor.parentNode;
          const parent = anchor?.parentElement;
          if (parent && parent.closest(".textLayer") === layer) {
            end.style.width = layer.clientWidth + "px";
            end.style.height = layer.clientHeight + "px";
            parent.insertBefore(end, anchor.nextSibling);
          }
        } else resetLayer(end, layer);
      }
    }, { signal });
  }
  function bindTextLayerSelection(layer) {
    const end = document.createElement("div");
    end.className = "endOfContent";
    layer.append(end);
    layer.addEventListener("mousedown", () => layer.classList.add("selecting"));
    textLayers.set(layer, end);
    enable();
  }
  window.BrokenTextLayerSel = { bindTextLayerSelection };
})();
`;

function fixtureHtml(modeJs, binder) {
  return `<!DOCTYPE html>
<html lang="fr"><head><meta charset="utf-8"><title>textlayer ${binder}</title>
<style>
${css}
body { margin: 40px; background: #ccc; }
.pdf-page { width: 420px; height: 90px; background: #fff; }
.pdf-page .textLayer :is(span, br) { color: #111 !important; }
</style></head><body>
<div class="pdf-page">
  <canvas class="pdf-page-canvas" width="420" height="90"></canvas>
  <div class="textLayer" id="layer" style="width:420px;height:90px">
    <span style="left:16px;top:28px;font-size:18px;font-family:monospace" id="s1">FOUCAULT</span>
    <span style="left:130px;top:28px;font-size:18px;font-family:monospace" id="s2">KEVIN</span>
    <span style="left:220px;top:28px;font-size:18px;font-family:monospace" id="s3">02.38.79.75.32</span>
  </div>
</div>
<script>${modeJs}</script>
<script>${binder}(document.getElementById("layer"));</script>
</body></html>`;
}

writeFileSync(join(outDir, "fixed.html"), fixtureHtml(fixedJs, "PdfTextLayerSel.bindTextLayerSelection"));
writeFileSync(join(outDir, "broken.html"), fixtureHtml(brokenJs, "BrokenTextLayerSel.bindTextLayerSelection"));

async function runMode(page, label) {
  const s1 = await page.locator("#s1").boundingBox();
  const s2 = await page.locator("#s2").boundingBox();
  if (!s1 || !s2) throw new Error("missing boxes");

  await page.mouse.move(s1.x + 2, s1.y + s1.height / 2);
  await page.mouse.down();
  await page.mouse.move(s2.x + s2.width - 2, s2.y + s2.height / 2, { steps: 16 });
  await page.mouse.up();
  // Critical: wait after pointerup before Ctrl+C.
  await page.waitForTimeout(150);

  const afterUp = await page.evaluate(() => {
    const sel = document.getSelection();
    const layer = document.getElementById("layer");
    const end = layer.querySelector(".endOfContent");
    return {
      toString: sel?.toString() ?? "",
      collapsed: sel?.isCollapsed ?? true,
      containsS1: sel?.containsNode(document.getElementById("s1"), true) ?? false,
      containsS2: sel?.containsNode(document.getElementById("s2"), true) ?? false,
      selecting: layer.classList.contains("selecting"),
      endTop: end ? getComputedStyle(end).top : null,
    };
  });
  console.log(`${label}_AFTER_POINTERUP`, JSON.stringify(afterUp, null, 2));

  await page.keyboard.press("Control+c");
  await page.waitForTimeout(80);
  let clipboard = "";
  try {
    clipboard = await page.evaluate(() => navigator.clipboard.readText());
  } catch (err) {
    clipboard = `ERR:${err.message}`;
  }
  console.log(`${label}_CLIPBOARD`, JSON.stringify(clipboard));
  return { afterUp, clipboard };
}

const browser = await chromium.launch({ headless: true });
const context = await browser.newContext();
await context.grantPermissions(["clipboard-read", "clipboard-write"]);

// --- Diagnosis control (broken teardown) ---
const brokenPage = await context.newPage();
await brokenPage.goto(`file://${join(outDir, "broken.html")}`);
const broken = await runMode(brokenPage, "BROKEN");
await brokenPage.close();

// --- Fixed path ---
const fixedPage = await context.newPage();
const copyLog = [];
await fixedPage.exposeFunction("reportCopy", (p) => copyLog.push(p));
await fixedPage.goto(`file://${join(outDir, "fixed.html")}`);
await fixedPage.evaluate(() => {
  document.addEventListener(
    "copy",
    (e) => {
      window.reportCopy({
        prevented: e.defaultPrevented,
        data: e.clipboardData?.getData("text/plain") ?? null,
        native: document.getSelection()?.toString() ?? null,
      });
    },
    true,
  );
});
const fixed = await runMode(fixedPage, "FIXED");
console.log("FIXED_COPY_LOG", JSON.stringify(copyLog, null, 2));
await browser.close();

const diagnosisOk =
  !broken.afterUp.containsS2 ||
  broken.clipboard === "FOUCAULT" ||
  broken.clipboard === "FOUCAULTKEVIN" ||
  !broken.afterUp.selecting ||
  broken.clipboard !== "FOUCAULT KEVIN";
const fixOk =
  fixed.clipboard === "FOUCAULT KEVIN" &&
  fixed.afterUp.containsS1 &&
  fixed.afterUp.containsS2 &&
  fixed.afterUp.selecting === true;

console.log("DIAGNOSIS_CONFIRMED", diagnosisOk ? "yes (broken path incomplete or bridge torn)" : "unexpected");
console.log("FIX_PASS", fixOk);

if (!fixOk) {
  console.error("FAIL expected FIXED clipboard FOUCAULT KEVIN with .selecting kept after pointerup");
  process.exit(2);
}
console.log("PASS clipboard === FOUCAULT KEVIN (after pointerup, bridge kept)");
