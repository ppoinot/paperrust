import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { Inbox, Mail } from "lucide-react";
import { toast } from "sonner";
import { AppShell } from "@/components/layout/app-shell";
import { Button } from "@/components/ui/button";
import { EmptyState } from "@/components/ui/empty-state";
import { fetchCatalog, simulateMailFetch } from "@/lib/paperless/actions";

export const Route = createFileRoute("/mail")({ component: Page });

function Page() {
  const qc = useQueryClient();
  const navigate = useNavigate();
  const catalog = useQuery({ queryKey: ["catalog"], queryFn: () => fetchCatalog() });
  const fetchMail = useMutation({
    mutationFn: (ruleId: number) => simulateMailFetch({ data: { ruleId } }),
    onSuccess: (res) => {
      if (!res.ok) {
        toast.error(res.error);
        return;
      }
      qc.invalidateQueries();
      toast.success("Pièce jointe consommée");
      void navigate({ to: "/documents/$id", params: { id: String(res.documentId) } });
    },
  });

  const accounts = catalog.data?.mailAccounts ?? [];
  const rules = catalog.data?.mailRules ?? [];
  const empty = !catalog.isLoading && accounts.length === 0 && rules.length === 0;

  return (
    <AppShell title="Courrier">
      <p className="mb-6 max-w-2xl text-sm text-muted">
        Comptes IMAP et règles de consommation, comme Paperless. Ici, « Récupérer » simule l'arrivée d'une pièce jointe et la classe.
      </p>
      {empty ? (
        <EmptyState
          icon={Mail}
          title="Aucun compte ni règle courrier"
          description="Configurez un compte IMAP et des règles pour importer automatiquement les pièces jointes dans l'archive."
        />
      ) : (
        <>
          <h2 className="font-display text-xl">Comptes</h2>
          <div className="mt-3 space-y-3">
            {accounts.length === 0 ? (
              <p className="rounded-xl bg-surface px-4 py-6 text-center text-sm text-muted shadow-[var(--shadow-border)]">
                Aucun compte IMAP configuré.
              </p>
            ) : (
              accounts.map((a) => (
                <div key={a.id} className="rounded-xl bg-surface p-4 shadow-[var(--shadow-border)]">
                  <p className="font-medium">{a.name}</p>
                  <p className="text-sm text-muted">
                    {a.username} · {a.imapServer}:{a.imapPort}
                  </p>
                </div>
              ))
            )}
          </div>
          <h2 className="mt-8 font-display text-xl">Règles</h2>
          <div className="mt-3 space-y-3">
            {rules.length === 0 ? (
              <p className="rounded-xl bg-surface px-4 py-6 text-center text-sm text-muted shadow-[var(--shadow-border)]">
                Aucune règle de consommation.
              </p>
            ) : (
              rules.map((r) => (
                <div key={r.id} className="flex items-center gap-3 rounded-xl bg-surface p-4 shadow-[var(--shadow-border)]">
                  <div className="min-w-0 flex-1">
                    <p className="font-medium">{r.name}</p>
                    <p className="truncate text-sm text-muted">
                      {r.folder} · de {r.filterFrom || "n'importe qui"}
                      {r.filterSubject ? ` · sujet « ${r.filterSubject} »` : ""}
                    </p>
                  </div>
                  <Button
                    size="sm"
                    variant="outline"
                    disabled={fetchMail.isPending}
                    onClick={() => fetchMail.mutate(r.id)}
                  >
                    <Inbox className="size-4" />
                    Récupérer
                  </Button>
                </div>
              ))
            )}
          </div>
        </>
      )}
    </AppShell>
  );
}
