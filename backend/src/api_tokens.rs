//! Personal API tokens (`pr_…`). Secrets are shown once at creation. Only SHA-256
//! hashes are stored.
//!
//! Scopes are chosen at creation and never widened:
//! - `ingest` — `POST /api/v1/ingest`, `POST /api/documents/post_document/`,
//!   and soft-delete of owned ids via `POST /api/documents/bulk`.
//! - `documents:read` — read the owner's documents and taxonomy. No upload,
//!   no mutation, no settings, no token management.
//! - `documents:meta` — `POST /api/documents/{id}/meta` for labels and fields.
//!   Cannot replace OCR `content`, delete, or upload.
//! - `diagnostics:read` — `GET /api/jobs` and `GET /api/diagnostics`.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::Rng;
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};

const TOKEN_PREFIX: &str = "pr_";
const TOKEN_BYTES: usize = 32;
/// Display prefix length including `pr_` (e.g. `pr_AbCdEfGh`).
const PREFIX_CHARS: usize = 11;

/// Upload and the existing ingest soft-delete. Default for tokens created
/// without an explicit scope (iOS Shortcut, Quickscan).
pub const SCOPE_INGEST: &str = "ingest";
/// Read the owning user's documents. Does not include `ingest`.
pub const SCOPE_DOCUMENTS_READ: &str = "documents:read";
/// Update labels and fields on an owned document. Does not include read,
/// ingest, or OCR text replacement.
pub const SCOPE_DOCUMENTS_META: &str = "documents:meta";
/// Consumption journal and the in-process diagnostic ring.
pub const SCOPE_DIAGNOSTICS_READ: &str = "diagnostics:read";

/// Page size applied when a read token omits `limit`.
pub const READ_PAGE_DEFAULT: i32 = 50;
/// Upper bound for an explicit `limit` on `POST /api/documents/query`.
pub const READ_PAGE_MAX: i32 = 100;
pub const READ_RATE_MAX: u32 = 120;
pub const READ_RATE_WINDOW_SECS: u64 = 60;
pub const META_RATE_MAX: u32 = 30;
pub const DIAG_RATE_MAX: u32 = 60;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiTokenStatus {
    pub id: i32,
    pub name: String,
    pub prefix: String,
    pub scopes: Vec<String>,
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
    pub scopes: Vec<String>,
    /// Raw secret — returned **once** at creation. Never stored or listed again.
    pub token: String,
    pub created_at: String,
}

/// A live `pr_…` token. `scopes` is the set stored at creation.
#[derive(Debug, Clone)]
pub struct TokenPrincipal {
    pub id: i32,
    pub owner_id: i32,
    pub scopes: Vec<String>,
}

impl TokenPrincipal {
    pub fn allows(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == scope)
    }
}

/// What `Authorization` presented, before a route decides if the scope fits.
pub enum PresentedToken {
    /// No `pr_…` credential (absent header, or an OIDC JWT / other scheme).
    NotPersonal,
    /// `pr_…` shape, but unknown or revoked.
    Invalid,
    Valid(TokenPrincipal),
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
        // Existing rows keep ingest-only. A read token is an explicit new secret.
        "alter table api_tokens add column if not exists scopes text[] not null default '{ingest}'",
    ] {
        sqlx::query(sql).execute(pool).await?;
    }
    Ok(())
}

/// `None` keeps the historical default (`ingest` only). An explicit list must
/// be non-empty and contain only known scopes. Unknown values are rejected
/// without being echoed back.
pub fn normalize_scopes(input: Option<Vec<String>>) -> Result<Vec<String>, &'static str> {
    let Some(input) = input else {
        return Ok(vec![SCOPE_INGEST.to_string()]);
    };
    if input.is_empty() {
        return Err("Au moins un scope est requis.");
    }
    let mut out = Vec::new();
    for raw in input {
        let scope = raw.trim();
        if !is_known_scope(scope) {
            return Err("Scope inconnu.");
        }
        if !out.iter().any(|seen: &String| seen == scope) {
            out.push(scope.to_string());
        }
    }
    out.sort_by_key(|scope| scope_rank(scope));
    Ok(out)
}

pub fn is_known_scope(scope: &str) -> bool {
    matches!(
        scope,
        SCOPE_INGEST | SCOPE_DOCUMENTS_READ | SCOPE_DOCUMENTS_META | SCOPE_DIAGNOSTICS_READ
    )
}

fn scope_rank(scope: &str) -> u8 {
    match scope {
        SCOPE_INGEST => 0,
        SCOPE_DOCUMENTS_READ => 1,
        SCOPE_DOCUMENTS_META => 2,
        SCOPE_DIAGNOSTICS_READ => 3,
        _ => 9,
    }
}

/// Default display name. The MCP trio is the only combination that becomes
/// "Connecteur MCP". A lone read scope stays "Bot lecture".
pub fn default_token_name(scopes: &[String]) -> &'static str {
    let has = |scope: &str| scopes.iter().any(|seen| seen == scope);
    if has(SCOPE_DOCUMENTS_READ)
        && has(SCOPE_DOCUMENTS_META)
        && has(SCOPE_DIAGNOSTICS_READ)
        && (scopes.len() == 3 || (scopes.len() == 4 && has(SCOPE_INGEST)))
    {
        "Connecteur MCP"
    } else if scopes.len() == 1 && has(SCOPE_DOCUMENTS_READ) {
        "Bot lecture"
    } else if !has(SCOPE_INGEST) {
        "Bot Paperrust"
    } else {
        "iOS Shortcut"
    }
}

/// A meta-scoped token must not replace the stored OCR text.
pub fn token_meta_blocks_content(reject_content: bool, content_present: bool) -> bool {
    reject_content && content_present
}

/// Adds one known scope to an active token. No-op when the token is unknown
/// or already has the scope. The raw secret is hashed and never logged.
pub async fn grant_scope(pool: &PgPool, raw: &str, scope: &str) -> sqlx::Result<u64> {
    if !is_known_scope(scope) {
        return Ok(0);
    }
    let hash = hash_token(raw);
    let result = sqlx::query(
        r#"update api_tokens
           set scopes = scopes || array[$2::text]
           where token_hash = $1
             and revoked_at is null
             and not ($2 = any(scopes))"#,
    )
    .bind(hash)
    .bind(scope)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

fn hash_token(raw: &str) -> String {
    let dig = Sha256::digest(raw.as_bytes());
    dig.iter().map(|b| format!("{b:02x}")).collect()
}

fn mint_raw_token() -> (String, String, String) {
    let mut bytes = [0u8; TOKEN_BYTES];
    rand::rng().fill_bytes(&mut bytes);
    let raw = format!("{TOKEN_PREFIX}{}", URL_SAFE_NO_PAD.encode(bytes));
    let prefix: String = raw.chars().take(PREFIX_CHARS).collect();
    let hash = hash_token(&raw);
    (raw, hash, prefix)
}

pub async fn create_token(
    pool: &PgPool,
    owner_id: i32,
    name: &str,
    scopes: &[String],
) -> anyhow::Result<CreatedApiToken> {
    if scopes.is_empty() || scopes.iter().any(|s| !is_known_scope(s)) {
        anyhow::bail!("Scope inconnu.");
    }
    let name = name.trim();
    let name = if name.is_empty() {
        default_token_name(scopes)
    } else {
        name
    };
    if name.chars().count() > 80 {
        anyhow::bail!("Nom trop long (80 caractères max).");
    }
    let (raw, hash, prefix) = mint_raw_token();
    let row = sqlx::query(
        r#"insert into api_tokens (owner_id, name, token_hash, prefix, scopes)
           values ($1, $2, $3, $4, $5)
           returning id, created_at::text as created_at"#,
    )
    .bind(owner_id)
    .bind(name)
    .bind(&hash)
    .bind(&prefix)
    .bind(scopes)
    .fetch_one(pool)
    .await?;
    Ok(CreatedApiToken {
        id: row.get("id"),
        name: name.to_string(),
        prefix,
        scopes: scopes.to_vec(),
        token: raw,
        created_at: row.get("created_at"),
    })
}

pub async fn list_tokens(pool: &PgPool, owner_id: i32) -> anyhow::Result<Vec<ApiTokenStatus>> {
    let rows = sqlx::query(
        r#"select id, name, prefix, scopes,
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
            scopes: r.get("scopes"),
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

/// Resolve a live `pr_…` token. Updates `last_used_at`. Does not check scope:
/// callers must call [`TokenPrincipal::allows`].
pub async fn authenticate(pool: &PgPool, raw: &str) -> anyhow::Result<Option<TokenPrincipal>> {
    let raw = raw.trim();
    if !raw.starts_with(TOKEN_PREFIX) || raw.len() < PREFIX_CHARS + 8 {
        return Ok(None);
    }
    let hash = hash_token(raw);
    let row = sqlx::query(
        r#"select id, owner_id, scopes from api_tokens
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
    let scopes: Vec<String> = row.get("scopes");
    let _ = sqlx::query(
        "update api_tokens set last_used_at = now() where id = $1 and revoked_at is null",
    )
    .bind(id)
    .execute(pool)
    .await;
    Ok(Some(TokenPrincipal {
        id,
        owner_id,
        scopes,
    }))
}

/// Classify an `Authorization` header. OIDC access tokens are
/// [`PresentedToken::NotPersonal`] so the session/JWT path stays unchanged.
pub async fn presented_token(
    pool: &PgPool,
    headers: &axum::http::HeaderMap,
) -> anyhow::Result<PresentedToken> {
    let Some(raw) = raw_token_from_headers(headers) else {
        return Ok(PresentedToken::NotPersonal);
    };
    if !raw.starts_with(TOKEN_PREFIX) {
        return Ok(PresentedToken::NotPersonal);
    }
    match authenticate(pool, &raw).await? {
        Some(principal) => Ok(PresentedToken::Valid(principal)),
        None => Ok(PresentedToken::Invalid),
    }
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

/// Routes a `documents:read` token may call. Everything else stays 403,
/// including settings, token management, upload, OCR, mail, and bulk edit.
pub fn is_documents_read_request(method: &axum::http::Method, path: &str) -> bool {
    let path = path.trim_end_matches('/');
    let path = if path.is_empty() { "/" } else { path };
    if method == axum::http::Method::POST && path == "/api/documents/query" {
        return true;
    }
    if method != axum::http::Method::GET && method != axum::http::Method::HEAD {
        return false;
    }
    if matches!(
        path,
        "/api/me"
            | "/api/catalog"
            | "/api/dashboard"
            | "/api/search"
            | "/api/tags"
            | "/api/correspondents"
            | "/api/types"
            | "/api/storage-paths"
            | "/api/fields"
    ) {
        return method == axum::http::Method::GET;
    }
    let Some(rest) = path.strip_prefix("/api/documents/") else {
        return false;
    };
    let mut parts = rest.split('/');
    let id = parts.next().unwrap_or("");
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    let a = parts.next();
    let b = parts.next();
    let c = parts.next();
    if parts.next().is_some() {
        return false;
    }
    match (a, b, c) {
        (None, None, None) => method == axum::http::Method::GET,
        (Some("file"), None, None) => true,
        (Some("preview"), None, None) => method == axum::http::Method::GET,
        (Some("preview"), Some("meta"), None) => method == axum::http::Method::GET,
        (Some("text"), None, None) => method == axum::http::Method::GET,
        (Some("pages"), Some(page), Some("text")) => {
            method == axum::http::Method::GET
                && !page.is_empty()
                && page.chars().all(|c| c.is_ascii_digit())
        }
        _ => false,
    }
}

fn normalized_path(path: &str) -> &str {
    let path = path.trim_end_matches('/');
    if path.is_empty() { "/" } else { path }
}

/// `POST /api/documents/{id}/meta` only. Not search, not bulk, not delete.
pub fn is_documents_meta_request(method: &axum::http::Method, path: &str) -> bool {
    if method != axum::http::Method::POST {
        return false;
    }
    let path = normalized_path(path);
    let Some(rest) = path.strip_prefix("/api/documents/") else {
        return false;
    };
    let mut parts = rest.split('/');
    let id = parts.next().unwrap_or("");
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    matches!((parts.next(), parts.next()), (Some("meta"), None)) && parts.next().is_none()
}

/// Consumption journal and the diagnostic ring. Not a document read.
pub fn is_diagnostics_request(method: &axum::http::Method, path: &str) -> bool {
    method == axum::http::Method::GET
        && matches!(normalized_path(path), "/api/jobs" | "/api/diagnostics")
}

/// Scope a known bot route requires. `None` when the route is not a bot route.
pub fn required_scope(method: &axum::http::Method, path: &str) -> Option<&'static str> {
    if is_documents_meta_request(method, path) {
        Some(SCOPE_DOCUMENTS_META)
    } else if is_diagnostics_request(method, path) {
        Some(SCOPE_DIAGNOSTICS_READ)
    } else if is_documents_read_request(method, path) {
        Some(SCOPE_DOCUMENTS_READ)
    } else if method == axum::http::Method::POST && normalized_path(path) == "/api/documents/bulk" {
        Some(SCOPE_INGEST)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Method;

    #[test]
    fn omitted_scopes_stay_ingest_only() {
        assert_eq!(normalize_scopes(None).unwrap(), vec!["ingest"]);
    }

    #[test]
    fn read_scope_is_exact_and_does_not_imply_ingest() {
        let scopes = normalize_scopes(Some(vec![" documents:read ".into()])).unwrap();
        assert_eq!(scopes, vec!["documents:read"]);
        let principal = TokenPrincipal {
            id: 1,
            owner_id: 2,
            scopes: scopes.clone(),
        };
        assert!(principal.allows(SCOPE_DOCUMENTS_READ));
        assert!(!principal.allows(SCOPE_INGEST));
    }

    #[test]
    fn unknown_or_empty_scopes_are_rejected() {
        assert!(normalize_scopes(Some(vec![])).is_err());
        assert!(normalize_scopes(Some(vec!["admin".into()])).is_err());
        assert!(normalize_scopes(Some(vec!["documents:write".into()])).is_err());
        assert!(normalize_scopes(Some(vec!["INGEST".into()])).is_err());
    }

    #[test]
    fn read_allowlist_is_document_reads_only() {
        assert!(is_documents_read_request(&Method::GET, "/api/documents/42"));
        assert!(is_documents_read_request(&Method::GET, "/api/documents/42/"));
        assert!(is_documents_read_request(&Method::HEAD, "/api/documents/42/file"));
        assert!(is_documents_read_request(&Method::GET, "/api/documents/42/preview"));
        assert!(is_documents_read_request(&Method::GET, "/api/documents/42/preview/meta"));
        assert!(is_documents_read_request(&Method::POST, "/api/documents/query"));
        assert!(is_documents_read_request(&Method::GET, "/api/me"));
        assert!(is_documents_read_request(&Method::GET, "/api/search"));
        assert!(is_documents_read_request(&Method::GET, "/api/tags"));

        assert!(!is_documents_read_request(&Method::POST, "/api/documents/42/meta"));
        assert!(!is_documents_read_request(&Method::POST, "/api/documents/42/ocr"));
        assert!(!is_documents_read_request(&Method::POST, "/api/documents/bulk"));
        assert!(!is_documents_read_request(&Method::POST, "/api/v1/ingest"));
        assert!(!is_documents_read_request(&Method::POST, "/api/documents/post_document"));
        assert!(!is_documents_read_request(&Method::GET, "/api/settings"));
        assert!(!is_documents_read_request(&Method::GET, "/api/settings/api-tokens"));
        assert!(!is_documents_read_request(&Method::DELETE, "/api/documents/42"));
        assert!(!is_documents_read_request(&Method::GET, "/api/documents/query"));
        assert!(!is_documents_read_request(&Method::GET, "/api/documents/42/edit/stirling"));
        assert!(!is_documents_read_request(&Method::GET, "/api/workflows"));
        assert!(!is_documents_read_request(&Method::POST, "/api/notes"));
        assert!(is_documents_read_request(&Method::GET, "/api/documents/42/text"));
        assert!(is_documents_read_request(&Method::GET, "/api/documents/42/pages/1/text"));
        assert!(!is_documents_read_request(&Method::GET, "/api/documents/42/pages/x/text"));
        assert!(!is_documents_read_request(&Method::GET, "/api/jobs"));
        assert!(!is_documents_read_request(&Method::GET, "/api/diagnostics"));
        assert!(!is_documents_read_request(&Method::POST, "/api/mcp"));
    }

    #[test]
    fn mcp_scopes_do_not_imply_ingest_or_each_other() {
        let scopes = normalize_scopes(Some(vec![
            "diagnostics:read".into(),
            "documents:meta".into(),
            "documents:read".into(),
            "documents:read".into(),
        ]))
        .unwrap();
        assert_eq!(
            scopes,
            vec!["documents:read", "documents:meta", "diagnostics:read"]
        );
        assert_eq!(default_token_name(&scopes), "Connecteur MCP");
        let mut with_ingest = scopes.clone();
        with_ingest.insert(0, SCOPE_INGEST.into());
        assert_eq!(default_token_name(&with_ingest), "Connecteur MCP");
        let principal = TokenPrincipal {
            id: 1,
            owner_id: 2,
            scopes: scopes.clone(),
        };
        assert!(!principal.allows(SCOPE_INGEST));
        assert!(principal.allows(SCOPE_DOCUMENTS_META));
        let read_only = TokenPrincipal {
            id: 1,
            owner_id: 2,
            scopes: vec![SCOPE_DOCUMENTS_READ.into()],
        };
        assert!(!read_only.allows(SCOPE_DOCUMENTS_META));
        assert!(!read_only.allows(SCOPE_DIAGNOSTICS_READ));
        let meta_only = TokenPrincipal {
            id: 1,
            owner_id: 2,
            scopes: vec![SCOPE_DOCUMENTS_META.into()],
        };
        assert!(!meta_only.allows(SCOPE_DOCUMENTS_READ));
        assert!(!meta_only.allows(SCOPE_DIAGNOSTICS_READ));
        assert!(!is_documents_read_request(&Method::POST, "/api/documents/42/meta"));
        assert!(meta_only.allows(SCOPE_DOCUMENTS_META));
        assert!(!is_documents_meta_request(&Method::GET, "/api/search"));
        assert!(!is_documents_meta_request(&Method::POST, "/api/documents/query"));
        assert!(is_documents_meta_request(&Method::POST, "/api/documents/42/meta"));
        assert!(!is_documents_meta_request(&Method::POST, "/api/documents/42/meta/extra"));
        assert_eq!(
            required_scope(&Method::POST, "/api/documents/42/meta"),
            Some(SCOPE_DOCUMENTS_META)
        );
        assert_eq!(
            required_scope(&Method::GET, "/api/diagnostics"),
            Some(SCOPE_DIAGNOSTICS_READ)
        );
        assert_eq!(
            required_scope(&Method::GET, "/api/search"),
            Some(SCOPE_DOCUMENTS_READ)
        );
        assert!(token_meta_blocks_content(true, true));
        assert!(!token_meta_blocks_content(false, true));
        assert!(!token_meta_blocks_content(true, false));
        assert!(normalize_scopes(Some(vec!["documents:write".into()])).is_err());
    }
}
