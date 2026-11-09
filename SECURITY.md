# Security

## Reporting a vulnerability

Report security issues through **GitHub private vulnerability reporting** on this repository. That is the only supported contact. Do not send vulnerability reports by personal email.

Open the **Security** tab, choose **Report a vulnerability**, and submit the report there. GitHub keeps that report private to the maintainers. Do not open a public issue, pull request, or discussion for a vulnerability.

We aim to acknowledge reports within **7 days**. There is **no bug bounty** program.

## What to include

Describe the affected version, the conditions that trigger the issue, and the impact. A minimal reproduction helps, as long as it contains no secrets.

## Never paste secrets

**Never paste tokens, `DATABASE_URL`, or cookies** into a vulnerability report, issue, pull request, commit, log, or chat.

That includes API tokens, OpenID Connect client secrets, session cookies, `SESSION_SECRET`, language-model credentials, and cron tokens. If a reproduction needs a credential, say which variable name is required and use a placeholder. Rotate any secret that has already been exposed.
