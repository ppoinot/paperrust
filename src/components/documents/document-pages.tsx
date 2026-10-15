import { memo, useCallback, useEffect, useState } from "react";
import { DocumentPdfViewer, isPdfMime } from "@/components/documents/document-pdf-viewer";
import { DocThumb } from "@/components/documents/thumb";

export const DocumentPages = memo(function DocumentPages({
  docId,
  title,
  svg,
  fileData,
  mimeType,
  fileRevision,
}: {
  docId: number;
  title: string;
  svg?: string | null;
  fileData?: string | null;
  mimeType?: string;
  fileRevision?: number;
}) {
  const preferPdf = isPdfMime(mimeType);
  const [pdfFailed, setPdfFailed] = useState(false);
  const onPdfError = useCallback(() => setPdfFailed(true), []);

  useEffect(() => {
    setPdfFailed(false);
  }, [docId, mimeType, fileRevision]);

  if (preferPdf && !pdfFailed) {
    return (
      <DocumentPdfViewer
        docId={docId}
        title={title}
        fileRevision={fileRevision}
        onError={onPdfError}
      />
    );
  }

  return (
    <RasterPages
      docId={docId}
      title={title}
      svg={svg}
      fileData={fileData}
      mimeType={mimeType}
      fileRevision={fileRevision}
    />
  );
});

const RasterPages = memo(function RasterPages({
  docId,
  title,
  svg,
  fileData,
  mimeType,
  fileRevision,
}: {
  docId: number;
  title: string;
  svg?: string | null;
  fileData?: string | null;
  mimeType?: string;
  fileRevision?: number;
}) {
  const [pages, setPages] = useState<string[] | null>(null);
  const [broken, setBroken] = useState(false);

  useEffect(() => {
    let live = true;
    setPages(null);
    setBroken(false);
    const preview =
      fileRevision && fileRevision > 0
        ? `/api/documents/${docId}/preview?v=${fileRevision}`
        : `/api/documents/${docId}/preview`;
    fetch(preview)
      .then((r) => {
        const ct = r.headers.get("content-type") ?? "";
        if (!r.ok || r.status === 204 || !ct.includes("json")) throw new Error("no preview");
        return r.json() as Promise<{ pages?: string[] }>;
      })
      .then((meta) => {
        if (!live) return;
        const list = Array.isArray(meta.pages) ? meta.pages : [];
        if (!list.length) setBroken(true);
        else setPages(list);
      })
      .catch(() => {
        if (live) setBroken(true);
      });
    return () => {
      live = false;
    };
  }, [docId, fileRevision]);

  const fallback = (
    <DocThumb
      documentId={docId}
      svg={svg}
      fileData={fileData}
      mimeType={mimeType}
      alt={title}
      className="mx-auto max-h-[72vh] w-full rounded-lg object-contain"
    />
  );

  if (broken || pages?.length === 0) return fallback;
  if (!pages) return fallback;

  return (
    <div className="flex flex-col gap-3">
      {pages.map((src, i) => (
        <img
          key={src}
          src={src}
          alt={`${title} — page ${i + 1}`}
          className="mx-auto w-full rounded-lg bg-white object-contain"
          onError={() => setBroken(true)}
        />
      ))}
    </div>
  );
});
