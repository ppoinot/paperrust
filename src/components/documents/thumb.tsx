import { useState } from "react";
import { cn } from "@/lib/utils";

const FALLBACK_SVG = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 84 112">
  <rect width="84" height="112" rx="4" fill="#f3efe6"/>
  <rect x="12" y="14" width="60" height="84" rx="2" fill="#fbf7ee" stroke="#d7d0c3"/>
  <path d="M24 40h36M24 52h36M24 64h22" stroke="#d7d0c3" stroke-width="2" fill="none"/>
</svg>`;

const FALLBACK_SRC = `data:image/svg+xml;charset=utf-8,${encodeURIComponent(FALLBACK_SVG)}`;

function rasterHref(fileData?: string | null): string | null {
  if (!fileData) return null;
  if (fileData.startsWith("data:image/png") || fileData.startsWith("data:image/jpeg")) {
    return fileData;
  }
  return null;
}

export function DocThumb({
  svg,
  fileData,
  mimeType,
  className,
  alt,
  documentId,
}: {
  svg?: string | null;
  fileData?: string | null;
  mimeType?: string;
  className?: string;
  alt: string;
  documentId?: number;
}) {
  const [broken, setBroken] = useState(false);
  void svg;
  void mimeType;
  const previewSrc = documentId && !broken ? `/api/documents/${documentId}/preview?page=1` : null;
  const src = previewSrc ?? rasterHref(fileData) ?? FALLBACK_SRC;

  return (
    <img
      src={src}
      alt={alt}
      className={cn("paper-thumb object-cover object-top", className)}
      onError={() => {
        if (documentId && !broken) setBroken(true);
      }}
    />
  );
}
