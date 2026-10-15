//! Personal API tokens for machine ingest (iOS Shortcuts, Quickscan / Paperless-ngx clients, etc.).
//!
//! Secrets are shown once at creation (`pr_…`). Only SHA-256 hashes are stored.
//! Scope v1: ingest only —
//! `POST /api/v1/ingest` and `POST /api/documents/post_document/` (ngx-compatible).

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::RngCore;
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};

const TOKEN_PREFIX: &str = "pr_";
const TOKEN_BYTES: usize = 32;
/// Display prefix length including `pr_` (e.g. `pr_AbCdEfGh`).
const PREFIX_CHARS: usize = 11;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiTokenStatus {
    pub id: i32,
    pub name: String,
    pub prefix: String,
    pub created_at: String,
    pub last_used_at: Option<String>,
    pub revoked_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatedApiToken {
    pub id: i32,
    pub name: String,
    pub prefix: String,
    /// Raw secret — returned **once** at creation. Never stored or listed again.
    pub token: String,
    pub created_at: String,
}

pub async fn ensure_schema(pool: &PgPool) -> anyhow::Result<()> {
    for sql in [
        r#"create table if not exists api_tokens (
            id serial primary key,
            owner_id int not null references users(id) on delete cascade,
            name text not null default '',
            token_hash text not null unique,
            prefix text not null,
            created_at timestamptz not null default now(),
            last_used_at timestamptz,
            revoked_at timestamptz
        )"#,
        "create index if not exists api_tokens_owner_id_idx on api_tokens (owner_id)",
        "create index if not exists api_tokens_hash_idx on api_tokens (token_hash) where revoked_at is null",
    ] {
        sqlx::query(sql).execute(pool).await?;
    }
    Ok(())
}

fn hash_token(raw: &str) -> String {
    let dig = Sha256::digest(raw.as_bytes());
    dig.iter().map(|b| format!("{b:02x}")).collect()
}

fn mint_raw_token() -> (String, String, String) {
    let mut bytes = [0u8; TOKEN_BYTES];
    rand::thread_rng().fill_bytes(&mut bytes);
    let raw = format!("{TOKEN_PREFIX}{}", URL_SAFE_NO_PAD.encode(bytes));
    let prefix: String = raw.chars().take(PREFIX_CHARS).collect();
    let hash = hash_token(&raw);
    (raw, hash, prefix)
}

pub async fn create_token(
    pool: &PgPool,
    owner_id: i32,
    name: &str,
) -> anyhow::Result<CreatedApiToken> {
    let name = name.trim();
    let name = if name.is_empty() { "iOS Shortcut" } else { name };
    if name.chars().count() > 80 {
        anyhow::bail!("Nom trop long (80 caractères max).");
    }
    let (raw, hash, prefix) = mint_raw_token();
    let row = sqlx::query(
        r#"insert into api_tokens (owner_id, name, token_hash, prefix)
           values ($1, $2, $3, $4)
           returning id, created_at::text as created_at"#,
    )
    .bind(owner_id)
    .bind(name)
    .bind(&hash)
    .bind(&prefix)
    .fetch_one(pool)
    .await?;
    Ok(CreatedApiToken {
        id: row.get("id"),
        name: name.to_string(),
        prefix,
        token: raw,
        created_at: row.get("created_at"),
    })
}

pub async fn list_tokens(pool: &PgPool, owner_id: i32) -> anyhow::Result<Vec<ApiTokenStatus>> {
    let rows = sqlx::query(
        r#"select id, name, prefix,
                  created_at::text as created_at,
                  last_used_at::text as last_used_at,
                  revoked_at::text as revoked_at
           from api_tokens
           where owner_id = $1
           order by created_at desc"#,
    )
    .bind(owner_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| ApiTokenStatus {
            id: r.get("id"),
            name: r.get("name"),
            prefix: r.get("prefix"),
            created_at: r.get("created_at"),
            last_used_at: r.get("last_used_at"),
            revoked_at: r.get("revoked_at"),
        })
        .collect())
}

pub async fn revoke_token(pool: &PgPool, owner_id: i32, token_id: i32) -> anyhow::Result<bool> {
    let res = sqlx::query(
        r#"update api_tokens
           set revoked_at = coalesce(revoked_at, now())
           where id = $1 and owner_id = $2 and revoked_at is null"#,
    )
    .bind(token_id)
    .bind(owner_id)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() > 0)
}

/// Resolve a live Bearer `pr_…` token to `(token_id, owner_id)`. Updates last_used_at.
pub async fn authenticate(pool: &PgPool, raw: &str) -> anyhow::Result<Option<(i32, i32)>> {
    let raw = raw.trim();
    if !raw.starts_with(TOKEN_PREFIX) || raw.len() < PREFIX_CHARS + 8 {
        return Ok(None);
    }
    let hash = hash_token(raw);
    let row = sqlx::query(
        r#"select id, owner_id from api_tokens
           where token_hash = $1 and revoked_at is null"#,
    )
    .bind(&hash)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let id: i32 = row.get("id");
    let owner_id: i32 = row.get("owner_id");
    let _ = sqlx::query(
        "update api_tokens set last_used_at = now() where id = $1 and revoked_at is null",
    )
    .bind(id)
    .execute(pool)
    .await;
    Ok(Some((id, owner_id)))
}

/// Extract a raw `pr_…` secret from `Authorization`.
///
/// Accepted schemes (same token table):
/// - `Bearer pr_…` — Paperrust / iOS Shortcut
/// - `Token pr_…` — Paperless-ngx / Quickscan
/// - `Basic …` — username or password is `pr_…` (ngx-style Basic)
pub fn raw_token_from_headers(headers: &axum::http::HeaderMap) -> Option<String> {
    let value = headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .trim();
    if let Some(rest) = value
        .strip_prefix("Bearer ")
        .or_else(|| value.strip_prefix("bearer "))
    {
        return nonempty_token(rest);
    }
    if let Some(rest) = value
        .strip_prefix("Token ")
        .or_else(|| value.strip_prefix("token "))
    {
        return nonempty_token(rest);
    }
    if let Some(rest) = value
        .strip_prefix("Basic ")
        .or_else(|| value.strip_prefix("basic "))
    {
        return token_from_basic(rest.trim());
    }
    None
}

/// Backward-compatible alias used by `/api/v1/ingest`.
pub fn bearer_from_headers(headers: &axum::http::HeaderMap) -> Option<String> {
    raw_token_from_headers(headers)
}

fn nonempty_token(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

fn token_from_basic(b64: &str) -> Option<String> {
    use base64::engine::general_purpose::STANDARD;
    let raw = STANDARD.decode(b64.as_bytes()).ok()?;
    let pair = String::from_utf8(raw).ok()?;
    let (user, pass) = pair.split_once(':').unwrap_or((pair.as_str(), ""));
    if user.starts_with(TOKEN_PREFIX) {
        return nonempty_token(user);
    }
    if pass.starts_with(TOKEN_PREFIX) {
        return nonempty_token(pass);
    }
    None
}
