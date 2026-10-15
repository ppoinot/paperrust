import { QueryClientProvider } from "@tanstack/react-query";
import { createRootRoute, HeadContent, Outlet, Scripts } from "@tanstack/react-router";
import { useEffect, useState } from "react";
import { Toaster } from "sonner";
import { TooltipProvider } from "@/components/ui/tooltip";
import { makeQueryClient, registerQueryClient } from "@/lib/query-client";
import { registerServiceWorker } from "@/lib/register-sw";
import appCss from "../styles.css?url";

const APP_NAME = "Paperrust";

export const Route = createRootRoute({
  head: () => ({
    meta: [
      { charSet: "utf-8" },
      { name: "viewport", content: "width=device-width, initial-scale=1" },
      { title: APP_NAME },
      { property: "og:title", content: APP_NAME },
      {
        name: "description",
        content: "Archive documentaire moderne — OCR par LLM, classement automatique.",
      },
      { name: "theme-color", content: "#161815" },
      { name: "mobile-web-app-capable", content: "yes" },
      { name: "apple-mobile-web-app-capable", content: "yes" },
      { name: "apple-mobile-web-app-title", content: APP_NAME },
      { name: "apple-mobile-web-app-status-bar-style", content: "black-translucent" },
    ],
    links: [
      { rel: "icon", type: "image/svg+xml", href: "/favicon.svg" },
      { rel: "manifest", href: "/manifest.webmanifest" },
      { rel: "apple-touch-icon", href: "/icons/icon-180.png" },
      { rel: "stylesheet", href: appCss },
    ],
  }),
  component: Root,
});

function Root() {
  const [queryClient] = useState(() => {
    const qc = makeQueryClient();
    registerQueryClient(qc);
    return qc;
  });
  useEffect(() => {
    registerServiceWorker();
  }, []);
  return (
    <html lang="fr" style={{ background: "#f3efe6", color: "#1c1915" }} suppressHydrationWarning>
      <head>
        <HeadContent />
      </head>
      <body className="antialiased bg-bg text-fg" style={{ background: "#f3efe6", color: "#1c1915", minHeight: "100%" }}>
          <QueryClientProvider client={queryClient}>
            <TooltipProvider>
              <Outlet />
              <Toaster position="bottom-right" richColors closeButton />
            </TooltipProvider>
          </QueryClientProvider>
        <Scripts />
      </body>
    </html>
  );
}
