//! Paperless-ngx → Paperrust bulk import (Settings API).
//!
//!  contract:
//! - POST `/api/settings/paperless-import` → 202 `{ jobId }`
//! - GET  `/api/settings/paperless-import/:jobId`
//! - POST `/api/settings/paperless-import/:jobId/cancel` → `{ ok: true }`
//!
//! Security:
//! - Token lives only in the running job task (never DB / logs / status JSON).
//! - HTTPS only; DNS pin + `.no_proxy()`; redirects never leave the
//!   pinned host:port:scheme.
//! - **Public HTTPS** (public DNS, non-private IP after pin): allowed by default.
//!   Does **not** apply `PAPERRUST_OCR_SELFHOSTED_HOST_ALLOWLIST`.
//! - **Private / RFC1918 / `.lan` / `.loc` / `.local` / literal private IPs**:
//!   allowed only when the host is on `PAPERRUST_PAPERLESS_HOST_ALLOWLIST`
//!   (comma-separated). Prefer exact hosts. Match is exact, or a suffix only
//!   when the entry has a leading dot (`.lab.example` allows `x.lab.example`;
//!   `lab.example` does not). Single-label suffixes (`.lan`, `.loc`,
//!   `.local`) are refused at startup. Private names are classified before DNS.
//!   Lab examples: `paperless.lab.loc`, `192.0.2.10`.
//! - **Deny-wins** even if listed: loopback, link-local, cloud metadata, CGNAT
//!   (`100.64/10`), `fe80::/10`, NAT64 (`64:ff9b::/96`, `64:ff9b:1::/48`),
//!   6to4 (`2002::/16`), Teredo (`2001::/32`). `::ffff:0:0/96` follows the
//!   embedded IPv4.
//!
//! list pagination uses `{base}/api/documents/?page=N` on the pinned
//! origin only — never follows Paperless `next` (SSRF).
//! max pages/docs, bounded download (`MAX_UPLOAD_BYTES`), job TTL,
//! per-user start rate-limit (handler).
//!
//! Concurrency: **one active (queued|running) job per user**.

use crate::consume::{consume_document, ensure_named};
use crate::models::ConsumeInput;
use crate::ocr_ssrf::{
    host_matches, is_always_denied_ssrf_ip, is_blocked_hostname, is_private_lan_ip,
    is_private_suffix_host, ocr_http_client_pinned, ValidatedOcrEndpoint,
};
use crate::preview::{looks_like_jpeg, looks_like_png};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::watch;
use url::Url;
use uuid::Uuid;

const SOURCE_TAG: &str = "paperless-import";
const AI_DONE_TAG: &str = "ai-done";
const MAX_ERROR_SAMPLES: usize = 20;

/// List only this many pages on the pinned base (`page_size` below).
pub const LIST_PAGE_SIZE: u32 = 100;
pub const DEFAULT_MAX_PAGES: u32 = 50;
pub const HARD_MAX_PAGES: u32 = 100;
pub const DEFAULT_MAX_DOCUMENTS: u32 = 2_000;
pub const HARD_MAX_DOCUMENTS: u32 = 5_000;

/// drop terminal / stale jobs so the in-memory hub cannot grow forever.
const JOB_TTL_TERMINAL: Duration = Duration::from_secs(30 * 60);
const JOB_TTL_QUEUED: Duration = Duration::from_secs(15 * 60);
const JOB_TTL_RUNNING: Duration = Duration::from_secs(2 * 60 * 60);

/// Top-level job lifecycle ( `status`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum JobStatus {
    Queued,
    Running,
    Done,
    Cancelled,
    Failed,
}

impl JobStatus {
    pub fn is_active(self) -> bool {
        matches!(self, Self::Queued | Self::Running)
    }
}

/// Sub-phase while `status == running` ( `phase`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum JobPhase {
    Listing,
    Importing,
    Finalizing,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorSample {
    pub external_id: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportStatus {
    pub job_id: String,
    pub status: JobStatus,
    pub phase: Option<JobPhase>,
    pub done: u32,
    pub total: u32,
    pub errors: u32,
    pub error_samples: Vec<ErrorSample>,
    pub message: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartImportRequest {
    pub base_url: String,
    /// Never Debug/Display — held only long enough to spawn the job.
    pub token: String,
    #[serde(default = "default_true")]
    pub skip_ocr_if_present: bool,
    #[serde(default = "default_true")]
    pub preserve_tags: bool,
    #[serde(default)]
    pub dry_run: bool,
    /// Optional cap (default 2000). Hard ceiling [`HARD_MAX_DOCUMENTS`].
    #[serde(default)]
    pub max_documents: Option<u32>,
}

impl std::fmt::Debug for StartImportRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StartImportRequest")
            .field("base_url", &self.base_url)
            .field("token", &"[redacted]")
            .field("skip_ocr_if_present", &self.skip_ocr_if_present)
            .field("preserve_tags", &self.preserve_tags)
            .field("dry_run", &self.dry_run)
            .field("max_documents", &self.max_documents)
            .finish()
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartImportResponse {
    pub job_id: String,
}

#[derive(Debug, Clone)]
struct ImportOptions {
    skip_ocr_if_present: bool,
    preserve_tags: bool,
    dry_run: bool,
    max_documents: u32,
    max_pages: u32,
}

struct LiveJob {
    user_id: i32,
    status: ImportStatus,
    cancel_tx: watch::Sender<bool>,
    updated_at: Instant,
}

/// Returns `(max_documents, max_pages)` after applying defaults + hard ceilings.
fn clamp_import_limits(requested: Option<u32>) -> (u32, u32) {
    let max_docs = requested
        .unwrap_or(DEFAULT_MAX_DOCUMENTS)
        .clamp(1, HARD_MAX_DOCUMENTS);
    let needed_pages = max_docs.div_ceil(LIST_PAGE_SIZE).max(1);
    let max_pages = needed_pages.min(DEFAULT_MAX_PAGES).min(HARD_MAX_PAGES);
    (max_docs, max_pages)
}

/// In-process registry — one active job per user; lookup by jobId for GET/cancel.
#[derive(Clone, Default)]
pub struct ImportHub {
    inner: Arc<Mutex<HubInner>>,
}

#[derive(Default)]
struct HubInner {
    /// user_id → active job_id (queued|running only).
    active_by_user: HashMap<i32, String>,
    jobs: HashMap<String, LiveJob>,
}

pub async fn ensure_schema(pool: &PgPool) -> anyhow::Result<()> {
    sqlx::query("alter table documents add column if not exists paperless_id int")
        .execute(pool)
        .await?;
    sqlx::query(
        r#"create unique index if not exists documents_owner_paperless_id_uidx
           on documents (owner_id, paperless_id)
           where paperless_id is not null"#,
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Validate Paperless base URL (HTTPS + SSRF). Returns DNS-pinned endpoint.
///
/// Public internet HTTPS is allowed by default. The Paperless host allowlist is
/// an **exception for private destinations only** — it is never required for a
/// normal public Paperless.
pub fn validate_paperless_base_url(raw: &str) -> Result<ValidatedOcrEndpoint, String> {
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err("URL Paperless requise (https://…).".into());
    }
    let url = Url::parse(trimmed).map_err(|_| {
        "URL Paperless invalide — utilisez une URL https:// complète.".to_string()
    })?;
    if url.scheme() != "https" {
        return Err("L'URL Paperless doit utiliser https:// (HTTP en clair refusé).".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("URL Paperless refusée : userinfo interdit.".into());
    }

    let host = match url.host() {
        Some(url::Host::Domain(d)) => d.to_ascii_lowercase(),
        Some(url::Host::Ipv4(ip)) => ip.to_string(),
        Some(url::Host::Ipv6(ip)) => format!("[{ip}]"),
        None => return Err("URL Paperless sans hôte.".into()),
    };

    if is_blocked_hostname(&host) {
        return Err(paperless_hard_deny_msg());
    }

    let allowlisted = host_on_paperless_allowlist(&host);
    if let Some(ip) = parse_host_ip(&host) {
        if is_always_denied_ssrf_ip(ip) {
            return Err(paperless_hard_deny_msg());
        }
        if is_private_lan_ip(ip) && !allowlisted {
            return Err(paperless_private_needs_allowlist_msg(&host));
        }
    } else if is_private_suffix_host(&host) && !allowlisted {
        // `.lan` / `.loc` / `.local` before any DNS lookup.
        return Err(paperless_private_needs_allowlist_msg(&host));
    }

    pin_paperless_endpoint(&url, &host, trimmed, allowlisted)
}

fn paperless_hard_deny_msg() -> String {
    "URL Paperless refusée : loopback, link-local, métadonnées cloud ou CGNAT interdit (SSRF)."
        .into()
}

fn paperless_private_needs_allowlist_msg(host: &str) -> String {
    format!(
        "Hôte Paperless privé « {host} » refusé. Ajoutez-le à PAPERRUST_PAPERLESS_HOST_ALLOWLIST (RFC1918 / .lan / .loc)."
    )
}

fn parse_host_ip(host: &str) -> Option<IpAddr> {
    host.trim_start_matches('[').trim_end_matches(']').parse().ok()
}

fn host_on_paperless_allowlist(host: &str) -> bool {
    let Ok(raw) = std::env::var("PAPERRUST_PAPERLESS_HOST_ALLOWLIST") else {
        return false;
    };
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .any(|allowed| host_matches(host, allowed))
}

/// DNS-pin the origin. Private RFC1918/ULA addrs need an allowlisted host;
/// loopback / link-local / metadata / CGNAT / fe80 always lose (deny-wins).
fn pin_paperless_endpoint(
    url: &Url,
    host: &str,
    trimmed: &str,
    allowlisted: bool,
) -> Result<ValidatedOcrEndpoint, String> {
    let port = url.port_or_known_default().unwrap_or(443);
    let lookup = host.trim_start_matches('[').trim_end_matches(']');
    let addrs: Vec<SocketAddr> = (lookup, port)
        .to_socket_addrs()
        .map_err(|_| format!("Impossible de résoudre l'hôte Paperless « {host} »."))?
        .collect();
    if addrs.is_empty() {
        return Err(format!("Aucune adresse DNS pour l'hôte Paperless « {host} »."));
    }
    for addr in &addrs {
        if is_always_denied_ssrf_ip(addr.ip()) {
            return Err(paperless_hard_deny_msg());
        }
        if is_private_lan_ip(addr.ip()) && !allowlisted {
            return Err(paperless_private_needs_allowlist_msg(&host));
        }
    }
    Ok(ValidatedOcrEndpoint {
        url: trimmed.to_string(),
        resolve_host: lookup.to_string(),
        addrs,
    })
}

/// Strip Paperless tokens / Authorization echoes from anything user-visible or logged.
/// Front toasts `error` literally — never leak PL credentials there.
/// Public scrubber for HTTP `error` bodies (no token in scope at the handler).
pub fn sanitize_for_client(msg: &str) -> String {
    sanitize_public_error(msg, None)
}

fn sanitize_public_error(msg: &str, token: Option<&str>) -> String {
    use std::sync::OnceLock;
    static AUTH_HEADER: OnceLock<regex::Regex> = OnceLock::new();
    static TOKEN_SCHEME: OnceLock<regex::Regex> = OnceLock::new();
    static BEARER_SCHEME: OnceLock<regex::Regex> = OnceLock::new();

    let mut out = msg.to_string();
    if let Some(t) = token.map(str::trim).filter(|t| !t.is_empty()) {
        out = out.replace(t, "[redacted]");
    }

    let token_scheme = TOKEN_SCHEME.get_or_init(|| {
        regex::Regex::new(r"(?i)\btoken\s+[A-Za-z0-9._\-+=/]{6,}").expect("token scheme regex")
    });
    let bearer = BEARER_SCHEME.get_or_init(|| {
        regex::Regex::new(r"(?i)\bbearer\s+[A-Za-z0-9._\-+=/]{6,}").expect("bearer scheme regex")
    });
    // Full header value (not just first word after `:`) — PL errors may echo `Token <secret>`.
    let auth = AUTH_HEADER.get_or_init(|| {
        regex::Regex::new(r"(?i)authorization\s*[:=]\s*[^\r\n]+").expect("auth header regex")
    });

    // Scheme patterns first so `Token <secret>` is gone before Authorization rewrite.
    out = token_scheme.replace_all(&out, "Token [redacted]").into_owned();
    out = bearer.replace_all(&out, "Bearer [redacted]").into_owned();
    out = auth.replace_all(&out, "Authorization: [redacted]").into_owned();
    // Defense in depth: shared OCR log scrubber (sk-/xai-/api_key=…).
    crate::ocr::redact_for_log(&out)
}

fn sanitize_status(status: &ImportStatus) -> ImportStatus {
    let mut out = status.clone();
    out.message = sanitize_public_error(&out.message, None);
    for sample in &mut out.error_samples {
        sample.message = sanitize_public_error(&sample.message, None);
    }
    out
}

impl ImportHub {
    pub fn start(
        &self,
        pool: PgPool,
        user_id: i32,
        req: StartImportRequest,
    ) -> Result<StartImportResponse, (axum::http::StatusCode, String)> {
        let token = req.token.trim().to_string();
        if token.is_empty() {
            return Err((
                axum::http::StatusCode::BAD_REQUEST,
                "Jeton Paperless requis.".into(),
            ));
        }
        let endpoint = validate_paperless_base_url(&req.base_url)
            .map_err(|e| (axum::http::StatusCode::BAD_REQUEST, sanitize_public_error(&e, Some(&token))))?;

        let job_id = Uuid::new_v4().to_string();
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let status = ImportStatus {
            job_id: job_id.clone(),
            status: JobStatus::Queued,
            phase: None,
            done: 0,
            total: 0,
            errors: 0,
            error_samples: Vec::new(),
            message: String::new(),
        };
        let (max_documents, max_pages) = clamp_import_limits(req.max_documents);
        let opts = ImportOptions {
            skip_ocr_if_present: req.skip_ocr_if_present,
            preserve_tags: req.preserve_tags,
            dry_run: req.dry_run,
            max_documents,
            max_pages,
        };

        {
            let mut inner = self.inner.lock().expect("paperless import hub");
            evict_expired(&mut inner);
            if let Some(existing_id) = inner.active_by_user.get(&user_id) {
                if let Some(existing) = inner.jobs.get(existing_id) {
                    if existing.status.status.is_active() {
                        return Err((
                            axum::http::StatusCode::CONFLICT,
                            "Un import Paperless est déjà en cours pour cet utilisateur.".into(),
                        ));
                    }
                }
            }
            inner.active_by_user.insert(user_id, job_id.clone());
            inner.jobs.insert(
                job_id.clone(),
                LiveJob {
                    user_id,
                    status: status.clone(),
                    cancel_tx,
                    updated_at: Instant::now(),
                },
            );
        }

        let hub = self.clone();
        let jid = job_id.clone();
        tokio::spawn(async move {
            run_job(hub, pool, user_id, jid, endpoint, token, opts, cancel_rx).await;
        });

        Ok(StartImportResponse { job_id })
    }

    pub fn status_for(&self, user_id: i32, job_id: &str) -> Option<ImportStatus> {
        let mut inner = self.inner.lock().expect("paperless import hub");
        evict_expired(&mut inner);
        let job = inner.jobs.get(job_id)?;
        if job.user_id != user_id {
            return None;
        }
        Some(sanitize_status(&job.status))
    }

    /// Best-effort cancel. Returns ok even if already terminal / unknown for this user.
    pub fn cancel(&self, user_id: i32, job_id: &str) -> bool {
        let mut inner = self.inner.lock().expect("paperless import hub");
        evict_expired(&mut inner);
        let Some(job) = inner.jobs.get_mut(job_id) else {
            return true;
        };
        if job.user_id != user_id {
            return true;
        }
        if job.status.status.is_active() {
            let _ = job.cancel_tx.send(true);
            job.status.status = JobStatus::Cancelled;
            job.status.phase = None;
            job.status.message = "cancelled".into();
            if inner.active_by_user.get(&user_id).map(|s| s.as_str()) == Some(job_id) {
                inner.active_by_user.remove(&user_id);
            }
        }
        true
    }

    fn patch<F: FnOnce(&mut ImportStatus)>(&self, job_id: &str, f: F) {
        let mut inner = self.inner.lock().expect("paperless import hub");
        let clear_user = if let Some(job) = inner.jobs.get_mut(job_id) {
            f(&mut job.status);
            job.updated_at = Instant::now();
            if !job.status.status.is_active() {
                Some(job.user_id)
            } else {
                None
            }
        } else {
            None
        };
        if let Some(uid) = clear_user {
            if inner.active_by_user.get(&uid).map(|s| s.as_str()) == Some(job_id) {
                inner.active_by_user.remove(&uid);
            }
        }
    }

    fn push_error(&self, job_id: &str, external_id: &str, message: String) {
        let message = sanitize_public_error(&message, None);
        self.patch(job_id, |s| {
            s.errors = s.errors.saturating_add(1);
            if s.error_samples.len() < MAX_ERROR_SAMPLES {
                s.error_samples.push(ErrorSample {
                    external_id: external_id.to_string(),
                    message,
                });
            }
        });
    }
}

fn job_ttl(status: JobStatus) -> Duration {
    match status {
        JobStatus::Queued => JOB_TTL_QUEUED,
        JobStatus::Running => JOB_TTL_RUNNING,
        JobStatus::Done | JobStatus::Cancelled | JobStatus::Failed => JOB_TTL_TERMINAL,
    }
}

fn evict_expired(inner: &mut HubInner) {
    let now = Instant::now();
    let drop_ids: Vec<String> = inner
        .jobs
        .iter()
        .filter(|(_, job)| now.saturating_duration_since(job.updated_at) > job_ttl(job.status.status))
        .map(|(id, _)| id.clone())
        .collect();
    for id in drop_ids {
        if let Some(job) = inner.jobs.remove(&id) {
            if inner.active_by_user.get(&job.user_id).map(|s| s.as_str()) == Some(id.as_str()) {
                inner.active_by_user.remove(&job.user_id);
            }
        }
    }
}

async fn run_job(
    hub: ImportHub,
    pool: PgPool,
    user_id: i32,
    job_id: String,
    endpoint: ValidatedOcrEndpoint,
    token: String,
    opts: ImportOptions,
    cancel_rx: watch::Receiver<bool>,
) {
    let outcome = job_body(
        &hub,
        &pool,
        user_id,
        &job_id,
        &endpoint,
        &token,
        &opts,
        cancel_rx,
    )
    .await;

    drop(token);

    if let Err(e) = outcome {
        let safe = sanitize_public_error(&e, None);
        hub.patch(&job_id, |s| {
            if s.status != JobStatus::Cancelled {
                s.status = JobStatus::Failed;
                s.phase = None;
                s.message = safe;
            }
        });
    }
}

async fn job_body(
    hub: &ImportHub,
    pool: &PgPool,
    user_id: i32,
    job_id: &str,
    endpoint: &ValidatedOcrEndpoint,
    token: &str,
    opts: &ImportOptions,
    mut cancel_rx: watch::Receiver<bool>,
) -> Result<(), String> {
    let client = ocr_http_client_pinned(&endpoint.resolve_host, &endpoint.addrs)
        .map_err(|e| sanitize_public_error(&e.to_string(), Some(token)))?;

    hub.patch(job_id, |s| {
        s.status = JobStatus::Running;
        s.phase = Some(JobPhase::Listing);
        s.message = "listing".into();
    });

    let docs = list_all_documents(
        &client,
        &endpoint.url,
        token,
        &mut cancel_rx,
        opts.max_pages,
        opts.max_documents,
    )
    .await
    .map_err(|e| sanitize_public_error(&e, Some(token)))?;
    if *cancel_rx.borrow() {
        hub.patch(job_id, |s| {
            s.status = JobStatus::Cancelled;
            s.phase = None;
            s.message = "cancelled".into();
        });
        return Ok(());
    }

    hub.patch(job_id, |s| {
        s.total = docs.len() as u32;
        s.phase = Some(JobPhase::Importing);
        s.message = if opts.dry_run {
            "dry run".into()
        } else {
            "importing".into()
        };
    });

    if opts.dry_run {
        hub.patch(job_id, |s| {
            s.phase = Some(JobPhase::Finalizing);
            s.message = format!("dry run: {} document(s)", docs.len());
            s.status = JobStatus::Done;
            s.phase = None;
        });
        return Ok(());
    }

    let mut tag_names: HashMap<i32, String> = HashMap::new();
    let mut corr_names: HashMap<i32, String> = HashMap::new();
    let mut type_names: HashMap<i32, String> = HashMap::new();

    for doc in docs {
        if *cancel_rx.borrow() {
            hub.patch(job_id, |s| {
                s.status = JobStatus::Cancelled;
                s.phase = None;
                s.message = "cancelled".into();
            });
            return Ok(());
        }

        match import_one(
            pool,
            &client,
            &endpoint.url,
            token,
            user_id,
            &doc,
            opts,
            &mut tag_names,
            &mut corr_names,
            &mut type_names,
        )
        .await
        {
            Ok(ImportOneOutcome::Imported) => {
                hub.patch(job_id, |s| s.done = s.done.saturating_add(1));
            }
            Ok(ImportOneOutcome::Skipped) => {
                // Already imported — count as done progress without error.
                hub.patch(job_id, |s| s.done = s.done.saturating_add(1));
            }
            Err(e) => {
                let msg = sanitize_public_error(&e, Some(token));
                tracing::warn!(
                    job_id = %job_id,
                    paperless_id = doc.id,
                    error = %msg,
                    "paperless import doc failed"
                );
                hub.push_error(job_id, &doc.id.to_string(), msg);
            }
        }
    }

    if *cancel_rx.borrow() {
        hub.patch(job_id, |s| {
            s.status = JobStatus::Cancelled;
            s.phase = None;
            s.message = "cancelled".into();
        });
        return Ok(());
    }

    hub.patch(job_id, |s| {
        s.phase = Some(JobPhase::Finalizing);
        s.message = "finalizing".into();
    });

    hub.patch(job_id, |s| {
        s.status = JobStatus::Done;
        s.phase = None;
        s.message = if s.errors > 0 {
            format!("done with {} error(s)", s.errors)
        } else {
            "done".into()
        };
    });
    Ok(())
}

#[derive(Debug)]
enum ImportOneOutcome {
    Imported,
    Skipped,
}

#[derive(Debug, Deserialize)]
struct PlListResponse {
    results: Vec<PlDoc>,
}

#[derive(Debug, Deserialize)]
struct PlDoc {
    id: i32,
    title: Option<String>,
    content: Option<String>,
    created: Option<String>,
    created_date: Option<String>,
    original_file_name: Option<String>,
    archived_file_name: Option<String>,
    mime_type: Option<String>,
    correspondent: Option<i32>,
    document_type: Option<i32>,
    #[serde(default)]
    tags: Vec<i32>,
}

/// always `{pinned_base}/api/documents/?page=N` — never an absolute `next`.
fn list_page_url(base: &str, page: u32) -> String {
    format!("{base}/api/documents/?page={page}&page_size={LIST_PAGE_SIZE}&ordering=id")
}

async fn list_all_documents(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    cancel_rx: &mut watch::Receiver<bool>,
    max_pages: u32,
    max_documents: u32,
) -> Result<Vec<PlDoc>, String> {
    let mut out = Vec::new();
    let max_pages = max_pages.clamp(1, HARD_MAX_PAGES);
    let max_documents = max_documents.clamp(1, HARD_MAX_DOCUMENTS);
    for page in 1..=max_pages {
        if *cancel_rx.borrow() {
            break;
        }
        if out.len() as u32 >= max_documents {
            break;
        }
        let url = list_page_url(base, page);
        let resp = client
            .get(&url)
            .header("authorization", format!("Token {token}"))
            .header("accept", "application/json")
            .send()
            .await
            .map_err(|e| format!("Paperless list HTTP: {e}"))?;
        if let Some(len) = resp.content_length() {
            if len > crate::limits::DEFAULT_BODY_LIMIT_BYTES as u64 * 4 {
                return Err("liste Paperless trop volumineuse".into());
            }
        }
        if !resp.status().is_success() {
            return Err(format!("Paperless list HTTP {}", resp.status().as_u16()));
        }
        let page_body: PlListResponse = resp
            .json()
            .await
            .map_err(|e| format!("Paperless list JSON: {e}"))?;
        let n = page_body.results.len() as u32;
        out.extend(page_body.results);
        if out.len() as u32 > max_documents {
            out.truncate(max_documents as usize);
            break;
        }
        if n < LIST_PAGE_SIZE {
            break;
        }
    }
    Ok(out)
}

async fn import_one(
    pool: &PgPool,
    client: &reqwest::Client,
    base: &str,
    token: &str,
    owner_id: i32,
    doc: &PlDoc,
    opts: &ImportOptions,
    tag_names: &mut HashMap<i32, String>,
    corr_names: &mut HashMap<i32, String>,
    type_names: &mut HashMap<i32, String>,
) -> Result<ImportOneOutcome, String> {
    let existing: Option<i32> = sqlx::query_scalar(
        "select id from documents where owner_id = $1 and paperless_id = $2",
    )
    .bind(owner_id)
    .bind(doc.id)
    .fetch_optional(pool)
    .await
    .map_err(|e| e.to_string())?;
    if existing.is_some() {
        return Ok(ImportOneOutcome::Skipped);
    }

    let bytes = download_original(client, base, token, doc.id).await?;
    if bytes.is_empty() {
        return Err("fichier original vide".into());
    }

    let filename = doc
        .original_file_name
        .clone()
        .or_else(|| doc.archived_file_name.clone())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| format!("paperless-{}.bin", doc.id));
    let mime = doc
        .mime_type
        .clone()
        .unwrap_or_else(|| sniff_mime(&bytes, &filename));

    let allowed =
        bytes.starts_with(b"%PDF") || looks_like_png(&bytes) || looks_like_jpeg(&bytes);
    if !allowed {
        return Err(format!("type non supporté ({mime})"));
    }

    let pl_content = doc.content.as_deref().unwrap_or("").trim();
    let has_ocr = opts.skip_ocr_if_present && !pl_content.is_empty();

    let created = doc
        .created_date
        .clone()
        .or_else(|| {
            doc.created
                .as_ref()
                .map(|s| s.chars().take(10).collect::<String>())
        })
        .filter(|s| s.len() == 10);

    let title = doc
        .title
        .clone()
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| filename.clone());

    let input = ConsumeInput {
        filename: filename.clone(),
        mime_type: mime,
        data_base64: String::new(),
        raw_bytes: Some(bytes),
        provider: "xai".into(),
        api_key: None,
        skip_ocr: Some(true),
        base_url: None,
        title_override: Some(title.clone()),
        created_override: created,
    };

    let result = consume_document(pool, input, owner_id)
        .await
        .map_err(|e| sanitize_public_error(&e.to_string(), Some(token)))?;
    let document_id = result.document_id;

    let mut tag_labels: Vec<String> = Vec::new();
    if opts.preserve_tags {
        for tid in &doc.tags {
            if let Some(n) = resolve_named(client, base, token, "tags", *tid, tag_names).await? {
                tag_labels.push(n);
            }
        }
    }
    if !tag_labels
        .iter()
        .any(|t| t.eq_ignore_ascii_case(SOURCE_TAG))
    {
        tag_labels.push(SOURCE_TAG.into());
    }
    if has_ocr
        && !tag_labels
            .iter()
            .any(|t| t.eq_ignore_ascii_case(AI_DONE_TAG))
    {
        tag_labels.push(AI_DONE_TAG.into());
    }

    let corr_name = match doc.correspondent {
        Some(id) => resolve_named(client, base, token, "correspondents", id, corr_names).await?,
        None => None,
    };
    let type_name = match doc.document_type {
        Some(id) => resolve_named(client, base, token, "document_types", id, type_names).await?,
        None => None,
    };

    let corr_id = match corr_name {
        Some(ref n) => Some(
            ensure_named(pool, "correspondents", n, None, owner_id)
                .await
                .map_err(|e| e.to_string())?,
        ),
        None => None,
    };
    let type_id = match type_name {
        Some(ref n) => Some(
            ensure_named(pool, "document_types", n, None, owner_id)
                .await
                .map_err(|e| e.to_string())?,
        ),
        None => None,
    };

    let ocr_status = if has_ocr { "done" } else { "pending" };
    let ocr_provider: Option<&str> = if has_ocr {
        Some("paperless-import")
    } else {
        None
    };
    let content = if has_ocr {
        pl_content.to_string()
    } else {
        String::new()
    };
    let summary = if has_ocr {
        "Importé depuis Paperless (OCR conservé).".to_string()
    } else {
        "Importé depuis Paperless — OCR Paperrust non lancé.".to_string()
    };

    sqlx::query(
        r#"update documents set
             paperless_id = $1,
             title = $2,
             content = $3,
             ocr_status = $4,
             ocr_provider = $5,
             summary = $6,
             correspondent_id = coalesce($7, correspondent_id),
             document_type_id = coalesce($8, document_type_id),
             modified_at = now()
           where id = $9 and owner_id = $10"#,
    )
    .bind(doc.id)
    .bind(&title)
    .bind(&content)
    .bind(ocr_status)
    .bind(ocr_provider)
    .bind(&summary)
    .bind(corr_id)
    .bind(type_id)
    .bind(document_id)
    .bind(owner_id)
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    for tag in tag_labels {
        let tid = ensure_named(pool, "tags", &tag, None, owner_id)
            .await
            .map_err(|e| e.to_string())?;
        sqlx::query(
            "insert into document_tags (document_id, tag_id) values ($1,$2) on conflict do nothing",
        )
        .bind(document_id)
        .bind(tid)
        .execute(pool)
        .await
        .map_err(|e| e.to_string())?;
    }

    Ok(ImportOneOutcome::Imported)
}

async fn download_original(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    id: i32,
) -> Result<Vec<u8>, String> {
    let url = format!("{base}/api/documents/{id}/download/");
    let resp = client
        .get(&url)
        .header("authorization", format!("Token {token}"))
        .send()
        .await
        .map_err(|e| format!("download HTTP: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("download HTTP {}", resp.status().as_u16()));
    }
    read_body_capped(resp, crate::limits::MAX_UPLOAD_BYTES).await
}

/// Content-Length gate + incremental read, never unbounded `bytes()`.
async fn read_body_capped(mut resp: reqwest::Response, max: usize) -> Result<Vec<u8>, String> {
    if let Some(len) = resp.content_length() {
        if len > max as u64 {
            return Err(format!(
                "fichier trop volumineux (max {} Mo)",
                max / (1024 * 1024)
            ));
        }
    }
    let mut out = Vec::new();
    if let Some(len) = resp.content_length() {
        out.reserve(len as usize);
    }
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| format!("download body: {e}"))?
    {
        if out.len().saturating_add(chunk.len()) > max {
            return Err(format!(
                "fichier trop volumineux (max {} Mo)",
                max / (1024 * 1024)
            ));
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

async fn resolve_named(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    kind: &str,
    id: i32,
    cache: &mut HashMap<i32, String>,
) -> Result<Option<String>, String> {
    if let Some(n) = cache.get(&id) {
        return Ok(Some(n.clone()));
    }
    let url = format!("{base}/api/{kind}/{id}/");
    let resp = client
        .get(&url)
        .header("authorization", format!("Token {token}"))
        .header("accept", "application/json")
        .send()
        .await
        .map_err(|e| format!("{kind} HTTP: {e}"))?;
    if resp.status().as_u16() == 404 {
        return Ok(None);
    }
    if !resp.status().is_success() {
        return Err(format!("{kind} HTTP {}", resp.status().as_u16()));
    }
    let v: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("{kind} JSON: {e}"))?;
    let name = v
        .get("name")
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    if let Some(ref n) = name {
        cache.insert(id, n.clone());
    }
    Ok(name)
}

fn sniff_mime(bytes: &[u8], filename: &str) -> String {
    if bytes.starts_with(b"%PDF") {
        return "application/pdf".into();
    }
    if looks_like_png(bytes) {
        return "image/png".into();
    }
    if looks_like_jpeg(bytes) {
        return "image/jpeg".into();
    }
    let lower = filename.to_ascii_lowercase();
    if lower.ends_with(".pdf") {
        "application/pdf".into()
    } else if lower.ends_with(".png") {
        "image/png".into()
    } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        "image/jpeg".into()
    } else {
        "application/octet-stream".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn with_allowlists<R>(
        paperless: Option<&str>,
        ocr: Option<&str>,
        f: impl FnOnce() -> R,
    ) -> R {
        let _guard = env_lock().lock().unwrap();
        let prev_pl = std::env::var("PAPERRUST_PAPERLESS_HOST_ALLOWLIST").ok();
        let prev_ocr = std::env::var("PAPERRUST_OCR_SELFHOSTED_HOST_ALLOWLIST").ok();
        // SAFETY: serialized via env_lock; restored before unlock.
        unsafe {
            match paperless {
                Some(v) => std::env::set_var("PAPERRUST_PAPERLESS_HOST_ALLOWLIST", v),
                None => std::env::remove_var("PAPERRUST_PAPERLESS_HOST_ALLOWLIST"),
            }
            match ocr {
                Some(v) => std::env::set_var("PAPERRUST_OCR_SELFHOSTED_HOST_ALLOWLIST", v),
                None => std::env::remove_var("PAPERRUST_OCR_SELFHOSTED_HOST_ALLOWLIST"),
            }
        }
        let out = f();
        unsafe {
            match prev_pl {
                Some(v) => std::env::set_var("PAPERRUST_PAPERLESS_HOST_ALLOWLIST", v),
                None => std::env::remove_var("PAPERRUST_PAPERLESS_HOST_ALLOWLIST"),
            }
            match prev_ocr {
                Some(v) => std::env::set_var("PAPERRUST_OCR_SELFHOSTED_HOST_ALLOWLIST", v),
                None => std::env::remove_var("PAPERRUST_OCR_SELFHOSTED_HOST_ALLOWLIST"),
            }
        }
        out
    }

    #[test]
    fn rejects_http_and_private_by_default() {
        with_allowlists(None, None, || {
            assert!(validate_paperless_base_url("http://paperless.example/").is_err());
            assert!(validate_paperless_base_url("https://127.0.0.1/").is_err());
            assert!(validate_paperless_base_url("https://192.168.1.5/").is_err());
            assert!(validate_paperless_base_url("https://10.0.0.8/").is_err());
            let loc = validate_paperless_base_url("https://paperless.lab.loc/").unwrap_err();
            assert!(
                loc.contains("PAPERRUST_PAPERLESS_HOST_ALLOWLIST"),
                "{loc}"
            );
            let lan = validate_paperless_base_url("https://paperless.lan/").unwrap_err();
            assert!(
                lan.contains("PAPERRUST_PAPERLESS_HOST_ALLOWLIST"),
                "{lan}"
            );
            let lan = validate_paperless_base_url("https://192.168.50.10/").unwrap_err();
            assert!(
                lan.contains("PAPERRUST_PAPERLESS_HOST_ALLOWLIST"),
                "{lan}"
            );
        });
    }

    #[test]
    fn public_https_ok_without_any_allowlist() {
        with_allowlists(None, None, || {
            let ep = validate_paperless_base_url("https://1.1.1.1/").expect("public ok");
            assert_eq!(ep.resolve_host, "1.1.1.1");
            assert!(!ep.addrs.is_empty());
        });
    }

    #[test]
    fn public_https_ignores_ocr_and_paperless_allowlists() {
        // OCR allowlist must not gate public Paperless; Paperless allowlist is
        // only an exception for private destinations.
        with_allowlists(Some("paperless.lab.loc"), Some("llm.only.example"), || {
            let ep = validate_paperless_base_url("https://1.1.1.1/").expect("public still ok");
            assert_eq!(ep.resolve_host, "1.1.1.1");
        });
    }

    #[test]
    fn private_ok_when_paperless_allowlisted() {
        with_allowlists(Some("192.168.50.10,paperless.lab.loc"), None, || {
            let ep = validate_paperless_base_url("https://192.168.50.10/").expect("LAN allowlisted");
            assert_eq!(ep.resolve_host, "192.168.50.10");
            assert!(!ep.addrs.is_empty());

            // .loc is classified private before DNS; allowlist lets it through
            // to pin (success if resolvable, otherwise a resolve error — not
            // the "add to allowlist" rejection).
            match validate_paperless_base_url("https://paperless.lab.loc/") {
                Ok(ep) => assert_eq!(ep.resolve_host, "paperless.lab.loc"),
                Err(e) => {
                    assert!(
                        !e.contains("PAPERRUST_PAPERLESS_HOST_ALLOWLIST"),
                        "allowlisted .loc must not be policy-rejected: {e}"
                    );
                    assert!(
                        e.contains("résoudre") || e.contains("adresse DNS"),
                        "{e}"
                    );
                }
            }
        });
    }

    #[test]
    fn deny_wins_for_metadata_link_local_even_if_listed() {
        with_allowlists(
            Some("169.254.169.254,127.0.0.1,100.64.1.1,fe80::1,[fe80::1]"),
            None,
            || {
                for url in [
                    "https://169.254.169.254/",
                    "https://127.0.0.1/",
                    "https://100.64.1.1/",
                    "https://[fe80::1]/",
                ] {
                    let err = validate_paperless_base_url(url).expect_err(url);
                    assert!(
                        err.contains("SSRF") || err.contains("interdit"),
                        "{url}: {err}"
                    );
                    assert!(
                        !err.contains("PAPERRUST_PAPERLESS_HOST_ALLOWLIST"),
                        "hard-deny must win over allowlist: {url}: {err}"
                    );
                }
            },
        );
    }

    #[test]
    fn suffix_allowlist_requires_leading_dot() {
        with_allowlists(Some(".lan"), None, || {
            assert!(host_on_paperless_allowlist("x.lan"));
            assert!(host_on_paperless_allowlist("lan"));
            assert!(!host_on_paperless_allowlist("evillan"));
            assert!(!host_on_paperless_allowlist("lan.evil.com"));
        });
        with_allowlists(Some("lan"), None, || {
            assert!(host_on_paperless_allowlist("lan"));
            assert!(!host_on_paperless_allowlist("x.lan"));
            assert!(!host_on_paperless_allowlist("evillan"));
            assert!(!host_on_paperless_allowlist("lan.evil.com"));
        });
    }

    #[test]
    fn private_suffix_rejected_before_dns() {
        with_allowlists(None, None, || {
            for host in ["x.lan", "paperless.lan", "a.b.loc", "printer.local"] {
                let err = validate_paperless_base_url(&format!("https://{host}/")).unwrap_err();
                assert!(
                    err.contains("PAPERRUST_PAPERLESS_HOST_ALLOWLIST"),
                    "{host}: {err}"
                );
                assert!(
                    !err.contains("résoudre") && !err.contains("adresse DNS"),
                    "{host}: {err}"
                );
            }
        });
    }

    #[test]
    fn listed_x_lan_is_not_policy_rejected() {
        with_allowlists(Some(".lan"), None, || {
            match validate_paperless_base_url("https://x.lan/") {
                Ok(ep) => assert_eq!(ep.resolve_host, "x.lan"),
                Err(e) => {
                    assert!(
                        !e.contains("PAPERRUST_PAPERLESS_HOST_ALLOWLIST"),
                        "listed x.lan must reach DNS: {e}"
                    );
                    assert!(e.contains("résoudre") || e.contains("adresse DNS"), "{e}");
                }
            }
            for host in ["evillan", "lan.evil.com"] {
                // Not a private suffix and not listed: public-host path may DNS.
                // The allowlist itself must not treat them as `.lan`.
                assert!(
                    !host_on_paperless_allowlist(host),
                    "{host} must not match .lan"
                );
            }
        });
    }

    #[test]
    fn refuses_transition_and_mapped_ipv6_before_dns() {
        with_allowlists(
            Some("64:ff9b::1,2002:0808:0808::,2001:0::1,::ffff:127.0.0.1,::ffff:10.0.0.1"),
            None,
            || {
                for url in [
                    "https://[64:ff9b::1]/",
                    "https://[64:ff9b:1::1]/",
                    "https://[2002:0808:0808::]/",
                    "https://[2001:0::1]/",
                    "https://[::ffff:127.0.0.1]/",
                ] {
                    let err = validate_paperless_base_url(url).unwrap_err();
                    assert!(
                        err.contains("SSRF") || err.contains("interdit"),
                        "{url}: {err}"
                    );
                    assert!(
                        !err.contains("PAPERRUST_PAPERLESS_HOST_ALLOWLIST"),
                        "hard-deny wins: {url}: {err}"
                    );
                    assert!(!err.contains("résoudre"), "{url}: {err}");
                }
                let mapped_private = validate_paperless_base_url("https://[::ffff:10.0.0.1]/")
                    .expect("allowlisted embedded RFC1918");
                assert!(!mapped_private.addrs.is_empty());
            },
        );
        with_allowlists(None, None, || {
            let err = validate_paperless_base_url("https://[::ffff:10.0.0.1]/").unwrap_err();
            assert!(
                err.contains("PAPERRUST_PAPERLESS_HOST_ALLOWLIST"),
                "{err}"
            );
            assert!(!err.contains("résoudre"), "{err}");
            let public = validate_paperless_base_url("https://[::ffff:1.1.1.1]/")
                .expect("mapped public v4");
            assert!(
                public.resolve_host == "::ffff:1.1.1.1" || public.resolve_host == "1.1.1.1",
                "{}",
                public.resolve_host
            );
        });
    }

    #[test]
    fn sanitize_redacts_token_and_authorization_echoes() {
        let token = "pltok_SUPER_SECRET_VALUE_9f3a";
        let raw = format!(
            "upstream said Authorization: Token {token} rejected; also Token {token} and Bearer {token}"
        );
        let clean = sanitize_public_error(&raw, Some(token));
        assert!(!clean.contains(token), "{clean}");
        assert!(!clean.to_ascii_lowercase().contains("pltok_"), "{clean}");
        assert!(clean.contains("[redacted]"), "{clean}");

        // Pattern scrub without knowing the secret (Authorization header echo).
        let echoed = "HTTP 401: Authorization: Token abcdefghijklmnop dead";
        let clean2 = sanitize_public_error(echoed, None);
        assert!(!clean2.contains("abcdefghijklmnop"), "{clean2}");
        assert!(clean2.contains("[redacted]"), "{clean2}");
    }

    #[test]
    fn status_for_sanitizes_message_and_samples() {
        let hub = ImportHub::default();
        {
            let mut inner = hub.inner.lock().unwrap();
            let (tx, _rx) = watch::channel(false);
            let token = "leakme_TOKEN_VALUE_ZZ99";
            inner.jobs.insert(
                "j-leak".into(),
                LiveJob {
                    user_id: 3,
                    status: ImportStatus {
                        job_id: "j-leak".into(),
                        status: JobStatus::Failed,
                        phase: None,
                        done: 0,
                        total: 1,
                        errors: 1,
                        error_samples: vec![ErrorSample {
                            external_id: "9".into(),
                            message: format!("Authorization: Token {token} from PL"),
                        }],
                        message: format!("failed: Token {token}"),
                    },
                    cancel_tx: tx,
                    updated_at: Instant::now(),
                },
            );
        }
        let st = hub.status_for(3, "j-leak").expect("status");
        assert!(!st.message.contains("leakme_TOKEN_VALUE_ZZ99"), "{}", st.message);
        assert!(
            !st.error_samples[0].message.contains("leakme_TOKEN_VALUE_ZZ99"),
            "{}",
            st.error_samples[0].message
        );
        assert!(st.message.contains("[redacted]"), "{}", st.message);
    }

    #[test]
    fn status_json_matches_import_shape() {
        let s = ImportStatus {
            job_id: "abc".into(),
            status: JobStatus::Running,
            phase: Some(JobPhase::Importing),
            done: 3,
            total: 10,
            errors: 1,
            error_samples: vec![ErrorSample {
                external_id: "42".into(),
                message: "boom".into(),
            }],
            message: "importing".into(),
        };
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["jobId"], "abc");
        assert_eq!(v["status"], "running");
        assert_eq!(v["phase"], "importing");
        assert_eq!(v["done"], 3);
        assert_eq!(v["total"], 10);
        assert_eq!(v["errors"], 1);
        assert_eq!(v["errorSamples"][0]["externalId"], "42");
        assert_eq!(v["message"], "importing");
        assert!(v.get("token").is_none());
        assert!(v.get("url").is_none());
        assert!(v.get("baseUrl").is_none());
    }

    #[test]
    fn start_request_deserializes_import_body() {
        let raw = r#"{
            "baseUrl": "https://1.1.1.1",
            "token": "tok",
            "skipOcrIfPresent": false,
            "preserveTags": false,
            "dryRun": true
        }"#;
        let req: StartImportRequest = serde_json::from_str(raw).unwrap();
        assert_eq!(req.base_url, "https://1.1.1.1");
        assert!(!req.skip_ocr_if_present);
        assert!(!req.preserve_tags);
        assert!(req.dry_run);
    }

    #[test]
    fn start_request_defaults() {
        let raw = r#"{"baseUrl":"https://1.1.1.1","token":"t"}"#;
        let req: StartImportRequest = serde_json::from_str(raw).unwrap();
        assert!(req.skip_ocr_if_present);
        assert!(req.preserve_tags);
        assert!(!req.dry_run);
    }

    #[test]
    fn cancel_best_effort_and_conflict_predicate() {
        let hub = ImportHub::default();
        {
            let mut inner = hub.inner.lock().unwrap();
            let (tx, _rx) = watch::channel(false);
            inner.active_by_user.insert(7, "j1".into());
            inner.jobs.insert(
                "j1".into(),
                LiveJob {
                    user_id: 7,
                    status: ImportStatus {
                        job_id: "j1".into(),
                        status: JobStatus::Running,
                        phase: Some(JobPhase::Listing),
                        done: 0,
                        total: 0,
                        errors: 0,
                        error_samples: Vec::new(),
                        message: String::new(),
                    },
                    cancel_tx: tx,
                    updated_at: Instant::now(),
                },
            );
        }
        assert!(JobStatus::Queued.is_active());
        assert!(JobStatus::Running.is_active());
        assert!(!JobStatus::Done.is_active());
        assert!(hub.cancel(7, "j1"));
        let st = hub.status_for(7, "j1").unwrap();
        assert_eq!(st.status, JobStatus::Cancelled);
        // Unknown job still ok.
        assert!(hub.cancel(7, "missing"));
    }

    #[test]
    fn rejects_userinfo() {
        assert!(validate_paperless_base_url("https://u:p@1.1.1.1/").is_err());
        assert!(validate_paperless_base_url("https://alice@1.1.1.1/").is_err());
    }

    #[test]
    fn list_page_stays_on_pinned_base() {
        let u = list_page_url("https://1.1.1.1", 3);
        assert_eq!(
            u,
            "https://1.1.1.1/api/documents/?page=3&page_size=100&ordering=id"
        );
        assert!(!u.contains("169.254"));
        assert!(!u.contains("next"));
    }

    #[test]
    fn allowlist_is_exact_unless_leading_dot() {
        assert!(host_matches("paperless.lan.loc", "paperless.lan.loc"));
        assert!(!host_matches(
            "evil.paperless.lan.loc",
            "paperless.lan.loc"
        ));
        assert!(!host_matches(
            "paperless.lan.loc.evil.com",
            "paperless.lan.loc"
        ));
        assert!(host_matches("paperless.lan", "paperless.lan"));
        assert!(!host_matches("evillan", "lan"));
        assert!(!host_matches("evillan.com", "lan"));
        assert!(!host_matches("evilpaperless.lan", "paperless.lan"));
        assert!(!host_matches("paperless.lan.evil.com", "paperless.lan"));
        assert!(host_matches("x.lan", ".lan"));
        assert!(!host_matches("x.lan", "lan"));
        assert!(!host_matches("evillan", ".lan"));
        assert!(!host_matches("lan.evil.com", ".lan"));
        assert!(is_private_suffix_host("paperless.lan.loc"));
        assert!(is_private_suffix_host("paperless.lan"));
        assert!(is_private_suffix_host("loc"));
        assert!(!is_private_suffix_host("evillan"));
        assert!(!is_private_suffix_host("lan.evil.com"));
        assert!(!is_private_suffix_host("paperless.example.com"));
        assert!(!is_private_suffix_host("evil.loc.example.com"));
        // !48 (): *.lan dotted suffix only, trailing dot tolerated.
        assert!(is_private_suffix_host("x.lan"));
        assert!(is_private_suffix_host("paperless.lan."));
        assert!(!is_private_suffix_host("x.lan.evil.com"));
    }

    #[test]
    fn allowlist_neutral_examples_exact_unless_leading_dot() {
        // !53 examples under !51 semantics: exact host, or `.suffix`.
        assert!(host_matches("paperless.internal.example", "paperless.internal.example"));
        assert!(!host_matches("evil.paperless.internal.example", "paperless.internal.example"));
        assert!(host_matches("evil.paperless.internal.example", ".paperless.internal.example"));
        assert!(!host_matches("paperless.internal.example.evil.com", "paperless.internal.example"));
        assert!(host_matches("paperless.example", "paperless.example"));
        assert!(host_matches("192.0.2.10", "192.0.2.10"));
        assert!(!host_matches("evilpaperless.example", "paperless.example"));
        assert!(!host_matches("paperless.example.evil.com", "paperless.example"));
        assert!(is_private_suffix_host("paperless.lab.loc"));
        assert!(!is_private_suffix_host("paperless.example.com"));
    }

    #[test]
    fn clamp_limits_apply_hard_ceiling() {
        let (docs, pages) = clamp_import_limits(None);
        assert_eq!(docs, DEFAULT_MAX_DOCUMENTS);
        assert!(pages <= DEFAULT_MAX_PAGES);
        let (docs, pages) = clamp_import_limits(Some(99_999));
        assert_eq!(docs, HARD_MAX_DOCUMENTS);
        assert!(pages <= HARD_MAX_PAGES);
        assert!(pages <= DEFAULT_MAX_PAGES);
    }

    #[test]
    fn evicts_terminal_jobs_past_ttl() {
        let hub = ImportHub::default();
        {
            let mut inner = hub.inner.lock().unwrap();
            let (tx, _rx) = watch::channel(false);
            inner.jobs.insert(
                "old".into(),
                LiveJob {
                    user_id: 1,
                    status: ImportStatus {
                        job_id: "old".into(),
                        status: JobStatus::Done,
                        phase: None,
                        done: 1,
                        total: 1,
                        errors: 0,
                        error_samples: Vec::new(),
                        message: "done".into(),
                    },
                    cancel_tx: tx,
                    updated_at: Instant::now() - JOB_TTL_TERMINAL - Duration::from_secs(5),
                },
            );
        }
        assert!(hub.status_for(1, "old").is_none());
    }
}
