import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  WORDCRAFT_EXPORT_DELAY_MS,
  WORD_APPLY_HELP,
  WORD_UPLOAD_ACCEPT,
  WORD_UPLOAD_REFUSAL,
  clientUploadRefusal,
  isWordDocument,
  requestWordSave,
  wordcraftFrameSrc,
  wordcraftSaveKind,
  wordEditionTabUrl,
} from "./wordcraft-edit.ts";

function bytes(text: string): Uint8Array {
  return new TextEncoder().encode(text);
}

describe("wordcraft edit", () => {
  it("routes word formats and refuses spreadsheets", () => {
    assert.equal(isWordDocument("", "contrat.docx"), true);
    assert.equal(isWordDocument("", "modele.dotx"), true);
    assert.equal(isWordDocument("", "lettre.odt"), true);
    assert.equal(isWordDocument("", "note.fodt"), true);
    assert.equal(isWordDocument("", "ancien.doc"), true);
    assert.equal(isWordDocument("application/rtf", "note.bin"), true);
    assert.equal(isWordDocument("", "table.xlsx"), false);
    assert.equal(isWordDocument("", "deck.pptx"), false);
    assert.equal(
      WORD_UPLOAD_REFUSAL,
      "Type de fichier non supporté (PDF, PNG, JPEG ou document Word).",
    );
  });

  it("builds a same-origin frame url", () => {
    const src = wordcraftFrameSrc("Alex Martin");
    assert.equal(src.startsWith("/wordcraft/?host=parent"), true);
    assert.match(src, /author=Alex(\+|%20)Martin/);
    assert.equal(src.includes("http"), false);
    assert.equal(/^https?:/i.test(src), false);
    const long = wordcraftFrameSrc("é".repeat(250));
    const author = new URL(long, "https://archive.example").searchParams.get("author") ?? "";
    assert.equal(author.length, 200);
    assert.equal(WORD_UPLOAD_ACCEPT.includes("image/*"), false);
    assert.equal(WORD_UPLOAD_ACCEPT.includes(".svg"), false);
    assert.equal(WORDCRAFT_EXPORT_DELAY_MS >= 1500 && WORDCRAFT_EXPORT_DELAY_MS <= 2000, true);
  });

  it("refuses files the archive does not store", () => {
    assert.equal(clientUploadRefusal({ name: "scan.pdf", type: "application/pdf" }), null);
    assert.equal(clientUploadRefusal({ name: "photo.jpg", type: "image/jpeg" }), null);
    assert.equal(clientUploadRefusal({ name: "photo.png", type: "" }), null);
    assert.equal(clientUploadRefusal({ name: "contrat.docx", type: "" }), null);
    assert.equal(clientUploadRefusal({ name: "dessin.svg", type: "image/svg+xml" }), WORD_UPLOAD_REFUSAL);
    assert.equal(clientUploadRefusal({ name: "table.xlsx", type: "" }), WORD_UPLOAD_REFUSAL);
    assert.equal(clientUploadRefusal({ name: "anim.gif", type: "image/gif" }), WORD_UPLOAD_REFUSAL);
  });

  it("routes a WordCraft save before the server sees it", () => {
    assert.equal(wordcraftSaveKind("export.pdf", bytes("%PDF-1.7")), "rendition");
    assert.equal(wordcraftSaveKind("page.pdf", new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d])), "ignore");
    assert.equal(wordcraftSaveKind("page.jpg", new Uint8Array([0xff, 0xd8, 0xff, 0xe0])), "ignore");
    assert.equal(wordcraftSaveKind("contrat.docx", bytes("PK")), "original");
    assert.equal(wordcraftSaveKind("lettre.odt", bytes("PK")), "original");
    assert.equal(wordcraftSaveKind("note.rtf", bytes("{\\rtf")), "original");
    assert.equal(wordcraftSaveKind("page.html", bytes("<html>")), "reject");
    assert.equal(wordcraftSaveKind("note.md", bytes("# hi")), "reject");
    assert.equal(wordcraftSaveKind("ancien.doc", bytes("PK")), "reject");
    assert.equal(wordcraftSaveKind("modele.dot", bytes("PK")), "reject");
    assert.equal(wordcraftSaveKind("table.xlsx", bytes("PK")), "reject");
  });

  it("asks WordCraft to save and opens this document in a new tab", () => {
    assert.equal(requestWordSave(null), false);
    assert.equal(requestWordSave({ wordcraft: {} }), false);
    let command = "";
    assert.equal(
      requestWordSave({
        wordcraft: {
          execute(next: string) {
            command = next;
            return Promise.resolve(null);
          },
        },
      }),
      true,
    );
    assert.equal(command, "file.save");
    assert.equal(
      requestWordSave({
        wordcraft: {
          execute() {
            return Promise.reject(new Error("editor"));
          },
        },
      }),
      true,
    );
    assert.equal(
      requestWordSave({
        wordcraft: {
          execute() {
            throw new Error("editor");
          },
        },
      }),
      true,
    );
    const url = wordEditionTabUrl("https://archive.example/", 1056);
    assert.equal(url, "https://archive.example/documents/1056#edition");
    assert.equal(url.includes("/wordcraft"), false);
    assert.equal(
      WORD_APPLY_HELP,
      "Enregistrer ici renvoie le document dans Paperrust. L’onglet ouvre le même éditeur.",
    );
    assert.equal(wordEditionTabUrl("https://archive.example", 1056).endsWith("/documents/1056#edition"), true);
  });
});
