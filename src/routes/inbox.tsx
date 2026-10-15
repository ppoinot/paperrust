import { createFileRoute } from "@tanstack/react-router";
import { DocumentList } from "@/components/documents/document-list";
import { AppShell } from "@/components/layout/app-shell";

export const Route = createFileRoute("/inbox")({ component: InboxPage });

function InboxPage() {
  return (
    <AppShell title="Boîte de réception">
      <p className="mb-4 max-w-2xl text-sm text-muted">
        Documents encore marqués Inbox. Retirez l'étiquette une fois le classement vérifié — c'est le flux Paperless.
      </p>
      <DocumentList initialFilter={{ inboxOnly: true, sort: "added_at", sortReverse: true }} />
    </AppShell>
  );
}
