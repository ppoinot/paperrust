import assert from "node:assert/strict";
import test from "node:test";

/** Mirror of sortTagsForGrid / compareTagNamesFr (node --test, no TS loader). */
function compareTagNamesFr(a, b) {
  return a.localeCompare(b, "fr", { sensitivity: "base" });
}

function sortTagsForGrid(tags, activeIds) {
  const active = new Set(activeIds ?? []);
  return [...tags].sort((a, b) => {
    const aRank = active.has(a.id) ? 0 : 1;
    const bRank = active.has(b.id) ? 0 : 1;
    if (aRank !== bRank) return aRank - bRank;
    return compareTagNamesFr(a.name, b.name);
  });
}

test("actives first, then inactive A→Z (fr)", () => {
  const tags = [
    { id: 1, name: "Zebra" },
    { id: 2, name: "école" },
    { id: 3, name: "Banque" },
    { id: 4, name: "Assurance" },
  ];
  assert.deepEqual(
    sortTagsForGrid(tags, [1, 3]).map((t) => t.name),
    ["Banque", "Zebra", "Assurance", "école"],
  );
});

test("deactivated tag returns to inactive alpha block", () => {
  const tags = [
    { id: 1, name: "Facture" },
    { id: 2, name: "Banque" },
    { id: 3, name: "Assurance" },
  ];
  assert.deepEqual(sortTagsForGrid(tags, [1, 2]).map((t) => t.id), [2, 1, 3]);
  assert.deepEqual(sortTagsForGrid(tags, [1]).map((t) => t.id), [1, 3, 2]);
});

test("no activeIds → full French alphabetical order", () => {
  const tags = [
    { id: 1, name: "été" },
    { id: 2, name: "avocat" },
    { id: 3, name: "École" },
  ];
  assert.deepEqual(
    sortTagsForGrid(tags).map((t) => t.name),
    ["avocat", "École", "été"],
  );
});
