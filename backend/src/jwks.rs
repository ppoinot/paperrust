//! OIDC JWKS fetch + JWT signature verification (RS256).
//! Used for OIDC `id_token` (login callback) and `access_token` (Bearer API).
//! Cache TTL ~1h; one forced refresh on unknown kid or bad signature.

use anyhow::{Context, anyhow, bail};
use jsonwebtoken::errors::ErrorKind;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const JWKS_TTL: Duration = Duration::from_secs(60 * 60);
const CLOCK_SKEW_SECS: u64 = 60;

#[derive(Debug, Clone, Deserialize)]
struct JwksDoc {
    keys: Vec<Jwk>,
}

#[derive(Debug, Clone, Deserialize)]
struct Jwk {
    #[serde(default)]
    kid: Option<String>,
    kty: String,
    #[serde(default)]
    alg: Option<String>,
    #[serde(default, rename = "use")]
    key_use: Option<String>,
    #[serde(default)]
    n: Option<String>,
    #[serde(default)]
    e: Option<String>,
}

impl Jwk {
    fn use_sig(&self) -> bool {
        self.key_use
            .as_deref()
            .unwrap_or("sig")
            .eq_ignore_ascii_case("sig")
    }
}

struct CacheEntry {
    fetched_at: Instant,
    keys: Vec<Jwk>,
}

fn cache() -> &'static Mutex<HashMap<String, CacheEntry>> {
    static CACHE: OnceLock<Mutex<HashMap<String, CacheEntry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Test helper: clear in-process JWKS cache.
#[cfg(test)]
pub fn clear_cache_for_tests() {
    if let Ok(mut g) = cache().lock() {
        g.clear();
    }
}

/// True when `PAPERRUST_OIDC_CA_BUNDLE` is set. The bundle is used for every
/// OIDC request; there is no issuer-name detection.
pub fn oidc_ca_bundle_configured() -> bool {
    crate::instance::oidc_ca_bundle_path().is_some()
}

/// OIDC HTTP client. A configured PEM bundle is added to the trust store.
/// Never uses `danger_accept_invalid_certs`.
pub fn oidc_http_client(_url_or_issuer: &str) -> anyhow::Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(4))
        .timeout(Duration::from_secs(10));
    if let Some(path) = crate::instance::oidc_ca_bundle_path() {
        let pem = std::fs::read(&path)
            .with_context(|| format!("read PAPERRUST_OIDC_CA_BUNDLE ({path})"))?;
        let certs = reqwest::Certificate::from_pem_bundle(&pem)
            .context("parse PAPERRUST_OIDC_CA_BUNDLE PEM")?;
        if certs.is_empty() {
            bail!("PAPERRUST_OIDC_CA_BUNDLE has no certificates");
        }
        for cert in certs {
            builder = builder.add_root_certificate(cert);
        }
    }
    Ok(builder.build()?)
}

fn jwk_to_decoding_key(jwk: &Jwk) -> anyhow::Result<DecodingKey> {
    if !jwk.kty.eq_ignore_ascii_case("RSA") {
        bail!("unsupported kty {}", jwk.kty);
    }
    let n = jwk.n.as_deref().ok_or_else(|| anyhow!("jwk missing n"))?;
    let e = jwk.e.as_deref().ok_or_else(|| anyhow!("jwk missing e"))?;
    DecodingKey::from_rsa_components(n, e).context("DecodingKey from RSA components")
}

async fn fetch_jwks(jwks_uri: &str) -> anyhow::Result<Vec<Jwk>> {
    let client = oidc_http_client(jwks_uri)?;
    let res = client
        .get(jwks_uri)
        .header("accept", "application/json")
        .send()
        .await
        .with_context(|| format!("jwks GET {jwks_uri}"))?;
    if !res.status().is_success() {
        bail!("jwks HTTP {}", res.status());
    }
    let doc: JwksDoc = res.json().await.context("jwks JSON")?;
    if doc.keys.is_empty() {
        bail!("jwks empty");
    }
    Ok(doc.keys)
}

async fn keys_for(jwks_uri: &str, force: bool) -> anyhow::Result<Vec<Jwk>> {
    if !force {
        if let Ok(guard) = cache().lock() {
            if let Some(ent) = guard.get(jwks_uri) {
                if ent.fetched_at.elapsed() < JWKS_TTL {
                    return Ok(ent.keys.clone());
                }
            }
        }
    }
    let keys = fetch_jwks(jwks_uri).await?;
    if let Ok(mut guard) = cache().lock() {
        guard.insert(jwks_uri.to_string(), CacheEntry {
            fetched_at: Instant::now(),
            keys: keys.clone(),
        });
    }
    Ok(keys)
}

/// Select a JWKS key by kid. Refuse missing kid when the JWKS set has kids;
/// never blindly accept a single key.
fn find_key<'a>(keys: &'a [Jwk], kid: Option<&str>) -> Option<&'a Jwk> {
    let sig_keys: Vec<_> = keys.iter().filter(|k| k.use_sig()).collect();
    let pool: Vec<_> = if sig_keys.is_empty() {
        keys.iter().collect()
    } else {
        sig_keys
    };
    let Some(kid) = kid.filter(|s| !s.is_empty()) else {
        // No kid in token: refuse (JWKS with kids must match; never fall back to a single key).
        return None;
    };
    pool.iter().find(|k| k.kid.as_deref() == Some(kid)).copied()
}

fn build_validation(issuer: &str, audience: &str) -> Validation {
    let mut validation = Validation::new(Algorithm::RS256);
    validation.validate_exp = true;
    validation.validate_nbf = true;
    validation.leeway = CLOCK_SKEW_SECS;
    let mut issuers = vec![issuer.to_string()];
    let trimmed = issuer.trim_end_matches('/').to_string();
    if trimmed != *issuer && trimmed != issuer.trim() {
        issuers.push(trimmed);
    } else if !issuer.ends_with('/') {
        issuers.push(format!("{issuer}/"));
    } else if issuer.ends_with('/') {
        issuers.push(trimmed);
    }
    issuers.sort();
    issuers.dedup();
    validation.set_issuer(&issuers);
    validation.set_audience(&[audience.to_string()]);
    // Require iss/aud/exp/sub. Some issuers omit `nbf` — do not require it.
    // `validate_nbf = true` still enforces not-before when the claim is present.
    validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
    validation
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Validate `iat` (required) with the same 60s leeway as exp/nbf.
fn validate_iat(claims: &serde_json::Value) -> anyhow::Result<()> {
    let iat = claims
        .get("iat")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| anyhow!("missing iat"))?;
    let now = now_secs();
    if iat > now.saturating_add(CLOCK_SKEW_SECS) {
        bail!("iat in the future");
    }
    Ok(())
}

fn is_invalid_signature(err: &jsonwebtoken::errors::Error) -> bool {
    matches!(err.kind(), ErrorKind::InvalidSignature)
}

/// OIDC access_token `aud` is often the provider / issuer, with the
/// public client in `azp`. Accept either claim containing `client_id`.
pub fn audience_or_azp_matches(claims: &serde_json::Value, client_id: &str) -> bool {
    if client_id.is_empty() {
        return false;
    }
    if aud_contains(claims.get("aud"), client_id) {
        return true;
    }
    claims
        .get("azp")
        .and_then(|v| v.as_str())
        .is_some_and(|azp| azp == client_id)
}

fn aud_contains(aud: Option<&serde_json::Value>, client_id: &str) -> bool {
    match aud {
        Some(serde_json::Value::String(s)) => s == client_id,
        Some(serde_json::Value::Array(items)) => {
            items.iter().any(|v| v.as_str() == Some(client_id))
        }
        _ => false,
    }
}

fn issuer_slash_aliases(issuers: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for issuer in issuers {
        let trimmed = issuer.trim().trim_end_matches('/').to_string();
        if trimmed.is_empty() {
            continue;
        }
        out.push(trimmed.clone());
        out.push(format!("{trimmed}/"));
    }
    out.sort();
    out.dedup();
    out
}

fn build_access_validation(issuers: &[String]) -> Validation {
    let mut validation = Validation::new(Algorithm::RS256);
    validation.validate_exp = true;
    validation.validate_nbf = true;
    // Access tokens may put the mobile client in `azp` instead of `aud`.
    validation.validate_aud = false;
    validation.leeway = CLOCK_SKEW_SECS;
    let issuers = issuer_slash_aliases(issuers);
    validation.set_issuer(&issuers);
    // Require iss/exp/sub. Do not require `aud` (azp fallback) or `nbf` / `iat`.
    validation.set_required_spec_claims(&["exp", "iss", "sub"]);
    validation
}

/// Validate `iat` when present (same 60s leeway). Access tokens may omit it.
fn validate_iat_optional(claims: &serde_json::Value) -> anyhow::Result<()> {
    let Some(iat_val) = claims.get("iat") else {
        return Ok(());
    };
    let iat = iat_val.as_u64().ok_or_else(|| anyhow!("invalid iat"))?;
    let now = now_secs();
    if iat > now.saturating_add(CLOCK_SKEW_SECS) {
        bail!("iat in the future");
    }
    Ok(())
}

fn decode_with_key(
    id_token: &str,
    jwk: &Jwk,
    issuer: &str,
    audience: &str,
) -> anyhow::Result<serde_json::Value> {
    let decoding = jwk_to_decoding_key(jwk)?;
    let validation = build_validation(issuer, audience);
    let data =
        decode::<serde_json::Value>(id_token, &decoding, &validation).map_err(|e| anyhow!(e))?;
    validate_iat(&data.claims)?;
    Ok(data.claims)
}

enum AttemptErr {
    NoKey,
    BadSig(anyhow::Error),
    Other(anyhow::Error),
}

fn attempt_verify(
    id_token: &str,
    keys: &[Jwk],
    kid: Option<&str>,
    issuer: &str,
    audience: &str,
) -> Result<serde_json::Value, AttemptErr> {
    let Some(jwk) = find_key(keys, kid) else {
        return Err(AttemptErr::NoKey);
    };
    let decoding = match jwk_to_decoding_key(jwk) {
        Ok(k) => k,
        Err(e) => return Err(AttemptErr::Other(e)),
    };
    let validation = build_validation(issuer, audience);
    match decode::<serde_json::Value>(id_token, &decoding, &validation) {
        Ok(data) => match validate_iat(&data.claims) {
            Ok(()) => Ok(data.claims),
            Err(e) => Err(AttemptErr::Other(e)),
        },
        Err(e) if is_invalid_signature(&e) => {
            Err(AttemptErr::BadSig(anyhow!(e).context("id_token signature")))
        }
        Err(e) => Err(AttemptErr::Other(
            anyhow!(e).context("id_token signature/claims"),
        )),
    }
}

/// Verify id_token signature (RS256 via JWKS) + iss/aud/exp/sub/iat.
/// `nbf` is optional; when present it is validated (not-before + leeway).
/// Returns the verified claims JSON (keeps nonce/sub for caller checks).
/// On unknown kid or bad signature: force JWKS refresh once, then retry.
pub async fn verify_id_token(
    id_token: &str,
    jwks_uri: &str,
    issuer: &str,
    audience: &str,
) -> anyhow::Result<serde_json::Value> {
    if jwks_uri.is_empty() {
        bail!("jwks_uri empty");
    }
    let header = decode_header(id_token).context("id_token header")?;
    if header.alg != Algorithm::RS256 {
        bail!("unsupported id_token alg {:?}", header.alg);
    }
    let kid = header.kid.clone();

    let keys = keys_for(jwks_uri, false).await?;
    match attempt_verify(id_token, &keys, kid.as_deref(), issuer, audience) {
        Ok(claims) => Ok(claims),
        Err(AttemptErr::NoKey) | Err(AttemptErr::BadSig(_)) => {
            let keys = keys_for(jwks_uri, true).await?;
            match attempt_verify(id_token, &keys, kid.as_deref(), issuer, audience) {
                Ok(claims) => Ok(claims),
                Err(AttemptErr::NoKey) => Err(anyhow!("no matching JWKS key for kid={kid:?}")),
                Err(AttemptErr::BadSig(e)) | Err(AttemptErr::Other(e)) => Err(e),
            }
        }
        Err(AttemptErr::Other(e)) => Err(e),
    }
}

fn finish_access_claims(
    claims: serde_json::Value,
    audience: &str,
) -> anyhow::Result<serde_json::Value> {
    validate_iat_optional(&claims)?;
    if !audience_or_azp_matches(&claims, audience) {
        bail!("access_token audience/azp mismatch");
    }
    Ok(claims)
}

fn attempt_verify_access(
    access_token: &str,
    keys: &[Jwk],
    kid: Option<&str>,
    issuers: &[String],
    audience: &str,
) -> Result<serde_json::Value, AttemptErr> {
    let Some(jwk) = find_key(keys, kid) else {
        return Err(AttemptErr::NoKey);
    };
    let decoding = match jwk_to_decoding_key(jwk) {
        Ok(k) => k,
        Err(e) => return Err(AttemptErr::Other(e)),
    };
    let validation = build_access_validation(issuers);
    match decode::<serde_json::Value>(access_token, &decoding, &validation) {
        Ok(data) => match finish_access_claims(data.claims, audience) {
            Ok(claims) => Ok(claims),
            Err(e) => Err(AttemptErr::Other(e)),
        },
        Err(e) if is_invalid_signature(&e) => Err(AttemptErr::BadSig(
            anyhow!(e).context("access_token signature"),
        )),
        Err(e) => Err(AttemptErr::Other(
            anyhow!(e).context("access_token signature/claims"),
        )),
    }
}

/// Verify OIDC `access_token` (RS256 via JWKS) + iss/exp/sub.
/// `aud` or `azp` must include `audience` (mobile client id).
/// `nbf` / `iat` are optional; when present they are validated (+ leeway).
/// On unknown kid or bad signature: force JWKS refresh once, then retry.
pub async fn verify_access_token(
    access_token: &str,
    jwks_uri: &str,
    issuers: &[String],
    audience: &str,
) -> anyhow::Result<serde_json::Value> {
    if jwks_uri.is_empty() {
        bail!("jwks_uri empty");
    }
    if issuers.is_empty() {
        bail!("no allowed issuers");
    }
    if audience.is_empty() {
        bail!("audience empty");
    }
    let header = decode_header(access_token).context("access_token header")?;
    if header.alg != Algorithm::RS256 {
        bail!("unsupported access_token alg {:?}", header.alg);
    }
    let kid = header.kid.clone();

    let keys = keys_for(jwks_uri, false).await?;
    match attempt_verify_access(access_token, &keys, kid.as_deref(), issuers, audience) {
        Ok(claims) => Ok(claims),
        Err(AttemptErr::NoKey) | Err(AttemptErr::BadSig(_)) => {
            let keys = keys_for(jwks_uri, true).await?;
            match attempt_verify_access(access_token, &keys, kid.as_deref(), issuers, audience) {
                Ok(claims) => Ok(claims),
                Err(AttemptErr::NoKey) => Err(anyhow!("no matching JWKS key for kid={kid:?}")),
                Err(AttemptErr::BadSig(e)) | Err(AttemptErr::Other(e)) => Err(e),
            }
        }
        Err(AttemptErr::Other(e)) => Err(e),
    }
}

/// Verify access_token with an explicit key set (unit tests; no network).
pub fn verify_access_token_with_keys(
    access_token: &str,
    keys: &[Jwk],
    issuers: &[String],
    audience: &str,
) -> anyhow::Result<serde_json::Value> {
    let header = decode_header(access_token).context("access_token header")?;
    if header.alg != Algorithm::RS256 {
        bail!("unsupported alg {:?}", header.alg);
    }
    match attempt_verify_access(access_token, keys, header.kid.as_deref(), issuers, audience) {
        Ok(claims) => Ok(claims),
        Err(AttemptErr::NoKey) => Err(anyhow!("no matching key")),
        Err(AttemptErr::BadSig(e)) | Err(AttemptErr::Other(e)) => Err(e),
    }
}

/// Build JWKS URI when discovery omitted it.
/// Bearer path: call only after `iss` matched the mobile allowlist.
pub fn fallback_jwks_uri(issuer: &str) -> String {
    let base = issuer.trim_end_matches('/');
    format!("{base}/jwks/")
}

/// RSA key generated once per test process. Not a checked-in private key.
#[cfg(test)]
struct TestRsa {
    pkcs1_pem: String,
    n_b64: String,
    e_b64: String,
}

#[cfg(test)]
fn test_rsa() -> &'static TestRsa {
    static KEY: OnceLock<TestRsa> = OnceLock::new();
    KEY.get_or_init(|| {
        use base64::Engine;
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        use rsa::pkcs1::EncodeRsaPrivateKey;
        use rsa::traits::PublicKeyParts;
        use rsa::RsaPrivateKey;
        let mut rng = rand::thread_rng();
        let key = RsaPrivateKey::new(&mut rng, 2048).expect("generate rsa");
        let pem = key
            .to_pkcs1_pem(rsa::pkcs8::LineEnding::LF)
            .expect("pkcs1 pem");
        let n_b64 = URL_SAFE_NO_PAD.encode(key.n().to_bytes_be());
        let e_b64 = URL_SAFE_NO_PAD.encode(key.e().to_bytes_be());
        TestRsa {
            pkcs1_pem: pem.to_string(),
            n_b64,
            e_b64,
        }
    })
}

/// Test helper: RS256-sign claims with a key generated for this process.
#[cfg(test)]
pub fn tests_sign_rs256(claims: serde_json::Value, kid: Option<&str>) -> String {
    use jsonwebtoken::{EncodingKey, Header, encode};
    let mut header = Header::new(Algorithm::RS256);
    header.kid = kid.map(|s| s.to_string());
    let key = EncodingKey::from_rsa_pem(test_rsa().pkcs1_pem.as_bytes()).expect("pem");
    encode(&header, &claims, &key).expect("sign")
}

/// Test helper: verify an access_token against the generated test JWKS (no network).
#[cfg(test)]
pub fn verify_access_token_with_test_keys(
    access_token: &str,
    issuers: &[String],
    audience: &str,
) -> anyhow::Result<serde_json::Value> {
    let material = test_rsa();
    let keys = vec![Jwk {
        kid: Some("test-kid".into()),
        kty: "RSA".into(),
        alg: Some("RS256".into()),
        key_use: Some("sig".into()),
        n: Some(material.n_b64.clone()),
        e: Some(material.e_b64.clone()),
    }];
    verify_access_token_with_keys(access_token, &keys, issuers, audience)
}

/// Verify with an explicit key set (unit tests; no network).
pub fn verify_id_token_with_keys(
    id_token: &str,
    keys: &[Jwk],
    issuer: &str,
    audience: &str,
) -> anyhow::Result<serde_json::Value> {
    let header = decode_header(id_token).context("id_token header")?;
    if header.alg != Algorithm::RS256 {
        bail!("unsupported alg {:?}", header.alg);
    }
    let jwk = find_key(keys, header.kid.as_deref()).ok_or_else(|| anyhow!("no matching key"))?;
    decode_with_key(id_token, jwk, issuer, audience)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{EncodingKey, Header, encode};
    use serde_json::json;

    fn test_keys(kid: &str) -> Vec<Jwk> {
        let material = test_rsa();
        vec![Jwk {
            kid: Some(kid.to_string()),
            kty: "RSA".into(),
            alg: Some("RS256".into()),
            key_use: Some("sig".into()),
            n: Some(material.n_b64.clone()),
            e: Some(material.e_b64.clone()),
        }]
    }

    fn sign_token(claims: serde_json::Value, kid: Option<&str>) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = kid.map(|s| s.to_string());
        let key = EncodingKey::from_rsa_pem(test_rsa().pkcs1_pem.as_bytes()).expect("pem");
        encode(&header, &claims, &key).expect("sign")
    }

    fn now() -> u64 {
        now_secs()
    }

    #[test]
    fn accepts_valid_rs256_id_token() {
        let iss = "https://auth.example/application/o/app/";
        let aud = "paperrust";
        let claims = json!({
            "iss": iss,
            "aud": aud,
            "sub": "user-1",
            "nonce": "abc",
            "exp": now() + 600,
            "iat": now(),
            "nbf": now() - 10,
        });
        let token = sign_token(claims, Some("test-kid"));
        let got = verify_id_token_with_keys(&token, &test_keys("test-kid"), iss, aud).unwrap();
        assert_eq!(got["sub"], "user-1");
        assert_eq!(got["nonce"], "abc");
    }

    #[test]
    fn rejects_wrong_audience() {
        let iss = "https://auth.example/application/o/app/";
        let claims = json!({
            "iss": iss,
            "aud": "other-client",
            "sub": "user-1",
            "exp": now() + 600,
            "iat": now(),
            "nbf": now() - 10,
        });
        let token = sign_token(claims, Some("test-kid"));
        assert!(
            verify_id_token_with_keys(&token, &test_keys("test-kid"), iss, "paperrust").is_err()
        );
    }

    #[test]
    fn rejects_expired() {
        let iss = "https://auth.example/application/o/app/";
        let aud = "paperrust";
        let claims = json!({
            "iss": iss,
            "aud": aud,
            "sub": "user-1",
            "exp": now() - 120,
            "iat": now() - 1000,
            "nbf": now() - 1000,
        });
        let token = sign_token(claims, Some("test-kid"));
        assert!(verify_id_token_with_keys(&token, &test_keys("test-kid"), iss, aud).is_err());
    }

    #[test]
    fn rejects_wrong_issuer() {
        let aud = "paperrust";
        let claims = json!({
            "iss": "https://evil.example/",
            "aud": aud,
            "sub": "user-1",
            "exp": now() + 600,
            "iat": now(),
            "nbf": now() - 10,
        });
        let token = sign_token(claims, Some("test-kid"));
        assert!(
            verify_id_token_with_keys(
                &token,
                &test_keys("test-kid"),
                "https://auth.example/application/o/app/",
                aud
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_tampered_signature() {
        let iss = "https://auth.example/application/o/app/";
        let aud = "paperrust";
        let claims = json!({
            "iss": iss,
            "aud": aud,
            "sub": "user-1",
            "exp": now() + 600,
            "iat": now(),
            "nbf": now() - 10,
        });
        let token = sign_token(claims, Some("test-kid"));
        let mut parts: Vec<String> = token.split('.').map(|s| s.to_string()).collect();
        assert_eq!(parts.len(), 3);
        let sig = &mut parts[2];
        let b = sig.pop().unwrap();
        sig.push(if b == 'A' { 'B' } else { 'A' });
        let tampered = parts.join(".");
        assert!(verify_id_token_with_keys(&tampered, &test_keys("test-kid"), iss, aud).is_err());
    }

    #[test]
    fn rejects_nbf_in_future_beyond_leeway() {
        let iss = "https://auth.example/application/o/app/";
        let aud = "paperrust";
        let claims = json!({
            "iss": iss,
            "aud": aud,
            "sub": "user-1",
            "exp": now() + 600,
            "iat": now(),
            "nbf": now() + 120,
        });
        let token = sign_token(claims, Some("test-kid"));
        assert!(verify_id_token_with_keys(&token, &test_keys("test-kid"), iss, aud).is_err());
    }

    #[test]
    fn accepts_missing_nbf() {
        // OIDC LAN id_tokens have no `nbf`.
        let iss = "https://auth.example/application/o/app/";
        let aud = "paperrust";
        let claims = json!({
            "iss": iss,
            "aud": aud,
            "sub": "user-1",
            "exp": now() + 600,
            "iat": now(),
        });
        let token = sign_token(claims, Some("test-kid"));
        assert!(
            verify_id_token_with_keys(&token, &test_keys("test-kid"), iss, aud).is_ok(),
            "id_token without nbf must verify"
        );
    }

    #[test]
    fn rejects_iat_in_future_beyond_leeway() {
        let iss = "https://auth.example/application/o/app/";
        let aud = "paperrust";
        let claims = json!({
            "iss": iss,
            "aud": aud,
            "sub": "user-1",
            "exp": now() + 600,
            "iat": now() + 120,
            "nbf": now() - 10,
        });
        let token = sign_token(claims, Some("test-kid"));
        assert!(verify_id_token_with_keys(&token, &test_keys("test-kid"), iss, aud).is_err());
    }

    #[test]
    fn rejects_missing_kid_when_jwks_has_kids() {
        let iss = "https://auth.example/application/o/app/";
        let aud = "paperrust";
        let claims = json!({
            "iss": iss,
            "aud": aud,
            "sub": "user-1",
            "exp": now() + 600,
            "iat": now(),
            "nbf": now() - 10,
        });
        let token = sign_token(claims, None);
        assert!(verify_id_token_with_keys(&token, &test_keys("test-kid"), iss, aud).is_err());
    }

    #[test]
    fn rejects_single_key_without_kid_match() {
        let iss = "https://auth.example/application/o/app/";
        let aud = "paperrust";
        let claims = json!({
            "iss": iss,
            "aud": aud,
            "sub": "user-1",
            "exp": now() + 600,
            "iat": now(),
            "nbf": now() - 10,
        });
        let token = sign_token(claims, Some("other-kid"));
        // Only one JWKS key, wrong kid — must not accept blindly.
        assert!(verify_id_token_with_keys(&token, &test_keys("test-kid"), iss, aud).is_err());
    }

    #[test]
    fn ca_bundle_follows_the_env_var_not_the_issuer_name() {
        let _guard = crate::instance::env_lock().lock().unwrap();
        let prev = std::env::var("PAPERRUST_OIDC_CA_BUNDLE").ok();
        unsafe {
            std::env::remove_var("PAPERRUST_OIDC_CA_BUNDLE");
        }
        assert!(!oidc_ca_bundle_configured());
        assert!(oidc_http_client("https://auth.example/jwks/").is_ok());
        unsafe {
            std::env::set_var("PAPERRUST_OIDC_CA_BUNDLE", "/no/such/bundle.pem");
        }
        assert!(oidc_ca_bundle_configured());
        assert!(oidc_http_client("https://auth.lan.example/jwks/").is_err());
        unsafe {
            match prev {
                Some(v) => std::env::set_var("PAPERRUST_OIDC_CA_BUNDLE", v),
                None => std::env::remove_var("PAPERRUST_OIDC_CA_BUNDLE"),
            }
        }
    }

    #[test]
    fn fallback_jwks_uri_appends_jwks() {
        assert_eq!(
            fallback_jwks_uri("https://auth.example/application/o/app/"),
            "https://auth.example/application/o/app/jwks/"
        );
    }

    fn access_issuers(iss: &str) -> Vec<String> {
        vec![iss.to_string()]
    }

    #[test]
    fn access_token_accepts_aud_client_id() {
        let iss = "https://auth.example/application/o/app/";
        let aud = "mobile";
        let claims = json!({
            "iss": iss,
            "aud": aud,
            "sub": "user-1",
            "exp": now() + 600,
            "iat": now(),
        });
        let token = sign_token(claims, Some("test-kid"));
        let got = verify_access_token_with_keys(
            &token,
            &test_keys("test-kid"),
            &access_issuers(iss),
            aud,
        )
        .unwrap();
        assert_eq!(got["sub"], "user-1");
    }

    #[test]
    fn access_token_accepts_azp_when_aud_is_provider() {
        let iss = "https://auth.lan.example/application/o/app/";
        let claims = json!({
            "iss": iss,
            "aud": iss,
            "azp": "mobile",
            "sub": "user-1",
            "exp": now() + 600,
        });
        let token = sign_token(claims, Some("test-kid"));
        let got = verify_access_token_with_keys(
            &token,
            &test_keys("test-kid"),
            &access_issuers(iss),
            "mobile",
        )
        .unwrap();
        assert_eq!(got["sub"], "user-1");
    }

    #[test]
    fn access_token_accepts_aud_array() {
        let iss = "https://auth.example/application/o/app/";
        let claims = json!({
            "iss": iss,
            "aud": ["app", "mobile"],
            "sub": "user-1",
            "exp": now() + 600,
        });
        let token = sign_token(claims, Some("test-kid"));
        assert!(
            verify_access_token_with_keys(
                &token,
                &test_keys("test-kid"),
                &access_issuers(iss),
                "mobile",
            )
            .is_ok()
        );
    }

    #[test]
    fn access_token_rejects_wrong_aud_and_azp() {
        let iss = "https://auth.example/application/o/app/";
        let claims = json!({
            "iss": iss,
            "aud": "paperrust",
            "azp": "paperrust",
            "sub": "user-1",
            "exp": now() + 600,
            "iat": now(),
        });
        let token = sign_token(claims, Some("test-kid"));
        assert!(
            verify_access_token_with_keys(
                &token,
                &test_keys("test-kid"),
                &access_issuers(iss),
                "mobile",
            )
            .is_err()
        );
    }

    #[test]
    fn access_token_rejects_expired() {
        let iss = "https://auth.example/application/o/app/";
        let claims = json!({
            "iss": iss,
            "aud": "mobile",
            "sub": "user-1",
            "exp": now() - 120,
            "iat": now() - 1000,
        });
        let token = sign_token(claims, Some("test-kid"));
        assert!(
            verify_access_token_with_keys(
                &token,
                &test_keys("test-kid"),
                &access_issuers(iss),
                "mobile",
            )
            .is_err()
        );
    }

    #[test]
    fn access_token_rejects_wrong_issuer() {
        let claims = json!({
            "iss": "https://evil.example/application/o/app/",
            "aud": "mobile",
            "sub": "user-1",
            "exp": now() + 600,
        });
        let token = sign_token(claims, Some("test-kid"));
        assert!(
            verify_access_token_with_keys(
                &token,
                &test_keys("test-kid"),
                &access_issuers("https://auth.example/application/o/app/"),
                "mobile",
            )
            .is_err()
        );
    }

    #[test]
    fn access_token_rejects_nbf_in_future() {
        let iss = "https://auth.example/application/o/app/";
        let claims = json!({
            "iss": iss,
            "aud": "mobile",
            "sub": "user-1",
            "exp": now() + 600,
            "nbf": now() + 120,
        });
        let token = sign_token(claims, Some("test-kid"));
        assert!(
            verify_access_token_with_keys(
                &token,
                &test_keys("test-kid"),
                &access_issuers(iss),
                "mobile",
            )
            .is_err()
        );
    }

    #[test]
    fn access_token_accepts_missing_nbf_and_iat() {
        let iss = "https://auth.example/application/o/app/";
        let claims = json!({
            "iss": iss,
            "azp": "mobile",
            "sub": "user-1",
            "exp": now() + 600,
        });
        let token = sign_token(claims, Some("test-kid"));
        assert!(
            verify_access_token_with_keys(
                &token,
                &test_keys("test-kid"),
                &access_issuers(iss),
                "mobile",
            )
            .is_ok()
        );
    }

    /// (e) alg≠RS256 → 401
    #[test]
    fn access_token_rejects_non_rs256_alg() {
        let iss = "https://auth.example/application/o/mobile/";
        let claims = json!({
            "iss": iss,
            "aud": "mobile",
            "sub": "user-1",
            "exp": now() + 600,
            "iat": now(),
        });
        let mut header = Header::new(Algorithm::HS256);
        header.kid = Some("test-kid".into());
        let token = encode(&header, &claims, &EncodingKey::from_secret(b"not-rsa")).expect("hs256");
        assert!(
            verify_access_token_with_keys(
                &token,
                &test_keys("test-kid"),
                &access_issuers(iss),
                "mobile",
            )
            .is_err(),
            "non-RS256 access_token must be rejected"
        );
    }

    #[test]
    fn audience_or_azp_matches_client() {
        assert!(audience_or_azp_matches(
            &json!({"aud": "mobile"}),
            "mobile"
        ));
        assert!(audience_or_azp_matches(
            &json!({"aud": ["other", "mobile"]}),
            "mobile"
        ));
        assert!(audience_or_azp_matches(
            &json!({"aud": "https://auth.example/", "azp": "mobile"}),
            "mobile"
        ));
        assert!(!audience_or_azp_matches(
            &json!({"aud": "paperrust", "azp": "paperrust"}),
            "mobile"
        ));
        assert!(!audience_or_azp_matches(&json!({}), "mobile"));
    }

    async fn serve_test_jwks() -> String {
        clear_cache_for_tests();
        let jwks = json!({
            "keys": [{
                "kty": "RSA",
                "kid": "test-kid",
                "alg": "RS256",
                "use": "sig",
                "n": test_rsa().n_b64,
                "e": test_rsa().e_b64,
            }]
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind jwks");
        let addr = listener.local_addr().expect("addr");
        let app = axum::Router::new().route(
            "/jwks/",
            axum::routing::get(move || {
                let body = jwks.clone();
                async move { axum::Json(body) }
            }),
        );
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        format!("http://{addr}/jwks/")
    }

    #[tokio::test]
    async fn access_token_valid_via_local_jwks() {
        let jwks_uri = serve_test_jwks().await;
        let iss = "https://auth.example/application/o/app/";
        let claims = json!({
            "iss": iss,
            "aud": "mobile",
            "sub": "user-1",
            "exp": now() + 600,
            "iat": now(),
        });
        let token = sign_token(claims, Some("test-kid"));
        let got = verify_access_token(&token, &jwks_uri, &[iss.to_string()], "mobile")
            .await
            .unwrap();
        assert_eq!(got["sub"], "user-1");
    }

    #[tokio::test]
    async fn access_token_expired_via_local_jwks() {
        let jwks_uri = serve_test_jwks().await;
        let iss = "https://auth.example/application/o/app/";
        let claims = json!({
            "iss": iss,
            "aud": "mobile",
            "sub": "user-1",
            "exp": now() - 120,
            "iat": now() - 1000,
        });
        let token = sign_token(claims, Some("test-kid"));
        assert!(
            verify_access_token(&token, &jwks_uri, &[iss.to_string()], "mobile",)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn access_token_wrong_aud_via_local_jwks() {
        let jwks_uri = serve_test_jwks().await;
        let iss = "https://auth.example/application/o/app/";
        let claims = json!({
            "iss": iss,
            "aud": "paperrust",
            "azp": "paperrust",
            "sub": "user-1",
            "exp": now() + 600,
        });
        let token = sign_token(claims, Some("test-kid"));
        assert!(
            verify_access_token(&token, &jwks_uri, &[iss.to_string()], "mobile",)
                .await
                .is_err()
        );
    }
}
