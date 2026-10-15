import { useMutation, useQuery } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { cancelPaperlessImport, fetchPaperlessImportJob, startPaperlessImport } from "@/lib/paperless/actions";
import {
  PAPERLESS_IMPORT_DEFAULTS,
  PAPERLESS_IMPORT_MODAL_HELP,
  PAPERLESS_IMPORT_SECTION_HELP,
  PAPERLESS_IMPORT_SECTION_HINT,
  PAPERLESS_IMPORT_TOC_ID,
  PAPERLESS_IMPORT_TOKEN_HINT,
  acceptPaperlessJobId,
  isPaperlessImportTerminal,
  paperlessImportPhaseLabel,
  paperlessImportPollIntervalMs,
  paperlessImportStateLabel,
  paperlessImportUiState,
  readActivePaperlessImportJobId,
  validatePaperlessHttpsUrl,
  writeActivePaperlessImportJobId,
  type PaperlessImportJob,
} from "@/lib/paperless/paperless-import";
import { cn } from "@/lib/utils";

function ProgressTrack({ job }: { job: PaperlessImportJob }) {
  const total = job.total;
  const done = job.done;
  const pct = total > 0 ? Math.min(100, Math.round((done / total) * 100)) : 0;
  const indeterminate = total <= 0 && !isPaperlessImportTerminal(job.status);

  return (
    <div className="space-y-3">
      <div className="flex items-center gap-4">
        <Donut percent={indeterminate ? null : pct} running={job.status === "running" || job.status === "queued"} />
        <div className="min-w-0 flex-1">
          <div className="flex items-baseline justify-between gap-3">
            <p className="text-sm font-medium text-fg">
              {total > 0 ? (
                <>
                  <span className="font-mono tabular-nums">{done}</span>
                  <span className="text-muted"> / </span>
                  <span className="font-mono tabular-nums">{total}</span>
                </>
              ) : (
                <span className="text-muted">Documents…</span>
              )}
            </p>
            {total > 0 ? (
              <span className="font-mono text-xs text-faint tabular-nums">{pct} %</span>
            ) : null}
          </div>
          <div
            className="mt-2 h-2 overflow-hidden rounded-full bg-surface-2"
            role="progressbar"
            aria-valuemin={0}
            aria-valuemax={total > 0 ? total : 100}
            aria-valuenow={total > 0 ? done : undefined}
            aria-label="Progression de l'import"
          >
            <div
              className={cn(
                "h-full rounded-full bg-primary",
                indeterminate && "w-1/3 animate-pulse",
              )}
              style={indeterminate ? undefined : { width: `${pct}%` }}
            />
          </div>
        </div>
      </div>
    </div>
  );
}

function Donut({ percent, running }: { percent: number | null; running: boolean }) {
  const r = 16;
  const c = 2 * Math.PI * r;
  const p = percent ?? 0;
  const dash = (p / 100) * c;
  return (
    <svg
      viewBox="0 0 40 40"
      className={cn("size-12 shrink-0 -rotate-90 text-primary", running && percent === null && "animate-pulse")}
      aria-hidden
    >
      <circle
        cx="20"
        cy="20"
        r={r}
        fill="none"
        className="text-surface-2"
        stroke="currentColor"
        strokeWidth="5"
      />
      <circle
        cx="20"
        cy="20"
        r={r}
        fill="none"
        stroke="currentColor"
        strokeWidth="5"
        strokeLinecap="round"
        strokeDasharray={percent === null ? `${c * 0.25} ${c}` : `${dash} ${c - dash}`}
      />
    </svg>
  );
}

function JobPanel({
  job,
  onCancel,
  cancelPending,
}: {
  job: PaperlessImportJob;
  onCancel: () => void;
  cancelPending: boolean;
}) {
  const ui = paperlessImportUiState(job);
  const phase = paperlessImportPhaseLabel(job.phase, job.status);
  const running = ui === "running";
  const tone =
    ui === "failed"
      ? "text-danger"
      : ui === "partial" || ui === "cancelled"
        ? "text-warn"
        : ui === "done"
          ? "text-ok"
          : "text-fg";

  return (
    <div className="space-y-3 rounded-lg bg-surface-2/80 px-3 py-3">
      <div className="flex flex-wrap items-start justify-between gap-2">
        <div>
          <p className={cn("text-sm font-medium", tone)}>{paperlessImportStateLabel(ui)}</p>
          {phase ? <p className="mt-0.5 text-xs text-muted">{phase}</p> : null}
          {job.message ? <p className="mt-1 text-xs text-muted">{job.message}</p> : null}
        </div>
        <p className="text-xs text-faint">
          Erreurs : <span className="font-mono tabular-nums text-fg">{job.errors}</span>
        </p>
      </div>
      <ProgressTrack job={job} />
      {job.errorSamples.length > 0 ? (
        <ul className="space-y-1 rounded-md bg-surface/70 px-2.5 py-2 text-xs">
          {job.errorSamples.map((s, i) => (
            <li key={`${s.externalId}-${i}`} className="break-words text-muted">
              {s.externalId ? (
                <span className="font-mono text-fg">{s.externalId}</span>
              ) : (
                <span className="text-faint">document</span>
              )}
              {s.message ? <span> — {s.message}</span> : null}
            </li>
          ))}
        </ul>
      ) : null}
      {running ? (
        <Button
          type="button"
          variant="outline"
          size="sm"
          disabled={cancelPending}
          onClick={onCancel}
        >
          Annuler
        </Button>
      ) : null}
    </div>
  );
}

export function PaperlessImportSection() {
  const [open, setOpen] = useState(false);
  const [baseUrl, setBaseUrl] = useState("");
  const [token, setToken] = useState("");
  const [skipOcrIfPresent, setSkipOcrIfPresent] = useState<boolean>(
    PAPERLESS_IMPORT_DEFAULTS.skipOcrIfPresent,
  );
  const [preserveTags, setPreserveTags] = useState<boolean>(PAPERLESS_IMPORT_DEFAULTS.preserveTags);
  const [dryRun, setDryRun] = useState<boolean>(PAPERLESS_IMPORT_DEFAULTS.dryRun);
  const [urlError, setUrlError] = useState<string | null>(null);
  const [jobId, setJobId] = useState<string | null>(() =>
    acceptPaperlessJobId(readActivePaperlessImportJobId()),
  );

  useEffect(() => {
    writeActivePaperlessImportJobId(jobId);
  }, [jobId]);

  const jobQ = useQuery({
    queryKey: ["paperless-import-job", jobId],
    queryFn: () => fetchPaperlessImportJob(jobId!),
    enabled: Boolean(jobId),
    refetchInterval: (query) => {
      if (query.state.error) return false;
      const status = query.state.data?.status;
      if (isPaperlessImportTerminal(status)) return false;
      return paperlessImportPollIntervalMs(query.state.dataUpdateCount);
    },
    retry: (count, err) => {
      const status = err instanceof Error ? (err as Error & { status?: number }).status : undefined;
      if (status === 404 || status === 409) return false;
      return count < 2;
    },
  });

  useEffect(() => {
    if (!jobQ.error) return;
    const status = (jobQ.error as Error & { status?: number }).status;
    if (status === 404) setJobId(null);
  }, [jobQ.error]);

  const start = useMutation({
    mutationFn: async () => {
      const httpsErr = validatePaperlessHttpsUrl(baseUrl);
      if (httpsErr) {
        setUrlError(httpsErr);
        throw new Error(httpsErr);
      }
      setUrlError(null);
      if (!token.trim()) {
        throw new Error("Le jeton API Paperless-ngx est requis.");
      }
      const res = await startPaperlessImport({
        baseUrl: baseUrl.trim(),
        token,
        skipOcrIfPresent,
        preserveTags,
        dryRun,
      });
      return res;
    },
    onSuccess: (res) => {
      setToken("");
      const accepted = acceptPaperlessJobId(res.jobId);
      if (accepted) setJobId(accepted);
      toast.success(dryRun ? "Simulation d'import lancée" : "Import Paperless-ngx lancé");
    },
    onError: (e: Error) => toast.error(e.message),
  });

  const cancel = useMutation({
    mutationFn: async () => {
      if (!jobId) throw new Error("Aucun import à annuler.");
      return cancelPaperlessImport(jobId);
    },
    onSuccess: () => {
      toast.success("Annulation demandée");
      void jobQ.refetch();
    },
    onError: (e: Error) => toast.error(e.message),
  });

  function closeModal(next: boolean) {
    setOpen(next);
    if (!next) {
      setToken("");
      setUrlError(null);
    }
  }

  const job = jobQ.data ?? null;
  const ui = paperlessImportUiState(job);
  const busy = ui === "running";
  const awaitingFirstStatus = Boolean(jobId) && !job && !jobQ.isError;
  const showProgressInModal = busy || awaitingFirstStatus;

  return (
    <section
      id={PAPERLESS_IMPORT_TOC_ID}
      className="scroll-mt-24 rounded-xl bg-surface p-5 shadow-[var(--shadow-border)]"
    >
      <h2 className="font-display text-xl">Import Paperless-ngx</h2>
      <p className="mt-1 text-sm text-muted">{PAPERLESS_IMPORT_SECTION_HELP}</p>
      <p className="mt-1 text-xs text-faint">{PAPERLESS_IMPORT_SECTION_HINT}</p>
      <div className="mt-4">
        <Button type="button" disabled={busy} onClick={() => setOpen(true)}>
          Importer depuis Paperless-ngx
        </Button>
      </div>
      {job ? (
        <div className="mt-4">
          <JobPanel job={job} onCancel={() => cancel.mutate()} cancelPending={cancel.isPending} />
        </div>
      ) : null}

      <Dialog open={open} onOpenChange={closeModal}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Importer depuis Paperless-ngx</DialogTitle>
            <DialogDescription>{PAPERLESS_IMPORT_MODAL_HELP}</DialogDescription>
          </DialogHeader>

          {showProgressInModal ? (
            job ? (
              <JobPanel job={job} onCancel={() => cancel.mutate()} cancelPending={cancel.isPending} />
            ) : (
              <p className="text-sm text-muted">Démarrage de l&apos;import…</p>
            )
          ) : (
            <form
              className="space-y-4"
              onSubmit={(e) => {
                e.preventDefault();
                start.mutate();
              }}
            >
              <div>
                <Label htmlFor="paperless-base-url">URL du serveur</Label>
                <Input
                  id="paperless-base-url"
                  className="mt-1 font-mono"
                  type="url"
                  inputMode="url"
                  autoComplete="off"
                  spellCheck={false}
                  placeholder="https://paperless.exemple"
                  value={baseUrl}
                  onChange={(e) => {
                    setBaseUrl(e.target.value);
                    if (urlError) setUrlError(validatePaperlessHttpsUrl(e.target.value));
                  }}
                  onBlur={() => {
                    if (baseUrl.trim()) setUrlError(validatePaperlessHttpsUrl(baseUrl));
                  }}
                />
                {urlError ? <p className="mt-1 text-xs text-danger">{urlError}</p> : null}
              </div>
              <div>
                <Label htmlFor="paperless-api-token">Jeton API</Label>
                <Input
                  id="paperless-api-token"
                  className="mt-1 font-mono"
                  type="password"
                  autoComplete="new-password"
                  autoCorrect="off"
                  spellCheck={false}
                  value={token}
                  onChange={(e) => setToken(e.target.value)}
                  placeholder="Jeton Paperless-ngx"
                />
                <p className="mt-1 text-xs text-faint">{PAPERLESS_IMPORT_TOKEN_HINT}</p>
              </div>
              <div className="space-y-2.5 rounded-lg bg-surface-2/80 px-3 py-3">
                <label className="flex items-center justify-between gap-3 text-sm">
                  <span>Ignorer l&apos;OCR si le texte est déjà présent</span>
                  <Switch checked={skipOcrIfPresent} onCheckedChange={setSkipOcrIfPresent} />
                </label>
                <label className="flex items-center justify-between gap-3 text-sm">
                  <span>Conserver les étiquettes</span>
                  <Switch checked={preserveTags} onCheckedChange={setPreserveTags} />
                </label>
                <label className="flex items-center justify-between gap-3 text-sm">
                  <span>Simulation (dry-run)</span>
                  <Switch checked={dryRun} onCheckedChange={setDryRun} />
                </label>
              </div>
              <div className="flex flex-wrap justify-end gap-2">
                <Button type="button" variant="outline" onClick={() => closeModal(false)}>
                  Fermer
                </Button>
                <Button type="submit" disabled={start.isPending}>
                  Lancer l&apos;import
                </Button>
              </div>
            </form>
          )}
        </DialogContent>
      </Dialog>
    </section>
  );
}
