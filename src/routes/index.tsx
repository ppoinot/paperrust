import { useQuery } from "@tanstack/react-query";
import { ClientOnly, createFileRoute, Link } from "@tanstack/react-router";
import { lazy, Suspense } from "react";
import { UploadDropzone } from "@/components/documents/upload-dropzone";
import { TagChip } from "@/components/documents/tag-chip";
import { DocThumb } from "@/components/documents/thumb";
import { AppShell } from "@/components/layout/app-shell";
import { isAuthRequired } from "@/lib/auth/auth-required";
import { fetchDashboard } from "@/lib/paperless/actions";
import { formatDate, ocrStatusLabel, sortTagsForGrid } from "@/lib/paperless/format";
import { formatBytes } from "@/lib/utils";

const MonthlyChart = lazy(() =>
  import("@/components/dashboard/monthly-chart.client").then((m) => ({ default: m.MonthlyChart })),
);

function dashboardErrorHint(error: unknown): string {
  const raw = (error as Error)?.message?.trim() ?? "";
  if (!raw) return "";
  try {
    const j = JSON.parse(raw) as { error?: string };
    if (j?.error) return ` : ${j.error}`;
  } catch {
    /* plain text body */
  }
  return ` : ${raw.slice(0, 120)}`;
}

export const Route = createFileRoute("/")({
  loader: async () => {
    try {
      return await fetchDashboard();
    } catch (err) {
      if (isAuthRequired(err)) throw err;
      console.error("[paperrust] dashboard loader", err);
      return null;
    }
  },
  component: Dashboard,
});

function Stat({ label, value, hint }: { label: string; value: string; hint?: string }) {
  return (
    <div className="rounded-xl bg-surface px-4 py-4 shadow-[var(--shadow-border)]">
      <p className="text-[11px] font-medium tracking-[0.14em] text-muted uppercase">{label}</p>
      <p className="mt-2 font-display text-3xl tabular">{value}</p>
      {hint ? <p className="mt-1 text-xs text-faint">{hint}</p> : null}
    </div>
  );
}

function Dashboard() {
  const initial = Route.useLoaderData();
  const { data, isLoading, isError, error } = useQuery({
    queryKey: ["dashboard"],
    queryFn: () => fetchDashboard(),
    ...(initial ? { initialData: initial } : {}),
  });

  return (
    <AppShell title="Tableau de bord">
      <div className="space-y-5">
        <div className="flex flex-col gap-1">
          <p className="text-sm text-muted">Archive personnelle · OCR par modèles de langage</p>
        </div>
        {isError ? (
          <p className="rounded-lg bg-surface px-3 py-2 text-sm text-danger shadow-[var(--shadow-border)]">
            Tableau de bord indisponible
            {dashboardErrorHint(error)}
          </p>
        ) : null}
        <div id="dashboard-upload" data-dashboard="upload">
          <UploadDropzone />
        </div>
        <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
          <Stat label="Documents" value={String(data?.documents ?? "—")} hint={`${data?.pages ?? 0} pages`} />
          <Stat label="Inbox" value={String(data?.inbox ?? "—")} hint="À classer" />
          <Stat
            label="OCR"
            value={data ? `${data.ocrDone}` : "—"}
            hint={data?.ocrPending ? `${data.ocrPending} en cours` : "À jour"}
          />
          <Stat label="Stockage" value={data ? formatBytes(data.storageBytes) : "—"} hint={`${data?.tags ?? 0} étiquettes`} />
        </div>
        <div className="grid items-start gap-4 lg:grid-cols-[1.4fr_1fr]">
          <div className="rounded-xl bg-surface p-4 shadow-[var(--shadow-border)]">
            <p className="text-[11px] font-medium tracking-[0.14em] text-muted uppercase">Entrées par mois</p>
            <div
              id="dashboard-month-chart"
              data-dashboard="month-chart"
              className="dashboard-month-chart mt-2 overflow-hidden"
              style={{ height: 112, maxHeight: 112 }}
            >
              {data?.byMonth.length ? (
                <ClientOnly fallback={<div className="h-full animate-pulse rounded-md bg-surface-2" />}>
                  <Suspense fallback={<div className="h-full animate-pulse rounded-md bg-surface-2" />}>
                    <MonthlyChart data={data.byMonth} />
                  </Suspense>
                </ClientOnly>
              ) : (
                <p className="pt-8 text-center text-sm text-muted">{isLoading ? "Chargement…" : "Pas encore de série"}</p>
              )}
            </div>
          </div>
          <div className="rounded-xl bg-surface p-4 shadow-[var(--shadow-border)]">
            <p className="text-[11px] font-medium tracking-[0.14em] text-muted uppercase">Par type</p>
            <ul className="mt-3 space-y-2">
              {(data?.byType ?? []).map((row) => (
                <li key={row.name} className="flex items-center gap-3 text-sm">
                  <span className="flex-1 truncate" title={row.name}>
                    {row.name}
                  </span>
                  <span className="h-1.5 flex-1 overflow-hidden rounded-full bg-surface-2">
                    <span
                      className="block h-full rounded-full bg-primary"
                      style={{ width: `${Math.min(100, (row.count / Math.max(1, data?.documents ?? 1)) * 100)}%` }}
                    />
                  </span>
                  <span className="tabular w-6 text-right text-muted">{row.count}</span>
                </li>
              ))}
            </ul>
          </div>
        </div>
        <div>
          <div className="mb-3 flex items-end justify-between">
            <h2 className="font-display text-xl">Récents</h2>
            <Link to="/documents" search={{}} className="text-sm text-primary hover:underline">
              Tout voir
            </Link>
          </div>
          <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
            {(data?.recent ?? []).map((d) => (
              <Link
                key={d.id}
                to="/documents/$id"
                params={{ id: String(d.id) }}
                className="flex gap-3 rounded-xl bg-surface p-3 shadow-[var(--shadow-border)]"
              >
                <DocThumb documentId={d.id} svg={d.thumbnailSvg} alt="" className="h-[88px] w-16 shrink-0 rounded-md" />
                <div className="min-w-0 flex-1">
                  <p className="truncate font-medium">{d.title}</p>
                  <p className="truncate text-xs text-muted">
                    {d.correspondentName ?? "Sans correspondant"} · {formatDate(d.createdDate)}
                  </p>
                  <div className="mt-2 flex flex-wrap gap-1">
                    {sortTagsForGrid(d.tags).slice(0, 3).map((t) => (
                      <TagChip key={t.id} tag={t} />
                    ))}
                  </div>
                </div>
              </Link>
            ))}
          </div>
        </div>
        <div className="grid gap-4 md:grid-cols-2">
          <div className="rounded-xl bg-surface p-4 shadow-[var(--shadow-border)]">
            <p className="text-[11px] font-medium tracking-[0.14em] text-muted uppercase">File d'OCR</p>
            <ul className="mt-3 space-y-2">
              {(data?.jobs ?? []).map((j) => (
                <li key={j.id} className="flex items-center gap-2 text-sm">
                  <span
                    className={
                      j.status === "success"
                        ? "size-1.5 rounded-full bg-ok"
                        : j.status === "failed"
                          ? "size-1.5 rounded-full bg-danger"
                          : "size-1.5 rounded-full bg-warn"
                    }
                  />
                  <span className="flex-1 truncate" title={j.filename}>
                    {j.filename}
                  </span>
                  <span className="text-xs text-faint">{ocrStatusLabel(j.status, j.message)}</span>
                </li>
              ))}
            </ul>
          </div>
          <div className="rounded-xl bg-surface p-4 shadow-[var(--shadow-border)]">
            <p className="text-[11px] font-medium tracking-[0.14em] text-muted uppercase">Vues</p>
            <ul className="mt-3 space-y-1">
              {(data?.views ?? []).map((v) => (
                <li key={v.id}>
                  <Link
                    to="/documents"
                    search={{ view: String(v.id) }}
                    className="block rounded-md px-2 py-2 text-sm hover:bg-surface-2"
                  >
                    {v.name}
                  </Link>
                </li>
              ))}
            </ul>
          </div>
        </div>
      </div>
    </AppShell>
  );
}
