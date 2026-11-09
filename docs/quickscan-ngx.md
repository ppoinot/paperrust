# Quickscan / Paperless-ngx → Paperrust (ingest only)

Paperrust exposes a minimal Paperless-ngx-compatible upload so Quickscan
(iOS) and similar clients can send scans without implementing the full ngx
CRUD API.

Use an origin this archive answers on. `https://archive.example.com` below
is a placeholder, not a default host you should leave in a client.

## What is supported

| Item | Value |
|------|--------|
| Endpoint | `POST /api/documents/post_document/` (trailing slash optional) |
| Auth | `Authorization: Token pr_…` (also `Bearer pr_…`, or Basic with `pr_…` as the user or the password) |
| Multipart | field **`document`** (file); optional `title`, `created` (`YYYY-MM-DD`) |
| Success | HTTP **200** and a JSON string (task id, ngx-shaped). The document is stored synchronously. The real id is response header `X-Document-Id`. |
| Not implemented | Full ngx REST (documents list/detail, tags CRUD, `/api/tasks/…`, and the rest) |

Existing machine ingest remains: `POST /api/v1/ingest` with
`Authorization: Bearer pr_…`. See `docs/ios-shortcut.md`.

## Quickscan

1. Open Paperrust → **Réglages → Tokens API** and create a token (for example « Quickscan »). Scope is ingest only.
2. Copy the secret `pr_…` (shown once).
3. In Quickscan, add a **Paperless-ngx** destination:
   - **URL de base**: `https://archive.example.com` (the HTTPS origin only, **without** `/api/…`)
   - **Token**: paste `pr_…` as-is
4. Scan or send. The client calls `…/api/documents/post_document/`.

## curl

```bash
BASE="${PAPERRUST_BASE:-https://archive.example.com}"

# Token auth (ngx style)
curl -sS -D- -o /tmp/ngx-body.txt -X POST "$BASE/api/documents/post_document/" \
  -H "Authorization: Token pr_YOUR_TOKEN" \
  -F "document=@./scan.pdf;type=application/pdf" \
  -F "title=Scan Quickscan"

# Expect: HTTP 200, body a JSON string task id, header X-Document-Id: <int>
cat /tmp/ngx-body.txt; echo

# Bearer still works on the same endpoint
curl -sS -o /dev/null -w "%{http_code}\n" -X POST "$BASE/api/documents/post_document/" \
  -H "Authorization: Bearer pr_YOUR_TOKEN" \
  -F "document=@./scan.jpg;type=image/jpeg"

# Legacy Paperrust ingest unchanged
curl -sS -X POST "$BASE/api/v1/ingest" \
  -H "Authorization: Bearer pr_YOUR_TOKEN" \
  -F "file=@./scan.jpg;type=image/jpeg"
```

Do not set `PAPERRUST_BASE` to a host you do not operate. The examples verify
TLS. They do not pass `-k`.

Optional local smoke: `scripts/smoke-ngx-post-document.sh`. It refuses to run
when `PAPERRUST_ENV=production` and when the database name is not disposable.
Its default base is `http://127.0.0.1:8788`. Pass `PAPERRUST_API_TOKEN` in
the environment. Do not commit the token.

## Security

- **Scope**: the token is ingest-only on these two routes. No session, no settings, no delete via this token.
- **Rate limit**: about 30 requests per minute per token (`429` when exceeded).
- **Auth required**: unauthenticated requests get `401`. There is no anonymous upload.
- Prefer a dedicated token named for the device. Revoke it in Réglages if it is lost.
