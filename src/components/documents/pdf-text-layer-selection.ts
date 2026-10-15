/**
 * PDF.js TextLayerBuilder-equivalent selection + reliable multi-span copy.
 *
 * Root cause: `pointerup` / `blur` used to call `resetLayer`
 * immediately — remove `.selecting` and park `.endOfContent` below the page
 * *before* Ctrl+C. Chromium then collapses the Selection to a single span
 * (FOUCAULT) while the highlight can still look like FOUCAULT KEVIN. A copy
 * rebuild from that collapsed range yields an incomplete clipboard.
 *
 * Fix (aligned with TextLayerBuilder):
 * 1) Keep `.selecting` + endOfContent bridging while a non-collapsed selection
 *    still intersects the textLayer (do not tear down on pointerup solely).
 * 2) On pointerup, expand the DOM range to every span under the drag rect and
 *    snapshot that text (insurance when Chromium still truncates).
 * 3) Document-level `copy` in *capture* that rebuilds from intersecting spans
 *    and inserts same-line spaces (prefer longer drag snapshot).
 */

const textLayers = new Map<HTMLElement, HTMLElement>();
/** Last multi-span text captured from a drag on a given layer. */
const dragSnapshots = new WeakMap<HTMLElement, string>();

let selectionAbort: AbortController | null = null;
let prevRange: Range | null = null;
let pointerDown = false;
let dragOrigin: { x: number; y: number; layer: HTMLElement } | null = null;

function resetLayer(end: HTMLElement, layer: HTMLElement) {
  layer.append(end);
  end.style.width = "";
  end.style.height = "";
  layer.classList.remove("selecting");
}

function layerSpans(layer: HTMLElement): HTMLElement[] {
  return [...layer.querySelectorAll(":scope > span, :scope .markedContent > span")].filter(
    (el) =>
      el instanceof HTMLElement &&
      !el.classList.contains("markedContent") &&
      !el.classList.contains("endOfContent") &&
      el.getAttribute("role") !== "img",
  ) as HTMLElement[];
}

function rectsOverlap(a: DOMRectReadOnly, b: DOMRectReadOnly, pad = 1): boolean {
  return !(
    a.right < b.left - pad ||
    a.left > b.right + pad ||
    a.bottom < b.top - pad ||
    a.top > b.bottom + pad
  );
}

function joinSpanTexts(pieces: { text: string; top: number; left: number }[]): string {
  if (pieces.length === 0) return "";
  const sorted = [...pieces].sort((a, b) =>
    Math.abs(a.top - b.top) > 4 ? a.top - b.top : a.left - b.left,
  );
  let out = sorted[0]!.text;
  for (let i = 1; i < sorted.length; i++) {
    const prev = sorted[i - 1]!;
    const cur = sorted[i]!;
    const sameLine = Math.abs(cur.top - prev.top) <= 4;
    const needsSpace =
      sameLine && !/\s$/.test(out) && !/^\s/.test(cur.text) && !/[\u00A0]$/.test(out);
    const needsNewline = !sameLine && !/\n$/.test(out);
    out += needsNewline ? "\n" : needsSpace ? " " : "";
    out += cur.text;
  }
  return out;
}

/** True when selection covers (partially) at least two text spans in the layer. */
function hasMultiSpanSelection(selection: Selection, layer: HTMLElement): boolean {
  if (selection.rangeCount === 0 || selection.isCollapsed) return false;
  let hit = 0;
  for (const span of layerSpans(layer)) {
    if (selection.containsNode(span, true)) {
      hit += 1;
      if (hit >= 2) return true;
    }
  }
  return false;
}

/** Spans whose boxes intersect the axis-aligned drag rectangle. */
export function spansInDragRect(
  layer: HTMLElement,
  x0: number,
  y0: number,
  x1: number,
  y1: number,
): HTMLElement[] {
  const left = Math.min(x0, x1);
  const right = Math.max(x0, x1);
  const top = Math.min(y0, y1);
  const bottom = Math.max(y0, y1);
  const pad = right - left < 2 && bottom - top < 2 ? 4 : 0;
  const drag = new DOMRect(left - pad, top - pad, right - left + pad * 2, bottom - top + pad * 2);
  return layerSpans(layer).filter((span) => {
    const r = span.getBoundingClientRect();
    return r.width > 0 && r.height > 0 && rectsOverlap(drag, r);
  });
}

function textFromSpans(spans: HTMLElement[]): string {
  const pieces = spans
    .map((span) => {
      const rect = span.getBoundingClientRect();
      return { text: span.textContent ?? "", top: rect.top, left: rect.left };
    })
    .filter((p) => p.text);
  return joinSpanTexts(pieces);
}

/**
 * Expand the live Selection so it covers every span under the drag rect.
 * Geometry-based — does not rely on endOfContent still bridging the range.
 */
function expandSelectionToDrag(
  layer: HTMLElement,
  origin: { x: number; y: number },
  endX: number,
  endY: number,
) {
  const spans = spansInDragRect(layer, origin.x, origin.y, endX, endY);
  const snapshot = textFromSpans(spans);
  if (snapshot) dragSnapshots.set(layer, snapshot);
  if (spans.length === 0) return;

  const ordered = [...spans].sort((a, b) => {
    const ar = a.getBoundingClientRect();
    const br = b.getBoundingClientRect();
    return Math.abs(ar.top - br.top) > 4 ? ar.top - br.top : ar.left - br.left;
  });
  const startNode = ordered[0]!.firstChild;
  const endNode = ordered[ordered.length - 1]!.firstChild;
  if (!startNode || !endNode) return;

  try {
    const range = document.createRange();
    range.setStart(startNode, 0);
    range.setEnd(endNode, endNode.textContent?.length ?? 0);
    const sel = document.getSelection();
    if (!sel) return;
    sel.removeAllRanges();
    sel.addRange(range);
  } catch {
    /* detached nodes */
  }
}

/** Reset layers that no longer hold a live (non-collapsed) selection. */
function resetInactiveLayers() {
  const selection = document.getSelection();
  if (!selection || selection.rangeCount === 0 || selection.isCollapsed) {
    textLayers.forEach(resetLayer);
    return;
  }
  for (const [layer, end] of textLayers) {
    let active = false;
    for (let i = 0; i < selection.rangeCount; i++) {
      if (selection.getRangeAt(i).intersectsNode(layer)) {
        active = true;
        break;
      }
    }
    if (!active) resetLayer(end, layer);
    else layer.classList.add("selecting");
  }
}

function enableGlobalSelectionListener() {
  if (selectionAbort) return;
  selectionAbort = new AbortController();
  const { signal } = selectionAbort;

  document.addEventListener(
    "pointerdown",
    (event) => {
      pointerDown = true;
      const layer = (event.target as Element | null)?.closest?.(".textLayer") as HTMLElement | null;
      if (layer && textLayers.has(layer)) {
        dragOrigin = { x: event.clientX, y: event.clientY, layer };
        layer.classList.add("selecting");
      } else {
        // Click outside text layers: allow teardown of stale bridges.
        dragOrigin = null;
        resetInactiveLayers();
      }
    },
    { signal },
  );

  document.addEventListener(
    "pointerup",
    (event) => {
      const origin = dragOrigin;
      dragOrigin = null;
      pointerDown = false;
      if (origin && textLayers.has(origin.layer)) {
        expandSelectionToDrag(origin.layer, origin, event.clientX, event.clientY);
        // Keep .selecting + endOfContent bridging while the multi-span range
        // remains — tearing down here collapses Chromium to FOUCAULT alone.
        origin.layer.classList.add("selecting");
        return;
      }
      resetInactiveLayers();
    },
    { signal },
  );

  window.addEventListener(
    "blur",
    () => {
      dragOrigin = null;
      pointerDown = false;
      // Window blur: selection may persist visually in some browsers; keep
      // bridge if multi-span still present so a later Ctrl+C still works.
      const selection = document.getSelection();
      let keep = false;
      if (selection && !selection.isCollapsed) {
        for (const layer of textLayers.keys()) {
          if (hasMultiSpanSelection(selection, layer)) {
            keep = true;
            layer.classList.add("selecting");
          }
        }
      }
      if (!keep) textLayers.forEach(resetLayer);
    },
    { signal },
  );

  document.addEventListener(
    "keyup",
    () => {
      if (pointerDown) return;
      // Escape / navigation may clear selection — only then tear down.
      resetInactiveLayers();
    },
    { signal },
  );

  // Capture on document: Ctrl+C / context-menu Copy often target body.
  document.addEventListener(
    "copy",
    (event) => {
      const selection = document.getSelection();
      if (!selection || selection.rangeCount === 0) return;

      for (const layer of textLayers.keys()) {
        if (!layer.contains(selection.anchorNode) && !layer.contains(selection.focusNode)) {
          continue;
        }
        let text = selectionTextFromLayer(selection, layer);
        const snap = dragSnapshots.get(layer);
        if (snap && (!text || snap.length > text.length)) text = snap;
        if (text == null || text === "") continue;
        event.clipboardData?.setData("text/plain", text);
        event.preventDefault();
        event.stopPropagation();
        return;
      }
    },
    { capture: true, signal },
  );

  let isFirefox: boolean | undefined;
  document.addEventListener(
    "selectionchange",
    () => {
      const selection = document.getSelection();
      if (!selection || selection.rangeCount === 0 || selection.isCollapsed) {
        textLayers.forEach(resetLayer);
        prevRange = null;
        return;
      }

      const active = new Set<HTMLElement>();
      for (let i = 0; i < selection.rangeCount; i++) {
        const range = selection.getRangeAt(i);
        for (const layer of textLayers.keys()) {
          if (range.intersectsNode(layer)) active.add(layer);
        }
      }
      for (const [layer, end] of textLayers) {
        if (active.has(layer)) layer.classList.add("selecting");
        else resetLayer(end, layer);
      }

      const firstEnd = textLayers.values().next().value as HTMLElement | undefined;
      if (!firstEnd) return;
      isFirefox ??= getComputedStyle(firstEnd).getPropertyValue("-moz-user-select") === "none";
      if (isFirefox) return;

      // Mozilla endOfContent dance: park the bridge next to the selection tip
      // so Chromium can keep spanning absolute word spans.
      const range = selection.getRangeAt(0);
      const modifyStart =
        !!prevRange &&
        (range.compareBoundaryPoints(Range.END_TO_END, prevRange) === 0 ||
          range.compareBoundaryPoints(Range.START_TO_END, prevRange) === 0);
      let anchor: Node | null = modifyStart ? range.startContainer : range.endContainer;
      if (anchor?.nodeType === Node.TEXT_NODE) anchor = anchor.parentNode;
      const parentTextLayer = (anchor as Element | null)?.parentElement?.closest(".textLayer") as
        | HTMLElement
        | null
        | undefined;
      const endDiv = parentTextLayer ? textLayers.get(parentTextLayer) : undefined;
      if (endDiv && parentTextLayer && anchor) {
        endDiv.style.width = parentTextLayer.style.width || `${parentTextLayer.clientWidth}px`;
        endDiv.style.height = parentTextLayer.style.height || `${parentTextLayer.clientHeight}px`;
        const parent = (anchor as Element).parentElement;
        if (parent) {
          parent.insertBefore(endDiv, modifyStart ? (anchor as Node) : (anchor as Node).nextSibling);
        }
      }
      prevRange = range.cloneRange();
    },
    { signal },
  );
}

/** Register a rendered `.textLayer` for PDF.js-style selection / copy. */
export function bindTextLayerSelection(layer: HTMLElement): () => void {
  layer.querySelector(".endOfContent")?.remove();
  const end = document.createElement("div");
  end.className = "endOfContent";
  layer.append(end);

  const onMouseDown = () => layer.classList.add("selecting");
  layer.addEventListener("mousedown", onMouseDown);

  textLayers.set(layer, end);
  enableGlobalSelectionListener();

  return () => {
    layer.removeEventListener("mousedown", onMouseDown);
    textLayers.delete(layer);
    dragSnapshots.delete(layer);
    end.remove();
    if (textLayers.size === 0) {
      selectionAbort?.abort();
      selectionAbort = null;
      prevRange = null;
      dragOrigin = null;
    }
  };
}

/**
 * Rebuild clipboard text from selected textLayer spans, inserting a space
 * between adjacent same-line spans when neither already carries whitespace.
 */
export function selectionTextFromLayer(selection: Selection, layer: HTMLElement): string | null {
  if (selection.rangeCount === 0) return null;
  const range = selection.getRangeAt(0);
  if (!range.intersectsNode(layer)) return null;

  const spans = layerSpans(layer);
  const rangeRects = [...range.getClientRects()];
  const pieces: { text: string; top: number; left: number }[] = [];

  for (const span of spans) {
    const rect = span.getBoundingClientRect();
    const inDom = selection.containsNode(span, true);
    const inGeom = rangeRects.length > 0 && rangeRects.some((r) => rectsOverlap(r, rect));
    if (!inDom && !inGeom) continue;
    const selected = selectedTextInSpan(span, range);
    const text = selected || (inGeom || inDom ? (span.textContent ?? "") : "");
    if (!text) continue;
    pieces.push({ text, top: rect.top, left: rect.left });
  }

  if (pieces.length === 0) {
    return selection.toString() || dragSnapshots.get(layer) || null;
  }

  return joinSpanTexts(pieces);
}

function selectedTextInSpan(span: HTMLElement, range: Range): string {
  const text = span.textContent ?? "";
  if (!text) return "";
  try {
    if (range.intersectsNode(span)) {
      const spanRange = document.createRange();
      spanRange.selectNodeContents(span);
      const fully =
        range.compareBoundaryPoints(Range.START_TO_START, spanRange) <= 0 &&
        range.compareBoundaryPoints(Range.END_TO_END, spanRange) >= 0;
      if (fully) return text;

      const inter = range.cloneRange();
      if (range.compareBoundaryPoints(Range.START_TO_START, spanRange) < 0) {
        inter.setStart(spanRange.startContainer, spanRange.startOffset);
      }
      if (range.compareBoundaryPoints(Range.END_TO_END, spanRange) > 0) {
        inter.setEnd(spanRange.endContainer, spanRange.endOffset);
      }
      return inter.toString();
    }
  } catch {
    /* intersectsNode can throw on detached nodes */
  }
  return "";
}

/** Pure helpers for node unit tests (no DOM Selection required). */
export const __test = { joinSpanTexts, spansInDragRect, textFromSpans };
