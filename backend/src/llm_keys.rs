//! Per-user LLM API keys for OCR (scoped by AuthUser id).
//!
//! Ciphertext is AES-256-GCM under `PAPERRUST_LLM_KEY_ENCRYPTION_KEY` (preferred)
//! or a SESSION_SECRET–derived key (dev fallback only). AAD binds
//! `user_id|provider`. Never return raw keys in list APIs. Legacy XOR blobs
//! (pre-v2) are decrypted only in the migrate path and rewrapped to GCM+AAD.

use aes_gcm::{
    aead::{Aead, KeyInit, Payload},
    Aes256Gcm, Nonce,
};
use base64::Engine;
use rand::Rng;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};

/// Key-storage allowlist (settings). Includes legacy kimi/mistral slots.
pub const PROVIDERS: &[&str] = &["xai", "openai", "anthropic", "kimi", "mistral", "selfhosted"];

/// OCR consume / re-OCR allowlist — dedicated endpoints only.
/// kimi/mistral have no wired endpoints; refusing avoids the OpenAI-compat
/// `else` path sending their keys to x.ai.
pub const CONSUME_PROVIDERS: &[&str] = &["xai", "openai", "anthropic", "selfhosted"];

/// Versioned blob: magic || 12-byte nonce || ciphertext+tag.
const V2_MAGIC: &[u8] = b"PRK2";
const V2_NONCE_LEN: usize = 12;
const MIN_SECRET_LEN: usize = 32;

pub fn normalize_provider(p: &str) -> Option<&'static str> {
    let t = p.trim().to_ascii_lowercase();
    PROVIDERS.iter().copied().find(|x| *x == t)
}

/// Providers with a real OCR HTTP path.
pub fn normalize_consume_provider(p: &str) -> Option<&'static str> {
    let t = p.trim().to_ascii_lowercase();
    CONSUME_PROVIDERS.iter().copied().find(|x| *x == t)
}

/// Call from `main` before serving. Fails closed if no usable encryption key.
pub fn assert_crypto_ready() -> anyhow::Result<()> {
    let _ = load_aes_key()?;
    Ok(())
}

pub async fn ensure_schema(pool: &PgPool) -> anyhow::Result<()> {
    for sql in [
        r#"create table if not exists user_llm_keys (
            user_id int not null references users(id) on delete cascade,
            provider text not null,
            ciphertext bytea not null,
            hint text not null default '',
            updated_at timestamptz not null default now(),
            primary key (user_id, provider)
        )"#,
        r#"create table if not exists user_settings (
            user_id int primary key references users(id) on delete cascade,
            ocr_provider text not null default 'xai',
            ocr_language text not null default 'fr',
            auto_match boolean not null default true,
            ocr_base_url text not null default '',
            updated_at timestamptz not null default now()
        )"#,
        "create index if not exists user_llm_keys_user_id_idx on user_llm_keys (user_id)",
        // Existing installs: add ocr_base_url when missing (selfhosted OpenAI-compatible endpoint).
        "alter table user_settings add column if not exists ocr_base_url text not null default ''",
        "alter table user_settings add column if not exists trash_retention_days int not null default 30",
    ] {
        if let Err(e) = sqlx::query(sql).execute(pool).await {
            tracing::warn!("llm_keys schema: {e}");
        }
    }

    if let Err(e) = migrate_legacy_ciphertexts(pool).await {
        tracing::warn!("llm_keys legacy migrate: {e}");
    }

    // scrub legacy rich `hint` values so list APIs cannot leak them.
    if let Err(e) = sqlx::query("update user_llm_keys set hint = '' where hint <> ''")
        .execute(pool)
        .await
    {
        tracing::warn!("llm_keys hint scrub: {e}");
    }

    // only persist env Vision keys to bootstrap user (id=1) when explicitly
    // opted in. Without PAPERRUST_LLM_ENV_FALLBACK=1, never write org keys into the DB
    // (clear_key must survive restarts).
    if std::env::var("PAPERRUST_LLM_ENV_FALLBACK").ok().as_deref() == Some("1") {
        for (provider, env_names) in [
            ("xai", &["XAI_API_KEY", "PAPERRUST_XAI_API_KEY"][..]),
            ("openai", &["OPENAI_API_KEY", "PAPERRUST_OPENAI_API_KEY"][..]),
            ("anthropic", &["ANTHROPIC_API_KEY", "PAPERRUST_ANTHROPIC_API_KEY"][..]),
        ] {
            let mut key: Option<String> = None;
            for env_name in env_names {
                if let Ok(raw) = std::env::var(env_name) {
                    let t = raw.trim();
                    if !t.is_empty() {
                        key = Some(t.to_string());
                        break;
                    }
                }
            }
            let Some(key) = key else { continue };
            let exists: i32 = sqlx::query_scalar(
                "select count(*)::int from user_llm_keys where user_id = 1 and provider = $1",
            )
            .bind(provider)
            .fetch_one(pool)
            .await
            .unwrap_or(0);
            if exists == 0 {
                match upsert_key(pool, 1, provider, &key).await {
                    Ok(_) => tracing::info!(
                        "seeded user_llm_keys for bootstrap user provider={provider} (PAPERRUST_LLM_ENV_FALLBACK=1, once)"
                    ),
                    Err(e) => tracing::warn!("seed user_llm_keys {provider}: {e}"),
                }
            }
        }
    }
    Ok(())
}

fn parse_key_bytes(raw: &str) -> Option<[u8; 32]> {
    let t = raw.trim();
    if t.is_empty() {
        return None;
    }
    if t.len() == 64 && t.chars().all(|c| c.is_ascii_hexdigit()) {
        let mut out = [0u8; 32];
        for i in 0..32 {
            out[i] = u8::from_str_radix(&t[i * 2..i * 2 + 2], 16).ok()?;
        }
        return Some(out);
    }
    if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(t) {
        if bytes.len() == 32 {
            return bytes.try_into().ok();
        }
    }
    if let Ok(bytes) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(t) {
        if bytes.len() == 32 {
            return bytes.try_into().ok();
        }
    }
    None
}

fn load_aes_key() -> anyhow::Result<[u8; 32]> {
    if let Ok(raw) = std::env::var("PAPERRUST_LLM_KEY_ENCRYPTION_KEY") {
        if let Some(k) = parse_key_bytes(&raw) {
            return Ok(k);
        }
        anyhow::bail!(
            "PAPERRUST_LLM_KEY_ENCRYPTION_KEY must be 32 bytes as 64 hex chars or base64"
        );
    }

    let secret = std::env::var("SESSION_SECRET").unwrap_or_default();
    let dev = std::env::var("PAPERRUST_DEV").ok().as_deref() == Some("1");
    if !dev {
        anyhow::bail!(
            "PAPERRUST_LLM_KEY_ENCRYPTION_KEY is required (32-byte hex or base64). \
             Generate with: openssl rand -hex 32"
        );
    }
    if secret.len() < MIN_SECRET_LEN {
        anyhow::bail!(
            "Dev fallback needs SESSION_SECRET of at least {MIN_SECRET_LEN} characters, \
             or set PAPERRUST_LLM_KEY_ENCRYPTION_KEY"
        );
    }
    tracing::warn!(
        "PAPERRUST_DEV=1: deriving LLM AES key from SESSION_SECRET (set PAPERRUST_LLM_KEY_ENCRYPTION_KEY for prod)"
    );
    let mut h = Sha256::new();
    h.update(b"paperrust-llm-aes-v2|");
    h.update(secret.as_bytes());
    Ok(h.finalize().into())
}

/// Legacy XOR keystream (pre-AES). Used only to rewrap old rows.
fn legacy_xor_master() -> [u8; 32] {
    let secret = std::env::var("SESSION_SECRET").unwrap_or_default();
    let mut h = Sha256::new();
    h.update(b"paperrust-llm-key-v1|");
    h.update(secret.as_bytes());
    h.finalize().into()
}

fn legacy_keystream_byte(master: &[u8; 32], nonce: &[u8], i: usize) -> u8 {
    let mut h = Sha256::new();
    h.update(master);
    h.update(nonce);
    h.update(&(i as u32).to_le_bytes());
    h.finalize()[i % 32]
}

fn decrypt_legacy_xor(cipher: &[u8]) -> Option<String> {
    if cipher.len() < 16 || cipher.starts_with(V2_MAGIC) {
        return None;
    }
    let (nonce, data) = cipher.split_at(16);
    let master = legacy_xor_master();
    let mut plain = Vec::with_capacity(data.len());
    for (i, b) in data.iter().enumerate() {
        plain.push(b ^ legacy_keystream_byte(&master, nonce, i));
    }
    String::from_utf8(plain).ok().filter(|s| !s.trim().is_empty())
}

fn aad_bytes(user_id: i32, provider: &str) -> Vec<u8> {
    format!("{user_id}|{provider}").into_bytes()
}

fn encrypt(plain: &str, user_id: i32, provider: &str) -> anyhow::Result<Vec<u8>> {
    let key = load_aes_key()?;
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| anyhow::anyhow!("aes key: {e}"))?;
    let mut nonce_bytes = [0u8; V2_NONCE_LEN];
    rand::rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let aad = aad_bytes(user_id, provider);
    let ct = cipher
        .encrypt(
            nonce,
            Payload {
                msg: plain.as_bytes(),
                aad: &aad,
            },
        )
        .map_err(|_| anyhow::anyhow!("aes-gcm encrypt failed"))?;
    let mut out = Vec::with_capacity(V2_MAGIC.len() + V2_NONCE_LEN + ct.len());
    out.extend_from_slice(V2_MAGIC);
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ct);
    Ok(out)
}

/// AES-GCM only. XOR is confined to `migrate_legacy_ciphertexts`.
/// AAD-bound decrypt only — empty-AAD legacy blobs fail closed.
fn decrypt_aes(cipher: &[u8], user_id: i32, provider: &str) -> Option<String> {
    if !cipher.starts_with(V2_MAGIC) {
        return None;
    }
    if cipher.len() < V2_MAGIC.len() + V2_NONCE_LEN + 16 {
        return None;
    }
    let key = load_aes_key().ok()?;
    let aes = Aes256Gcm::new_from_slice(&key).ok()?;
    let nonce = Nonce::from_slice(&cipher[V2_MAGIC.len()..V2_MAGIC.len() + V2_NONCE_LEN]);
    let ct = &cipher[V2_MAGIC.len() + V2_NONCE_LEN..];
    let aad = aad_bytes(user_id, provider);
    let plain = aes
        .decrypt(
            nonce,
            Payload {
                msg: ct,
                aad: &aad,
            },
        )
        .ok()?;
    String::from_utf8(plain)
        .ok()
        .filter(|s| !s.trim().is_empty())
}


async fn migrate_legacy_ciphertexts(pool: &PgPool) -> anyhow::Result<()> {
    let rows = sqlx::query("select user_id, provider, ciphertext from user_llm_keys")
        .fetch_all(pool)
        .await?;
    let mut migrated = 0u32;
    for row in rows {
        let cipher: Vec<u8> = row.get("ciphertext");
        if cipher.starts_with(V2_MAGIC) {
            continue;
        }
        let uid: i32 = row.get("user_id");
        let provider: String = row.get("provider");
        let Some(plain) = decrypt_legacy_xor(&cipher) else {
            tracing::warn!(
                "llm_keys: cannot decrypt legacy blob user_id={uid} provider={provider} — user must re-enter key"
            );
            continue;
        };
        let new_cipher = encrypt(&plain, uid, &provider)?;
        sqlx::query(
            "update user_llm_keys set ciphertext = $1, updated_at = now() where user_id = $2 and provider = $3",
        )
        .bind(&new_cipher)
        .bind(uid)
        .bind(&provider)
        .execute(pool)
        .await?;
        migrated += 1;
    }
    if migrated > 0 {
        tracing::info!("llm_keys: rewrapped {migrated} legacy XOR ciphertext(s) to AES-GCM");
    }
    Ok(())
}

/// never persist or return rich key hints — use `has_key` instead.
pub fn mask_key(_raw: &str) -> String {
    String::new()
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmKeyStatus {
    pub provider: String,
    pub has_key: bool,
    pub masked: String,
}

pub async fn list_statuses(pool: &PgPool, user_id: i32) -> sqlx::Result<Vec<LlmKeyStatus>> {
    // never SELECT/return `hint` — presence only (`has_key`). Legacy rich
    // hints are scrubbed in `ensure_schema` and ignored here even if still present.
    let rows = sqlx::query("select provider from user_llm_keys where user_id = $1")
        .bind(user_id)
        .fetch_all(pool)
        .await?;
    let mut have: std::collections::HashSet<String> = std::collections::HashSet::new();
    for r in rows {
        let p: String = r.get("provider");
        have.insert(p);
    }
    Ok(PROVIDERS
        .iter()
        .map(|p| {
            let has = have.contains(*p);
            LlmKeyStatus {
                provider: (*p).to_string(),
                has_key: has,
                masked: String::new(),
            }
        })
        .collect())
}

pub async fn get_decrypted(pool: &PgPool, user_id: i32, provider: &str) -> sqlx::Result<Option<String>> {
    let Some(p) = normalize_provider(provider) else {
        return Ok(None);
    };
    let row = sqlx::query("select ciphertext from user_llm_keys where user_id = $1 and provider = $2")
        .bind(user_id)
        .bind(p)
        .fetch_optional(pool)
        .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let cipher: Vec<u8> = row.get("ciphertext");
    // Runtime is AES-only. Legacy XOR must already have been migrated at boot.
    let Some(plain) = decrypt_aes(&cipher, user_id, p) else {
        if !cipher.starts_with(V2_MAGIC) {
            tracing::warn!(
                user_id,
                provider = %p,
                "llm_keys: refusing non-AES ciphertext at runtime (re-enter key)"
            );
        }
        return Ok(None);
    };
    Ok(Some(plain))
}

pub async fn upsert_key(pool: &PgPool, user_id: i32, provider: &str, api_key: &str) -> anyhow::Result<LlmKeyStatus> {
    let p = normalize_provider(provider).ok_or_else(|| anyhow::anyhow!("fournisseur inconnu"))?;
    let key = api_key.trim();
    if key.is_empty() {
        anyhow::bail!("clé vide");
    }
    if key.len() > 512 {
        anyhow::bail!("clé trop longue");
    }
    let hint = mask_key(key); // always empty — rely on has_key
    let cipher = encrypt(key, user_id, p)?;
    sqlx::query(
        r#"insert into user_llm_keys (user_id, provider, ciphertext, hint, updated_at)
           values ($1, $2, $3, $4, now())
           on conflict (user_id, provider) do update
             set ciphertext = excluded.ciphertext, hint = excluded.hint, updated_at = now()"#,
    )
    .bind(user_id)
    .bind(p)
    .bind(&cipher)
    .bind(&hint)
    .execute(pool)
    .await?;
    let _ = cipher; // never log
    Ok(LlmKeyStatus {
        provider: p.to_string(),
        has_key: true,
        masked: hint,
    })
}

pub async fn clear_key(pool: &PgPool, user_id: i32, provider: &str) -> anyhow::Result<LlmKeyStatus> {
    let p = normalize_provider(provider).ok_or_else(|| anyhow::anyhow!("fournisseur inconnu"))?;
    sqlx::query("delete from user_llm_keys where user_id = $1 and provider = $2")
        .bind(user_id)
        .bind(p)
        .execute(pool)
        .await?;
    Ok(LlmKeyStatus {
        provider: p.to_string(),
        has_key: false,
        masked: String::new(),
    })
}

pub async fn user_has_key(pool: &PgPool, user_id: i32, provider: &str) -> sqlx::Result<bool> {
    let Some(p) = normalize_provider(provider) else {
        return Ok(false);
    };
    let n: i32 = sqlx::query_scalar(
        "select count(*)::int from user_llm_keys where user_id = $1 and provider = $2",
    )
    .bind(user_id)
    .bind(p)
    .fetch_one(pool)
    .await?;
    Ok(n > 0)
}

/// Debug helper: never expose in APIs. Kept for tests.
#[allow(dead_code)]
pub fn roundtrip_ok(plain: &str) -> bool {
    encrypt(plain, 1, "xai")
        .ok()
        .and_then(|c| decrypt_aes(&c, 1, "xai"))
        .as_deref()
        == Some(plain)
}

/// Encode ciphertext as base64 for accidental logging avoidance checks.
#[allow(dead_code)]
pub fn cipher_b64(cipher: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(cipher)
}

#[cfg(test)]
mod syn52_hint_tests {
    use super::*;

    #[test]
    fn mask_key_never_returns_material() {
        assert_eq!(mask_key("xai-secret-ABCDEF123456"), "");
        assert_eq!(mask_key("sk-short"), "");
        assert!(mask_key("anything-at-all").is_empty());
    }

    #[test]
    fn status_shape_exposes_has_key_not_hint() {
        // Compile-time / API contract: LlmKeyStatus carries has_key + empty masked.
        let s = LlmKeyStatus {
            provider: "xai".into(),
            has_key: true,
            masked: mask_key("xai-legacy-hint-would-have-leaked"),
        };
        assert!(s.has_key);
        assert!(s.masked.is_empty(), "masked must stay empty");
        let json = serde_json::to_value(&s).expect("serialize");
        assert_eq!(json["hasKey"], true);
        assert_eq!(json["masked"], "");
        assert!(json.get("hint").is_none(), "hint must not appear in JSON");
    }
}
