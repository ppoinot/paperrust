//! Same-origin WordCraft static proxy.
//!
//! The iframe is `/wordcraft/?host=parent`. The query stays on the browser URL.
//! It is not forwarded upstream.

use axum::body::Body;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use url::Url;

/// Compose service name: one DNS label, no dots. Loopback is test-only.
fn wordcraft_upstream_host_ok(host: &str) -> bool {
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

/// Static WordCraft beside this process. HTTP only, no path, user, query, or fragment.
pub fn wordcraft_upstream() -> String {
    let fallback = "http://wordcraft";
    let raw = std::env::var("PAPERRUST_WORDCRAFT_UPSTREAM").unwrap_or_else(|_| fallback.into());
    let trimmed = raw.trim().trim_end_matches('/').to_string();
    let Ok(url) = Url::parse(&trimmed) else {
        return fallback.into();
    };
    let host = url.host_str().unwrap_or("");
    let path_ok = url.path().is_empty() || url.path() == "/";
    let port_ok = url.port().is_none() || (cfg!(test) && host == "127.0.0.1");
    if url.scheme() == "http"
        && wordcraft_upstream_host_ok(host)
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && path_ok
        && port_ok
    {
        let mut clean = url;
        clean.set_path("");
        clean.set_query(None);
        clean.set_fragment(None);
        return clean.to_string().trim_end_matches('/').to_string();
    }
    fallback.into()
}

/// Map a same-origin `/wordcraft…` request onto the static site. Rejects `..`.
pub fn wordcraft_upstream_path(request_path: &str) -> Result<String, ()> {
    let Some(rest) = request_path.strip_prefix("/wordcraft") else {
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

/// Fetch the static site. The query string (`host`, `author`) is not forwarded.
pub async fn proxy_wordcraft(method_is_head: bool, request_path: &str) -> Response {
    let Ok(upstream_path) = wordcraft_upstream_path(request_path) else {
        return (StatusCode::BAD_REQUEST, "bad path").into_response();
    };
    let url = format!("{}{upstream_path}", wordcraft_upstream());
    let client = match reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(60))
        .build()
    {
        Ok(c) => c,
        Err(_) => return (StatusCode::BAD_GATEWAY, "wordcraft").into_response(),
    };
    let method = if method_is_head {
        reqwest::Method::HEAD
    } else {
        reqwest::Method::GET
    };
    let Ok(upstream) = client.request(method, &url).send().await else {
        return (StatusCode::BAD_GATEWAY, "wordcraft").into_response();
    };
    if upstream.status().is_redirection() {
        return (StatusCode::BAD_GATEWAY, "wordcraft").into_response();
    }
    let status = StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let content_type = upstream
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let cache = upstream
        .headers()
        .get(header::CACHE_CONTROL)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let body = if method_is_head {
        Vec::new()
    } else {
        match upstream.bytes().await {
            Ok(b) => b.to_vec(),
            Err(_) => return (StatusCode::BAD_GATEWAY, "wordcraft").into_response(),
        }
    };
    let mut builder = Response::builder().status(status);
    let wasm = upstream_path.ends_with(".wasm");
    let ct = if wasm {
        "application/wasm"
    } else {
        content_type.as_deref().unwrap_or("application/octet-stream")
    };
    builder = builder.header(header::CONTENT_TYPE, ct);
    if let Some(cache) = cache {
        builder = builder.header(header::CACHE_CONTROL, cache);
    }
    builder = builder.header(header::REFERRER_POLICY, "no-referrer");
    builder
        .body(Body::from(body))
        .unwrap_or_else(|_| (StatusCode::BAD_GATEWAY, "wordcraft").into_response())
}

pub const WORDCRAFT_CSP: &str = "default-src 'self'; base-uri 'self'; object-src 'none'; frame-ancestors 'self'; img-src 'self' data: blob:; font-src 'self'; style-src 'self' 'unsafe-inline'; script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval'; connect-src 'self'; worker-src 'self' blob:";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wordcraft_path_rejects_dotdot() {
        assert_eq!(wordcraft_upstream_path("/wordcraft").unwrap(), "/index.html");
        assert_eq!(wordcraft_upstream_path("/wordcraft/").unwrap(), "/index.html");
        assert_eq!(
            wordcraft_upstream_path("/wordcraft/wordcraft_bg.wasm").unwrap(),
            "/wordcraft_bg.wasm"
        );
        assert!(wordcraft_upstream_path("/wordcraft/../secret").is_err());
        assert!(wordcraft_upstream_path("/wordcraft/%2e%2e/secret").is_err());
        assert!(wordcraft_upstream_path("/wordcraft/%2E%2E/secret").is_err());
        assert!(wordcraft_upstream_path("/other").is_err());
        assert!(wordcraft_upstream_path("/wordcraft/a\\b").is_err());
    }

    #[test]
    fn wordcraft_upstream_rejects_foreign_hosts() {
        let _g = crate::instance::env_lock().lock().unwrap();
        let prev = std::env::var("PAPERRUST_WORDCRAFT_UPSTREAM").ok();
        unsafe {
            std::env::set_var("PAPERRUST_WORDCRAFT_UPSTREAM", "http://169.254.169.254/latest");
        }
        assert_eq!(wordcraft_upstream(), "http://wordcraft");
        unsafe {
            std::env::set_var("PAPERRUST_WORDCRAFT_UPSTREAM", "https://wordcraft");
        }
        assert_eq!(wordcraft_upstream(), "http://wordcraft");
        unsafe {
            std::env::set_var("PAPERRUST_WORDCRAFT_UPSTREAM", "http://wordcraft");
        }
        assert_eq!(wordcraft_upstream(), "http://wordcraft");
        unsafe {
            match prev {
                Some(v) => std::env::set_var("PAPERRUST_WORDCRAFT_UPSTREAM", v),
                None => std::env::remove_var("PAPERRUST_WORDCRAFT_UPSTREAM"),
            }
        }
    }
}
