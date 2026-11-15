//! PDF edit sessions for the same-origin PdfCraft editor.
//!
//! 1. Authenticated start returns a signed short-lived session and
//!    `{origin}/pdfcraft/?file=…&host=parent`.
//! 2. Token-gated file bytes (no storage paths) so the editor can fetch the PDF.
//! 3. Commit accepts the edited PDF (multipart), replaces `original_bytes`,
//!    and re-runs the same OCR path as consume / `:id/ocr`.
//!
//! Tokens are HMAC-SHA256 over session claims (`SESSION_SECRET`). Raw filesystem
//! paths are never returned. In-flight sessions from the previous editor are
//! not migrated.

use crate::auth;
use crate::limits::MAX_UPLOAD_BYTES;
use crate::preview::looks_like_pdf;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, body::Body};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::Rng;
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use std::time::{SystemTime, UNIX_EPOCH};
use url::Url;

const TOKEN_VERSION: &str = "pr-pdf-edit-v1";
const DEFAULT_TTL_SECS: u64 = 30 * 60;
const MIN_TTL_SECS: u64 = 60;
const MAX_TTL_SECS: u64 = 60 * 60;
const HMAC_PREFIX: &str = "pr-pdf-edit-v1";

/// Tokens are scoped to one operation. The `file` token is the one that ends
/// up in GET URLs (editor fetch, access logs, browser history) and can only
/// read. Only the `commit` token (POST path / cookie route body) can replace
/// the PDF.
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
        r#"create table if not exists editor_sessions (
            id text primary key,
            document_id int not null references documents(id) on delete cascade,
            owner_id int not null references users(id) on delete cascade,
            filename text not null,
            expires_at timestamptz not null,
            consumed_at timestamptz,
            created_at timestamptz not null default now()
        )"#,
        "create index if not exists editor_sessions_doc_owner_idx on editor_sessions (document_id, owner_id)",
        "create index if not exists editor_sessions_expires_idx on editor_sessions (expires_at)",
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
            tracing::warn!("pdf_edit schema: {e}");
        }
    }
    Ok(())
}

/// Token-gated file / commit — no OIDC cookie. Token is verified in the handler.
pub fn is_token_public_path(path: &str) -> bool {
    let path = path.trim_end_matches('/');
    let Some(rest) = path.strip_prefix("/api/edit/pdfcraft/") else {
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
    let raw = std::env::var("PAPERRUST_PDF_EDIT_SESSION_TTL_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(DEFAULT_TTL_SECS);
    raw.clamp(MIN_TTL_SECS, MAX_TTL_SECS)
}

/// PdfCraft editor deep-link: `{base}/?file={urlencoded sourceFileUrl}&host=parent`.
/// Same signed Paperrust file URL as before (no second token). The query key is
/// `file`, which PdfCraft fetches in the browser. `fileUrl` is never set.
pub fn editor_url_with_file_url(editor_base: &str, source_file_url: &str) -> String {
    let base = editor_base.trim().trim_end_matches('/');
    let mut url = Url::parse(&format!("{base}/")).unwrap_or_else(|_| {
        Url::parse("https://pdf.invalid/pdfcraft/").expect("static fallback origin")
    });
    {
        let mut pairs = url.query_pairs_mut();
        pairs.clear();
        pairs.append_pair("file", source_file_url);
        pairs.append_pair("host", "parent");
    }
    url.to_string()
}

/// Browser-facing origin allowed to fetch the signed file URL.
/// Empty unless `PAPERRUST_PDFCRAFT_ORIGIN` is one https origin. Never `*`.
pub fn configured_pdfcraft_origin() -> String {
    std::env::var("PAPERRUST_PDFCRAFT_ORIGIN")
        .ok()
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| {
            s.starts_with("https://") && !s.contains('*') && Url::parse(s).is_ok()
        })
        .unwrap_or_default()
}

/// Compose service name: one DNS label, no dots. Loopback is test-only.
fn pdfcraft_upstream_host_ok(host: &str) -> bool {
    if cfg!(test) && host == "127.0.0.1" {
        return true;
    }
    let bytes = host.as_bytes();
    if bytes.is_empty() || bytes.len() > 63 {
        return false;
    }
    let edges = bytes[0].is_ascii_alphanumeric() && bytes[bytes.len() - 1].is_ascii_alphanumeric();
    edges
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}

/// Static PdfCraft beside this process. HTTP only, no path, user, query, or fragment.
pub fn pdfcraft_upstream() -> String {
    let fallback = "http://pdfcraft";
    let raw = std::env::var("PAPERRUST_PDFCRAFT_UPSTREAM").unwrap_or_else(|_| fallback.into());
    let trimmed = raw.trim().trim_end_matches('/').to_string();
    let Ok(url) = Url::parse(&trimmed) else {
        return fallback.into();
    };
    let host = url.host_str().unwrap_or("");
    let path_ok = url.path().is_empty() || url.path() == "/";
    let port_ok = url.port().is_none() || (cfg!(test) && host == "127.0.0.1");
    if url.scheme() == "http"
        && pdfcraft_upstream_host_ok(host)
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && path_ok
        && port_ok
    {
        // Serialize the parsed origin. syn16 forbids format!("http://{host}")
        // outside the validated constructors. Default ports are omitted.
        let mut clean = url;
        clean.set_path("");
        clean.set_query(None);
        clean.set_fragment(None);
        return clean.to_string().trim_end_matches('/').to_string();
    }
    fallback.into()
}

/// Map a same-origin `/pdfcraft…` request onto the static site. Rejects `..`.
pub fn pdfcraft_upstream_path(request_path: &str) -> Result<String, ()> {
    let Some(rest) = request_path.strip_prefix("/pdfcraft") else {
        return Err(());
    };
    let lower = rest.to_ascii_lowercase();
    if rest.contains("..") || rest.contains('\\') || rest.contains('\0') || lower.contains('%') {
        return Err(());
    }
    if rest.is_empty() || rest == "/" {
        return Ok("/index.html".into());
    }
    if !rest.starts_with('/') {
        return Err(());
    }
    Ok(rest.to_string())
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
    rand::rng().fill_bytes(&mut buf);
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
        r#"update editor_sessions
           set consumed_at = now()
           where document_id = $1 and owner_id = $2 and consumed_at is null"#,
    )
    .bind(document_id)
    .bind(owner_id)
    .execute(pool)
    .await;

    sqlx::query(
        r#"insert into editor_sessions
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

    let origin = auth::app_origin_for(request_host);
    let source_file_url = format!("{origin}/api/edit/pdfcraft/{file_token}/file");
    // Same origin as the Paperrust page so PdfCraft can post pdfcraft:save.
    let editor_url = editor_url_with_file_url(&format!("{origin}/pdfcraft"), &source_file_url);

    Ok(StartEditResponse {
        session_id: sid,
        token: token.clone(),
        editor_url,
        source_file_url,
        commit_url: format!("{origin}/api/edit/pdfcraft/{token}/commit"),
        expires_at: expires_at.to_rfc3339(),
        filename,
        document_id,
        mime_type: if mime.eq_ignore_ascii_case("application/pdf") {
            mime
        } else {
            "application/pdf".into()
        },
    })
}

struct LiveSession {
    claims: EditClaims,
    filename: String,
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
        r#"select filename, consumed_at, expires_at
           from editor_sessions
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

/// CORS for the token file response. ACAO is the configured PdfCraft origin
/// when the request Origin is exactly that origin, or the request Origin when
/// it is another allowlisted atelier host. Never `*`. No session cookie.
pub fn apply_pdf_file_cors(headers: &HeaderMap, res: &mut Response) {
    res.headers_mut().remove(axum::http::header::SET_COOKIE);
    res.headers_mut().insert(
        axum::http::header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    let Some(origin) = headers
        .get(axum::http::header::ORIGIN)
        .and_then(|v| v.to_str().ok())
    else {
        return;
    };
    // File-token CORS is the atelier only. It does not use the credentialed API allowlist.
    if !is_pdfcraft_file_origin(origin) {
        return;
    }
    let allow = origin.to_string();
    if allow == "*" {
        return;
    }
    if let Ok(v) = HeaderValue::from_str(&allow) {
        res.headers_mut()
            .insert(axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN, v);
        res.headers_mut().insert(
            axum::http::header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("GET, HEAD"),
        );
        res.headers_mut().insert(
            axum::http::header::VARY,
            HeaderValue::from_static("Origin"),
        );
        res.headers_mut().remove(axum::http::header::SET_COOKIE);
    }
}

/// The configured https origin may read the signed file URL. No port, no userinfo, never `*`.
/// An empty `PAPERRUST_PDFCRAFT_ORIGIN` allows no cross-origin read. The same-origin editor
/// does not need this header.
pub fn is_pdfcraft_file_origin(origin: &str) -> bool {
    let configured = configured_pdfcraft_origin();
    if configured.is_empty() || configured.contains('*') {
        return false;
    }
    if origin == "*" || origin.contains('*') || origin.contains('@') {
        return false;
    }
    let Ok(url) = Url::parse(origin) else {
        return false;
    };
    if url.scheme() != "https" || url.port().is_some() || !url.username().is_empty() || url.password().is_some()
    {
        return false;
    }
    if !(url.path().is_empty() || url.path() == "/") || url.query().is_some() || url.fragment().is_some() {
        return false;
    }
    origin == configured
}

/// Same-origin iframe. The official web zip boots from an inline module, and
/// that script changes between releases, so this policy allows inline scripts
/// and wasm on `/pdfcraft` only.
pub const PDFCRAFT_CSP: &str = "default-src 'self'; base-uri 'self'; object-src 'none'; frame-ancestors 'self'; img-src 'self' data: blob:; font-src 'self'; style-src 'self' 'unsafe-inline'; script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval'; connect-src 'self'; worker-src 'self' blob:";

/// Fetch the pinned static site. The query string (the file token) is not forwarded.
pub async fn proxy_pdfcraft(method_is_head: bool, request_path: &str) -> Response {
    let Ok(upstream_path) = pdfcraft_upstream_path(request_path) else {
        return (StatusCode::BAD_REQUEST, "bad path").into_response();
    };
    let url = format!("{}{upstream_path}", pdfcraft_upstream());
    let client = match reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(60))
        .build()
    {
        Ok(c) => c,
        Err(_) => return (StatusCode::BAD_GATEWAY, "pdfcraft").into_response(),
    };
    let method = if method_is_head {
        reqwest::Method::HEAD
    } else {
        reqwest::Method::GET
    };
    let sent = client.request(method, &url).send().await;
    let Ok(upstream) = sent else {
        return (StatusCode::BAD_GATEWAY, "pdfcraft").into_response();
    };
    if upstream.status().is_redirection() {
        return (StatusCode::BAD_GATEWAY, "pdfcraft").into_response();
    }
    let status = StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let content_type = upstream
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let cache = upstream
        .headers()
        .get(axum::http::header::CACHE_CONTROL)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let body = if method_is_head {
        Vec::new()
    } else {
        match upstream.bytes().await {
            Ok(b) => b.to_vec(),
            Err(_) => return (StatusCode::BAD_GATEWAY, "pdfcraft").into_response(),
        }
    };
    let mut builder = Response::builder().status(status);
    let wasm = upstream_path.ends_with(".wasm");
    let ct = if wasm {
        "application/wasm"
    } else {
        content_type.as_deref().unwrap_or("application/octet-stream")
    };
    builder = builder.header(axum::http::header::CONTENT_TYPE, ct);
    if let Some(cache) = cache {
        builder = builder.header(axum::http::header::CACHE_CONTROL, cache);
    }
    builder = builder.header(axum::http::header::REFERRER_POLICY, "no-referrer");
    builder
        .body(Body::from(body))
        .unwrap_or_else(|_| (StatusCode::BAD_GATEWAY, "pdfcraft").into_response())
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
) -> Result<CommitEditResponse, StartError> {
    let session = load_live_session(pool, token, TokenOp::Commit).await?;
    let Some((bytes, name)) = uploaded else {
        return Err(StartError::bad("missing_file"));
    };
    let filename = pdf_filename(&name, Some(&session.filename));
    finish_replace(pool, &session.claims.sid, session.claims.document_id, session.claims.owner_id, bytes, &filename).await
}

/// Cookie-authenticated commit for the latest live session on this document.
pub async fn commit_for_document(
    pool: &PgPool,
    owner_id: i32,
    document_id: i32,
    token: Option<&str>,
    uploaded: Option<(Vec<u8>, String)>,
) -> Result<CommitEditResponse, StartError> {
    if let Some(token) = token.map(str::trim).filter(|s| !s.is_empty()) {
        let session = load_live_session(pool, token, TokenOp::Commit).await?;
        if session.claims.document_id != document_id || session.claims.owner_id != owner_id {
            return Err(StartError::not_found());
        }
        return commit_session(pool, token, uploaded).await;
    }
    let row = sqlx::query(
        r#"select id from editor_sessions
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
    // Cookie path uses the live row + owner check. The public token path still
    // requires a fresh HMAC (`commit_session`).
    commit_session_by_sid(pool, &sid, owner_id, document_id, uploaded).await
}

async fn commit_session_by_sid(
    pool: &PgPool,
    sid: &str,
    owner_id: i32,
    document_id: i32,
    uploaded: Option<(Vec<u8>, String)>,
) -> Result<CommitEditResponse, StartError> {
    let row = sqlx::query(
        r#"select filename, consumed_at, expires_at
           from editor_sessions
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
    let Some((bytes, name)) = uploaded else {
        return Err(StartError::bad("missing_file"));
    };
    let filename = pdf_filename(&name, Some(&filename));
    finish_replace(pool, sid, document_id, owner_id, bytes, &filename).await
}

async fn finish_replace(
    pool: &PgPool,
    sid: &str,
    document_id: i32,
    owner_id: i32,
    bytes: Vec<u8>,
    filename: &str,
) -> Result<CommitEditResponse, StartError> {
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
    let ocr = match crate::consume::replace_document_pdf(pool, document_id, owner_id, bytes, filename).await
    {
        Ok(ocr) => ocr,
        Err(e) => {
            tracing::warn!(error = %e, "pdf replace+ocr failed");
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

/// M2 (Syn RO !36): atomically claim the session *before* replacing the PDF.
/// Only one concurrent commit (or replay while OCR runs) can win the claim.
async fn claim_session(pool: &PgPool, sid: &str) -> Result<(), StartError> {
    let claimed: Option<String> = sqlx::query_scalar(
        r#"update editor_sessions
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
    let _ = sqlx::query("update editor_sessions set consumed_at = null where id = $1")
        .bind(sid)
        .execute(pool)
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
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
            "/api/edit/pdfcraft/abcDEF0123456789._-/file"
        ));
        assert!(is_token_public_path(
            "/api/edit/pdfcraft/abcDEF0123456789._-/commit/"
        ));
        assert!(!is_token_public_path("/api/edit/pdfcraft/abcDEF0123456789/open"));
        assert!(!is_token_public_path("/api/documents/1/edit/pdfcraft"));
        assert!(!is_token_public_path("/api/edit/pdfcraft/../file"));
        assert!(!is_token_public_path("/api/edit/pdfcraft/short/file"));
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
    fn editor_url_embeds_urlencoded_source_file() {
        let source = "https://archive.example/api/edit/pdfcraft/TOKEN_REDACTED/file";
        let editor = editor_url_with_file_url("https://archive.example/pdfcraft", source);
        assert!(
            editor.starts_with("https://archive.example/pdfcraft/?file="),
            "unexpected editorUrl shape: {editor}"
        );
        assert!(!editor.contains("fileUrl"), "fileUrl must not be used: {editor}");
        let parsed = Url::parse(&editor).expect("editor url");
        assert_eq!(parsed.query_pairs().find(|(k, _)| k == "file").unwrap().1.as_ref(), source);
        assert_eq!(parsed.query_pairs().find(|(k, _)| k == "host").unwrap().1.as_ref(), "parent");
        assert!(
            editor.contains("file=https%3A%2F%2Farchive.example%2Fapi%2Fedit%2Fpdfcraft%2F"),
            "expected urlencoded file in {editor}"
        );
    }

    #[test]
    fn file_cors_matches_configured_origin_and_drops_cookie() {
        let _g = crate::instance::env_lock().lock().unwrap();
        let prev = std::env::var("PAPERRUST_PDFCRAFT_ORIGIN").ok();
        unsafe { std::env::set_var("PAPERRUST_PDFCRAFT_ORIGIN", "https://archive.example") };
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::ORIGIN,
            HeaderValue::from_static("https://archive.example"),
        );
        let mut res = Response::builder().body(Body::empty()).unwrap();
        res.headers_mut().insert(
            axum::http::header::SET_COOKIE,
            HeaderValue::from_static("session=nope"),
        );
        apply_pdf_file_cors(&headers, &mut res);
        let acao = res
            .headers()
            .get(axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .and_then(|v| v.to_str().ok())
            .unwrap();
        assert_eq!(acao, "https://archive.example");
        assert_ne!(acao, "*");
        assert!(res.headers().get(axum::http::header::SET_COOKIE).is_none());
        assert_eq!(
            res.headers()
                .get(axum::http::header::REFERRER_POLICY)
                .and_then(|v| v.to_str().ok()),
            Some("no-referrer")
        );
        unsafe {
            match prev {
                Some(v) => std::env::set_var("PAPERRUST_PDFCRAFT_ORIGIN", v),
                None => std::env::remove_var("PAPERRUST_PDFCRAFT_ORIGIN"),
            }
        }
    }

    #[test]
    fn pdfcraft_path_rejects_dotdot() {
        assert_eq!(pdfcraft_upstream_path("/pdfcraft").unwrap(), "/index.html");
        assert_eq!(pdfcraft_upstream_path("/pdfcraft/").unwrap(), "/index.html");
        assert_eq!(
            pdfcraft_upstream_path("/pdfcraft/pdfcraft-web.wasm").unwrap(),
            "/pdfcraft-web.wasm"
        );
        assert!(pdfcraft_upstream_path("/pdfcraft/../secret").is_err());
        assert!(pdfcraft_upstream_path("/pdfcraft/%2e%2e/secret").is_err());
        assert!(pdfcraft_upstream_path("/pdfcraft/%2E%2E/secret").is_err());
        assert!(pdfcraft_upstream_path("/other").is_err());
    }

    #[test]
    fn pdfcraft_file_cors_is_atelier_only_and_proxy_drops_redirects() {
        let _g = crate::instance::env_lock().lock().unwrap();
        let prev_origin = std::env::var("PAPERRUST_PDFCRAFT_ORIGIN").ok();
        let prev_up = std::env::var("PAPERRUST_PDFCRAFT_UPSTREAM").ok();
        unsafe {
            std::env::set_var("PAPERRUST_PDFCRAFT_ORIGIN", "https://archive.example");
        }
        assert!(is_pdfcraft_file_origin("https://archive.example"));
        assert!(!is_pdfcraft_file_origin("https://evil.example"));
        assert!(!is_pdfcraft_file_origin("https://archive.example:444"));
        unsafe {
            std::env::set_var("PAPERRUST_PDFCRAFT_ORIGIN", "*");
        }
        assert!(!is_pdfcraft_file_origin("https://archive.example"));
        unsafe {
            std::env::set_var("PAPERRUST_PDFCRAFT_UPSTREAM", "http://169.254.169.254/latest");
        }
        assert_eq!(pdfcraft_upstream(), "http://pdfcraft");
        unsafe {
            std::env::set_var("PAPERRUST_PDFCRAFT_UPSTREAM", "http://pdfcraft:8080");
        }
        assert_eq!(pdfcraft_upstream(), "http://pdfcraft");

        let evil_hit = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let evil_flag = evil_hit.clone();
        let evil = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let evil_addr = evil.local_addr().unwrap();
        std::thread::spawn(move || {
            let Ok((mut sock, _)) = evil.accept() else { return };
            evil_flag.store(true, std::sync::atomic::Ordering::SeqCst);
            let mut buf = [0u8; 2048];
            let _ = std::io::Read::read(&mut sock, &mut buf);
            let _ = std::io::Write::write_all(
                &mut sock,
                b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nPWNED",
            );
        });
        let redir_hit = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let redir_flag = redir_hit.clone();
        let redir = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let redir_addr = redir.local_addr().unwrap();
        let evil_port = evil_addr.port();
        std::thread::spawn(move || {
            let Ok((mut sock, _)) = redir.accept() else { return };
            redir_flag.store(true, std::sync::atomic::Ordering::SeqCst);
            let mut buf = [0u8; 2048];
            let _ = std::io::Read::read(&mut sock, &mut buf);
            let body = format!(
                "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{evil_port}/secret\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            let _ = std::io::Write::write_all(&mut sock, body.as_bytes());
        });
        unsafe {
            std::env::set_var(
                "PAPERRUST_PDFCRAFT_UPSTREAM",
                format!("http://127.0.0.1:{}", redir_addr.port()),
            );
        }
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let res = rt.block_on(proxy_pdfcraft(false, "/pdfcraft/index.html"));
        assert!(
            redir_hit.load(std::sync::atomic::Ordering::SeqCst),
            "proxy did not contact the pinned upstream"
        );
        assert!(
            !evil_hit.load(std::sync::atomic::Ordering::SeqCst),
            "proxy followed a redirect off the pinned host"
        );
        assert_eq!(res.status(), StatusCode::BAD_GATEWAY);
        unsafe {
            match prev_origin {
                Some(v) => std::env::set_var("PAPERRUST_PDFCRAFT_ORIGIN", v),
                None => std::env::remove_var("PAPERRUST_PDFCRAFT_ORIGIN"),
            }
            match prev_up {
                Some(v) => std::env::set_var("PAPERRUST_PDFCRAFT_UPSTREAM", v),
                None => std::env::remove_var("PAPERRUST_PDFCRAFT_UPSTREAM"),
            }
        }
    }

    #[test]
    fn token_version_constant_documented() {
        assert_eq!(TOKEN_VERSION, "pr-pdf-edit-v1");
    }
}
