import { createFileRoute } from "@tanstack/react-router";
import { useEffect } from "react";
import { BrandMark } from "@/components/layout/brand-mark";
import { startOidcLogin } from "@/lib/auth/oidc";

export const Route = createFileRoute("/login/")({
  component: LoginPage,
});

function LoginPage() {
  useEffect(() => {
    startOidcLogin();
  }, []);

  return (
    <div className="flex min-h-dvh flex-col items-center justify-center bg-bg px-4 text-fg">
      <div className="w-full max-w-sm rounded-xl bg-surface px-6 py-8 text-center shadow-[var(--shadow-border)]">
        <div className="mx-auto flex size-12 items-center justify-center rounded-lg bg-sidebar text-sidebar-fg">
          <BrandMark className="size-8" />
        </div>
        <h1 className="mt-4 font-display text-2xl">Paperrust</h1>
        <p className="mt-2 text-sm text-muted">Connexion OpenID Connect</p>
        <button
          type="button"
          className="mt-6 text-sm text-primary hover:underline"
          onClick={() => startOidcLogin()}
        >
          Continuer avec OpenID Connect
        </button>
      </div>
    </div>
  );
}
