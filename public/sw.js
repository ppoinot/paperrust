/* Paperrust installability SW (GH#2).
 * no network interception, no cache storage, no imported scripts.
 * The browser handles every request (API, /api/auth/* OIDC, documents) as if
 * there were no service worker.
 */
self.addEventListener("install", () => {
  self.skipWaiting();
});

self.addEventListener("activate", (event) => {
  event.waitUntil(self.clients.claim());
});
