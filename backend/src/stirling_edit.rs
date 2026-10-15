//! PDF edit sessions for Stirling PDF (LAN).
//!
//! Stirling's visual editor is **browser-side** (IndexedDB / FileContext). There is
//! no supported deep-link to open a remote URL in the workbench, and no save
//! webhook back to an external DMS (Stirling#1004, docs: view-pdf is frontend-only).
//!
//! Cleanest Paperrust UX that still matches "Edit opens Stirling" + "save updates
//! Paperrust":
//! 1. Authenticated start → signed short-lived session + Stirling URL (new tab).
//! 2. Token-gated file bytes (no storage paths) so the browser can hand the PDF to the user
//!    (download / drop into Stirling). Optional server-side Stirling storage upload
//!    when `PAPERRUST_STIRLING_API_KEY` is set (alpha API; best-effort).
//! 3. Commit accepts the edited PDF (multipart) or pulls Stirling storage, replaces
//!    `original_bytes`, and re-runs the same OCR path as consume / `:id/ocr`.
//!
//! Tokens are HMAC-SHA256 over session claims (`SESSION_SECRET`). Raw filesystem
//! paths are never returned.

use crate::auth;
use crate::limits::MAX_UPLOAD_BYTES;
use crate::preview::looks_like_pdf;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, body::Body};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngCore;
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use std::time::{SystemTime, UNIX_EPOCH};
use url::Url;

const TOKEN_VERSION: &str = "pr-stirling-v1";
const DEFAULT_TTL_SECS: u64 = 30 * 60;
const MIN_TTL_SECS: u64 = 60;
const MAX_TTL_SECS: u64 = 60 * 60;
const HMAC_PREFIX: &str = "pr-stirling-v1";
/// In-cluster Stirling Service for server-to-server calls (upload / pull).
/// Plain HTTP is accepted ONLY for these exact hosts, and only for the API base
/// (never for the browser-facing editor URL).
const INTERNAL_API_DEFAULT: &str = "http://stirling-pdf.apps.svc:8080";
const INTERNAL_API_HOSTS: [&str; 2] = ["stirling-pdf.apps.svc", "stirling-pdf.apps.svc.cluster.local"];

/// Tokens are scoped to one operation. The `file` token is the
/// one that ends up in GET URLs (Stirling fetch, access logs, browser history) and
/// can only read. Only the `commit` token (POST path / cookie route body) can
/// replace the PDF.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenOp {
    File,
    Commit,
}

impl TokenOp {
    fn as_str(self) -> &'static str {
        match self {
            TokenOp::File => "file",
            TokenOp::Commit => "commit",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditClaims {
    pub sid: String,
    pub document_id: i32,
    pub owner_id: i32,
    pub exp: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartEditResponse {
    pub session_id: String,
    pub token: String,
    pub editor_url: String,
    pub source_file_url: String,
    pub commit_url: String,
    pub expires_at: String,
    pub filename: String,
    pub document_id: i32,
    pub mime_type: String,
    /// `skipped` (no API key), `ok`, or `failed` — never a storage path.
    pub stirling_upload: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitEditResponse {
    pub ok: bool,
    pub document_id: i32,
    pub ocr: serde_json::Value,
}

#[derive(Debug)]
pub struct StartError {
    pub status: StatusCode,
    pub code: &'static str,
}

impl StartError {
    pub fn not_found() -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: "not_found",
        }
    }
    pub fn bad(code: &'static str) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code,
        }
    }
}

impl IntoResponse for StartError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(serde_json::json!({ "error": self.code })),
        )
            .into_response()
    }
}

pub async fn ensure_schema(pool: &PgPool) -> anyhow::Result<()> {
    for sql in [
        r#"create table if not exists stirling_edit_sessions (
            id text primary key,
            document_id int not null references documents(id) on delete cascade,
            owner_id int not null references users(id) on delete cascade,
            filename text not null,
            stirling_file_id text,
            expires_at timestamptz not null,
            consumed_at timestamptz,
            created_at timestamptz not null default now()
        )"#,
        "create index if not exists stirling_edit_sessions_doc_owner_idx on stirling_edit_sessions (document_id, owner_id)",
        "create index if not exists stirling_edit_sessions_expires_idx on stirling_edit_sessions (expires_at)",
        r#"create table if not exists document_previous_versions (
            id bigserial primary key,
            document_id int not null references documents(id) on delete cascade,
            owner_id int not null references users(id) on delete cascade,
            original_bytes bytea not null,
            original_filename text,
            mime_type text,
            reason text not null,
            created_at timestamptz not null default now()
        )"#,
        "create index if not exists document_previous_versions_doc_idx on document_previous_versions (document_id, created_at desc)",
    ] {
        if let Err(e) = sqlx::query(sql).execute(pool).await {
            tracing::warn!("stirling_edit schema: {e}");
        }
    }
    Ok(())
}

/// Token-gated file / commit — no OIDC cookie. Token is verified in the handler.
pub fn is_token_public_path(path: &str) -> bool {
    let path = path.trim_end_matches('/');
    let Some(rest) = path.strip_prefix("/api/edit/stirling/") else {
        return false;
    };
    let Some((token, action)) = rest.split_once('/') else {
        return false;
    };
    token_charset_ok(token) && matches!(action, "file" | "commit")
}

fn token_charset_ok(token: &str) -> bool {
    let len = token.len();
    (16..=256).contains(&len)
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

pub fn session_ttl_secs() -> u64 {
    let raw = std::env::var("PAPERRUST_STIRLING_SESSION_TTL_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(DEFAULT_TTL_SECS);
    raw.clamp(MIN_TTL_SECS, MAX_TTL_SECS)
}

pub fn stirling_url_for_host(request_host: &str) -> String {
    let lan = env_url("PAPERRUST_STIRLING_URL_LAN");
    let public = env_url("PAPERRUST_STIRLING_URL");
    if auth::is_lan_request_host(request_host) {
        lan.unwrap_or_default()
    } else {
        public.unwrap_or_default()
    }
}

/// Server-to-server Stirling base (upload / pull). Never follows the request
/// host. Empty when no URL is configured (edit integration off).
///
/// Order: `PAPERRUST_STIRLING_API_URL` (https allowlist, or http on the exact
/// in-cluster Service) > in k8s (`KUBERNETES_SERVICE_HOST`) the in-cluster
/// Service > `PAPERRUST_STIRLING_URL_LAN` > `PAPERRUST_STIRLING_URL`.
pub fn stirling_api_base() -> String {
    if let Some(u) = api_env_url("PAPERRUST_STIRLING_API_URL") {
        return u;
    }
    if std::env::var_os("KUBERNETES_SERVICE_HOST").is_some() {
        return INTERNAL_API_DEFAULT.to_string();
    }
    env_url("PAPERRUST_STIRLING_URL_LAN")
        .or_else(|| env_url("PAPERRUST_STIRLING_URL"))
        .unwrap_or_default()
}

fn is_internal_api_host(host: &str) -> bool {
    INTERNAL_API_HOSTS.iter().any(|h| *h == host)
}

/// API base validation: the https allowlist, or `http://<internal host>[:port]`
/// with no credentials, path, query or fragment.
pub fn validate_stirling_api_base(raw: &str) -> Result<String, &'static str> {
    let trimmed = raw.trim().trim_end_matches('/');
    let url = Url::parse(trimmed).map_err(|_| "invalid_stirling_url")?;
    let host = url.host_str().map(|h| h.to_ascii_lowercase()).unwrap_or_default();
    if url.scheme() == "http" && is_internal_api_host(&host) {
        if url.username() != "" || url.password().is_some() {
            return Err("stirling_url_credentials_forbidden");
        }
        if url.query().is_some() || url.fragment().is_some() || url.path() != "/" {
            return Err("stirling_url_must_be_origin");
        }
        return Ok(trimmed.to_string());
    }
    validate_stirling_base(trimmed)
}

fn api_env_url(name: &str) -> Option<String> {
    let raw = std::env::var(name).ok()?;
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    validate_stirling_api_base(trimmed).ok()
}

/// Stirling editor deep-link: `{base}/?fileUrl={urlencoded sourceFileUrl}`.
/// Uses the existing signed Paperrust file URL (no second token).
pub fn editor_url_with_file_url(stirling_base: &str, source_file_url: &str) -> String {
    let base = stirling_base.trim_end_matches('/');
    let mut url = Url::parse(&format!("{base}/")).unwrap_or_else(|_| {
        Url::parse("http://127.0.0.1/").expect("loopback fallback origin")
    });
    url.query_pairs_mut().append_pair("fileUrl", source_file_url);
    url.to_string()
}

fn env_url(name: &str) -> Option<String> {
    let raw = std::env::var(name).ok()?;
    let trimmed = raw.trim().trim_end_matches('/').to_string();
    if trimmed.is_empty() {
        return None;
    }
    match validate_stirling_base(&trimmed) {
        Ok(u) => Some(u),
        Err(_) => None,
    }
}

/// HTTPS Stirling base; host must be on the allowlist (defaults + configured URLs).
pub fn validate_stirling_base(raw: &str) -> Result<String, &'static str> {
    let trimmed = raw.trim().trim_end_matches('/');
    let url = Url::parse(trimmed).map_err(|_| "invalid_stirling_url")?;
    if url.scheme() != "https" {
        return Err("stirling_https_required");
    }
    if url.username() != "" || url.password().is_some() {
        return Err("stirling_url_credentials_forbidden");
    }
    let host = match url.host() {
        Some(url::Host::Domain(d)) => d.to_ascii_lowercase(),
        Some(url::Host::Ipv4(ip)) => ip.to_string(),
        Some(url::Host::Ipv6(ip)) => format!("[{ip}]"),
        None => return Err("stirling_url_host_required"),
    };
    if !stirling_host_allowed(&host) {
        return Err("stirling_host_not_allowed");
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err("stirling_url_must_be_origin");
    }
    Ok(trimmed.to_string())
}

fn stirling_host_allowed(host: &str) -> bool {
    configured_stirling_hosts()
        .iter()
        .any(|allowed| allowed == host)
}

/// Exact hosts we will ever open or call. No suffix match.
pub fn configured_stirling_hosts() -> Vec<String> {
    let mut out = Vec::new();
    for name in [
        "PAPERRUST_STIRLING_URL",
        "PAPERRUST_STIRLING_URL_LAN",
        "PAPERRUST_STIRLING_API_URL",
    ] {
        if let Ok(raw) = std::env::var(name) {
            if let Ok(url) = Url::parse(raw.trim()) {
                if let Some(h) = url.host_str() {
                    let h = h.to_ascii_lowercase();
                    if !h.is_empty() && !out.iter().any(|x| x == &h) {
                        out.push(h);
                    }
                }
            }
        }
    }
    for host in crate::instance::stirling_extra_hosts() {
        if !out.iter().any(|x| x == &host) {
            out.push(host);
        }
    }
    out
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut key_block = [0u8; BLOCK];
    if key.len() > BLOCK {
        let d = Sha256::digest(key);
        key_block[..32].copy_from_slice(&d);
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; BLOCK];
    let mut opad = [0x5cu8; BLOCK];
    for i in 0..BLOCK {
        ipad[i] ^= key_block[i];
        opad[i] ^= key_block[i];
    }
    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(data);
    let inner_hash = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner_hash);
    let out = outer.finalize();
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&out);
    arr
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut r = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        r |= x ^ y;
    }
    r == 0
}

fn signing_key() -> Option<String> {
    auth::session_secret()
}

fn mac_message(claims: &EditClaims, op: TokenOp) -> String {
    format!(
        "{HMAC_PREFIX}\n{}\n{}\n{}\n{}\n{}",
        op.as_str(),
        claims.sid,
        claims.document_id,
        claims.owner_id,
        claims.exp
    )
}

pub fn sign_edit_token(claims: &EditClaims, op: TokenOp) -> Option<String> {
    let secret = signing_key()?;
    let mac = hmac_sha256(secret.as_bytes(), mac_message(claims, op).as_bytes());
    Some(format!(
        "{}.{}.{}.{}.{}",
        claims.sid,
        claims.exp,
        claims.document_id,
        claims.owner_id,
        URL_SAFE_NO_PAD.encode(mac)
    ))
}

pub fn verify_edit_token(token: &str, op: TokenOp, now: u64) -> Option<EditClaims> {
    if !token_charset_ok(token) {
        return None;
    }
    let secret = signing_key()?;
    let mut parts = token.rsplitn(5, '.');
    let sig = parts.next()?;
    let uid = parts.next()?;
    let doc = parts.next()?;
    let exp = parts.next()?;
    let sid = parts.next()?;
    if parts.next().is_some() || sid.is_empty() {
        return None;
    }
    let owner_id: i32 = uid.parse().ok()?;
    let document_id: i32 = doc.parse().ok()?;
    let exp: u64 = exp.parse().ok()?;
    if sid.len() < 16 || sid.len() > 40 {
        return None;
    }
    if !sid
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return None;
    }
    let claims = EditClaims {
        sid: sid.to_string(),
        document_id,
        owner_id,
        exp,
    };
    let got = URL_SAFE_NO_PAD.decode(sig).ok()?;
    let expected = hmac_sha256(secret.as_bytes(), mac_message(&claims, op).as_bytes());
    if !ct_eq(&got, &expected) {
        return None;
    }
    if now >= claims.exp {
        return None;
    }
    Some(claims)
}

fn new_sid() -> String {
    let mut buf = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

fn pdf_filename(hint: &str, existing: Option<&str>) -> String {
    let existing = existing.unwrap_or("").trim();
    if existing.to_ascii_lowercase().ends_with(".pdf") && !existing.is_empty() {
        return crate::limits::clamp_filename(existing);
    }
    let hint = crate::limits::clamp_filename(hint.trim());
    if hint.to_ascii_lowercase().ends_with(".pdf") && !hint.is_empty() {
        return hint;
    }
    if hint.is_empty() {
        "document.pdf".into()
    } else {
        format!("{hint}.pdf")
    }
}

pub async fn start_session(
    pool: &PgPool,
    owner_id: i32,
    document_id: i32,
    request_host: &str,
) -> Result<StartEditResponse, StartError> {
    let row = sqlx::query(
        r#"select mime_type, original_filename,
                  (original_bytes is not null and octet_length(original_bytes) > 0) as has_bytes,
                  original_bytes
           from documents
           where id = $1 and owner_id = $2 and deleted_at is null"#,
    )
    .bind(document_id)
    .bind(owner_id)
    .fetch_optional(pool)
    .await
    .map_err(|_| StartError::bad("request_failed"))?;
    let Some(row) = row else {
        return Err(StartError::not_found());
    };
    let has_bytes: bool = row.get("has_bytes");
    if !has_bytes {
        return Err(StartError::bad("file_missing"));
    }
    let bytes: Vec<u8> = row.get("original_bytes");
    if !looks_like_pdf(&bytes) {
        return Err(StartError::bad("not_pdf"));
    }
    let mime: String = row.get("mime_type");
    let orig: Option<String> = row.get("original_filename");
    let filename = pdf_filename(orig.as_deref().unwrap_or("document.pdf"), orig.as_deref());

    let ttl = session_ttl_secs();
    let exp = now_secs().saturating_add(ttl);
    let sid = new_sid();
    let claims = EditClaims {
        sid: sid.clone(),
        document_id,
        owner_id,
        exp,
    };
    let (Some(file_token), Some(token)) = (
        sign_edit_token(&claims, TokenOp::File),
        sign_edit_token(&claims, TokenOp::Commit),
    ) else {
        return Err(StartError {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "session_secret_missing",
        });
    };

    let expires_at = chrono::Utc::now() + chrono::Duration::seconds(ttl as i64);
    let _ = sqlx::query(
        r#"update stirling_edit_sessions
           set consumed_at = now()
           where document_id = $1 and owner_id = $2 and consumed_at is null"#,
    )
    .bind(document_id)
    .bind(owner_id)
    .execute(pool)
    .await;

    sqlx::query(
        r#"insert into stirling_edit_sessions
           (id, document_id, owner_id, filename, expires_at)
           values ($1,$2,$3,$4,$5)"#,
    )
    .bind(&sid)
    .bind(document_id)
    .bind(owner_id)
    .bind(&filename)
    .bind(expires_at)
    .execute(pool)
    .await
    .map_err(|_| StartError::bad("request_failed"))?;

    let stirling_base = stirling_url_for_host(request_host);
    let origin = auth::app_origin_for(request_host);
    let source_file_url = format!("{origin}/api/edit/stirling/{file_token}/file");
    let editor_url = editor_url_with_file_url(&stirling_base, &source_file_url);
    let mut stirling_upload = "skipped".to_string();
    if let Some(api_key) = stirling_api_key() {
        match upload_to_stirling(&stirling_api_base(), &api_key, &filename, &bytes).await {
            Ok(file_id) => {
                let _ = sqlx::query(
                    "update stirling_edit_sessions set stirling_file_id = $1 where id = $2",
                )
                .bind(&file_id)
                .bind(&sid)
                .execute(pool)
                .await;
                stirling_upload = "ok".into();
            }
            Err(e) => {
                tracing::warn!(error = %e, "stirling storage upload skipped");
                stirling_upload = "failed".into();
            }
        }
    }

    Ok(StartEditResponse {
        session_id: sid,
        token: token.clone(),
        editor_url,
        source_file_url,
        commit_url: format!("{origin}/api/edit/stirling/{token}/commit"),
        expires_at: expires_at.to_rfc3339(),
        filename,
        document_id,
        mime_type: if mime.eq_ignore_ascii_case("application/pdf") {
            mime
        } else {
            "application/pdf".into()
        },
        stirling_upload,
    })
}

struct LiveSession {
    claims: EditClaims,
    filename: String,
    stirling_file_id: Option<String>,
}

async fn load_live_session(
    pool: &PgPool,
    token: &str,
    op: TokenOp,
) -> Result<LiveSession, StartError> {
    let Some(claims) = verify_edit_token(token, op, now_secs()) else {
        return Err(StartError {
            status: StatusCode::UNAUTHORIZED,
            code: "invalid_token",
        });
    };
    let row = sqlx::query(
        r#"select filename, stirling_file_id, consumed_at, expires_at
           from stirling_edit_sessions
           where id = $1 and document_id = $2 and owner_id = $3"#,
    )
    .bind(&claims.sid)
    .bind(claims.document_id)
    .bind(claims.owner_id)
    .fetch_optional(pool)
    .await
    .map_err(|_| StartError::bad("request_failed"))?;
    let Some(row) = row else {
        return Err(StartError {
            status: StatusCode::UNAUTHORIZED,
            code: "invalid_token",
        });
    };
    let consumed: Option<chrono::DateTime<chrono::Utc>> = row.get("consumed_at");
    if consumed.is_some() {
        return Err(StartError {
            status: StatusCode::GONE,
            code: "session_consumed",
        });
    }
    let expires_at: chrono::DateTime<chrono::Utc> = row.get("expires_at");
    if expires_at <= chrono::Utc::now() {
        return Err(StartError {
            status: StatusCode::UNAUTHORIZED,
            code: "invalid_token",
        });
    }
    Ok(LiveSession {
        claims,
        filename: row.get("filename"),
        stirling_file_id: row.get("stirling_file_id"),
    })
}

pub async fn serve_session_file(pool: &PgPool, token: &str, method_is_head: bool) -> Response {
    let session = match load_live_session(pool, token, TokenOp::File).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    let row = sqlx::query(
        r#"select original_bytes from documents
           where id = $1 and owner_id = $2 and deleted_at is null"#,
    )
    .bind(session.claims.document_id)
    .bind(session.claims.owner_id)
    .fetch_optional(pool)
    .await;
    let Ok(Some(row)) = row else {
        return StartError::not_found().into_response();
    };
    let bytes: Option<Vec<u8>> = row.get("original_bytes");
    let Some(bytes) = bytes.filter(|b| looks_like_pdf(b)) else {
        return StartError::not_found().into_response();
    };
    let filename = sanitize_download_name(&session.filename);
    let disp = format!("inline; filename=\"{filename}\"");
    let mut builder = Response::builder()
        .status(StatusCode::OK)
        .header(axum::http::header::CONTENT_TYPE, "application/pdf")
        .header(axum::http::header::CACHE_CONTROL, "private, no-store")
        .header(axum::http::header::CONTENT_DISPOSITION, disp)
        .header(axum::http::header::CONTENT_LENGTH, bytes.len());
    if let Some(h) = builder.headers_mut() {
        h.insert(
            axum::http::header::VARY,
            HeaderValue::from_static("Origin"),
        );
    }
    if method_is_head {
        return builder.body(Body::empty()).unwrap_or_else(|_| {
            (StatusCode::INTERNAL_SERVER_ERROR, "head").into_response()
        });
    }
    builder
        .body(Body::from(bytes))
        .unwrap_or_else(|_| (StatusCode::INTERNAL_SERVER_ERROR, "file").into_response())
}

/// CORS for `GET /api/edit/stirling/{token}/file` only.
/// The token is in the URL, so credentials are not allowed. HEAD and OPTIONS
/// get no CORS headers. ACAO mirrors a known Stirling Origin (never `*`).
pub fn apply_stirling_file_cors(method: &Method, headers: &HeaderMap, res: &mut Response) {
    if *method != Method::GET {
        return;
    }
    let Some(origin) = headers
        .get(axum::http::header::ORIGIN)
        .and_then(|v| v.to_str().ok())
    else {
        return;
    };
    if !is_allowed_stirling_origin(origin) {
        return;
    }
    if let Ok(v) = HeaderValue::from_str(origin) {
        res.headers_mut()
            .insert(axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN, v);
        res.headers_mut().insert(
            axum::http::header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("GET"),
        );
        res.headers_mut().insert(
            axum::http::header::VARY,
            HeaderValue::from_static("Origin"),
        );
    }
}

pub fn is_allowed_stirling_origin(origin: &str) -> bool {
    let Ok(url) = Url::parse(origin) else {
        return false;
    };
    if url.scheme() != "https" {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    stirling_host_allowed(&host.to_ascii_lowercase())
}

fn sanitize_download_name(name: &str) -> String {
    let t: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
            c
        } else {
            '_'
        })
        .take(180)
        .collect();
    if t.to_ascii_lowercase().ends_with(".pdf") && t.len() > 4 {
        t
    } else {
        "document.pdf".into()
    }
}

pub async fn commit_session(
    pool: &PgPool,
    token: &str,
    uploaded: Option<(Vec<u8>, String)>,
    pull: bool,
) -> Result<CommitEditResponse, StartError> {
    let session = load_live_session(pool, token, TokenOp::Commit).await?;
    let (bytes, filename) = if let Some((b, name)) = uploaded {
        (b, pdf_filename(&name, Some(&session.filename)))
    } else if pull {
        let Some(file_id) = session.stirling_file_id.as_deref() else {
            return Err(StartError::bad("stirling_file_unavailable"));
        };
        if !stirling_file_id_ok(file_id) {
            return Err(StartError::bad("stirling_file_unavailable"));
        }
        let Some(api_key) = stirling_api_key() else {
            return Err(StartError::bad("stirling_file_unavailable"));
        };
        let base = stirling_api_base();
        match download_from_stirling(&base, &api_key, file_id).await {
            Ok(b) => (b, session.filename.clone()),
            Err(e) => {
                tracing::warn!(error = %e, "stirling storage pull failed");
                return Err(StartError::bad("stirling_pull_failed"));
            }
        }
    } else {
        return Err(StartError::bad("missing_file"));
    };

    if bytes.len() > MAX_UPLOAD_BYTES {
        return Err(StartError {
            status: StatusCode::PAYLOAD_TOO_LARGE,
            code: "payload_too_large",
        });
    }
    if !looks_like_pdf(&bytes) {
        return Err(StartError::bad("not_pdf"));
    }

    claim_session(pool, &session.claims.sid).await?;
    let ocr = match crate::consume::replace_document_pdf(
        pool,
        session.claims.document_id,
        session.claims.owner_id,
        bytes,
        &filename,
    )
    .await
    {
        Ok(ocr) => ocr,
        Err(e) => {
            tracing::warn!(error = %e, "stirling replace+ocr failed");
            release_claim(pool, &session.claims.sid).await;
            return Err(StartError::bad("replace_failed"));
        }
    };

    Ok(CommitEditResponse {
        ok: true,
        document_id: session.claims.document_id,
        ocr,
    })
}

/// Cookie-authenticated commit for the latest live session on this document.
pub async fn commit_for_document(
    pool: &PgPool,
    owner_id: i32,
    document_id: i32,
    token: Option<&str>,
    uploaded: Option<(Vec<u8>, String)>,
    pull: bool,
) -> Result<CommitEditResponse, StartError> {
    if let Some(token) = token.map(str::trim).filter(|s| !s.is_empty()) {
        let session = load_live_session(pool, token, TokenOp::Commit).await?;
        if session.claims.document_id != document_id || session.claims.owner_id != owner_id {
            return Err(StartError::not_found());
        }
        return commit_session(pool, token, uploaded, pull).await;
    }
    let row = sqlx::query(
        r#"select id from stirling_edit_sessions
           where document_id = $1 and owner_id = $2 and consumed_at is null and expires_at > now()
           order by created_at desc
           limit 1"#,
    )
    .bind(document_id)
    .bind(owner_id)
    .fetch_optional(pool)
    .await
    .map_err(|_| StartError::bad("request_failed"))?;
    let Some(row) = row else {
        return Err(StartError::bad("no_active_session"));
    };
    let sid: String = row.get("id");
    // Reconstruct a token we can re-verify via DB row (HMAC still required for the
    // public token path). Cookie path uses the live row + owner check instead.
    commit_session_by_sid(pool, &sid, owner_id, document_id, uploaded, pull).await
}

async fn commit_session_by_sid(
    pool: &PgPool,
    sid: &str,
    owner_id: i32,
    document_id: i32,
    uploaded: Option<(Vec<u8>, String)>,
    pull: bool,
) -> Result<CommitEditResponse, StartError> {
    let row = sqlx::query(
        r#"select filename, stirling_file_id, consumed_at, expires_at
           from stirling_edit_sessions
           where id = $1 and document_id = $2 and owner_id = $3"#,
    )
    .bind(sid)
    .bind(document_id)
    .bind(owner_id)
    .fetch_optional(pool)
    .await
    .map_err(|_| StartError::bad("request_failed"))?;
    let Some(row) = row else {
        return Err(StartError::not_found());
    };
    let consumed: Option<chrono::DateTime<chrono::Utc>> = row.get("consumed_at");
    if consumed.is_some() {
        return Err(StartError {
            status: StatusCode::GONE,
            code: "session_consumed",
        });
    }
    let expires_at: chrono::DateTime<chrono::Utc> = row.get("expires_at");
    if expires_at <= chrono::Utc::now() {
        return Err(StartError {
            status: StatusCode::UNAUTHORIZED,
            code: "invalid_token",
        });
    }
    let filename: String = row.get("filename");
    let stirling_file_id: Option<String> = row.get("stirling_file_id");

    let (bytes, filename) = if let Some((b, name)) = uploaded {
        (b, pdf_filename(&name, Some(&filename)))
    } else if pull {
        let Some(file_id) = stirling_file_id.as_deref() else {
            return Err(StartError::bad("stirling_file_unavailable"));
        };
        if !stirling_file_id_ok(file_id) {
            return Err(StartError::bad("stirling_file_unavailable"));
        }
        let Some(api_key) = stirling_api_key() else {
            return Err(StartError::bad("stirling_file_unavailable"));
        };
        let base = stirling_api_base();
        match download_from_stirling(&base, &api_key, file_id).await {
            Ok(b) => (b, filename),
            Err(e) => {
                tracing::warn!(error = %e, "stirling storage pull failed");
                return Err(StartError::bad("stirling_pull_failed"));
            }
        }
    } else {
        return Err(StartError::bad("missing_file"));
    };

    if bytes.len() > MAX_UPLOAD_BYTES {
        return Err(StartError {
            status: StatusCode::PAYLOAD_TOO_LARGE,
            code: "payload_too_large",
        });
    }
    if !looks_like_pdf(&bytes) {
        return Err(StartError::bad("not_pdf"));
    }

    claim_session(pool, sid).await?;
    let ocr =
        match crate::consume::replace_document_pdf(pool, document_id, owner_id, bytes, &filename)
            .await
        {
            Ok(ocr) => ocr,
            Err(e) => {
                tracing::warn!(error = %e, "stirling replace+ocr failed");
                release_claim(pool, sid).await;
                return Err(StartError::bad("replace_failed"));
            }
        };

    Ok(CommitEditResponse {
        ok: true,
        document_id,
        ocr,
    })
}

/// Atomically claim the session *before* replacing the PDF.
/// Only one concurrent commit (or replay while OCR runs) can win the claim.
async fn claim_session(pool: &PgPool, sid: &str) -> Result<(), StartError> {
    let claimed: Option<String> = sqlx::query_scalar(
        r#"update stirling_edit_sessions
           set consumed_at = now()
           where id = $1 and consumed_at is null and expires_at > now()
           returning id"#,
    )
    .bind(sid)
    .fetch_optional(pool)
    .await
    .map_err(|_| StartError::bad("request_failed"))?;
    if claimed.is_none() {
        return Err(StartError {
            status: StatusCode::GONE,
            code: "session_consumed",
        });
    }
    Ok(())
}

/// Replace failed before anything was written: let the user retry within the TTL.
async fn release_claim(pool: &PgPool, sid: &str) {
    let _ = sqlx::query("update stirling_edit_sessions set consumed_at = null where id = $1")
        .bind(sid)
        .execute(pool)
        .await;
}

fn stirling_api_key() -> Option<String> {
    std::env::var("PAPERRUST_STIRLING_API_KEY")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn stirling_file_id_ok(id: &str) -> bool {
    let len = id.len();
    (1..=128).contains(&len)
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn stirling_storage_url(base: &str, suffix: &str) -> Result<Url, anyhow::Error> {
    let base = validate_stirling_api_base(base).map_err(|e| anyhow::anyhow!(e))?;
    let url = Url::parse(&format!("{base}/"))?;
    let joined = url.join(suffix)?;
    let host = joined
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("stirling_url_host_required"))?
        .to_ascii_lowercase();
    let internal_http = joined.scheme() == "http" && is_internal_api_host(&host);
    if joined.scheme() != "https" && !internal_http {
        anyhow::bail!("stirling_https_required");
    }
    if !internal_http && !stirling_host_allowed(&host) {
        anyhow::bail!("stirling_host_not_allowed");
    }
    if !joined.path().starts_with("/api/v1/storage/") {
        anyhow::bail!("stirling_path_refused");
    }
    Ok(joined)
}

fn stirling_http_client(_base: &str) -> anyhow::Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(30));
    if let Some(path) = std::env::var("PAPERRUST_STIRLING_CA_BUNDLE")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(crate::instance::oidc_ca_bundle_path)
    {
        let pem = std::fs::read(&path)?;
        let certs = reqwest::Certificate::from_pem_bundle(&pem)?;
        if certs.is_empty() {
            anyhow::bail!("Stirling CA bundle has no certificates");
        }
        for cert in certs {
            builder = builder.add_root_certificate(cert);
        }
    }
    Ok(builder.build()?)
}

async fn upload_to_stirling(
    base: &str,
    api_key: &str,
    filename: &str,
    bytes: &[u8],
) -> anyhow::Result<String> {
    let url = stirling_storage_url(base, "api/v1/storage/files")?;
    let client = stirling_http_client(base)?;
    let part = reqwest::multipart::Part::bytes(bytes.to_vec())
        .file_name(sanitize_download_name(filename))
        .mime_str("application/pdf")?;
    let form = reqwest::multipart::Form::new().part("file", part);
    let res = client
        .post(url)
        .header("X-API-KEY", api_key)
        .multipart(form)
        .send()
        .await?;
    if !res.status().is_success() {
        anyhow::bail!("stirling_upload_http");
    }
    let v: serde_json::Value = res.json().await?;
    parse_stirling_file_id(&v).ok_or_else(|| anyhow::anyhow!("stirling_upload_no_id"))
}

async fn download_from_stirling(base: &str, api_key: &str, file_id: &str) -> anyhow::Result<Vec<u8>> {
    if !stirling_file_id_ok(file_id) {
        anyhow::bail!("stirling_file_id");
    }
    let url = stirling_storage_url(base, &format!("api/v1/storage/files/{file_id}/download"))?;
    let client = stirling_http_client(base)?;
    let res = client
        .get(url)
        .header("X-API-KEY", api_key)
        .send()
        .await?;
    if !res.status().is_success() {
        anyhow::bail!("stirling_download_http");
    }
    let bytes = res.bytes().await?;
    if bytes.len() > MAX_UPLOAD_BYTES {
        anyhow::bail!("payload_too_large");
    }
    if !looks_like_pdf(&bytes) {
        anyhow::bail!("not_pdf");
    }
    Ok(bytes.to_vec())
}

fn parse_stirling_file_id(v: &serde_json::Value) -> Option<String> {
    for key in ["id", "fileId", "file_id"] {
        if let Some(s) = v.get(key).and_then(|x| x.as_str()) {
            if stirling_file_id_ok(s) {
                return Some(s.to_string());
            }
        }
    }
    if let Some(s) = v
        .pointer("/file/id")
        .or_else(|| v.pointer("/data/id"))
        .and_then(|x| x.as_str())
    {
        if stirling_file_id_ok(s) {
            return Some(s.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_lock() -> &'static std::sync::Mutex<()> {
        crate::instance::env_lock()
    }

    fn with_secret<R>(f: impl FnOnce() -> R) -> R {
        let _g = env_lock().lock().unwrap();
        let prev = std::env::var("SESSION_SECRET").ok();
        // SAFETY: serialized via env_lock.
        unsafe {
            std::env::set_var(
                "SESSION_SECRET",
                "test-session-secret-32chars-minimum!!",
            );
        }
        let out = f();
        unsafe {
            match prev {
                Some(v) => std::env::set_var("SESSION_SECRET", v),
                None => std::env::remove_var("SESSION_SECRET"),
            }
        }
        out
    }

    #[test]
    fn token_roundtrip_and_expiry() {
        with_secret(|| {
            let claims = EditClaims {
                sid: "abcdefghijklmnopqr".into(),
                document_id: 42,
                owner_id: 7,
                exp: now_secs() + 600,
            };
            let tok = sign_edit_token(&claims, TokenOp::File).expect("sign");
            assert!(token_charset_ok(&tok));
            let got = verify_edit_token(&tok, TokenOp::File, now_secs()).expect("verify");
            assert_eq!(got, claims);
            assert!(verify_edit_token(&tok, TokenOp::File, claims.exp).is_none());
            assert!(verify_edit_token(&tok[..tok.len() - 2], TokenOp::File, now_secs()).is_none());
        });
    }

    #[test]
    fn token_is_scoped_to_operation() {
        with_secret(|| {
            let claims = EditClaims {
                sid: "abcdefghijklmnopqr".into(),
                document_id: 5,
                owner_id: 2,
                exp: now_secs() + 600,
            };
            let file_tok = sign_edit_token(&claims, TokenOp::File).unwrap();
            let commit_tok = sign_edit_token(&claims, TokenOp::Commit).unwrap();
            assert_ne!(file_tok, commit_tok);
            // M1: the read-only file token (seen in GET URLs / logs) cannot commit.
            assert!(verify_edit_token(&file_tok, TokenOp::Commit, now_secs()).is_none());
            assert!(verify_edit_token(&commit_tok, TokenOp::File, now_secs()).is_none());
            assert_eq!(
                verify_edit_token(&commit_tok, TokenOp::Commit, now_secs()),
                Some(claims)
            );
        });
    }

    #[test]
    fn token_rejects_tampered_doc_id() {
        with_secret(|| {
            let claims = EditClaims {
                sid: "abcdefghijklmnopqr".into(),
                document_id: 1,
                owner_id: 1,
                exp: now_secs() + 600,
            };
            let tok = sign_edit_token(&claims, TokenOp::File).unwrap();
            let tampered = tok.replace(".1.1.", ".99.1.");
            assert!(verify_edit_token(&tampered, TokenOp::File, now_secs()).is_none());
        });
    }

    #[test]
    fn token_does_not_embed_paths() {
        with_secret(|| {
            let claims = EditClaims {
                sid: "abcdefghijklmnopqr".into(),
                document_id: 9,
                owner_id: 3,
                exp: now_secs() + 60,
            };
            let tok = sign_edit_token(&claims, TokenOp::File).unwrap();
            assert!(!tok.contains('/'));
            assert!(!tok.contains("original_bytes"));
            assert!(!tok.contains("documents"));
        });
    }

    #[test]
    fn public_path_only_file_and_commit() {
        assert!(is_token_public_path(
            "/api/edit/stirling/abcDEF0123456789._-/file"
        ));
        assert!(is_token_public_path(
            "/api/edit/stirling/abcDEF0123456789._-/commit/"
        ));
        assert!(!is_token_public_path("/api/edit/stirling/abcDEF0123456789/open"));
        assert!(!is_token_public_path("/api/documents/1/edit/stirling"));
        assert!(!is_token_public_path("/api/edit/stirling/../file"));
        assert!(!is_token_public_path("/api/edit/stirling/short/file"));
    }

    fn with_stirling_hosts<T>(f: impl FnOnce() -> T) -> T {
        let _g = env_lock().lock().unwrap();
        let keys = [
            "PAPERRUST_STIRLING_URL",
            "PAPERRUST_STIRLING_URL_LAN",
            "PAPERRUST_STIRLING_EXTRA_HOSTS",
            "PAPERRUST_STIRLING_API_URL",
            "PAPERRUST_PUBLIC_URLS",
            "PAPERRUST_OIDC_ISSUERS",
            "PAPERRUST_OIDC_ISSUER",
            "PAPERRUST_LAN_URLS",
            "KUBERNETES_SERVICE_HOST",
        ];
        let prev: Vec<Option<String>> = keys.iter().map(|k| std::env::var(k).ok()).collect();
        unsafe {
            std::env::set_var("PAPERRUST_STIRLING_URL", "https://pdf.example");
            std::env::set_var("PAPERRUST_STIRLING_URL_LAN", "https://stirling.example");
            std::env::set_var("PAPERRUST_STIRLING_EXTRA_HOSTS", "stirling.legacy.example");
            std::env::remove_var("PAPERRUST_STIRLING_API_URL");
            std::env::remove_var("KUBERNETES_SERVICE_HOST");
            std::env::set_var(
                "PAPERRUST_PUBLIC_URLS",
                "https://archive.lan.test,https://archive.example",
            );
            std::env::set_var(
                "PAPERRUST_OIDC_ISSUERS",
                "https://archive.example=https://auth.example,https://archive.lan.test=https://auth.lan.test",
            );
            std::env::set_var("PAPERRUST_OIDC_ISSUER", "https://auth.lan.test");
            std::env::set_var("PAPERRUST_LAN_URLS", "https://archive.lan.test");
        }
        let out = f();
        unsafe {
            for (key, value) in keys.iter().zip(prev) {
                match value {
                    Some(v) => std::env::set_var(key, v),
                    None => std::env::remove_var(key),
                }
            }
        }
        out
    }

    #[test]
    fn stirling_url_https_and_allowlist() {
        with_stirling_hosts(|| {
            assert_eq!(
                validate_stirling_base("https://stirling.example").unwrap(),
                "https://stirling.example"
            );
            assert_eq!(
                validate_stirling_base("https://stirling.legacy.example/").unwrap(),
                "https://stirling.legacy.example"
            );
            assert_eq!(
                validate_stirling_base("http://stirling.example"),
                Err("stirling_https_required")
            );
            assert_eq!(
                validate_stirling_base("https://evil.example"),
                Err("stirling_host_not_allowed")
            );
            assert_eq!(
                validate_stirling_base("https://user:pass@stirling.example"),
                Err("stirling_url_credentials_forbidden")
            );
            assert_eq!(
                validate_stirling_base("https://stirling.example/api?x=1"),
                Err("stirling_url_must_be_origin")
            );
        });
    }

    #[test]
    fn internal_api_base_is_http_only_for_exact_service() {
        assert_eq!(
            validate_stirling_api_base("http://stirling-pdf.apps.svc:8080").unwrap(),
            "http://stirling-pdf.apps.svc:8080"
        );
        assert!(validate_stirling_api_base("http://stirling-pdf.apps.svc.evil:8080").is_err());
        assert!(validate_stirling_api_base("http://stirling.example").is_err());
        assert!(validate_stirling_api_base("http://user:pw@stirling-pdf.apps.svc:8080").is_err());
        assert!(validate_stirling_api_base("http://stirling-pdf.apps.svc:8080/x").is_err());
        // The browser-facing editor base never accepts the internal http host.
        assert!(validate_stirling_base("http://stirling-pdf.apps.svc:8080").is_err());
        let url = stirling_storage_url("http://stirling-pdf.apps.svc:8080", "api/v1/storage/files").unwrap();
        assert_eq!(url.as_str(), "http://stirling-pdf.apps.svc:8080/api/v1/storage/files");
        assert!(stirling_storage_url("http://stirling-pdf.apps.svc:8080", "/etc/passwd").is_err());
    }

    #[test]
    fn storage_url_stays_on_allowlisted_host() {
        with_stirling_hosts(|| {
            let url = stirling_storage_url("https://stirling.legacy.example", "api/v1/storage/files")
                .expect("join");
            assert_eq!(
                url.as_str(),
                "https://stirling.legacy.example/api/v1/storage/files"
            );
            assert!(
                stirling_storage_url(
                    "https://stirling.legacy.example",
                    "https://evil.example/api/v1/storage/files"
                )
                .is_err()
            );
            assert!(
                stirling_storage_url("https://stirling.legacy.example", "/etc/passwd").is_err()
            );
        });
    }

    #[test]
    fn pdf_filename_keeps_existing() {
        assert_eq!(
            pdf_filename("upload.bin", Some("Facture 2026.pdf")),
            "Facture 2026.pdf"
        );
        assert_eq!(pdf_filename("scan", None), "scan.pdf");
        assert_eq!(pdf_filename("", None), "document.pdf");
    }

    #[test]
    fn download_name_strips_paths() {
        assert_eq!(sanitize_download_name("../../etc/passwd"), "document.pdf");
        assert_eq!(sanitize_download_name("ok-file_1.pdf"), "ok-file_1.pdf");
    }

    #[test]
    fn parse_file_id_variants() {
        assert_eq!(
            parse_stirling_file_id(&serde_json::json!({"id": "abc-123"})),
            Some("abc-123".into())
        );
        assert_eq!(
            parse_stirling_file_id(&serde_json::json!({"fileId": "file1"})),
            Some("file1".into())
        );
        assert_eq!(
            parse_stirling_file_id(&serde_json::json!({"id": "../x"})),
            None
        );
    }

    #[test]
    fn stirling_origin_cors_allowlist() {
        with_stirling_hosts(|| {
            assert!(is_allowed_stirling_origin("https://stirling.example"));
            assert!(is_allowed_stirling_origin("https://stirling.legacy.example"));
            assert!(!is_allowed_stirling_origin("http://stirling.example"));
            assert!(!is_allowed_stirling_origin("https://evil.example"));
        });
    }

    #[test]
    fn file_cors_is_get_only_without_credentials() {
        use axum::http::header::{
            ACCESS_CONTROL_ALLOW_CREDENTIALS, ACCESS_CONTROL_ALLOW_ORIGIN, ORIGIN,
        };
        use axum::response::IntoResponse;

        with_stirling_hosts(|| {
            let mut headers = HeaderMap::new();
            headers.insert(ORIGIN, "https://stirling.example".parse().unwrap());

            let mut get_res = StatusCode::OK.into_response();
            apply_stirling_file_cors(&Method::GET, &headers, &mut get_res);
            assert_eq!(
                get_res
                    .headers()
                    .get(ACCESS_CONTROL_ALLOW_ORIGIN)
                    .and_then(|v| v.to_str().ok()),
                Some("https://stirling.example")
            );
            assert!(get_res.headers().get(ACCESS_CONTROL_ALLOW_CREDENTIALS).is_none());

            for method in [Method::HEAD, Method::OPTIONS, Method::POST] {
                let mut res = StatusCode::OK.into_response();
                apply_stirling_file_cors(&method, &headers, &mut res);
                assert!(
                    res.headers().get(ACCESS_CONTROL_ALLOW_ORIGIN).is_none(),
                    "{method}"
                );
                assert!(res.headers().get(ACCESS_CONTROL_ALLOW_CREDENTIALS).is_none());
            }

            headers.insert(ORIGIN, "https://evil.example".parse().unwrap());
            let mut denied = StatusCode::OK.into_response();
            apply_stirling_file_cors(&Method::GET, &headers, &mut denied);
            assert!(denied.headers().get(ACCESS_CONTROL_ALLOW_ORIGIN).is_none());
            assert!(denied.headers().get(ACCESS_CONTROL_ALLOW_CREDENTIALS).is_none());
        });
    }

    #[test]
    fn editor_urls_come_from_env() {
        with_stirling_hosts(|| {
            assert_eq!(stirling_url_for_host("archive.lan.test"), "https://stirling.example");
            assert_eq!(stirling_url_for_host("archive.example"), "https://pdf.example");
            assert_eq!(stirling_url_for_host("127.0.0.1"), "https://pdf.example");
            assert_eq!(
                validate_stirling_base("https://pdf.example/").unwrap(),
                "https://pdf.example"
            );
            assert!(is_allowed_stirling_origin("https://pdf.example"));
            assert!(!is_allowed_stirling_origin("https://evil.example"));
            unsafe {
                std::env::remove_var("PAPERRUST_STIRLING_URL");
                std::env::remove_var("PAPERRUST_STIRLING_URL_LAN");
            }
            assert_eq!(stirling_url_for_host("archive.example"), "");
            assert_eq!(stirling_api_base(), "");
            unsafe { std::env::set_var("KUBERNETES_SERVICE_HOST", "192.0.2.1") };
            assert_eq!(stirling_api_base(), "http://stirling-pdf.apps.svc:8080");
            unsafe {
                std::env::set_var(
                    "PAPERRUST_STIRLING_API_URL",
                    "http://stirling-pdf.apps.svc.cluster.local:8080/",
                );
            }
            assert_eq!(
                stirling_api_base(),
                "http://stirling-pdf.apps.svc.cluster.local:8080"
            );
            unsafe { std::env::set_var("PAPERRUST_STIRLING_API_URL", "http://evil.example:8080") };
            assert_eq!(stirling_api_base(), "http://stirling-pdf.apps.svc:8080");
        });
    }

    #[test]
    fn editor_url_embeds_urlencoded_source_file() {
        let source = "https://archive.example/api/edit/stirling/TOKEN_REDACTED/file";
        let editor = editor_url_with_file_url("https://stirling.example", source);
        assert!(
            editor.starts_with("https://stirling.example/?fileUrl="),
            "unexpected editorUrl shape: {editor}"
        );
        let parsed = Url::parse(&editor).expect("editor url");
        let pairs: Vec<_> = parsed.query_pairs().collect();
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].0, "fileUrl");
        assert_eq!(pairs[0].1.as_ref(), source);
        // percent-encoding must appear in the raw query (not a bare nested URL)
        assert!(
            editor.contains("fileUrl=https%3A%2F%2Farchive.example%2Fapi%2Fedit%2Fstirling%2F"),
            "expected urlencoded fileUrl in {editor}"
        );
        assert!(!editor.contains("?fileUrl=https://"), "fileUrl must be percent-encoded");
    }

    #[test]
    fn token_version_constant_documented() {
        assert_eq!(TOKEN_VERSION, "pr-stirling-v1");
    }
}
