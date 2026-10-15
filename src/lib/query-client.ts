import { QueryClient } from "@tanstack/react-query";

let registered: QueryClient | null = null;

export function registerQueryClient(qc: QueryClient) {
  registered = qc;
}

export function clearDocumentQueries() {
  if (!registered) return;
  registered.removeQueries({ queryKey: ["documents"] });
  registered.removeQueries({ queryKey: ["document"] });
  registered.removeQueries({ queryKey: ["catalog"] });
  registered.removeQueries({ queryKey: ["dashboard"] });
  registered.removeQueries({ queryKey: ["session-identity"] });
}

export function makeQueryClient() {
  return new QueryClient({
    defaultOptions: {
      queries: {
        staleTime: 15_000,
        refetchOnWindowFocus: false,
        retry: (failureCount, error) => {
          if (error instanceof Error && error.name === "AuthRequired") return false;
          return failureCount < 2;
        },
      },
    },
  });
}
