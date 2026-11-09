//! OAuth authorization-code + PKCE for MCP connectors.
//!
//! Public clients only, no client secret. Grok.com keeps client `paperrust`
//! and its two exchange URLs. Grok Bot registers itself and returns to the
//! Cursor HTTPS callback, the Cursor app callback, or an HTTP loopback
//! `/callback`. The access token handed back is `PAPERRUST_MCP_BEARER`. The
//! process `pr_…` never leaves the server. Consent is limited to the owner
//! of that token, because the MCP gate always acts as that owner.

use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};

use axum::Json;
use axum::body::to_bytes;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE, LOCATION, PRAGMA, SET_COOKIE};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use rand::Rng;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};

use crate::AppState;
use crate::auth;
use crate::rate_limit_ok;

const CLIENT_ID: &str = "paperrust";
const REDIRECT_WITH_SLASH: &str = "https://grok.com/connectors-oauth-exchange-code/";
const REDIRECT_BARE: &str = "https://grok.com/connectors-oauth-exchange-code";
const CURSOR_HTTPS: &str = "https://www.cursor.com/agents/mcp/oauth/callback";
const CURSOR_APP: &str = "cursor://anysphere.cursor-mcp/oauth/callback";
fn public_origin() -> Option<String> {
    std::env::var("PAPERRUST_PUBLIC_URL")
        .ok()
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            crate::instance::public_urls()
                .into_iter()
                .find(|o| o.https)
                .map(|o| o.origin)
        })
}

fn issuer() -> Option<String> {
    public_origin()
}

fn resource() -> Option<String> {
    issuer().map(|o| format!("{o}/api/mcp"))
}
const CODE_PREFIX: &str = "mc_";
const REFRESH_PREFIX: &str = "mr_";
const ACCESS_EXPIRES_IN: u64 = 3600;
const QUERY_MAX: usize = 1500;
const BODY_MAX: usize = 8 * 1024;

const OWNER_ONLY: &str = "Ce connecteur est réservé au propriétaire de l'archive.";

#[derive(Debug)]
struct Checked {
    client_id: String,
    redirect_uri: String,
    state: Option<String>,
    challenge: String,
    scope: Option<String>,
}

#[derive(Debug)]
enum CheckFail {
    Reject,
    Bounce {
        redirect_uri: String,
        error: &'static str,
        state: Option<String>,
    },
}

enum OwnerLookup {
    Ready(i32),
    NotConfigured,
    Unavailable,
}

enum ExchangeErr {
    InvalidGrant,
    Server,
}

pub async fn ensure_schema(pool: &PgPool) -> anyhow::Result<()> {
    sqlx::query(
        r#"create table if not exists mcp_oauth_codes (
            code_hash text primary key,
            owner_id int not null references users(id) on delete cascade,
            code_challenge text not null,
            redirect_uri text not null,
            expires_at timestamptz not null,
            used_at timestamptz
        )"#,
    )
    .execute(pool)
    .await?;
    sqlx::query(
        r#"create table if not exists mcp_oauth_refresh (
            token_hash text primary key,
            owner_id int not null references users(id) on delete cascade,
            expires_at timestamptz not null,
            used_at timestamptz
        )"#,
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "alter table mcp_oauth_codes add column if not exists client_id text not null default 'paperrust'",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "alter table mcp_oauth_refresh add column if not exists client_id text not null default 'paperrust'",
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn authorize(State(state): State<AppState>, req: Request) -> Response {
    let peer = peer_of(&req);
    let headers = req.headers().clone();
    if !rate_limit_ok(
        &format!("mcp-oauth-authz:{}", client_ip(&headers, peer)),
        30,
        60,
    ) {
        return rate_page();
    }
    let query = req.uri().query().unwrap_or("");
    if query.len() > QUERY_MAX || has_ctl(query) {
        return rejected_page();
    }
    let map = match parse_form(query) {
        Ok(m) => m,
        Err(()) => return rejected_page(),
    };
    let checked = match check_authorize(&map) {
        Ok(c) => c,
        Err(CheckFail::Reject) => return rejected_page(),
        Err(CheckFail::Bounce {
            redirect_uri,
            error,
            state,
        }) => return bounce(&redirect_uri, error, state.as_deref()),
    };
    match configured_owner(&state.pool).await {
        OwnerLookup::NotConfigured => not_configured_page(),
        OwnerLookup::Unavailable => unavailable_page(),
        OwnerLookup::Ready(owner) => match session_user(&state, &headers, peer).await {
            Session::None => redirect_login(&headers, peer, query),
            Session::User(id) if id != owner => owner_only_page(),
            Session::User(_) => consent_page(&checked),
        },
    }
}

pub async fn authorize_submit(State(state): State<AppState>, req: Request) -> Response {
    let peer = peer_of(&req);
    let (parts, body) = req.into_parts();
    let headers = parts.headers;
    if !rate_limit_ok(
        &format!("mcp-oauth-authz:{}", client_ip(&headers, peer)),
        30,
        60,
    ) {
        return rate_page();
    }
    if !form_content_type(&headers) {
        return rejected_page();
    }
    let text = match read_body(body).await {
        Ok(t) => t,
        Err(()) => return rejected_page(),
    };
    let map = match parse_form(&text) {
        Ok(m) => m,
        Err(()) => return rejected_page(),
    };
    let checked = match check_authorize(&map) {
        Ok(c) => c,
        Err(CheckFail::Reject) => return rejected_page(),
        Err(CheckFail::Bounce {
            redirect_uri,
            error,
            state,
        }) => return bounce(&redirect_uri, error, state.as_deref()),
    };
    if !auth::csrf_ok(&headers, peer, &Method::POST) {
        return auth::csrf_forbidden();
    }
    let owner = match configured_owner(&state.pool).await {
        OwnerLookup::Ready(id) => id,
        OwnerLookup::NotConfigured => return not_configured_page(),
        OwnerLookup::Unavailable => return unavailable_page(),
    };
    let user_id = match session_user(&state, &headers, peer).await {
        Session::User(id) => id,
        Session::None => {
            let q = canonical_authorize_query(&checked);
            return redirect_login(&headers, peer, &q);
        }
    };
    if user_id != owner {
        return owner_only_page();
    }
    match map.get("decision").map(String::as_str) {
        Some("deny") => bounce(
            &checked.redirect_uri,
            "access_denied",
            checked.state.as_deref(),
        ),
        Some("allow") => mint_code(&state.pool, owner, &checked).await,
        _ => bounce(
            &checked.redirect_uri,
            "invalid_request",
            checked.state.as_deref(),
        ),
    }
}

pub async fn token(State(state): State<AppState>, req: Request) -> Response {
    let peer = peer_of(&req);
    let (parts, body) = req.into_parts();
    let headers = parts.headers;
    if !rate_limit_ok(
        &format!("mcp-oauth-token:{}", client_ip(&headers, peer)),
        30,
        60,
    ) {
        return oauth_error(StatusCode::TOO_MANY_REQUESTS, "temporarily_unavailable");
    }
    if !form_content_type(&headers) {
        return oauth_error(StatusCode::BAD_REQUEST, "invalid_request");
    }
    let text = match read_body(body).await {
        Ok(t) => t,
        Err(()) => return oauth_error(StatusCode::BAD_REQUEST, "invalid_request"),
    };
    let map = match parse_form(&text) {
        Ok(m) => m,
        Err(()) => return oauth_error(StatusCode::BAD_REQUEST, "invalid_request"),
    };
    let authorization = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    let Some(client_id) = presented_client_id(&map, authorization) else {
        return oauth_error(StatusCode::UNAUTHORIZED, "invalid_client");
    };
    if map.get("scope").is_some_and(|s| !scope_ok(Some(s))) {
        return oauth_error(StatusCode::BAD_REQUEST, "invalid_scope");
    }
    let grant = map.get("grant_type").map(String::as_str).unwrap_or("");
    let OwnerLookup::Ready(owner) = configured_owner(&state.pool).await else {
        return oauth_error(StatusCode::INTERNAL_SERVER_ERROR, "server_error");
    };
    let refresh = match grant {
        "authorization_code" => {
            let code = map.get("code").map(String::as_str).unwrap_or("");
            let redirect = map.get("redirect_uri").map(String::as_str).unwrap_or("");
            let verifier = map.get("code_verifier").map(String::as_str).unwrap_or("");
            if !prefixed_ok(code, CODE_PREFIX)
                || !redirect_allowed(redirect)
                || !verifier_ok(verifier)
            {
                return oauth_error(StatusCode::BAD_REQUEST, "invalid_grant");
            }
            exchange_code(&state.pool, owner, &client_id, code, redirect, verifier).await
        }
        "refresh_token" => {
            let raw = map.get("refresh_token").map(String::as_str).unwrap_or("");
            if !prefixed_ok(raw, REFRESH_PREFIX) {
                return oauth_error(StatusCode::BAD_REQUEST, "invalid_grant");
            }
            exchange_refresh(&state.pool, owner, &client_id, raw).await
        }
        "" => return oauth_error(StatusCode::BAD_REQUEST, "invalid_request"),
        _ => return oauth_error(StatusCode::BAD_REQUEST, "unsupported_grant_type"),
    };
    let refresh = match refresh {
        Ok(v) => v,
        Err(ExchangeErr::InvalidGrant) => {
            return oauth_error(StatusCode::BAD_REQUEST, "invalid_grant");
        }
        Err(ExchangeErr::Server) => {
            return oauth_error(StatusCode::INTERNAL_SERVER_ERROR, "server_error");
        }
    };
    let Some(bearer) = env_secret("PAPERRUST_MCP_BEARER") else {
        return oauth_error(StatusCode::INTERNAL_SERVER_ERROR, "server_error");
    };
    token_ok(bearer, refresh)
}

enum Session {
    User(i32),
    None,
}

async fn session_user(state: &AppState, headers: &HeaderMap, peer: Option<IpAddr>) -> Session {
    match auth::user_from_headers(&state.pool, headers, peer).await {
        Some(user) => Session::User(user.id),
        None => Session::None,
    }
}

async fn configured_owner(pool: &PgPool) -> OwnerLookup {
    let Some(token) = env_secret("PAPERRUST_MCP_API_TOKEN") else {
        return OwnerLookup::NotConfigured;
    };
    if !token.starts_with("pr_") || env_secret("PAPERRUST_MCP_BEARER").is_none() {
        return OwnerLookup::NotConfigured;
    }
    let hash = sha256_hex(&token);
    match sqlx::query(
        "select owner_id from api_tokens where token_hash = $1 and revoked_at is null",
    )
    .bind(&hash)
    .fetch_optional(pool)
    .await
    {
        Ok(Some(row)) => OwnerLookup::Ready(row.get("owner_id")),
        Ok(None) => OwnerLookup::NotConfigured,
        Err(e) => {
            tracing::error!("mcp oauth owner lookup failed: {e}");
            OwnerLookup::Unavailable
        }
    }
}

async fn mint_code(pool: &PgPool, owner: i32, checked: &Checked) -> Response {
    let _ = sqlx::query("delete from mcp_oauth_codes where expires_at < now() - interval '1 day'")
        .execute(pool)
        .await;
    let code = mint_prefixed(CODE_PREFIX);
    let hash = sha256_hex(&code);
    let inserted = sqlx::query(
        r#"insert into mcp_oauth_codes
           (code_hash, owner_id, client_id, code_challenge, redirect_uri, expires_at)
           values ($1, $2, $3, $4, $5, now() + interval '5 minutes')"#,
    )
    .bind(&hash)
    .bind(owner)
    .bind(&checked.client_id)
    .bind(&checked.challenge)
    .bind(&checked.redirect_uri)
    .execute(pool)
    .await;
    if let Err(e) = inserted {
        tracing::error!("mcp oauth code insert failed: {e}");
        return unavailable_page();
    }
    code_redirect(&checked.redirect_uri, &code, checked.state.as_deref())
}

async fn exchange_code(
    pool: &PgPool,
    owner: i32,
    client_id: &str,
    code: &str,
    redirect: &str,
    verifier: &str,
) -> Result<String, ExchangeErr> {
    let hash = sha256_hex(code);
    let mut tx = pool.begin().await.map_err(|e| {
        tracing::error!("mcp oauth begin: {e}");
        ExchangeErr::Server
    })?;
    let row = sqlx::query(
        r#"update mcp_oauth_codes
           set used_at = now()
           where code_hash = $1 and used_at is null and expires_at > now()
           returning owner_id, client_id, code_challenge, redirect_uri"#,
    )
    .bind(&hash)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| {
        tracing::error!("mcp oauth code consume: {e}");
        ExchangeErr::Server
    })?;
    let Some(row) = row else {
        let _ = tx.rollback().await;
        return Err(ExchangeErr::InvalidGrant);
    };
    let code_owner: i32 = row.get("owner_id");
    let stored_client: String = row.get("client_id");
    let challenge: String = row.get("code_challenge");
    let stored_redirect: String = row.get("redirect_uri");
    let ok = code_owner == owner
        && stored_client == client_id
        && stored_redirect == redirect
        && pkce_matches(verifier, &challenge);
    if !ok {
        tx.commit().await.map_err(|e| {
            tracing::error!("mcp oauth burn commit: {e}");
            ExchangeErr::Server
        })?;
        return Err(ExchangeErr::InvalidGrant);
    }
    let refresh = mint_prefixed(REFRESH_PREFIX);
    insert_refresh(&mut tx, owner, client_id, &refresh).await?;
    tx.commit().await.map_err(|e| {
        tracing::error!("mcp oauth code commit: {e}");
        ExchangeErr::Server
    })?;
    Ok(refresh)
}

async fn exchange_refresh(
    pool: &PgPool,
    owner: i32,
    client_id: &str,
    raw: &str,
) -> Result<String, ExchangeErr> {
    let hash = sha256_hex(raw);
    let mut tx = pool.begin().await.map_err(|e| {
        tracing::error!("mcp oauth begin: {e}");
        ExchangeErr::Server
    })?;
    let row = sqlx::query(
        r#"update mcp_oauth_refresh
           set used_at = now()
           where token_hash = $1 and used_at is null and expires_at > now()
           returning owner_id, client_id"#,
    )
    .bind(&hash)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| {
        tracing::error!("mcp oauth refresh consume: {e}");
        ExchangeErr::Server
    })?;
    if let Some(row) = row {
        let token_owner: i32 = row.get("owner_id");
        let stored_client: String = row.get("client_id");
        if token_owner != owner || stored_client != client_id {
            revoke_refresh_family(&mut tx, token_owner).await?;
            tx.commit().await.map_err(|_| ExchangeErr::Server)?;
            return Err(ExchangeErr::InvalidGrant);
        }
        let refresh = mint_prefixed(REFRESH_PREFIX);
        insert_refresh(&mut tx, owner, client_id, &refresh).await?;
        tx.commit().await.map_err(|_| ExchangeErr::Server)?;
        return Ok(refresh);
    }
    let existing = sqlx::query("select owner_id from mcp_oauth_refresh where token_hash = $1")
        .bind(&hash)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| {
            tracing::error!("mcp oauth refresh lookup: {e}");
            ExchangeErr::Server
        })?;
    if let Some(row) = existing {
        let token_owner: i32 = row.get("owner_id");
        revoke_refresh_family(&mut tx, token_owner).await?;
        tx.commit().await.map_err(|_| ExchangeErr::Server)?;
        return Err(ExchangeErr::InvalidGrant);
    }
    let _ = tx.rollback().await;
    Err(ExchangeErr::InvalidGrant)
}

async fn insert_refresh(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    owner: i32,
    client_id: &str,
    refresh: &str,
) -> Result<(), ExchangeErr> {
    let hash = sha256_hex(refresh);
    sqlx::query(
        r#"insert into mcp_oauth_refresh (token_hash, owner_id, client_id, expires_at)
           values ($1, $2, $3, now() + interval '30 days')"#,
    )
    .bind(&hash)
    .bind(owner)
    .bind(client_id)
    .execute(&mut **tx)
    .await
    .map_err(|e| {
        tracing::error!("mcp oauth refresh insert: {e}");
        ExchangeErr::Server
    })?;
    Ok(())
}

async fn revoke_refresh_family(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    owner: i32,
) -> Result<(), ExchangeErr> {
    sqlx::query("delete from mcp_oauth_refresh where owner_id = $1")
        .bind(owner)
        .execute(&mut **tx)
        .await
        .map_err(|e| {
            tracing::error!("mcp oauth refresh revoke: {e}");
            ExchangeErr::Server
        })?;
    Ok(())
}

fn redirect_login(headers: &HeaderMap, peer: Option<IpAddr>, query: &str) -> Response {
    if query.len() > QUERY_MAX || has_ctl(query) {
        return rejected_page();
    }
    let encoded = percent_encode_strict(query);
    if encoded.len() > 4096 {
        return rejected_page();
    }
    let mut res = redirect_to("/login/");
    res.headers_mut().append(
        SET_COOKIE,
        auth::set_oauth_resume_cookie(&encoded, auth::cookie_secure(headers, peer)),
    );
    res
}

/// Relative `/oauth/authorize?…` recovered after login. Never an absolute URL.
pub fn resume_location(headers: &HeaderMap) -> Option<String> {
    let encoded = auth::oauth_resume_cookie(headers)?;
    if encoded.len() > 4096
        || !encoded.bytes().all(|b| {
            matches!(b, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'%' | b'-' | b'.' | b'_' | b'~')
        })
    {
        return None;
    }
    let query = percent_decode_strict(&encoded)?;
    if query.len() > QUERY_MAX || has_ctl(&query) {
        return None;
    }
    let map = parse_form(&query).ok()?;
    let checked = check_authorize(&map).ok()?;
    let loc = format!("/oauth/authorize?{}", canonical_authorize_query(&checked));
    if relative_next_ok(&loc) {
        Some(loc)
    } else {
        None
    }
}

fn check_authorize(map: &BTreeMap<String, String>) -> Result<Checked, CheckFail> {
    let redirect = map.get("redirect_uri").map(String::as_str).unwrap_or("");
    if !redirect_allowed(redirect) {
        return Err(CheckFail::Reject);
    }
    let redirect_uri = redirect.to_string();
    let state_raw = map.get("state").map(String::as_str).unwrap_or("");
    let state = if state_raw.is_empty() {
        None
    } else if state_ok(state_raw) {
        Some(state_raw.to_string())
    } else {
        return Err(CheckFail::Bounce {
            redirect_uri,
            error: "invalid_request",
            state: None,
        });
    };
    let method = map
        .get("code_challenge_method")
        .map(String::as_str)
        .unwrap_or("");
    let challenge = map.get("code_challenge").map(String::as_str).unwrap_or("");
    let client_id = map.get("client_id").map(String::as_str).unwrap_or("");
    let error = if !client_id_ok(client_id) {
        Some("unauthorized_client")
    } else if map.get("resource").is_some_and(|r| !resource_ok(r)) {
        Some("invalid_request")
    } else if map.get("response_type").map(String::as_str) != Some("code")
        || map
            .get("response_mode")
            .map(String::as_str)
            .is_some_and(|m| m != "query")
        || method != "S256"
        || !challenge_ok(challenge)
    {
        Some("invalid_request")
    } else if !scope_ok(map.get("scope").map(String::as_str)) {
        Some("invalid_scope")
    } else {
        None
    };
    if let Some(error) = error {
        return Err(CheckFail::Bounce {
            redirect_uri,
            error,
            state,
        });
    }
    let scope = map
        .get("scope")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|_| "mcp".to_string());
    Ok(Checked {
        client_id: client_id.to_string(),
        redirect_uri,
        state,
        challenge: challenge.to_string(),
        scope,
    })
}

fn redirect_allowed(uri: &str) -> bool {
    if uri.contains(['?', '#', '\\', '@', ' ', '\n', '\r']) || uri.len() > 256 {
        return false;
    }
    uri == REDIRECT_WITH_SLASH
        || uri == REDIRECT_BARE
        || uri == CURSOR_HTTPS
        || uri == CURSOR_APP
        || loopback_callback(uri)
}

/// `http://127.0.0.1:port/callback`, `http://localhost:port/callback`, or the
/// same hosts with `/oauth/callback`. One optional trailing slash. No other
/// scheme, host, or path.
fn loopback_callback(uri: &str) -> bool {
    let Some(rest) = uri.strip_prefix("http://") else {
        return false;
    };
    let Some((hostport, path)) = rest.split_once('/') else {
        return false;
    };
    let path = path.trim_end_matches('/');
    if path != "callback" && path != "oauth/callback" {
        return false;
    }
    let hostport = hostport.to_ascii_lowercase();
    let (host, port) = if let Some(port) = hostport.strip_prefix("[::1]:") {
        ("::1", port)
    } else {
        let Some((host, port)) = hostport.rsplit_once(':') else {
            return false;
        };
        (host, port)
    };
    if host != "127.0.0.1" && host != "localhost" && host != "::1" {
        return false;
    }
    matches!(port.parse::<u16>(), Ok(p) if p != 0)
}

fn client_id_ok(id: &str) -> bool {
    (1..=128).contains(&id.len())
        && id.bytes().all(
            |b| matches!(b, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~'),
        )
}

fn resource_ok(raw: &str) -> bool {
    resource().is_some_and(|expected| raw == expected)
}

fn scope_ok(scope: Option<&str>) -> bool {
    match scope.map(str::trim).filter(|s| !s.is_empty()) {
        None => true,
        Some("mcp") => true,
        Some(_) => false,
    }
}

fn state_ok(s: &str) -> bool {
    (1..=256).contains(&s.len())
        && s.bytes().all(
            |b| matches!(b, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'~' | b'-'),
        )
}

fn challenge_ok(s: &str) -> bool {
    s.len() == 43
        && s.bytes()
            .all(|b| matches!(b, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_'))
}

fn verifier_ok(s: &str) -> bool {
    (43..=128).contains(&s.len())
        && s.bytes().all(
            |b| matches!(b, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~'),
        )
}

fn prefixed_ok(raw: &str, prefix: &str) -> bool {
    let Some(rest) = raw.strip_prefix(prefix) else {
        return false;
    };
    (43..=128).contains(&rest.len())
        && rest
            .bytes()
            .all(|b| matches!(b, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_'))
}

fn pkce_matches(verifier: &str, challenge: &str) -> bool {
    if !verifier_ok(verifier) || !challenge_ok(challenge) {
        return false;
    }
    let digest = Sha256::digest(verifier.as_bytes());
    let got = URL_SAFE_NO_PAD.encode(digest);
    ct_eq(got.as_bytes(), challenge.as_bytes())
}

fn presented_client_id(
    map: &BTreeMap<String, String>,
    authorization: Option<&str>,
) -> Option<String> {
    if map.get("client_secret").is_some_and(|s| !s.is_empty()) {
        return None;
    }
    let basic_id = basic_public_client(authorization).ok()?;
    let form = map.get("client_id").map(String::as_str);
    let id = match (form, basic_id.as_deref()) {
        (Some(id), None) | (None, Some(id)) => id,
        (Some(form_id), Some(basic)) if form_id == basic => form_id,
        _ => return None,
    };
    client_id_ok(id).then(|| id.to_string())
}

fn client_auth_ok(map: &BTreeMap<String, String>, authorization: Option<&str>) -> bool {
    presented_client_id(map, authorization).is_some()
}

fn basic_public_client(authorization: Option<&str>) -> Result<Option<String>, ()> {
    let Some(header) = authorization.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    let Some(b64) = header
        .strip_prefix("Basic ")
        .or_else(|| header.strip_prefix("basic "))
    else {
        return Err(());
    };
    let b64 = b64.trim();
    let bytes = STANDARD
        .decode(b64.as_bytes())
        .or_else(|_| URL_SAFE_NO_PAD.decode(b64.as_bytes()))
        .map_err(|_| ())?;
    let text = String::from_utf8(bytes).map_err(|_| ())?;
    let Some((id, secret)) = text.split_once(':') else {
        return Err(());
    };
    if !client_id_ok(id) || !secret.is_empty() {
        return Err(());
    }
    Ok(Some(id.to_string()))
}

fn canonical_authorize_query(c: &Checked) -> String {
    let mut ser = url::form_urlencoded::Serializer::new(String::new());
    ser.append_pair("response_type", "code");
    ser.append_pair("client_id", &c.client_id);
    ser.append_pair("redirect_uri", &c.redirect_uri);
    ser.append_pair("code_challenge", &c.challenge);
    ser.append_pair("code_challenge_method", "S256");
    if c.scope.is_some() {
        ser.append_pair("scope", "mcp");
    }
    if let Some(state) = &c.state {
        ser.append_pair("state", state);
    }
    ser.finish()
}

fn parse_form(input: &str) -> Result<BTreeMap<String, String>, ()> {
    let mut map = BTreeMap::new();
    if input.is_empty() {
        return Ok(map);
    }
    let mut n = 0usize;
    for pair in input.split('&') {
        if pair.is_empty() {
            continue;
        }
        n += 1;
        if n > 32 {
            return Err(());
        }
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        if k.is_empty() || k.len() > 64 || v.len() > 2048 {
            return Err(());
        }
        let k = percent_decode_form(k).ok_or(())?;
        let v = percent_decode_form(v).ok_or(())?;
        if k.len() > 64
            || v.len() > 2048
            || has_ctl(&k)
            || has_ctl(&v)
            || k.chars().any(|c| !c.is_ascii())
        {
            return Err(());
        }
        if tracked_key(&k) && map.insert(k, v).is_some() {
            return Err(());
        }
    }
    Ok(map)
}

fn tracked_key(k: &str) -> bool {
    matches!(
        k,
        "response_type"
            | "client_id"
            | "redirect_uri"
            | "scope"
            | "state"
            | "code_challenge"
            | "code_challenge_method"
            | "response_mode"
            | "grant_type"
            | "code"
            | "code_verifier"
            | "refresh_token"
            | "client_secret"
            | "decision"
            | "resource"
    )
}

fn sha256_hex(raw: &str) -> String {
    let dig = Sha256::digest(raw.as_bytes());
    dig.iter().map(|b| format!("{b:02x}")).collect()
}

fn mint_prefixed(prefix: &str) -> String {
    let mut buf = [0u8; 32];
    rand::rng().fill_bytes(&mut buf);
    format!("{prefix}{}", URL_SAFE_NO_PAD.encode(buf))
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

fn env_secret(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn peer_of(req: &Request) -> Option<IpAddr> {
    req.extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0.ip())
}

fn client_ip(headers: &HeaderMap, peer: Option<IpAddr>) -> String {
    if auth::peer_trusts_xfh(peer) {
        if let Some(xff) = headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.split(',').next())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return xff.to_string();
        }
    }
    peer.map(|p| p.to_string())
        .unwrap_or_else(|| "0.0.0.0".to_string())
}

fn form_content_type(headers: &HeaderMap) -> bool {
    headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| {
            v.split(';')
                .next()
                .unwrap_or("")
                .trim()
                .eq_ignore_ascii_case("application/x-www-form-urlencoded")
        })
        .unwrap_or(false)
}

async fn read_body(body: axum::body::Body) -> Result<String, ()> {
    let bytes = to_bytes(body, BODY_MAX).await.map_err(|_| ())?;
    if bytes.len() > BODY_MAX {
        return Err(());
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| ())?.trim();
    if text.len() > BODY_MAX || has_ctl(text) {
        return Err(());
    }
    Ok(text.to_string())
}

fn has_ctl(s: &str) -> bool {
    s.bytes().any(|b| b < 0x20 || b == 0x7f)
}

fn percent_encode_strict(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for b in input.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn percent_decode_strict(input: &str) -> Option<String> {
    percent_decode(input, false)
}

fn percent_decode_form(input: &str) -> Option<String> {
    percent_decode(input, true)
}

fn percent_decode(input: &str, plus_is_space: bool) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                if i + 2 >= bytes.len() {
                    return None;
                }
                let h = hex_val(bytes[i + 1])?;
                let l = hex_val(bytes[i + 2])?;
                out.push((h << 4) | l);
                i += 3;
            }
            b'+' if plus_is_space => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

fn page(title: &str, inner: &str) -> String {
    format!(
        "<!doctype html><html lang=\"fr\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>{}</title></head><body><main>{}</main></body></html>",
        escape_html(title),
        inner
    )
}

fn html(status: StatusCode, body: &str) -> Response {
    let mut res = (status, body.to_string()).into_response();
    let h = res.headers_mut();
    h.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    h.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    res
}

fn rejected_page() -> Response {
    html(
        StatusCode::BAD_REQUEST,
        &page(
            "Demande refusée",
            "<h1>Demande refusée</h1><p>Cette demande d'autorisation est refusée.</p>",
        ),
    )
}

fn owner_only_page() -> Response {
    html(
        StatusCode::FORBIDDEN,
        &page(
            "Accès refusé",
            &format!("<h1>Accès refusé</h1><p>{OWNER_ONLY}</p>"),
        ),
    )
}

fn not_configured_page() -> Response {
    html(
        StatusCode::NOT_FOUND,
        &page(
            "Connecteur indisponible",
            "<h1>Connecteur indisponible</h1><p>Le connecteur n'est pas disponible.</p>",
        ),
    )
}

fn unavailable_page() -> Response {
    html(
        StatusCode::SERVICE_UNAVAILABLE,
        &page(
            "Service indisponible",
            "<h1>Service indisponible</h1><p>Réessayez dans un moment.</p>",
        ),
    )
}

fn rate_page() -> Response {
    html(
        StatusCode::TOO_MANY_REQUESTS,
        &page(
            "Trop de demandes",
            "<h1>Trop de demandes</h1><p>Réessayez dans une minute.</p>",
        ),
    )
}

fn consent_page(c: &Checked) -> Response {
    html(StatusCode::OK, &consent_html(c))
}

fn consent_html(c: &Checked) -> String {
    let redir = escape_html(&c.redirect_uri);
    let challenge = escape_html(&c.challenge);
    let state_input = match &c.state {
        Some(s) => format!(
            "<input type=\"hidden\" name=\"state\" value=\"{}\">",
            escape_html(s)
        ),
        None => String::new(),
    };
    let scope_input = if c.scope.is_some() {
        "<input type=\"hidden\" name=\"scope\" value=\"mcp\">"
    } else {
        ""
    };
    let client = escape_html(&c.client_id);
    let inner = format!(
        "<h1>Autoriser le connecteur</h1>\
         <p>Un connecteur externe demande l'accès à cette archive : lecture des documents, modification des étiquettes et consultation des journaux de diagnostic. Le jeton d'archive ne quitte pas ce serveur.</p>\
         <p>Adresse de retour : <code>{redir}</code></p>\
         <form method=\"post\" action=\"/oauth/authorize\" accept-charset=\"utf-8\">\
         <input type=\"hidden\" name=\"response_type\" value=\"code\">\
         <input type=\"hidden\" name=\"client_id\" value=\"{client}\">\
         <input type=\"hidden\" name=\"redirect_uri\" value=\"{redir}\">\
         <input type=\"hidden\" name=\"code_challenge\" value=\"{challenge}\">\
         <input type=\"hidden\" name=\"code_challenge_method\" value=\"S256\">\
         {scope_input}{state_input}\
         <p><button type=\"submit\" name=\"decision\" value=\"allow\">Autoriser</button> \
         <button type=\"submit\" name=\"decision\" value=\"deny\">Refuser</button></p>\
         </form>"
    );
    page("Autoriser le connecteur", &inner)
}

fn redirect_to(loc: &str) -> Response {
    let Ok(value) = HeaderValue::from_str(loc) else {
        return rejected_page();
    };
    if !location_ok(loc) {
        return rejected_page();
    }
    let mut res = StatusCode::FOUND.into_response();
    let h = res.headers_mut();
    h.insert(LOCATION, value);
    h.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    res
}

fn location_ok(loc: &str) -> bool {
    if loc.is_empty() || loc.len() > 2048 || has_ctl(loc) || loc.contains('\\') {
        return false;
    }
    if loc == "/login/" || relative_next_ok(loc) {
        return true;
    }
    match loc.split_once('?') {
        Some((base, query)) => redirect_allowed(base) && !query.is_empty() && query.len() < 1024,
        None => false,
    }
}

fn relative_next_ok(loc: &str) -> bool {
    loc.starts_with("/oauth/authorize?")
        && loc.len() <= 2048
        && !has_ctl(loc)
        && !loc.contains('\\')
        && !loc.contains("//")
}

fn bounce(redirect_uri: &str, error: &str, state: Option<&str>) -> Response {
    if !matches!(
        error,
        "invalid_request" | "unauthorized_client" | "invalid_scope" | "access_denied"
    ) {
        return rejected_page();
    }
    let mut ser = url::form_urlencoded::Serializer::new(String::new());
    ser.append_pair("error", error);
    if let Some(state) = state.filter(|s| state_ok(s)) {
        ser.append_pair("state", state);
    }
    redirect_to(&format!("{redirect_uri}?{}", ser.finish()))
}

fn code_redirect(redirect_uri: &str, code: &str, state: Option<&str>) -> Response {
    if !prefixed_ok(code, CODE_PREFIX) {
        return rejected_page();
    }
    let mut ser = url::form_urlencoded::Serializer::new(String::new());
    ser.append_pair("code", code);
    if let Some(state) = state.filter(|s| state_ok(s)) {
        ser.append_pair("state", state);
    }
    redirect_to(&format!("{redirect_uri}?{}", ser.finish()))
}

fn oauth_error(status: StatusCode, error: &'static str) -> Response {
    let mut res = (status, Json(serde_json::json!({"error": error}))).into_response();
    let h = res.headers_mut();
    h.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(PRAGMA, HeaderValue::from_static("no-cache"));
    res
}

pub fn unauthorized_challenge() -> Response {
    let Some(iss) = issuer() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let meta = format!("{iss}/.well-known/oauth-protected-resource/api/mcp");
    let mut res = (
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({"error": "unauthorized"})),
    )
        .into_response();
    let value = format!("Bearer realm=\"paperrust\", resource_metadata=\"{meta}\", scope=\"mcp\"");
    if let Ok(header) = HeaderValue::from_str(&value) {
        res.headers_mut()
            .insert(axum::http::header::WWW_AUTHENTICATE, header);
    }
    res.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    res
}

fn metadata_response(body: serde_json::Value) -> Response {
    let mut res = (StatusCode::OK, Json(body)).into_response();
    let headers = res.headers_mut();
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    res
}

pub fn protected_resource_metadata() -> Option<serde_json::Value> {
    let iss = issuer()?;
    let res = resource()?;
    Some(serde_json::json!({
        "resource": res,
        "authorization_servers": [iss],
        "scopes_supported": ["mcp"],
        "bearer_methods_supported": ["header"],
        "resource_name": "Paperrust"
    }))
}

pub fn authorization_server_metadata() -> Option<serde_json::Value> {
    let iss = issuer()?;
    Some(serde_json::json!({
        "issuer": iss,
        "authorization_endpoint": format!("{iss}/oauth/authorize"),
        "token_endpoint": format!("{iss}/oauth/token"),
        "registration_endpoint": format!("{iss}/oauth/register"),
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "response_modes_supported": ["query"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["none"],
        "scopes_supported": ["mcp"]
    }))
}

fn not_configured() -> Response {
    StatusCode::NOT_FOUND.into_response()
}

pub async fn protected_resource() -> Response {
    match protected_resource_metadata() {
        Some(body) => metadata_response(body),
        None => not_configured(),
    }
}

pub async fn authorization_server() -> Response {
    match authorization_server_metadata() {
        Some(body) => metadata_response(body),
        None => not_configured(),
    }
}

fn register_redirects(body: &serde_json::Value) -> Result<Vec<String>, &'static str> {
    let Some(uris) = body.get("redirect_uris").and_then(|v| v.as_array()) else {
        return Err("invalid_client_metadata");
    };
    if uris.is_empty() || uris.len() > 8 {
        return Err("invalid_client_metadata");
    }
    let mut out = Vec::new();
    for uri in uris {
        let Some(uri) = uri.as_str() else {
            return Err("invalid_redirect_uri");
        };
        if !redirect_allowed(uri) {
            return Err("invalid_redirect_uri");
        }
        if !out.iter().any(|seen: &String| seen == uri) {
            out.push(uri.to_string());
        }
    }
    if body
        .get("client_secret")
        .and_then(|v| v.as_str())
        .is_some_and(|s| !s.is_empty())
    {
        return Err("invalid_client_metadata");
    }
    if let Some(method) = body
        .get("token_endpoint_auth_method")
        .and_then(|v| v.as_str())
    {
        if method != "none" {
            return Err("invalid_client_metadata");
        }
    }
    if let Some(grants) = body.get("grant_types").and_then(|v| v.as_array()) {
        let ok = !grants.is_empty()
            && grants.iter().all(|g| {
                matches!(
                    g.as_str(),
                    Some("authorization_code") | Some("refresh_token")
                )
            });
        if !ok {
            return Err("invalid_client_metadata");
        }
    }
    if let Some(types) = body.get("response_types").and_then(|v| v.as_array()) {
        if types.is_empty() || types.iter().any(|t| t.as_str() != Some("code")) {
            return Err("invalid_client_metadata");
        }
    }
    Ok(out)
}

pub async fn register(req: Request) -> Response {
    let peer = peer_of(&req);
    let (parts, body) = req.into_parts();
    if !rate_limit_ok(
        &format!("mcp-oauth-register:{}", client_ip(&parts.headers, peer)),
        30,
        60,
    ) {
        return oauth_error(StatusCode::TOO_MANY_REQUESTS, "temporarily_unavailable");
    }
    let ctype = parts
        .headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !ctype.to_ascii_lowercase().starts_with("application/json") {
        return oauth_error(StatusCode::BAD_REQUEST, "invalid_client_metadata");
    }
    let text = match read_body(body).await {
        Ok(t) => t,
        Err(()) => return oauth_error(StatusCode::BAD_REQUEST, "invalid_client_metadata"),
    };
    let parsed: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => return oauth_error(StatusCode::BAD_REQUEST, "invalid_client_metadata"),
    };
    let redirects = match register_redirects(&parsed) {
        Ok(v) => v,
        Err(error) => return oauth_error(StatusCode::BAD_REQUEST, error),
    };
    let client_id = mint_prefixed("cid");
    let mut res = (
        StatusCode::CREATED,
        Json(serde_json::json!({
            "client_id": client_id,
            "redirect_uris": redirects,
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
            "scope": "mcp",
        })),
    )
        .into_response();
    let headers = res.headers_mut();
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(PRAGMA, HeaderValue::from_static("no-cache"));
    res
}

fn token_ok(bearer: String, refresh: String) -> Response {
    let mut res = (
        StatusCode::OK,
        Json(serde_json::json!({
            "access_token": bearer,
            "token_type": "Bearer",
            "expires_in": ACCESS_EXPIRES_IN,
            "refresh_token": refresh,
            "scope": "mcp",
        })),
    )
        .into_response();
    let h = res.headers_mut();
    h.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(PRAGMA, HeaderValue::from_static("no-cache"));
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::header::COOKIE;

    const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    const CHALLENGE: &str = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";

    fn good_query() -> String {
        format!(
            "response_type=code&client_id=paperrust&redirect_uri={}&code_challenge={CHALLENGE}&code_challenge_method=S256&scope=mcp&state=abc.def~_-",
            percent_encode_strict(REDIRECT_WITH_SLASH)
        )
    }

    fn cookie_headers(value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(
            COOKIE,
            HeaderValue::from_str(&format!("{}={value}", auth::OAUTH_RESUME_COOKIE)).unwrap(),
        );
        h
    }

    #[test]
    fn pkce_rfc7636_vector() {
        assert!(pkce_matches(VERIFIER, CHALLENGE));
        assert!(!pkce_matches(
            VERIFIER,
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cN"
        ));
        assert!(!pkce_matches("plain", CHALLENGE));
        assert!(verifier_ok(&"a".repeat(43)));
        assert!(verifier_ok(&"a".repeat(128)));
        assert!(!verifier_ok(&"a".repeat(42)));
        assert!(!verifier_ok(&"a".repeat(129)));
        assert!(!verifier_ok(&format!("{}!", "a".repeat(42))));
        assert!(!challenge_ok(&format!("{CHALLENGE}=")));
    }

    #[test]
    fn redirect_allowlist_is_exact() {
        assert!(redirect_allowed(REDIRECT_WITH_SLASH));
        assert!(redirect_allowed(REDIRECT_BARE));
        assert!(!redirect_allowed(
            "https://grok.com/connectors-oauth-exchange-code/extra"
        ));
        assert!(!redirect_allowed(
            "https://grok.com.evil/connectors-oauth-exchange-code/"
        ));
        assert!(!redirect_allowed("https://evil.example/cb"));
        assert!(!redirect_allowed(
            "https://user@grok.com/connectors-oauth-exchange-code/"
        ));
        assert!(!redirect_allowed(
            "//grok.com/connectors-oauth-exchange-code/"
        ));
        assert!(!redirect_allowed(&format!("{REDIRECT_WITH_SLASH}?x=1")));
        assert!(redirect_allowed(CURSOR_HTTPS));
        assert!(redirect_allowed(CURSOR_APP));
        assert!(redirect_allowed("http://127.0.0.1:8787/callback"));
        assert!(redirect_allowed("http://localhost:43123/callback/"));
        assert!(redirect_allowed("http://[::1]:8787/oauth/callback"));
        assert!(!redirect_allowed("https://127.0.0.1:8787/callback"));
        assert!(!redirect_allowed("http://127.0.0.1:8787/steal"));
        assert!(!redirect_allowed("http://localhost.evil/callback"));
        assert!(!redirect_allowed("http://127.0.0.1:0/callback"));
        assert!(!redirect_allowed("https://www.cursor.com.evil/agents/mcp/oauth/callback"));
        assert!(!redirect_allowed(&format!("{CURSOR_APP}/extra")));
    }

    #[test]
    fn scope_is_only_mcp() {
        assert!(scope_ok(None));
        assert!(scope_ok(Some("")));
        assert!(scope_ok(Some("  mcp  ")));
        assert!(!scope_ok(Some("mcp extra")));
        assert!(!scope_ok(Some("offline_access")));
    }

    #[test]
    fn authorize_rejects_bad_client_without_leaving_the_allowlist() {
        let mut map = parse_form(&good_query()).unwrap();
        map.insert("client_id".into(), "bad id".into());
        match check_authorize(&map) {
            Err(CheckFail::Bounce {
                error,
                redirect_uri,
                ..
            }) => {
                assert_eq!(error, "unauthorized_client");
                assert_eq!(redirect_uri, REDIRECT_WITH_SLASH);
            }
            other => panic!("expected bounce, got {other:?}"),
        }
    }

    #[test]
    fn bad_redirect_is_not_a_bounce() {
        let q = format!(
            "response_type=code&client_id=paperrust&redirect_uri={}&code_challenge={CHALLENGE}&code_challenge_method=S256",
            percent_encode_strict("https://evil.example/cb")
        );
        let map = parse_form(&q).unwrap();
        assert!(matches!(check_authorize(&map), Err(CheckFail::Reject)));
    }

    #[test]
    fn pkce_plain_and_bad_state_do_not_echo_state() {
        let mut map = parse_form(&good_query()).unwrap();
        map.insert("code_challenge_method".into(), "plain".into());
        assert!(matches!(
            check_authorize(&map),
            Err(CheckFail::Bounce {
                error: "invalid_request",
                ..
            })
        ));
        let mut map = parse_form(&good_query()).unwrap();
        map.insert("state".into(), "bad state".into());
        match check_authorize(&map) {
            Err(CheckFail::Bounce {
                state: None, error, ..
            }) => {
                assert_eq!(error, "invalid_request");
            }
            other => panic!("expected silent state, got {other:?}"),
        }
    }

    #[test]
    fn duplicate_redirect_is_rejected() {
        let q = format!(
            "{}&redirect_uri={}",
            good_query(),
            percent_encode_strict("https://evil.example/cb")
        );
        assert!(parse_form(&q).is_err());
    }

    #[test]
    fn public_client_id_is_accepted_on_an_allowlisted_redirect() {
        let mut map = parse_form(&good_query()).unwrap();
        map.insert("client_id".into(), "cid-bot.1".into());
        let checked = check_authorize(&map).expect("public client");
        assert_eq!(checked.client_id, "cid-bot.1");
        let q = format!(
            "response_type=code&client_id=cid-bot.1&redirect_uri={}&code_challenge={CHALLENGE}&code_challenge_method=S256",
            percent_encode_strict(CURSOR_HTTPS)
        );
        assert!(check_authorize(&parse_form(&q).unwrap()).is_ok());
        let q = format!(
            "response_type=code&client_id=cid-bot.1&redirect_uri={}&code_challenge={CHALLENGE}&code_challenge_method=S256",
            percent_encode_strict("http://127.0.0.1:8787/callback")
        );
        assert!(check_authorize(&parse_form(&q).unwrap()).is_ok());
    }

    #[test]
    fn resource_must_name_this_mcp_server() {
        let _guard = crate::instance::env_lock().lock().unwrap();
        let prev = std::env::var("PAPERRUST_PUBLIC_URL").ok();
        unsafe {
            std::env::set_var("PAPERRUST_PUBLIC_URL", "https://archive.example");
        }
        let q = format!(
            "{}&resource={}",
            good_query(),
            percent_encode_strict("https://archive.example/api/mcp")
        );
        let map = parse_form(&q).unwrap();
        assert!(check_authorize(&map).is_ok());
        assert_eq!(
            map.get("resource").map(String::as_str),
            Some("https://archive.example/api/mcp")
        );
        let q = format!(
            "{}&resource={}",
            good_query(),
            percent_encode_strict("https://evil.example/api/mcp")
        );
        match check_authorize(&parse_form(&q).unwrap()) {
            Err(CheckFail::Bounce {
                error: "invalid_request",
                ..
            }) => {}
            other => panic!("expected bounce, got {other:?}"),
        }
        unsafe {
            match prev {
                Some(v) => std::env::set_var("PAPERRUST_PUBLIC_URL", v),
                None => std::env::remove_var("PAPERRUST_PUBLIC_URL"),
            }
        }
    }

    fn with_public_url<T>(f: impl FnOnce() -> T) -> T {
        let _guard = crate::instance::env_lock().lock().unwrap();
        let prev = std::env::var("PAPERRUST_PUBLIC_URL").ok();
        unsafe {
            std::env::set_var("PAPERRUST_PUBLIC_URL", "https://archive.example");
        }
        let out = f();
        unsafe {
            match prev {
                Some(v) => std::env::set_var("PAPERRUST_PUBLIC_URL", v),
                None => std::env::remove_var("PAPERRUST_PUBLIC_URL"),
            }
        }
        out
    }

    #[test]
    fn discovery_documents_name_the_public_issuer() {
        with_public_url(|| {
            let protected = protected_resource_metadata().expect("issuer");
            assert_eq!(protected["resource"], "https://archive.example/api/mcp");
            assert_eq!(protected["authorization_servers"][0], "https://archive.example");
            assert_eq!(protected["scopes_supported"][0], "mcp");
            let server = authorization_server_metadata().expect("issuer");
            assert_eq!(server["issuer"], "https://archive.example");
            assert_eq!(
                server["authorization_endpoint"],
                "https://archive.example/oauth/authorize"
            );
            assert_eq!(server["token_endpoint"], "https://archive.example/oauth/token");
            assert_eq!(
                server["registration_endpoint"],
                "https://archive.example/oauth/register"
            );
            assert_eq!(server["code_challenge_methods_supported"][0], "S256");
            assert_eq!(server["token_endpoint_auth_methods_supported"][0], "none");
        });
    }

    #[test]
    fn discovery_is_absent_without_public_url() {
        let _guard = crate::instance::env_lock().lock().unwrap();
        let prev_url = std::env::var("PAPERRUST_PUBLIC_URL").ok();
        let prev_urls = std::env::var("PAPERRUST_PUBLIC_URLS").ok();
        unsafe {
            std::env::remove_var("PAPERRUST_PUBLIC_URL");
            std::env::remove_var("PAPERRUST_PUBLIC_URLS");
        }
        assert!(protected_resource_metadata().is_none());
        assert!(authorization_server_metadata().is_none());
        assert_eq!(unauthorized_challenge().status(), StatusCode::NOT_FOUND);
        unsafe {
            match prev_url {
                Some(v) => std::env::set_var("PAPERRUST_PUBLIC_URL", v),
                None => std::env::remove_var("PAPERRUST_PUBLIC_URL"),
            }
            match prev_urls {
                Some(v) => std::env::set_var("PAPERRUST_PUBLIC_URLS", v),
                None => std::env::remove_var("PAPERRUST_PUBLIC_URLS"),
            }
        }
    }

    #[test]
    fn registration_accepts_only_allowlisted_redirects() {
        let ok = serde_json::json!({
            "redirect_uris": [CURSOR_HTTPS, CURSOR_APP, "http://localhost:8787/callback"],
            "token_endpoint_auth_method": "none",
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"]
        });
        let uris = register_redirects(&ok).expect("bot registration");
        assert_eq!(uris.len(), 3);
        let mixed = serde_json::json!({
            "redirect_uris": [CURSOR_HTTPS, "https://evil.example/cb"]
        });
        assert_eq!(
            register_redirects(&mixed).unwrap_err(),
            "invalid_redirect_uri"
        );
        let secret = serde_json::json!({
            "redirect_uris": [CURSOR_HTTPS],
            "client_secret": "nope"
        });
        assert_eq!(
            register_redirects(&secret).unwrap_err(),
            "invalid_client_metadata"
        );
    }

    #[test]
    fn resume_stays_on_our_authorize_path() {
        let enc = percent_encode_strict(&good_query());
        let loc = resume_location(&cookie_headers(&enc)).expect("resume");
        assert!(loc.starts_with("/oauth/authorize?"));
        assert!(!loc.starts_with("http"));
        assert!(!loc.contains("//"));
        assert!(loc.contains("client_id=paperrust"));
        assert!(loc.contains(&percent_encode_strict(REDIRECT_WITH_SLASH)));
    }

    #[test]
    fn resume_rejects_open_redirects() {
        for raw in [
            "https://evil.example/cb",
            "//evil.example",
            "/oauth/authorize?redirect_uri=https://evil.example",
            "https://grok.com.evil/connectors-oauth-exchange-code/",
        ] {
            let enc = percent_encode_strict(raw);
            assert!(resume_location(&cookie_headers(&enc)).is_none(), "{raw}");
        }
        let evil = format!(
            "response_type=code&client_id=paperrust&redirect_uri={}&code_challenge={CHALLENGE}&code_challenge_method=S256",
            percent_encode_strict("https://grok.com.evil/connectors-oauth-exchange-code/")
        );
        assert!(resume_location(&cookie_headers(&percent_encode_strict(&evil))).is_none());
    }

    #[test]
    fn client_secret_is_rejected() {
        let mut map = BTreeMap::new();
        map.insert("client_id".into(), CLIENT_ID.into());
        assert!(client_auth_ok(&map, None));
        map.insert("client_secret".into(), "secret".into());
        assert!(!client_auth_ok(&map, None));
        map.insert("client_secret".into(), "   ".into());
        assert!(!client_auth_ok(&map, None));
        map.remove("client_secret");
        let basic = STANDARD.encode(b"paperrust:");
        assert!(client_auth_ok(&map, Some(&format!("Basic {basic}"))));
        let basic = STANDARD.encode(b"paperrust:nope");
        assert!(!client_auth_ok(&map, Some(&format!("Basic {basic}"))));
        assert!(!client_auth_ok(&map, Some("Bearer aaa")));
        map.insert("client_id".into(), "cid-bot.1".into());
        assert!(client_auth_ok(&map, None));
        map.insert("client_id".into(), "bad id".into());
        assert!(!client_auth_ok(&map, None));
    }

    #[test]
    fn html_escape_and_consent_copy() {
        assert_eq!(escape_html("a<b>&\"'"), "a&lt;b&gt;&amp;&quot;&#39;");
        let checked = check_authorize(&parse_form(&good_query()).unwrap()).unwrap();
        let text = consent_html(&checked);
        assert!(text.contains("Autoriser le connecteur"));
        assert!(text.contains("Autoriser</button>"));
        assert!(text.contains("Refuser</button>"));
        assert!(text.contains("Le jeton d'archive ne quitte pas ce serveur."));
        assert!(text.contains(&escape_html(REDIRECT_WITH_SLASH)));
        assert!(!text.to_ascii_lowercase().contains("<script"));
        assert!(OWNER_ONLY.contains("propriétaire"));
    }
}
