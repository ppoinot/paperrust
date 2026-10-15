import { useQuery } from "@tanstack/react-query";
import { createFileRoute } from "@tanstack/react-router";
import { DocumentList } from "@/components/documents/document-list";
import { AppShell } from "@/components/layout/app-shell";
import { fetchCatalog } from "@/lib/paperless/actions";
import type { DocumentFilter } from "@/lib/paperless/types";

export type DocumentsSearch = {
  view?: string;
  inbox?: boolean;
  correspondent?: string;
};

export const Route = createFileRoute("/documents/")({
  validateSearch: (search: Record<string, unknown>): DocumentsSearch => {
    const next: DocumentsSearch = {};
    // TanStack JSON-parses ?view=2 as a number — coerce so saved views match.
    if (search.view !== undefined && search.view !== null && String(search.view).trim()) {
      next.view = String(search.view);
    }
    if (search.inbox === "1" || search.inbox === true || search.inbox === 1) next.inbox = true;
    if (
      search.correspondent !== undefined &&
      search.correspondent !== null &&
      String(search.correspondent).trim()
    ) {
      next.correspondent = String(search.correspondent);
    }
    return next;
  },
  component: DocumentsPage,
});

function DocumentsPage() {
  const { view, inbox, correspondent } = Route.useSearch();
  const catalog = useQuery({ queryKey: ["catalog"], queryFn: () => fetchCatalog() });
  const saved = catalog.data?.savedViews.find((v) => String(v.id) === view);
  const correspondentId = correspondent ? Number(correspondent) : NaN;
  const hasCorrespondent = Number.isFinite(correspondentId) && correspondentId > 0;

  const initialFilter: DocumentFilter = saved
    ? {
        ...saved.filter,
        sort: saved.sortField,
        sortReverse: saved.sortReverse,
      }
    : {
        inboxOnly: inbox || undefined,
        correspondentId: hasCorrespondent ? correspondentId : undefined,
        sort: "created_date",
        sortReverse: true,
      };

  const listKey =
    view ?? (inbox ? "inbox" : hasCorrespondent ? `c-${correspondentId}` : "all");

  return (
    <AppShell title="Documents">
      {view && catalog.isLoading ? (
        <p className="text-sm text-muted">Chargement…</p>
      ) : (
        <DocumentList key={listKey} initialFilter={initialFilter} />
      )}
    </AppShell>
  );
}
