use axum::Json;
use axum::extract::FromRequestParts;
use axum::http::header::{COOKIE, HOST, LOCATION, SET_COOKIE};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngCore;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use url::Url;

const SESSION_COOKIE_DEFAULT: &str = "paperrust_session";
const STATE_COOKIE: &str = "paperrust_oidc_state";
const SESSION_TTL_SECS: i64 = 60 * 60 * 24 * 14;
const PENDING_TTL_SECS: i64 = 60 * 15;
const SLUG_DEFAULT: &str = "app";

fn oidc_slug() -> String {
    // OIDC application slug. Set PAPERRUST_OIDC_CLIENT_ID; this default is only
    // a placeholder when discovery must invent a path.
    client_id().unwrap_or_else(|| SLUG_DEFAULT.to_string())
}

#[derive(Clone, Debug)]
pub struct AuthUser {
    pub id: i32,
    pub email: String,
    pub name: String,
    pub sub: String,
}

impl<S> FromRequestParts<S> for AuthUser
where
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<AuthUser>()
            .cloned()
            .ok_or_else(unauthorized)
    }
}

pub fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({"error": "unauthorized"})),
    )
        .into_response()
}

pub fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({"error": "not_found"})),
    )
        .into_response()
}

/// Stirling edit file/commit — HMAC token is checked in the handler (no cookie).
pub fn is_stirling_edit_token_path(path: &str) -> bool {
    let Some(rest) = path.strip_prefix("/api/edit/stirling/") else {
        return false;
    };
    let Some((token, action)) = rest.split_once('/') else {
        return false;
    };
    let len = token.len();
    (16..=256).contains(&len)
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
        && matches!(action, "file" | "commit")
}

/// LAN realm: used to pick the LAN vs public Stirling host.
pub fn is_lan_request_host(host: &str) -> bool {
    matches!(host_realm(host), Realm::Lan)
}

pub fn is_public_path(path: &str) -> bool {
    let path = path.trim_end_matches("/");
    let path = if path.is_empty() { "/" } else { path };
    if is_stirling_edit_token_path(path) {
        return true;
    }
    matches!(
        path,
        "/health"
            | "/api/health"
            | "/api/config"
            | "/api/login"
            | "/api/logout"
            | "/api/auth"
            | "/api/auth/login"
            | "/api/auth/logout"
            | "/api/auth/callback"
            | "/login/callback"
            // Bearer / Token API auth is handled inside ingest handlers (no session cookie).
            | "/api/v1/ingest"
            | "/api/documents/post_document"
            // Cron purge — gated by PAPERRUST_CRON_TOKEN header inside the handler.
            | "/api/cron/purge-trash"
    )
}

pub async fn ensure_schema(pool: &PgPool) -> anyhow::Result<()> {
    let stmts = [
        r#"create table if not exists users (
            id serial primary key,
            oidc_sub text not null unique,
            email text not null,
            name text not null,
            created_at timestamptz not null default now()
        )"#,
        "alter table users add column if not exists last_login_at timestamptz",
        r#"create table if not exists paperrust_bootstrap_log (
            id int primary key check (id = 1),
            applied_at timestamptz not null default now()
        )"#,
        r#"create table if not exists auth_sessions (
            token text primary key,
            user_id int not null references users(id) on delete cascade,
            issuer_base text not null,
            id_token text,
            expires_at timestamptz not null,
            created_at timestamptz not null default now()
        )"#,
        r#"create table if not exists oidc_pending (
            state text primary key,
            code_verifier text not null,
            nonce text not null,
            issuer_base text not null,
            redirect_uri text not null,
            expires_at timestamptz not null
        )"#,
        "alter table documents add column if not exists owner_id int references users(id)",
        "alter table tags add column if not exists owner_id int references users(id)",
        "alter table document_types add column if not exists owner_id int references users(id)",
        "alter table correspondents add column if not exists owner_id int references users(id)",
        "alter table storage_paths add column if not exists owner_id int references users(id)",
        "alter table custom_fields add column if not exists owner_id int references users(id)",
        "alter table saved_views add column if not exists owner_id int references users(id)",
        "alter table workflows add column if not exists owner_id int references users(id)",
        "alter table mail_accounts add column if not exists owner_id int references users(id)",
        "alter table consume_jobs add column if not exists owner_id int references users(id)",
        "alter table notes add column if not exists owner_id int references users(id)",
        "update documents set owner_id = 1 where owner_id is null",
        "update tags set owner_id = 1 where owner_id is null",
        "update document_types set owner_id = 1 where owner_id is null",
        "update correspondents set owner_id = 1 where owner_id is null",
        "update storage_paths set owner_id = 1 where owner_id is null",
        "update custom_fields set owner_id = 1 where owner_id is null",
        "update saved_views set owner_id = 1 where owner_id is null",
        "update workflows set owner_id = 1 where owner_id is null",
        "update mail_accounts set owner_id = 1 where owner_id is null",
        "update consume_jobs set owner_id = 1 where owner_id is null",
        "update notes set owner_id = 1 where owner_id is null",
    ];
    for sql in stmts {
        if let Err(e) = sqlx::query(sql).execute(pool).await {
            tracing::warn!("auth schema: {e}");
        }
    }
    if let Err(e) = ensure_bootstrap_user(pool).await {
        tracing::warn!("bootstrap user: {e}");
    }
    // Sequence after a possible explicit id=1 insert.
    let _ = sqlx::query(
        "select setval('users_id_seq', (select greatest(coalesce(max(id), 1), 1) from users))",
    )
    .execute(pool)
    .await;
    for sql in [
        "alter table documents alter column owner_id set not null",
        "alter table tags alter column owner_id set not null",
        "alter table document_types alter column owner_id set not null",
        "alter table correspondents alter column owner_id set not null",
        "alter table storage_paths alter column owner_id set not null",
        "alter table custom_fields alter column owner_id set not null",
        "alter table saved_views alter column owner_id set not null",
        "alter table workflows alter column owner_id set not null",
        "alter table mail_accounts alter column owner_id set not null",
        "alter table notes alter column owner_id set not null",
        "alter table tags drop constraint if exists tags_name_key",
        "alter table correspondents drop constraint if exists correspondents_name_key",
        "alter table document_types drop constraint if exists document_types_name_key",
        "alter table storage_paths drop constraint if exists storage_paths_name_key",
        "alter table custom_fields drop constraint if exists custom_fields_name_key",
        "alter table documents drop constraint if exists documents_asn_key",
        "drop index if exists tags_name_key",
        "drop index if exists correspondents_name_key",
        "drop index if exists document_types_name_key",
        "drop index if exists storage_paths_name_key",
        "drop index if exists documents_asn_key",
        "create unique index if not exists tags_owner_name_ci on tags (owner_id, lower(name))",
        "create unique index if not exists correspondents_owner_name_ci on correspondents (owner_id, lower(name))",
        "create unique index if not exists document_types_owner_name_ci on document_types (owner_id, lower(name))",
        "create unique index if not exists storage_paths_owner_name_ci on storage_paths (owner_id, lower(name))",
        "create unique index if not exists saved_views_owner_name_ci on saved_views (owner_id, lower(name))",
        "create unique index if not exists custom_fields_owner_name_ci on custom_fields (owner_id, lower(name))",
        "create unique index if not exists documents_owner_asn on documents (owner_id, asn) where asn is not null",
        "create index if not exists documents_owner_id_idx on documents (owner_id)",
        "create index if not exists tags_owner_id_idx on tags (owner_id)",
        "create index if not exists document_types_owner_id_idx on document_types (owner_id)",
        "create index if not exists correspondents_owner_id_idx on correspondents (owner_id)",
        "create index if not exists auth_sessions_user_id_idx on auth_sessions (user_id)",
    ] {
        if let Err(e) = sqlx::query(sql).execute(pool).await {
            tracing::warn!("auth schema constraint: {e}");
        }
    }
    tracing::info!("auth schema ready (users + owner_id)");
    Ok(())
}

/// Create the first user from `PAPERRUST_BOOTSTRAP_*` when the table is empty,
/// and only with `PAPERRUST_BOOTSTRAP=1` ().
/// Rewriting an existing user 1 is allowed only while it is an unclaimed
/// `bootstrap:` row that never logged in (`last_login_at` null, no session),
/// at most once (`paperrust_bootstrap_log`), and it is logged.
async fn ensure_bootstrap_user(pool: &PgPool) -> anyhow::Result<()> {
    let Some(bootstrap) = crate::instance::bootstrap_identity() else {
        return Ok(());
    };
    let mut tx = pool.begin().await?;
    let inserted = sqlx::query(
        "insert into users (id, oidc_sub, email, name) \
         select 1, $1, $2, $3 where not exists (select 1 from users)",
    )
    .bind(&bootstrap.subject)
    .bind(&bootstrap.email)
    .bind(&bootstrap.name)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if inserted == 1 {
        sqlx::query("insert into paperrust_bootstrap_log (id) values (1) on conflict do nothing")
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        tracing::warn!("bootstrap: created user 1 from PAPERRUST_BOOTSTRAP_* (empty users table)");
        return Ok(());
    }
    let row: Option<(String, bool, bool)> = sqlx::query_as(
        "select u.oidc_sub, \
                (u.last_login_at is not null \
                 or exists (select 1 from auth_sessions s where s.user_id = u.id)), \
                exists (select 1 from paperrust_bootstrap_log) \
         from users u where u.id = 1 for update",
    )
    .fetch_optional(&mut *tx)
    .await?;
    let Some((current_sub, ever_logged_in, already_applied)) = row else {
        return Ok(());
    };
    if !crate::instance::bootstrap_rewrite_allowed(true, &current_sub, ever_logged_in, already_applied) {
        if current_sub.starts_with("bootstrap:") {
            tracing::warn!(
                "bootstrap: user 1 rewrite refused (ever_logged_in={ever_logged_in}, already_applied={already_applied})"
            );
        }
        return Ok(());
    }
    let res = sqlx::query(
        "update users set oidc_sub = $1, email = $2, name = $3 \
         where id = 1 and oidc_sub like 'bootstrap:%' and last_login_at is null",
    )
    .bind(&bootstrap.subject)
    .bind(&bootstrap.email)
    .bind(&bootstrap.name)
    .execute(&mut *tx)
    .await?;
    if res.rows_affected() == 1 {
        sqlx::query("insert into paperrust_bootstrap_log (id) values (1) on conflict do nothing")
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        tracing::warn!("bootstrap: user 1 rewritten from PAPERRUST_BOOTSTRAP_* (once; logged)");
    }
    Ok(())
}

fn first_host_header(headers: &HeaderMap, key: axum::http::HeaderName) -> String {
    headers
        .get(key)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .split(',')
        .next()
        .unwrap_or("")
        .trim()
        .to_string()
}

/// Trust X-Forwarded-For / -Proto only when the TCP peer is exactly 127.0.0.1
/// (after IPv4-mapped canonicalize). A non-loopback peer must not select a
/// public origin. X-Forwarded-Host is never used ().
pub fn peer_trusts_xfh(peer: Option<std::net::IpAddr>) -> bool {
    match peer {
        Some(std::net::IpAddr::V4(v4)) => v4 == std::net::Ipv4Addr::LOCALHOST,
        Some(std::net::IpAddr::V6(v6)) => {
            v6.to_ipv4_mapped() == Some(std::net::Ipv4Addr::LOCALHOST)
        }
        None => false,
    }
}

/// a public-realm Host from a peer that is not the trusted loopback
/// proxy. The request is refused (400) instead of being remapped to a LAN origin.
pub fn untrusted_public_host(headers: &HeaderMap, peer: Option<std::net::IpAddr>) -> bool {
    if peer_trusts_xfh(peer) {
        return false;
    }
    let host = first_host_header(headers, HOST);
    matches!(host_realm(&host), Realm::Public)
}

/// from the trusted loopback peer (tunnel sidecar), Host must be a
/// `PAPERRUST_PUBLIC_URLS` host (or a loopback name for local dev / probes on
/// loopback). Anything else is refused with 400. X-Forwarded-Host plays no part.
pub fn loopback_host_unlisted(headers: &HeaderMap, peer: Option<std::net::IpAddr>) -> bool {
    if !peer_trusts_xfh(peer) {
        return false;
    }
    let host = first_host_header(headers, HOST);
    let name = host_name(&host);
    if name.is_empty() || crate::instance::is_loopback_host(&name) {
        return false;
    }
    crate::instance::find_public_url(&name).is_none()
}

#[allow(dead_code)]
fn peer_looks_private(peer: Option<std::net::IpAddr>) -> bool {
    match peer {
        Some(std::net::IpAddr::V4(v4)) => v4.is_private(),
        Some(std::net::IpAddr::V6(v6)) => v6
            .to_ipv4_mapped()
            .map(|v4| v4.is_private())
            .unwrap_or(false),
        None => false,
    }
}

/// Pick the request Host used for realm / OIDC issuer.
///
/// A public origin (see `instance`) is honored only when the TCP peer is a
/// trusted proxy. An untrusted peer that presents one is refused with 400 by
/// `reject_untrusted_public_host` before any handler runs (): no origin
/// or issuer is ever inferred from untrusted input. Unknown hosts are not
/// copied into `redirect_uri` (`app_origin_for`).
pub fn request_host(headers: &HeaderMap, peer: Option<std::net::IpAddr>) -> String {
    // X-Forwarded-Host is ignored entirely. Only Host is used
    // (cloudflared sets Host from its route; ingress-nginx passes Host).
    let host = first_host_header(headers, HOST);
    match host_realm(&host) {
        // Public origin only from the trusted loopback proxy (tunnel sidecar).
        Realm::Public if peer_trusts_xfh(peer) => host,
        // Untrusted peer with a public Host: refused upstream (400); fixed
        // loopback default, never an origin picked from the request.
        Realm::Public => "127.0.0.1:8788".into(),
        Realm::Lan => host,
        Realm::Loopback if host.is_empty() => "127.0.0.1:8788".into(),
        Realm::Loopback => host,
    }
}

/// strip optional `:port` with exact hostname/IP match — never `starts_with("127.0.0.1")`
/// (that would treat `127.0.0.1.evil` as loopback). Same parse used for HSTS Host checks.
pub fn host_name(host: &str) -> String {
    let h = host.trim().to_ascii_lowercase();
    // Bracketed IPv6: `[::1]:443` or `[::1]`
    if let Some(rest) = h.strip_prefix('[') {
        if let Some(end) = rest.find(']') {
            return format!("[{}]", &rest[..end]);
        }
    }
    // Hostname / IPv4 with optional numeric port (avoid splitting bare IPv6).
    match h.rsplit_once(':') {
        Some((name, port))
            if !name.is_empty()
                && !name.contains(':')
                && !port.is_empty()
                && port.chars().all(|c| c.is_ascii_digit()) =>
        {
            name.to_string()
        }
        _ => h,
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Realm {
    Public,
    Lan,
    Loopback,
}

fn host_realm(host: &str) -> Realm {
    let h = host_name(host);
    if h.is_empty() || crate::instance::is_loopback_host(&h) {
        return Realm::Loopback;
    }
    match crate::instance::find_public_url(&h) {
        Some(origin) if crate::instance::origin_is_direct_lan(&origin) => Realm::Lan,
        Some(_) => Realm::Public,
        None => Realm::Loopback,
    }
}

pub fn request_proto(headers: &HeaderMap, host: &str) -> &'static str {
    match host_realm(host) {
        Realm::Public | Realm::Lan => "https",
        Realm::Loopback => {
            let xf = headers
                .get("x-forwarded-proto")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .split(',')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            if xf == "https" { "https" } else { "http" }
        }
    }
}

fn trim_issuer(s: String) -> String {
    s.trim().trim_end_matches('/').to_string()
}

pub fn issuer_base(host: &str) -> String {
    let name = host_name(host);
    if let Some(origin) = crate::instance::find_public_url(&name) {
        if let Some(issuer) = crate::instance::issuer_for_origin(&origin.origin) {
            return issuer;
        }
    }
    crate::instance::oidc_issuer_default().unwrap_or_default()
}

/// !53: neutral default. With neither env var set, only loopback is listed
/// (no hostnames in code); a deployment lists its origins in
/// `PAPERRUST_PUBLIC_URLS`.
const DEFAULT_PUBLIC_URLS: &str = "http://127.0.0.1:8788";
const FAIL_CLOSED_ORIGIN: &str = "http://127.0.0.1:8788";

#[cfg(test)]
thread_local! {
    static PUBLIC_URL_RAW_OVERRIDE: std::cell::Cell<Option<&'static str>> =
        const { std::cell::Cell::new(None) };
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ListedOrigin {
    origin: String,
    host: String,
    port: u16,
}

/// `PAPERRUST_PUBLIC_URLS` wins. `PAPERRUST_PUBLIC_URL` is the alias.
fn resolve_public_url_raw(urls: Option<&str>, url: Option<&str>) -> Option<String> {
    for candidate in [urls, url] {
        if let Some(s) = candidate.map(str::trim).filter(|s| !s.is_empty()) {
            return Some(s.to_string());
        }
    }
    None
}

fn configured_public_url_raw() -> Option<String> {
    #[cfg(test)]
    {
        if let Some(raw) = PUBLIC_URL_RAW_OVERRIDE.with(|c| c.get()) {
            return resolve_public_url_raw(Some(raw), None);
        }
    }
    resolve_public_url_raw(
        std::env::var("PAPERRUST_PUBLIC_URLS").ok().as_deref(),
        std::env::var("PAPERRUST_PUBLIC_URL").ok().as_deref(),
    )
}

fn parse_listed_origin(raw: &str) -> Option<ListedOrigin> {
    let url = Url::parse(raw.trim()).ok()?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return None;
    }
    if !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    let host = url.host_str()?.trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() {
        return None;
    }
    let port = url.port_or_known_default()?;
    let display = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.clone()
    };
    let origin =
        if (url.scheme() == "https" && port == 443) || (url.scheme() == "http" && port == 80) {
            format!("{}://{display}", url.scheme())
        } else {
            format!("{}://{display}:{port}", url.scheme())
        };
    Some(ListedOrigin { origin, host, port })
}

fn parse_origin_list(raw: &str) -> Vec<ListedOrigin> {
    let mut out = Vec::new();
    for part in raw.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some(origin) = parse_listed_origin(part) {
            if !out.iter().any(|existing: &ListedOrigin| existing.origin == origin.origin) {
                out.push(origin);
            }
        }
    }
    out
}

fn listed_origins() -> Vec<ListedOrigin> {
    match configured_public_url_raw() {
        None => parse_origin_list(DEFAULT_PUBLIC_URLS),
        Some(raw) => {
            let parsed = parse_origin_list(&raw);
            if parsed.is_empty() {
                parse_origin_list(FAIL_CLOSED_ORIGIN)
            } else {
                parsed
            }
        }
    }
}

fn explicit_port(host: &str) -> Option<u16> {
    let h = host.trim();
    if let Some(rest) = h.strip_prefix('[') {
        let end = rest.find(']')?;
        let after = rest[end + 1..].strip_prefix(':')?;
        return after.parse().ok();
    }
    let (name, port) = h.rsplit_once(':')?;
    if name.is_empty() || name.contains(':') || port.is_empty() {
        return None;
    }
    if !port.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    port.parse().ok()
}

fn listed_matches(origin: &ListedOrigin, request_host: &str) -> bool {
    let name = host_name(request_host);
    let name = name
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string();
    if name != origin.host {
        return false;
    }
    match explicit_port(request_host) {
        Some(port) => port == origin.port,
        None => true,
    }
}

/// `redirect_uri` for the OIDC authorize and token requests.
/// Taken from `PAPERRUST_PUBLIC_URLS` (`PAPERRUST_PUBLIC_URL` is the alias),
/// never from the raw Host or X-Forwarded-Host.
pub fn redirect_uri_for(host: &str) -> String {
    format!("{}/api/auth/callback", app_origin_for(host))
}

/// Origin of this archive for browser redirects. Exact listed host, else the
/// first `PAPERRUST_PUBLIC_URLS` entry. The request host is only a lookup key.
pub fn app_origin_for(host: &str) -> String {
    let list = listed_origins();
    if let Some(hit) = list.iter().find(|origin| listed_matches(origin, host)) {
        return hit.origin.clone();
    }
    // !53: an unlisted loopback/empty Host stays on loopback (never the first
    // public origin).
    let name = host_name(host);
    if name.is_empty() || crate::instance::is_loopback_host(&name) {
        return FAIL_CLOSED_ORIGIN.to_string();
    }
    list.first()
        .map(|origin| origin.origin.clone())
        .unwrap_or_else(|| FAIL_CLOSED_ORIGIN.to_string())
}

/// Where the browser lands after a successful login.
pub fn post_login_url_for(host: &str) -> String {
    format!("{}/", app_origin_for(host))
}

/// `post_logout_redirect_uri` and the local logout fallback.
pub fn post_logout_redirect_for(host: &str) -> String {
    post_login_url_for(host)
}

pub fn end_session_for(host: &str) -> String {
    format!("{}/application/o/{}/end-session/", issuer_base(host), oidc_slug())
}

pub fn missing_oidc_names() -> Vec<&'static str> {
    let mut v = Vec::new();
    if client_id().is_none() {
        v.push("PAPERRUST_OIDC_CLIENT_ID");
    }
    if client_secret().is_none() {
        v.push("PAPERRUST_OIDC_CLIENT_SECRET");
    }
    v
}

/// always set `Secure` when the TCP peer is not loopback (LAN HTTP
/// binds must not mint cleartext-usable session cookies). Loopback peers keep
/// the previous HTTPS-via-Host/XFP behaviour for local Vite over HTTP.
pub fn cookie_secure(headers: &HeaderMap, peer: Option<std::net::IpAddr>) -> bool {
    if !peer_trusts_xfh(peer) {
        return true;
    }
    request_proto(headers, &request_host(headers, peer)) == "https"
}

/// True when the request carries a session cookie (CSRF gate).
pub fn has_session_cookie(headers: &HeaderMap) -> bool {
    session_cookie_names_to_read()
        .iter()
        .any(|n| cookie_from_headers(headers, n).is_some())
}

fn mutating_method(method: &axum::http::Method) -> bool {
    !matches!(
        *method,
        axum::http::Method::GET | axum::http::Method::HEAD | axum::http::Method::OPTIONS
    )
}

/// cookie-auth mutations need `Sec-Fetch-Site: same-origin` or an
/// exact `Origin` matching this request's host/proto. Bearer-only callers
/// (OIDC access_token or ingest `pr_…`, no session cookie) skip this
/// check the same way ingest already does.
pub fn csrf_ok(
    headers: &HeaderMap,
    peer: Option<std::net::IpAddr>,
    method: &axum::http::Method,
) -> bool {
    if !mutating_method(method) || !has_session_cookie(headers) {
        return true;
    }
    if let Some(site) = headers
        .get("sec-fetch-site")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
    {
        if site.eq_ignore_ascii_case("same-origin") {
            return true;
        }
    }
    let Some(origin) = headers
        .get(axum::http::header::ORIGIN)
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    origin == request_host_origin_for_compare(headers, peer)
}

/// Origin of this request as seen by the server, for **comparison only**
/// (CSRF `Origin` check). Never emitted into a redirect, link or response:
/// browser-facing URLs come from `app_origin_for` (closed list) only.
fn request_host_origin_for_compare(headers: &HeaderMap, peer: Option<std::net::IpAddr>) -> String {
    let host = request_host(headers, peer);
    format!("{}://{}", request_proto(headers, &host), host)
}

pub fn csrf_forbidden() -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(serde_json::json!({"error": "csrf_rejected"})),
    )
        .into_response()
}

fn discovery_urls(base: &str) -> Vec<String> {
    let base = base.trim_end_matches('/');
    let mut urls = vec![format!("{base}/.well-known/openid-configuration")];
    if !base.contains("/application/o/") {
        let slug = oidc_slug();
        urls.push(format!(
            "{base}/application/o/{slug}/.well-known/openid-configuration"
        ));
    }
    urls
}

fn random_b64url(n_bytes: usize) -> String {
    let mut buf = vec![0u8; n_bytes];
    rand::thread_rng().fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

fn pkce_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}

/// OIDC HTTP client. `PAPERRUST_OIDC_CA_BUNDLE`, when set, is added to the
/// trust store for every issuer. Otherwise system roots are used.
/// Never soft-fails TLS (`danger_accept_invalid_certs` removed).
fn http_client(url_or_issuer: &str) -> anyhow::Result<reqwest::Client> {
    crate::jwks::oidc_http_client(url_or_issuer)
}

#[derive(Debug, Deserialize)]
struct Discovery {
    authorization_endpoint: String,
    token_endpoint: String,
    userinfo_endpoint: String,
    #[serde(default)]
    end_session_endpoint: Option<String>,
    #[serde(default)]
    issuer: String,
    #[serde(default)]
    jwks_uri: Option<String>,
}

async fn fetch_discovery(issuer: &str) -> anyhow::Result<Discovery> {
    let client = http_client(issuer)?;
    let mut last = None;
    for url in discovery_urls(issuer) {
        match client.get(&url).send().await {
            Ok(res) if res.status().is_success() => {
                return Ok(res.json().await?);
            }
            Ok(res) => last = Some(format!("{url} -> {}", res.status())),
            Err(e) => last = Some(format!("{url} -> {e}")),
        }
    }
    anyhow::bail!(
        "oidc discovery failed for {issuer} ({}); the OIDC application for this archive must exist",
        last.unwrap_or_else(|| "no url".into())
    )
}

fn env_first(keys: &[&str]) -> Option<String> {
    for k in keys {
        if let Ok(s) = std::env::var(k) {
            let s = s.trim().to_string();
            if !s.is_empty() {
                return Some(s);
            }
        }
    }
    None
}

pub fn client_id() -> Option<String> {
    // PAPERRUST_OIDC_CLIENT_ID only — unprefixed OIDC_CLIENT_ID is ignored.
    env_first(&["PAPERRUST_OIDC_CLIENT_ID"])
}

/// Public OIDC client used by mobile / native interactive API (PKCE).
/// Placeholder `mobile` until `PAPERRUST_OIDC_MOBILE_CLIENT_ID` is set.
pub fn mobile_client_id() -> String {
    env_first(&["PAPERRUST_OIDC_MOBILE_CLIENT_ID"])
        .unwrap_or_else(|| "mobile".to_string())
}

/// Public mobile OIDC issuer (`PAPERRUST_OIDC_MOBILE_ISSUER`).
/// Empty / unset = Bearer mobile disabled (fail-closed).
pub fn mobile_issuer() -> Option<String> {
    env_first(&["PAPERRUST_OIDC_MOBILE_ISSUER"])
}

/// LAN mobile OIDC issuer (`PAPERRUST_OIDC_MOBILE_ISSUER_LAN`).
/// Empty / unset = LAN Bearer mobile disabled (fail-closed).
pub fn mobile_issuer_lan() -> Option<String> {
    env_first(&["PAPERRUST_OIDC_MOBILE_ISSUER_LAN"])
}

pub fn client_secret() -> Option<String> {
    // PAPERRUST_OIDC_CLIENT_SECRET only — unprefixed OIDC_CLIENT_SECRET is ignored.
    env_first(&["PAPERRUST_OIDC_CLIENT_SECRET"])
}

pub fn session_secret() -> Option<String> {
    env_first(&["SESSION_SECRET"])
}

/// refuse missing/weak SESSION_SECRET (HMAC session cookies).
pub const MIN_SESSION_SECRET_LEN: usize = 32;

pub fn assert_session_secret_ready() -> anyhow::Result<()> {
    let Some(s) = session_secret() else {
        anyhow::bail!("SESSION_SECRET is required (≥{MIN_SESSION_SECRET_LEN} chars)");
    };
    if s.len() < MIN_SESSION_SECRET_LEN {
        anyhow::bail!(
            "SESSION_SECRET must be at least {MIN_SESSION_SECRET_LEN} characters (got {})",
            s.len()
        );
    }
    Ok(())
}

/// Bare cookie name from env / default (without `__Host-` prefix).
fn session_cookie_base_name() -> String {
    // Cookie NAME from PAPERRUST_OIDC_SESSION; SESSION_SECRET signs it.
    let raw = env_first(&["PAPERRUST_OIDC_SESSION"])
        .unwrap_or_else(|| SESSION_COOKIE_DEFAULT.to_string());
    raw.strip_prefix("__Host-")
        .or_else(|| raw.strip_prefix("__Secure-"))
        .unwrap_or(&raw)
        .to_string()
}

pub fn session_cookie_name() -> String {
    session_cookie_base_name()
}

/// on HTTPS use `__Host-` + base (Secure + Path=/ + no Domain). HTTP (Vite) keeps bare name.
pub fn session_cookie_name_for(secure: bool) -> String {
    let base = session_cookie_base_name();
    if secure {
        format!("__Host-{base}")
    } else {
        base
    }
}

fn session_cookie_names_to_read() -> Vec<String> {
    let base = session_cookie_base_name();
    let mut names = vec![format!("__Host-{base}"), base.clone()];
    if let Some(raw) = env_first(&["PAPERRUST_OIDC_SESSION"]) {
        if !names.iter().any(|n| n == &raw) {
            names.push(raw);
        }
    }
    names
}

pub fn is_configured() -> bool {
    client_id().is_some() && client_secret().is_some() && session_secret().is_some()
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

fn sign_session_token(token: &str) -> Option<String> {
    let secret = session_secret()?;
    let mac = hmac_sha256(secret.as_bytes(), token.as_bytes());
    Some(format!("{token}.{}", URL_SAFE_NO_PAD.encode(mac)))
}

fn verify_session_cookie(cookie: &str) -> Option<String> {
    let secret = session_secret()?;
    let (token, sig) = cookie.rsplit_once('.')?;
    if token.is_empty() || sig.is_empty() {
        return None;
    }
    let got = URL_SAFE_NO_PAD.decode(sig).ok()?;
    let expected = hmac_sha256(secret.as_bytes(), token.as_bytes());
    if ct_eq(&expected, &got) {
        Some(token.to_string())
    } else {
        None
    }
}

fn cookie_from_headers(headers: &HeaderMap, name: &str) -> Option<String> {
    let raw = headers.get(COOKIE)?.to_str().ok()?;
    for part in raw.split(';') {
        let part = part.trim();
        if let Some((k, v)) = part.split_once('=') {
            if k.trim() == name {
                let v = v.trim();
                if !v.is_empty() {
                    return Some(v.to_string());
                }
            }
        }
    }
    None
}

fn auth_user_from_row(row: sqlx::postgres::PgRow) -> AuthUser {
    AuthUser {
        id: row.get("id"),
        email: row.get("email"),
        name: row.get("name"),
        sub: row.get("oidc_sub"),
    }
}

async fn user_from_session_cookie(pool: &PgPool, headers: &HeaderMap) -> Option<AuthUser> {
    let mut token = None;
    for name in session_cookie_names_to_read() {
        if let Some(cookie) = cookie_from_headers(headers, &name) {
            if let Some(t) = verify_session_cookie(&cookie) {
                token = Some(t);
                break;
            }
        }
    }
    let token = token?;
    let row = sqlx::query(
        r#"select u.id, u.email, u.name, u.oidc_sub
           from auth_sessions s
           join users u on u.id = s.user_id
           where s.token = $1 and s.expires_at > now()"#,
    )
    .bind(&token)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()?;
    Some(auth_user_from_row(row))
}

/// Compact JWT from `Authorization: Bearer` — never `pr_…` ingest tokens.
fn oidc_bearer_from_headers(headers: &HeaderMap) -> Option<String> {
    let value = headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .trim();
    let rest = value
        .strip_prefix("Bearer ")
        .or_else(|| value.strip_prefix("bearer "))?
        .trim();
    if rest.is_empty() || rest.starts_with("pr_") {
        return None;
    }
    let mut parts = rest.split('.');
    if parts.next()?.is_empty() {
        return None;
    }
    if parts.next()?.is_empty() {
        return None;
    }
    if parts.next()?.is_empty() {
        return None;
    }
    if parts.next().is_some() {
        return None;
    }
    Some(rest.to_string())
}

/// Exact configured issuer plus its trailing-slash twin.
/// Never adds bare-host `https://auth.example[/]` or other web application
/// paths — those stay cookie/web only.
fn exact_issuer_forms(raw: &str) -> Vec<String> {
    let no_slash = trim_issuer(raw.to_string());
    if no_slash.is_empty() {
        return Vec::new();
    }
    vec![no_slash.clone(), format!("{no_slash}/")]
}

/// Bearer-only issuer allowlist (Choice B′). Cookie/web discovery is unchanged
/// (`issuer_base` + `SLUG` / `oidc_slug()`).
///
/// Public host → exact `PAPERRUST_OIDC_MOBILE_ISSUER` forms only.
/// LAN / loopback → exact `PAPERRUST_OIDC_MOBILE_ISSUER_LAN` forms only.
/// Unset env → empty (fail-closed: Bearer mobile disabled).
pub fn allowed_issuers_for_host(host: &str) -> Vec<String> {
    let raw = match host_realm(host) {
        Realm::Public => mobile_issuer(),
        Realm::Lan | Realm::Loopback => mobile_issuer_lan(),
    };
    let Some(raw) = raw else {
        return Vec::new();
    };
    let mut out = exact_issuer_forms(&raw);
    out.sort();
    out.dedup();
    out
}

fn unverified_jwt_iss(token: &str) -> Option<String> {
    let payload = token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    v.get("iss")
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

fn issuer_allowed(iss: &str, allowlist: &[String]) -> bool {
    allowlist.iter().any(|a| a == iss)
}

/// After exact `iss ∈ allowlist`, JWKS is `<iss with trailing slash> + "jwks/"`.
/// Returns `None` (no JWKS fetch) when env is unset or `iss` is outside the
/// allowlist — never derive JWKS from `SLUG` / `CLIENT_ID` on the Bearer path.
pub fn resolve_bearer_jwks(host: &str, token: &str) -> Option<(Vec<String>, String)> {
    let allowlist = allowed_issuers_for_host(host);
    if allowlist.is_empty() {
        return None;
    }
    let iss = unverified_jwt_iss(token)?;
    if !issuer_allowed(&iss, &allowlist) {
        return None;
    }
    Some((allowlist, crate::jwks::fallback_jwks_uri(&iss)))
}

#[derive(Debug, PartialEq, Eq)]
struct OidcIdentity {
    sub: String,
    email: String,
    name: String,
}

fn identity_from_claims(claims: &serde_json::Value) -> Option<OidcIdentity> {
    let sub = claims
        .get("sub")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())?
        .to_string();
    let email = claims
        .get("email")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("{sub}@oidc.local"));
    let name = claims
        .get("name")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .or_else(|| {
            claims
                .get("preferred_username")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| email.clone());
    Some(OidcIdentity { sub, email, name })
}

async fn fetch_user_by_sub(pool: &PgPool, sub: &str) -> Option<AuthUser> {
    let row = sqlx::query("select id, email, name, oidc_sub from users where oidc_sub = $1")
        .bind(sub)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()?;
    Some(auth_user_from_row(row))
}

async fn fetch_user_by_id(pool: &PgPool, id: i32) -> Option<AuthUser> {
    let row = sqlx::query("select id, email, name, oidc_sub from users where id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()?;
    Some(auth_user_from_row(row))
}

/// Map verified OIDC claims to `users` the same way the callback does:
/// lookup by `oidc_sub`, else upsert (bootstrap email claim or insert).
async fn user_from_oidc_identity(pool: &PgPool, ident: &OidcIdentity) -> Option<AuthUser> {
    if crate::instance::oidc_sub_is_reserved(&ident.sub) {
        tracing::warn!("oidc bearer: reserved bootstrap subject refused");
        return None;
    }
    if let Some(user) = fetch_user_by_sub(pool, &ident.sub).await {
        return Some(user);
    }
    let user_id = match upsert_user(pool, &ident.sub, &ident.email, &ident.name).await {
        Ok(id) => id,
        Err(e) => {
            tracing::error!("oidc bearer upsert user: {e:#}");
            return None;
        }
    };
    fetch_user_by_id(pool, user_id).await
}

async fn user_from_oidc_bearer(
    pool: &PgPool,
    headers: &HeaderMap,
    peer: Option<std::net::IpAddr>,
) -> Option<AuthUser> {
    let token = oidc_bearer_from_headers(headers)?;
    let host = request_host(headers, peer);
    let (issuers, jwks_uri) = resolve_bearer_jwks(&host, &token)?;
    let audience = mobile_client_id();
    let claims =
        match crate::jwks::verify_access_token(&token, &jwks_uri, &issuers, &audience).await {
            Ok(v) => v,
            Err(e) => {
                tracing::debug!("oidc access_token rejected: {e:#}");
                return None;
            }
        };
    let ident = identity_from_claims(&claims)?;
    user_from_oidc_identity(pool, &ident).await
}

/// Session cookie first (unchanged web path). Else OIDC Bearer `access_token`.
/// `pr_…` personal tokens are not accepted here — ingest handlers only.
pub async fn user_from_headers(
    pool: &PgPool,
    headers: &HeaderMap,
    peer: Option<std::net::IpAddr>,
) -> Option<AuthUser> {
    if let Some(user) = user_from_session_cookie(pool, headers).await {
        return Some(user);
    }
    user_from_oidc_bearer(pool, headers, peer).await
}

/// Marker: this request authenticated with an ingest `pr_…` token.
/// Bulk soft-delete is the only mutation that token may perform.
#[derive(Clone, Copy, Debug)]
pub struct IngestTokenScope;

/// Ingest `pr_…` token → owning user. Not a session and not a general API login.
pub async fn user_from_ingest_token(pool: &PgPool, headers: &HeaderMap) -> Option<AuthUser> {
    let raw = crate::api_tokens::raw_token_from_headers(headers)?;
    let (_token_id, owner_id) = crate::api_tokens::authenticate(pool, &raw).await.ok()??;
    fetch_user_by_id(pool, owner_id).await
}

fn set_named_cookie(name: &str, value: &str, max_age: i64, secure: bool) -> HeaderValue {
    // never set Domain (required for `__Host-`). Secure is mandatory for `__Host-`.
    let mut v = format!("{name}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}");
    if secure || name.starts_with("__Host-") {
        v.push_str("; Secure");
    }
    HeaderValue::from_str(&v).unwrap_or_else(|_| HeaderValue::from_static("cookie"))
}

fn set_cookie_header(token: &str, max_age: i64, secure: bool) -> HeaderValue {
    set_named_cookie(&session_cookie_name_for(secure), token, max_age, secure)
}

fn set_state_cookie(state: &str, secure: bool) -> HeaderValue {
    set_named_cookie(STATE_COOKIE, state, PENDING_TTL_SECS, secure)
}

/// Clear bare + `__Host-` session cookies with matching Path=/ (+ Secure for `__Host-`
/// and for bare when the request was HTTPS). Browsers only drop a cookie when
/// name/Path/Secure attributes match what was Set.
fn clear_session_cookie_headers(secure: bool) -> Vec<HeaderValue> {
    let base = session_cookie_base_name();
    let mut out = vec![
        // Always clear non-Secure bare (Vite / HTTP).
        set_named_cookie(&base, "deleted", 0, false),
        // Always clear `__Host-` (prod HTTPS): requires Secure + Path=/ + no Domain.
        set_named_cookie(&format!("__Host-{base}"), "deleted", 0, true),
    ];
    if secure {
        // Also clear a bare cookie that may have been minted with Secure before __Host-.
        out.push(set_named_cookie(&base, "deleted", 0, true));
    }
    out
}

fn clear_state_cookie(secure: bool) -> HeaderValue {
    set_named_cookie(STATE_COOKIE, "deleted", 0, secure)
}

fn found(location: &str) -> Response {
    let mut res = StatusCode::FOUND.into_response();
    if let Ok(v) = HeaderValue::from_str(location) {
        res.headers_mut().insert(LOCATION, v);
    }
    res
}

fn found_with_cookie(location: &str, cookie: HeaderValue) -> Response {
    found_with_cookies(location, [cookie])
}

fn found_with_cookies(location: &str, cookies: impl IntoIterator<Item = HeaderValue>) -> Response {
    let mut res = found(location);
    for cookie in cookies {
        res.headers_mut().append(SET_COOKIE, cookie);
    }
    res
}

pub async fn start_login(
    pool: &PgPool,
    headers: &HeaderMap,
    peer: Option<std::net::IpAddr>,
    _callback_path: &str,
) -> Response {
    if !is_configured() {
        tracing::error!(
            "oidc not configured (PAPERRUST_OIDC_CLIENT_ID/SECRET or SESSION_SECRET missing)"
        );
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error": "oidc_not_configured"})),
        )
            .into_response();
    }
    let Some(cid) = client_id() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error": "oidc_not_configured"})),
        )
            .into_response();
    };
    let host = request_host(headers, peer);
    let issuer = issuer_base(&host);
    let redirect_uri = redirect_uri_for(&host);
    let discovery = match fetch_discovery(&issuer).await {
        Ok(d) => d,
        Err(e) => {
            tracing::error!("oidc discovery failed: {e:#}");
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({"error": "oidc_discovery_failed"})),
            )
                .into_response();
        }
    };
    let verifier = random_b64url(32);
    let challenge = pkce_challenge(&verifier);
    let state = random_b64url(24);
    let nonce = random_b64url(24);
    let pending_exp = chrono::Utc::now() + chrono::Duration::seconds(PENDING_TTL_SECS);
    if let Err(e) = sqlx::query(
        r#"insert into oidc_pending (state, code_verifier, nonce, issuer_base, redirect_uri, expires_at)
           values ($1,$2,$3,$4,$5,$6)"#,
    )
    .bind(&state)
    .bind(&verifier)
    .bind(&nonce)
    .bind(&issuer)
    .bind(&redirect_uri)
    .bind(pending_exp)
    .execute(pool)
    .await
    {
        tracing::error!("oidc pending insert: {e}");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "oidc_pending"})),
        )
            .into_response();
    }
    let mut url = match Url::parse(&discovery.authorization_endpoint) {
        Ok(u) => u,
        Err(e) => {
            tracing::error!("oidc authorize url: {e}");
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({"error": "oidc_authorize_url"})),
            )
                .into_response();
        }
    };
    url.query_pairs_mut()
        .append_pair("client_id", &cid)
        .append_pair("redirect_uri", &redirect_uri)
        .append_pair("response_type", "code")
        .append_pair("scope", "openid email profile")
        .append_pair("state", &state)
        .append_pair("nonce", &nonce)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256");
    found_with_cookie(
        url.as_str(),
        set_state_cookie(&state, cookie_secure(headers, peer)),
    )
}

#[derive(Deserialize)]
pub struct CallbackQuery {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
    pub error_description: Option<String>,
}

#[derive(Deserialize)]
struct TokenResponse {
    #[serde(default)]
    access_token: String,
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Deserialize)]
struct UserInfo {
    #[serde(default)]
    sub: String,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    preferred_username: Option<String>,
}

pub async fn finish_callback(
    pool: &PgPool,
    headers: &HeaderMap,
    peer: Option<std::net::IpAddr>,
    q: CallbackQuery,
) -> Response {
    if let Some(err) = q.error {
        tracing::warn!(
            "oidc callback error={err} desc={}",
            q.error_description.as_deref().unwrap_or("")
        );
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": err})),
        )
            .into_response();
    }
    let (Some(code), Some(state)) = (q.code, q.state) else {
        return start_login(pool, headers, peer, "/api/auth/callback").await;
    };
    let cookie_state = cookie_from_headers(headers, STATE_COOKIE);
    if cookie_state.as_deref() != Some(state.as_str()) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "invalid_state"})),
        )
            .into_response();
    }
    let _ = sqlx::query("delete from oidc_pending where expires_at < now()")
        .execute(pool)
        .await;
    let pending = sqlx::query(
        r#"delete from oidc_pending where state = $1 and expires_at > now()
           returning code_verifier, nonce, issuer_base, redirect_uri"#,
    )
    .bind(&state)
    .fetch_optional(pool)
    .await;
    let pending = match pending {
        Ok(Some(row)) => row,
        Ok(None) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "invalid_state"})),
            )
                .into_response();
        }
        Err(e) => {
            tracing::error!("oidc pending load: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "oidc_pending"})),
            )
                .into_response();
        }
    };
    let verifier: String = pending.get("code_verifier");
    let expected_nonce: String = pending.get("nonce");
    let issuer_stored: String = pending.get("issuer_base");
    let redirect_uri: String = pending.get("redirect_uri");
    let Some(cid) = client_id() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error": "oidc_not_configured"})),
        )
            .into_response();
    };
    let secret = client_secret();
    let discovery = match fetch_discovery(&issuer_stored).await {
        Ok(d) => d,
        Err(e) => {
            tracing::error!("oidc discovery (callback): {e:#}");
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({"error": "oidc_discovery_failed"})),
            )
                .into_response();
        }
    };
    let client = match http_client(&issuer_stored) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("oidc http client: {e:#}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "oidc_http"})),
            )
                .into_response();
        }
    };
    let mut form = vec![
        ("grant_type", "authorization_code".to_string()),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("client_id", cid.clone()),
        ("code_verifier", verifier),
    ];
    if let Some(s) = secret {
        form.push(("client_secret", s));
    } else {
        tracing::error!("PAPERRUST_OIDC_CLIENT_SECRET missing at token exchange");
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "error": "oidc_client_secret_missing"
            })),
        )
            .into_response();
    }
    let token_res = match client
        .post(&discovery.token_endpoint)
        .header("accept", "application/json")
        .form(&form)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("oidc token request: {e:#}");
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({"error": "oidc_token_failed"})),
            )
                .into_response();
        }
    };
    let token_status = token_res.status();
    let token_body: TokenResponse = match token_res.json().await {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("oidc token json: {e:#} status={token_status}");
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({"error": "oidc_token_failed"})),
            )
                .into_response();
        }
    };
    if let Some(err) = token_body.error {
        tracing::error!("oidc token error={err} status={token_status}");
        return (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({"error": "oidc_token_failed"})),
        )
            .into_response();
    }
    if token_body.access_token.is_empty() {
        tracing::error!("oidc token missing access_token status={token_status}");
        return (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({"error": "oidc_token_failed"})),
        )
            .into_response();
    }
    let info: UserInfo = match client
        .get(&discovery.userinfo_endpoint)
        .bearer_auth(&token_body.access_token)
        .header("accept", "application/json")
        .send()
        .await
    {
        Ok(r) => match r.json().await {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("oidc userinfo json: {e:#}");
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({"error": "oidc_userinfo_failed"})),
                )
                    .into_response();
            }
        },
        Err(e) => {
            tracing::error!("oidc userinfo: {e:#}");
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({"error": "oidc_userinfo_failed"})),
            )
                .into_response();
        }
    };
    // Require id_token: JWKS signature + iss/aud/exp, then nonce (replay) + sub match.
    let Some(id_token) = token_body.id_token.as_deref().filter(|s| !s.is_empty()) else {
        return (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({"error": "oidc_missing_id_token"})),
        )
            .into_response();
    };
    let issuer = if discovery.issuer.trim().is_empty() {
        format!("{}/application/o/{}/", issuer_stored.trim_end_matches('/'), oidc_slug())
    } else {
        discovery.issuer.clone()
    };
    let jwks_uri = discovery
        .jwks_uri
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| crate::jwks::fallback_jwks_uri(&issuer));
    let id_claims = match crate::jwks::verify_id_token(id_token, &jwks_uri, &issuer, &cid).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("oidc id_token jwks verify: {e:#}");
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "oidc_id_token_invalid"})),
            )
                .into_response();
        }
    };
    let token_nonce = id_claims
        .get("nonce")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if token_nonce.is_empty() || !ct_eq(token_nonce.as_bytes(), expected_nonce.as_bytes()) {
        tracing::warn!("oidc nonce mismatch");
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "invalid_nonce"})),
        )
            .into_response();
    }
    // Require a non-empty userinfo `sub` (do not invent identity from an empty claim).
    if info.sub.trim().is_empty() {
        return (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({"error": "oidc_empty_sub"})),
        )
            .into_response();
    }
    let token_sub = id_claims.get("sub").and_then(|v| v.as_str()).unwrap_or("");
    if token_sub.is_empty() || !ct_eq(info.sub.as_bytes(), token_sub.as_bytes()) {
        tracing::warn!("oidc userinfo/id_token sub mismatch");
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "oidc_sub_mismatch"})),
        )
            .into_response();
    }
    let sub = info.sub;
    if crate::instance::oidc_sub_is_reserved(&sub) {
        tracing::warn!("oidc callback: reserved bootstrap subject refused");
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": "oidc_sub_reserved"})),
        )
            .into_response();
    }
    let email = info
        .email
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("{sub}@oidc.local"));
    let name = info
        .name
        .filter(|s| !s.is_empty())
        .or(info.preferred_username)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| email.clone());
    let user_id = match upsert_user(pool, &sub, &email, &name).await {
        Ok(id) => id,
        Err(e) => {
            tracing::error!("oidc upsert user: {e:#}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "oidc_user"})),
            )
                .into_response();
        }
    };
    let token = random_b64url(32);
    let sess_exp = chrono::Utc::now() + chrono::Duration::seconds(SESSION_TTL_SECS);
    if let Err(e) = sqlx::query(
        r#"insert into auth_sessions (token, user_id, issuer_base, id_token, expires_at)
           values ($1,$2,$3,$4,$5)"#,
    )
    .bind(&token)
    .bind(user_id)
    .bind(&issuer_stored)
    .bind(&token_body.id_token)
    .bind(sess_exp)
    .execute(pool)
    .await
    {
        tracing::error!("oidc session insert: {e}");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "oidc_session"})),
        )
            .into_response();
    }
    let host = request_host(headers, peer);
    let landing = post_login_url_for(&host);
    let Some(cookie_val) = sign_session_token(&token) else {
        tracing::error!("SESSION_SECRET missing at session cookie");
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error": "oidc_not_configured"})),
        )
            .into_response();
    };
    found_with_cookies(&landing, [
        set_cookie_header(&cookie_val, SESSION_TTL_SECS, cookie_secure(headers, peer)),
        clear_state_cookie(cookie_secure(headers, peer)),
    ])
}

async fn upsert_user(pool: &PgPool, sub: &str, email: &str, name: &str) -> anyhow::Result<i32> {
    // defense in depth; callers already refuse reserved subjects.
    if crate::instance::oidc_sub_is_reserved(sub) {
        anyhow::bail!("reserved bootstrap subject from OIDC");
    }
    let id = upsert_user_inner(pool, sub, email, name).await?;
    sqlx::query("update users set last_login_at = now() where id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(id)
}

async fn upsert_user_inner(pool: &PgPool, sub: &str, email: &str, name: &str) -> anyhow::Result<i32> {
    if let Some(id) = sqlx::query_scalar::<_, i32>("select id from users where oidc_sub = $1")
        .bind(sub)
        .fetch_optional(pool)
        .await?
    {
        sqlx::query("update users set email = $1, name = $2 where id = $3")
            .bind(email)
            .bind(name)
            .bind(id)
            .execute(pool)
            .await?;
        return Ok(id);
    }
    // Claim bootstrap id=1 only on exact email match, atomically.
    // UPDATE … WHERE oidc_sub LIKE 'bootstrap:%'; 0 rows → insert (lost race / already claimed).
    let mut tx = pool.begin().await?;
    let admin: Option<(i32, String, String)> = sqlx::query_as::<_, (i32, String, String)>(
        "select id, oidc_sub, email from users where id = 1 for update",
    )
    .fetch_optional(&mut *tx)
    .await?;
    if let Some((_id, existing_sub, seeded_email)) = admin {
        let bootstrap = existing_sub.starts_with("bootstrap:");
        let email_ok = !seeded_email.is_empty() && email.eq_ignore_ascii_case(seeded_email.trim());
        if bootstrap && email_ok {
            let res = sqlx::query(
                "update users set oidc_sub = $1, email = $2, name = $3 \
                 where id = 1 and oidc_sub like 'bootstrap:%'",
            )
            .bind(sub)
            .bind(email)
            .bind(name)
            .execute(&mut *tx)
            .await?;
            if res.rows_affected() == 1 {
                tx.commit().await?;
                return Ok(1);
            }
            // Lost the claim race — fall through to insert a new user.
        }
    }
    let id: i32 = sqlx::query_scalar(
        "insert into users (oidc_sub, email, name) values ($1,$2,$3) returning id",
    )
    .bind(sub)
    .bind(email)
    .bind(name)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(id)
}

pub async fn logout(
    pool: &PgPool,
    headers: &HeaderMap,
    peer: Option<std::net::IpAddr>,
) -> Response {
    let host = request_host(headers, peer);
    let landing = post_logout_redirect_for(&host);
    let secure = cookie_secure(headers, peer);
    // nit: prod sessions live under `__Host-{name}` — read the same name set
    // as login (not bare `session_cookie_name()` alone) so DELETE hits the row.
    let mut token = None;
    for name in session_cookie_names_to_read() {
        if let Some(cookie) = cookie_from_headers(headers, &name) {
            if let Some(t) = verify_session_cookie(&cookie) {
                token = Some(t);
                break;
            }
        }
    }
    let mut id_token: Option<String> = None;
    if let Some(token) = token.as_deref() {
        if let Ok(Some(row)) = sqlx::query(
            "delete from auth_sessions where token = $1 returning issuer_base, id_token",
        )
        .bind(token)
        .fetch_optional(pool)
        .await
        {
            id_token = row.get("id_token");
        }
    }
    // End-session follows the issuer for this host.
    let end_session = end_session_for(&host);
    let mut url = match Url::parse(&end_session) {
        Ok(u) => u,
        Err(_) => {
            return found_with_cookies(&landing, clear_session_cookie_headers(secure));
        }
    };
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("post_logout_redirect_uri", &landing);
        q.append_pair("client_id", client_id().unwrap_or_else(|| SLUG_DEFAULT.to_string()).as_str());
        if let Some(hint) = id_token.as_deref().filter(|s| !s.is_empty()) {
            q.append_pair("id_token_hint", hint);
        }
    }
    found_with_cookies(url.as_str(), clear_session_cookie_headers(secure))
}

pub async fn document_owned(pool: &PgPool, id: i32, owner_id: i32) -> sqlx::Result<bool> {
    let n: i32 =
        sqlx::query_scalar("select count(*)::int from documents where id = $1 and owner_id = $2")
            .bind(id)
            .bind(owner_id)
            .fetch_one(pool)
            .await?;
    Ok(n > 0)
}

pub async fn tag_owned(pool: &PgPool, id: i32, owner_id: i32) -> sqlx::Result<bool> {
    let n: i32 =
        sqlx::query_scalar("select count(*)::int from tags where id = $1 and owner_id = $2")
            .bind(id)
            .bind(owner_id)
            .fetch_one(pool)
            .await?;
    Ok(n > 0)
}

pub async fn correspondent_owned(pool: &PgPool, id: i32, owner_id: i32) -> sqlx::Result<bool> {
    let n: i32 = sqlx::query_scalar(
        "select count(*)::int from correspondents where id = $1 and owner_id = $2",
    )
    .bind(id)
    .bind(owner_id)
    .fetch_one(pool)
    .await?;
    Ok(n > 0)
}

pub async fn type_owned(pool: &PgPool, id: i32, owner_id: i32) -> sqlx::Result<bool> {
    let n: i32 = sqlx::query_scalar(
        "select count(*)::int from document_types where id = $1 and owner_id = $2",
    )
    .bind(id)
    .bind(owner_id)
    .fetch_one(pool)
    .await?;
    Ok(n > 0)
}

pub async fn path_owned(pool: &PgPool, id: i32, owner_id: i32) -> sqlx::Result<bool> {
    let n: i32 = sqlx::query_scalar(
        "select count(*)::int from storage_paths where id = $1 and owner_id = $2",
    )
    .bind(id)
    .bind(owner_id)
    .fetch_one(pool)
    .await?;
    Ok(n > 0)
}

pub async fn field_owned(pool: &PgPool, id: i32, owner_id: i32) -> sqlx::Result<bool> {
    let n: i32 = sqlx::query_scalar(
        "select count(*)::int from custom_fields where id = $1 and owner_id = $2",
    )
    .bind(id)
    .bind(owner_id)
    .fetch_one(pool)
    .await?;
    Ok(n > 0)
}

pub async fn view_owned(pool: &PgPool, id: i32, owner_id: i32) -> sqlx::Result<bool> {
    let n: i32 =
        sqlx::query_scalar("select count(*)::int from saved_views where id = $1 and owner_id = $2")
            .bind(id)
            .bind(owner_id)
            .fetch_one(pool)
            .await?;
    Ok(n > 0)
}

pub async fn workflow_owned(pool: &PgPool, id: i32, owner_id: i32) -> sqlx::Result<bool> {
    let n: i32 =
        sqlx::query_scalar("select count(*)::int from workflows where id = $1 and owner_id = $2")
            .bind(id)
            .bind(owner_id)
            .fetch_one(pool)
            .await?;
    Ok(n > 0)
}

#[cfg(test)]
mod syn16_xfh_tests {
    use super::*;
    use axum::http::HeaderMap;
    use std::net::{IpAddr, Ipv4Addr};

    const PUBLIC_HOST: &str = "archive.example";
    const LAN_HOST: &str = "archive.lan.test";
    const LEGACY_HOST: &str = "archive.legacy.test";
    const PUBLIC_ISS: &str = "https://auth.example";
    const LAN_ISS: &str = "https://auth.lan.test";
    const BETA_MOBILE_ISS: &str = "https://auth.example/application/o/archive-mobile/";
    const LAN_MOBILE_ISS: &str = "https://auth.lan.test/application/o/archive-mobile/";
    const WEB_PROD_ISS: &str = "https://auth.example/application/o/app/";
    const BARE_PUBLIC_ISS: &str = "https://auth.example/";

    struct EnvRestore {
        saved: Vec<(&'static str, Option<String>)>,
    }

    impl EnvRestore {
        fn apply(pairs: &[(&'static str, Option<&str>)]) -> Self {
            let saved = pairs
                .iter()
                .map(|(key, _)| (*key, std::env::var(key).ok()))
                .collect();
            unsafe {
                for (key, value) in pairs {
                    match value {
                        Some(v) => std::env::set_var(key, v),
                        None => std::env::remove_var(key),
                    }
                }
            }
            Self { saved }
        }
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            unsafe {
                for (key, value) in &self.saved {
                    match value {
                        Some(v) => std::env::set_var(key, v),
                        None => std::env::remove_var(key),
                    }
                }
            }
        }
    }

    fn with_example_realms<T>(f: impl FnOnce() -> T) -> T {
        let _guard = crate::instance::env_lock().lock().unwrap();
        let _env = EnvRestore::apply(&[
            (
                "PAPERRUST_PUBLIC_URLS",
                Some("https://archive.lan.test,https://archive.example,https://archive.legacy.test"),
            ),
            (
                "PAPERRUST_OIDC_ISSUERS",
                Some(
                    "https://archive.example=https://auth.example,\
https://archive.lan.test=https://auth.lan.test,\
https://archive.legacy.test=https://auth.legacy.test",
                ),
            ),
            ("PAPERRUST_OIDC_ISSUER", Some("https://auth.legacy.test")),
            (
                "PAPERRUST_LAN_URLS",
                Some("https://archive.lan.test,https://archive.legacy.test"),
            ),
        ]);
        f()
    }

    /// Shape of the beta/prod instances: LAN origin behind ingress-nginx whose
    /// issuer is on a different domain than `PAPERRUST_OIDC_ISSUER`, plus a
    /// public origin reached only through the loopback tunnel sidecar.
    fn with_split_domains<T>(f: impl FnOnce() -> T) -> T {
        let _guard = crate::instance::env_lock().lock().unwrap();
        let _env = EnvRestore::apply(&[
            ("PAPERRUST_PUBLIC_URLS", Some("https://beta.archive.lan,https://archive.example")),
            (
                "PAPERRUST_OIDC_ISSUERS",
                Some("https://beta.archive.lan=https://auth.lan,https://archive.example=https://auth.example"),
            ),
            ("PAPERRUST_OIDC_ISSUER", Some("https://auth.example/application/o/x/")),
            ("PAPERRUST_LAN_URLS", Some("https://beta.archive.lan")),
        ]);
        f()
    }

    #[test]
    fn explicit_lan_url_from_ingress_peer_is_accepted() {
        with_split_domains(|| {
            let ingress = Some(IpAddr::V4(Ipv4Addr::new(10, 42, 0, 17)));
            let sidecar = Some(IpAddr::V4(Ipv4Addr::LOCALHOST));
            // LAN origin from an ingress-nginx peer: served (no 400).
            let h = headers_with("beta.archive.lan", None);
            assert!(!untrusted_public_host(&h, ingress));
            assert!(!loopback_host_unlisted(&h, ingress));
            let host = request_host(&h, ingress);
            assert_eq!(host_realm(&host), Realm::Lan);
            assert_eq!(redirect_uri_for(&host), "https://beta.archive.lan/api/auth/callback");
            assert_eq!(issuer_base(&host), "https://auth.lan");
            // Public origin from an ingress peer: 400.
            let p = headers_with("archive.example", None);
            assert!(untrusted_public_host(&p, ingress));
            assert!(untrusted_public_host(&headers_with("archive.example", Some("beta.archive.lan")), ingress));
            // Public origin from the loopback sidecar: served on its own origin.
            assert!(!untrusted_public_host(&p, sidecar));
            assert!(!loopback_host_unlisted(&p, sidecar));
            let host = request_host(&p, sidecar);
            assert_eq!(host_realm(&host), Realm::Public);
            assert_eq!(redirect_uri_for(&host), "https://archive.example/api/auth/callback");
            assert_eq!(issuer_base(&host), "https://auth.example");
            // LAN origin through the sidecar is listed too: not refused.
            assert!(!loopback_host_unlisted(&h, sidecar));
            // evil.com: 400 from the sidecar, never in any URL from any peer.
            assert!(loopback_host_unlisted(&headers_with("evil.com", None), sidecar));
            for peer in [ingress, sidecar] {
                for xfh in [None, Some("evil.com")] {
                    let host = request_host(&headers_with("evil.com", xfh), peer);
                    for u in [
                        app_origin_for(&host),
                        redirect_uri_for(&host),
                        post_login_url_for(&host),
                        post_logout_redirect_for(&host),
                        issuer_base(&host),
                    ] {
                        assert!(!u.contains("evil"), "{peer:?} {xfh:?} -> {u}");
                    }
                }
            }
        });
    }

    #[test]
    fn unmarked_origin_is_public_fail_closed() {
        let _guard = crate::instance::env_lock().lock().unwrap();
        let _env = EnvRestore::apply(&[
            ("PAPERRUST_PUBLIC_URLS", Some("https://beta.archive.lan")),
            ("PAPERRUST_OIDC_ISSUERS", Some("https://beta.archive.lan=https://auth.lan")),
            ("PAPERRUST_OIDC_ISSUER", Some("https://auth.lan")),
            ("PAPERRUST_LAN_URLS", None),
        ]);
        // Even with an issuer on the same domain, no LAN marker means public.
        let ingress = Some(IpAddr::V4(Ipv4Addr::new(10, 42, 0, 17)));
        assert!(untrusted_public_host(&headers_with("beta.archive.lan", None), ingress));
    }

    fn headers_with(host: &str, xfh: Option<&str>) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(axum::http::header::HOST, host.parse().unwrap());
        if let Some(xfh) = xfh {
            h.insert("x-forwarded-host", xfh.parse().unwrap());
        }
        h
    }

    #[test]
    fn only_loopback_peer_trusts_xfh() {
        assert!(peer_trusts_xfh(Some(IpAddr::V4(Ipv4Addr::LOCALHOST))));
        assert!(peer_trusts_xfh(Some("::ffff:127.0.0.1".parse().unwrap())));
        assert!(!peer_trusts_xfh(Some(IpAddr::V4(Ipv4Addr::new(
            192, 168, 0, 50
        )))));
        assert!(!peer_trusts_xfh(Some(IpAddr::V4(Ipv4Addr::new(
            192, 0, 2, 10
        )))));
        assert!(!peer_trusts_xfh(None));
    }

    #[test]
    fn lan_peer_cannot_spoof_public_realm_via_xfh() {
        with_example_realms(|| {
            let h = headers_with("192.0.2.10:8788", Some(PUBLIC_HOST));
            let host = request_host(&h, Some(IpAddr::V4(Ipv4Addr::new(192, 168, 0, 50))));
            assert_ne!(host_realm(&host), Realm::Public);
            assert!(!host.contains("archive.example"));
            assert_ne!(issuer_base(&host), PUBLIC_ISS);
        });
    }

    #[test]
    fn lan_peer_cannot_spoof_public_realm_via_bare_host() {
        with_example_realms(|| {
            // refused (400), never remapped to a LAN origin/issuer.
            let h = headers_with(PUBLIC_HOST, None);
            let lan_peer = Some(IpAddr::V4(Ipv4Addr::new(192, 168, 0, 50)));
            assert!(untrusted_public_host(&h, lan_peer));
            let host = request_host(&h, lan_peer);
            assert_ne!(host_realm(&host), Realm::Public);
            assert_ne!(host_name(&host), LAN_HOST);
            assert_ne!(issuer_base(&host), PUBLIC_ISS);
            // Non-private untrusted peer: refused too.
            assert!(untrusted_public_host(&h, Some(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10)))));
            assert!(untrusted_public_host(&h, None));
        });
    }

    #[test]
    fn trusted_proxy_and_lan_hosts_are_not_refused() {
        with_example_realms(|| {
            let lan_peer = Some(IpAddr::V4(Ipv4Addr::new(192, 168, 0, 50)));
            assert!(!untrusted_public_host(&headers_with(PUBLIC_HOST, None), Some(IpAddr::V4(Ipv4Addr::LOCALHOST))));
            assert!(!untrusted_public_host(&headers_with(LAN_HOST, None), lan_peer));
            assert!(!untrusted_public_host(&headers_with("10.0.0.5:8788", None), lan_peer));
        });
    }

    #[test]
    fn lan_peer_keeps_lan_host() {
        with_example_realms(|| {
            let h = headers_with(LEGACY_HOST, None);
            let host = request_host(&h, Some(IpAddr::V4(Ipv4Addr::new(192, 168, 0, 50))));
            assert_eq!(host_realm(&host), Realm::Lan);
            assert_eq!(host_name(&host), LEGACY_HOST);
        });
    }

    #[test]
    fn loopback_peer_xfh_is_ignored() {
        with_example_realms(|| {
            // X-Forwarded-Host never selects the origin, even from 127.0.0.1.
            let h = headers_with("127.0.0.1:8788", Some(PUBLIC_HOST));
            let peer = Some(IpAddr::V4(Ipv4Addr::LOCALHOST));
            assert!(!loopback_host_unlisted(&h, peer));
            let host = request_host(&h, peer);
            assert_ne!(host_realm(&host), Realm::Public);
            assert_ne!(issuer_base(&host), PUBLIC_ISS);
            assert_eq!(redirect_uri_for(&host), "http://127.0.0.1:8788/api/auth/callback");
        });
    }

    /// (A): no origin is built from the Host header outside the validated
    /// constructors. Scans non-test source for `://{` format strings and only
    /// allows them in the listed functions.
    #[test]
    fn origins_are_built_only_by_validated_functions() {
        const ALLOWED: &[&str] = &[
            "parse_listed_origin",             // auth: closed PUBLIC_URLS list
            "parse_origin",                    // instance: PUBLIC_URLS parsing
            "request_host_origin_for_compare", // CSRF comparison only
        ];
        let sources = [
            ("auth.rs", include_str!("auth.rs")),
            ("main.rs", include_str!("main.rs")),
            ("instance.rs", include_str!("instance.rs")),
            ("stirling_edit.rs", include_str!("stirling_edit.rs")),
            ("jwks.rs", include_str!("jwks.rs")),
        ];
        let mut offenders = Vec::new();
        for (file, src) in sources {
            let lines: Vec<&str> = src.lines().collect();
            let mut current_fn = String::new();
            let mut i = 0;
            while i < lines.len() {
                let line = lines[i];
                if line.trim() == "#[cfg(test)]"
                    && lines.get(i + 1).is_some_and(|n| n.trim_start().starts_with("mod "))
                {
                    // Skip the whole test module (closes with `}` at column 0).
                    i += 2;
                    while i < lines.len() && lines[i] != "}" {
                        i += 1;
                    }
                    i += 1;
                    continue;
                }
                let t = line.trim_start();
                if let Some(pos) = t.find("fn ") {
                    if t[..pos].split_whitespace().all(|w| matches!(w, "pub" | "async" | "pub(crate)" | "const")) {
                        let rest = &t[pos + 3..];
                        current_fn = rest
                            .split(|c: char| c == '(' || c == '<')
                            .next()
                            .unwrap_or("")
                            .to_string();
                    }
                }
                if !t.starts_with("//") && line.contains("://{") && !ALLOWED.contains(&current_fn.as_str()) {
                    offenders.push(format!("{file}:{}: fn {current_fn}", i + 1));
                }
                i += 1;
            }
        }
        // jwks.rs builds a loopback test JWKS URL from a bound socket address
        // inside its test helpers only; anything else is an offender.
        offenders.retain(|o| !o.starts_with("jwks.rs") || !o.contains("fn spawn"));
        assert!(offenders.is_empty(), "origin built outside validated fns: {offenders:?}");
        assert!(!include_str!("auth.rs").contains(concat!("fn public", "_origin(")));
    }

    #[test]
    fn b6_exception_lan_peer_unlisted_host_never_enters_urls() {
        // exception: a non-loopback peer (10.x) with an unlisted Host is
        // not refused (cron, kubelet probes), but that Host must never be used
        // to build a redirect or URL (login, callback, post-login, logout).
        with_example_realms(|| {
            let peer = Some(IpAddr::V4(Ipv4Addr::new(10, 0, 1, 50)));
            for xfh in [None, Some("evil.com"), Some(PUBLIC_HOST)] {
                let h = headers_with("evil.com", xfh);
                assert!(!untrusted_public_host(&h, peer));
                assert!(!loopback_host_unlisted(&h, peer));
                let host = request_host(&h, peer);
                assert_ne!(host_realm(&host), Realm::Public);
                let urls = [
                    app_origin_for(&host),
                    redirect_uri_for(&host),
                    post_login_url_for(&host),
                    post_logout_redirect_for(&host),
                    end_session_for(&host),
                    issuer_base(&host),
                    crate::stirling_edit::stirling_url_for_host(&host),
                ];
                for u in &urls {
                    assert!(!u.contains("evil"), "{xfh:?} -> {u}");
                }
                assert_eq!(
                    redirect_uri_for(&host),
                    "https://archive.lan.test/api/auth/callback"
                );
                assert_ne!(issuer_base(&host), PUBLIC_ISS);
            }
        });
    }

    #[test]
    fn b6_listed_host_wins_over_evil_xfh_and_unlisted_host_is_refused() {
        with_example_realms(|| {
            let peer = Some(IpAddr::V4(Ipv4Addr::LOCALHOST));
            // Host = allowed public origin, XFH evil.com: Host origin, never evil.com.
            let h = headers_with(PUBLIC_HOST, Some("evil.com"));
            assert!(!loopback_host_unlisted(&h, peer));
            assert!(!untrusted_public_host(&h, peer));
            let host = request_host(&h, peer);
            assert_eq!(host_name(&host), PUBLIC_HOST);
            let redir = redirect_uri_for(&host);
            assert_eq!(redir, format!("https://{PUBLIC_HOST}/api/auth/callback"));
            assert!(!redir.contains("evil"));
            assert!(!post_login_url_for(&host).contains("evil"));
            // Host evil.com from 127.0.0.1: refused (400).
            let evil = headers_with("evil.com", None);
            assert!(loopback_host_unlisted(&evil, peer));
            assert!(loopback_host_unlisted(&headers_with("evil.com", Some(PUBLIC_HOST)), peer));
            // Legitimate paths stay open: loopback names, LAN host from LAN peer.
            assert!(!loopback_host_unlisted(&headers_with("127.0.0.1:8788", None), peer));
            assert!(!loopback_host_unlisted(&headers_with("localhost:5173", None), peer));
            assert!(!loopback_host_unlisted(&headers_with(LAN_HOST, None), peer));
            let lan_peer = Some(IpAddr::V4(Ipv4Addr::new(192, 168, 0, 50)));
            assert!(!loopback_host_unlisted(&headers_with(LAN_HOST, None), lan_peer));
            assert!(!untrusted_public_host(&headers_with(LAN_HOST, None), lan_peer));
        });
    }

    #[test]
    fn loopback_peer_may_use_bare_public_host() {
        with_example_realms(|| {
            let h = headers_with(PUBLIC_HOST, None);
            let host = request_host(&h, Some(IpAddr::V4(Ipv4Addr::LOCALHOST)));
            assert_eq!(host_realm(&host), Realm::Public);
        });
    }

    #[test]
    fn loopback_uses_configured_issuer() {
        with_example_realms(|| {
            assert_eq!(issuer_base("127.0.0.1:8788"), "https://auth.legacy.test");
            assert_eq!(
                redirect_uri_for("127.0.0.1:8788"),
                "http://127.0.0.1:8788/api/auth/callback"
            );
        });
    }

    struct PublicUrls;
    impl PublicUrls {
        fn set(raw: &'static str) -> Self {
            PUBLIC_URL_RAW_OVERRIDE.with(|cell| cell.set(Some(raw)));
            Self
        }
    }
    impl Drop for PublicUrls {
        fn drop(&mut self) {
            PUBLIC_URL_RAW_OVERRIDE.with(|cell| cell.set(None));
        }
    }

    /// The former built-in list (develop), now passed explicitly as config.
    const LEGACY_URLS: &str = "https://paperrust.lan,https://paperrust.example.com,https://paperrust.lan.loc,https://beta.paperrust.lan,http://127.0.0.1:8788";

    #[test]
    fn neutral_default_lists_only_loopback() {
        let _env = crate::instance::env_lock().lock().unwrap();
        let prev: Vec<Option<String>> = ["PAPERRUST_PUBLIC_URLS", "PAPERRUST_PUBLIC_URL"]
            .iter()
            .map(|k| std::env::var(k).ok())
            .collect();
        unsafe {
            std::env::remove_var("PAPERRUST_PUBLIC_URLS");
            std::env::remove_var("PAPERRUST_PUBLIC_URL");
        }
        let got = (redirect_uri_for("paperrust.lan"), app_origin_for("127.0.0.1:8788"));
        unsafe {
            for (k, v) in ["PAPERRUST_PUBLIC_URLS", "PAPERRUST_PUBLIC_URL"].iter().zip(prev) {
                if let Some(v) = v {
                    std::env::set_var(k, v);
                }
            }
        }
        assert_eq!(got.0, "http://127.0.0.1:8788/api/auth/callback");
        assert_eq!(got.1, "http://127.0.0.1:8788");
    }

    #[allow(dead_code)]
    fn neutral_default_lists_only_loopback_unlocked() {
        assert_eq!(
            redirect_uri_for("paperrust.lan"),
            "http://127.0.0.1:8788/api/auth/callback"
        );
        assert_eq!(app_origin_for("127.0.0.1:8788"), "http://127.0.0.1:8788");
    }

    #[test]
    fn listed_hosts_keep_redirect_login_and_logout_origins() {
        let _guard = PublicUrls::set(LEGACY_URLS);
        assert_eq!(
            redirect_uri_for("paperrust.lan"),
            "https://paperrust.lan/api/auth/callback"
        );
        assert_eq!(
            redirect_uri_for("paperrust.example.com"),
            "https://paperrust.example.com/api/auth/callback"
        );
        assert_eq!(
            redirect_uri_for("paperrust.lan.loc"),
            "https://paperrust.lan.loc/api/auth/callback"
        );
        assert_eq!(
            redirect_uri_for("beta.paperrust.lan"),
            "https://beta.paperrust.lan/api/auth/callback"
        );
        assert_eq!(
            redirect_uri_for("127.0.0.1:8788"),
            "http://127.0.0.1:8788/api/auth/callback"
        );
        assert_eq!(
            post_login_url_for("paperrust.lan"),
            "https://paperrust.lan/"
        );
        assert_eq!(
            post_logout_redirect_for("paperrust.example.com"),
            "https://paperrust.example.com/"
        );
        assert_eq!(
            post_logout_redirect_for("127.0.0.1:8788"),
            "http://127.0.0.1:8788/"
        );
    }

    #[test]
    fn unknown_host_and_forwarded_host_never_enter_oidc_urls() {
        let _guard = PublicUrls::set(LEGACY_URLS);
        for evil in [
            "evil.lan",
            "attacker.lan.loc",
            "notpaperrust.lan",
            "paperrust.lan.evil.com",
        ] {
            let redir = redirect_uri_for(evil);
            let login = post_login_url_for(evil);
            let logout = post_logout_redirect_for(evil);
            assert_eq!(redir, "https://paperrust.lan/api/auth/callback", "{evil}");
            assert_eq!(login, "https://paperrust.lan/", "{evil}");
            assert_eq!(logout, "https://paperrust.lan/", "{evil}");
            assert!(!redir.contains(evil), "{redir}");
            assert!(!login.contains(evil), "{login}");
            assert!(!logout.contains(evil), "{logout}");
        }

        let h = headers_with("127.0.0.1:8788", Some("attacker.lan.loc"));
        let host = request_host(&h, Some(IpAddr::V4(Ipv4Addr::LOCALHOST)));
        // !53: an unlisted X-Forwarded-Host is not even taken as the request host.
        assert!(!host.contains("attacker"), "{host}");
        // Loopback Host stays on loopback (!53), never the first public origin.
        let redir = redirect_uri_for(&host);
        assert_eq!(redir, "http://127.0.0.1:8788/api/auth/callback");
        assert!(!redir.contains("attacker"));
        assert_eq!(post_login_url_for(&host), "http://127.0.0.1:8788/");
        assert_eq!(post_logout_redirect_for(&host), "http://127.0.0.1:8788/");

        // Untrusted peer: X-Forwarded-Host is ignored, and a raw Host is still
        // not copied when it is not on the closed list.
        let spoof = headers_with("evil.lan", Some("paperrust.example.com"));
        let host = request_host(&spoof, Some(IpAddr::V4(Ipv4Addr::new(10, 0, 1, 50))));
        let redir = redirect_uri_for(&host);
        assert!(!redir.contains("evil"), "{host} -> {redir}");
        assert_eq!(post_logout_redirect_for(&host), "https://paperrust.lan/");
    }

    #[test]
    fn public_urls_closed_list_and_singular_alias() {
        assert_eq!(
            resolve_public_url_raw(Some("https://a.example"), Some("https://b.example")).as_deref(),
            Some("https://a.example")
        );
        assert_eq!(
            resolve_public_url_raw(Some("  "), Some("https://b.example")).as_deref(),
            Some("https://b.example")
        );
        assert_eq!(
            resolve_public_url_raw(None, Some("https://alias.example")).as_deref(),
            Some("https://alias.example")
        );
        assert!(resolve_public_url_raw(None, Some("")).is_none());

        let _guard = PublicUrls::set("https://archive.example,https://beta.example");
        assert_eq!(app_origin_for("archive.example"), "https://archive.example");
        assert_eq!(app_origin_for("beta.example"), "https://beta.example");
        assert_eq!(
            redirect_uri_for("evil.lan"),
            "https://archive.example/api/auth/callback"
        );
        assert_eq!(post_login_url_for("nope.example"), "https://archive.example/");
        assert_eq!(
            post_logout_redirect_for("injected.example"),
            "https://archive.example/"
        );
        assert_eq!(
            app_origin_for("evil.archive.example"),
            "https://archive.example"
        );
    }

    #[test]
    fn unusable_public_url_list_does_not_echo_host() {
        let _guard = PublicUrls::set("javascript:alert(1),not a url");
        let redir = redirect_uri_for("evil.lan");
        assert_eq!(redir, "http://127.0.0.1:8788/api/auth/callback");
        assert!(!redir.contains("evil"));
        assert_eq!(post_logout_redirect_for("evil.lan"), "http://127.0.0.1:8788/");
    }

    #[test]
    fn host_name_rejects_evil_loopback_suffix() {
        assert_ne!(host_name("127.0.0.1.evil.com"), "127.0.0.1");
        assert_eq!(host_name("127.0.0.1.evil.com"), "127.0.0.1.evil.com");
    }

    #[test]
    fn host_name_strips_numeric_port() {
        assert_eq!(host_name("127.0.0.1:443"), "127.0.0.1");
    }

    #[test]
    fn session_cookie_names_include_host_prefix() {
        let names = session_cookie_names_to_read();
        assert!(names.iter().any(|n| n.starts_with("__Host-")), "{names:?}");
        assert!(names.iter().any(|n| !n.starts_with("__Host-")), "{names:?}");
    }

    #[test]
    fn clear_session_cookies_emit_host_secure() {
        let headers = clear_session_cookie_headers(true);
        let joined: Vec<String> = headers
            .iter()
            .map(|h| h.to_str().unwrap_or("").to_string())
            .collect();
        assert!(
            joined
                .iter()
                .any(|c| c.starts_with("__Host-") && c.contains("Secure") && c.contains("Path=/")),
            "{joined:?}"
        );
        assert!(
            joined
                .iter()
                .any(|c| !c.starts_with("__Host-") && c.contains("Path=/")),
            "{joined:?}"
        );
    }

    fn hdrs(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                v.parse().unwrap(),
            );
        }
        h
    }

    #[test]
    fn csrf_skips_without_session_cookie() {
        let h = hdrs(&[("host", PUBLIC_HOST)]);
        assert!(csrf_ok(
            &h,
            Some(IpAddr::V4(Ipv4Addr::LOCALHOST)),
            &axum::http::Method::POST
        ));
    }

    #[test]
    fn csrf_rejects_cookie_mutation_without_origin() {
        with_example_realms(|| {
            let h = hdrs(&[
                ("host", PUBLIC_HOST),
                ("cookie", "paperrust_session=fake.sig"),
            ]);
            assert!(!csrf_ok(
                &h,
                Some(IpAddr::V4(Ipv4Addr::LOCALHOST)),
                &axum::http::Method::POST
            ));
        });
    }

    #[test]
    fn csrf_allows_cookie_mutation_same_origin() {
        with_example_realms(|| {
            let h = hdrs(&[
                ("host", PUBLIC_HOST),
                ("cookie", "paperrust_session=fake.sig"),
                ("origin", "https://archive.example"),
            ]);
            assert!(csrf_ok(
                &h,
                Some(IpAddr::V4(Ipv4Addr::LOCALHOST)),
                &axum::http::Method::POST
            ));
        });
    }

    #[test]
    fn csrf_get_with_session_cookie_ok() {
        let h = hdrs(&[
            ("host", PUBLIC_HOST),
            ("cookie", "paperrust_session=fake.sig"),
        ]);
        assert!(csrf_ok(
            &h,
            Some(IpAddr::V4(Ipv4Addr::LOCALHOST)),
            &axum::http::Method::GET
        ));
    }

    #[test]
    fn oidc_bearer_reads_jwt_and_skips_pr_tokens() {
        let jwt = "eyJhbGciOiJSUzI1NiJ9.eyJzdWIiOiIxIn0.sig";
        let h = hdrs(&[("authorization", &format!("Bearer {jwt}"))]);
        assert_eq!(oidc_bearer_from_headers(&h).as_deref(), Some(jwt));
        let missing = hdrs(&[("host", PUBLIC_HOST)]);
        assert!(oidc_bearer_from_headers(&missing).is_none());
        let pr = hdrs(&[("authorization", "Bearer pr_AbCdEfGh0123456789secret")]);
        assert!(
            oidc_bearer_from_headers(&pr).is_none(),
            "pr_ tokens must stay ingest-only"
        );
        let empty = hdrs(&[("authorization", "Bearer ")]);
        assert!(oidc_bearer_from_headers(&empty).is_none());
        let not_jwt = hdrs(&[("authorization", "Bearer not-a-jwt")]);
        assert!(oidc_bearer_from_headers(&not_jwt).is_none());
    }

    fn with_mobile_issuers<T>(f: impl FnOnce() -> T) -> T {
        with_example_realms(|| {
            let _mobile = EnvRestore::apply(&[
                ("PAPERRUST_OIDC_MOBILE_ISSUER", Some(BETA_MOBILE_ISS)),
                ("PAPERRUST_OIDC_MOBILE_ISSUER_LAN", Some(LAN_MOBILE_ISS)),
            ]);
            f()
        })
    }

    fn unsigned_iss_jwt(iss: &str, extra: serde_json::Value) -> String {
        let mut claims = serde_json::json!({
            "iss": iss,
            "sub": "user-1",
            "exp": 4_102_444_800u64,
        });
        if let Some(obj) = extra.as_object() {
            if let Some(dst) = claims.as_object_mut() {
                for (k, v) in obj {
                    dst.insert(k.clone(), v.clone());
                }
            }
        }
        let payload = URL_SAFE_NO_PAD.encode(claims.to_string());
        format!("eyJhbGciOiJSUzI1NiIsImtpZCI6InRlc3Qta2lkIn0.{payload}.sig")
    }

    fn allowlist_excludes_web_and_bare(issuers: &[String]) {
        assert!(
            issuers.iter().all(|i| {
                let t = i.trim_end_matches('/');
                t != "https://auth.example"
                    && t != "https://auth.lan.test"
                    && t != "https://auth.legacy.test"
                    && !t.ends_with("/application/o/app")
                    && !t.ends_with("/application/o/preview")
            }),
            "Bearer allowlist must not include bare host or web slugs: {issuers:?}"
        );
    }

    #[test]
    fn bearer_mobile_disabled_when_env_unset() {
        with_example_realms(|| {
            let _clear = EnvRestore::apply(&[
                ("PAPERRUST_OIDC_MOBILE_ISSUER", None),
                ("PAPERRUST_OIDC_MOBILE_ISSUER_LAN", None),
            ]);
            assert!(
                allowed_issuers_for_host(PUBLIC_HOST).is_empty(),
                "unset MOBILE_ISSUER must fail-closed on public host"
            );
            assert!(
                allowed_issuers_for_host(LEGACY_HOST).is_empty(),
                "unset MOBILE_ISSUER_LAN must fail-closed on LAN host"
            );
            let token = unsigned_iss_jwt(BETA_MOBILE_ISS, serde_json::json!({"aud": "mobile"}));
            assert!(
                resolve_bearer_jwks(PUBLIC_HOST, &token).is_none(),
                "Bearer mobile disabled when env unset"
            );
        });
    }

    #[test]
    fn bearer_allowlist_is_exact_mobile_issuer_forms_only() {
        with_mobile_issuers(|| {
            let public = allowed_issuers_for_host(PUBLIC_HOST);
            assert_eq!(
                public,
                vec![
                    "https://auth.example/application/o/archive-mobile".into(),
                    BETA_MOBILE_ISS.to_string(),
                ]
            );
            allowlist_excludes_web_and_bare(&public);
            assert!(public.iter().all(|i| !i.contains("lan.test")), "{public:?}");

            let lan = allowed_issuers_for_host(LEGACY_HOST);
            assert_eq!(
                lan,
                vec![
                    "https://auth.lan.test/application/o/archive-mobile".into(),
                    LAN_MOBILE_ISS.to_string(),
                ]
            );
            allowlist_excludes_web_and_bare(&lan);
            assert!(lan.iter().all(|i| !i.contains("auth.example/")), "{lan:?}");
        });
    }

    #[test]
    fn cookie_web_discovery_still_uses_paperrust_slug() {
        with_example_realms(|| {
            assert_eq!(issuer_base(PUBLIC_HOST), PUBLIC_ISS);
            assert_eq!(
                end_session_for(PUBLIC_HOST),
                "https://auth.example/application/o/app/end-session/"
            );
            assert_eq!(
                discovery_urls(PUBLIC_ISS),
                vec![
                    "https://auth.example/.well-known/openid-configuration".to_string(),
                    "https://auth.example/application/o/app/.well-known/openid-configuration"
                        .to_string(),
                ]
            );
        });
    }

    #[test]
    fn bearer_mobile_iss_and_aud_ok_on_public_host() {
        with_mobile_issuers(|| {
            let token = unsigned_iss_jwt(
                BETA_MOBILE_ISS,
                serde_json::json!({"aud": "mobile"}),
            );
            let (issuers, jwks) =
                resolve_bearer_jwks(PUBLIC_HOST, &token).expect("allowlisted");
            assert!(issuer_allowed(BETA_MOBILE_ISS, &issuers), "{issuers:?}");
            assert_eq!(
                jwks,
                "https://auth.example/application/o/archive-mobile/jwks/"
            );
            let signed = crate::jwks::tests_sign_rs256(
                serde_json::json!({
                    "iss": BETA_MOBILE_ISS,
                    "aud": "mobile",
                    "sub": "user-1",
                    "exp": 4_102_444_800u64,
                    "iat": 1_700_000_000u64,
                }),
                Some("test-kid"),
            );
            crate::jwks::verify_access_token_with_test_keys(
                &signed,
                &issuers,
                "mobile",
            )
            .expect("mobile token must verify");
        });
    }

    #[test]
    fn bearer_web_prod_token_rejected() {
        with_mobile_issuers(|| {
            let token = unsigned_iss_jwt(
                WEB_PROD_ISS,
                serde_json::json!({"azp": "paperrust", "aud": "paperrust"}),
            );
            assert!(
                resolve_bearer_jwks(PUBLIC_HOST, &token).is_none(),
                "web token must not fetch JWKS on the Bearer path"
            );
        });
    }

    #[test]
    fn bearer_bare_host_iss_rejected() {
        with_mobile_issuers(|| {
            let token = unsigned_iss_jwt(
                BARE_PUBLIC_ISS,
                serde_json::json!({"aud": "mobile"}),
            );
            assert!(
                resolve_bearer_jwks(PUBLIC_HOST, &token).is_none(),
                "bare-host iss must not fetch JWKS on the Bearer path"
            );
        });
    }

    #[test]
    fn bearer_lan_host_rejects_public_iss() {
        with_mobile_issuers(|| {
            let token = unsigned_iss_jwt(
                BETA_MOBILE_ISS,
                serde_json::json!({"aud": "mobile"}),
            );
            assert!(
                resolve_bearer_jwks(LEGACY_HOST, &token).is_none(),
                "public mobile iss on LAN host must not resolve"
            );
            let lan_ok = unsigned_iss_jwt(
                LAN_MOBILE_ISS,
                serde_json::json!({"aud": "mobile"}),
            );
            let (_, jwks) = resolve_bearer_jwks(LEGACY_HOST, &lan_ok).expect("LAN issuer");
            assert_eq!(
                jwks,
                "https://auth.lan.test/application/o/archive-mobile/jwks/"
            );
        });
    }

    #[test]
    fn identity_from_claims_uses_sub_then_email() {
        let ident = identity_from_claims(&serde_json::json!({
            "sub": "ak-uuid",
            "email": "ada@example.invalid",
            "name": "Ada Example",
        }))
        .unwrap();
        assert_eq!(ident.sub, "ak-uuid");
        assert_eq!(ident.email, "ada@example.invalid");
        assert_eq!(ident.name, "Ada Example");
        let fallback = identity_from_claims(&serde_json::json!({"sub": "only-sub"})).unwrap();
        assert_eq!(fallback.email, "only-sub@oidc.local");
        assert!(identity_from_claims(&serde_json::json!({"email": "x@y.z"})).is_none());
    }

    #[test]
    fn session_cookie_hmac_roundtrip() {
        let _guard = crate::instance::env_lock().lock().unwrap();
        let _env = EnvRestore::apply(&[("SESSION_SECRET", Some(&"a".repeat(32)))]);
        let signed = sign_session_token("tok123").expect("sign");
        assert_eq!(verify_session_cookie(&signed).as_deref(), Some("tok123"));
        assert!(verify_session_cookie("tok123.notasignature").is_none());
    }

    #[test]
    fn mobile_client_id_defaults_and_env() {
        let _guard = crate::instance::env_lock().lock().unwrap();
        let _clear = EnvRestore::apply(&[("PAPERRUST_OIDC_MOBILE_CLIENT_ID", None)]);
        assert_eq!(mobile_client_id(), "mobile");
        unsafe {
            std::env::set_var("PAPERRUST_OIDC_MOBILE_CLIENT_ID", "custom-mobile");
        }
        assert_eq!(mobile_client_id(), "custom-mobile");
    }
}
