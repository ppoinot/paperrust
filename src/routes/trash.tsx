import { createFileRoute } from "@tanstack/react-router";
import { DocumentList } from "@/components/documents/document-list";
import { AppShell } from "@/components/layout/app-shell";

export const Route = createFileRoute("/trash")({ component: TrashPage });

function TrashPage() {
  return (
    <AppShell title="Corbeille">
      <p className="mb-4 max-w-2xl text-sm text-muted">
        Documents supprimés. Restaurez-les ou supprimez-les définitivement. Les fichiers d’origine sont
        conservés jusqu’à la suppression définitive.
      </p>
      <DocumentList initialFilter={{ trashOnly: true, sort: "modified_at", sortReverse: true }} />
    </AppShell>
  );
}
