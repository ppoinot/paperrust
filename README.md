# Paperrust

Paperrust is a self-hosted document archive. You upload originals, read them in the browser, and keep correspondents, tags, types, and custom fields on each document. A Rust API stores the archive in PostgreSQL and serves the web interface from the same process.

The interface is in French.

## Features

- Dashboard with upload, counts, and monthly intake
- Document library, search, inbox, and trash with a retention delay
- In-browser PDF viewing, plus server-side page previews
- Optical character recognition through a language-model provider you configure
- Correspondents, tags, document types, custom fields, and storage paths
- Saved views and workflows that apply tags from document content
- Optional import from a Paperless-ngx instance
- Optional hand-off to Stirling-PDF for editing
- OpenID Connect sign-in, and API tokens for ingestion
- Activity log

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
