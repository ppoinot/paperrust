//! Deployment identity. Every host-specific value is an environment variable.
//!
//! Defaults are neutral: loopback bind, empty origin lists, optional features off.
//! An existing instance keeps today's behavior by setting the variables documented
//! in `.env.example` (public URLs, issuer map, CORS, Stirling, bootstrap).
//!
//! Realm split (no hostnames in this file, nothing inferred):
//! `PAPERRUST_PUBLIC_URLS` lists every origin this instance answers on.
//! `PAPERRUST_LAN_URLS` marks, explicitly, the subset that any TCP peer allowed
//! by the NetworkPolicy (ingress-nginx, cron) may claim in `Host`. Every other
//! listed origin is public: honored only when the TCP peer is the trusted
//! loopback proxy (tunnel sidecar), refused with 400 otherwise ().
//! The split is never derived from issuer hosts, DNS labels or request input.

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppOrigin {
    pub origin: String,
    pub host: String,
    pub https: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapIdentity {
    pub subject: String,
    pub email: String,
    pub name: String,
}

pub fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub fn env_flag(names: &[&str]) -> bool {
    names.iter().any(|name| {
        std::env::var(name)
            .ok()
            .map(|v| v.trim() == "1")
            .unwrap_or(false)
    })
}

pub fn split_csv(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn trim_slash(s: &str) -> String {
    s.trim().trim_end_matches('/').to_string()
}

/// `https://host[:port]` with no path. Returns `None` when the value is not an origin.
pub fn parse_origin(raw: &str) -> Option<AppOrigin> {
    let url = url::Url::parse(raw.trim()).ok()?;
    if url.scheme() != "https" && url.scheme() != "http" {
        return None;
    }
    if url.query().is_some() || url.fragment().is_some() {
        return None;
    }
    let host = url.host_str()?.to_ascii_lowercase();
    let https = url.scheme() == "https";
    let origin = match url.port() {
        Some(port) if !((https && port == 443) || (!https && port == 80)) => {
            format!("{}://{host}:{port}", url.scheme())
        }
        _ => format!("{}://{host}", url.scheme()),
    };
    Some(AppOrigin {
        origin,
        host,
        https,
    })
}

pub fn public_urls() -> Vec<AppOrigin> {
    // `PAPERRUST_PUBLIC_URL` is the singular alias (develop/#56), same as auth.
    let Some(raw) =
        env_nonempty("PAPERRUST_PUBLIC_URLS").or_else(|| env_nonempty("PAPERRUST_PUBLIC_URL"))
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for part in split_csv(&raw) {
        if let Some(origin) = parse_origin(&part) {
            if !out.iter().any(|e: &AppOrigin| e.origin == origin.origin) {
                out.push(origin);
            }
        }
    }
    out
}

/// `PAPERRUST_LAN_URLS`: origins (subset of `PAPERRUST_PUBLIC_URLS`) that any
/// NetworkPolicy-allowed peer may claim. Empty by default: fail-closed, every
/// listed origin is then public (sidecar only).
pub fn lan_urls() -> Vec<AppOrigin> {
    let Some(raw) = env_nonempty("PAPERRUST_LAN_URLS") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for part in split_csv(&raw) {
        if let Some(origin) = parse_origin(&part) {
            if !out.iter().any(|e: &AppOrigin| e.origin == origin.origin) {
                out.push(origin);
            }
        }
    }
    out
}

fn split_issuer_pair(part: &str) -> Option<(String, String)> {
    let idx = part.find("=http://").or_else(|| part.find("=https://"))?;
    let left = part[..idx].trim().to_string();
    let right = part[idx + 1..].trim().to_string();
    if left.is_empty() || right.is_empty() {
        return None;
    }
    Some((left, right))
}

/// `origin=issuer` pairs from `PAPERRUST_OIDC_ISSUERS`.
pub fn oidc_issuers() -> BTreeMap<String, String> {
    let Some(raw) = env_nonempty("PAPERRUST_OIDC_ISSUERS") else {
        return BTreeMap::new();
    };
    let mut map = BTreeMap::new();
    for part in split_csv(&raw) {
        let Some((left, right)) = split_issuer_pair(&part) else {
            continue;
        };
        let Some(origin) = parse_origin(&left) else {
            continue;
        };
        let issuer = trim_slash(&right);
        if !issuer.is_empty() {
            map.insert(origin.origin, issuer);
        }
    }
    map
}

pub fn oidc_issuer_default() -> Option<String> {
    env_nonempty("PAPERRUST_OIDC_ISSUER").map(|s| trim_slash(&s))
}

pub fn issuer_for_origin(origin: &str) -> Option<String> {
    oidc_issuers().get(origin).cloned()
}

/// Direct LAN origin: explicitly listed in `PAPERRUST_LAN_URLS`. Exact origin
/// match only (scheme, host, port); no issuer or DNS-label heuristic.
pub fn origin_is_direct_lan(origin: &AppOrigin) -> bool {
    lan_urls().iter().any(|lan| lan.origin == origin.origin)
}

pub fn find_public_url(host: &str) -> Option<AppOrigin> {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    public_urls().into_iter().find(|origin| origin.host == host)
}

pub fn first_origin() -> Option<AppOrigin> {
    public_urls().into_iter().next()
}

/// the issuer map is checked at startup. Every `PAPERRUST_OIDC_ISSUERS`
/// pair must parse, its origin must be listed in `PAPERRUST_PUBLIC_URLS`, and
/// its issuer must be an http(s) URL. Malformed entries are an error, never
/// silently skipped, and no issuer is ever inferred for an unmapped origin.
pub fn validate_issuer_config() -> Result<(), String> {
    let listed: Vec<String> = public_urls().into_iter().map(|o| o.origin).collect();
    // `PAPERRUST_LAN_URLS`: every entry must parse and be listed in
    // `PAPERRUST_PUBLIC_URLS`. Never skipped silently.
    if let Some(raw) = env_nonempty("PAPERRUST_LAN_URLS") {
        for part in split_csv(&raw) {
            let Some(origin) = parse_origin(&part) else {
                return Err(format!("PAPERRUST_LAN_URLS: \"{part}\" is not an origin"));
            };
            if !listed.iter().any(|o| o == &origin.origin) {
                return Err(format!(
                    "PAPERRUST_LAN_URLS: origin {} is not listed in PAPERRUST_PUBLIC_URLS",
                    origin.origin
                ));
            }
        }
    }
    let Some(raw) = env_nonempty("PAPERRUST_OIDC_ISSUERS") else {
        return Ok(());
    };
    for part in split_csv(&raw) {
        let Some((left, right)) = split_issuer_pair(&part) else {
            return Err(format!(
                "PAPERRUST_OIDC_ISSUERS: malformed entry \"{part}\" (expected origin=https://issuer)"
            ));
        };
        let Some(origin) = parse_origin(&left) else {
            return Err(format!("PAPERRUST_OIDC_ISSUERS: \"{left}\" is not an origin"));
        };
        if !listed.iter().any(|o| o == &origin.origin) {
            return Err(format!(
                "PAPERRUST_OIDC_ISSUERS: origin {} is not listed in PAPERRUST_PUBLIC_URLS",
                origin.origin
            ));
        }
        let issuer = trim_slash(&right);
        match url::Url::parse(&issuer) {
            Ok(u) if (u.scheme() == "https" || u.scheme() == "http") && u.host_str().is_some() => {}
            _ => {
                return Err(format!(
                    "PAPERRUST_OIDC_ISSUERS: issuer for {} is not an http(s) URL",
                    origin.origin
                ))
            }
        }
    }
    Ok(())
}

pub fn is_loopback_host(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "localhost" | "::1" | "[::1]")
}

/// `https` configured origins imply HTTPS. Anything else outside loopback does too
/// (the neutral default when no list matches). Loopback stays cleartext unless a
/// configured `http` origin says otherwise — loopback is never implied HTTPS.
pub fn host_implies_https(host_name: &str) -> bool {
    let name = host_name.trim().trim_end_matches('.').to_ascii_lowercase();
    if name.is_empty() || is_loopback_host(&name) {
        return false;
    }
    if let Some(origin) = find_public_url(&name) {
        return origin.https;
    }
    true
}

pub fn cors_origins() -> Vec<String> {
    let Some(raw) = env_nonempty("PAPERRUST_CORS_ORIGINS") else {
        return Vec::new();
    };
    split_csv(&raw)
        .into_iter()
        .map(|part| trim_slash(&part))
        .filter(|part| !part.is_empty())
        .collect()
}

pub fn cors_origin_listed(origin: &str) -> bool {
    let origin = trim_slash(origin);
    cors_origins().iter().any(|allowed| allowed == &origin)
}

/// bootstrap is opt-in. Without `PAPERRUST_BOOTSTRAP=1` the
/// `PAPERRUST_BOOTSTRAP_*` values are ignored.
pub fn bootstrap_enabled() -> bool {
    env_flag(&["PAPERRUST_BOOTSTRAP"])
}

/// user 1 may be rewritten from the configured identity only when the
/// flag is on, user 1 is still an unclaimed `bootstrap:` row, it has never
/// logged in (no `last_login_at`, no session), and the rewrite never ran before.
pub fn bootstrap_rewrite_allowed(
    enabled: bool,
    current_sub: &str,
    ever_logged_in: bool,
    already_applied: bool,
) -> bool {
    enabled && current_sub.starts_with("bootstrap:") && !ever_logged_in && !already_applied
}

/// a `bootstrap:` subject is reserved for seeding; an OIDC token must
/// never carry one (refused at login, cookie and Bearer).
pub fn oidc_sub_is_reserved(sub: &str) -> bool {
    sub.trim().to_ascii_lowercase().starts_with("bootstrap:")
}

/// All three `PAPERRUST_BOOTSTRAP_*` values, or nothing. A partial set is ignored.
/// Returns nothing unless `PAPERRUST_BOOTSTRAP=1`.
pub fn bootstrap_identity() -> Option<BootstrapIdentity> {
    if !bootstrap_enabled() {
        return None;
    }
    let email = env_nonempty("PAPERRUST_BOOTSTRAP_EMAIL");
    let name = env_nonempty("PAPERRUST_BOOTSTRAP_NAME");
    let subject = env_nonempty("PAPERRUST_BOOTSTRAP_SUBJECT");
    match (email, name, subject) {
        (Some(email), Some(name), Some(subject)) => Some(BootstrapIdentity {
            subject,
            email,
            name,
        }),
        (None, None, None) => None,
        _ => {
            tracing::warn!(
                "PAPERRUST_BOOTSTRAP_EMAIL, PAPERRUST_BOOTSTRAP_NAME and PAPERRUST_BOOTSTRAP_SUBJECT must be set together; bootstrap skipped"
            );
            None
        }
    }
}

pub fn stirling_extra_hosts() -> Vec<String> {
    let Some(raw) = env_nonempty("PAPERRUST_STIRLING_EXTRA_HOSTS") else {
        return Vec::new();
    };
    split_csv(&raw)
        .into_iter()
        .map(|host| host.trim().trim_end_matches('.').to_ascii_lowercase())
        .filter(|host| !host.is_empty())
        .collect()
}

pub fn oidc_ca_bundle_path() -> Option<String> {
    env_nonempty("PAPERRUST_OIDC_CA_BUNDLE")
}

/// `0.0.0.0` is allowed when either gate is set. `PAPERRUST_BIND_K8S` is the old name.
pub fn unspecified_bind_allowed() -> bool {
    env_flag(&["PAPERRUST_BIND_ALLOW_UNSPECIFIED", "PAPERRUST_BIND_K8S"])
}

#[cfg(test)]
pub fn env_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const URLS: &str =
        "https://archive.lan.test,https://archive.example,https://archive.legacy.test";
    const ISSUERS: &str = "https://archive.example=https://auth.example,https://archive.lan.test=https://auth.lan.test,https://archive.legacy.test=https://auth.legacy.test";
    const ANCHOR: &str = "https://auth.legacy.test";
    const LAN: &str = "https://archive.lan.test,https://archive.legacy.test";

    fn with_example_instance<T>(f: impl FnOnce() -> T) -> T {
        let _guard = env_lock().lock().unwrap();
        let keys = [
            "PAPERRUST_PUBLIC_URLS",
            "PAPERRUST_OIDC_ISSUERS",
            "PAPERRUST_OIDC_ISSUER",
            "PAPERRUST_LAN_URLS",
        ];
        let prev: Vec<Option<String>> = keys
            .iter()
            .map(|k| std::env::var(k).ok())
            .collect();
        unsafe {
            std::env::set_var("PAPERRUST_LAN_URLS", LAN);
            std::env::set_var("PAPERRUST_PUBLIC_URLS", URLS);
            std::env::set_var("PAPERRUST_OIDC_ISSUERS", ISSUERS);
            std::env::set_var("PAPERRUST_OIDC_ISSUER", ANCHOR);
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
    fn issuer_map_and_explicit_lan_split() {
        with_example_instance(|| {
            let public = find_public_url("archive.example").unwrap();
            assert!(!origin_is_direct_lan(&public));
            assert_eq!(
                issuer_for_origin(&public.origin).as_deref(),
                Some("https://auth.example")
            );
            let lan = find_public_url("archive.lan.test").unwrap();
            assert!(origin_is_direct_lan(&lan));
            assert_eq!(
                issuer_for_origin(&lan.origin).as_deref(),
                Some("https://auth.lan.test")
            );
            let legacy = find_public_url("archive.legacy.test").unwrap();
            assert!(origin_is_direct_lan(&legacy));
            assert!(host_implies_https("archive.example"));
            assert!(!host_implies_https("127.0.0.1"));
        });
    }

    #[test]
    fn empty_lists_are_the_default() {
        let _guard = env_lock().lock().unwrap();
        let keys = [
            "PAPERRUST_PUBLIC_URLS",
            "PAPERRUST_OIDC_ISSUERS",
            "PAPERRUST_OIDC_ISSUER",
            "PAPERRUST_CORS_ORIGINS",
            "PAPERRUST_BOOTSTRAP_EMAIL",
            "PAPERRUST_BOOTSTRAP_NAME",
            "PAPERRUST_BOOTSTRAP_SUBJECT",
        ];
        let prev: Vec<Option<String>> = keys.iter().map(|k| std::env::var(k).ok()).collect();
        unsafe {
            for key in keys {
                std::env::remove_var(key);
            }
        }
        assert!(public_urls().is_empty());
        assert!(oidc_issuers().is_empty());
        assert!(oidc_issuer_default().is_none());
        assert!(cors_origins().is_empty());
        assert!(bootstrap_identity().is_none());
        unsafe {
            for (key, value) in keys.iter().zip(prev) {
                match value {
                    Some(v) => std::env::set_var(key, v),
                    None => std::env::remove_var(key),
                }
            }
        }
    }

    #[test]
    fn bootstrap_rewrite_requires_flag_never_logged_in_and_once() {
        // Flag off: never.
        assert!(!bootstrap_rewrite_allowed(false, "bootstrap:admin", false, false));
        // Flag on, unclaimed, never logged in, first time: allowed.
        assert!(bootstrap_rewrite_allowed(true, "bootstrap:admin", false, false));
        // User 1 already logged in (last_login_at or a session): refused.
        assert!(!bootstrap_rewrite_allowed(true, "bootstrap:admin", true, false));
        // Already applied once: refused.
        assert!(!bootstrap_rewrite_allowed(true, "bootstrap:admin", false, true));
        // Claimed account (real OIDC subject): refused.
        assert!(!bootstrap_rewrite_allowed(true, "8f2c-real-sub", false, false));
    }

    #[test]
    fn bootstrap_identity_needs_explicit_flag() {
        let _guard = env_lock().lock().unwrap();
        let keys = [
            "PAPERRUST_BOOTSTRAP",
            "PAPERRUST_BOOTSTRAP_EMAIL",
            "PAPERRUST_BOOTSTRAP_NAME",
            "PAPERRUST_BOOTSTRAP_SUBJECT",
        ];
        let prev: Vec<Option<String>> = keys.iter().map(|k| std::env::var(k).ok()).collect();
        unsafe {
            std::env::remove_var("PAPERRUST_BOOTSTRAP");
            std::env::set_var("PAPERRUST_BOOTSTRAP_EMAIL", "a@example.test");
            std::env::set_var("PAPERRUST_BOOTSTRAP_NAME", "A");
            std::env::set_var("PAPERRUST_BOOTSTRAP_SUBJECT", "sub-a");
        }
        assert!(bootstrap_identity().is_none());
        unsafe { std::env::set_var("PAPERRUST_BOOTSTRAP", "1") };
        assert!(bootstrap_identity().is_some());
        unsafe {
            for (key, value) in keys.iter().zip(prev) {
                match value {
                    Some(v) => std::env::set_var(key, v),
                    None => std::env::remove_var(key),
                }
            }
        }
    }

    #[test]
    fn oidc_token_never_carries_bootstrap_subject() {
        assert!(oidc_sub_is_reserved("bootstrap:admin"));
        assert!(oidc_sub_is_reserved(" BOOTSTRAP:x"));
        assert!(!oidc_sub_is_reserved("8f2c-real-sub"));
        assert!(!oidc_sub_is_reserved("user-bootstrap:x"));
    }

    fn with_issuer_env<T>(urls: &str, issuers: &str, f: impl FnOnce() -> T) -> T {
        let _guard = env_lock().lock().unwrap();
        let keys = ["PAPERRUST_PUBLIC_URLS", "PAPERRUST_OIDC_ISSUERS", "PAPERRUST_LAN_URLS"];
        let prev: Vec<Option<String>> = keys.iter().map(|k| std::env::var(k).ok()).collect();
        unsafe {
            std::env::remove_var("PAPERRUST_LAN_URLS");
            std::env::set_var("PAPERRUST_PUBLIC_URLS", urls);
            std::env::set_var("PAPERRUST_OIDC_ISSUERS", issuers);
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
    fn issuer_config_is_validated_at_startup() {
        with_issuer_env(URLS, ISSUERS, || assert!(validate_issuer_config().is_ok()));
        // Origin missing from PAPERRUST_PUBLIC_URLS.
        with_issuer_env("https://archive.example", ISSUERS, || {
            assert!(validate_issuer_config().unwrap_err().contains("not listed"));
        });
        // Malformed pair is an error, not skipped.
        with_issuer_env(URLS, "https://archive.example", || {
            assert!(validate_issuer_config().unwrap_err().contains("malformed"));
        });
        with_issuer_env(URLS, "not-an-origin=https://auth.example", || {
            assert!(validate_issuer_config().is_err());
        });
    }

    /// Root cause of the 2026-10-15 beta 400: issuers on unrelated domains
    /// (LAN origin -> auth.lan, default issuer on auth.example) must not decide
    /// the realm. Only `PAPERRUST_LAN_URLS` does.
    #[test]
    fn lan_split_ignores_issuer_domains() {
        let _guard = env_lock().lock().unwrap();
        let keys = [
            "PAPERRUST_PUBLIC_URLS",
            "PAPERRUST_OIDC_ISSUERS",
            "PAPERRUST_OIDC_ISSUER",
            "PAPERRUST_LAN_URLS",
        ];
        let prev: Vec<Option<String>> = keys.iter().map(|k| std::env::var(k).ok()).collect();
        unsafe {
            std::env::set_var("PAPERRUST_PUBLIC_URLS", "https://beta.archive.lan");
            std::env::set_var("PAPERRUST_OIDC_ISSUERS", "https://beta.archive.lan=https://auth.lan");
            std::env::set_var("PAPERRUST_OIDC_ISSUER", "https://auth.example/application/o/x/");
            std::env::remove_var("PAPERRUST_LAN_URLS");
        }
        let o = find_public_url("beta.archive.lan").unwrap();
        // Not marked: public (fail-closed), whatever the issuers share.
        assert!(!origin_is_direct_lan(&o));
        unsafe { std::env::set_var("PAPERRUST_LAN_URLS", "https://beta.archive.lan") };
        assert!(origin_is_direct_lan(&o));
        assert!(validate_issuer_config().is_ok());
        // Sharing a label with the default issuer no longer makes an origin LAN.
        unsafe {
            std::env::set_var("PAPERRUST_PUBLIC_URLS", "https://beta.archive.lan,https://archive.example");
            std::env::set_var(
                "PAPERRUST_OIDC_ISSUERS",
                "https://beta.archive.lan=https://auth.lan,https://archive.example=https://auth.example",
            );
        }
        let public = find_public_url("archive.example").unwrap();
        assert!(!origin_is_direct_lan(&public));
        // A LAN entry not listed in PUBLIC_URLS is a startup error.
        unsafe { std::env::set_var("PAPERRUST_LAN_URLS", "https://other.lan") };
        assert!(validate_issuer_config().unwrap_err().contains("PAPERRUST_LAN_URLS"));
        unsafe { std::env::set_var("PAPERRUST_LAN_URLS", "not an origin") };
        assert!(validate_issuer_config().is_err());
        unsafe {
            for (key, value) in keys.iter().zip(prev) {
                match value {
                    Some(v) => std::env::set_var(key, v),
                    None => std::env::remove_var(key),
                }
            }
        }
    }
}
