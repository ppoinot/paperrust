/** Register the pass-through service worker required for Chromium installability. */
export function registerServiceWorker() {
  if (typeof window === "undefined") return;
  if (!("serviceWorker" in navigator)) return;
  // Avoid registering under Vite/dev HMR noise; still fine on localhost preview.
  const ready = () => {
    navigator.serviceWorker.register("/sw.js", { scope: "/" }).catch((err) => {
      console.warn("[pwa] service worker registration failed", err);
    });
  };
  if (document.readyState === "complete") ready();
  else window.addEventListener("load", ready, { once: true });
}
