# PdfCraft and WordCraft

The supported download starts four pieces together:

| Piece | Role |
| --- | --- |
| Paperrust | Archive, sign-in, and the web interface |
| pgrust | Database. Speaks the PostgreSQL wire protocol |
| PdfCraft | Editor for PDF files |
| WordCraft | Editor for Word files |

Docker Compose builds the two editors from `packaging/pdfcraft` and `packaging/wordcraft`. Each Dockerfile downloads one official web zip and checks its SHA-256. The zips are not committed in this repository. The editor containers are not published on the host. Paperrust reaches them as `http://pdfcraft` and `http://wordcraft`.

## What you see

On a PDF, **Éditer** opens PdfCraft at `/pdfcraft/`. On a Word file, the same button opens WordCraft at `/wordcraft/`. The editor is an iframe on the Paperrust origin. The query string (`file`, `host`, `author`) stays in the browser. Paperrust does not forward it to the editor container.

**Enregistrer** sends the file back to the archive:

- A PDF replaces the stored original. OCR runs again.
- A Word save keeps the word-processor file as the original. A PDF export from WordCraft is stored beside it and used for the page preview.

Upload accepts PDF, PNG, JPEG, and the Word formats WordCraft rewrites (DOCX, DOCM, DOTX, DOTM, ODT, OTT, FODT, RTF, and the older DOC/DOT names). The server classifies the bytes.

## Configuration

The example Compose file sets:

```text
PAPERRUST_PDFCRAFT_UPSTREAM=http://pdfcraft
PAPERRUST_WORDCRAFT_UPSTREAM=http://wordcraft
```

Each value must be `http://` plus one DNS label, with no port, user, path, or query. Anything else falls back to those two defaults.

Leave `PAPERRUST_PDFCRAFT_ORIGIN` empty. Set it only when a browser on another HTTPS origin must fetch the signed PDF. The value is one exact `https://` origin. A wildcard is ignored.

Stirling-PDF is still in the tree as an optional integration. The document button does not use it.
