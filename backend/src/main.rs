mod auth;
mod instance;
mod api_tokens;
mod jwks;
mod consume;
mod limits;
mod llm_keys;
mod matching;
mod models;
mod ocr;
mod ocr_ssrf;
mod paper;
mod preview;
mod queries;
mod paperless_import;
mod seed;
mod stirling_edit;

use axum::extract::{
    ConnectInfo, DefaultBodyLimit, Extension, FromRequest, Multipart, Path, Query, Request, State,
};
use axum::http::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use axum::middleware::{from_fn, from_fn_with_state, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::services::ServeDir;

use crate::auth::AuthUser;
use crate::models::*;

#[derive(Clone)]
struct AppState {
    pool: PgPool,
    paperless_imports: crate::paperless_import::ImportHub,
}

struct AppError(anyhow::Error);

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        tracing::error!("api error: {:#}", self.0);
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "request_failed"})),
        )
            .into_response()
    }
}

impl From<anyhow::Error> for AppError {
    fn from(err: anyhow::Error) -> Self {
        AppError(err)
    }
}

impl From<sqlx::Error> for AppError {
    fn from(err: sqlx::Error) -> Self {
        AppError(err.into())
    }
}

impl From<serde_json::Error> for AppError {
    fn from(err: serde_json::Error) -> Self {
        AppError(err.into())
    }
}

#[derive(Serialize)]
struct Health {
    ok: bool,
    service: &'static str,
    postgres: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    llm_keys::assert_crypto_ready()?;
    auth::assert_session_secret_ready()?;
    ocr_ssrf::validate_host_allowlists_from_env().map_err(|e| anyhow::anyhow!(e))?;
    instance::validate_issuer_config().map_err(|e| anyhow::anyhow!(e))?;

    let url = std::env::var("DATABASE_URL").map_err(|_| anyhow::anyhow!("DATABASE_URL is required"))?;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(8)
        .connect(&url)
        .await?;
    crate::preview::ensure_schema(&pool).await?;
    crate::queries::ensure_trash_schema(&pool).await?;
    auth::ensure_schema(&pool).await?;
    llm_keys::ensure_schema(&pool).await?;
    api_tokens::ensure_schema(&pool).await?;
    crate::paperless_import::ensure_schema(&pool).await?;
    crate::stirling_edit::ensure_schema(&pool).await?;
    seed::ensure_seeded(&pool).await?;
    let state = AppState {
        pool: pool.clone(),
        paperless_imports: crate::paperless_import::ImportHub::default(),
    };

    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(|origin: &HeaderValue, _| cors_origin_allowed(origin)))
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::OPTIONS,
            Method::PATCH,
        ])
        .allow_headers([
            CONTENT_TYPE,
            AUTHORIZATION,
            ACCEPT,
            HeaderName::from_static("x-requested-with"),
        ])
        .allow_credentials(true);

    // large body limit only on upload routes; other API routes stay at ~2 MiB.
    let upload_routes = Router::new()
        .route("/api/documents/consume", post(consume))
        .route("/api/v1/ingest", post(ingest_v1))
        // Paperless-ngx / Quickscan ingest (same token table; Token or Bearer).
        .route("/api/documents/post_document/", post(post_document_ngx))
        .route("/api/documents/post_document", post(post_document_ngx))
        .route(
            "/api/documents/{id}/edit/stirling/commit",
            post(stirling_commit_auth),
        )
        .route("/api/edit/stirling/{token}/commit", post(stirling_commit_token))
        .layer(DefaultBodyLimit::max(crate::limits::MAX_UPLOAD_BYTES))
        .layer(from_fn_with_state(state.clone(), auth_gate));

    let api_routes = Router::new()
        .route("/health", get(health))
        .route("/api/health", get(health))
        .route("/api/config", get(instance_config))
        .route("/api/login", get(oidc_login_alias).post(oidc_login_alias))
        .route("/api/logout", post(oidc_logout))
        .route("/api/auth", get(oidc_auth).post(oidc_login))
        .route("/api/auth/login", get(oidc_login).post(oidc_login))
        .route("/api/auth/login/", get(oidc_login).post(oidc_login))
        .route("/api/auth/logout", post(oidc_logout))
        .route("/api/auth/callback", get(oidc_callback).post(oidc_callback))
        .route("/api/auth/callback/", get(oidc_callback).post(oidc_callback))
        .route("/login/callback", get(login_callback_alias).post(login_callback_alias))
        .route("/api/me", get(me))
        .route("/api/catalog", get(catalog))
        .route("/api/dashboard", get(dashboard))
        .route("/api/documents/query", post(query_documents))
        .route("/api/documents/bulk", post(bulk))
        .route("/api/documents/purge-expired", post(purge_expired_for_user))
        .route("/api/cron/purge-trash", post(cron_purge_trash))
        .route("/api/documents/{id}", get(document))
        .route("/api/documents/{id}/preview", get(preview))
        .route("/api/documents/{id}/preview/meta", get(preview_meta))
        .route("/api/documents/{id}/file", get(file_original).head(file_original))
        .route("/api/documents/{id}/meta", post(meta))
        .route("/api/documents/{id}/ocr", post(ocr_rerun))
        .route("/api/documents/{id}/edit/stirling", post(stirling_start))
        .route(
            "/api/edit/stirling/{token}/file",
            get(stirling_session_file)
                .head(stirling_session_file)
                .options(stirling_session_file_options),
        )
        .route("/api/search", get(search))
        .route("/api/tags", get(list_tags_h).post(upsert_tag).put(upsert_tag))
        .route("/api/tags/{id}", delete(delete_tag))
        .route("/api/correspondents", get(list_correspondents_h).post(upsert_correspondent).put(upsert_correspondent))
        .route("/api/correspondents/{id}", delete(delete_correspondent))
        .route("/api/types", get(list_types_h).post(upsert_type).put(upsert_type))
        .route("/api/types/{id}", delete(delete_type))
        .route("/api/storage-paths", get(list_paths_h).post(upsert_path).put(upsert_path))
        .route("/api/storage-paths/{id}", delete(delete_path))
        .route("/api/fields", get(list_fields_h).post(upsert_field).put(upsert_field))
        .route("/api/fields/{id}", delete(delete_field))
        .route("/api/views", get(list_views_h).post(upsert_view).put(upsert_view))
        .route("/api/views/{id}", delete(delete_view))
        .route("/api/workflows", get(list_workflows_h).post(upsert_workflow).put(upsert_workflow))
        .route("/api/workflows/{id}", delete(delete_workflow))
        .route("/api/notes", post(add_note))
        .route("/api/mail/simulate", post(simulate_mail))
        .route("/api/jobs", get(jobs))
        .route("/api/settings", get(get_settings).post(save_settings).put(save_settings))
        .route("/api/settings/llm-keys", patch(patch_llm_key).post(patch_llm_key))
        .route("/api/settings/paperless-import", post(paperless_import_start))
        .route("/api/settings/paperless-import/{job_id}", get(paperless_import_status))
        .route("/api/settings/paperless-import/{job_id}/cancel", post(paperless_import_cancel))
        .route("/api/settings/api-tokens", get(list_api_tokens).post(create_api_token))
        .route("/api/settings/api-tokens/{id}", delete(revoke_api_token))
        .layer(DefaultBodyLimit::max(crate::limits::DEFAULT_BODY_LIMIT_BYTES))
        .layer(from_fn_with_state(state.clone(), auth_gate));

    let app = Router::new()
        .merge(api_routes)
        .merge(upload_routes)
        .layer(cors)
        .with_state(state);

    let ui_dir = resolve_ui_dir();
    let index = ui_dir.join("index.html");
    let app = if index.is_file() {
        tracing::info!("serving Paperrust UI from {}", ui_dir.display());
        // Real files under /assets (and any path with an extension) must never
        // SPA-fallback to index.html — that turns missing JS/CSS into 200 text/html.
        let assets = ServeDir::new(ui_dir.join("assets"))
            .append_index_html_on_directories(false);
        let static_files = ServeDir::new(&ui_dir).append_index_html_on_directories(true);
        app.nest_service("/assets", assets).fallback_service(
            Router::new()
                .fallback_service(static_files)
                .layer(from_fn(spa_fallback_html_routes_only))
                .layer(from_fn(no_store_html))
                .layer(from_fn(pwa_no_cache))
                .layer(from_fn(reject_api_health_fallback)),
        )
    } else {
        tracing::warn!(
            "PAPERRUST_UI_DIR has no index.html ({}); GET / will 404",
            ui_dir.display()
        );
        app
    };

    // security headers on API + SPA/assets (outer layer after nest/fallback).
    // refuse (400) a public Host from an untrusted peer; never remap it.
    let app = app.layer(from_fn(reject_untrusted_public_host));
    let app = app.layer(from_fn(security_headers));

    // PAPERRUST_BIND defaults to loopback. 0.0.0.0 needs PAPERRUST_BIND_ALLOW_UNSPECIFIED=1
    // (PAPERRUST_BIND_K8S=1 is the same gate). Any other explicit IPv4 is accepted.
    let addrs = bind_addrs()?;
    refuse_dev_flag_outside_loopback(&addrs)?;
    let mut set = tokio::task::JoinSet::new();
    for addr in addrs {
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| anyhow::anyhow!("failed to bind {addr}: {e}"))?;
        tracing::info!("paperrust-api listening on {addr}");
        let app = app.clone();
        set.spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
        });
    }
    while let Some(res) = set.join_next().await {
        res.map_err(|e| anyhow::anyhow!("server task: {e}"))??;
    }
    Ok(())
}

const BIND_DEFAULTS: &str = "127.0.0.1:8788";

fn bind_v4_allowed(v4: Ipv4Addr) -> bool {
    if v4.is_unspecified() {
        return instance::unspecified_bind_allowed();
    }
    true
}

fn canonicalize_bind_v4(ip: IpAddr) -> Option<Ipv4Addr> {
    match ip {
        IpAddr::V4(v4) => Some(v4),
        // Only IPv4-mapped IPv6 (::ffff:a.b.c.d). Pure IPv6 (fd7a::, ::1, …) → None.
        IpAddr::V6(v6) => v6.to_ipv4_mapped(),
    }
}

fn is_allowed_bind_ip(ip: IpAddr) -> bool {
    canonicalize_bind_v4(ip)
        .map(bind_v4_allowed)
        .unwrap_or(false)
}

fn parse_bind_list(raw: &str) -> Vec<SocketAddr> {
    let mut out = Vec::new();
    for part in raw.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let Ok(addr) = part.parse::<SocketAddr>() else {
            tracing::warn!("PAPERRUST_BIND: skip unparsable {part}");
            continue;
        };
        let Some(v4) = canonicalize_bind_v4(addr.ip()) else {
            tracing::warn!("PAPERRUST_BIND: refusing non-IPv4/mapped {addr}");
            continue;
        };
        if !bind_v4_allowed(v4) {
            tracing::warn!("PAPERRUST_BIND: refusing unspecified {addr} (set PAPERRUST_BIND_ALLOW_UNSPECIFIED=1)");
            continue;
        }
        let canon = SocketAddr::from((v4, addr.port()));
        if !out.contains(&canon) {
            out.push(canon);
        }
    }
    out
}


/// `PAPERRUST_DEV=1` must not arm on a non-loopback bind (or when
/// `PAPERRUST_ENV=production`). Widens CORS + accepts client apiKey.
fn refuse_dev_flag_outside_loopback(addrs: &[SocketAddr]) -> anyhow::Result<()> {
    if std::env::var("PAPERRUST_DEV").ok().as_deref() != Some("1") {
        return Ok(());
    }
    if std::env::var("PAPERRUST_ENV").ok().as_deref() == Some("production") {
        anyhow::bail!("PAPERRUST_DEV=1 refused when PAPERRUST_ENV=production");
    }
    for addr in addrs {
        if !addr.ip().is_loopback() {
            anyhow::bail!(
                "PAPERRUST_DEV=1 refused when binding non-loopback {addr} (loopback-only)"
            );
        }
    }
    tracing::warn!("PAPERRUST_DEV=1 active (loopback binds only)");
    Ok(())
}

fn bind_addrs() -> anyhow::Result<Vec<SocketAddr>> {
    match std::env::var("PAPERRUST_BIND") {
        Ok(raw) => {
            let out = parse_bind_list(&raw);
            if out.is_empty() {
                anyhow::bail!(
                    "PAPERRUST_BIND set but empty after allowlist filter; refusing DEFAULTS fallback (would reopen unintended binds)"
                );
            }
            Ok(out)
        }
        Err(_) => Ok(parse_bind_list(BIND_DEFAULTS)),
    }
}

#[cfg(test)]
mod bind_tests {
    use super::*;

    fn without_unspecified_gate<T>(f: impl FnOnce() -> T) -> T {
        let _guard = crate::instance::env_lock().lock().unwrap();
        let keys = ["PAPERRUST_BIND_ALLOW_UNSPECIFIED", "PAPERRUST_BIND_K8S"];
        let prev: Vec<Option<String>> = keys.iter().map(|k| std::env::var(k).ok()).collect();
        unsafe {
            for key in keys {
                std::env::remove_var(key);
            }
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
    fn allowlist_accepts_loopback_and_explicit_ipv4() {
        without_unspecified_gate(|| {
            assert!(is_allowed_bind_ip(IpAddr::V4(Ipv4Addr::LOCALHOST)));
            assert!(is_allowed_bind_ip(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10))));
            let addrs = parse_bind_list("192.0.2.10:8788");
            assert_eq!(addrs, vec![SocketAddr::from(([192, 0, 2, 10], 8788))]);
        });
    }

    #[test]
    fn unspecified_requires_gate_either_name() {
        without_unspecified_gate(|| {
            assert!(!is_allowed_bind_ip("0.0.0.0".parse().unwrap()));
            assert!(parse_bind_list("0.0.0.0:8788").is_empty());
        });
        for key in ["PAPERRUST_BIND_ALLOW_UNSPECIFIED", "PAPERRUST_BIND_K8S"] {
            let _guard = crate::instance::env_lock().lock().unwrap();
            let prev_allow = std::env::var("PAPERRUST_BIND_ALLOW_UNSPECIFIED").ok();
            let prev_k8s = std::env::var("PAPERRUST_BIND_K8S").ok();
            unsafe {
                std::env::remove_var("PAPERRUST_BIND_ALLOW_UNSPECIFIED");
                std::env::remove_var("PAPERRUST_BIND_K8S");
                std::env::set_var(key, "1");
            }
            assert!(is_allowed_bind_ip("0.0.0.0".parse().unwrap()), "{key}");
            assert_eq!(
                parse_bind_list("0.0.0.0:8788"),
                vec![SocketAddr::from(([0, 0, 0, 0], 8788))]
            );
            unsafe {
                match prev_allow {
                    Some(v) => std::env::set_var("PAPERRUST_BIND_ALLOW_UNSPECIFIED", v),
                    None => std::env::remove_var("PAPERRUST_BIND_ALLOW_UNSPECIFIED"),
                }
                match prev_k8s {
                    Some(v) => std::env::set_var("PAPERRUST_BIND_K8S", v),
                    None => std::env::remove_var("PAPERRUST_BIND_K8S"),
                }
            }
        }
    }

    #[test]
    fn rejects_pure_ipv6() {
        without_unspecified_gate(|| {
            assert!(!is_allowed_bind_ip("fd7a:115c:a1e0::1".parse().unwrap()));
            assert!(!is_allowed_bind_ip("::1".parse().unwrap()));
            assert!(parse_bind_list("fd7a::1:8788").is_empty());
            let mapped: IpAddr = "::ffff:192.0.2.10".parse().unwrap();
            assert_eq!(
                canonicalize_bind_v4(mapped),
                Some(Ipv4Addr::new(192, 0, 2, 10))
            );
            assert!(is_allowed_bind_ip(mapped));
        });
    }

    #[test]
    fn defaults_parse_to_loopback_only() {
        let addrs = parse_bind_list(BIND_DEFAULTS);
        assert_eq!(addrs, vec![SocketAddr::from(([127, 0, 0, 1], 8788))]);
    }
}

async fn instance_config(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Json<serde_json::Value> {
    let host = auth::request_host(&headers, Some(peer.ip()));
    let stirling = crate::stirling_edit::stirling_url_for_host(&host);
    Json(serde_json::json!({
        "stirlingUrl": if stirling.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::Value::String(stirling)
        },
    }))
}

async fn health(State(state): State<AppState>) -> Json<Health> {
    let ok = sqlx::query_scalar::<_, i32>("select 1")
        .fetch_optional(&state.pool)
        .await
        .ok()
        .flatten()
        .is_some();
    Json(Health {
        ok,
        service: "paperrust",
        postgres: if ok { "pgrust".into() } else { "not-connected".into() },
    })
}

async fn catalog(State(state): State<AppState>, user: AuthUser) -> Result<Json<Catalog>, AppError> {
    Ok(Json(queries::get_catalog(&state.pool, user.id).await?))
}

async fn dashboard(State(state): State<AppState>, user: AuthUser) -> Result<Json<DashboardStats>, AppError> {
    Ok(Json(queries::get_dashboard(&state.pool, user.id).await?))
}

async fn query_documents(
    State(state): State<AppState>,
    user: AuthUser,
    Json(filter): Json<DocumentFilter>,
) -> Result<Json<Vec<DocumentListItem>>, AppError> {
    Ok(Json(queries::list_documents(&state.pool, &filter, user.id).await?))
}

#[derive(serde::Deserialize)]
struct SearchQ {
    q: Option<String>,
}

async fn search(
    State(state): State<AppState>,
    user: AuthUser,
    Query(q): Query<SearchQ>,
) -> Result<Json<Vec<DocumentListItem>>, AppError> {
    let q = q.q.unwrap_or_default();
    if q.trim().is_empty() {
        return Ok(Json(vec![]));
    }
    let rows = queries::list_documents_limited(
        &state.pool,
        &DocumentFilter {
            q: Some(q),
            sort: Some("added_at".into()),
            ..Default::default()
        },
        user.id,
        Some(12),
    )
    .await?;
    Ok(Json(rows))
}

async fn document(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i32>,
) -> Result<Response, AppError> {
    match queries::get_document(&state.pool, id, user.id).await? {
        Some(doc) => Ok(Json(doc).into_response()),
        None => Ok(not_found_json()),
    }
}

fn not_found_json() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({"error": "not_found"})),
    )
        .into_response()
}

fn jpeg_response(bytes: Vec<u8>) -> Response {
    use axum::http::header;
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "image/jpeg"),
            (header::CACHE_CONTROL, "private, max-age=86400"),
        ],
        bytes,
    )
        .into_response()
}

#[derive(serde::Deserialize)]
struct PreviewQuery {
    page: Option<i32>,
}

async fn preview_meta(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i32>,
) -> Result<Response, AppError> {
    Ok(preview_meta_inner(&state, id, user.id).await?)
}

async fn preview(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i32>,
    Query(q): Query<PreviewQuery>,
) -> Result<Response, AppError> {
    if let Some(page) = q.page {
        Ok(preview_page_inner(&state, id, page, user.id).await?)
    } else {
        Ok(preview_meta_inner(&state, id, user.id).await?)
    }
}

async fn preview_meta_inner(state: &AppState, id: i32, owner_id: i32) -> Result<Response, AppError> {
    let Some(meta) = crate::preview::load_original_meta(&state.pool, id, owner_id).await? else {
        return Ok(not_found_json());
    };
    let (_id, _mime, _name, stored_pages, has_bytes, _byte_len) = meta;
    let mut page_count = crate::preview::cached_max_page(&state.pool, id).await?;
    if page_count <= 0 {
        page_count = stored_pages;
    }
    // Only recount from bytes when we have no trusted page_count yet.
    if page_count <= 0 && has_bytes {
        if let Some(doc) = crate::preview::load_original(&state.pool, id, owner_id).await? {
            if let Some(bytes) = doc.bytes {
                let mime = doc.mime_type;
                let counted = match crate::limits::run_heavy_blocking(
                    std::time::Duration::from_secs(crate::preview::RENDER_TIMEOUT_SECS),
                    move || crate::preview::count_pages(&bytes, &mime),
                )
                .await
                {
                    Ok(n) => Some(n),
                    Err(e) => {
                        tracing::warn!("preview count_pages id={id}: {e}");
                        None
                    }
                };
                if let Some(n) = counted {
                    if n > 0 {
                        page_count = n;
                        let _ = sqlx::query(
                            "update documents set page_count = $1 where id = $2 and owner_id = $3",
                        )
                        .bind(n)
                        .bind(id)
                        .bind(owner_id)
                        .execute(&state.pool)
                        .await;
                    }
                }

            }
        }
    }
    if page_count <= 0 {
        page_count = 1;
    }
    if page_count > crate::preview::MAX_PAGES {
        page_count = crate::preview::MAX_PAGES;
    }
    if !has_bytes && crate::preview::cached_max_page(&state.pool, id).await? <= 0 {
        return Ok(StatusCode::NO_CONTENT.into_response());
    }
    let pages: Vec<String> = (1..=page_count)
        .map(|p| format!("/api/documents/{id}/preview?page={p}"))
        .collect();
    let mut res = Json(serde_json::json!({"pageCount": page_count, "pages": pages})).into_response();
    res.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=60"),
    );
    Ok(res)
}

async fn preview_page_inner(state: &AppState, id: i32, page: i32, owner_id: i32) -> Result<Response, AppError> {
    if page < 1 || page > crate::preview::MAX_PAGES {
        return Ok(not_found_json());
    }
    let Some(doc) = crate::preview::load_original(&state.pool, id, owner_id).await? else {
        return Ok(not_found_json());
    };
    if let Some(jpeg) = crate::preview::cached_jpeg(&state.pool, id, page).await? {
        return Ok(jpeg_response(jpeg));
    }
    let Some(bytes) = doc.bytes else {
        return Ok(StatusCode::NO_CONTENT.into_response());
    };
    let mime = doc.mime_type;
    // Prefer stored/cached page_count; avoid a second full PDF open just to validate.
    let n = doc.page_count.max(1);
    if page > n.max(1) && page > crate::preview::cached_max_page(&state.pool, id).await?.max(1) {
        // Soft check only when stored count looks wrong — still render if page is within MAX.
        if page > crate::preview::MAX_PAGES {
            return Ok(not_found_json());
        }
    } else if page > n.max(1) {
        return Ok(not_found_json());
    }
    let jpeg = match crate::limits::run_heavy_blocking(
        std::time::Duration::from_secs(20.min(crate::preview::RENDER_TIMEOUT_SECS)),
        move || crate::preview::render_page(&bytes, &mime, page),
    )
    .await
    {
        Err(e) => {
            tracing::warn!("preview id={id} page={page}: {e}");
            return Ok(StatusCode::NO_CONTENT.into_response());
        }
        Ok(jpeg) => jpeg,
    };
    let _ = crate::preview::store_jpeg(&state.pool, id, page, &jpeg).await;
    Ok(jpeg_response(jpeg))
}

fn sanitize_filename(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ' ') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let s = s.trim().trim_matches('.');
    if s.is_empty() {
        "document".into()
    } else {
        s.chars().take(crate::limits::MAX_FILENAME_CHARS).collect()
    }
}

async fn file_original(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i32>,
    req: Request,
) -> Result<Response, AppError> {
    use axum::http::header::{self, HeaderValue};
    let Some(meta) = crate::preview::load_original_meta(&state.pool, id, user.id).await? else {
        return Ok(not_found_json());
    };
    let (_doc_id, mime_stored, original_filename, _pages, has_bytes, byte_length) = meta;
    if !has_bytes || byte_length <= 0 {
        return Ok(not_found_json());
    }
    let filename = sanitize_filename(
        original_filename
            .as_deref()
            .unwrap_or("document"),
    );
    let disp = format!("attachment; filename=\"{filename}\"");
    if *req.method() == Method::HEAD {
        let mut res = Response::new(axum::body::Body::empty());
        *res.status_mut() = StatusCode::OK;
        let h = res.headers_mut();
        if let Ok(v) = HeaderValue::from_str(&mime_stored) {
            h.insert(header::CONTENT_TYPE, v);
        }
        if let Ok(v) = HeaderValue::from_str(&byte_length.to_string()) {
            h.insert(header::CONTENT_LENGTH, v);
        }
        if let Ok(v) = HeaderValue::from_str(&disp) {
            h.insert(header::CONTENT_DISPOSITION, v);
        }
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("private"));
        return Ok(res);
    }
    let Some(doc) = crate::preview::load_original(&state.pool, id, user.id).await? else {
        return Ok(not_found_json());
    };
    let Some(bytes) = doc.bytes else {
        return Ok(not_found_json());
    };
    // Never serve the paper SVG placeholder as the original file.
    if crate::preview::looks_like_svg(&bytes) {
        return Ok(not_found_json());
    }
    // Serve original_bytes for PDF / PNG / JPEG with a real content-type.
    // Detect by magic so a PNG is never 404 just because mime was wrong or PDF-only.
    // Never fall back to the paper SVG / file_data placeholder.
    let mime = if crate::preview::looks_like_pdf(&bytes) {
        "application/pdf".to_string()
    } else if crate::preview::looks_like_png(&bytes) {
        "image/png".to_string()
    } else if crate::preview::looks_like_jpeg(&bytes) {
        "image/jpeg".to_string()
    } else {
        return Ok(not_found_json());
    };
    let mut res = (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, mime.as_str()),
            (header::CACHE_CONTROL, "private, max-age=3600"),
        ],
        bytes,
    )
        .into_response();
    if let Ok(v) = HeaderValue::from_str(&disp) {
        res.headers_mut().insert(header::CONTENT_DISPOSITION, v);
    }
    Ok(res)
}

/// production origins + explicit Vite ports only.
/// Arbitrary `localhost:<any>` is refused unless `PAPERRUST_DEV=1`.
/// Vite loopback origins are refused when `PAPERRUST_ENV=production`.
fn cors_origin_allowed(origin: &HeaderValue) -> bool {
    let Ok(s) = origin.to_str() else {
        return false;
    };
    // Stirling origins are not global (develop/#56). CORS for the edit file is
    // applied only on GET /api/edit/stirling/{token}/file, without credentials.
    // App origins come from PAPERRUST_CORS_ORIGINS (!53), no hostnames in code.
    if instance::cors_origin_listed(s) {
        return true;
    }
    const VITE_ORIGINS: &[&str] = &[
        "http://localhost:8080",
        "http://localhost:8081",
        "http://localhost:5173",
        "http://127.0.0.1:8080",
        "http://127.0.0.1:8081",
        "http://127.0.0.1:5173",
    ];
    if VITE_ORIGINS.contains(&s) {
        return std::env::var("PAPERRUST_ENV").ok().as_deref() != Some("production");
    }
    // Optional escape hatch: any loopback origin when explicitly developing.
    if std::env::var("PAPERRUST_DEV").ok().as_deref() == Some("1") {
        for prefix in ["http://localhost", "http://127.0.0.1"] {
            if let Some(rest) = s.strip_prefix(prefix) {
                return rest.is_empty() || rest.starts_with(':');
            }
        }
    }
    false
}


async fn paperless_import_start(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<crate::paperless_import::StartImportRequest>,
) -> Response {
    // per-user start rate-limit (in addition to 409 one-active-job).
    if !rate_limit_ok(&format!("plimport:{}", user.id), 5, 60) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({ "error": "rate_limited" })),
        )
            .into_response();
    }
    match state.paperless_imports.start(state.pool.clone(), user.id, body) {
        Ok(resp) => (StatusCode::ACCEPTED, Json(resp)).into_response(),
        Err((code, msg)) => {
            // Front toasts `error` literally — never echo PL tokens.
            let safe = crate::paperless_import::sanitize_for_client(&msg);
            (code, Json(serde_json::json!({ "error": safe }))).into_response()
        }
    }
}

async fn paperless_import_status(
    State(state): State<AppState>,
    user: AuthUser,
    Path(job_id): Path<String>,
) -> Response {
    match state.paperless_imports.status_for(user.id, &job_id) {
        Some(status) => Json(status).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "job_not_found" })),
        )
            .into_response(),
    }
}

async fn paperless_import_cancel(
    State(state): State<AppState>,
    user: AuthUser,
    Path(job_id): Path<String>,
) -> Response {
    let _ = state.paperless_imports.cancel(user.id, &job_id);
    (StatusCode::OK, Json(serde_json::json!({ "ok": true }))).into_response()
}

#[cfg(test)]
mod cors_origin_tests {
    #[test]
    fn host_implies_https_defaults_outside_loopback() {
        assert!(super::host_implies_https("archive.example"));
        assert!(super::host_implies_https("archive.lan"));
        assert!(super::host_implies_https("x.lan.loc"));
        assert!(!super::host_implies_https("localhost"));
        assert!(!super::host_implies_https("127.0.0.1"));
        assert!(!super::host_implies_https("::1"));
    }

    #[test]
    fn csp_is_strict_without_external_origins() {
        let csp = super::content_security_policy("abc123");
        let directives: Vec<&str> = csp.split(';').map(str::trim).collect();
        assert!(directives.contains(&"default-src 'self'"));
        assert!(directives.contains(&"frame-ancestors 'none'"));
        assert!(directives.contains(&"font-src 'self'"));
        assert!(directives.contains(&"connect-src 'self'"));
        assert!(directives.contains(&"script-src 'self' 'nonce-abc123'"));
        assert!(directives.contains(&"object-src 'none'"));
        // No third-party origin anywhere (no external script host, no Google Fonts, no CDN).
        assert!(!csp.contains("http:") && !csp.contains("https:"), "{csp}");
        assert!(!csp.contains("googleapis") && !csp.contains("gstatic"), "{csp}");
        assert!(!csp.contains("unsafe-eval"), "{csp}");
    }

    #[test]
    fn csp_nonce_is_at_least_128_bits_and_fresh() {
        use base64::Engine;
        let a = super::csp_nonce();
        let b = super::csp_nonce();
        let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(&a).unwrap();
        assert!(raw.len() * 8 >= 128, "{} bits", raw.len() * 8);
        assert_ne!(a, b);
    }

    #[test]
    fn csp_script_src_has_no_unsafe_keywords() {
        let csp = super::content_security_policy(&super::csp_nonce());
        let script = csp
            .split(';')
            .map(str::trim)
            .find(|d| d.starts_with("script-src "))
            .expect("script-src");
        assert!(!script.contains("unsafe-inline"), "{script}");
        assert!(!script.contains("unsafe-eval"), "{script}");
        assert!(!csp.contains("unsafe-eval"), "{csp}");
    }

    /// Every response (index, SPA fallback, 404/500 HTML error pages,
    /// API JSON) carries the CSP; nonces differ per request and match the
    /// `nonce=` injected into the HTML.
    #[tokio::test]
    async fn csp_on_all_html_responses_with_per_request_nonce() {
        use axum::{http::StatusCode, middleware::from_fn, response::Html, routing::get, Json, Router};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let dir = std::env::temp_dir().join(format!("pr-csp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("index.html"),
            "<html><head><script type=\"module\" src=\"/assets/a.js\"></script></head></html>",
        )
        .unwrap();
        let spa = Router::new()
            .fallback_service(
                tower_http::services::ServeDir::new(&dir)
                    .append_index_html_on_directories(true)
                    .fallback(get(|| async { Html("<html><script src=\"/assets/a.js\"></script></html>") })),
            );
        let app = Router::new()
            .route(
                "/boom",
                get(|| async {
                    (StatusCode::INTERNAL_SERVER_ERROR, Html("<html><script>x()</script>error</html>"))
                }),
            )
            .route(
                "/missing",
                get(|| async { (StatusCode::NOT_FOUND, Html("<html>not found</html>")) }),
            )
            .route("/api/x", get(|| async { Json(serde_json::json!({"ok": true})) }))
            .fallback_service(spa)
            .layer(from_fn(super::security_headers));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        async fn fetch(addr: std::net::SocketAddr, path: &str) -> (String, String) {
            let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
            let req = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
            s.write_all(req.as_bytes()).await.unwrap();
            let mut buf = Vec::new();
            s.read_to_end(&mut buf).await.unwrap();
            let txt = String::from_utf8_lossy(&buf).to_string();
            let (head, body) = txt.split_once("\r\n\r\n").unwrap();
            (head.to_ascii_lowercase(), body.to_string())
        }
        fn nonce_of(head: &str) -> String {
            head.lines()
                .find_map(|l| l.strip_prefix("x-paperrust-csp-nonce: "))
                .expect("nonce header")
                .trim()
                .to_string()
        }
        let mut nonces = Vec::new();
        for path in ["/", "/", "/some/spa/route", "/boom", "/missing", "/api/x"] {
            let (head, body) = fetch(addr, path).await;
            let csp = head
                .lines()
                .find_map(|l| l.strip_prefix("content-security-policy: "))
                .unwrap_or_else(|| panic!("no CSP on {path}: {head}"));
            let script = csp.split(';').map(str::trim).find(|d| d.starts_with("script-src")).unwrap();
            assert!(!script.contains("unsafe-inline") && !script.contains("unsafe-eval"), "{path}: {script}");
            let n = nonce_of(&head);
            // Header is lowercased above; the nonce in the CSP must match the body's.
            assert!(script.contains(&format!("'nonce-{n}'")), "{path}");
            if body.contains("<script") {
                assert!(body.to_ascii_lowercase().contains(&format!("nonce=\"{n}\"")), "{path}: {body}");
            }
            nonces.push(n);
        }
        let uniq: std::collections::HashSet<_> = nonces.iter().collect();
        assert_eq!(uniq.len(), nonces.len(), "nonce reused: {nonces:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pwa_no_cache_exact_paths_only() {
        assert!(super::pwa_no_cache_path("/sw.js"));
        assert!(super::pwa_no_cache_path("/manifest.webmanifest"));
        assert!(!super::pwa_no_cache_path("/sw.js/x"));
        assert!(!super::pwa_no_cache_path("/assets/sw.js"));
        assert!(!super::pwa_no_cache_path("/api/sw.js"));
    }

    use super::cors_origin_allowed;
    use axum::http::HeaderValue;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn hv(s: &str) -> HeaderValue {
        HeaderValue::from_str(s).expect("origin header")
    }

    fn with_paperrust_env<R>(value: Option<&str>, f: impl FnOnce() -> R) -> R {
        let _guard = env_lock().lock().unwrap();
        let prev = std::env::var("PAPERRUST_ENV").ok();
        // SAFETY: serialized via env_lock; restored before unlock.
        unsafe {
            match value {
                Some(v) => std::env::set_var("PAPERRUST_ENV", v),
                None => std::env::remove_var("PAPERRUST_ENV"),
            }
        }
        let out = f();
        unsafe {
            match prev {
                Some(v) => std::env::set_var("PAPERRUST_ENV", v),
                None => std::env::remove_var("PAPERRUST_ENV"),
            }
        }
        out
    }

    #[test]
    fn production_rejects_vite_allows_configured_origins() {
        let _guard = crate::instance::env_lock().lock().unwrap();
        let prev = std::env::var("PAPERRUST_CORS_ORIGINS").ok();
        unsafe {
            std::env::set_var(
                "PAPERRUST_CORS_ORIGINS",
                "https://archive.example,https://archive.lan.example",
            );
        }
        with_paperrust_env(Some("production"), || {
            assert!(!cors_origin_allowed(&hv("http://localhost:8080")));
            assert!(!cors_origin_allowed(&hv("http://127.0.0.1:5173")));
            assert!(cors_origin_allowed(&hv("https://archive.lan.example")));
            assert!(cors_origin_allowed(&hv("https://archive.example")));
            assert!(!cors_origin_allowed(&hv("https://evil.example")));
            assert!(!cors_origin_allowed(&hv("https://stirling.lan")));
            assert!(!cors_origin_allowed(&hv("https://stirling.lan.loc")));
            assert!(!cors_origin_allowed(&hv("https://pdf.example.com")));
        });
        with_paperrust_env(None, || {
            // Stirling origins are never global CORS, in any env.
            assert!(!cors_origin_allowed(&hv("https://stirling.lan")));
            assert!(!cors_origin_allowed(&hv("https://pdf.example.com")));
            assert!(!cors_origin_allowed(&hv("https://stirling.lan.loc")));
            assert!(cors_origin_allowed(&hv("https://archive.example")));
        });
        unsafe {
            match prev {
                Some(v) => std::env::set_var("PAPERRUST_CORS_ORIGINS", v),
                None => std::env::remove_var("PAPERRUST_CORS_ORIGINS"),
            }
        }
    }

    #[test]
    fn non_production_allows_vite_localhost() {
        with_paperrust_env(None, || {
            assert!(cors_origin_allowed(&hv("http://localhost:8080")));
        });
        with_paperrust_env(Some("development"), || {
            assert!(cors_origin_allowed(&hv("http://localhost:8080")));
        });
    }
}

#[cfg(test)]
mod bulk_token_tests {
    use super::bulk_token_delete_only;
    use crate::models::BulkUpdate;

    fn bulk(delete: Option<bool>) -> BulkUpdate {
        BulkUpdate {
            ids: vec![1],
            add_tag_ids: None,
            remove_tag_ids: None,
            correspondent_id: None,
            document_type_id: None,
            delete_selected: delete,
            restore_selected: None,
            purge_selected: None,
        }
    }

    #[test]
    fn ingest_token_may_only_soft_delete() {
        assert!(bulk_token_delete_only(&bulk(Some(true))));
        assert!(!bulk_token_delete_only(&bulk(None)));
        assert!(!bulk_token_delete_only(&bulk(Some(false))));
        let mut tagged = bulk(Some(true));
        tagged.add_tag_ids = Some(vec![4]);
        assert!(!bulk_token_delete_only(&tagged));
        let mut purge = bulk(Some(true));
        purge.purge_selected = Some(true);
        assert!(!bulk_token_delete_only(&purge));
        let mut restore = bulk(Some(true));
        restore.restore_selected = Some(true);
        assert!(!bulk_token_delete_only(&restore));
    }
}

fn resolve_ui_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("PAPERRUST_UI_DIR") {
        return PathBuf::from(dir);
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            candidates.push(exe_dir.join("ui"));
            candidates.push(exe_dir.join("dist"));
            // backend/target/release -> repo root dist / dist/client / ui
            // release -> target -> backend -> repo root
            if let Some(root) = exe_dir
                .parent()
                .and_then(|p| p.parent())
                .and_then(|p| p.parent())
            {
                candidates.push(root.join("dist"));
                candidates.push(root.join("dist/client"));
                candidates.push(root.join("ui"));
            }
        }
    }
    candidates.push(PathBuf::from("dist"));
    candidates.push(PathBuf::from("dist/client"));
    candidates.push(PathBuf::from("ui"));
    for cand in candidates {
        if cand.join("index.html").is_file() {
            return cand;
        }
    }
    PathBuf::from("dist")
}


/// fresh 256-bit nonce from the OS CSPRNG on every response; never
/// cached or reused (called inside `security_headers`, once per request).
fn csp_nonce() -> String {
    use base64::Engine;
    use rand::TryRng;
    let mut buf = [0u8; 32];
    rand::rngs::SysRng
        .try_fill_bytes(&mut buf)
        .expect("os rng");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf)
}

/// Inject `nonce="…"` on every `<script` opening tag that lacks one.
fn inject_script_nonces(html: &str, nonce: &str) -> String {
    let mut out = String::with_capacity(html.len() + 128);
    let lower = html.to_ascii_lowercase();
    let mut pos = 0;
    while let Some(rel) = lower[pos..].find("<script") {
        let start = pos + rel;
        out.push_str(&html[pos..start]);
        let Some(rel_gt) = html[start..].find('>') else {
            out.push_str(&html[start..]);
            return out;
        };
        let end = start + rel_gt + 1;
        let tag = &html[start..end];
        if tag.to_ascii_lowercase().contains("nonce=") {
            out.push_str(tag);
        } else {
            out.push_str(&tag[..tag.len() - 1]);
            out.push_str(&format!(" nonce=\"{nonce}\""));
            out.push('>');
        }
        pos = end;
    }
    out.push_str(&html[pos..]);
    out
}

/// HTTPS for HSTS and the `Secure` cookie flag.
/// An origin listed in `PAPERRUST_PUBLIC_URLS` follows that URL's scheme.
/// Any other non-loopback host defaults to HTTPS. Loopback stays cleartext.
fn host_implies_https(name: &str) -> bool {
    instance::host_implies_https(name)
}

/// strict CSP, no third-party origin at all. Fonts are self-hosted
/// (`/fonts`, #58), no external script (E1). OIDC login/logout and the Stirling
/// editor are top-level navigations / popups, not fetches, so `connect-src`
/// stays `'self'`.
fn content_security_policy(nonce: &str) -> String {
    format!(
        "default-src 'self'; base-uri 'self'; object-src 'none'; frame-ancestors 'none'; img-src 'self' data: blob:; font-src 'self'; style-src 'self' 'unsafe-inline'; script-src 'self' 'nonce-{nonce}'; connect-src 'self'; worker-src 'self'; manifest-src 'self'"
    )
}

async fn security_headers(req: Request, next: Next) -> Response {
    // Baseline browser isolation + per-response CSP nonce.
    // HSTS: never trust X-Forwarded-Proto unless TCP peer is trusted (same as XFH).
    let peer = connect_peer(&req);
    let host = auth::request_host(req.headers(), peer);
    let xfp_https = auth::peer_trusts_xfh(peer)
        && req
            .headers()
            .get("x-forwarded-proto")
            .and_then(|v| v.to_str().ok())
            .map(|v| {
                v.split(',')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .eq_ignore_ascii_case("https")
            })
            .unwrap_or(false);
    // HTTPS when the matched public URL is https, or for any non-loopback host
    // when no listed origin matches (instance).
    let name = auth::host_name(&host);
    let host_implies_https = host_implies_https(&name) || instance::host_implies_https(&name);
    let https = req.uri().scheme_str() == Some("https") || host_implies_https || xfp_https;
    let nonce = csp_nonce();
    let csp = content_security_policy(&nonce);
    let mut res = next.run(req).await;
    let is_html = res
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.starts_with("text/html"));
    if is_html {
        let (mut parts, body) = res.into_parts();
        let bytes = match axum::body::to_bytes(body, 8 * 1024 * 1024).await {
            Ok(b) => b,
            Err(_) => {
                parts.status = StatusCode::INTERNAL_SERVER_ERROR;
                return Response::from_parts(parts, axum::body::Body::empty());
            }
        };
        let html = String::from_utf8_lossy(&bytes);
        let injected = inject_script_nonces(&html, &nonce);
        let body_bytes = injected.into_bytes();
        parts.headers.insert(
            axum::http::header::CONTENT_LENGTH,
            HeaderValue::from(body_bytes.len()),
        );
        res = Response::from_parts(parts, axum::body::Body::from(body_bytes));
    }
    let h = res.headers_mut();
    h.insert("x-frame-options", HeaderValue::from_static("DENY"));
    h.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    h.insert(
        "referrer-policy",
        HeaderValue::from_static("strict-origin-when-cross-origin"),
    );
    if let Ok(v) = HeaderValue::from_str(&csp) {
        h.insert("content-security-policy", v);
    }
    if let Ok(v) = HeaderValue::from_str(&nonce) {
        h.insert("x-paperrust-csp-nonce", v);
    }
    if https {
        h.insert(
            "strict-transport-security",
            HeaderValue::from_static("max-age=31536000; includeSubDomains"),
        );
    }
    res
}

async fn reject_api_health_fallback(req: Request, next: Next) -> Response {
    let path = req.uri().path();
    if path == "/health" || path.starts_with("/api") {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "not_found"})),
        )
            .into_response();
    }
    next.run(req).await
}

fn is_html_spa_route(path: &str) -> bool {
    if path == "/assets" || path.starts_with("/assets/") {
        return false;
    }
    let last = path.rsplit('/').next().unwrap_or(path);
    !last.contains('.')
}

async fn spa_fallback_html_routes_only(req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    let res = next.run(req).await;
    if res.status() != StatusCode::NOT_FOUND {
        return res;
    }
    if !is_html_spa_route(&path) {
        return res;
    }
    let index = resolve_ui_dir().join("index.html");
    match tokio::fs::read(&index).await {
        Ok(bytes) => (
            StatusCode::OK,
            [
                (CONTENT_TYPE, "text/html; charset=utf-8"),
                (
                    axum::http::header::CACHE_CONTROL,
                    "no-store, no-cache, must-revalidate",
                ),
            ],
            bytes,
        )
            .into_response(),
        Err(_) => res,
    }
}

/// PWA (GH#2): the service worker script and the manifest must
/// always be revalidated so a fixed/removed SW is picked up immediately.
/// Exact paths only.
fn pwa_no_cache_path(path: &str) -> bool {
    matches!(path, "/sw.js" | "/manifest.webmanifest")
}

async fn pwa_no_cache(req: Request, next: Next) -> Response {
    let hit = pwa_no_cache_path(req.uri().path());
    let mut res = next.run(req).await;
    if hit && res.status().is_success() {
        res.headers_mut().insert(
            axum::http::header::CACHE_CONTROL,
            HeaderValue::from_static("no-cache"),
        );
    }
    res
}

/// HTML entrypoints must never be heuristically cached — a stale index.html
/// that points at deleted Vite chunks produces a permanent blank page.
async fn no_store_html(req: Request, next: Next) -> Response {
    let mut res = next.run(req).await;
    let is_html = res
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|ct| ct.starts_with("text/html"))
        .unwrap_or(false);
    if is_html {
        res.headers_mut().insert(
            axum::http::header::CACHE_CONTROL,
            HeaderValue::from_static("no-store, no-cache, must-revalidate"),
        );
        res.headers_mut().insert(
            axum::http::header::PRAGMA,
            HeaderValue::from_static("no-cache"),
        );
    }
    res
}

async fn consume(State(state): State<AppState>, user: AuthUser, req: Request) -> Result<Json<ConsumeResult>, AppError> {
    if !rate_limit_ok(&format!("consume:{}", user.id), 30, 60) {
        return Err(anyhow::anyhow!("rate limited").into());
    }
    let input = consume_input_from_request(&state, req).await?;
    Ok(Json(consume::consume_document(&state.pool, input, user.id).await?))
}

async fn consume_input_from_request(state: &AppState, req: Request) -> Result<ConsumeInput, AppError> {
    let ct = req
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ct.starts_with("multipart/form-data") {
        parse_consume_multipart(state, req).await
    } else {
        let Json(mut input) = Json::<ConsumeInput>::from_request(req, state)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        // JSON consume must not override the OCR endpoint or inject keys.
        input.base_url = None;
        input.api_key = None;
        Ok(input)
    }
}

async fn parse_consume_multipart(state: &AppState, req: Request) -> Result<ConsumeInput, AppError> {
    let mut multipart = Multipart::from_request(req, state)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut filename = String::new();
    let mut mime_type = String::new();
    let mut data_base64 = String::new();
    let mut raw_bytes: Option<Vec<u8>> = None;
    let mut provider = "xai".to_string();
    let mut api_key: Option<String> = None;
    let mut skip_ocr: Option<bool> = None;
    let base_url: Option<String> = None;
    let mut title_override: Option<String> = None;
    let mut created_override: Option<String> = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?
    {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "file" | "document" | "upload" => {
                if filename.is_empty() {
                    filename = field.file_name().unwrap_or("upload.bin").to_string();
                }
                if mime_type.is_empty() {
                    mime_type = field
                        .content_type()
                        .unwrap_or("application/octet-stream")
                        .to_string();
                }
                let bytes = field.bytes().await.map_err(|e| anyhow::anyhow!("{e}"))?;
                if mime_type.is_empty()
                    || mime_type.eq_ignore_ascii_case("application/octet-stream")
                    || mime_type.eq_ignore_ascii_case("binary/octet-stream")
                    || mime_type.to_ascii_lowercase().contains("svg")
                {
                    if bytes.starts_with(b"%PDF") {
                        mime_type = "application/pdf".to_string();
                    } else if crate::preview::looks_like_png(&bytes) {
                        mime_type = "image/png".to_string();
                    } else if crate::preview::looks_like_jpeg(&bytes) {
                        mime_type = "image/jpeg".to_string();
                    }
                }
                raw_bytes = Some(bytes.to_vec());
            }
            "filename" => filename = field.text().await.map_err(|e| anyhow::anyhow!("{e}"))?,
            "mimeType" | "mime_type" => {
                mime_type = field.text().await.map_err(|e| anyhow::anyhow!("{e}"))?
            }
            "dataBase64" | "data_base64" => {
                data_base64 = field.text().await.map_err(|e| anyhow::anyhow!("{e}"))?
            }
            "provider" => provider = field.text().await.map_err(|e| anyhow::anyhow!("{e}"))?,
            "apiKey" | "api_key" => {
                let t = field.text().await.map_err(|e| anyhow::anyhow!("{e}"))?;
                let _ = t.len(); // never log the key
                if std::env::var("PAPERRUST_DEV").ok().as_deref() == Some("1") && !t.is_empty() {
                    api_key = Some(t);
                }
            }
            "skipOcr" | "skip_ocr" => {
                let t = field.text().await.map_err(|e| anyhow::anyhow!("{e}"))?;
                skip_ocr = Some(t == "1" || t.eq_ignore_ascii_case("true"));
            }
            // ignore consume `baseUrl` — selfhosted OCR endpoint comes from
            // settings (`ocrBaseUrl`) only. Still drain the field so multipart parses.
            "baseUrl" | "base_url" => {
                let _ = field.text().await.map_err(|e| anyhow::anyhow!("{e}"))?;
            }
            "title" => {
                let t = field.text().await.map_err(|e| anyhow::anyhow!("{e}"))?;
                let trimmed = t.trim();
                if !trimmed.is_empty() {
                    title_override = Some(crate::limits::clamp_title(trimmed));
                }
            }
            "created" => {
                let t = field.text().await.map_err(|e| anyhow::anyhow!("{e}"))?;
                let trimmed = t.trim();
                if !trimmed.is_empty() {
                    // Accept "2016-04-19" or longer datetime — keep date prefix.
                    let date = trimmed.get(..10).unwrap_or(trimmed).to_string();
                    created_override = Some(date);
                }
            }
            _ => {}
        }
    }
    if filename.is_empty() {
        filename = "upload.bin".into();
    }
    filename = crate::limits::clamp_filename(&filename);
    if mime_type.is_empty() {
        mime_type = "application/octet-stream".into();
    }
    if raw_bytes.is_none() && data_base64.is_empty() {
        return Err(anyhow::anyhow!("Fichier manquant (champ file).").into());
    }
    Ok(ConsumeInput {
        filename,
        mime_type,
        data_base64,
        raw_bytes,
        provider,
        api_key,
        skip_ocr,
        base_url,
        title_override,
        created_override,
    })
}

async fn meta(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i32>,
    Json(mut data): Json<MetaUpdate>,
) -> Result<Response, AppError> {
    if !auth::document_owned(&state.pool, id, user.id).await? {
        return Ok(not_found_json());
    }
    data.id = id;
    // FK targets must belong to the same owner (null = clear is allowed).
    if let Some(Some(cid)) = data.correspondent_id {
        if !auth::correspondent_owned(&state.pool, cid, user.id).await? {
            return Ok(not_found_json());
        }
    }
    if let Some(Some(tid)) = data.document_type_id {
        if !auth::type_owned(&state.pool, tid, user.id).await? {
            return Ok(not_found_json());
        }
    }
    if let Some(Some(pid)) = data.storage_path_id {
        if !auth::path_owned(&state.pool, pid, user.id).await? {
            return Ok(not_found_json());
        }
    }
    let corr_set = data.correspondent_id.is_some();
    let type_set = data.document_type_id.is_some();
    let path_set = data.storage_path_id.is_some();
    let date_set = data.created_date.is_some();
    let asn_set = data.asn.is_some();
    if let Some(ref mut title) = data.title {
        *title = crate::limits::clamp_title(title);
    }
    sqlx::query(
        r#"update documents set
        title = coalesce($1, title),
        content = coalesce($2, content),
        correspondent_id = case when $9 then $3 else correspondent_id end,
        document_type_id = case when $10 then $4 else document_type_id end,
        storage_path_id = case when $11 then $5 else storage_path_id end,
        created_date = case when $12 then $6::date else created_date end,
        asn = case when $13 then $7 else asn end,
        modified_at = now()
       where id = $8 and owner_id = $14"#,
    )
    .bind(&data.title)
    .bind(&data.content)
    .bind(data.correspondent_id.clone().flatten())
    .bind(data.document_type_id.flatten())
    .bind(data.storage_path_id.flatten())
    .bind(data.created_date.clone().flatten())
    .bind(data.asn.flatten())
    .bind(id)
    .bind(corr_set)
    .bind(type_set)
    .bind(path_set)
    .bind(date_set)
    .bind(asn_set)
    .bind(user.id)
    .execute(&state.pool)
    .await?;
    if let Some(tag_ids) = &data.tag_ids {
        sqlx::query("delete from document_tags where document_id = $1")
            .bind(id)
            .execute(&state.pool)
            .await?;
        for tag_id in tag_ids {
            sqlx::query(
                r#"insert into document_tags (document_id, tag_id)
                   select $1, t.id from tags t where t.id = $2 and t.owner_id = $3
                   on conflict do nothing"#,
            )
            .bind(id)
            .bind(*tag_id)
            .bind(user.id)
            .execute(&state.pool)
            .await?;
        }
    }
    if let Some(cvs) = &data.custom_values {
        for cv in cvs {
            if !auth::field_owned(&state.pool, cv.field_id, user.id).await? {
                return Ok(not_found_json());
            }
            if cv.value.as_deref().unwrap_or("").is_empty() {
                sqlx::query("delete from document_custom_values where document_id = $1 and field_id = $2")
                    .bind(id)
                    .bind(cv.field_id)
                    .execute(&state.pool)
                    .await?;
            } else {
                sqlx::query(
                    r#"insert into document_custom_values (document_id, field_id, value)
                       select $1, f.id, $3 from custom_fields f
                       where f.id = $2 and f.owner_id = $4
                     on conflict (document_id, field_id) do update set value = excluded.value"#,
                )
                .bind(id)
                .bind(cv.field_id)
                .bind(&cv.value)
                .bind(user.id)
                .execute(&state.pool)
                .await?;
            }
        }
    }
    queries::log_audit(&state.pool, id, "updated", Some("Métadonnées")).await?;
    Ok(Json(queries::get_document(&state.pool, id, user.id).await?).into_response())
}

/// Ingest tokens may soft-delete the documents they just created. They cannot
/// retag, restore, or purge through the same route.
fn bulk_token_delete_only(data: &BulkUpdate) -> bool {
    let empty = |ids: &Option<Vec<i32>>| ids.as_ref().map(|v| v.is_empty()).unwrap_or(true);
    data.delete_selected == Some(true)
        && data.restore_selected != Some(true)
        && data.purge_selected != Some(true)
        && data.correspondent_id.is_none()
        && data.document_type_id.is_none()
        && empty(&data.add_tag_ids)
        && empty(&data.remove_tag_ids)
}

async fn bulk(
    State(state): State<AppState>,
    user: AuthUser,
    token_scope: Option<Extension<auth::IngestTokenScope>>,
    Json(data): Json<BulkUpdate>,
) -> Result<Response, AppError> {
    if token_scope.is_some() && !bulk_token_delete_only(&data) {
        return Ok((
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": "token_delete_only" })),
        )
            .into_response());
    }
    if data.ids.is_empty() {
        return Ok(Json(serde_json::json!({"ok": true, "count": 0})).into_response());
    }
    if data.ids.len() > 50 {
        return Err(anyhow::anyhow!("Trop de documents sélectionnés.").into());
    }
    for id in &data.ids {
        if !auth::document_owned(&state.pool, *id, user.id).await? {
            return Ok(not_found_json());
        }
    }
    if data.delete_selected == Some(true) {
        // Soft-delete → Corbeille (keep original_bytes until hard purge).
        let res = sqlx::query(
            "update documents set deleted_at = now(), modified_at = now() where id = any($1) and owner_id = $2 and deleted_at is null",
        )
        .bind(&data.ids)
        .bind(user.id)
        .execute(&state.pool)
        .await?;
        return Ok(Json(serde_json::json!({"ok": true, "count": res.rows_affected()})).into_response());
    }
    if data.restore_selected == Some(true) {
        let res = sqlx::query(
            "update documents set deleted_at = null, modified_at = now() where id = any($1) and owner_id = $2 and deleted_at is not null",
        )
        .bind(&data.ids)
        .bind(user.id)
        .execute(&state.pool)
        .await?;
        return Ok(Json(serde_json::json!({"ok": true, "count": res.rows_affected()})).into_response());
    }
    if data.purge_selected == Some(true) {
        let res = sqlx::query("delete from documents where id = any($1) and owner_id = $2 and deleted_at is not null")
            .bind(&data.ids)
            .bind(user.id)
            .execute(&state.pool)
            .await?;
        return Ok(Json(serde_json::json!({"ok": true, "count": res.rows_affected()})).into_response());
    }
    if let Some(cid) = data.correspondent_id {
        if !auth::correspondent_owned(&state.pool, cid, user.id).await? {
            return Ok(not_found_json());
        }
    }
    if let Some(tid) = data.document_type_id {
        if !auth::type_owned(&state.pool, tid, user.id).await? {
            return Ok(not_found_json());
        }
    }
    for id in &data.ids {
        if let Some(cid) = data.correspondent_id {
            sqlx::query(
                r#"update documents set correspondent_id = $1, modified_at = now()
                   where id = $2 and owner_id = $3
                     and exists (select 1 from correspondents c where c.id = $1 and c.owner_id = $3)"#,
            )
                .bind(cid)
                .bind(*id)
                .bind(user.id)
                .execute(&state.pool)
                .await?;
        }
        if let Some(tid) = data.document_type_id {
            sqlx::query(
                r#"update documents set document_type_id = $1, modified_at = now()
                   where id = $2 and owner_id = $3
                     and exists (select 1 from document_types dt where dt.id = $1 and dt.owner_id = $3)"#,
            )
                .bind(tid)
                .bind(*id)
                .bind(user.id)
                .execute(&state.pool)
                .await?;
        }
        for tag_id in data.add_tag_ids.iter().flatten() {
            sqlx::query(
                r#"insert into document_tags (document_id, tag_id)
                   select $1, t.id from tags t where t.id = $2 and t.owner_id = $3
                   on conflict do nothing"#,
            )
            .bind(*id)
            .bind(*tag_id)
            .bind(user.id)
            .execute(&state.pool)
            .await?;
        }
        for tag_id in data.remove_tag_ids.iter().flatten() {
            sqlx::query("delete from document_tags where document_id = $1 and tag_id = $2")
                .bind(*id)
                .bind(*tag_id)
                .execute(&state.pool)
                .await?;
        }
        queries::log_audit(&state.pool, *id, "bulk", Some("Modification groupée")).await?;
    }
    Ok(Json(serde_json::json!({"ok": true, "count": data.ids.len()})).into_response())
}

async fn ocr_rerun(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i32>,
    Json(mut opts): Json<OcrInput>,
) -> Result<Response, AppError> {
    if !rate_limit_ok(&format!("ocr:{}", user.id), 20, 60) {
        return Err(anyhow::anyhow!("rate limited").into());
    }
    if !auth::document_owned(&state.pool, id, user.id).await? {
        return Ok(not_found_json());
    }
    opts.id = id;
    if std::env::var("PAPERRUST_DEV").ok().as_deref() != Some("1") {
        opts.api_key = None;
    }
    Ok(Json(consume::re_ocr_document(&state.pool, opts, user.id).await?).into_response())
}

async fn stirling_start(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i32>,
    req: Request,
) -> Response {
    if !rate_limit_ok(&format!("stirling-start:{}", user.id), 10, 60) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({ "error": "rate_limited" })),
        )
            .into_response();
    }
    let host = auth::request_host(req.headers(), connect_peer(&req));
    match stirling_edit::start_session(&state.pool, user.id, id, &host).await {
        Ok(resp) => Json(resp).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn stirling_session_file(
    State(state): State<AppState>,
    Path(token): Path<String>,
    req: Request,
) -> Response {
    if !rate_limit_ok(&format!("stirling-file:{token}"), 60, 60) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({ "error": "rate_limited" })),
        )
            .into_response();
    }
    let method = req.method().clone();
    let method_is_head = method == Method::HEAD;
    let headers = req.headers().clone();
    let mut res = stirling_edit::serve_session_file(&state.pool, &token, method_is_head).await;
    stirling_edit::apply_stirling_file_cors(&method, &headers, &mut res);
    res
}

async fn stirling_session_file_options(headers: HeaderMap) -> Response {
    let mut res = StatusCode::NO_CONTENT.into_response();
    // Preflight is not a GET: no CORS headers (the file response carries them).
    stirling_edit::apply_stirling_file_cors(&Method::OPTIONS, &headers, &mut res);
    res
}

async fn stirling_commit_token(
    State(state): State<AppState>,
    Path(token): Path<String>,
    req: Request,
) -> Response {
    if !rate_limit_ok(&format!("stirling-commit:{token}"), 10, 60) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({ "error": "rate_limited" })),
        )
            .into_response();
    }
    match parse_stirling_commit_body(&state, req).await {
        Ok((file, pull, _)) => match stirling_edit::commit_session(&state.pool, &token, file, pull).await
        {
            Ok(resp) => Json(resp).into_response(),
            Err(e) => e.into_response(),
        },
        Err(e) => e.into_response(),
    }
}

async fn stirling_commit_auth(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i32>,
    req: Request,
) -> Response {
    if !rate_limit_ok(&format!("stirling-commit-auth:{}", user.id), 10, 60) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({ "error": "rate_limited" })),
        )
            .into_response();
    }
    if !auth::document_owned(&state.pool, id, user.id)
        .await
        .unwrap_or(false)
    {
        return not_found_json();
    }
    match parse_stirling_commit_body(&state, req).await {
        Ok((file, pull, token)) => {
            match stirling_edit::commit_for_document(
                &state.pool,
                user.id,
                id,
                token.as_deref(),
                file,
                pull,
            )
            .await
            {
                Ok(resp) => Json(resp).into_response(),
                Err(e) => e.into_response(),
            }
        }
        Err(e) => e.into_response(),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StirlingCommitJson {
    #[serde(default)]
    pull: bool,
    #[serde(default)]
    token: Option<String>,
}

async fn parse_stirling_commit_body(
    state: &AppState,
    req: Request,
) -> Result<(Option<(Vec<u8>, String)>, bool, Option<String>), stirling_edit::StartError> {
    let ct = req
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ct.starts_with("multipart/form-data") {
        let mut multipart = Multipart::from_request(req, state)
            .await
            .map_err(|_| stirling_edit::StartError::bad("missing_file"))?;
        let mut raw: Option<Vec<u8>> = None;
        let mut filename = String::new();
        let mut pull = false;
        let mut token: Option<String> = None;
        while let Ok(Some(field)) = multipart.next_field().await {
            let name = field.name().unwrap_or("").to_string();
            if name == "file" || name == "document" || name == "upload" {
                filename = field.file_name().unwrap_or("document.pdf").to_string();
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|_| stirling_edit::StartError::bad("missing_file"))?;
                if bytes.len() > crate::limits::MAX_UPLOAD_BYTES {
                    return Err(stirling_edit::StartError {
                        status: StatusCode::PAYLOAD_TOO_LARGE,
                        code: "payload_too_large",
                    });
                }
                raw = Some(bytes.to_vec());
            } else if name == "pull" {
                let v = field.text().await.unwrap_or_default();
                pull = matches!(v.trim(), "1" | "true" | "yes");
            } else if name == "token" {
                let v = field.text().await.unwrap_or_default();
                if !v.trim().is_empty() {
                    token = Some(v.trim().to_string());
                }
            }
        }
        Ok((raw.map(|b| (b, filename)), pull, token))
    } else {
        let Json(body) = Json::<StirlingCommitJson>::from_request(req, state)
            .await
            .map_err(|_| stirling_edit::StartError::bad("missing_file"))?;
        Ok((None, body.pull, body.token))
    }
}

async fn add_note(
    State(state): State<AppState>,
    user: AuthUser,
    Json(data): Json<NoteInput>,
) -> Result<Response, AppError> {
    if !auth::document_owned(&state.pool, data.document_id, user.id).await? {
        return Ok(not_found_json());
    }
    let body: String = data.body.chars().take(4000).collect();
    sqlx::query("insert into notes (document_id, body, owner_id) values ($1,$2,$3)")
        .bind(data.document_id)
        .bind(body)
        .bind(user.id)
        .execute(&state.pool)
        .await?;
    queries::log_audit(&state.pool, data.document_id, "note", Some("Note ajoutée")).await?;
    Ok(Json(queries::get_document(&state.pool, data.document_id, user.id).await?).into_response())
}

async fn upsert_tag(
    State(state): State<AppState>,
    user: AuthUser,
    Json(data): Json<TagInput>,
) -> Result<Response, AppError> {
    if let Some(id) = data.id {
        if !auth::tag_owned(&state.pool, id, user.id).await? {
            return Ok(not_found_json());
        }
        sqlx::query(
            "update tags set name=$1, color=$2, is_inbox_tag=$3, match=$4, matching_algorithm=$5 where id=$6 and owner_id=$7",
        )
        .bind(&data.name)
        .bind(&data.color)
        .bind(data.is_inbox_tag)
        .bind(&data.match_text)
        .bind(&data.matching_algorithm)
        .bind(id)
        .bind(user.id)
        .execute(&state.pool)
        .await?;
        let tag = queries::get_tag(&state.pool, id, user.id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Étiquette introuvable"))?;
        return Ok(Json(tag).into_response());
    }
    if let Some(existing) = queries::find_tag_by_name_ci(&state.pool, &data.name, user.id).await? {
        return Ok(Json(existing).into_response());
    }
    let tag = queries::insert_tag(
        &state.pool,
        user.id,
        &data.name,
        &data.color,
        data.is_inbox_tag,
        &data.match_text,
        &data.matching_algorithm,
    )
    .await?;
    Ok(Json(tag).into_response())
}

async fn delete_tag(State(state): State<AppState>, user: AuthUser, Path(id): Path<i32>) -> Result<Response, AppError> {
    if !auth::tag_owned(&state.pool, id, user.id).await? {
        return Ok(not_found_json());
    }
    sqlx::query("delete from tags where id = $1 and owner_id = $2")
        .bind(id)
        .bind(user.id)
        .execute(&state.pool)
        .await?;
    Ok(Json(serde_json::json!({"ok": true})).into_response())
}

async fn upsert_correspondent(
    State(state): State<AppState>,
    user: AuthUser,
    Json(data): Json<CorrespondentInput>,
) -> Result<Response, AppError> {
    if let Some(id) = data.id {
        if !auth::correspondent_owned(&state.pool, id, user.id).await? {
            return Ok(not_found_json());
        }
        sqlx::query("update correspondents set name=$1, match=$2, matching_algorithm=$3, notes=$4 where id=$5 and owner_id=$6")
            .bind(&data.name)
            .bind(&data.match_text)
            .bind(&data.matching_algorithm)
            .bind(&data.notes)
            .bind(id)
            .bind(user.id)
            .execute(&state.pool)
            .await?;
        let corr = queries::get_correspondent(&state.pool, id, user.id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Correspondant introuvable"))?;
        return Ok(Json(corr).into_response());
    }
    if let Some(existing) = queries::find_correspondent_by_name_ci(&state.pool, &data.name, user.id).await? {
        return Ok(Json(existing).into_response());
    }
    let corr = queries::insert_correspondent(
        &state.pool,
        user.id,
        &data.name,
        &data.match_text,
        &data.matching_algorithm,
        data.notes.as_deref(),
    )
    .await?;
    Ok(Json(corr).into_response())
}

async fn delete_correspondent(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i32>,
) -> Result<Response, AppError> {
    if !auth::correspondent_owned(&state.pool, id, user.id).await? {
        return Ok(not_found_json());
    }
    sqlx::query("delete from correspondents where id = $1 and owner_id = $2")
        .bind(id)
        .bind(user.id)
        .execute(&state.pool)
        .await?;
    Ok(Json(serde_json::json!({"ok": true})).into_response())
}

async fn upsert_type(
    State(state): State<AppState>,
    user: AuthUser,
    Json(data): Json<TypeInput>,
) -> Result<Response, AppError> {
    if let Some(id) = data.id {
        if !auth::type_owned(&state.pool, id, user.id).await? {
            return Ok(not_found_json());
        }
        sqlx::query("update document_types set name=$1, match=$2, matching_algorithm=$3 where id=$4 and owner_id=$5")
            .bind(&data.name)
            .bind(&data.match_text)
            .bind(&data.matching_algorithm)
            .bind(id)
            .bind(user.id)
            .execute(&state.pool)
            .await?;
        let dt = queries::get_type(&state.pool, id, user.id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Type introuvable"))?;
        return Ok(Json(dt).into_response());
    }
    if let Some(existing) = queries::find_type_by_name_ci(&state.pool, &data.name, user.id).await? {
        return Ok(Json(existing).into_response());
    }
    let dt = queries::insert_type(
        &state.pool,
        user.id,
        &data.name,
        &data.match_text,
        &data.matching_algorithm,
    )
    .await?;
    Ok(Json(dt).into_response())
}

async fn delete_type(State(state): State<AppState>, user: AuthUser, Path(id): Path<i32>) -> Result<Response, AppError> {
    if !auth::type_owned(&state.pool, id, user.id).await? {
        return Ok(not_found_json());
    }
    sqlx::query("delete from document_types where id = $1 and owner_id = $2")
        .bind(id)
        .bind(user.id)
        .execute(&state.pool)
        .await?;
    Ok(Json(serde_json::json!({"ok": true})).into_response())
}

async fn upsert_path(
    State(state): State<AppState>,
    user: AuthUser,
    Json(data): Json<PathInput>,
) -> Result<Response, AppError> {
    if let Some(id) = data.id {
        if !auth::path_owned(&state.pool, id, user.id).await? {
            return Ok(not_found_json());
        }
        sqlx::query("update storage_paths set name=$1, path=$2, match=$3, matching_algorithm=$4 where id=$5 and owner_id=$6")
            .bind(&data.name)
            .bind(&data.path)
            .bind(&data.match_text)
            .bind(&data.matching_algorithm)
            .bind(id)
            .bind(user.id)
            .execute(&state.pool)
            .await?;
    } else {
        sqlx::query("insert into storage_paths (owner_id, name, path, match, matching_algorithm) values ($1,$2,$3,$4,$5)")
            .bind(user.id)
            .bind(&data.name)
            .bind(&data.path)
            .bind(&data.match_text)
            .bind(&data.matching_algorithm)
            .execute(&state.pool)
            .await?;
    }
    Ok(Json(serde_json::json!({"ok": true})).into_response())
}

async fn delete_path(State(state): State<AppState>, user: AuthUser, Path(id): Path<i32>) -> Result<Response, AppError> {
    if !auth::path_owned(&state.pool, id, user.id).await? {
        return Ok(not_found_json());
    }
    sqlx::query("delete from storage_paths where id = $1 and owner_id = $2")
        .bind(id)
        .bind(user.id)
        .execute(&state.pool)
        .await?;
    Ok(Json(serde_json::json!({"ok": true})).into_response())
}

async fn upsert_field(
    State(state): State<AppState>,
    user: AuthUser,
    Json(data): Json<FieldInput>,
) -> Result<Response, AppError> {
    if let Some(id) = data.id {
        if !auth::field_owned(&state.pool, id, user.id).await? {
            return Ok(not_found_json());
        }
        sqlx::query("update custom_fields set name=$1, data_type=$2 where id=$3 and owner_id=$4")
            .bind(&data.name)
            .bind(&data.data_type)
            .bind(id)
            .bind(user.id)
            .execute(&state.pool)
            .await?;
    } else {
        sqlx::query("insert into custom_fields (owner_id, name, data_type) values ($1,$2,$3)")
            .bind(user.id)
            .bind(&data.name)
            .bind(&data.data_type)
            .execute(&state.pool)
            .await?;
    }
    Ok(Json(serde_json::json!({"ok": true})).into_response())
}

async fn delete_field(State(state): State<AppState>, user: AuthUser, Path(id): Path<i32>) -> Result<Response, AppError> {
    if !auth::field_owned(&state.pool, id, user.id).await? {
        return Ok(not_found_json());
    }
    sqlx::query("delete from custom_fields where id = $1 and owner_id = $2")
        .bind(id)
        .bind(user.id)
        .execute(&state.pool)
        .await?;
    Ok(Json(serde_json::json!({"ok": true})).into_response())
}

async fn upsert_view(
    State(state): State<AppState>,
    user: AuthUser,
    Json(data): Json<ViewInput>,
) -> Result<Response, AppError> {
    let filter = serde_json::to_string(&data.filter)?;
    if let Some(id) = data.id {
        if !auth::view_owned(&state.pool, id, user.id).await? {
            return Ok(not_found_json());
        }
        sqlx::query(
            "update saved_views set name=$1, show_on_dashboard=$2, show_in_sidebar=$3, sort_field=$4, sort_reverse=$5, filter=$6::jsonb where id=$7 and owner_id=$8",
        )
        .bind(&data.name)
        .bind(data.show_on_dashboard)
        .bind(data.show_in_sidebar)
        .bind(&data.sort_field)
        .bind(data.sort_reverse)
        .bind(&filter)
        .bind(id)
        .bind(user.id)
        .execute(&state.pool)
        .await?;
    } else {
        sqlx::query(
            "insert into saved_views (owner_id, name, show_on_dashboard, show_in_sidebar, sort_field, sort_reverse, filter) values ($1,$2,$3,$4,$5,$6,$7::jsonb)",
        )
        .bind(user.id)
        .bind(&data.name)
        .bind(data.show_on_dashboard)
        .bind(data.show_in_sidebar)
        .bind(&data.sort_field)
        .bind(data.sort_reverse)
        .bind(&filter)
        .execute(&state.pool)
        .await?;
    }
    Ok(Json(serde_json::json!({"ok": true})).into_response())
}

async fn delete_view(State(state): State<AppState>, user: AuthUser, Path(id): Path<i32>) -> Result<Response, AppError> {
    if !auth::view_owned(&state.pool, id, user.id).await? {
        return Ok(not_found_json());
    }
    sqlx::query("delete from saved_views where id = $1 and owner_id = $2")
        .bind(id)
        .bind(user.id)
        .execute(&state.pool)
        .await?;
    Ok(Json(serde_json::json!({"ok": true})).into_response())
}

async fn upsert_workflow(
    State(state): State<AppState>,
    user: AuthUser,
    Json(data): Json<WorkflowInput>,
) -> Result<Response, AppError> {
    let filters = data.filters.to_string();
    let actions = data.actions.to_string();
    if let Some(id) = data.id {
        if !auth::workflow_owned(&state.pool, id, user.id).await? {
            return Ok(not_found_json());
        }
        sqlx::query(
            "update workflows set name=$1, enabled=$2, trigger_type=$3, filters=$4::jsonb, actions=$5::jsonb where id=$6 and owner_id=$7",
        )
        .bind(&data.name)
        .bind(data.enabled)
        .bind(&data.trigger_type)
        .bind(&filters)
        .bind(&actions)
        .bind(id)
        .bind(user.id)
        .execute(&state.pool)
        .await?;
    } else {
        sqlx::query(
            "insert into workflows (owner_id, name, enabled, trigger_type, filters, actions) values ($1,$2,$3,$4,$5::jsonb,$6::jsonb)",
        )
        .bind(user.id)
        .bind(&data.name)
        .bind(data.enabled)
        .bind(&data.trigger_type)
        .bind(&filters)
        .bind(&actions)
        .execute(&state.pool)
        .await?;
    }
    Ok(Json(serde_json::json!({"ok": true})).into_response())
}

async fn delete_workflow(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i32>,
) -> Result<Response, AppError> {
    if !auth::workflow_owned(&state.pool, id, user.id).await? {
        return Ok(not_found_json());
    }
    sqlx::query("delete from workflows where id = $1 and owner_id = $2")
        .bind(id)
        .bind(user.id)
        .execute(&state.pool)
        .await?;
    Ok(Json(serde_json::json!({"ok": true})).into_response())
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct MailSim {
    rule_id: i32,
}

async fn simulate_mail(
    State(state): State<AppState>,
    user: AuthUser,
    Json(data): Json<MailSim>,
) -> Result<Json<serde_json::Value>, AppError> {
    let row = sqlx::query(
        "select r.name, r.filter_from from mail_rules r join mail_accounts a on a.id = r.account_id where r.id = $1 and a.owner_id = $2",
    )
        .bind(data.rule_id)
        .bind(user.id)
        .fetch_optional(&state.pool)
        .await?;
    let Some(row) = row else {
        return Ok(Json(serde_json::json!({"ok": false, "error": "Règle introuvable"})));
    };
    let name: String = row.get("name");
    let filter_from: Option<String> = row.get("filter_from");
    let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let lines = vec![
        format!("Règle : {name}"),
        format!("Expéditeur : {}", filter_from.as_deref().unwrap_or("inconnnu")),
        "Pièce jointe consommée depuis la boîte.".into(),
        "Classement automatique via workflows Paperrust.".into(),
    ];
    let corr = filter_from.clone().unwrap_or_else(|| "IMAP".into());
    let title = format!("Courrier · {name}");
    let svg = crate::paper::render_paper_svg(crate::paper::PaperSpec {
        kicker: Some("Mail"),
        title: &title,
        correspondent: &corr,
        date: &date,
        r#ref: None,
        lines: &lines,
        footer: Some("Consommation mail simulée"),
        stamp: Some("IMAP"),
        accent: None,
    });
    let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, svg.as_bytes());
    let result = consume::consume_document(
        &state.pool,
        ConsumeInput {
            filename: format!("mail-{}-{}.svg", data.rule_id, chrono::Utc::now().timestamp_millis()),
            mime_type: "image/svg+xml".into(),
            data_base64: b64,
            raw_bytes: None,
            provider: "xai".into(),
            api_key: None,
            skip_ocr: Some(true),
            base_url: None,
            title_override: None,
            created_override: None,
        },
        user.id,
    )
    .await?;
    Ok(Json(serde_json::json!({"ok": true, "documentId": result.document_id})))
}

async fn jobs(State(state): State<AppState>, user: AuthUser) -> Result<Json<Vec<JobOut>>, AppError> {
    Ok(Json(queries::list_jobs(&state.pool, user.id).await?))
}

async fn list_tags_h(State(state): State<AppState>, user: AuthUser) -> Result<Json<Vec<Tag>>, AppError> {
    Ok(Json(queries::list_tags(&state.pool, user.id).await?))
}

async fn list_correspondents_h(State(state): State<AppState>, user: AuthUser) -> Result<Json<Vec<Correspondent>>, AppError> {
    Ok(Json(queries::list_correspondents(&state.pool, user.id).await?))
}

async fn list_types_h(State(state): State<AppState>, user: AuthUser) -> Result<Json<Vec<DocumentType>>, AppError> {
    Ok(Json(queries::list_types(&state.pool, user.id).await?))
}

async fn list_paths_h(State(state): State<AppState>, user: AuthUser) -> Result<Json<Vec<StoragePath>>, AppError> {
    Ok(Json(queries::list_paths(&state.pool, user.id).await?))
}

async fn list_fields_h(State(state): State<AppState>, user: AuthUser) -> Result<Json<Vec<CustomField>>, AppError> {
    Ok(Json(queries::list_custom_fields(&state.pool, user.id).await?))
}

async fn list_views_h(State(state): State<AppState>, user: AuthUser) -> Result<Json<Vec<SavedView>>, AppError> {
    Ok(Json(queries::list_saved_views(&state.pool, user.id).await?))
}

async fn list_workflows_h(State(state): State<AppState>, user: AuthUser) -> Result<Json<Vec<Workflow>>, AppError> {
    Ok(Json(queries::list_workflows(&state.pool, user.id).await?))
}


async fn purge_expired_for_user(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<serde_json::Value>, AppError> {
    let n = queries::purge_expired_trash_for_owner(&state.pool, user.id).await?;
    Ok(Json(serde_json::json!({"ok": true, "purged": n})))
}

async fn cron_purge_trash(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> Result<Response, AppError> {
    let expected = std::env::var("PAPERRUST_CRON_TOKEN").unwrap_or_default();
    let got = headers
        .get("x-paperrust-cron-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if expected.is_empty() || got != expected {
        return Ok(auth::unauthorized());
    }
    let n = queries::purge_expired_trash(&state.pool).await?;
    Ok(Json(serde_json::json!({"ok": true, "purged": n})).into_response())
}

async fn get_settings(State(state): State<AppState>, user: AuthUser) -> Result<Json<SettingsOut>, AppError> {
    let row = sqlx::query(
        "select ocr_provider, ocr_language, auto_match, coalesce(ocr_base_url, '') as ocr_base_url, trash_retention_days from user_settings where user_id = $1",
    )
    .bind(user.id)
    .fetch_optional(&state.pool)
    .await?;
    let (ocr_provider, ocr_language, auto_match, ocr_base_url, trash_retention_days) = if let Some(r) = row {
        let days = r.get::<i32, _>("trash_retention_days");
        let days = match days {
            60 | 90 => days,
            _ => 30,
        };
        (
            r.get::<String, _>("ocr_provider"),
            r.get::<String, _>("ocr_language"),
            r.get::<bool, _>("auto_match"),
            r.get::<String, _>("ocr_base_url"),
            days,
        )
    } else {
        // Legacy global defaults (app_settings), then hard defaults.
        let rows = sqlx::query("select key, value::text as value from app_settings")
            .fetch_all(&state.pool)
            .await
            .unwrap_or_default();
        let mut map = std::collections::HashMap::new();
        for r in rows {
            let k: String = r.get("key");
            let v: String = r.get("value");
            map.insert(k, v);
        }
        let ocr_provider = map
            .get("ocr_provider")
            .and_then(|v| serde_json::from_str::<String>(v).ok())
            .unwrap_or_else(|| "xai".into());
        let ocr_language = map
            .get("ocr_language")
            .and_then(|v| serde_json::from_str::<String>(v).ok())
            .unwrap_or_else(|| "fr".into());
        let auto_match = map
            .get("auto_match")
            .and_then(|v| serde_json::from_str::<bool>(v).ok())
            .unwrap_or(true);
        (ocr_provider, ocr_language, auto_match, String::new(), 30)
    };
    let llm_keys = llm_keys::list_statuses(&state.pool, user.id).await?;
    let has_user_key = llm_keys
        .iter()
        .any(|k| k.provider == ocr_provider && k.has_key);
    let selfhosted_ready =
        ocr_provider == "selfhosted" && !ocr_base_url.trim().is_empty();
    let ai_available = selfhosted_ready
        || has_user_key
        || (std::env::var("PAPERRUST_LLM_ENV_FALLBACK").ok().as_deref() == Some("1")
            && ocr::env_key_configured(&ocr_provider));
    Ok(Json(SettingsOut {
        ocr_provider,
        ocr_language,
        auto_match,
        ai_available,
        llm_keys,
        ocr_base_url,
        trash_retention_days,
    }))
}

async fn save_settings(
    State(state): State<AppState>,
    user: AuthUser,
    Json(data): Json<SettingsInput>,
) -> Result<Json<serde_json::Value>, AppError> {
    // active OCR provider must be a consume allowlist entry (not kimi/mistral).
    let provider = llm_keys::normalize_consume_provider(&data.ocr_provider)
        .unwrap_or("xai")
        .to_string();
    let ocr_base_url = data
        .ocr_base_url
        .as_deref()
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    let trash_retention_days = match data.trash_retention_days.unwrap_or(30) {
        60 => 60,
        90 => 90,
        _ => 30,
    };
    let ocr_base_url = if provider == "selfhosted" || !ocr_base_url.is_empty() {
        if provider == "selfhosted" && ocr_base_url.is_empty() {
            return Err(anyhow::anyhow!(
                "URL de base requise pour Auto-hébergé (ex. https://llm.exemple.org/v1)."
            )
            .into());
        }
        if !ocr_base_url.is_empty() {
            crate::ocr_ssrf::validate_selfhosted_ocr_base_url(&ocr_base_url)
                .map_err(|msg| anyhow::anyhow!("{msg}"))?
        } else {
            ocr_base_url
        }
    } else {
        ocr_base_url
    };
    // Persist base URL whenever provided; keep previous value if switching away with empty string.
    sqlx::query(
        r#"insert into user_settings (user_id, ocr_provider, ocr_language, auto_match, ocr_base_url, trash_retention_days, updated_at)
           values ($1, $2, $3, $4, $5, $6, now())
           on conflict (user_id) do update set
             ocr_provider = excluded.ocr_provider,
             ocr_language = excluded.ocr_language,
             auto_match = excluded.auto_match,
             ocr_base_url = case
               when $2 = 'selfhosted' then excluded.ocr_base_url
               when excluded.ocr_base_url <> '' then excluded.ocr_base_url
               else user_settings.ocr_base_url
             end,
             trash_retention_days = excluded.trash_retention_days,
             updated_at = now()"#,
    )
    .bind(user.id)
    .bind(&provider)
    .bind(&data.ocr_language)
    .bind(data.auto_match)
    .bind(&ocr_base_url)
    .bind(trash_retention_days)
    .execute(&state.pool)
    .await?;
    Ok(Json(serde_json::json!({"ok": true})))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateApiTokenBody {
    #[serde(default)]
    name: Option<String>,
}

async fn list_api_tokens(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<serde_json::Value>, AppError> {
    let tokens = api_tokens::list_tokens(&state.pool, user.id).await?;
    Ok(Json(serde_json::json!({ "tokens": tokens })))
}

async fn create_api_token(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<CreateApiTokenBody>,
) -> Result<Json<serde_json::Value>, AppError> {
    if !rate_limit_ok(&format!("apitoken:{}", user.id), 10, 60) {
        return Err(anyhow::anyhow!("Trop de créations de tokens — réessayez dans une minute.").into());
    }
    let name = body.name.unwrap_or_default();
    let created = api_tokens::create_token(&state.pool, user.id, &name).await?;
    Ok(Json(serde_json::json!({
        "ok": true,
        "id": created.id,
        "name": created.name,
        "prefix": created.prefix,
        "token": created.token,
        "createdAt": created.created_at,
    })))
}

async fn revoke_api_token(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i32>,
) -> Result<Json<serde_json::Value>, AppError> {
    let ok = api_tokens::revoke_token(&state.pool, user.id, id).await?;
    if !ok {
        return Ok(Json(serde_json::json!({
            "ok": false,
            "error": "Token introuvable ou déjà révoqué."
        })));
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// iOS Shortcut / machine ingest: Bearer `pr_…` + multipart `file`.
async fn ingest_v1(State(state): State<AppState>, req: Request) -> Response {
    let bearer = match api_tokens::bearer_from_headers(req.headers()) {
        Some(t) => t,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({
                    "ok": false,
                    "error": "Token API manquant (Authorization: Bearer pr_…)."
                })),
            )
                .into_response();
        }
    };
    let auth = match api_tokens::authenticate(&state.pool, &bearer).await {
        Ok(Some(pair)) => pair,
        Ok(None) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({
                    "ok": false,
                    "error": "Token API invalide ou révoqué."
                })),
            )
                .into_response();
        }
        Err(e) => {
            tracing::error!("ingest token lookup: {e:#}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "ok": false,
                    "error": "Erreur serveur."
                })),
            )
                .into_response();
        }
    };
    let (token_id, owner_id) = auth;
    if !rate_limit_ok(&format!("ingest:{token_id}"), 30, 60) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({
                "ok": false,
                "error": "Trop de requêtes — réessayez dans une minute."
            })),
        )
            .into_response();
    }

    let input = match consume_input_from_request(&state, req).await {
        Ok(i) => i,
        Err(AppError(err)) => {
            tracing::warn!(error = %crate::ocr::redact_for_log(&err.to_string()), "ingest multipart failed");
            let lower = err.to_string().to_ascii_lowercase();
            let (status, code) = if lower.contains("trop volumineux")
                || lower.contains("too large")
                || lower.contains("length limit")
            {
                (StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large")
            } else if lower.contains("manquant") || lower.contains("missing") {
                (StatusCode::BAD_REQUEST, "missing_file")
            } else {
                (StatusCode::BAD_REQUEST, "request_failed")
            };
            return (
                status,
                Json(serde_json::json!({ "ok": false, "error": code })),
            )
                .into_response();
        }
    };

    match consume::consume_document(&state.pool, input, owner_id).await {
        Ok(result) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "ok": true,
                "documentId": result.document_id,
                "usedLlm": result.used_llm,
                "error": result.error,
            })),
        )
            .into_response(),
        Err(err) => {
            tracing::warn!(error = %crate::ocr::redact_for_log(&err.to_string()), "ingest consume failed");
            let lower = err.to_string().to_ascii_lowercase();
            let (status, code) = if lower.contains("trop volumineux")
                || lower.contains("too large")
                || lower.contains("length limit")
            {
                (StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large")
            } else {
                (StatusCode::BAD_REQUEST, "request_failed")
            };
            (
                status,
                Json(serde_json::json!({ "ok": false, "error": code })),
            )
                .into_response()
        }
    }
}

/// Paperless-ngx / Quickscan: `POST /api/documents/post_document/`
/// Auth: `Authorization: Token pr_…` (also Bearer / Basic with pr_…).
/// Multipart field `document` (aliases `file` / `upload`). Optional `title`, `created`.
/// Success: HTTP 200 + JSON string task id (ngx-shaped) — document is already stored.
async fn post_document_ngx(State(state): State<AppState>, req: Request) -> Response {
    let raw = match api_tokens::raw_token_from_headers(req.headers()) {
        Some(t) => t,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({
                    "detail": "Authentication credentials were not provided."
                })),
            )
                .into_response();
        }
    };
    let auth = match api_tokens::authenticate(&state.pool, &raw).await {
        Ok(Some(pair)) => pair,
        Ok(None) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({
                    "detail": "Invalid token."
                })),
            )
                .into_response();
        }
        Err(e) => {
            tracing::error!("post_document token lookup: {e:#}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "detail": "Server error." })),
            )
                .into_response();
        }
    };
    let (token_id, owner_id) = auth;
    if !rate_limit_ok(&format!("ingest:{token_id}"), 30, 60) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({
                "detail": "Request was throttled. Expected available in 60 seconds."
            })),
        )
            .into_response();
    }

    let input = match consume_input_from_request(&state, req).await {
        Ok(i) => i,
        Err(AppError(err)) => {
            tracing::warn!(error = %crate::ocr::redact_for_log(&err.to_string()), "post_document multipart failed");
            let lower = err.to_string().to_ascii_lowercase();
            let (status, detail) = if lower.contains("trop volumineux")
                || lower.contains("too large")
                || lower.contains("length limit")
            {
                (StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large")
            } else if lower.contains("manquant") || lower.contains("missing") {
                (StatusCode::BAD_REQUEST, "missing_file")
            } else {
                (StatusCode::BAD_REQUEST, "request_failed")
            };
            return (status, Json(serde_json::json!({ "detail": detail }))).into_response();
        }
    };

    match consume::consume_document(&state.pool, input, owner_id).await {
        Ok(result) => {
            // ngx returns a JSON string (task UUID). We return a UUID receipt while the
            // document is already committed (synchronous consume). Clients that only
            // check HTTP 200 (Quickscan) succeed; the real id is also in X-Document-Id.
            let task_id = uuid::Uuid::new_v4().to_string();
            let mut res = (
                StatusCode::OK,
                Json(serde_json::Value::String(task_id)),
            )
                .into_response();
            if let Ok(v) = axum::http::HeaderValue::from_str(&result.document_id.to_string()) {
                res.headers_mut().insert("x-document-id", v);
            }
            res
        }
        Err(err) => {
            tracing::warn!(error = %crate::ocr::redact_for_log(&err.to_string()), "post_document consume failed");
            (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "detail": "request_failed" })),
            )
                .into_response()
        }
    }
}

async fn patch_llm_key(
    State(state): State<AppState>,
    user: AuthUser,
    Json(data): Json<LlmKeyPatch>,
) -> Result<Json<serde_json::Value>, AppError> {
    if !rate_limit_ok(&format!("llmkey:{}", user.id), 30, 60) {
        return Err(anyhow::anyhow!("rate limited").into());
    }
    let status = if data.clear {
        llm_keys::clear_key(&state.pool, user.id, &data.provider).await?
    } else {
        let Some(raw) = data.api_key.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
            return Err(anyhow::anyhow!("apiKey requis (ou clear: true)").into());
        };
        let _ = raw.len(); // never log the key
        llm_keys::upsert_key(&state.pool, user.id, &data.provider, raw).await?
    };
    Ok(Json(serde_json::json!({
        "ok": true,
        "provider": status.provider,
        "hasKey": status.has_key,
        "masked": status.masked,
    })))
}


async fn reject_untrusted_public_host(req: Request, next: Next) -> Response {
    let peer = connect_peer(&req);
    if auth::untrusted_public_host(req.headers(), peer) {
        tracing::warn!("refused public Host from untrusted peer");
        return (
            axum::http::StatusCode::BAD_REQUEST,
            axum::Json(serde_json::json!({"error": "untrusted_public_host"})),
        )
            .into_response();
    }
    if auth::loopback_host_unlisted(req.headers(), peer) {
        tracing::warn!("refused unlisted Host from loopback proxy");
        return (
            axum::http::StatusCode::BAD_REQUEST,
            axum::Json(serde_json::json!({"error": "unlisted_host"})),
        )
            .into_response();
    }
    next.run(req).await
}

async fn auth_gate(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    let peer = connect_peer(&req);
    // cookie-auth mutations (incl. public logout) need same-origin CSRF.
    if path.starts_with("/api") && !auth::csrf_ok(req.headers(), peer, req.method()) {
        return auth::csrf_forbidden();
    }
    if auth::is_public_path(&path) || !path.starts_with("/api") {
        return next.run(req).await;
    }
    match auth::user_from_headers(&state.pool, req.headers(), peer).await {
        Some(user) => {
            req.extensions_mut().insert(user);
            next.run(req).await
        }
        // Smoke cleanup: the same `pr_` ingest token may soft-delete by id.
        None if path == "/api/documents/bulk" && *req.method() == Method::POST => {
            if let Some(user) = auth::user_from_ingest_token(&state.pool, req.headers()).await {
                req.extensions_mut().insert(auth::IngestTokenScope);
                req.extensions_mut().insert(user);
                next.run(req).await
            } else {
                auth::unauthorized()
            }
        }
        None => auth::unauthorized(),
    }
}


/// Client IP for rate limits. Uses X-Forwarded-For only when the TCP peer is a
/// trusted reverse-proxy (loopback), via `auth::peer_trusts_xfh`.
fn client_ip(headers: &axum::http::HeaderMap, peer: Option<IpAddr>) -> String {
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
    peer.map(|p| p.to_string()).unwrap_or_else(|| "0.0.0.0".to_string())
}

fn rate_limit_ok(key: &str, max: u32, window_secs: u64) -> bool {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    use std::time::{SystemTime, UNIX_EPOCH};
    static RATE: OnceLock<Mutex<HashMap<String, (u64, u32)>>> = OnceLock::new();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut map = RATE.get_or_init(|| Mutex::new(HashMap::new())).lock().unwrap();
    // Evict stale windows so the map cannot grow without bound.
    let ttl = window_secs.saturating_mul(2).max(120);
    map.retain(|_, (start, _)| now.saturating_sub(*start) < ttl);
    let entry = map.entry(key.to_string()).or_insert((now, 0));
    if now.saturating_sub(entry.0) >= window_secs {
        *entry = (now, 1);
        return true;
    }
    entry.1 = entry.1.saturating_add(1);
    entry.1 <= max
}


fn connect_peer(req: &Request) -> Option<IpAddr> {
    req.extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ci| ci.0.ip())
}

async fn oidc_login_alias(State(state): State<AppState>, req: Request) -> Response {
    if !rate_limit_ok(&format!("login:{}", client_ip(req.headers(), connect_peer(&req))), 20, 60) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({"error": "rate_limited"})),
        )
            .into_response();
    }
    auth::start_login(&state.pool, req.headers(), connect_peer(&req), "/api/auth/callback").await
}

async fn login_callback_alias(req: Request) -> Response {
    let loc = match req.uri().query() {
        Some(q) if !q.is_empty() => format!("/api/auth/callback?{q}"),
        _ => "/api/auth/callback".to_string(),
    };
    let mut res = StatusCode::FOUND.into_response();
    if let Ok(v) = HeaderValue::from_str(&loc) {
        res.headers_mut().insert(axum::http::header::LOCATION, v);
    }
    res
}

async fn oidc_login(State(state): State<AppState>, req: Request) -> Response {
    if !rate_limit_ok(&format!("login:{}", client_ip(req.headers(), connect_peer(&req))), 20, 60) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({"error": "rate_limited"})),
        )
            .into_response();
    }
    auth::start_login(&state.pool, req.headers(), connect_peer(&req), "/api/auth/callback").await
}

async fn oidc_auth(
    State(state): State<AppState>,
    Query(q): Query<auth::CallbackQuery>,
    req: Request,
) -> Response {
    if q.code.is_some() || q.error.is_some() {
        return auth::finish_callback(&state.pool, req.headers(), connect_peer(&req), q).await;
    }
    match auth::user_from_headers(&state.pool, req.headers(), connect_peer(&req)).await {
        Some(user) => Json(serde_json::json!({
            "email": user.email,
            "name": user.name,
            "sub": user.sub,
        }))
        .into_response(),
        None => auth::unauthorized(),
    }
}

async fn oidc_callback(
    State(state): State<AppState>,
    Query(q): Query<auth::CallbackQuery>,
    req: Request,
) -> Response {
    if q.code.is_some() || q.error.is_some() {
        auth::finish_callback(&state.pool, req.headers(), connect_peer(&req), q).await
    } else {
        auth::start_login(&state.pool, req.headers(), connect_peer(&req), "/api/auth/callback").await
    }
}

async fn oidc_logout(State(state): State<AppState>, req: Request) -> Response {
    auth::logout(&state.pool, req.headers(), connect_peer(&req)).await
}

async fn me(user: AuthUser) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "email": user.email,
        "name": user.name,
        "sub": user.sub,
    }))
}

/// The explicit LAN split, end to end through the real
/// `reject_untrusted_public_host` layer, with the TCP peer forced per case.
/// Placeholder hosts: `beta.archive.lan` stands for the LAN origin behind
/// ingress-nginx, `archive.example` for the public origin behind the tunnel.
#[cfg(test)]
mod lan_split_http_tests {
    use super::reject_untrusted_public_host;
    use axum::extract::{ConnectInfo, Request};
    use axum::middleware::{from_fn, Next};
    use axum::routing::get;
    use axum::Router;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    const LAN: &str = "beta.archive.lan";
    const PUBLIC: &str = "archive.example";

    type Saved = Vec<(&'static str, Option<String>)>;

    fn set_env() -> Saved {
        let vars: [(&'static str, &str); 4] = [
            ("PAPERRUST_PUBLIC_URLS", "https://beta.archive.lan,https://archive.example"),
            (
                "PAPERRUST_OIDC_ISSUERS",
                "https://beta.archive.lan=https://auth.lan,https://archive.example=https://auth.example",
            ),
            ("PAPERRUST_OIDC_ISSUER", "https://auth.example/application/o/x/"),
            ("PAPERRUST_LAN_URLS", "https://beta.archive.lan"),
        ];
        let prev = vars
            .iter()
            .map(|(k, _)| (*k, std::env::var(k).ok()))
            .collect();
        for (k, v) in vars {
            unsafe { std::env::set_var(k, v) };
        }
        prev
    }

    fn restore(prev: Saved) {
        for (k, v) in prev {
            match v {
                Some(v) => unsafe { std::env::set_var(k, v) },
                None => unsafe { std::env::remove_var(k) },
            }
        }
    }

    /// Status and body for `GET /probe` with `Host: host` from TCP peer `peer`.
    fn probe(peer: Ipv4Addr, host: &str) -> (u16, String) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let host = host.to_string();
        rt.block_on(async move {
            let fake = SocketAddr::new(IpAddr::V4(peer), 40000);
            let app = Router::new()
                .route("/probe", get(|| async { "ok" }))
                .layer(from_fn(reject_untrusted_public_host))
                .layer(from_fn(move |mut req: Request, next: Next| async move {
                    req.extensions_mut().insert(ConnectInfo(fake));
                    next.run(req).await
                }));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            });
            let resp = reqwest::Client::new()
                .get(format!("http://{addr}/probe"))
                .header(reqwest::header::HOST, host)
                .send()
                .await
                .unwrap();
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap();
            server.abort();
            (status, body)
        })
    }

    #[test]
    fn explicit_lan_split_through_the_host_layer() {
        let _guard = crate::instance::env_lock().lock().unwrap();
        let prev = set_env();
        let ingress = Ipv4Addr::new(10, 42, 0, 17);
        // 10.x peer (ingress-nginx) + Host listed in PAPERRUST_LAN_URLS: 200.
        let (status, body) = probe(ingress, LAN);
        assert_eq!((status, body.as_str()), (200, "ok"));
        // 10.x peer + public Host (not in PAPERRUST_LAN_URLS): 400.
        let (status, body) = probe(ingress, PUBLIC);
        assert_eq!(status, 400);
        assert!(body.contains("untrusted_public_host"), "{body}");
        // 127.0.0.1 peer (tunnel sidecar) + LAN Host: served.
        let (status, body) = probe(Ipv4Addr::LOCALHOST, LAN);
        assert_eq!((status, body.as_str()), (200, "ok"));
        // 127.0.0.1 peer + public Host: served; unknown Host: 400 unlisted_host.
        assert_eq!(probe(Ipv4Addr::LOCALHOST, PUBLIC).0, 200);
        let (status, body) = probe(Ipv4Addr::LOCALHOST, "evil.com");
        assert_eq!(status, 400);
        assert!(body.contains("unlisted_host"), "{body}");
        restore(prev);
    }

    #[test]
    fn lan_url_absent_from_public_urls_refuses_startup() {
        let _guard = crate::instance::env_lock().lock().unwrap();
        let prev = set_env();
        unsafe {
            std::env::set_var(
                "PAPERRUST_LAN_URLS",
                "https://beta.archive.lan,https://other.lan",
            )
        };
        // main() runs this check before binding and stops on Err.
        let err = crate::instance::validate_issuer_config().unwrap_err();
        assert!(err.contains("PAPERRUST_LAN_URLS"), "{err}");
        assert!(err.contains("https://other.lan"), "{err}");
        restore(prev);
    }
}
