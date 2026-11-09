//! Streamable HTTP MCP.
//!
//! The connector authenticates with `PAPERRUST_MCP_BEARER`. The process then uses
//! `PAPERRUST_MCP_API_TOKEN` (a `pr_…` from Réglages) and enforces that
//! token's scopes. The bearer is never a `pr_…`, and the `pr_…` is never
//! read from the caller.

use axum::body::to_bytes;
use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::api_tokens::{
    self, PresentedToken, TokenPrincipal, SCOPE_DIAGNOSTICS_READ, SCOPE_DOCUMENTS_META,
    SCOPE_DOCUMENTS_READ,
};
use crate::auth::{self, AuthUser};
use crate::models::{CustomValuePatch, DocumentDetail, DocumentFilter, DocumentListItem, MetaUpdate};
use crate::queries::{self, TextWindow};
use crate::{rate_limit_ok, AppState, MetaWrite};

const PROTOCOL: &str = "2025-03-26";
/// JSON-RPC body cap. Large enough for one base64 upload under [`MCP_UPLOAD_MAX`].
pub const BODY_LIMIT: usize = 12 * 1024 * 1024;
const MCP_UPLOAD_MAX: usize = 8 * 1024 * 1024;
const TOOL_JSON_MAX: usize = 14_000;
/// Raw bytes of the original file returned in one `get_file` call.
const FILE_CHUNK_MAX: i32 = 262_144;

#[derive(Debug, PartialEq, Eq)]
enum Gate {
    NotConfigured,
    Unauthorized,
    Authorized,
}

fn eq_secret(presented: &str, expected: &str) -> bool {
    let a = Sha256::digest(presented.as_bytes());
    let b = Sha256::digest(expected.as_bytes());
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn gate(presented: Option<&str>, expected_bearer: Option<&str>, expected_token: Option<&str>) -> Gate {
    let (Some(expected_bearer), Some(expected_token)) = (expected_bearer, expected_token) else {
        return Gate::NotConfigured;
    };
    if expected_bearer.is_empty()
        || expected_token.is_empty()
        || !expected_token.starts_with("pr_")
    {
        return Gate::NotConfigured;
    }
    let Some(presented) = presented.map(str::trim).filter(|s| !s.is_empty()) else {
        return Gate::Unauthorized;
    };
    // A personal token presented here is not the connector key.
    if presented.starts_with("pr_") || !eq_secret(presented, expected_bearer) {
        return Gate::Unauthorized;
    }
    Gate::Authorized
}

fn env_secret(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn presented_bearer(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?.trim();
    let rest = value
        .strip_prefix("Bearer ")
        .or_else(|| value.strip_prefix("bearer "))?;
    let rest = rest.trim();
    if rest.is_empty() {
        None
    } else {
        Some(rest.to_string())
    }
}

fn accept_ok(headers: &HeaderMap) -> bool {
    let Some(value) = headers.get(header::ACCEPT).and_then(|v| v.to_str().ok()) else {
        return true;
    };
    let value = value.to_ascii_lowercase();
    value.contains("application/json") || value.contains("text/event-stream") || value.contains("*/*")
}

pub async fn mcp_http(State(state): State<AppState>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let bearer = env_secret("PAPERRUST_MCP_BEARER");
    let token = env_secret("PAPERRUST_MCP_API_TOKEN");
    let presented = presented_bearer(&parts.headers);
    match gate(presented.as_deref(), bearer.as_deref(), token.as_deref()) {
        Gate::NotConfigured => return StatusCode::NOT_FOUND.into_response(),
        Gate::Unauthorized => return crate::mcp_oauth::unauthorized_challenge(),
        Gate::Authorized => {}
    }
    let Some(token) = token else {
        return StatusCode::NOT_FOUND.into_response();
    };

    match parts.method {
        axum::http::Method::GET => {
            let mut res = StatusCode::METHOD_NOT_ALLOWED.into_response();
            res.headers_mut().insert(
                header::ALLOW,
                HeaderValue::from_static("POST, DELETE"),
            );
            res
        }
        axum::http::Method::DELETE => StatusCode::NO_CONTENT.into_response(),
        axum::http::Method::POST => {
            if !accept_ok(&parts.headers) {
                return StatusCode::NOT_ACCEPTABLE.into_response();
            }
            let bytes = match to_bytes(body, BODY_LIMIT).await {
                Ok(bytes) => bytes,
                Err(_) => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
            };
            let principal = match load_principal(&state, &token).await {
                Ok(principal) => principal,
                Err(response) => return response,
            };
            let Some(user) = auth::user_by_id(&state.pool, principal.owner_id).await else {
                return crate::mcp_oauth::unauthorized_challenge();
            };
            let parsed: Value = match serde_json::from_slice(&bytes) {
                Ok(value) => value,
                Err(_) => {
                    return Json(rpc_error(None, -32700, "Parse error")).into_response();
                }
            };
            if parsed.is_array() {
                return Json(rpc_error(None, -32600, "Invalid Request")).into_response();
            }
            dispatch(&state, &user, &principal, &parsed).await
        }
        _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
    }
}

async fn load_principal(state: &AppState, token: &str) -> Result<TokenPrincipal, Response> {
    let mut headers = HeaderMap::new();
    let Ok(value) = HeaderValue::from_str(&format!("Bearer {token}")) else {
        return Err(crate::mcp_oauth::unauthorized_challenge());
    };
    headers.insert(header::AUTHORIZATION, value);
    match api_tokens::presented_token(&state.pool, &headers).await {
        Ok(PresentedToken::Valid(principal)) => Ok(principal),
        Ok(_) => Err(crate::mcp_oauth::unauthorized_challenge()),
        Err(_) => {
            tracing::error!("mcp token lookup failed");
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "server_error"})),
            )
                .into_response())
        }
    }
}

async fn dispatch(
    state: &AppState,
    user: &AuthUser,
    principal: &TokenPrincipal,
    request: &Value,
) -> Response {
    let method = request.get("method").and_then(|v| v.as_str()).unwrap_or("");
    let id = request.get("id").cloned();
    let params = request.get("params").cloned().unwrap_or(Value::Null);
    if method.is_empty() {
        return Json(rpc_error(id.as_ref(), -32600, "Invalid Request")).into_response();
    }
    if id.is_none() {
        return StatusCode::ACCEPTED.into_response();
    }
    let id = id.unwrap_or(Value::Null);
    let result = match method {
        "initialize" => {
            if !charge(principal, "botread", api_tokens::READ_RATE_MAX) {
                return rate_limited();
            }
            rpc_ok(
                &id,
                json!({
                    "protocolVersion": negotiated(&params),
                    "capabilities": {"tools": {"listChanged": false}},
                    "serverInfo": {"name": "paperrust", "version": "0.1.0"}
                }),
            )
        }
        "ping" => {
            if !charge(principal, "botread", api_tokens::READ_RATE_MAX) {
                return rate_limited();
            }
            rpc_ok(&id, json!({}))
        }
        "tools/list" => {
            if !charge(principal, "botread", api_tokens::READ_RATE_MAX) {
                return rate_limited();
            }
            rpc_ok(&id, json!({"tools": tool_defs(&principal.scopes)}))
        }
        "tools/call" => {
            let Some(name) = params.get("name").and_then(|v| v.as_str()) else {
                return Json(rpc_error(Some(&id), -32602, "Invalid params")).into_response();
            };
            let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
            if !tool_allowed(name, &principal.scopes) {
                return Json(tool_err(&id, json!({"error":"insufficient_scope"}))).into_response();
            }
            let (bucket, max) = tool_budget(name);
            if !charge(principal, bucket, max) {
                return Json(tool_err(&id, json!({"error":"rate_limited"}))).into_response();
            }
            match call_tool(state, user, &principal.scopes, name, &args).await {
                Ok(payload) => tool_ok(&id, payload),
                Err(payload) => tool_err(&id, payload),
            }
        }
        _ => return Json(rpc_error(Some(&id), -32601, "Method not found")).into_response(),
    };
    Json(result).into_response()
}

fn negotiated(params: &Value) -> &'static str {
    match params.get("protocolVersion").and_then(|v| v.as_str()) {
        Some("2024-11-05") => "2024-11-05",
        _ => PROTOCOL,
    }
}

fn charge(principal: &TokenPrincipal, bucket: &str, max: u32) -> bool {
    rate_limit_ok(
        &format!("{bucket}:{}", principal.id),
        max,
        api_tokens::READ_RATE_WINDOW_SECS,
    )
}

fn rate_limited() -> Response {
    (
        StatusCode::TOO_MANY_REQUESTS,
        Json(json!({"error": "rate_limited"})),
    )
        .into_response()
}

fn rpc_ok(id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn rpc_error(id: Option<&Value>, code: i32, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id.cloned().unwrap_or(Value::Null),
        "error": {"code": code, "message": message}
    })
}

fn tool_ok(id: &Value, payload: Value) -> Value {
    rpc_ok(id, json!({"content": [{"type": "text", "text": encode_payload(&payload)}]}))
}

fn tool_err(id: &Value, payload: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "content": [{"type": "text", "text": encode_payload(&payload)}],
            "isError": true
        }
    })
}

fn encode_payload(payload: &Value) -> String {
    serde_json::to_string(payload).unwrap_or_else(|_| "{\"error\":\"encode\"}".into())
}

fn tool_names_for(scopes: &[String]) -> Vec<&'static str> {
    let allows = |scope: &str| scopes.iter().any(|seen| seen == scope);
    let mut names = vec!["whoami"];
    if allows(SCOPE_DOCUMENTS_READ) {
        names.extend(["search_documents", "get_document", "get_page_text", "get_file"]);
    }
    if allows(SCOPE_DOCUMENTS_META) {
        names.push("set_labels");
    }
    if allows(SCOPE_DIAGNOSTICS_READ) {
        names.extend(["consumption_journal", "diagnostics"]);
    }
    if allows(api_tokens::SCOPE_INGEST) {
        names.push("upload_file");
    }
    names
}

fn tool_allowed(name: &str, scopes: &[String]) -> bool {
    tool_names_for(scopes).contains(&name)
}

fn tool_budget(name: &str) -> (&'static str, u32) {
    match name {
        "upload_file" => ("botingest", api_tokens::META_RATE_MAX),
        "set_labels" => ("botmeta", api_tokens::META_RATE_MAX),
        "consumption_journal" | "diagnostics" => ("botdiag", api_tokens::DIAG_RATE_MAX),
        _ => ("botread", api_tokens::READ_RATE_MAX),
    }
}

fn obj_schema(properties: &[(&str, &str)], required: &[&str]) -> Value {
    let mut props = Map::new();
    for (name, kind) in properties {
        props.insert((*name).to_string(), json!({"type": kind}));
    }
    json!({
        "type": "object",
        "properties": props,
        "required": required,
    })
}

fn tool_defs(scopes: &[String]) -> Vec<Value> {
    let spec = [
        ("whoami", "Identité du propriétaire et scopes du jeton. Jamais le secret.", &[][..], &[][..]),
        (
            "search_documents",
            "Recherche dans les documents du propriétaire. Extrait court, sans miniature ni fichier.",
            &[("q", "string"), ("limit", "integer"), ("offset", "integer")][..],
            &[][..],
        ),
        (
            "get_document",
            "Métadonnées et une fenêtre du texte OCR. maxChars est plafonné à 12000.",
            &[("id", "integer"), ("offset", "integer"), ("maxChars", "integer")][..],
            &["id"][..],
        ),
        (
            "get_page_text",
            "Texte d'une page PDF (1 = première). Pas le fichier. Une page hors limite est une erreur courte.",
            &[("id", "integer"), ("page", "integer"), ("offset", "integer"), ("maxChars", "integer")][..],
            &["id", "page"][..],
        ),
        (
            "get_file",
            "Octets d'origine du fichier, en base64. size est la taille totale. nextOffset est null quand la lecture est finie. maxBytes est plafonné à 262144.",
            &[("id", "integer"), ("offset", "integer"), ("maxBytes", "integer")][..],
            &["id"][..],
        ),
        (
            "upload_file",
            "Enregistre un PDF, PNG ou JPEG. data est le fichier en base64. Le texte OCR est lancé comme pour un import. max 8 Mio.",
            &[
                ("filename", "string"),
                ("data", "string"),
                ("mimeType", "string"),
                ("title", "string"),
                ("skipOcr", "boolean"),
            ][..],
            &["filename", "data"][..],
        ),
        (
            "set_labels",
            "Modifie titre, correspondant, type, étiquettes, date, chemin et champs. N'écrit pas le texte OCR et ne supprime pas le document. createMissing crée un tag, un correspondant ou un type manquant.",
            &[
                ("id", "integer"),
                ("title", "string"),
                ("correspondent", "string"),
                ("documentType", "string"),
                ("tags", "array"),
                ("createdDate", "string"),
                ("storagePath", "string"),
                ("fields", "array"),
                ("createMissing", "boolean"),
            ][..],
            &["id"][..],
        ),
        (
            "consumption_journal",
            "Derniers imports et OCR du propriétaire (fichier, statut, message).",
            &[("limit", "integer")][..],
            &[][..],
        ),
        (
            "diagnostics",
            "Journal technique du processus, déjà redacté. level = warn (défaut) ou info.",
            &[("level", "string"), ("limit", "integer")][..],
            &[][..],
        ),
    ];
    let allowed = tool_names_for(scopes);
    spec.into_iter()
        .filter(|(name, _, _, _)| allowed.contains(name))
        .map(|(name, description, props, required)| {
            json!({
                "name": name,
                "description": description,
                "inputSchema": obj_schema(props, required),
            })
        })
        .collect()
}

fn args_of(value: &Value) -> Option<&Map<String, Value>> {
    value.as_object()
}

fn arg_i32(map: &Map<String, Value>, key: &str) -> Option<i32> {
    let n = map.get(key)?.as_i64()?;
    i32::try_from(n).ok()
}

async fn call_tool(
    state: &AppState,
    user: &AuthUser,
    scopes: &[String],
    name: &str,
    args: &Value,
) -> Result<Value, Value> {
    let map = args_of(args).cloned().unwrap_or_default();
    match name {
        "whoami" => Ok(json!({
            "email": user.email,
            "name": user.name,
            "scopes": scopes,
        })),
        "search_documents" => search_documents(state, user, &map).await,
        "get_document" => get_document_tool(state, user, &map).await,
        "get_page_text" => get_page_text_tool(state, user, &map).await,
        "get_file" => get_file_tool(state, user, &map).await,
        "upload_file" => upload_file_tool(state, user, &map).await,
        "set_labels" => set_labels(state, user, &map).await,
        "consumption_journal" => journal(state, user, &map).await,
        "diagnostics" => diagnostics_tool(user, &map),
        _ => Err(json!({"error":"unknown_tool"})),
    }
}

fn search_hit_json(item: &DocumentListItem) -> Value {
    json!({
        "id": item.id,
        "title": item.title,
        "excerpt": item.content,
        "correspondent": item.correspondent_name,
        "documentType": item.document_type_name,
        "tags": item.tags.iter().map(|tag| tag.name.clone()).collect::<Vec<_>>(),
        "pageCount": item.page_count,
        "createdDate": item.created_date,
        "ocrStatus": item.ocr_status,
    })
}

fn compact_meta(doc: &DocumentDetail) -> Value {
    json!({
        "id": doc.item.id,
        "title": doc.item.title,
        "correspondent": doc.item.correspondent_name,
        "documentType": doc.item.document_type_name,
        "storagePath": doc.item.storage_path_name,
        "tags": doc.item.tags.iter().map(|tag| tag.name.clone()).collect::<Vec<_>>(),
        "fields": doc.custom_values.iter().map(|field| json!({
            "name": field.name,
            "value": field.value,
        })).collect::<Vec<_>>(),
        "createdDate": doc.item.created_date,
        "pageCount": doc.item.page_count,
        "hasOriginal": doc.has_original,
        "mimeType": doc.item.mime_type,
    })
}

fn document_view(doc: &DocumentDetail, offset: i32, max_chars: i32) -> Value {
    let window: TextWindow = queries::text_window(&doc.item.content, offset, max_chars);
    let mut body = compact_meta(doc);
    if let Some(obj) = body.as_object_mut() {
        obj.insert("text".into(), json!(window.text));
        obj.insert("offset".into(), json!(window.offset));
        obj.insert("nextOffset".into(), json!(window.next_offset));
        obj.insert("truncated".into(), json!(window.truncated));
        obj.insert("totalChars".into(), json!(window.total_chars));
    }
    body
}

fn shrink_to_fit(mut items: Vec<Value>, wrap: impl Fn(&[Value], bool) -> Value) -> String {
    let mut truncated = false;
    loop {
        let payload = wrap(&items, truncated);
        let text = encode_payload(&payload);
        if text.len() <= TOOL_JSON_MAX || items.len() <= 1 {
            return text;
        }
        items.pop();
        truncated = true;
    }
}

async fn search_documents(
    state: &AppState,
    user: &AuthUser,
    map: &Map<String, Value>,
) -> Result<Value, Value> {
    let limit = map
        .get("limit")
        .and_then(|v| v.as_i64())
        .and_then(|n| i32::try_from(n).ok())
        .unwrap_or(20)
        .clamp(1, 50);
    let offset = map
        .get("offset")
        .and_then(|v| v.as_i64())
        .and_then(|n| i32::try_from(n).ok())
        .unwrap_or(0)
        .clamp(0, 1_000_000);
    let q = map.get("q").and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    let rows = queries::list_documents_limited(
        &state.pool,
        &DocumentFilter {
            q,
            limit: Some(limit),
            offset: Some(offset),
            include_thumbnails: Some(false),
            ..Default::default()
        },
        user.id,
        Some(limit),
    )
    .await
    .map_err(|_| json!({"error":"request_failed"}))?;
    let hits: Vec<Value> = rows.iter().map(search_hit_json).collect();
    let text = shrink_to_fit(hits, |items, truncated| {
        json!({"documents": items, "truncated": truncated})
    });
    serde_json::from_str(&text).map_err(|_| json!({"error":"encode"}))
}

async fn get_document_tool(
    state: &AppState,
    user: &AuthUser,
    map: &Map<String, Value>,
) -> Result<Value, Value> {
    let id = arg_i32(map, "id").ok_or_else(|| json!({"error":"invalid_params"}))?;
    let offset = arg_i32(map, "offset").unwrap_or(0);
    let max_chars = arg_i32(map, "maxChars").unwrap_or(queries::TEXT_WINDOW_DEFAULT);
    let Some(doc) = queries::get_document(&state.pool, id, user.id)
        .await
        .map_err(|_| json!({"error":"request_failed"}))?
    else {
        return Err(json!({"error":"not_found"}));
    };
    Ok(fit_document(&doc, offset, max_chars))
}

fn fit_document(doc: &DocumentDetail, offset: i32, mut max_chars: i32) -> Value {
    max_chars = if max_chars <= 0 {
        queries::TEXT_WINDOW_DEFAULT
    } else {
        max_chars.min(queries::TEXT_WINDOW_MAX)
    };
    loop {
        let view = document_view(doc, offset, max_chars);
        if encode_payload(&view).len() <= TOOL_JSON_MAX || max_chars <= 400 {
            return view;
        }
        max_chars /= 2;
    }
}

fn file_chunk_len(requested: Option<i32>) -> i32 {
    match requested {
        Some(n) if n > 0 => n.min(FILE_CHUNK_MAX),
        _ => FILE_CHUNK_MAX,
    }
}

fn file_payload(
    id: i32,
    filename: Option<&str>,
    mime: &str,
    page_count: i32,
    total: i64,
    offset: i64,
    chunk: &[u8],
) -> Value {
    let next = offset.saturating_add(chunk.len() as i64);
    json!({
        "id": id,
        "filename": filename,
        "mimeType": mime,
        "pageCount": page_count,
        "size": total,
        "offset": offset,
        "length": chunk.len(),
        "nextOffset": if next >= total { Value::Null } else { json!(next) },
        "encoding": "base64",
        "data": STANDARD.encode(chunk),
    })
}

async fn get_file_tool(
    state: &AppState,
    user: &AuthUser,
    map: &Map<String, Value>,
) -> Result<Value, Value> {
    let id = arg_i32(map, "id").ok_or_else(|| json!({"error":"invalid_params"}))?;
    let offset = arg_i32(map, "offset").unwrap_or(0);
    if offset < 0 {
        return Err(json!({"error":"invalid_params","field":"offset"}));
    }
    let len = file_chunk_len(arg_i32(map, "maxBytes"));
    let Some(slice) = crate::preview::load_original_slice(
        &state.pool,
        id,
        user.id,
        i64::from(offset),
        len,
    )
    .await
    .map_err(|_| json!({"error":"request_failed"}))?
    else {
        return Err(json!({"error":"not_found"}));
    };
    if slice.unavailable {
        return Err(json!({"error":"file_unavailable"}));
    }
    if i64::from(offset) > slice.byte_length {
        return Err(json!({"error":"offset_out_of_range","size": slice.byte_length}));
    }
    Ok(file_payload(
        id,
        slice.original_filename.as_deref(),
        &slice.mime_type,
        slice.page_count,
        slice.byte_length,
        i64::from(offset),
        &slice.chunk,
    ))
}

fn decode_upload(data: &str) -> Result<Vec<u8>, Value> {
    let trimmed = data.trim();
    if trimmed.is_empty() {
        return Err(json!({"error":"missing_file"}));
    }
    let b64 = if let Some(rest) = trimmed.strip_prefix("data:") {
        rest.split_once(',').map(|(_, payload)| payload).unwrap_or(trimmed)
    } else {
        trimmed
    };
    let bytes = STANDARD
        .decode(b64)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(b64))
        .map_err(|_| json!({"error":"invalid_file"}))?;
    if bytes.is_empty() {
        return Err(json!({"error":"missing_file"}));
    }
    if bytes.len() > MCP_UPLOAD_MAX {
        return Err(json!({"error":"payload_too_large","maxBytes": MCP_UPLOAD_MAX}));
    }
    Ok(bytes)
}

async fn upload_file_tool(
    state: &AppState,
    user: &AuthUser,
    map: &Map<String, Value>,
) -> Result<Value, Value> {
    let filename = map
        .get("filename")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| json!({"error":"invalid_params","field":"filename"}))?;
    let data = map
        .get("data")
        .and_then(|v| v.as_str())
        .ok_or_else(|| json!({"error":"invalid_params","field":"data"}))?;
    let bytes = decode_upload(data)?;
    let size = bytes.len();
    let mime = map
        .get("mimeType")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("application/octet-stream");
    let title = map
        .get("title")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let skip_ocr = map.get("skipOcr").and_then(|v| v.as_bool()).unwrap_or(false);
    let provider = crate::consume::load_ocr_provider(&state.pool, user.id).await;
    let input = crate::models::ConsumeInput {
        filename: filename.to_string(),
        mime_type: mime.to_string(),
        data_base64: String::new(),
        raw_bytes: Some(bytes),
        provider,
        api_key: None,
        skip_ocr: Some(skip_ocr),
        base_url: None,
        title_override: title,
        created_override: None,
    };
    match crate::consume::consume_document(&state.pool, input, user.id).await {
        Ok(result) => Ok(json!({
            "documentId": result.document_id,
            "size": size,
            "usedLlm": result.used_llm,
        })),
        Err(err) => {
            let lower = err.to_string().to_ascii_lowercase();
            tracing::warn!(owner_id = user.id, "mcp upload failed");
            if lower.contains("trop volumineux") || lower.contains("too large") {
                Err(json!({"error":"payload_too_large","maxBytes": MCP_UPLOAD_MAX}))
            } else if lower.contains("non support") || lower.contains("unsupported") {
                Err(json!({"error":"unsupported_type"}))
            } else if lower.contains("base64") {
                Err(json!({"error":"invalid_file"}))
            } else {
                Err(json!({"error":"request_failed"}))
            }
        }
    }
}

async fn get_page_text_tool(
    state: &AppState,
    user: &AuthUser,
    map: &Map<String, Value>,
) -> Result<Value, Value> {
    let id = arg_i32(map, "id").ok_or_else(|| json!({"error":"invalid_params"}))?;
    let page = arg_i32(map, "page").ok_or_else(|| json!({"error":"invalid_params"}))?;
    let offset = arg_i32(map, "offset").unwrap_or(0);
    let max_chars = arg_i32(map, "maxChars").unwrap_or(queries::TEXT_WINDOW_DEFAULT);
    if page < 1 || page > crate::preview::MAX_PAGES {
        return Err(json!({"error":"page_out_of_range"}));
    }
    let Some(doc) = queries::get_document(&state.pool, id, user.id)
        .await
        .map_err(|_| json!({"error":"request_failed"}))?
    else {
        return Err(json!({"error":"not_found"}));
    };
    if let Some(orig) = crate::preview::load_original(&state.pool, id, user.id)
        .await
        .map_err(|_| json!({"error":"request_failed"}))?
    {
        if let Some(bytes) = orig.bytes.clone() {
            if crate::preview::is_pdf(&orig.mime_type, &bytes) {
                let mime = orig.mime_type.clone();
                match crate::limits::run_heavy_blocking(
                    std::time::Duration::from_secs(20),
                    move || crate::preview::page_text(&bytes, &mime, page),
                )
                .await
                {
                    Ok(text) => {
                        let excerpt = if text.trim().is_empty() {
                            Some(queries::text_window(&doc.item.content, 0, 480).text)
                        } else {
                            None
                        };
                        return Ok(fit_window(&text, offset, max_chars, doc.item.page_count, |obj| {
                            obj.insert("source".into(), json!("pdf"));
                            obj.insert("page".into(), json!(page));
                            if let Some(excerpt) = excerpt.clone() {
                                obj.insert("ocrExcerpt".into(), json!(excerpt));
                            }
                        }));
                    }
                    Err(err) => {
                        if err.to_string().contains("out of range") {
                            return Err(json!({"error":"page_out_of_range"}));
                        }
                        tracing::warn!(owner_id = user.id, "mcp page text failed");
                        return Err(json!({"error":"page_text_unavailable"}));
                    }
                }
            }
        }
    }
    if page != 1 {
        return Err(json!({"error":"page_out_of_range"}));
    }
    Ok(fit_window(
        &doc.item.content,
        offset,
        max_chars,
        doc.item.page_count,
        |obj| {
            obj.insert("source".into(), json!("ocr"));
            obj.insert("page".into(), json!(1));
        },
    ))
}

fn window_value(content: &str, offset: i32, max_chars: i32, page_count: i32) -> Value {
    let window = queries::text_window(content, offset, max_chars);
    json!({
        "text": window.text,
        "offset": window.offset,
        "nextOffset": window.next_offset,
        "truncated": window.truncated,
        "totalChars": window.total_chars,
        "pageCount": page_count,
    })
}

/// Halve the window until the JSON stays under the Grok tool cap.
fn fit_window(
    content: &str,
    offset: i32,
    mut max_chars: i32,
    page_count: i32,
    extra: impl Fn(&mut Map<String, Value>),
) -> Value {
    max_chars = if max_chars <= 0 {
        queries::TEXT_WINDOW_DEFAULT
    } else {
        max_chars.min(queries::TEXT_WINDOW_MAX)
    };
    loop {
        let mut body = window_value(content, offset, max_chars, page_count);
        if let Some(obj) = body.as_object_mut() {
            extra(obj);
        }
        if encode_payload(&body).len() <= TOOL_JSON_MAX || max_chars <= 400 {
            return body;
        }
        max_chars /= 2;
    }
}

enum NamePatch {
    Absent,
    Clear,
    Set(String),
}

fn name_patch(map: &Map<String, Value>, key: &str) -> Result<NamePatch, Value> {
    match map.get(key) {
        None => Ok(NamePatch::Absent),
        Some(Value::Null) => Ok(NamePatch::Clear),
        Some(Value::String(value)) => {
            let value = value.trim();
            if value.is_empty() {
                Ok(NamePatch::Clear)
            } else if value.chars().count() > 80 {
                Err(json!({"error":"invalid_params","field":key}))
            } else {
                Ok(NamePatch::Set(value.to_string()))
            }
        }
        Some(_) => Err(json!({"error":"invalid_params","field":key})),
    }
}

fn is_unique_violation(err: &sqlx::Error) -> bool {
    err.as_database_error()
        .and_then(|db| db.code())
        .is_some_and(|code| code == "23505")
}

/// Insert won a race against another request: look the name up again.
async fn id_after_unique(
    inserted: sqlx::Result<i32>,
    refetch: impl std::future::Future<Output = sqlx::Result<Option<i32>>>,
) -> Result<i32, Value> {
    match inserted {
        Ok(id) => Ok(id),
        Err(err) if is_unique_violation(&err) => refetch
            .await
            .map_err(|_| json!({"error":"request_failed"}))?
            .ok_or_else(|| json!({"error":"request_failed"})),
        Err(_) => Err(json!({"error":"request_failed"})),
    }
}

async fn set_labels(
    state: &AppState,
    user: &AuthUser,
    map: &Map<String, Value>,
) -> Result<Value, Value> {
    let id = arg_i32(map, "id").ok_or_else(|| json!({"error":"invalid_params"}))?;
    let create_missing = match map.get("createMissing") {
        None => false,
        Some(Value::Bool(value)) => *value,
        Some(_) => return Err(json!({"error":"invalid_params","field":"createMissing"})),
    };
    let mut update = MetaUpdate {
        id,
        title: None,
        content: None,
        correspondent_id: None,
        document_type_id: None,
        storage_path_id: None,
        created_date: None,
        asn: None,
        tag_ids: None,
        custom_values: None,
    };
    if let Some(Value::String(title)) = map.get("title") {
        let title = title.trim();
        if !title.is_empty() {
            update.title = Some(title.to_string());
        }
    } else if map.get("title").is_some() {
        return Err(json!({"error":"invalid_params","field":"title"}));
    }
    match name_patch(map, "correspondent")? {
        NamePatch::Absent => {}
        NamePatch::Clear => update.correspondent_id = Some(None),
        NamePatch::Set(name) => {
            let rows = queries::list_correspondents(&state.pool, user.id)
                .await
                .map_err(|_| json!({"error":"request_failed"}))?;
            let found = rows.iter().find(|row| row.name.eq_ignore_ascii_case(&name)).map(|row| row.id);
            let cid = if let Some(id) = found {
                id
            } else if !create_missing {
                let existing: Vec<_> = rows.iter().take(50).map(|row| row.name.clone()).collect();
                return Err(json!({"error":"unknown_name","kind":"correspondent","name":name,"existing":existing}));
            } else {
                let inserted = queries::insert_correspondent(&state.pool, user.id, &name, "", "none", None)
                    .await
                    .map(|row| row.id);
                id_after_unique(inserted, async {
                    queries::find_correspondent_by_name_ci(&state.pool, &name, user.id)
                        .await
                        .map(|row| row.map(|found| found.id))
                })
                .await?
            };
            update.correspondent_id = Some(Some(cid));
        }
    }
    match name_patch(map, "documentType")? {
        NamePatch::Absent => {}
        NamePatch::Clear => update.document_type_id = Some(None),
        NamePatch::Set(name) => {
            let rows = queries::list_types(&state.pool, user.id)
                .await
                .map_err(|_| json!({"error":"request_failed"}))?;
            let found = rows.iter().find(|row| row.name.eq_ignore_ascii_case(&name)).map(|row| row.id);
            let tid = if let Some(id) = found {
                id
            } else if !create_missing {
                let existing: Vec<_> = rows.iter().take(50).map(|row| row.name.clone()).collect();
                return Err(json!({"error":"unknown_name","kind":"documentType","name":name,"existing":existing}));
            } else {
                let inserted = queries::insert_type(&state.pool, user.id, &name, "", "none")
                    .await
                    .map(|row| row.id);
                id_after_unique(inserted, async {
                    queries::find_type_by_name_ci(&state.pool, &name, user.id)
                        .await
                        .map(|row| row.map(|found| found.id))
                })
                .await?
            };
            update.document_type_id = Some(Some(tid));
        }
    }
    match name_patch(map, "storagePath")? {
        NamePatch::Absent => {}
        NamePatch::Clear => update.storage_path_id = Some(None),
        NamePatch::Set(name) => {
            let rows = queries::list_paths(&state.pool, user.id)
                .await
                .map_err(|_| json!({"error":"request_failed"}))?;
            let Some(found) = rows.iter().find(|row| row.name.eq_ignore_ascii_case(&name)) else {
                let existing: Vec<_> = rows.iter().take(50).map(|row| row.name.clone()).collect();
                return Err(json!({"error":"unknown_name","kind":"storagePath","name":name,"existing":existing}));
            };
            update.storage_path_id = Some(Some(found.id));
        }
    }
    match name_patch(map, "createdDate")? {
        NamePatch::Absent => {}
        NamePatch::Clear => update.created_date = Some(None),
        NamePatch::Set(value) => update.created_date = Some(Some(value)),
    }
    if let Some(tags) = map.get("tags") {
        let Some(tags) = tags.as_array() else {
            return Err(json!({"error":"invalid_params","field":"tags"}));
        };
        let rows = queries::list_tags(&state.pool, user.id)
            .await
            .map_err(|_| json!({"error":"request_failed"}))?;
        let mut ids = Vec::new();
        for tag in tags {
            let Some(name) = tag.as_str().map(str::trim).filter(|s| !s.is_empty()) else {
                return Err(json!({"error":"invalid_params","field":"tags"}));
            };
            if name.chars().count() > 80 {
                return Err(json!({"error":"invalid_params","field":"tags"}));
            }
            if let Some(found) = rows.iter().find(|row| row.name.eq_ignore_ascii_case(name)) {
                if !ids.contains(&found.id) {
                    ids.push(found.id);
                }
                continue;
            }
            if !create_missing {
                let existing: Vec<_> = rows.iter().take(50).map(|row| row.name.clone()).collect();
                return Err(json!({"error":"unknown_name","kind":"tag","name":name,"existing":existing}));
            }
            let inserted = queries::insert_tag(&state.pool, user.id, name, "#57534e", false, "", "none")
                .await
                .map(|row| row.id);
            let created_id = id_after_unique(inserted, async {
                queries::find_tag_by_name_ci(&state.pool, name, user.id)
                    .await
                    .map(|row| row.map(|found| found.id))
            })
            .await?;
            ids.push(created_id);
        }
        update.tag_ids = Some(ids);
    }
    if let Some(fields) = map.get("fields") {
        let Some(fields) = fields.as_array() else {
            return Err(json!({"error":"invalid_params","field":"fields"}));
        };
        let known = queries::list_custom_fields(&state.pool, user.id)
            .await
            .map_err(|_| json!({"error":"request_failed"}))?;
        let mut patches = Vec::new();
        for field in fields {
            let Some(obj) = field.as_object() else {
                return Err(json!({"error":"invalid_params","field":"fields"}));
            };
            let Some(name) = obj.get("name").and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty()) else {
                return Err(json!({"error":"invalid_params","field":"fields"}));
            };
            let Some(found) = known.iter().find(|row| row.name.eq_ignore_ascii_case(name)) else {
                let existing: Vec<_> = known.iter().take(50).map(|row| row.name.clone()).collect();
                return Err(json!({"error":"unknown_name","kind":"field","name":name,"existing":existing}));
            };
            let value = match obj.get("value") {
                None | Some(Value::Null) => None,
                Some(Value::String(value)) => {
                    let value: String = value.chars().take(2_000).collect();
                    if value.trim().is_empty() { None } else { Some(value) }
                }
                Some(_) => return Err(json!({"error":"invalid_params","field":"fields"})),
            };
            patches.push(CustomValuePatch { field_id: found.id, value });
        }
        update.custom_values = Some(patches);
    }
    match crate::write_document_meta(&state.pool, user.id, id, update, true).await {
        Ok(MetaWrite::Saved(doc)) => Ok(compact_meta(&doc)),
        Ok(MetaWrite::NotFound) => Err(json!({"error":"not_found"})),
        Ok(MetaWrite::ContentRejected) => Err(json!({"error":"content_not_writable"})),
        Err(_) => {
            tracing::error!("mcp set_labels failed");
            Err(json!({"error":"request_failed"}))
        }
    }
}

async fn journal(state: &AppState, user: &AuthUser, map: &Map<String, Value>) -> Result<Value, Value> {
    let limit = arg_i32(map, "limit").unwrap_or(50).clamp(1, 50) as usize;
    let jobs = queries::list_jobs(&state.pool, user.id)
        .await
        .map_err(|_| json!({"error":"request_failed"}))?;
    let items: Vec<Value> = jobs
        .into_iter()
        .take(limit)
        .map(|job| {
            json!({
                "id": job.id,
                "filename": job.filename,
                "status": job.status,
                "message": job.message.as_deref().map(crate::diagnostics::scrub).unwrap_or_default(),
                "documentId": job.document_id,
                "createdAt": job.created_at,
                "finishedAt": job.finished_at,
            })
        })
        .collect();
    let text = shrink_to_fit(items, |items, truncated| json!({"jobs": items, "truncated": truncated}));
    serde_json::from_str(&text).map_err(|_| json!({"error":"encode"}))
}

fn diagnostics_tool(user: &AuthUser, map: &Map<String, Value>) -> Result<Value, Value> {
    let level = map.get("level").and_then(|v| v.as_str()).unwrap_or("warn");
    if !matches!(level, "warn" | "info") {
        return Err(json!({"error":"invalid_params","field":"level"}));
    }
    let limit = arg_i32(map, "limit").unwrap_or(30).clamp(1, 100) as usize;
    let events = crate::diagnostics::recent(user.id, level, limit);
    let items: Vec<Value> = events.into_iter().map(|event| json!(event)).collect();
    let text = shrink_to_fit(items, |items, truncated| {
        json!({"events": items, "truncated": truncated})
    });
    serde_json::from_str(&text).map_err(|_| json!({"error":"encode"}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{CustomValue, DocumentNote, HistoryItem, Tag};

    #[test]
    fn gate_fails_closed_and_rejects_personal_tokens() {
        assert_eq!(gate(None, None, None), Gate::NotConfigured);
        assert_eq!(gate(Some("secret"), Some("secret"), None), Gate::NotConfigured);
        assert_eq!(gate(Some("secret"), Some("secret"), Some("not-a-token")), Gate::NotConfigured);
        assert_eq!(gate(None, Some("secret"), Some("pr_aaaaaaaa")), Gate::Unauthorized);
        assert_eq!(
            gate(Some("pr_aaaaaaaa"), Some("pr_aaaaaaaa"), Some("pr_bbbbbbbb")),
            Gate::Unauthorized
        );
        assert_eq!(gate(Some("nope"), Some("secret"), Some("pr_aaaaaaaa")), Gate::Unauthorized);
        assert_eq!(gate(Some("secret"), Some("secret"), Some("pr_aaaaaaaa")), Gate::Authorized);
    }

    #[test]
    fn tool_list_follows_scopes() {
        let read = tool_names_for(&[SCOPE_DOCUMENTS_READ.into()]);
        assert!(read.contains(&"search_documents"));
        assert!(read.contains(&"get_page_text"));
        assert!(read.contains(&"get_file"));
        assert!(!read.contains(&"set_labels"));
        assert!(!read.contains(&"diagnostics"));
        let meta = tool_names_for(&[SCOPE_DOCUMENTS_META.into()]);
        assert!(meta.contains(&"set_labels"));
        assert!(!meta.contains(&"search_documents"));
        let diag = tool_names_for(&[SCOPE_DIAGNOSTICS_READ.into()]);
        assert!(diag.contains(&"diagnostics"));
        assert!(diag.contains(&"consumption_journal"));
        assert!(!diag.contains(&"get_document"));
        assert!(!diag.contains(&"upload_file"));
        let ingest = tool_names_for(&[api_tokens::SCOPE_INGEST.into()]);
        assert!(ingest.contains(&"upload_file"));
        assert!(!ingest.contains(&"get_file"));
        assert!(!read.contains(&"upload_file"));
    }

    fn sample_item(content: &str) -> DocumentListItem {
        DocumentListItem {
            id: 7,
            title: "Facture".into(),
            content: content.into(),
            correspondent_id: None,
            correspondent_name: Some("EDF".into()),
            document_type_id: None,
            document_type_name: Some("Facture".into()),
            storage_path_id: None,
            storage_path_name: None,
            created_date: None,
            added_at: "t".into(),
            modified_at: "t".into(),
            asn: None,
            mime_type: "application/pdf".into(),
            original_filename: None,
            page_count: 2,
            language: "fr".into(),
            ocr_status: "ok".into(),
            ocr_provider: None,
            thumbnail_svg: Some("<svg>secret-thumb</svg>".into()),
            file_size: 10,
            summary: None,
            tags: vec![Tag {
                id: 1,
                name: "2024".into(),
                color: "#000".into(),
                is_inbox_tag: false,
                match_text: String::new(),
                matching_algorithm: "none".into(),
                is_insensitive: true,
            }],
        }
    }

    #[test]
    fn search_hit_omits_thumbnails_and_document_view_is_a_window() {
        let hit = search_hit_json(&sample_item("extrait"));
        let encoded = hit.to_string();
        assert!(!encoded.contains("thumbnailSvg"));
        assert!(!encoded.contains("secret-thumb"));
        assert!(!encoded.contains("<svg"));
        assert_eq!(hit["excerpt"], "extrait");
        let long = "UNIQUECONTENT".repeat(100);
        let doc = DocumentDetail {
            item: sample_item(&long),
            file_data: Some("raw-bytes".into()),
            has_original: true,
            notes: vec![DocumentNote { id: 1, body: "note-secret".into(), created_at: "t".into() }],
            custom_values: vec![CustomValue {
                field_id: 3,
                name: "Montant".into(),
                data_type: "string".into(),
                value: Some("12".into()),
            }],
            history: vec![HistoryItem { id: 1, action: "updated".into(), detail: None, created_at: "t".into() }],
        };
        let view = document_view(&doc, 0, 10);
        let encoded = view.to_string();
        assert_eq!(view["text"].as_str().unwrap().chars().count(), 10);
        assert_eq!(view["truncated"], true);
        assert!(view.get("content").is_none());
        assert!(view.get("fileData").is_none());
        assert!(!encoded.contains("UNIQUECONTENTUNIQUECONTENT"));
        assert!(!encoded.contains("raw-bytes"));
        assert!(!encoded.contains("note-secret"));
        assert!(!encoded.contains("secret-thumb"));
        let framed = tool_ok(&json!(1), view);
        let text = framed["result"]["content"][0]["text"].as_str().unwrap();
        assert!(!text.contains("thumbnailSvg"));
        assert!(!text.contains("secret-thumb"));
    }

    #[test]
    fn initialize_advertises_tools_only() {
        let response = rpc_ok(
            &json!(1),
            json!({
                "protocolVersion": negotiated(&json!({"protocolVersion":"2025-03-26"})),
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": "paperrust", "version": "0.1.0"}
            }),
        );
        assert_eq!(response["result"]["protocolVersion"], "2025-03-26");
        assert!(response["result"]["capabilities"].get("resources").is_none());
        assert!(response["result"]["capabilities"].get("prompts").is_none());
    }

    #[test]
    fn page_window_stays_under_the_tool_cap() {
        let huge = "é".repeat(12_000);
        let view = fit_window(&huge, 0, 12_000, 1, |obj| {
            obj.insert("source".into(), json!("pdf"));
            obj.insert("page".into(), json!(1));
        });
        assert!(encode_payload(&view).len() <= TOOL_JSON_MAX);
        assert_eq!(view["truncated"], true);
        assert!(!view["text"].as_str().unwrap().contains("%PDF"));
    }

    #[test]
    fn file_payload_is_the_original_bytes() {
        let raw = b"%PDF-1.7 hello";
        let view = file_payload(4, Some("lettre.pdf"), "application/pdf", 2, raw.len() as i64, 0, raw);
        assert_eq!(view["encoding"], "base64");
        assert_eq!(view["size"], raw.len() as i64);
        assert!(view["nextOffset"].is_null());
        let data = view["data"].as_str().unwrap();
        assert_eq!(STANDARD.decode(data).unwrap(), raw);
        assert_eq!(view["filename"], "lettre.pdf");
        let partial = file_payload(4, None, "application/pdf", 1, 10, 4, b"abcd");
        assert_eq!(partial["nextOffset"], 8);
        assert_eq!(partial["length"], 4);
        assert_eq!(file_chunk_len(None), FILE_CHUNK_MAX);
        assert_eq!(file_chunk_len(Some(10)), 10);
        assert_eq!(file_chunk_len(Some(9_000_000)), FILE_CHUNK_MAX);
        assert_eq!(file_chunk_len(Some(0)), FILE_CHUNK_MAX);
    }

    #[test]
    fn upload_decode_accepts_a_pdf_and_rejects_junk() {
        let raw = b"%PDF-1.4";
        let encoded = STANDARD.encode(raw);
        assert_eq!(decode_upload(&encoded).unwrap(), raw);
        let data_url = format!("data:application/pdf;base64,{encoded}");
        assert_eq!(decode_upload(&data_url).unwrap(), raw);
        assert_eq!(decode_upload("").unwrap_err()["error"], "missing_file");
        assert_eq!(decode_upload("!!!!").unwrap_err()["error"], "invalid_file");
        let huge = STANDARD.encode(&vec![0u8; MCP_UPLOAD_MAX + 1]);
        assert_eq!(decode_upload(&huge).unwrap_err()["error"], "payload_too_large");
    }
}
