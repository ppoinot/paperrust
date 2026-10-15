import { useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { useCallback } from "react";
import { toast } from "sonner";
import { deleteView } from "@/lib/paperless/actions";

/** Confirm + DELETE /api/views/:id (owner-scoped) + clear ?view= if active. */
export function useDeleteSavedView() {
  const qc = useQueryClient();
  const navigate = useNavigate();

  return useCallback(
    async (view: { id: number; name: string }): Promise<boolean> => {
      if (!confirm(`Supprimer la vue « ${view.name} » ?`)) return false;
      try {
        await deleteView({ data: { id: view.id } });
        await qc.invalidateQueries({ queryKey: ["catalog"] });
        toast.success("Vue supprimée");
        const active = new URLSearchParams(window.location.search).get("view");
        if (active === String(view.id)) {
          void navigate({ to: "/documents", search: {} });
        }
        return true;
      } catch {
        toast.error("Impossible de supprimer la vue");
        return false;
      }
    },
    [navigate, qc],
  );
}
