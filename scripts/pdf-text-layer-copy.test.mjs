import assert from "node:assert/strict";
import test from "node:test";

/** Pure join logic mirrored from joinSpanTexts (no DOM). */
function joinSpanTexts(pieces) {
  if (pieces.length === 0) return "";
  const sorted = [...pieces].sort((a, b) =>
    Math.abs(a.top - b.top) > 4 ? a.top - b.top : a.left - b.left,
  );
  let out = sorted[0].text;
  for (let i = 1; i < sorted.length; i++) {
    const prev = sorted[i - 1];
    const cur = sorted[i];
    const sameLine = Math.abs(cur.top - prev.top) <= 4;
    const needsSpace = sameLine && !/\s$/.test(out) && !/^\s/.test(cur.text);
    const needsNewline = !sameLine && !/\n$/.test(out);
    out += needsNewline ? "\n" : needsSpace ? " " : "";
    out += cur.text;
  }
  return out;
}

/** Pure drag-rect intersection (mirrors spansInDragRect geometry). */
function spansInDragRect(spans, x0, y0, x1, y1) {
  const left = Math.min(x0, x1);
  const right = Math.max(x0, x1);
  const top = Math.min(y0, y1);
  const bottom = Math.max(y0, y1);
  const pad = right - left < 2 && bottom - top < 2 ? 4 : 0;
  const drag = {
    left: left - pad,
    right: right + pad,
    top: top - pad,
    bottom: bottom + pad,
  };
  return spans.filter((s) => {
    const r = s.rect;
    return !(
      r.right < drag.left - 1 ||
      r.left > drag.right + 1 ||
      r.bottom < drag.top - 1 ||
      r.top > drag.bottom + 1
    );
  });
}

test("FOUCAULT + KEVIN same line → space inserted", () => {
  assert.equal(
    joinSpanTexts([
      { text: "FOUCAULT", top: 100, left: 10 },
      { text: "KEVIN", top: 100.5, left: 90 },
    ]),
    "FOUCAULT KEVIN",
  );
});

test("does not double-space when span already has trailing space", () => {
  assert.equal(
    joinSpanTexts([
      { text: "FOUCAULT ", top: 100, left: 10 },
      { text: "KEVIN", top: 100, left: 90 },
    ]),
    "FOUCAULT KEVIN",
  );
});

test("different lines → newline", () => {
  assert.equal(
    joinSpanTexts([
      { text: "ligne1", top: 10, left: 0 },
      { text: "ligne2", top: 30, left: 0 },
    ]),
    "ligne1\nligne2",
  );
});

test("drag rect across FOUCAULT+KEVIN excludes phone", () => {
  const spans = [
    { text: "FOUCAULT", rect: { left: 16, right: 110, top: 28, bottom: 46 } },
    { text: "KEVIN", rect: { left: 130, right: 190, top: 28, bottom: 46 } },
    { text: "02.38.79.75.32", rect: { left: 220, right: 360, top: 28, bottom: 46 } },
  ];
  const hit = spansInDragRect(spans, 18, 36, 185, 36);
  assert.deepEqual(
    hit.map((s) => s.text),
    ["FOUCAULT", "KEVIN"],
  );
  assert.equal(
    joinSpanTexts(hit.map((s) => ({ text: s.text, top: s.rect.top, left: s.rect.left }))),
    "FOUCAULT KEVIN",
  );
});
