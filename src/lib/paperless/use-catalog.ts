import { useQuery } from "@tanstack/react-query";
import { fetchCatalog } from "@/lib/paperless/actions";

/** Shared catalog query — AppShell + document fiche subscribe to the same key. */
export function useCatalog() {
  return useQuery({
    queryKey: ["catalog"],
    queryFn: () => fetchCatalog(),
    staleTime: 60_000,
  });
}
