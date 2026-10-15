# Install

Paperrust runs as one application process in front of PostgreSQL. Docker Compose is the supported way to start both.

## Requirements

- Docker Engine with the Compose plugin
- A public HTTPS origin you control, if browsers outside the machine will sign in
- An OpenID Connect identity provider for interactive sign-in

Serve Paperrust behind HTTPS and an identity provider. Do not disable OpenID Connect for testing on a shared or exposed host.

## Configure

Configuration is environment variables. The names and comments live in [`.env.example`](../.env.example) at the repository root.

```bash
cp .env.example .env
```

Edit `.env` before you start the stack. Compose reads that file for variable substitution and for the application environment. Do not commit `.env`.

Generate every secret yourself. Do not reuse example passwords or paste a shared sample value into `.env`. For each secret, prefer:

```bash
openssl rand -hex 32
```

Set at least:

| Variable | Role |
| --- | --- |
| `DATABASE_URL` | PostgreSQL connection string for the archive. Use a strong password you generate; do not reuse a sample value |
| `SESSION_SECRET` | Signs the session cookie. Set to a unique value such as `<generate: openssl rand -hex 32>` (at least 32 bytes of randomness) |
| `PAPERRUST_OIDC_ISSUER` | Issuer URL of the OpenID Connect provider |
| `PAPERRUST_OIDC_CLIENT_ID` | Client id registered at that provider |
| `PAPERRUST_OIDC_CLIENT_SECRET` | Client secret from the provider; generate or copy from the provider, never from a sample |
| `PAPERRUST_PUBLIC_URLS` | Comma-separated public HTTPS origins of this archive, with scheme. By default only a loopback peer (reverse proxy or tunnel on 127.0.0.1) may use them |
| `PAPERRUST_LAN_URLS` | Origins from `PAPERRUST_PUBLIC_URLS` that any peer may use, for example behind a reverse proxy that is not on loopback. Restrict access to the port when you set it |
| `PAPERRUST_BIND` | Listen address. The example uses loopback |
| `PAPERRUST_ENV` | `production` for a real install |

`PAPERRUST_CORS_ORIGINS` is the exact list of browser origins allowed to call the API. Leave it empty when the browser uses the same origin as the API.

Optional variables in `.env.example` cover a custom CA bundle, a first user, Stirling-PDF, Paperless-ngx, self-hosted OCR, and language-model credentials. An empty URL leaves that integration off.

`PAPERRUST_DEV=1` widens local behavior. Do not set it when `PAPERRUST_ENV=production`.

Private Paperless-ngx or OCR hosts are refused unless the exact host is listed in `PAPERRUST_PAPERLESS_HOST_ALLOWLIST` or `PAPERRUST_OCR_SELFHOSTED_HOST_ALLOWLIST`. Public HTTPS Paperless-ngx instances do not need the Paperless allowlist.

## Start

From the repository root:

```bash
docker compose -f docker-compose.example.yml up --build
```

The example Compose file builds Paperrust from this tree and starts PostgreSQL. The first build compiles the API and the web interface.

## Check

When the containers are running, request `GET /health` on the host port published by the Compose file. A healthy process returns HTTP 200. The web interface is served by the same process.

Open the HTTPS origin you set in `PAPERRUST_PUBLIC_URLS` and sign in through the OpenID Connect provider.

## Secrets

Keep tokens, `DATABASE_URL`, and cookies out of git, issues, and chat. See [SECURITY.md](../SECURITY.md).
