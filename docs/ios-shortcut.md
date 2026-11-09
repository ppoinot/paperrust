# iOS Shortcut → Paperrust ingest

Personal API tokens let an iPhone Shortcut upload a photo or scan without a
browser session. Scope for this shortcut: **ingest only**. A read token
cannot upload. See `docs/bot-access.md` for a client that may read documents.

## Token format

- Secret: `pr_` + URL-safe random (shown **once** in Réglages → Tokens API)
- Stored server-side as SHA-256 hex only (`api_tokens.token_hash`)
- Display prefix e.g. `pr_AbCdEfGh…` for identification after creation

Create, list, and revoke require a signed-in session (the OIDC cookie), from
Réglages or:

```http
POST /api/settings/api-tokens
Cookie: paperrust_session=…
Content-Type: application/json

{"name":"iPhone"}
```

Response (secret once):

```json
{"ok":true,"id":1,"name":"iPhone","prefix":"pr_AbCdEfGh","token":"pr_…","createdAt":"…"}
```

## Ingest endpoint

```http
POST https://archive.example.com/api/v1/ingest
Authorization: Bearer pr_…
Content-Type: multipart/form-data

file=<JPEG|PNG|PDF>   # field name: file (aliases: document, upload)
```

`https://archive.example.com` is a placeholder for an origin this archive
answers on. Optional multipart fields (same as consume): `filename`,
`provider`, `skipOcr`.

Success:

```json
{"ok":true,"documentId":42,"usedLlm":true,"error":null}
```

Errors (French, short):

| HTTP | Meaning |
|------|---------|
| 401 | Token manquant / invalide / révoqué |
| 400 | Fichier manquant ou rejeté |
| 413 | Fichier trop volumineux (max 100 Mo) |
| 429 | Trop de requêtes (≈30/min par token) |

The 413 limit is 100 MiB. The 429 limit is about 30 requests per minute per
token.

## curl (Shortcut-ready)

```bash
curl -sS -X POST 'https://archive.example.com/api/v1/ingest' \
  -H "Authorization: Bearer pr_YOUR_TOKEN" \
  -F 'file=@scan.jpg;type=image/jpeg'
```

## iOS Shortcut outline

1. **Take Photo** / **Scan Document** / **Get Images from Input**
2. **Get Contents of URL**
   - Method: POST
   - URL: `https://archive.example.com/api/v1/ingest`
   - Headers: `Authorization` = `Bearer pr_…`
   - Request body: Form → file = Shortcut Input (image)
3. Show a notification with `documentId` from the JSON

## Reverse proxy

If a reverse proxy authenticates the whole origin, a bare Shortcut call to
`POST /api/v1/ingest` may receive a login page instead of JSON.

Exempt `POST /api/v1/ingest` only, so the proxy does not challenge that
path. Application auth remains the Paperrust Bearer token. Session-cookie
routes stay behind the proxy and OIDC.

If the proxy cannot exempt that one path, send the proxy's own service
headers on the Shortcut request in addition to `Authorization: Bearer pr_…`.
