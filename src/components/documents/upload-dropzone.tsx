import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Loader2, Upload } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { fetchSettings, uploadDocument } from "@/lib/paperless/actions";
import { OCR_STATUS_LABEL } from "@/lib/paperless/format";
import { resolveHasConfiguredKey } from "@/lib/paperless/llm-key-status";
import {
  ocrViaHint,
  providerLabel,
  readLlmSettings,
  type ClientLlmSettings,
} from "@/lib/paperless/llm-settings";
import type { OcrStatus } from "@/lib/paperless/types";
import {
  humanizeUploadError,
  MAX_UPLOAD_BYTES,
  UPLOAD_TOO_LARGE_FR,
} from "@/lib/paperless/upload-errors";
import { cn } from "@/lib/utils";

type ConsumeResult = {
  documentId: number;
  ocr: unknown;
  usedLlm: boolean;
  error?: string;
  ocrStatus?: string;
};

type Row = { name: string; status: OcrStatus };

function ocrStatusOf(r: ConsumeResult): OcrStatus {
  const raw = r.ocrStatus;
  if (raw === "pending" || raw === "processing") return raw;
  if (r.usedLlm) return "done";
  if (raw === "done" && !r.usedLlm) return "skipped";
  if (raw === "skipped" || raw === "failed") return "skipped";
  if (r.error) return "skipped";
  return "skipped";
}

const OCR_TONE: Record<OcrStatus, string> = {
  pending: "text-warn",
  processing: "text-warn",
  done: "text-ok",
  skipped: "text-muted",
  failed: "text-danger",
};

/** Upload errors: toast only as fallback (top-center), never bottom-right. */
function uploadErrorToast(message: string) {
  toast.error(message, { position: "top-center" });
}

export function UploadDropzone({ compact }: { compact?: boolean }) {
  const qc = useQueryClient();
  const inputRef = useRef<HTMLInputElement>(null);
  const [drag, setDrag] = useState(false);
  const [rows, setRows] = useState<Row[]>([]);
  const [inlineError, setInlineError] = useState<string | null>(null);
  const [llm, setLlm] = useState<ClientLlmSettings>(() => readLlmSettings());

  const settingsQ = useQuery({
    queryKey: ["settings"],
    queryFn: () => fetchSettings(),
    staleTime: 30_000,
  });

  useEffect(() => {
    const sync = () => setLlm(readLlmSettings());
    sync();
    window.addEventListener("paperrust:llm-settings", sync as EventListener);
    window.addEventListener("storage", sync);
    return () => {
      window.removeEventListener("paperrust:llm-settings", sync as EventListener);
      window.removeEventListener("storage", sync);
    };
  }, []);

  const hasKey = resolveHasConfiguredKey(
    llm.provider,
    settingsQ.data?.llmKeys,
    Boolean(settingsQ.data?.llmKeysFromApi),
  );
  const hint = ocrViaHint(llm.provider);

  const mutation = useMutation({
    mutationFn: async (files: File[]) => {
      setInlineError(null);
      const current = readLlmSettings();
      setLlm(current);
      if (current.provider === "selfhosted" && !current.selfhostedBaseUrl.trim()) {
        throw new Error(
          "URL de base Auto-hébergé manquante — renseignez-la dans Réglages → OCR.",
        );
      }
      const batch = files.slice(0, 5);
      setRows(batch.map((file) => ({ name: file.name, status: "pending" as const })));
      const results: ConsumeResult[] = [];
      for (const file of batch) {
        try {
          const res = (await uploadDocument({
            data: {
              file,
              filename: file.name,
              provider: current.provider,
            },
          })) as ConsumeResult;
          results.push(res);
          const status = ocrStatusOf(res);
          setRows((prev) =>
            prev.map((row) =>
              row.name === file.name && row.status === "pending" ? { ...row, status } : row,
            ),
          );
        } catch (err) {
          throw new Error(humanizeUploadError(err));
        }
      }
      return results;
    },
    onSuccess: (results, files) => {
      qc.invalidateQueries();
      setInlineError(null);
      const batch = files.slice(0, 5);
      const next = results.map((r, i) => ({
        name: batch[i]?.name ?? `document ${r.documentId}`,
        status: ocrStatusOf(r),
      }));
      setRows(next);
      const n = next.length;
      const done = next.filter((r) => r.status === "done").length;
      const skipped = next.filter((r) => r.status === "skipped").length;
      const failed = next.filter((r) => r.status === "failed").length;
      const summary =
        n === 1
          ? `Document importé · ${OCR_STATUS_LABEL[next[0]!.status]}`
          : `${n} importés · ${done} OCR · ${skipped} ignoré${skipped > 1 ? "s" : ""}${failed ? ` · ${failed} échec${failed > 1 ? "s" : ""}` : ""}`;
      toast.success(summary, { position: "top-center" });
    },
    onError: (err: Error) => {
      const message = humanizeUploadError(err);
      setInlineError(message);
      uploadErrorToast(message);
    },
  });

  function take(files: FileList | File[] | null) {
    if (!files || files.length === 0) return;
    const list = [...files];
    const oversized = list.find((f) => f.size > MAX_UPLOAD_BYTES);
    if (oversized) {
      setInlineError(UPLOAD_TOO_LARGE_FR);
      setRows([]);
      uploadErrorToast(UPLOAD_TOO_LARGE_FR);
      return;
    }
    setInlineError(null);
    mutation.mutate(list);
  }

  return (
    <div
      onDragOver={(e) => {
        e.preventDefault();
        setDrag(true);
      }}
      onDragLeave={() => setDrag(false)}
      onDrop={(e) => {
        e.preventDefault();
        setDrag(false);
        take(e.dataTransfer.files);
      }}
      className={cn(
        "rounded-xl border border-dashed border-border bg-surface transition-colors",
        drag && "border-primary bg-surface-2",
        inlineError && "border-danger/50",
        compact ? "p-3" : "p-6",
      )}
    >
      <input
        ref={inputRef}
        type="file"
        accept="image/*,.pdf,.svg,application/pdf"
        multiple
        className="hidden"
        onChange={(e) => take(e.target.files)}
      />
      <div className={cn("flex items-center gap-3", !compact && "flex-col text-center")}>
        <div className="flex size-10 items-center justify-center rounded-lg bg-surface-2 text-primary">
          {mutation.isPending ? (
            <Loader2 className="size-4 animate-spin" />
          ) : (
            <Upload className="size-4" />
          )}
        </div>
        <div className="min-w-0 flex-1">
          <p className="text-sm font-medium">
            {mutation.isPending ? "Consommation et OCR…" : "Déposer des documents"}
          </p>
          <p className="text-xs text-muted">
            PDF, JPEG, PNG — {hint}
            {hasKey ? (
              <span className="ml-1 text-emerald-700 dark:text-emerald-300">
                · Clé {providerLabel(llm.provider)} configurée
              </span>
            ) : null}
          </p>
        </div>
        <Button
          type="button"
          size={compact ? "sm" : "default"}
          disabled={mutation.isPending}
          onClick={() => inputRef.current?.click()}
        >
          Importer
        </Button>
      </div>
      {inlineError ? (
        <p role="alert" className={cn("mt-3 text-sm text-danger", !compact && "text-left")}>
          {inlineError}
        </p>
      ) : null}
      {rows.length > 0 ? (
        <ul className={cn("mt-3 space-y-1", compact ? "" : "text-left")}>
          {rows.map((row, i) => (
            <li key={`${row.name}-${i}`} className="flex items-center justify-between gap-2 text-xs">
              <span className="min-w-0 truncate">{row.name}</span>
              <span className={cn("shrink-0 font-medium", OCR_TONE[row.status])}>
                {OCR_STATUS_LABEL[row.status]}
              </span>
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  );
}
