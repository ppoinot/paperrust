import { useQuery } from "@tanstack/react-query";
import { createFileRoute, Link } from "@tanstack/react-router";
import { ScrollText } from "lucide-react";
import { AppShell } from "@/components/layout/app-shell";
import { EmptyState } from "@/components/ui/empty-state";
import { fetchJobs } from "@/lib/paperless/actions";
import { formatDateTime, ocrStatusLabel } from "@/lib/paperless/format";

export const Route = createFileRoute("/logs")({ component: Page });

function Page() {
  const jobs = useQuery({ queryKey: ["jobs"], queryFn: () => fetchJobs() });
  const items = jobs.data ?? [];

  return (
    <AppShell title="Journal de consommation">
      {items.length === 0 && !jobs.isLoading ? (
        <EmptyState
          icon={ScrollText}
          title="Aucune consommation récente"
          description="Les imports et tentatives d'OCR apparaîtront ici."
        />
      ) : (
        <div className="overflow-hidden rounded-xl bg-surface shadow-[var(--shadow-border)]">
          <table className="w-full text-sm">
            <thead className="text-[11px] tracking-wide text-muted uppercase">
              <tr className="border-b border-border">
                <th className="px-4 py-2 text-left">Fichier</th>
                <th className="px-4 py-2 text-left">Statut</th>
                <th className="px-4 py-2 text-left">Message</th>
                <th className="px-4 py-2 text-left">Quand</th>
              </tr>
            </thead>
            <tbody>
              {items.map((j) => (
                <tr key={j.id} className="border-b border-border/70 last:border-0">
                  <td className="px-4 py-2">
                    {j.document_id ? (
                      <Link
                        to="/documents/$id"
                        params={{ id: String(j.document_id) }}
                        className="font-medium hover:underline"
                      >
                        {j.filename}
                      </Link>
                    ) : (
                      j.filename
                    )}
                  </td>
                  <td className="px-4 py-2">{ocrStatusLabel(j.status, j.message)}</td>
                  <td className="px-4 py-2 text-muted">{j.message}</td>
                  <td className="px-4 py-2 text-muted">{formatDateTime(j.created_at)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </AppShell>
  );
}
