/** Thrown when a document/API call returns 401. Never treat the body as docs. */
export class AuthRequired extends Error {
  readonly status = 401 as const;
  constructor(message = "auth_required") {
    super(message);
    this.name = "AuthRequired";
  }
}

export function isAuthRequired(err: unknown): boolean {
  return (
    err instanceof AuthRequired ||
    (err instanceof Error && err.name === "AuthRequired")
  );
}
