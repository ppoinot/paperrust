# Contributing

Thank you for considering a contribution to Paperrust. The project is licensed under **AGPL-3.0-or-later** (see [LICENSE](LICENSE)). By submitting a contribution, you agree that it may be distributed under those same terms.

## Setup

Follow [docs/install.md](docs/install.md). Configuration is environment variables; the commented list is [`.env.example`](.env.example).

Do not commit `.env`, credentials, or local database files.

## Checks

From the repository root:

```bash
npm test
```

API tests, from `backend/`:

```bash
cargo test
```

## Pull requests

- Target the `develop` branch.
- Describe the behavior change and how you tested it.
- Keep unrelated edits out of the change.
- Do not include secrets, live hostnames, or captured cookies in the diff, the description, or screenshots.

## Security issues

Do not file a public issue for a vulnerability. Use GitHub private vulnerability reporting, as described in [SECURITY.md](SECURITY.md). Never paste tokens, `DATABASE_URL`, or cookies.
