use serde::{Deserialize, Serialize};

const MAX_OCR_TAGS: usize = 8;

/// Paperless / pipeline noise — never offer these to the LLM or attach from OCR.
/// Known names: `ai-done`, `ai-pending`, `ai-error`, `ai-unmatched`.
/// Any other `ai-*` marker is treated the same. Real owner labels
/// (`Véhicule`, `Finance`, `Santé`, …) stay in the allowlist.
pub fn is_ocr_noise_tag(name: &str) -> bool {
    let lower = name.trim().to_ascii_lowercase();
    if lower == "ai-done" || lower == "ai-pending" || lower == "ai-error" || lower == "ai-unmatched"
    {
        return true;
    }
    lower.starts_with("ai-")
}

pub fn names_eq_ci(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

pub fn is_year_tag(name: &str) -> bool {
    let t = name.trim();
    t.len() == 4 && t.chars().all(|c| c.is_ascii_digit())
}

/// Inbox / année YYYY may still be created; Véhicule only when a plate was seen.
pub fn may_ensure_ocr_tag(name: &str, license_plate: bool) -> bool {
    if is_ocr_noise_tag(name) {
        return false;
    }
    if is_year_tag(name) || names_eq_ci(name, "Inbox") {
        return true;
    }
    license_plate && names_eq_ci(name, "Véhicule")
}

fn sanitize_tag_for_prompt(name: &str) -> String {
    name.chars()
        .filter(|c| !c.is_control() && *c != '"' && *c != '\\')
        .take(80)
        .collect()
}

/// Owner labels the model may pick — every non-noise tag, exact names.
pub fn owner_tag_allowlist<'a, I>(names: I) -> Vec<String>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut out: Vec<String> = Vec::new();
    for name in names {
        let t = name.trim();
        if t.is_empty() || is_ocr_noise_tag(t) {
            continue;
        }
        if !out.iter().any(|x| names_eq_ci(x, t)) {
            out.push(t.to_string());
        }
    }
    out
}

/// Keep only LLM tags that exist on the owner (canonical spelling).
/// Inventions are dropped. Year YYYY and Inbox are left for the classify
/// special-case — they are not invented labels. Cap ~8.
pub fn filter_llm_tags(llm_tags: &[String], allowlist: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in llm_tags {
        let t = raw.trim();
        if t.is_empty() || is_ocr_noise_tag(t) {
            continue;
        }
        let Some(canon) = allowlist.iter().find(|a| names_eq_ci(a, t)) else {
            continue;
        };
        if out.iter().any(|x| names_eq_ci(x, canon)) {
            continue;
        }
        out.push(canon.clone());
        if out.len() >= MAX_OCR_TAGS {
            break;
        }
    }
    out
}

pub fn system_prompt(owner_tags: &[String]) -> String {
    let allow = owner_tag_allowlist(owner_tags.iter().map(String::as_str));
    let tags_clause = if allow.is_empty() {
        "aucun libellé existant — n'invente aucun tag thématique ; uniquement l'année YYYY si visible".to_string()
    } else {
        let list = allow
            .iter()
            .map(|n| sanitize_tag_for_prompt(n))
            .filter(|n| !n.is_empty())
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "{list}. Recopie le nom EXACT (casse et accents). N'invente aucun tag hors de cette liste. Tu peux ajouter l'année YYYY si elle est visible"
        )
    };
    format!(
        r#"Tu es le moteur OCR et de classement de Paperrust, une archive documentaire.
À partir des images JPEG des pages (et du nom de fichier), extraits le texte intégral et les métadonnées.
Réponds UNIQUEMENT en JSON avec ce schéma :
{{
  "title": "titre court et humain du document",
  "content": "texte OCR intégral, conservant la structure, toutes les pages de ce lot",
  "correspondent": "organisme ou personne émettrice, ou null",
  "documentType": "un de: Facture, Quittance, Contrat, Relevé, Courrier, Attestation, Ordonnance, Garantie, ou null",
  "createdDate": "YYYY-MM-DD ou null",
  "tags": ["UNIQUEMENT parmi les libellés existants: {tags_clause}"],
  "language": "fr|en|de|it",
  "summary": "une phrase",
  "amount": "montant principal en nombre ou null",
  "currency": "CHF|EUR|USD ou null",
  "reference": "n° de facture/police/dossier ou null",
  "paid": true,
  "licensePlate": "plaque d'immatriculation si visible, sinon null",
  "year": "année du document (YYYY) ou null"
}}
paid : booléen true si le document indique un paiement déjà effectué (payé, acquitté, soldé), false ou null sinon.
licensePlate : plaque / immatriculation / immat si visible (ex. VD 123456, GE 123.456, F-AB-123-CD).
Si une immatriculation est visible, le document se classe sous le chemin de stockage Véhicule/{{year}} (year = année du document, sinon année courante).
INTERDICTION : inventer un nouveau tag. Choisis uniquement parmi les libellés listés ci-dessus (plus l'année YYYY si visible).
Ne invente pas de données personnelles réelles. Si illisible, content court et title = nom de fichier."#
    )
}

const PAGE_CHUNK: usize = 6;
/// Hardcoded default is still `grok-4.5`: docs.x.ai lists it as
/// `text, image → text` (image_url). Override with `PAPERRUST_XAI_MODEL`.
const DEFAULT_XAI_MODEL: &str = "grok-4.5";
/// xAI image-understanding limit: 20MiB jpeg/png (docs.x.ai Models).
const MAX_VISION_IMAGE_BYTES: usize = 20 * 1024 * 1024;
const CHAT_MAX_TOKENS: u32 = 8000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrResult {
    pub title: String,
    pub content: String,
    pub correspondent: Option<String>,
    pub document_type: Option<String>,
    pub created_date: Option<String>,
    pub tags: Vec<String>,
    pub language: String,
    pub summary: String,
    pub amount: Option<String>,
    pub currency: Option<String>,
    pub reference: Option<String>,
    pub paid: Option<bool>,
    pub license_plate: Option<String>,
    pub year: Option<String>,
    pub provider: String,
}

pub fn fallback_ocr(filename: &str) -> OcrResult {
    let title = filename
        .rsplit_once('.')
        .map(|(s, _)| s)
        .unwrap_or(filename)
        .to_string();
    OcrResult {
        title,
        content: String::new(),
        correspondent: None,
        document_type: None,
        created_date: Some(chrono::Utc::now().format("%Y-%m-%d").to_string()),
        tags: vec!["Inbox".into()],
        language: "fr".into(),
        summary: "En attente d'OCR.".into(),
        amount: None,
        currency: None,
        reference: None,
        paid: None,
        license_plate: None,
        year: None,
        provider: "xai".into(),
    }
}

fn json_str(parsed: &serde_json::Value, key: &str) -> Option<String> {
    let v = parsed.get(key)?;
    if let Some(s) = v.as_str() {
        let t = s.trim();
        if t.is_empty() || t.eq_ignore_ascii_case("null") {
            return None;
        }
        return Some(t.to_string());
    }
    if let Some(n) = v.as_f64() {
        if n.fract() == 0.0 {
            return Some(format!("{n:.0}"));
        }
        return Some(n.to_string());
    }
    if let Some(b) = v.as_bool() {
        return Some(b.to_string());
    }
    None
}

fn json_bool(parsed: &serde_json::Value, key: &str) -> Option<bool> {
    let v = parsed.get(key)?;
    if let Some(b) = v.as_bool() {
        return Some(b);
    }
    if let Some(s) = v.as_str() {
        let t = s.trim().to_lowercase();
        return match t.as_str() {
            "true" | "1" | "oui" | "yes" | "payé" | "paye" | "acquitté" | "acquitte" => {
                Some(true)
            }
            "false" | "0" | "non" | "no" => Some(false),
            _ => None,
        };
    }
    v.as_i64().map(|n| n != 0)
}

pub(crate) fn parse_result(raw: &str, filename: &str, provider: &str) -> OcrResult {
    let start = raw.find('{').unwrap_or(0);
    let end = raw.rfind('}').unwrap_or(raw.len().saturating_sub(1));
    let parsed: serde_json::Value =
        serde_json::from_str(&raw[start..=end]).unwrap_or(serde_json::json!({}));
    let title_fallback = filename
        .rsplit_once('.')
        .map(|(s, _)| s)
        .unwrap_or(filename);
    let tags = parsed
        .get("tags")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .take(8)
                .collect()
        })
        .unwrap_or_default();
    OcrResult {
        title: json_str(&parsed, "title")
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| title_fallback.to_string()),
        content: json_str(&parsed, "content").unwrap_or_default(),
        correspondent: json_str(&parsed, "correspondent"),
        document_type: json_str(&parsed, "documentType"),
        created_date: json_str(&parsed, "createdDate"),
        tags,
        language: json_str(&parsed, "language").unwrap_or_else(|| "fr".into()),
        summary: json_str(&parsed, "summary").unwrap_or_default(),
        amount: json_str(&parsed, "amount"),
        currency: json_str(&parsed, "currency"),
        reference: json_str(&parsed, "reference"),
        paid: json_bool(&parsed, "paid"),
        license_plate: json_str(&parsed, "licensePlate"),
        year: json_str(&parsed, "year"),
        provider: provider.to_string(),
    }
}

fn merge_ocr(parts: Vec<OcrResult>, filename: &str, provider: &str) -> OcrResult {
    let mut out = fallback_ocr(filename);
    out.provider = provider.to_string();
    out.tags.clear();
    out.summary.clear();
    out.created_date = None;
    let mut contents: Vec<String> = Vec::new();
    let mut tagset: Vec<String> = Vec::new();
    for p in parts {
        if out.title.is_empty()
            || out.title
                == filename
                    .rsplit_once('.')
                    .map(|(s, _)| s)
                    .unwrap_or(filename)
        {
            if !p.title.is_empty() {
                out.title = p.title;
            }
        }
        if !p.content.trim().is_empty() {
            contents.push(p.content);
        }
        if out.correspondent.is_none() {
            out.correspondent = p.correspondent;
        }
        if out.document_type.is_none() {
            out.document_type = p.document_type;
        }
        if out.created_date.is_none() {
            out.created_date = p.created_date;
        }
        for t in p.tags {
            if !tagset.iter().any(|x| x.eq_ignore_ascii_case(&t)) {
                tagset.push(t);
            }
        }
        if out.language.is_empty() {
            out.language = p.language;
        } else if !p.language.is_empty() {
            out.language = p.language;
        }
        if out.summary.is_empty() && !p.summary.is_empty() {
            out.summary = p.summary;
        }
        if out.amount.is_none() {
            out.amount = p.amount;
        }
        if out.currency.is_none() {
            out.currency = p.currency;
        }
        if out.reference.is_none() {
            out.reference = p.reference;
        }
        if out.paid.is_none() {
            out.paid = p.paid;
        }
        if out.license_plate.is_none() {
            out.license_plate = p.license_plate;
        }
        if out.year.is_none() {
            out.year = p.year;
        }
    }
    out.content = contents.join("\n\n");
    out.tags = tagset;
    out
}

fn env_api_key(provider: &str) -> Option<String> {
    // Prefer canonical names; accept PAPERRUST_* aliases.
    // never map unknown / kimi / mistral onto XAI env keys.
    let names: &[&str] = match provider {
        "openai" => &["OPENAI_API_KEY", "PAPERRUST_OPENAI_API_KEY"],
        "anthropic" => &["ANTHROPIC_API_KEY", "PAPERRUST_ANTHROPIC_API_KEY"],
        "xai" => &["XAI_API_KEY", "PAPERRUST_XAI_API_KEY"],
        _ => return None,
    };
    for name in names {
        if let Ok(raw) = std::env::var(name) {
            let t = raw.trim();
            if !t.is_empty() {
                return Some(t.to_string());
            }
        }
    }
    None
}

/// Resolve OCR API key for a request.
/// Priority: per-user DB key → optional shared env (`PAPERRUST_LLM_ENV_FALLBACK=1`) →
/// client-sent key only when `PAPERRUST_DEV=1`. Never log keys.
pub fn resolve_key(
    provider: &str,
    user_key: Option<&str>,
    client_key: Option<&str>,
) -> Option<String> {
    if let Some(k) = user_key.map(str::trim).filter(|s| !s.is_empty()) {
        return Some(k.to_string());
    }
    if std::env::var("PAPERRUST_LLM_ENV_FALLBACK").ok().as_deref() == Some("1") {
        if let Some(k) = env_api_key(provider) {
            return Some(k);
        }
    }
    if std::env::var("PAPERRUST_DEV").ok().as_deref() == Some("1") {
        return client_key
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
    }
    let _ = client_key; // ignore client apiKey in prod; never log
    None
}

/// True when a shared env key exists (admin bootstrap). Prefer per-user keys for OCR.
pub fn env_key_configured(provider: &str) -> bool {
    env_api_key(provider).is_some()
}

fn jpeg_data_url(jpeg: &[u8]) -> String {
    let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, jpeg);
    format!("data:image/jpeg;base64,{b64}")
}

fn jpeg_b64(jpeg: &[u8]) -> String {
    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, jpeg)
}

pub(crate) fn resolve_xai_model(env_val: Option<&str>) -> String {
    env_val
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_XAI_MODEL)
        .to_string()
}

fn xai_model() -> String {
    resolve_xai_model(std::env::var("PAPERRUST_XAI_MODEL").ok().as_deref())
}

/// Public xAI chat URL, or `PAPERRUST_XAI_BASE_URL` when that override is set.
/// The default is the vendor's public API. There is no embedded key and no
/// private host. A private override is refused by the same checks as self-hosted OCR.
pub(crate) fn resolve_xai_chat_url(env_val: Option<&str>) -> Result<String, String> {
    let trimmed = env_val.map(str::trim).filter(|s| !s.is_empty());
    match trimmed {
        None => Ok("https://api.x.ai/v1/chat/completions".to_string()),
        Some(base) => {
            let safe = crate::ocr_ssrf::validate_selfhosted_ocr_base_url(base)?;
            Ok(openai_compatible_chat_url(&safe))
        }
    }
}

/// grok-4.5 / grok-4.6 accept `reasoning_effort`; grok-4.20-* 400s if it is set.
pub(crate) fn xai_reasoning_effort(model: &str) -> Option<&'static str> {
    let m = model.trim();
    if m.starts_with("grok-4.6") || m.starts_with("grok-4.5") {
        Some("low")
    } else {
        None
    }
}

pub(crate) fn ensure_vision_jpeg(jpeg: &[u8]) -> Result<(), String> {
    if jpeg.is_empty() {
        return Err("empty JPEG page for OCR".into());
    }
    if !crate::preview::looks_like_jpeg(jpeg) {
        return Err("OCR page is not JPEG".into());
    }
    if jpeg.len() > MAX_VISION_IMAGE_BYTES {
        return Err(format!(
            "OCR JPEG exceeds {}MiB image limit",
            MAX_VISION_IMAGE_BYTES / (1024 * 1024)
        ));
    }
    Ok(())
}

fn vision_user_content(
    user_text: &str,
    pages: &[Vec<u8>],
    detail: Option<&str>,
) -> serde_json::Value {
    let mut parts = vec![serde_json::json!({"type": "text", "text": user_text})];
    for jpeg in pages {
        let mut image_url = serde_json::Map::new();
        image_url.insert("url".into(), serde_json::json!(jpeg_data_url(jpeg)));
        if let Some(d) = detail {
            image_url.insert("detail".into(), serde_json::json!(d));
        }
        parts.push(serde_json::json!({
            "type": "image_url",
            "image_url": serde_json::Value::Object(image_url)
        }));
    }
    serde_json::Value::Array(parts)
}

/// OpenAI / self-hosted Chat Completions body.
/// Never sends `response_format: json_object` when images are present (CoS #2).
pub(crate) fn openai_compat_vision_chat_body(
    model: &str,
    system: &str,
    user_text: &str,
    pages: &[Vec<u8>],
) -> serde_json::Value {
    serde_json::json!({
        "model": model,
        "temperature": 0.1,
        "max_tokens": CHAT_MAX_TOKENS,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": vision_user_content(user_text, pages, None)}
        ]
    })
}

/// xAI vision Chat Completions body.
///
/// Vision Chat Completions body for xAI. `response_format: json_object` is
/// omitted whenever images are present (CoS #2); JSON still comes from the
/// system prompt and [`parse_result`].
pub(crate) fn xai_vision_chat_body(
    model: &str,
    system: &str,
    user_text: &str,
    pages: &[Vec<u8>],
) -> serde_json::Value {
    let mut body = serde_json::Map::new();
    body.insert("model".into(), serde_json::json!(model));
    body.insert("max_tokens".into(), serde_json::json!(CHAT_MAX_TOKENS));
    if let Some(effort) = xai_reasoning_effort(model) {
        body.insert("reasoning_effort".into(), serde_json::json!(effort));
    }
    body.insert(
        "messages".into(),
        serde_json::json!([
            {"role": "system", "content": system},
            {"role": "user", "content": vision_user_content(user_text, pages, Some("high"))}
        ]),
    );
    serde_json::Value::Object(body)
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct ProviderErrorFields {
    pub code: Option<String>,
    pub message: Option<String>,
    pub type_name: Option<String>,
}

fn sanitize_error_field(s: &str) -> Option<String> {
    let cleaned = redact_for_log(&strip_inline_media(s));
    let cleaned: String = cleaned.trim().chars().take(160).collect();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

/// CoS #1: only `error.code` / `error.message` / `error.type` (redacted).
pub(crate) fn parse_provider_error_fields(raw: &str) -> ProviderErrorFields {
    let parsed: serde_json::Value = match serde_json::from_str(raw.trim()) {
        Ok(v) => v,
        Err(_) => return ProviderErrorFields::default(),
    };
    let nested = parsed.get("error");
    let mut out = ProviderErrorFields::default();
    match nested {
        Some(serde_json::Value::Object(map)) => {
            out.code = map
                .get("code")
                .and_then(|v| v.as_str())
                .and_then(sanitize_error_field);
            out.message = map
                .get("message")
                .and_then(|v| v.as_str())
                .and_then(sanitize_error_field);
            out.type_name = map
                .get("type")
                .and_then(|v| v.as_str())
                .and_then(sanitize_error_field);
        }
        Some(serde_json::Value::String(s)) => {
            out.message = sanitize_error_field(s);
        }
        _ => {}
    }
    if out.code.is_none() {
        out.code = parsed
            .get("code")
            .and_then(|v| v.as_str())
            .and_then(sanitize_error_field);
    }
    out
}

pub(crate) fn summarize_provider_error_body(raw: &str) -> String {
    let f = parse_provider_error_fields(raw);
    let mut parts = Vec::new();
    if let Some(t) = f.type_name {
        parts.push(t);
    }
    if let Some(c) = f.code {
        parts.push(c);
    }
    if let Some(m) = f.message {
        parts.push(m);
    }
    parts.join(" — ")
}

fn strip_inline_media(s: &str) -> String {
    let lower = s.to_ascii_lowercase();
    let mut out = String::with_capacity(s.len().min(256));
    let mut i = 0;
    while i < s.len() {
        if lower
            .get(i..)
            .is_some_and(|rest| rest.starts_with("data:image"))
        {
            out.push_str("data:image/[omitted]");
            let rest = &s[i..];
            // Do not stop at ',' — data URLs use it as the base64 separator.
            let skip = rest
                .find(|c: char| matches!(c, ' ' | '"' | '\'' | '\n' | '\r' | '}' | ']'))
                .unwrap_or(rest.len());
            i += skip;
            continue;
        }
        let ch = s[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn provider_http_error(provider: &str, status: reqwest::StatusCode, body: &str) -> anyhow::Error {
    // Body is already drained by the caller (`res.text()`). Log only the three
    // redacted fields — never Authorization, keys, or image/base64.
    let fields = parse_provider_error_fields(body);
    tracing::warn!(
        provider = %provider,
        status = %status,
        code = fields.code.as_deref().unwrap_or(""),
        message = fields.message.as_deref().unwrap_or(""),
        error_type = fields.type_name.as_deref().unwrap_or(""),
        "llm provider error"
    );
    let summary = summarize_provider_error_body(body);
    if summary.is_empty() {
        anyhow::anyhow!("{provider} HTTP {status}")
    } else {
        anyhow::anyhow!("{provider} HTTP {status}: {summary}")
    }
}

/// scrub key-like fragments before logging error strings.
pub(crate) fn redact_for_log(s: &str) -> String {
    fn is_secret_byte(c: u8) -> bool {
        c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.')
    }

    let mut out = String::with_capacity(s.len());
    let lower = s.to_ascii_lowercase();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let rest = &lower[i..];
        let mut end = None::<usize>;

        // Token forms: sk-… / xai-…
        for prefix in ["sk-", "xai-"] {
            if rest.starts_with(prefix) {
                let mut j = i + prefix.len();
                while j < bytes.len() && is_secret_byte(bytes[j]) {
                    j += 1;
                }
                if j > i + prefix.len() {
                    end = Some(j);
                }
                break;
            }
        }

        // Keyword forms: api_key / apikey / api-key / bearer + optional separator + value
        if end.is_none() {
            for prefix in ["api_key", "apikey", "api-key", "bearer "] {
                if !rest.starts_with(prefix) {
                    continue;
                }
                let mut j = i + prefix.len();
                // skip quotes / separators: "api_key":"VALUE" or api_key=VALUE
                while j < bytes.len() && matches!(bytes[j], b'"' | b'\'' | b':' | b'=' | b' ') {
                    j += 1;
                }
                let value_start = j;
                while j < bytes.len() && is_secret_byte(bytes[j]) {
                    j += 1;
                }
                if j > value_start {
                    end = Some(j);
                }
                break;
            }
        }

        if let Some(j) = end {
            out.push_str("[REDACTED]");
            i = j;
            continue;
        }
        let ch = s[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

async fn call_llm_chunk(
    client: &reqwest::Client,
    provider: &str,
    api_key: &str,
    filename: &str,
    pages: &[Vec<u8>],
    page_from: usize,
    page_total: usize,
    base_url: Option<&str>,
    owner_tags: &[String],
) -> anyhow::Result<OcrResult> {
    let page_to = page_from + pages.len() - 1;
    let system = system_prompt(owner_tags);
    let user_text = format!(
        "Nom de fichier : {filename}\nPages JPEG {page_from}–{page_to} sur {page_total}.\nOCR de ces pages uniquement ; content = texte de ce lot.\nTags : choisis UNIQUEMENT parmi les libellés du message système (plus l'année YYYY si visible)."
    );
    for jpeg in pages {
        ensure_vision_jpeg(jpeg).map_err(|m| anyhow::anyhow!("{m}"))?;
    }
    let raw = if provider == "anthropic" {
        let mut content = vec![serde_json::json!({"type": "text", "text": user_text})];
        for jpeg in pages {
            content.push(serde_json::json!({
                "type": "image",
                "source": {
                    "type": "base64",
                    "media_type": "image/jpeg",
                    "data": jpeg_b64(jpeg)
                }
            }));
        }
        let body = serde_json::json!({
            "model": "claude-sonnet-4-5",
            "max_tokens": CHAT_MAX_TOKENS,
            "system": system,
            "messages": [{ "role": "user", "content": content }]
        });
        let res = client
            .post("https://api.anthropic.com/v1/messages")
            .header("content-type", "application/json")
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()
            .await?;
        if !res.status().is_success() {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            return Err(provider_http_error("anthropic", status, &body));
        }
        let v: serde_json::Value = res.json().await?;
        v["content"]
            .as_array()
            .and_then(|a| {
                a.iter()
                    .find(|c| c["type"] == "text")
                    .and_then(|c| c["text"].as_str())
                    .map(str::to_string)
            })
            .unwrap_or_default()
    } else {
        let (endpoint, model) = if provider == "selfhosted" {
            let base = base_url
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| anyhow::anyhow!("URL de base Auto-hébergé manquante"))?;
            let endpoint = openai_compatible_chat_url(base);
            let model = std::env::var("PAPERRUST_SELFHOSTED_MODEL")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| "llava".into());
            (endpoint, model)
        } else if provider == "openai" {
            (
                "https://api.openai.com/v1/chat/completions".into(),
                "gpt-4.1-mini".into(),
            )
        } else if provider == "xai" {
            let endpoint = resolve_xai_chat_url(
                std::env::var("PAPERRUST_XAI_BASE_URL").ok().as_deref(),
            )
            .map_err(|e| anyhow::anyhow!(e))?;
            (endpoint, xai_model())
        } else {
            // refuse kimi/mistral/unknown — never fall through to x.ai.
            anyhow::bail!("unsupported_ocr_provider");
        };
        let body = if provider == "xai" {
            xai_vision_chat_body(&model, &system, &user_text, pages)
        } else {
            openai_compat_vision_chat_body(&model, &system, &user_text, pages)
        };
        let mut req = client
            .post(&endpoint)
            .header("content-type", "application/json")
            .json(&body);
        if !api_key.trim().is_empty() {
            req = req.bearer_auth(api_key);
        }
        let res = req.send().await?;
        if !res.status().is_success() {
            let status = res.status();
            // Drain body for connection reuse. Log only redacted code/type/error.
            let body = res.text().await.unwrap_or_default();
            return Err(provider_http_error(provider, status, &body));
        }
        let v: serde_json::Value = res.json().await?;
        v["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string()
    };
    Ok(parse_result(&raw, filename, provider))
}

fn openai_compatible_chat_url(base: &str) -> String {
    let b = base.trim().trim_end_matches('/');
    if b.ends_with("/chat/completions") {
        b.to_string()
    } else if b.ends_with("/v1") {
        format!("{b}/chat/completions")
    } else {
        format!("{b}/v1/chat/completions")
    }
}

/// Send JPEG page bytes to the vision model. Never pass SVG or PDF bytes.
/// `base_url` is required for provider `selfhosted` (OpenAI-compatible).
/// `owner_tags` is the document owner's existing tag names (noise already
/// filtered or not — [`system_prompt`] / [`filter_llm_tags`] drop `ai-*`).
pub async fn run_llm_ocr(
    provider: &str,
    api_key: &str,
    filename: &str,
    jpeg_pages: &[Vec<u8>],
    base_url: Option<&str>,
    owner_tags: &[String],
) -> anyhow::Result<OcrResult> {
    if crate::llm_keys::normalize_consume_provider(provider).is_none() {
        anyhow::bail!("unsupported_ocr_provider");
    }
    if jpeg_pages.is_empty() {
        anyhow::bail!("Aucune page JPEG pour l'OCR.");
    }
    // DNS TOCTOU: re-validate at call time and pin SocketAddrs on the client
    // so reqwest cannot re-resolve to a private IP after validation.
    let (client, safe_base) = if provider == "selfhosted" {
        let raw = base_url
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                anyhow::anyhow!("URL de base Auto-hébergé manquante (Réglages → OCR).")
            })?;
        let ep = crate::ocr_ssrf::validate_selfhosted_ocr_endpoint(raw)
            .map_err(|m| anyhow::anyhow!("{m}"))?;
        let client = crate::ocr_ssrf::ocr_http_client_pinned(&ep.resolve_host, &ep.addrs)?;
        (client, Some(ep.url))
    } else {
        (crate::ocr_ssrf::ocr_http_client()?, None)
    };
    let total = jpeg_pages.len();
    let mut parts: Vec<OcrResult> = Vec::new();
    let mut last_err: Option<anyhow::Error> = None;
    for (i, chunk) in jpeg_pages.chunks(PAGE_CHUNK).enumerate() {
        let page_from = i * PAGE_CHUNK + 1;
        tracing::info!(
            "ocr {} pages {page_from}–{} / {total} for {filename}",
            provider,
            page_from + chunk.len() - 1
        );
        match call_llm_chunk(
            &client,
            provider,
            api_key,
            filename,
            chunk,
            page_from,
            total,
            safe_base.as_deref(),
            owner_tags,
        )
        .await
        {
            Ok(res) => parts.push(res),
            Err(err) => {
                tracing::warn!(error = %redact_for_log(&err.to_string()), "ocr chunk failed");
                last_err = Some(err);
            }
        }
    }
    if parts.is_empty() {
        return Err(last_err.unwrap_or_else(|| anyhow::anyhow!("OCR vide")));
    }
    let mut merged = merge_ocr(parts, filename, provider);
    let allow = owner_tag_allowlist(owner_tags.iter().map(String::as_str));
    merged.tags = filter_llm_tags(&merged.tags, &allow);
    Ok(merged)
}

#[cfg(test)]
mod redact_tests {
    use super::redact_for_log;

    #[test]
    fn redacts_sk_and_xai_tokens() {
        let s = redact_for_log("fail sk-abcDEF123456 and xai-ZZZ999 end");
        assert!(!s.contains("sk-abc"), "{s}");
        assert!(!s.contains("xai-ZZZ"), "{s}");
        assert!(s.contains("[REDACTED]"), "{s}");
    }

    #[test]
    fn redacts_api_key_assignment() {
        let s = redact_for_log(r#"{"error":"bad","api_key":"secretVALUE"}"#);
        assert!(!s.contains("secretVALUE"), "{s}");
        assert!(s.contains("[REDACTED]"), "{s}");
    }

    #[test]
    fn keeps_benign_status_lines() {
        let s = redact_for_log("xai HTTP 503");
        assert_eq!(s, "xai HTTP 503");
    }

    #[test]
    fn parse_nested_error_code_message_type_only() {
        let raw = r#"{"error":{"message":"Invalid request: response_format is not supported with images","type":"invalid_request_error","code":"invalid_request","param":"response_format","authorization":"Bearer xai-LEAK"}}"#;
        let f = super::parse_provider_error_fields(raw);
        assert_eq!(f.code.as_deref(), Some("invalid_request"));
        assert_eq!(f.type_name.as_deref(), Some("invalid_request_error"));
        assert!(
            f.message
                .as_deref()
                .unwrap_or("")
                .contains("response_format"),
            "{f:?}"
        );
        assert!(!format!("{f:?}").contains("xai-LEAK"), "{f:?}");
        assert!(!format!("{f:?}").contains("Bearer"), "{f:?}");
    }

    #[test]
    fn parse_xai_flat_error_redacts_key_and_image() {
        let raw = r#"{"code":"Client specified an invalid argument","error":"bad key xai-SECRET99 and data:image/jpeg;base64,/9j/abcPAYLOAD"}"#;
        let f = super::parse_provider_error_fields(raw);
        assert_eq!(
            f.code.as_deref(),
            Some("Client specified an invalid argument")
        );
        let msg = f.message.as_deref().unwrap_or("");
        assert!(!msg.contains("xai-SECRET99"), "{msg}");
        assert!(!msg.contains("/9j/"), "{msg}");
        assert!(!msg.contains("abcPAYLOAD"), "{msg}");
        assert!(
            msg.contains("[REDACTED]") || msg.contains("data:image/[omitted]"),
            "{msg}"
        );
    }

    #[test]
    fn summarize_ignores_non_json() {
        let f = super::parse_provider_error_fields("<html>nope</html>");
        assert_eq!(f, super::ProviderErrorFields::default());
        assert_eq!(
            super::summarize_provider_error_body("<html>nope</html>"),
            ""
        );
    }
}

#[cfg(test)]
mod xai_request_tests {
    use super::*;

    fn tiny_jpeg() -> Vec<u8> {
        // Minimal SOI/EOI — enough for looks_like_jpeg + body tests (not a decode).
        vec![0xFF, 0xD8, 0xFF, 0xD9]
    }

    #[test]
    fn default_model_is_grok_4_5() {
        assert_eq!(resolve_xai_model(None), "grok-4.5");
        assert_eq!(resolve_xai_model(Some("")), "grok-4.5");
        assert_eq!(resolve_xai_model(Some("   ")), "grok-4.5");
        assert_eq!(resolve_xai_model(Some(" grok-4.6 ")), "grok-4.6");
    }

    #[test]
    fn chat_url_defaults_to_public_xai_and_refuses_private_override() {
        assert_eq!(
            resolve_xai_chat_url(None).unwrap(),
            "https://api.x.ai/v1/chat/completions"
        );
        assert_eq!(
            resolve_xai_chat_url(Some("  ")).unwrap(),
            "https://api.x.ai/v1/chat/completions"
        );
        assert_eq!(
            openai_compatible_chat_url("https://llm.example/v1"),
            "https://llm.example/v1/chat/completions"
        );
        assert!(resolve_xai_chat_url(Some("https://10.0.0.5/v1")).is_err());
        assert!(resolve_xai_chat_url(Some("http://llm.example/v1")).is_err());
    }

    #[test]
    fn reasoning_effort_only_for_4_5_and_4_6() {
        assert_eq!(xai_reasoning_effort("grok-4.6"), Some("low"));
        assert_eq!(xai_reasoning_effort("grok-4.5-latest"), Some("low"));
        assert_eq!(xai_reasoning_effort("grok-4.20-0309-non-reasoning"), None);
        assert_eq!(xai_reasoning_effort("grok-2-vision-1212"), None);
    }

    #[test]
    fn xai_body_is_documented_vision_shape() {
        let pages = [tiny_jpeg()];
        let body = xai_vision_chat_body("grok-4.6", &system_prompt(&[]), "OCR this page", &pages);
        assert_eq!(body["model"], "grok-4.6");
        assert_eq!(body["max_tokens"], 8000);
        assert_eq!(body["reasoning_effort"], "low");
        assert!(body.get("temperature").is_none(), "{body}");
        assert!(body.get("response_format").is_none(), "{body}");
        let user = &body["messages"][1]["content"];
        assert_eq!(user[0]["type"], "text");
        assert_eq!(user[1]["type"], "image_url");
        assert_eq!(user[1]["image_url"]["detail"], "high");
        let url = user[1]["image_url"]["url"].as_str().unwrap();
        assert!(url.starts_with("data:image/jpeg;base64,"), "{url}");
        assert!(!url.contains("xai-"), "{url}");
    }

    #[test]
    fn openai_compat_body_omits_json_object_when_images() {
        let pages = [tiny_jpeg()];
        let body = openai_compat_vision_chat_body("gpt-4.1-mini", &system_prompt(&[]), "OCR", &pages);
        assert!(body.get("response_format").is_none(), "{body}");
        assert_eq!(body["temperature"], 0.1);
        assert!(body.get("reasoning_effort").is_none());
    }

    #[test]
    fn vision_jpeg_guards() {
        assert!(ensure_vision_jpeg(&[]).is_err());
        assert!(ensure_vision_jpeg(b"%PDF-1.4").is_err());
        assert!(ensure_vision_jpeg(&tiny_jpeg()).is_ok());
        assert_eq!(MAX_VISION_IMAGE_BYTES, 20 * 1024 * 1024);
    }
}

#[cfg(test)]
mod provider_route_tests {
    #[test]
    fn consume_allowlist_excludes_kimi_mistral() {
        assert_eq!(
            crate::llm_keys::normalize_consume_provider("xai"),
            Some("xai")
        );
        assert_eq!(
            crate::llm_keys::normalize_consume_provider("OpenAI"),
            Some("openai")
        );
        assert_eq!(
            crate::llm_keys::normalize_consume_provider("anthropic"),
            Some("anthropic")
        );
        assert_eq!(
            crate::llm_keys::normalize_consume_provider("selfhosted"),
            Some("selfhosted")
        );
        assert_eq!(crate::llm_keys::normalize_consume_provider("kimi"), None);
        assert_eq!(crate::llm_keys::normalize_consume_provider("mistral"), None);
        assert_eq!(crate::llm_keys::normalize_consume_provider("grok"), None);
        // Settings storage may still know kimi:
        assert_eq!(crate::llm_keys::normalize_provider("kimi"), Some("kimi"));
    }
}

#[cfg(test)]
mod owner_tag_tests {
    use super::*;

    #[test]
    fn noise_tags_excluded_real_labels_kept() {
        assert!(is_ocr_noise_tag("ai-done"));
        assert!(is_ocr_noise_tag("AI-PENDING"));
        assert!(is_ocr_noise_tag("ai-error"));
        assert!(is_ocr_noise_tag("ai-unmatched"));
        assert!(is_ocr_noise_tag("ai-custom-noise"));
        assert!(!is_ocr_noise_tag("Véhicule"));
        assert!(!is_ocr_noise_tag("Finance"));
        assert!(!is_ocr_noise_tag("Santé"));
        assert!(!is_ocr_noise_tag("Inbox"));
        assert!(!is_ocr_noise_tag("AI"));
    }

    #[test]
    fn allowlist_drops_noise_and_dedupes() {
        let allow = owner_tag_allowlist(
            ["Finance", "ai-done", "finance", "Véhicule", "ai-pending"].into_iter(),
        );
        assert_eq!(allow, vec!["Finance".to_string(), "Véhicule".to_string()]);
    }

    #[test]
    fn filter_llm_tags_keeps_owner_spelling_drops_inventions() {
        let allow = vec!["Finance".into(), "Santé".into(), "Véhicule".into()];
        let llm = vec![
            "finance".into(),
            "Inventé".into(),
            "santé".into(),
            "ai-done".into(),
            "Véhicule".into(),
        ];
        let got = filter_llm_tags(&llm, &allow);
        assert_eq!(got, vec!["Finance", "Santé", "Véhicule"]);
    }

    #[test]
    fn filter_llm_tags_caps_at_eight() {
        let allow: Vec<String> = (1..=12).map(|i| format!("Tag{i}")).collect();
        let llm: Vec<String> = (1..=12).map(|i| format!("tag{i}")).collect();
        assert_eq!(filter_llm_tags(&llm, &allow).len(), 8);
    }

    #[test]
    fn may_ensure_year_inbox_and_vehicle_when_plated() {
        assert!(may_ensure_ocr_tag("2025", false));
        assert!(may_ensure_ocr_tag("Inbox", false));
        assert!(may_ensure_ocr_tag("inbox", false));
        assert!(!may_ensure_ocr_tag("Véhicule", false));
        assert!(may_ensure_ocr_tag("Véhicule", true));
        assert!(!may_ensure_ocr_tag("Inventé", false));
        assert!(!may_ensure_ocr_tag("ai-done", false));
    }

    #[test]
    fn system_prompt_injects_owner_list_not_fixed_six() {
        let p = system_prompt(&["Assurances".into(), "Syndic".into(), "ai-done".into()]);
        assert!(p.contains("Assurances"));
        assert!(p.contains("Syndic"));
        assert!(!p.contains("ai-done"));
        assert!(
            !p.contains("parmi Finance, Santé, Logement, Impôts, Garantie, Véhicule"),
            "old fixed allowlist must be gone: {p}"
        );
        assert!(p.contains("INTERDICTION"));
    }

    #[test]
    fn parse_result_reads_tags_and_plate() {
        let raw = r#"{
            "title": "Police auto",
            "content": "VD 123456",
            "tags": ["Finance", "Inventé", "Santé"],
            "licensePlate": "VD 123456",
            "year": "2024"
        }"#;
        let parsed = parse_result(raw, "scan.pdf", "xai");
        assert_eq!(parsed.title, "Police auto");
        assert_eq!(parsed.tags, vec!["Finance", "Inventé", "Santé"]);
        assert_eq!(parsed.license_plate.as_deref(), Some("VD 123456"));
        assert_eq!(parsed.year.as_deref(), Some("2024"));
        let allow = vec!["Finance".into(), "Santé".into()];
        assert_eq!(
            filter_llm_tags(&parsed.tags, &allow),
            vec!["Finance", "Santé"]
        );
    }
}
