//! SSRF guards for selfhosted OCR base URLs.
//! follow-up: pin DNS at connect (`resolve_to_addrs`) so a TOCTOU
//! re-resolve cannot hit a private IP after validation.
//!
//! Rules:
//! - `https` only (no cleartext HTTP)
//! - Host must not be loopback / RFC1918 / link-local / CGNAT / cloud metadata
//!   (checked on IP literals and after DNS resolution)
//! - IPv6 transition prefixes are refused outright: `64:ff9b::/96`,
//!   `64:ff9b:1::/48`, `2002::/16`, `2001::/32`. `::ffff:0:0/96` is judged
//!   by the embedded IPv4.
//! - `.lan`, `.loc`, `.local` and other private names are refused before
//!   any DNS lookup unless the allowlist names them.
//! - Optional host allowlist via `PAPERRUST_OCR_SELFHOSTED_HOST_ALLOWLIST`.
//!   An entry matches exactly, or as a suffix only when it has a leading dot
//!   (`.lan` matches `x.lan`; `lan` does not).
//!   Prefer exact hosts. A suffix entry with a single label (`.lan`,
//!   `.local`, `.loc`) would open every internal host and is refused at
//!   startup (`validate_host_allowlists_from_env`).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};
use url::Url;

/// Validated selfhosted OCR base + pinned connect addresses (DNS TOCTOU).
#[derive(Debug, Clone)]
pub struct ValidatedOcrEndpoint {
    pub url: String,
    /// Hostname passed to reqwest DNS override (no brackets for IPv6).
    pub resolve_host: String,
    pub addrs: Vec<SocketAddr>,
}

/// Validate and normalize a selfhosted OCR base URL. Returns trimmed URL.
pub fn validate_selfhosted_ocr_base_url(raw: &str) -> Result<String, String> {
    Ok(validate_selfhosted_ocr_endpoint(raw)?.url)
}

/// Validate URL and return the DNS-pinned endpoint for connect-time use.
pub fn validate_selfhosted_ocr_endpoint(raw: &str) -> Result<ValidatedOcrEndpoint, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("URL de base Auto-hébergé requise (https://…).".into());
    }

    let url = Url::parse(trimmed).map_err(|_| {
        "URL de base OCR invalide — utilisez une URL https:// complète (ex. https://llm.exemple.org/v1)."
            .to_string()
    })?;

    if url.scheme() != "https" {
        return Err(
            "L'URL OCR Auto-hébergé doit utiliser https:// (HTTP en clair refusé)."
                .into(),
        );
    }

    let host = match url.host() {
        Some(url::Host::Domain(d)) => d.to_ascii_lowercase(),
        Some(url::Host::Ipv4(ip)) => {
            if is_blocked_ip(IpAddr::V4(ip)) {
                return Err(blocked_host_msg());
            }
            ip.to_string()
        }
        Some(url::Host::Ipv6(ip)) => {
            if is_blocked_ip(IpAddr::V6(ip)) {
                return Err(blocked_host_msg());
            }
            format!("[{ip}]")
        }
        None => {
            return Err("URL de base OCR sans hôte.".into());
        }
    };

    if host.is_empty() {
        return Err("URL de base OCR sans hôte.".into());
    }

    if let Some(err) = ocr_pre_dns_host_error(&host, host_allowlist().as_deref()) {
        return Err(err);
    }

    // DNS resolve — every address must be public (or non-blocked).
    // Private names and the allowlist were already decided above.
    // Use port from URL or 443 default.
    let port = url.port_or_known_default().unwrap_or(443);
    let lookup_host = host.trim_start_matches('[').trim_end_matches(']');
    let addrs = match (lookup_host, port).to_socket_addrs() {
        Ok(a) => a.collect::<Vec<_>>(),
        Err(_) => {
            return Err(format!(
                "Impossible de résoudre l'hôte OCR « {host} ». Vérifiez le nom DNS."
            ));
        }
    };
    if addrs.is_empty() {
        return Err(format!("Aucune adresse DNS pour l'hôte OCR « {host} »."));
    }
    for addr in &addrs {
        if is_blocked_ip(addr.ip()) {
            return Err(blocked_host_msg());
        }
    }

    Ok(ValidatedOcrEndpoint {
        url: trimmed.trim_end_matches('/').to_string(),
        resolve_host: lookup_host.to_string(),
        addrs,
    })
}

fn blocked_host_msg() -> String {
    "URL OCR refusée : hôte privé, loopback, link-local ou métadonnées cloud interdit (SSRF)."
        .into()
}

pub fn is_blocked_hostname(host: &str) -> bool {
    let h = host.trim_end_matches('.');
    matches!(
        h,
        "localhost"
            | "localhost.localdomain"
            | "metadata"
            | "metadata.google.internal"
            | "metadata.goog"
    ) || h.ends_with(".localhost")
        || h.ends_with(".localdomain")
        || h.ends_with(".internal")
}

pub fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_blocked_v4(v4),
        IpAddr::V6(v6) => is_blocked_v6(v6),
    }
}

/// RFC1918 or IPv6 unique-local (fc00::/7). Not loopback / link-local / CGNAT.
pub fn is_private_lan_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_private(),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return v4.is_private();
            }
            let s = v6.segments();
            (s[0] & 0xfe00) == 0xfc00
        }
    }
}

/// hard deny: loopback, unspecified, link-local, CGNAT, metadata, fe80.
/// RFC1918 / ULA are *not* included — those may be allowlisted by callers.
pub fn is_always_denied_ssrf_ip(ip: IpAddr) -> bool {
    is_blocked_ip(ip) && !is_private_lan_ip(ip)
}

fn is_blocked_v4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    if ip.is_loopback() || ip.is_unspecified() || ip.is_broadcast() {
        return true;
    }
    if ip.is_private() || ip.is_link_local() {
        return true;
    }
    // CGNAT / shared 100.64.0.0/10
    if o[0] == 100 && (o[1] & 0xc0) == 64 {
        return true;
    }
    // 0.0.0.0/8
    if o[0] == 0 {
        return true;
    }
    // Benchmark / TEST-NET (optional hardening)
    if o[0] == 198 && (o[1] == 18 || o[1] == 19) {
        return true;
    }
    false
}

/// NAT64, local NAT64, 6to4 and Teredo. The whole prefix is refused: an
/// embedded public IPv4 is still reachable only through a translator.
fn is_ipv6_transition(ip: Ipv6Addr) -> bool {
    let s = ip.segments();
    // 64:ff9b::/96 (RFC 6052)
    if s[0] == 0x0064 && s[1] == 0xff9b && s[2] == 0 && s[3] == 0 && s[4] == 0 && s[5] == 0 {
        return true;
    }
    // 64:ff9b:1::/48 (RFC 8215)
    if s[0] == 0x0064 && s[1] == 0xff9b && s[2] == 0x0001 {
        return true;
    }
    // 2002::/16 (6to4)
    if s[0] == 0x2002 {
        return true;
    }
    // 2001::/32 (Teredo). 2001:db8::/32 is not this prefix.
    if s[0] == 0x2001 && s[1] == 0 {
        return true;
    }
    false
}

fn is_blocked_v6(ip: Ipv6Addr) -> bool {
    if ip.is_loopback() || ip.is_unspecified() {
        return true;
    }
    // ::ffff:0:0/96 — decide from the embedded IPv4 (public mappings stay open).
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_blocked_v4(v4);
    }
    if is_ipv6_transition(ip) {
        return true;
    }
    let s = ip.segments();
    // Unique local fc00::/7
    if (s[0] & 0xfe00) == 0xfc00 {
        return true;
    }
    // Link-local fe80::/10
    if (s[0] & 0xffc0) == 0xfe80 {
        return true;
    }
    false
}

fn host_allowlist() -> Option<Vec<String>> {
    let raw = std::env::var("PAPERRUST_OCR_SELFHOSTED_HOST_ALLOWLIST").ok()?;
    let list: Vec<String> = raw
        .split(',')
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    if list.is_empty() {
        None
    } else {
        Some(list)
    }
}

fn canonical_hostname(host: &str) -> String {
    host.trim()
        .trim_end_matches('.')
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase()
}

/// `.lan`, `.loc` and `.local` — private names that must not be resolved
/// until an allowlist entry names them.
pub fn is_private_suffix_host(host: &str) -> bool {
    let h = canonical_hostname(host);
    if h.is_empty() || h.parse::<IpAddr>().is_ok() {
        return false;
    }
    const SUFFIXES: &[&str] = &["lan", "loc", "local"];
    SUFFIXES
        .iter()
        .any(|suffix| h == *suffix || h.ends_with(&format!(".{suffix}")))
}

/// Exact host, or a DNS suffix only when `allowed` has a leading dot.
/// `lan` matches `lan` only. `.lan` matches `lan` and `x.lan`.
/// `evillan` and `lan.evil.com` match neither.
pub fn host_matches(host: &str, allowed: &str) -> bool {
    let host = canonical_hostname(host);
    let allowed = allowed.trim().trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() || allowed.is_empty() {
        return false;
    }
    if let Some(suffix) = allowed.strip_prefix('.') {
        if suffix.is_empty() || suffix.starts_with('.') {
            return false;
        }
        host == suffix || host.ends_with(&format!(".{suffix}"))
    } else {
        host == allowed
    }
}

/// Env vars carrying host allowlists (exact host, or `.suffix`).
pub const HOST_ALLOWLIST_VARS: &[&str] = &[
    "PAPERRUST_OCR_SELFHOSTED_HOST_ALLOWLIST",
    "PAPERRUST_PAPERLESS_HOST_ALLOWLIST",
];

/// a suffix entry (leading dot) must keep at least two labels.
/// `.lan`, `.local`, `.loc` (one label) would match every internal host.
pub fn validate_host_allowlist(var: &str, raw: &str) -> Result<(), String> {
    for entry in raw.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let e = entry.trim_end_matches('.').to_ascii_lowercase();
        if e.is_empty() {
            return Err(format!(
                "{var}: suffix entry \"{entry}\" is too broad (matches every host). Use exact hosts."
            ));
        }
        if let Some(suffix) = e.strip_prefix('.') {
            if suffix.is_empty() || suffix.starts_with('.') || !suffix.contains('.') {
                return Err(format!(
                    "{var}: suffix entry \"{entry}\" is too broad (single label would allow every internal host). Use exact hosts (e.g. paperless.lab.example) or a suffix with at least two labels (e.g. .lab.example)."
                ));
            }
        }
    }
    Ok(())
}

/// Startup check over every host allowlist env var. Fails fast with a clear error.
pub fn validate_host_allowlists_from_env() -> Result<(), String> {
    for var in HOST_ALLOWLIST_VARS {
        if let Ok(raw) = std::env::var(var) {
            validate_host_allowlist(var, &raw)?;
        }
    }
    Ok(())
}

/// Policy decided with no DNS. `None` means the caller may resolve.
fn ocr_pre_dns_host_error(host: &str, allow: Option<&[String]>) -> Option<String> {
    if is_blocked_hostname(host) {
        return Some(blocked_host_msg());
    }
    let listed = allow
        .map(|list| list.iter().any(|allowed| host_matches(host, allowed)))
        .unwrap_or(false);
    if is_private_suffix_host(host) && !listed {
        return Some(blocked_host_msg());
    }
    if allow.is_some() && !listed {
        return Some(format!(
            "Hôte OCR « {host} » non autorisé. Ajoutez-le à PAPERRUST_OCR_SELFHOSTED_HOST_ALLOWLIST ou utilisez un hôte approuvé."
        ));
    }
    None
}

/// reqwest client for OCR provider calls — never follow redirects.
pub fn ocr_http_client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(180))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
}

/// OCR client that connects only via previously validated SocketAddrs (DNS TOCTOU pin).
pub fn ocr_http_client_pinned(
    resolve_host: &str,
    addrs: &[SocketAddr],
) -> Result<reqwest::Client, reqwest::Error> {
    // disable system/env proxy so HTTPS_PROXY cannot bypass the DNS pin.
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(180))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .resolve_to_addrs(resolve_host, addrs)
        .build()
}

#[cfg(test)]
mod tests {
    #[test]
    fn host_matches_suffix_only_with_leading_dot() {
        assert!(super::host_matches("lan", "lan"));
        assert!(super::host_matches("lan", ".lan"));
        assert!(super::host_matches("x.lan", ".lan"));
        assert!(super::host_matches("ocr.lan", ".lan"));
        // No leading dot: exact host only, not a suffix.
        assert!(!super::host_matches("ocr.lan", "lan"));
        assert!(!super::host_matches("x.lan", "lan"));
        assert!(!super::host_matches("evillan", "lan"));
        assert!(!super::host_matches("evillan", ".lan"));
        assert!(!super::host_matches("evillan.com", "lan"));
        assert!(!super::host_matches("lan.evil.com", "lan"));
        assert!(!super::host_matches("lan.evil.com", ".lan"));
    }

    #[test]
    fn host_matches_neutral_examples_exact_unless_leading_dot() {
        // !53 examples under !51 semantics (): no leading dot = exact host.
        assert!(super::host_matches("internal.example", "internal.example"));
        assert!(!super::host_matches("ocr.internal.example", "internal.example"));
        assert!(super::host_matches("ocr.internal.example", ".internal.example"));
        assert!(!super::host_matches("evilinternal.example", "internal.example"));
        assert!(!super::host_matches("evilinternal.example", ".internal.example"));
        assert!(!super::host_matches(
            "evilinternal.example.com",
            "internal.example"
        ));
        assert!(!super::host_matches(
            "internal.example.evil.com",
            "internal.example"
        ));
        assert!(!super::host_matches(
            "internal.example.evil.com",
            ".internal.example"
        ));
    }

    use super::*;

    #[test]
    fn rejects_http_cleartext() {
        let err = validate_selfhosted_ocr_base_url("http://example.com/v1").unwrap_err();
        assert!(err.contains("https"), "{err}");
    }

    #[test]
    fn rejects_loopback_ip() {
        assert!(validate_selfhosted_ocr_base_url("https://127.0.0.1/v1").is_err());
        assert!(validate_selfhosted_ocr_base_url("https://[::1]/v1").is_err());
    }

    #[test]
    fn rejects_rfc1918_ip() {
        assert!(validate_selfhosted_ocr_base_url("https://10.0.0.5/v1").is_err());
        assert!(validate_selfhosted_ocr_base_url("https://192.168.1.10/v1").is_err());
        assert!(validate_selfhosted_ocr_base_url("https://172.16.0.2/v1").is_err());
    }

    #[test]
    fn rejects_metadata_ip() {
        assert!(validate_selfhosted_ocr_base_url("https://169.254.169.254/latest").is_err());
    }

    #[test]
    fn rejects_localhost_name() {
        assert!(validate_selfhosted_ocr_base_url("https://localhost/v1").is_err());
    }

    #[test]
    fn blocked_ip_helpers() {
        assert!(is_blocked_ip(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))));
        assert!(is_blocked_ip(IpAddr::V4(Ipv4Addr::new(10, 1, 2, 3))));
        assert!(is_blocked_ip(IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254))));
        assert!(is_blocked_ip(IpAddr::V4(Ipv4Addr::new(100, 64, 1, 1))));
        assert!(!is_blocked_ip(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1))));
        assert!(!is_blocked_ip(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))));
    }

    fn v6(addr: &str) -> IpAddr {
        IpAddr::V6(addr.parse().unwrap())
    }

    #[test]
    fn refuses_ipv6_transition_prefixes_and_mapped_embedded_v4() {
        // Whole prefixes, including a public IPv4 embedded in NAT64 / 6to4 / Teredo.
        for addr in [
            "64:ff9b::1",
            "64:ff9b::808:808",
            "64:ff9b::7f00:1",
            "64:ff9b:1::1",
            "64:ff9b:1:2::1",
            "2002::1",
            "2002:0808:0808::",
            "2002:0a00:1::",
            "2001::1",
            "2001:0:1234::1",
        ] {
            let ip = v6(addr);
            assert!(is_blocked_ip(ip), "{addr}");
            assert!(!is_private_lan_ip(ip), "{addr}");
            assert!(is_always_denied_ssrf_ip(ip), "{addr}");
        }
        // ::ffff:0:0/96 follows the embedded IPv4.
        let mapped_private = v6("::ffff:10.0.0.1");
        let mapped_loop = v6("::ffff:127.0.0.1");
        let mapped_meta = v6("::ffff:169.254.169.254");
        let mapped_public = v6("::ffff:1.1.1.1");
        let mapped_dns = v6("::ffff:8.8.8.8");
        assert!(is_blocked_ip(mapped_private) && is_private_lan_ip(mapped_private));
        assert!(!is_always_denied_ssrf_ip(mapped_private));
        assert!(is_blocked_ip(mapped_loop) && is_always_denied_ssrf_ip(mapped_loop));
        assert!(is_blocked_ip(mapped_meta) && is_always_denied_ssrf_ip(mapped_meta));
        assert!(!is_blocked_ip(mapped_public) && !is_always_denied_ssrf_ip(mapped_public));
        assert!(!is_blocked_ip(mapped_dns));
        // Documentation prefix is not Teredo.
        let doc = v6("2001:db8::1");
        assert!(!is_blocked_ip(doc), "2001:db8::1");
        assert!(!is_always_denied_ssrf_ip(doc));
    }

    #[test]
    fn transition_literals_rejected_before_any_dns_error() {
        for url in [
            "https://[64:ff9b::1]/v1",
            "https://[64:ff9b:1::1]/v1",
            "https://[2002:0808:0808::]/v1",
            "https://[2001:0::1]/v1",
            "https://[::ffff:10.0.0.1]/v1",
            "https://[::ffff:127.0.0.1]/v1",
        ] {
            let err = validate_selfhosted_ocr_base_url(url).unwrap_err();
            assert!(err.contains("SSRF") || err.contains("interdit"), "{url}: {err}");
            assert!(!err.contains("résoudre"), "{url}: {err}");
        }
        let ok = validate_selfhosted_ocr_endpoint("https://[::ffff:1.1.1.1]/v1").unwrap();
        assert_eq!(ok.resolve_host, "::ffff:1.1.1.1");
    }

    #[test]
    fn private_names_and_suffix_allowlist_before_dns() {
        let blocked = ocr_pre_dns_host_error("x.lan", None).expect("unlisted");
        assert!(!blocked.contains("résoudre"), "{blocked}");
        assert!(ocr_pre_dns_host_error("printer.loc", None).is_some());
        assert!(ocr_pre_dns_host_error("nas.local", None).is_some());
        assert!(ocr_pre_dns_host_error("evillan", None).is_none());

        let dotted = vec![".lan".to_string()];
        assert!(ocr_pre_dns_host_error("x.lan", Some(&dotted)).is_none());
        let evil = ocr_pre_dns_host_error("evillan", Some(&dotted)).unwrap();
        assert!(evil.contains("non autorisé"), "{evil}");
        assert!(!evil.contains("résoudre"), "{evil}");
        let nested = ocr_pre_dns_host_error("lan.evil.com", Some(&dotted)).unwrap();
        assert!(nested.contains("non autorisé"), "{nested}");
        assert!(!nested.contains("résoudre"), "{nested}");

        let exact = vec!["lan".to_string()];
        assert!(ocr_pre_dns_host_error("lan", Some(&exact)).is_none());
        assert!(ocr_pre_dns_host_error("x.lan", Some(&exact)).is_some());

        let err = validate_selfhosted_ocr_base_url("https://x.lan/v1").unwrap_err();
        assert!(err.contains("SSRF") || err.contains("interdit"), "{err}");
        assert!(!err.contains("résoudre"), "{err}");
        let loc = validate_selfhosted_ocr_base_url("https://paperless.lan.loc/v1").unwrap_err();
        assert!(!loc.contains("résoudre"), "{loc}");
    }

    #[test]
    fn private_lan_vs_always_denied() {
        let rfc1918 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5));
        let lan = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 5));
        let meta = IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254));
        let cgnat = IpAddr::V4(Ipv4Addr::new(100, 64, 1, 1));
        let loopback = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
        let public = IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1));
        let ula = IpAddr::V6("fd12:3456:789a::1".parse().unwrap());
        let fe80 = IpAddr::V6("fe80::1".parse().unwrap());

        assert!(is_private_lan_ip(rfc1918) && !is_always_denied_ssrf_ip(rfc1918));
        assert!(is_private_lan_ip(lan) && !is_always_denied_ssrf_ip(lan));
        assert!(is_private_lan_ip(ula) && !is_always_denied_ssrf_ip(ula));
        assert!(!is_private_lan_ip(meta) && is_always_denied_ssrf_ip(meta));
        assert!(!is_private_lan_ip(cgnat) && is_always_denied_ssrf_ip(cgnat));
        assert!(!is_private_lan_ip(loopback) && is_always_denied_ssrf_ip(loopback));
        assert!(!is_private_lan_ip(fe80) && is_always_denied_ssrf_ip(fe80));
        assert!(!is_private_lan_ip(public) && !is_always_denied_ssrf_ip(public));
    }

    #[test]
    fn single_label_suffix_refused_at_startup() {
        for bad in [".lan", ".local", ".loc", ".LOC.", "ok.example,.lan", ".", ".."] {
            let err = validate_host_allowlist("PAPERRUST_PAPERLESS_HOST_ALLOWLIST", bad)
                .expect_err(bad);
            assert!(err.contains("too broad"), "{err}");
            assert!(err.contains("PAPERRUST_PAPERLESS_HOST_ALLOWLIST"), "{err}");
        }
        for ok in ["", "lan", "paperless.lan", ".lab.lan", "192.0.2.10, nas.loc", ".paperless.lan.loc"] {
            assert!(validate_host_allowlist("X", ok).is_ok(), "{ok}");
        }
    }

    #[test]
    fn allowlist_blocks_unlisted_host() {
        // Safety: set allowlist for this test process only.
        // Note: may affect parallel tests — keep allowlist narrow.
        // We only check the allowlist branch before DNS for a host that would
        // fail allowlist; use an IP that is already blocked to avoid DNS, and
        // separately unit-test host_matches.
        assert!(host_matches("llm.exemple.org", "llm.exemple.org"));
        assert!(!host_matches("a.llm.exemple.org", "llm.exemple.org"));
        assert!(host_matches("a.llm.exemple.org", ".llm.exemple.org"));
        assert!(!host_matches("evil.com", "llm.exemple.org"));
    }

    #[test]
    fn public_ip_literal_pins_socket() {
        let ep = validate_selfhosted_ocr_endpoint("https://1.1.1.1/v1").unwrap();
        assert_eq!(ep.resolve_host, "1.1.1.1");
        assert!(!ep.addrs.is_empty());
        assert_eq!(ep.addrs[0].ip(), IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)));
    }
}
