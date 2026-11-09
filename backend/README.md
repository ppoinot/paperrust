# Paperrust API

Rust service (`paperrust-api`). Build with `cargo build --release` from this directory.

## Tests

```bash
cargo test --locked
```

`DATABASE_URL` is required to run the server and is not committed. Tests that do not open Postgres do not need it.

## Listen address

`PAPERRUST_BIND` defaults to `127.0.0.1:8788`. Any other explicit IPv4 is an operator choice. `0.0.0.0` is refused unless `PAPERRUST_BIND_ALLOW_UNSPECIFIED=1` or `PAPERRUST_BIND_K8S=1`. Pure IPv6 is refused.

## Public origin

`PAPERRUST_PUBLIC_URL` is the public origin, with no trailing slash. Example: `https://paperrust.example.com`.

OAuth discovery uses that value as the issuer and `{origin}/api/mcp` as the protected resource. If it is unset, the first `https` origin in `PAPERRUST_PUBLIC_URLS` is used. If neither is set, the metadata routes and the `WWW-Authenticate` challenge return 404. They do not invent a host.

`X-Forwarded-Host` is honored only from the TCP peer `127.0.0.1`, and only when the forwarded host is already a configured public or LAN origin. Other peers cannot select an origin with that header.

## MCP

`POST /api/mcp` is 404 unless both variables are non-empty and the API token starts with `pr_`:

| Variable | Role |
| --- | --- |
| `PAPERRUST_MCP_BEARER` | Value the connector sends as `Authorization: Bearer`. Compared as a SHA-256 digest. A presented `pr_…` token is never accepted as this bearer. |
| `PAPERRUST_MCP_API_TOKEN` | Server-side `pr_…` token. Tools run as its owner and scopes. Never read from the caller and never returned. When set, startup grants that token the `ingest` scope if it is missing. |

OAuth is public clients only. A non-empty `client_secret` is rejected. Allowed redirects are the `https://grok.com/connectors-oauth-exchange-code` URLs (with or without a trailing slash), `https://www.cursor.com/agents/mcp/oauth/callback`, `cursor://anysphere.cursor-mcp/oauth/callback`, and HTTP loopback `/callback` or `/oauth/callback` on `127.0.0.1`, `localhost`, and `[::1]`.

Other origins, CORS, OIDC issuers, and the Stirling editor come from environment variables. Leave them unset to keep those features off. `PAPERRUST_DEV=1` widens loopback CORS and accepts a client-supplied OCR key. Do not set it in production.
