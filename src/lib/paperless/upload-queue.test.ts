import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { sha256Hex } from "./upload-queue.ts";

describe("sha256Hex", () => {
  it("matches the FIPS vector for abc", async () => {
    assert.equal(
      await sha256Hex(new TextEncoder().encode("abc")),
      "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
    );
  });
});
