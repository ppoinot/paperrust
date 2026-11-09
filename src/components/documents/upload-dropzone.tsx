import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Loader2, Upload } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { fetchSettings, findDuplicateBySha256, uploadDocument } from "@/lib/paperless/actions";
import { OCR_STATUS_LABEL } from "@/lib/paperless/format";
import { resolveHasConfiguredKey } from "@/lib/paperless/llm-key-status";
import {
  ocrViaHint,
  providerLabel,
  readLlmSettings,
  type ClientLlmSettings,
} from "@/lib/paperless/llm-settings";
import type { OcrStatus } from "@/lib/paperless/types";
import { sha256Hex } from "@/lib/paperless/upload-queue";
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
  duplicate?: boolean;
};

type Phase = "queued" | "hashing" | "uploading" | "duplicate" | "failed" | OcrStatus;

type QueueItem = {
  id: string;
  file: File;
  name: string;
  phase: Phase;
  detail?: string;
  sha256?: string;
};

type Burst = { imported: number; duplicates: number; failed: number };

const PHASE_LABEL: Record<"queued" | "hashing" | "uploading" | "duplicate" | "failed", string> = {
  queued: "En attente",
  hashing: "Contrôle",
  uploading: "Envoi",
  duplicate: "Déjà présent",
  failed: "Échec",
};

function phaseLabel(phase: Phase): string {
  if (
    phase === "queued" ||
    phase === "hashing" ||
    phase === "uploading" ||
    phase === "duplicate" ||
    phase === "failed"
  ) {
    return PHASE_LABEL[phase];
  }
  return OCR_STATUS_LABEL[phase];
}

function phaseTone(phase: Phase): string {
  if (phase === "queued" || phase === "hashing" || phase === "uploading" || phase === "pending" || phase === "processing") {
    return "text-warn";
  }
  if (phase === "done") return "text-ok";
  if (phase === "failed") return "text-danger";
  return "text-muted";
}

function ocrStatusOf(r: ConsumeResult): OcrStatus {
  const raw = r.ocrStatus;
  if (raw === "pending" || raw === "processing") return raw;
  if (r.usedLlm) return "done";
  if (raw === "done" && !r.usedLlm) return "skipped";
  if (raw === "skipped" || raw === "failed") return "skipped";
  if (r.error) return "skipped";
  return "skipped";
}

function burstSummary(burst: Burst): string | null {
  const parts: string[] = [];
  if (burst.imported > 0) {
    parts.push(burst.imported === 1 ? "1 importé" : `${burst.imported} importés`);
  }
  if (burst.duplicates > 0) {
    parts.push(
      burst.duplicates === 1 ? "1 déjà présent" : `${burst.duplicates} déjà présents`,
    );
  }
  if (burst.failed > 0) {
    parts.push(burst.failed === 1 ? "1 échec" : `${burst.failed} échecs`);
  }
  return parts.length > 0 ? parts.join(" · ") : null;
}

/** Upload errors: toast only as fallback (top-center), never bottom-right. */
function uploadErrorToast(message: string) {
  toast.error(message, { position: "top-center" });
}

function isSessionError(message: string): boolean {
  return message.toLowerCase().includes("session expirée");
}

export function UploadDropzone({ compact }: { compact?: boolean }) {
  const qc = useQueryClient();
  const inputRef = useRef<HTMLInputElement>(null);
  const itemsRef = useRef<QueueItem[]>([]);
  const runningRef = useRef(false);
  const burstRef = useRef<Burst>({ imported: 0, duplicates: 0, failed: 0 });
  const [drag, setDrag] = useState(false);
  const [rows, setRows] = useState<QueueItem[]>([]);
  const [busy, setBusy] = useState(false);
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

  function publish() {
    setRows(itemsRef.current.map((item) => ({ ...item })));
  }

  function patch(id: string, partial: Partial<QueueItem>) {
    itemsRef.current = itemsRef.current.map((item) =>
      item.id === id ? { ...item, ...partial } : item,
    );
    publish();
  }

  function finishBurst() {
    const summary = burstSummary(burstRef.current);
    burstRef.current = { imported: 0, duplicates: 0, failed: 0 };
    if (summary) toast.success(summary, { position: "top-center" });
  }

  async function processOne(item: QueueItem): Promise<"ok" | "stop"> {
    const current = readLlmSettings();
    setLlm(current);
    if (current.provider === "selfhosted" && !current.selfhostedBaseUrl.trim()) {
      patch(item.id, {
        phase: "failed",
        detail: "URL de base Auto-hébergé manquante — renseignez-la dans Réglages → OCR.",
      });
      burstRef.current.failed += 1;
      return "ok";
    }

    patch(item.id, { phase: "hashing" });
    let hex: string;
    try {
      hex = await sha256Hex(await item.file.arrayBuffer());
    } catch (err) {
      patch(item.id, { phase: "failed", detail: humanizeUploadError(err) });
      burstRef.current.failed += 1;
      return "ok";
    }
    patch(item.id, { sha256: hex });

    try {
      const hit = await findDuplicateBySha256(hex);
      if (hit?.duplicate) {
        patch(item.id, { phase: "duplicate" });
        burstRef.current.duplicates += 1;
        return "ok";
      }
    } catch (err) {
      const message = humanizeUploadError(err);
      if (isSessionError(message)) {
        setInlineError(message);
        uploadErrorToast(message);
        return "stop";
      }
    }

    patch(item.id, { phase: "uploading" });
    try {
      const res = (await uploadDocument({
        data: {
          file: item.file,
          filename: item.name,
          provider: current.provider,
        },
      })) as ConsumeResult;
      if (res.duplicate) {
        patch(item.id, { phase: "duplicate" });
        burstRef.current.duplicates += 1;
        return "ok";
      }
      patch(item.id, { phase: ocrStatusOf(res) });
      burstRef.current.imported += 1;
      void qc.invalidateQueries();
      return "ok";
    } catch (err) {
      const message = humanizeUploadError(err);
      patch(item.id, { phase: "failed", detail: message });
      burstRef.current.failed += 1;
      if (isSessionError(message)) {
        setInlineError(message);
        uploadErrorToast(message);
        return "stop";
      }
      return "ok";
    }
  }

  async function pump() {
    if (runningRef.current) return;
    runningRef.current = true;
    setBusy(true);
    let stop = false;
    try {
      for (;;) {
        const next = itemsRef.current.find((item) => item.phase === "queued");
        if (!next || stop) break;
        const outcome = await processOne(next);
        if (outcome === "stop") {
          itemsRef.current = itemsRef.current.map((item) =>
            item.phase === "queued"
              ? { ...item, phase: "failed", detail: "Session expirée — reconnectez-vous." }
              : item,
          );
          publish();
          stop = true;
        }
      }
    } finally {
      runningRef.current = false;
    }
    if (!stop && itemsRef.current.some((item) => item.phase === "queued")) {
      void pump();
      return;
    }
    setBusy(false);
    finishBurst();
  }

  function take(files: FileList | File[] | null) {
    if (!files || files.length === 0) return;
    const added: QueueItem[] = [];
    let rejected = false;
    for (const file of files) {
      if (file.size > MAX_UPLOAD_BYTES) {
        rejected = true;
        added.push({
          id: crypto.randomUUID(),
          file,
          name: file.name,
          phase: "failed",
          detail: UPLOAD_TOO_LARGE_FR,
        });
        burstRef.current.failed += 1;
        continue;
      }
      added.push({
        id: crypto.randomUUID(),
        file,
        name: file.name,
        phase: "queued",
      });
    }
    itemsRef.current = [...itemsRef.current, ...added];
    publish();
    if (rejected) {
      setInlineError(UPLOAD_TOO_LARGE_FR);
      uploadErrorToast(UPLOAD_TOO_LARGE_FR);
    } else {
      setInlineError(null);
    }
    if (inputRef.current) inputRef.current.value = "";
    void pump();
  }

  const waiting = rows.filter((row) => row.phase === "queued").length;

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
          {busy ? <Loader2 className="size-4 animate-spin" /> : <Upload className="size-4" />}
        </div>
        <div className="min-w-0 flex-1">
          <p className="text-sm font-medium">
            {busy ? "Import en cours" : "Déposer des documents"}
          </p>
          <p className="text-xs text-muted">
            {busy
              ? `Vous pouvez ajouter d'autres fichiers.${waiting > 0 ? ` ${waiting} en attente.` : ""}`
              : `PDF, JPEG, PNG — ${hint}`}
            {!busy && hasKey ? (
              <span className="ml-1 text-emerald-700 dark:text-emerald-300">
                · Clé {providerLabel(llm.provider)} configurée
              </span>
            ) : null}
          </p>
        </div>
        <Button
          type="button"
          size={compact ? "sm" : "default"}
          onClick={() => inputRef.current?.click()}
        >
          {busy ? "Ajouter" : "Importer"}
        </Button>
      </div>
      {inlineError ? (
        <p role="alert" className={cn("mt-3 text-sm text-danger", !compact && "text-left")}>
          {inlineError}
        </p>
      ) : null}
      {rows.length > 0 ? (
        <ul className={cn("mt-3 max-h-64 space-y-1 overflow-y-auto", compact ? "" : "text-left")}>
          {rows.map((row) => (
            <li key={row.id} className="flex items-center justify-between gap-2 text-xs">
              <span className="min-w-0 truncate" title={row.detail ?? row.name}>
                {row.name}
                {row.detail ? <span className="text-danger"> — {row.detail}</span> : null}
              </span>
              <span className={cn("shrink-0 font-medium", phaseTone(row.phase))}>
                {phaseLabel(row.phase)}
              </span>
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  );
}
