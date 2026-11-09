# Bot read access

A machine client reads one user's archive with its own `pr_…` secret. That
secret is not a session and not an ingest token.

## What you create

In **Réglages → Tokens API**, choose **Lecture (bot)** and create a token.
The raw `pr_…` value is shown once. The server stores only its SHA-256.
Revoke it from the same screen. Existing phone and scanner tokens stay
`ingest` and cannot read.

The token sees only the documents of the account that created it. It cannot
upload, edit metadata, OCR, delete, change settings, manage tokens, or call
mail / workflow routes. A read token presented to an ingest route gets
`403 insufficient_scope`. An ingest token presented to a read route gets the
same.

## Calls

```
Authorization: Bearer pr_…
```

`Token pr_…` and Basic (the `pr_…` as the user or the password) also work.
Do not send the browser session cookie with this header: a session wins and
is a different credential.

| Call | What it returns |
|------|-----------------|
| `GET /api/me` | email, name, `sub` of the owner |
| `GET /api/catalog` | that user's tags, correspondents, types |
| `GET /api/search?q=` | up to 12 matches, content excerpt, no thumbnail |
| `POST /api/documents/query` | one page of documents |
| `GET /api/documents/{id}` | full OCR `content`, notes, no embedded file |
| `GET /api/documents/{id}/file` | original bytes |
| `GET /api/documents/{id}/preview` | page JPEG |
| `GET /api/tags` `/api/correspondents` `/api/types` `/api/storage-paths` `/api/fields` | taxonomy |

`POST /api/documents/query` body (all fields optional):

```json
{ "q": "facture", "limit": 50, "offset": 0, "includeThumbnails": false }
```

A read token that omits `limit` gets 50. `limit` is clamped to 1..100.
`includeThumbnails` defaults to false for this token, so the JSON does not
carry SVG thumbnails. List `content` is the first 480 characters. The full
text is on `GET /api/documents/{id}`.

Trash is excluded unless `"trashOnly": true`.

Other people's documents are not in these responses. A missing or foreign id
is `404`.

## Limits

120 requests per minute per token (`429 rate_limited`). Creation of tokens
still requires a logged-in session. Do not put the secret in git, in a prompt
that gets logged, or in this repo. Store it next to the client the same way
as any other API secret, and revoke it when the client no longer needs it.

A `documents:read` token may also call `GET /api/documents/{id}/text` (an OCR
window: `offset`, `maxChars` clamped to 1..12000, `truncated`, `nextOffset`)
and `GET /api/documents/{id}/pages/{n}/text` (one PDF page via pdfium, or the
OCR window for page 1 of a non-PDF). It still cannot `POST …/meta`,
`GET /api/jobs`, or `GET /api/diagnostics`.

## Extra scopes

Create these only on a new token. An existing ingest token does not gain them.

| Scope | Allows |
|-------|--------|
| `documents:meta` | `POST /api/documents/{id}/meta` for title, correspondent, type, tags, date, storage path, and custom fields. A `content` field on this token is `403 content_not_writable`. No delete, restore, notes, OCR, mail, or settings. |
| `diagnostics:read` | `GET /api/jobs` (last 50 consume/OCR rows for this owner) and `GET /api/diagnostics` (in-process log ring, warn+error by default). |

Rates, per token, per minute: read 120, meta 30, diagnostics 60. A session
that calls diagnostics is limited on its own key and is not double-counted
with a token.

## MCP connector

Public Streamable HTTP, one JSON-RPC response per POST, no SSE:

```
https://archive.example.com/api/mcp
```

`https://archive.example.com` is a placeholder. The issuer is not compiled
into the server. It is `PAPERRUST_PUBLIC_URL` when that variable is set,
otherwise the first `https` origin in `PAPERRUST_PUBLIC_URLS`. The example
above is that origin. The protected-resource identifier is
`{origin}/api/mcp`. If neither variable is set, the OAuth discovery routes
respond `404`.

The connector form is OAuth 2.0 authorization-code with PKCE S256. Paperrust
is a public client: client id `paperrust`, client secret empty, token
endpoint authentication method **none**. A non-empty `client_secret` is
refused (`invalid_client`).

| Form field | Value |
|------------|--------|
| MCP server URL | `https://archive.example.com/api/mcp` |
| Client ID | `paperrust` |
| Client secret | empty |
| Authorization endpoint | `https://archive.example.com/oauth/authorize` |
| Token endpoint | `https://archive.example.com/oauth/token` |
| Scope | `mcp` |
| Token auth method | none |

Replace `https://archive.example.com` with the issuer origin from the rule
above. Authorization is `{origin}/oauth/authorize`. The token endpoint is
`{origin}/oauth/token`.

Accepted redirect URIs, exact match only:

- `https://grok.com/connectors-oauth-exchange-code/`
- `https://grok.com/connectors-oauth-exchange-code`
- `https://www.cursor.com/agents/mcp/oauth/callback`
- `cursor://anysphere.cursor-mcp/oauth/callback`

Also allowed: `http://127.0.0.1`, `http://localhost`, or `http://[::1]`, with
a non-zero port, on `/callback` or `/callback/` (and the same paths under
`/oauth`, so `/oauth/callback` and `/oauth/callback/`). `https` on those
hosts is refused. A query string, a fragment, or any other redirect is
refused.

Sign in as the archive owner before consent. Any other account gets `403`.
The browser consent page is French (`Accès refusé` /
`Ce connecteur est réservé au propriétaire de l'archive.`).

The access token the connector stores is not the `pr_…`. Set both
`PAPERRUST_MCP_BEARER` and `PAPERRUST_MCP_API_TOKEN`. The bearer is the
access token the connector sends (`expires_in` 3600) plus a rotating refresh
token. It must not be a `pr_…`. The process alone reads
`PAPERRUST_MCP_API_TOKEN`, the `pr_…` created in Réglages (preset
**Connecteur MCP**: `ingest`, `documents:read`, `documents:meta`,
`diagnostics:read`). The connector never holds the `pr_…`. If either
variable is empty, or the API token does not start with `pr_`, `/api/mcp`
and `/oauth/authorize` return `404`.
Revoking that token stops the tools even if a bearer is still valid.
Reusing a refresh token revokes the refresh family; it does not rotate the
bearer. Rotate `PAPERRUST_MCP_BEARER` to cut connectors that already stored
an access token. A `pr_…` presented to `/api/mcp` is `401`. The bearer
presented to the rest of the API is `401`. A session cookie is not enough
for `/api/mcp`.

## Tools

French descriptions, English names. Text tools stay under about 14 KB.
`get_file` returns the original bytes as base64, at most 262144 bytes per
call.

| Tool | Scope | What it returns |
|------|-------|-----------------|
| `whoami` | any | owner email and scopes, never the secret |
| `search_documents` | `documents:read` | hits with a short excerpt, no thumbnail, no file |
| `get_document` | `documents:read` | metadata plus an OCR window (`offset`, `maxChars`) |
| `get_page_text` | `documents:read` | one page of PDF text, or OCR for a non-PDF page 1 |
| `get_file` | `documents:read` | original file bytes, base64, windowed by `offset` and `maxBytes` (max 262144) |
| `upload_file` | `ingest` | stores a PDF, PNG, or JPEG (`filename`, base64 `data`). Max 8 MiB. Returns `documentId` |
| `set_labels` | `documents:meta` | sets title, correspondent, type, tags, date, storage path, custom fields by name |
| `consumption_journal` | `diagnostics:read` | last consume/OCR jobs, scrubbed messages |
| `diagnostics` | `diagnostics:read` | redacted process log (`level` `warn` or `info`) |

`set_labels` does not write OCR content, ASN, or delete. `createMissing`
defaults to false: an unknown tag, correspondent, or type fails with
`unknown_name`. `true` inserts only those three. A storage path or a custom
field must already exist. Names are resolved before one metadata write.

The diagnostic ring keeps 300 info/warn/error lines, 400 characters each,
after redaction (`pr_…`, model keys, `postgres://`, `Authorization`). Events
tagged with another owner are dropped. It is not pod logs, not a secret
manager, and not the process environment.

## Refused

Refused: delete, restore, purge, notes, OCR triggers, mail, workflows,
settings, token management, and any tool outside the token's scopes
(`isError`, HTTP 200). SVG originals are `file_unavailable`. `initialize`,
`ping`, and `tools/list` share the read budget and may return HTTP 429.
