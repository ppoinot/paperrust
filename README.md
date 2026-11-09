# Paperrust

Paperrust is a self-hosted document archive. You upload originals, read them in the browser, and keep correspondents, tags, types, and custom fields on each document. A Rust API stores the archive in PostgreSQL and serves the web interface from the same process.

The interface is in French.

## Features

The browser app can be installed (web app manifest, service worker that does not cache requests).

### Archive

- Dashboard with upload, counts, and monthly intake
- Document library, full-text search, inbox, and trash with a retention delay
- In-browser PDF viewing with a selectable text layer, and server-side page previews
- Notes on each document
- Correspondents, tags, document types, custom fields, and storage paths
- Saved views, and workflows that apply tags from document content
- Activity log

### Upload and text recognition

- Drop many files. They queue and upload one at a time, under a 100 MB request cap
- SHA-256 duplicate skip. A file already in the archive is not stored again. A copy that only exists in the trash does not count
- Optical character recognition through a language-model provider you configure

### Edit and bring documents in

- Optional hand-off to Stirling-PDF. The editor tab opens on the editor origin only
- Optional import from a Paperless-ngx instance
- Paperless-ngx compatible upload, `POST /api/documents/post_document/` ([docs/quickscan-ngx.md](docs/quickscan-ngx.md))
- iOS Shortcut ingest, `POST /api/v1/ingest` ([docs/ios-shortcut.md](docs/ios-shortcut.md))

### Sign-in and API tokens

People sign in with OpenID Connect. Machines use a `pr_…` token, shown once and stored as a SHA-256. Scopes do not imply each other:

- `ingest` deposits a file
- `documents:read` searches and reads that account's documents, including the original file
- `documents:meta` changes the title, correspondent, type, tags, date, storage path, and custom fields
- `diagnostics:read` reads recent consume jobs and a redacted process log

Calls, page size, and rate limits are in [docs/bot-access.md](docs/bot-access.md).

### MCP connector

Optional. A public OAuth client (PKCE, empty client secret) at `{origin}/api/mcp`. Set `PAPERRUST_PUBLIC_URL`, or the first `https` origin in `PAPERRUST_PUBLIC_URLS`, and set both `PAPERRUST_MCP_BEARER` and `PAPERRUST_MCP_API_TOKEN`. Discovery responds 404 until an origin is set.

Tools follow the token's scopes: `whoami`, `search_documents`, `get_document`, `get_page_text`, `get_file` (original bytes, base64, windows of at most 256 KiB), `upload_file` (PDF, PNG, or JPEG, at most 8 MiB), `set_labels`, `consumption_journal`, and `diagnostics`.

Allowed redirects are the Grok and Cursor connector callbacks, and HTTP loopback `/callback`. The setup table is in [docs/bot-access.md](docs/bot-access.md).

## Install

Docker Compose is the supported install. It builds Paperrust from this repository and starts PostgreSQL.

```bash
cp .env.example .env
docker compose -f docker-compose.example.yml up --build
```

Edit `.env` before the first start. Do not commit it. Generate every secret with a command such as `openssl rand -hex 32`; do not reuse sample passwords. When the containers are up, `GET /health` on the published application port returns HTTP 200. The example file publishes that port on the loopback address.

Serve the archive over **HTTPS** behind an **OpenID Connect** identity provider. Do not disable OpenID Connect for testing on a shared or exposed host.

Step-by-step notes, including what to set before sign-in works, are in [docs/install.md](docs/install.md).

## Configuration

Paperrust reads configuration from environment variables. Names, comments, and which values are required are in [`.env.example`](.env.example). Copy that file to `.env` for Compose, or export the same names in the process environment.

The groups you will set:

- **Database.** `DATABASE_URL` (use a generated password, not a sample)
- **HTTP.** `PAPERRUST_BIND`, `PAPERRUST_ENV`, `PAPERRUST_PUBLIC_URLS` (HTTPS origins), `PAPERRUST_CORS_ORIGINS`
- **Sign-in.** `SESSION_SECRET` set to `<generate: openssl rand -hex 32>`, plus `PAPERRUST_OIDC_ISSUER`, `PAPERRUST_OIDC_CLIENT_ID`, `PAPERRUST_OIDC_CLIENT_SECRET`
- **Optional integrations.** Stirling-PDF URLs, a Paperless-ngx host allowlist, a self-hosted OCR host allowlist, and provider credentials for OCR

Leave an optional URL empty to keep that integration off. Do not put secrets in the Compose file; keep them in `.env` or a secret store.

`PAPERRUST_DEV=1` is a local-only switch. Do not set it when `PAPERRUST_ENV=production`.

## pgrust (alternative to PostgreSQL)

[pgrust](https://github.com/malisper/pgrust) is a PostgreSQL-compatible database server written in Rust. You can use it in place of PostgreSQL for local development and testing. The supported install above still uses PostgreSQL.

`scripts/start-pgrust.sh` starts a local pgrust server:

```bash
scripts/start-pgrust.sh
```

The script:

- needs the PostgreSQL 18 tools (`initdb`, `psql`) installed
- downloads the official pgrust v0.3 Linux x86_64 binary to `bin/pgrust` and checks its SHA-256 against a pinned value
- creates a data directory in `pgrust-data/` on first run
- starts pgrust on `127.0.0.1`, port `5433` by default (set `PGRUST_PORT` to change it), with its log in `pgrust.log`
- checks that the server reports pgrust 0.3, creates the `paperrust` database if needed, and applies `migrations/0002_paperrust.sql`
- prints the connection string to use:

```
DATABASE_URL=postgres://postgres@127.0.0.1:5433/paperrust?sslmode=disable
```

Export that `DATABASE_URL` before starting the API. This server listens on the loopback address only, with no password: any user or process on this machine can connect to it as the `postgres` superuser. Use it only on a single-user development machine; do not expose it or use it for a production archive.

## License

SPDX-License-Identifier: AGPL-3.0-or-later

Paperrust is free software under **AGPL-3.0-or-later**: the GNU Affero General Public License, version 3, or (at your option) any later version. The full license text is [LICENSE](LICENSE). Third-party components are listed in [NOTICE](NOTICE).

## Security

Report vulnerabilities privately through GitHub private vulnerability reporting. See [SECURITY.md](SECURITY.md).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).
