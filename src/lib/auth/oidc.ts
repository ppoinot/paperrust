/**
 * OIDC login is the backend, not the SPA.
 * Same-origin GET /api/auth/login; logout is POST-only (CSRF).
 */
import { clearDocumentQueries } from "@/lib/query-client";

const LOGIN_API = "/api/auth/login";
const LOGOUT_API = "/api/auth/logout";

export function startOidcLogin(): void {
  window.location.assign(LOGIN_API);
}

export function logoutOidc(): void {
  clearDocumentQueries();
  const form = document.createElement("form");
  form.method = "POST";
  form.action = LOGOUT_API;
  form.style.display = "none";
  document.body.appendChild(form);
  form.submit();
}
