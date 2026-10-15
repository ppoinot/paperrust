use sqlx::{PgPool, Row};

use crate::matching::{match_entity_pre, normalize};
use crate::models::*;
use crate::ocr::{
    fallback_ocr, filter_llm_tags, is_ocr_noise_tag, may_ensure_ocr_tag, names_eq_ci,
    owner_tag_allowlist, resolve_key, run_llm_ocr, OcrResult,
};
use crate::paper::{render_paper_svg, PaperSpec};
use crate::queries::{allocate_asn, list_correspondents, list_paths, list_tags, list_types, log_audit};

/// Hard cap for document upload / consume (multipart or base64).
/// Single source: [`crate::limits::MAX_UPLOAD_BYTES`] (/48).
pub const MAX_UPLOAD_BYTES: usize = crate::limits::MAX_UPLOAD_BYTES;

/// Skip embedding `file_data` data-URLs above this size (raw stays in `original_bytes`).
const MAX_EMBED_FILE_DATA: usize = 2 * 1024 * 1024;

/// Opaque OCR failure codes for clients / job summary ().
/// Never forward raw provider body snippets — classify only.
fn opaque_ocr_provider_error(err: &anyhow::Error) -> &'static str {
    let lower = err.to_string().to_ascii_lowercase();
    if lower.contains("unsupported_ocr_provider") {
        return "ocr_unsupported_provider";
    }
    if lower.contains("incorrect api key")
        || lower.contains("invalid api key")
        || lower.contains("invalid_api_key")
        || lower.contains("authentication")
        || lower.contains("unauthorized")
        || lower.contains(" 401")
        || lower.contains("http 401")
        || lower.contains("api key")
        || lower.contains("missing key")
    {
        return "ocr_auth_error";
    }
    if lower.contains("rate limit") || lower.contains("429") || lower.contains("http 429") {
        return "ocr_rate_limited";
    }
    "ocr_provider_error"
}

/// only xai|openai|anthropic|selfhosted may drive OCR.
fn require_consume_provider(raw: &str) -> anyhow::Result<&'static str> {
    crate::llm_keys::normalize_consume_provider(raw).ok_or_else(|| {
        anyhow::anyhow!("unsupported_ocr_provider")
    })
}

pub(crate) async fn ensure_named(pool: &PgPool, table: &str, name: &str, extra_color: Option<&str>, owner_id: i32) -> sqlx::Result<i32> {
    let existing: Option<i32> = sqlx::query_scalar(&format!(
        "select id from {table} where owner_id = $1 and lower(name) = lower($2)"
    ))
    .bind(owner_id)
    .bind(name)
    .fetch_optional(pool)
    .await?;
    if let Some(id) = existing {
        return Ok(id);
    }
    match table {
        "tags" => {
            let color = extra_color.unwrap_or("#57534e");
            sqlx::query_scalar(
                "insert into tags (name, color, matching_algorithm, owner_id) values ($1,$2,'auto',$3) returning id",
            )
            .bind(name)
            .bind(color)
            .bind(owner_id)
            .fetch_one(pool)
            .await
        }
        "correspondents" => sqlx::query_scalar(
            "insert into correspondents (name, match, matching_algorithm, owner_id) values ($1,$2,'auto',$3) returning id",
        )
        .bind(name)
        .bind(name)
        .bind(owner_id)
        .fetch_one(pool)
        .await,
        _ => sqlx::query_scalar(
            "insert into document_types (name, match, matching_algorithm, owner_id) values ($1,$2,'auto',$3) returning id",
        )
        .bind(name)
        .bind(name)
        .bind(owner_id)
        .fetch_one(pool)
        .await,
    }
}

async fn ensure_storage_path(pool: &PgPool, name: &str, path: &str, owner_id: i32) -> sqlx::Result<i32> {
    let existing: Option<i32> = sqlx::query_scalar(
        "select id from storage_paths where owner_id = $1 and lower(name) = lower($2)",
    )
    .bind(owner_id)
    .bind(name)
    .fetch_optional(pool)
    .await?;
    if let Some(id) = existing {
        return Ok(id);
    }
    sqlx::query_scalar(
        "insert into storage_paths (owner_id, name, path, matching_algorithm) values ($1,$2,$3,'none') returning id",
    )
    .bind(owner_id)
    .bind(name)
    .bind(path)
    .fetch_one(pool)
    .await
}

async fn apply_workflows(
    pool: &PgPool,
    document_id: i32,
    owner_id: i32,
    content: &str,
    trigger: &str,
) -> sqlx::Result<()> {
    let content_lc = content.to_lowercase();
    let workflows = sqlx::query(
        r#"select w.id, w.enabled, w.trigger_type, w.filters::text as filters, w.actions::text as actions
           from workflows w
           join documents d on d.id = $1
           where w.enabled = true and w.owner_id = d.owner_id"#,
    )
    .bind(document_id)
    .fetch_all(pool)
    .await?;
    let corr: Option<i32> = sqlx::query_scalar::<_, Option<i32>>("select correspondent_id from documents where id = $1")
        .bind(document_id)
        .fetch_one(pool)
        .await?;
    for wf in workflows {
        let trigger_type: String = wf.get("trigger_type");
        if trigger_type != trigger && trigger_type != "document_added" {
            continue;
        }
        let filters_s: String = wf.get("filters");
        let filters: serde_json::Value = serde_json::from_str(&filters_s).unwrap_or(serde_json::json!({}));
        if let Some(contains) = filters.get("contentContains").and_then(|v| v.as_str()) {
            if !content_lc.contains(&contains.to_lowercase()) {
                continue;
            }
        }
        if filters.get("hasCorrespondent").and_then(|v| v.as_bool()) == Some(true) && corr.is_none() {
            continue;
        }
        let actions: Vec<WorkflowAction> =
            serde_json::from_str(&wf.get::<String, _>("actions")).unwrap_or_default();
        for action in actions {
            match action.type_.as_str() {
                "add_tag" => {
                    if let Some(tag_id) = action.tag_id {
                        sqlx::query(
                            r#"insert into document_tags (document_id, tag_id)
                               select $1, $2 from tags t
                               join documents d on d.id = $1
                               where t.id = $2 and t.owner_id = d.owner_id
                               on conflict do nothing"#,
                        )
                        .bind(document_id)
                        .bind(tag_id)
                        .execute(pool)
                        .await?;
                    }
                }
                "remove_tag" => {
                    if let Some(tag_id) = action.tag_id {
                        sqlx::query("delete from document_tags where document_id = $1 and tag_id = $2")
                            .bind(document_id)
                            .bind(tag_id)
                            .execute(pool)
                            .await?;
                    }
                }
                "set_correspondent" => {
                    if let Some(id) = action.correspondent_id {
                        sqlx::query(
                            r#"update documents set correspondent_id = $1
                               where id = $2 and owner_id = $3 and exists (
                                 select 1 from correspondents c
                                 where c.id = $1 and c.owner_id = documents.owner_id
                               )"#,
                        )
                            .bind(id)
                            .bind(document_id)
                            .bind(owner_id)
                            .execute(pool)
                            .await?;
                    }
                }
                "set_type" => {
                    if let Some(id) = action.document_type_id {
                        sqlx::query(
                            r#"update documents set document_type_id = $1
                               where id = $2 and owner_id = $3 and exists (
                                 select 1 from document_types dt
                                 where dt.id = $1 and dt.owner_id = documents.owner_id
                               )"#,
                        )
                            .bind(id)
                            .bind(document_id)
                            .bind(owner_id)
                            .execute(pool)
                            .await?;
                    }
                }
                "set_storage" => {
                    if let Some(id) = action.storage_path_id {
                        sqlx::query(
                            r#"update documents set storage_path_id = $1
                               where id = $2 and owner_id = $3 and exists (
                                 select 1 from storage_paths sp
                                 where sp.id = $1 and sp.owner_id = documents.owner_id
                               )"#
                        )
                            .bind(id)
                            .bind(document_id)
                            .bind(owner_id)
                            .execute(pool)
                            .await?;
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}


pub async fn load_ocr_provider(pool: &PgPool, owner_id: i32) -> String {
    let p: Option<String> = sqlx::query_scalar(
        "select nullif(trim(ocr_provider), '') from user_settings where user_id = $1",
    )
    .bind(owner_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    match p.as_deref().and_then(crate::llm_keys::normalize_consume_provider) {
        Some(x) => x.to_string(),
        None => "xai".into(),
    }
}

/// Replace stored PDF bytes, invalidate preview cache, re-run OCR (same as `:id/ocr`).
pub async fn replace_document_pdf(
    pool: &PgPool,
    document_id: i32,
    owner_id: i32,
    raw: Vec<u8>,
    filename: &str,
) -> anyhow::Result<serde_json::Value> {
    if raw.len() > MAX_UPLOAD_BYTES {
        anyhow::bail!(
            "Fichier trop volumineux (max {} Mo).",
            MAX_UPLOAD_BYTES / (1024 * 1024)
        );
    }
    if !crate::preview::looks_like_pdf(&raw) {
        anyhow::bail!("Le fichier édité doit être un PDF.");
    }
    let filename = crate::limits::clamp_filename(filename);
    let filename = if filename.to_ascii_lowercase().ends_with(".pdf") && !filename.is_empty() {
        filename
    } else if filename.is_empty() {
        "document.pdf".into()
    } else {
        format!("{filename}.pdf")
    };
    let checksum = format!("sha-{document_id}-{}", raw.len());
    // Keep the previous PDF before overwriting it, so an
    // unwanted/malicious Stirling commit is recoverable.
    let saved = sqlx::query(
        r#"insert into document_previous_versions
             (document_id, owner_id, original_bytes, original_filename, mime_type, reason)
           select id, owner_id, original_bytes, original_filename, mime_type, 'stirling_replace'
           from documents
           where id = $1 and owner_id = $2 and deleted_at is null
             and original_bytes is not null"#,
    )
    .bind(document_id)
    .bind(owner_id)
    .execute(pool)
    .await?;
    if saved.rows_affected() == 0 {
        anyhow::bail!("document introuvable");
    }
    crate::preview::clear_pages(pool, document_id).await?;
    let updated = sqlx::query(
        r#"update documents set
            original_bytes = $1,
            mime_type = 'application/pdf',
            original_filename = $2,
            file_size = $3,
            file_data = null,
            checksum = $4,
            ocr_status = 'pending',
            summary = 'Fichier remplacé — OCR en cours',
            modified_at = now()
          where id = $5 and owner_id = $6 and deleted_at is null"#,
    )
    .bind(&raw)
    .bind(&filename)
    .bind(raw.len() as i32)
    .bind(&checksum)
    .bind(document_id)
    .bind(owner_id)
    .execute(pool)
    .await?;
    if updated.rows_affected() == 0 {
        anyhow::bail!("document introuvable");
    }
    log_audit(
        pool,
        document_id,
        "file_replaced",
        Some("PDF remplacé (Stirling)"),
    )
    .await?;
    let provider = load_ocr_provider(pool, owner_id).await;
    let ocr = re_ocr_document(
        pool,
        crate::models::OcrInput {
            id: document_id,
            provider,
            api_key: None,
            data_base64: None,
            mime_type: Some("application/pdf".into()),
        },
        owner_id,
    )
    .await?;
    Ok(ocr)
}

async fn load_ocr_base_url(pool: &PgPool, owner_id: i32) -> Option<String> {
    sqlx::query_scalar::<_, String>(
        "select nullif(trim(ocr_base_url), '') from user_settings where user_id = $1",
    )
    .bind(owner_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
}

/// Collect tag names to attach: owner-allowlisted OCR picks, `matching_algorithm`
/// (including `auto` text match) as a supplement, année YYYY if visible,
/// forced Véhicule when a plate is present, Inbox if the OCR/fallback asked for it.
/// LLM inventions never enter this set unless they already exist on the owner.
pub(crate) fn classify_tag_names(
    owner_tags: &[Tag],
    ocr: &OcrResult,
    hay_ci: &str,
    hay_cs: &str,
    haystack: &str,
    vehicle_year: &str,
) -> Vec<String> {
    let allow = owner_tag_allowlist(owner_tags.iter().map(|t| t.name.as_str()));
    let mut wanted: std::collections::HashSet<String> =
        filter_llm_tags(&ocr.tags, &allow).into_iter().collect();

    // matching_algorithm=auto (and others) stay as a supplement — existing labels only.
    for tag in owner_tags {
        if is_ocr_noise_tag(&tag.name) {
            continue;
        }
        if tag.is_inbox_tag {
            wanted.insert(tag.name.clone());
        } else if match_entity_pre(
            &tag.name,
            &tag.match_text,
            &tag.matching_algorithm,
            tag.is_insensitive,
            hay_ci,
            hay_cs,
            haystack,
        ) {
            wanted.insert(tag.name.clone());
        }
    }

    let year = ocr.created_date.as_deref().unwrap_or("").get(..4).unwrap_or("");
    if year.chars().all(|c| c.is_ascii_digit()) && year.len() == 4 {
        wanted.insert(year.to_string());
    }
    if ocr.year.as_deref().is_some_and(crate::ocr::is_year_tag) {
        if let Some(y) = &ocr.year {
            wanted.insert(y.trim().to_string());
        }
    }
    let has_plate = ocr.license_plate.as_deref().filter(|s| !s.is_empty()).is_some();
    if has_plate {
        wanted.insert("Véhicule".into());
        wanted.insert(vehicle_year.to_string());
    }
    if ocr.tags.iter().any(|t| names_eq_ci(t, "Inbox")) {
        wanted.insert("Inbox".into());
    }

    let mut out: Vec<String> = wanted.into_iter().collect();
    out.sort();
    out
}

/// Attach an existing owner tag, or `ensure_named` only for année YYYY / Inbox /
/// Véhicule-when-plated. Never create a tag the LLM invented.
async fn resolve_classify_tag_id(
    pool: &PgPool,
    owner_tags: &[Tag],
    name: &str,
    license_plate: bool,
    owner_id: i32,
) -> sqlx::Result<Option<i32>> {
    if is_ocr_noise_tag(name) {
        return Ok(None);
    }
    if let Some(existing) = owner_tags.iter().find(|t| names_eq_ci(&t.name, name)) {
        return Ok(Some(existing.id));
    }
    if !may_ensure_ocr_tag(name, license_plate) {
        return Ok(None);
    }
    let color = if names_eq_ci(name, "Inbox") {
        Some("#9a4a28")
    } else {
        None
    };
    Ok(Some(ensure_named(pool, "tags", name, color, owner_id).await?))
}

pub async fn auto_classify(pool: &PgPool, document_id: i32, content: &str, ocr: &OcrResult, owner_id: i32) -> sqlx::Result<()> {
    let (tags, correspondents, types, paths) = tokio::try_join!(
        list_tags(pool, owner_id),
        list_correspondents(pool, owner_id),
        list_types(pool, owner_id),
        list_paths(pool, owner_id),
    )?;
    let haystack = format!(
        "{} {} {}",
        ocr.title,
        content,
        ocr.correspondent.as_deref().unwrap_or("")
    );
    let hay_ci = normalize(&haystack, true);
    let hay_cs = normalize(&haystack, false);

    let correspondent_id = if let Some(name) = ocr.correspondent.as_ref().filter(|s| !s.is_empty()) {
        Some(ensure_named(pool, "correspondents", name, None, owner_id).await?)
    } else {
        correspondents.iter().find(|c| {
            match_entity_pre(
                &c.name,
                &c.match_text,
                &c.matching_algorithm,
                c.is_insensitive,
                &hay_ci,
                &hay_cs,
                &haystack,
            )
        }).map(|c| c.id)
    };

    let type_id = if let Some(name) = ocr.document_type.as_ref().filter(|s| !s.is_empty()) {
        Some(ensure_named(pool, "document_types", name, None, owner_id).await?)
    } else {
        types.iter().find(|t| {
            match_entity_pre(
                &t.name,
                &t.match_text,
                &t.matching_algorithm,
                t.is_insensitive,
                &hay_ci,
                &hay_cs,
                &haystack,
            )
        }).map(|t| t.id)
    };

    let vehicle_year = ocr
        .created_date
        .as_deref()
        .and_then(|d| d.get(..4))
        .filter(|y| y.chars().all(|c| c.is_ascii_digit()) && y.len() == 4)
        .map(str::to_string)
        .or_else(|| ocr.year.clone())
        .unwrap_or_else(|| chrono::Utc::now().format("%Y").to_string());

    let path_id = if ocr.license_plate.as_deref().filter(|s| !s.is_empty()).is_some() {
        let name = format!("Véhicule/{vehicle_year}");
        let path = format!("Véhicule/{vehicle_year}/{{title}}");
        Some(ensure_storage_path(pool, &name, &path, owner_id).await?)
    } else {
        paths.iter().find(|p| {
            match_entity_pre(
                &p.name,
                &p.match_text,
                &p.matching_algorithm,
                p.is_insensitive,
                &hay_ci,
                &hay_cs,
                &haystack,
            )
        }).map(|p| p.id)
    };

    sqlx::query(
        r#"update documents set
          correspondent_id = case
            when $1::int is null then correspondent_id
            when exists (select 1 from correspondents c where c.id = $1 and c.owner_id = $5) then $1
            else correspondent_id end,
          document_type_id = case
            when $2::int is null then document_type_id
            when exists (select 1 from document_types dt where dt.id = $2 and dt.owner_id = $5) then $2
            else document_type_id end,
          storage_path_id = case
            when $3::int is null then storage_path_id
            when exists (select 1 from storage_paths sp where sp.id = $3 and sp.owner_id = $5) then $3
            else storage_path_id end,
          modified_at = now()
         where id = $4 and owner_id = $5"#,
    )
    .bind(correspondent_id)
    .bind(type_id)
    .bind(path_id)
    .bind(document_id)
    .bind(owner_id)
    .execute(pool)
    .await?;

    let has_plate = ocr.license_plate.as_deref().filter(|s| !s.is_empty()).is_some();
    let names_to_attach = classify_tag_names(&tags, ocr, &hay_ci, &hay_cs, &haystack, &vehicle_year);
    for name in names_to_attach {
        let Some(id) = resolve_classify_tag_id(pool, &tags, &name, has_plate, owner_id).await? else {
            continue;
        };
        sqlx::query("insert into document_tags (document_id, tag_id) values ($1,$2) on conflict do nothing")
            .bind(document_id)
            .bind(id)
            .execute(pool)
            .await?;
    }

    async fn set_field(
        pool: &PgPool,
        document_id: i32,
        field_name: &str,
        value: &str,
        owner_id: i32,
    ) -> sqlx::Result<()> {
        // resolve custom field by name + owner only.
        let field: Option<i32> = sqlx::query_scalar(
            "select id from custom_fields where name = $1 and owner_id = $2",
        )
        .bind(field_name)
        .bind(owner_id)
        .fetch_optional(pool)
        .await?;
        if let Some(fid) = field {
            sqlx::query(
                r#"insert into document_custom_values (document_id, field_id, value) values ($1,$2,$3)
                   on conflict (document_id, field_id) do update set value = excluded.value"#,
            )
            .bind(document_id)
            .bind(fid)
            .bind(value)
            .execute(pool)
            .await?;
        }
        Ok(())
    }
    if let Some(v) = &ocr.amount {
        set_field(pool, document_id, "Montant", v, owner_id).await?;
    }
    if let Some(v) = &ocr.currency {
        set_field(pool, document_id, "Monnaie", v, owner_id).await?;
    }
    if let Some(v) = &ocr.reference {
        set_field(pool, document_id, "N° de référence", v, owner_id).await?;
    }
    if let Some(paid) = ocr.paid {
        set_field(pool, document_id, "Payé", if paid { "true" } else { "false" }, owner_id).await?;
    }
    apply_workflows(pool, document_id, owner_id, &haystack, "document_added").await?;
    Ok(())
}

pub async fn consume_document(pool: &PgPool, mut input: ConsumeInput, owner_id: i32) -> anyhow::Result<ConsumeResult> {
    // refuse kimi/mistral / unknown before any OCR call.
    input.provider = require_consume_provider(&input.provider)?.to_string();
    input.filename = crate::limits::clamp_filename(&input.filename);
    if let Some(t) = input.title_override.as_mut() {
        *t = crate::limits::clamp_title(t);
    }
    let raw = if let Some(bytes) = input.raw_bytes.take() {
        bytes
    } else {
        let b64_in = input.data_base64.trim();
        let b64 = if let Some(rest) = b64_in.strip_prefix("data:") {
            rest.split_once(',').map(|(_, d)| d).unwrap_or(b64_in)
        } else {
            b64_in
        };
        base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64)
            .or_else(|_| base64::Engine::decode(&base64::engine::general_purpose::STANDARD_NO_PAD, b64))
            .map_err(|_| anyhow::anyhow!("Base64 invalide"))?
    };
    if raw.len() as u64 > MAX_UPLOAD_BYTES as u64 {
        anyhow::bail!(
            "Fichier trop volumineux (max {} Mo).",
            MAX_UPLOAD_BYTES / (1024 * 1024)
        );
    }
    // Browsers often send application/octet-stream; sniff magic so mime + /file stay PDF/PNG/JPEG.
    {
        let m = input.mime_type.to_ascii_lowercase();
        let sniff = m.is_empty()
            || m == "application/octet-stream"
            || m == "binary/octet-stream"
            || m.contains("svg");
        if raw.starts_with(b"%PDF") {
            if sniff {
                input.mime_type = "application/pdf".into();
            }
        } else if crate::preview::looks_like_png(raw.as_ref()) {
            if sniff || !m.starts_with("image/") {
                input.mime_type = "image/png".into();
            }
        } else if crate::preview::looks_like_jpeg(raw.as_ref()) {
            if sniff || !m.starts_with("image/") {
                input.mime_type = "image/jpeg".into();
            }
        }
    }
    // magic allowlist — reject anything that is not PDF/PNG/JPEG.
    {
        let ok = raw.starts_with(b"%PDF")
            || crate::preview::looks_like_png(raw.as_ref())
            || crate::preview::looks_like_jpeg(raw.as_ref());
        if !ok {
            anyhow::bail!("Type de fichier non supporté (PDF, PNG ou JPEG uniquement).");
        }
        let m = input.mime_type.to_ascii_lowercase();
        let allowed = m == "application/pdf"
            || m == "image/png"
            || m == "image/jpeg"
            || m == "image/jpg";
        if !allowed {
            // Force mime from magic when client lied.
            if raw.starts_with(b"%PDF") {
                input.mime_type = "application/pdf".into();
            } else if crate::preview::looks_like_png(raw.as_ref()) {
                input.mime_type = "image/png".into();
            } else {
                input.mime_type = "image/jpeg".into();
            }
        }
    }
    let job_id: i32 = sqlx::query_scalar(
        "insert into consume_jobs (filename, status, message, owner_id) values ($1,'processing','OCR en cours',$2) returning id",
    )
    .bind(&input.filename)
    .bind(owner_id)
    .fetch_one(pool)
    .await?;

    let mut ocr = fallback_ocr(&input.filename);
    if let Some(t) = input
        .title_override
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        ocr.title = t.to_string();
    }
    if let Some(d) = input
        .created_override
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let date = d.get(..10).unwrap_or(d).to_string();
        if date.len() == 10 {
            ocr.created_date = Some(date);
        }
    }
    let mut used_llm = false;
    let mut error: Option<String> = None;
    let user_key = crate::llm_keys::get_decrypted(pool, owner_id, &input.provider)
        .await
        .ok()
        .flatten();
    let key = resolve_key(
        &input.provider,
        user_key.as_deref(),
        input.api_key.as_deref(),
    );
    // never honor consume `baseUrl` — settings `ocrBaseUrl` only.
    let base_url = load_ocr_base_url(pool, owner_id).await;
    let skip = input.skip_ocr.unwrap_or(false);
    let selfhosted_ok = input.provider == "selfhosted" && base_url.is_some();
    // Selfhosted may omit API key; other providers need a key.
    let can_call = if input.provider == "selfhosted" {
        selfhosted_ok
    } else {
        key.is_some()
    };

    // Arc avoids a full raw clone for PDFium; semaphore bounds concurrency.
    let raw = std::sync::Arc::<[u8]>::from(raw.into_boxed_slice());
    let mime_for_preview = input.mime_type.clone();
    let raw_for_preview = std::sync::Arc::clone(&raw);
    let rendered_pages = match crate::limits::run_heavy_blocking(
        std::time::Duration::from_secs(crate::preview::RENDER_TIMEOUT_SECS),
        move || crate::preview::render_all(&raw_for_preview, &mime_for_preview),
    )
    .await
    {
        Ok(pages) => Some(pages),
        Err(e) => {
            tracing::warn!(error = %e, "consume preview render failed");
            None
        }
    };
    if let Some(pages) = rendered_pages.as_ref() {
        tracing::info!("consume preview: {} page(s) for {}", pages.len(), input.filename);
    } else {
        tracing::info!("consume preview skipped (no raster) for {}", input.filename);
    }

    // JPEG pages only — never send paper SVG or image/svg+xml to Grok.
    // borrow rendered pages for OCR (no clone); store_pages still uses the same Vec.
    let rendered_pages = rendered_pages.unwrap_or_default();
    let can_vision = !rendered_pages.is_empty();

    // Owner labels for the OCR prompt (noise `ai-*` dropped in owner_tag_allowlist).
    let owner_tag_names: Vec<String> = list_tags(pool, owner_id)
        .await?
        .into_iter()
        .map(|t| t.name)
        .collect();
    let ocr_allowlist = owner_tag_allowlist(owner_tag_names.iter().map(String::as_str));

    if !skip && can_call && can_vision {
        match run_llm_ocr(
            &input.provider,
            key.as_deref().unwrap_or(""),
            &input.filename,
            &rendered_pages,
            base_url.as_deref(),
            &ocr_allowlist,
        )
        .await
        {
            Ok(res) => {
                ocr = res;
                used_llm = true;
            }
            Err(err) => {
                tracing::warn!(error = %crate::ocr::redact_for_log(&err.to_string()), "consume ocr failed");
                let code = opaque_ocr_provider_error(&err);
                error = Some(code.to_string());
                ocr.summary = code.to_string();
                ocr.tags = vec!["Inbox".into()];
            }
        }
    } else if !can_vision {
        ocr.content = String::new();
        ocr.tags = vec!["Inbox".into()];
        ocr.summary = if input.mime_type.starts_with("image/") {
            "Aperçu image indisponible — impossible de préparer l'OCR Vision pour ce fichier."
                .into()
        } else {
            "Prévisualisation PDF : lancez l'OCR (Vision) depuis la fiche après rendu.".into()
        };
    } else {
        // No LLM key (or skip_ocr): keep fallback OCR (filename title) + Inbox.
        // This is not a failure — upload must succeed in the UI.
        ocr.tags = vec!["Inbox".into()];
        if key.is_none() && !skip {
            ocr.summary =
                "Aucune clé API pour ce fournisseur — ajoutez-la dans Réglages → OCR.".into();
        }
    }

    ocr.title = crate::limits::clamp_title(&ocr.title);
    let date = ocr
        .created_date
        .clone()
        .unwrap_or_else(|| chrono::Utc::now().format("%Y-%m-%d").to_string());
    let lines: Vec<String> = (if ocr.content.is_empty() {
        "En attente de texte OCR."
    } else {
        ocr.content.as_str()
    })
    .split('\n')
    .map(str::trim)
    .filter(|l| !l.is_empty())
    .take(18)
    .map(str::to_string)
    .collect();
    // allocate ASN under advisory lock in the same txn as INSERT so races
    // cannot collide; on insert failure after OCR was paid, mark the job failed.
    let mut tx = pool.begin().await?;
    let asn = match allocate_asn(&mut tx, owner_id).await {
        Ok(a) => a,
        Err(e) => {
            let _ = tx.rollback().await;
            let msg = format!("Échec allocation ASN: {e}");
            let _ = sqlx::query(
                "update consume_jobs set status = 'failed', message = $1, finished_at = now() where id = $2",
            )
            .bind(&msg)
            .bind(job_id)
            .execute(pool)
            .await;
            return Err(e.into());
        }
    };
    let footer = format!("ASN {asn}  ·  {}", input.filename);
    let stamp = if used_llm { "OCR" } else { "IMPORT" };
    let kicker = ocr.document_type.clone().unwrap_or_else(|| "Document".into());
    let corr_s = ocr.correspondent.clone().unwrap_or_else(|| "Inconnu".into());
    let thumbnail = if input.mime_type.starts_with("image/") && input.mime_type != "image/svg+xml" {
        None
    } else {
        Some(render_paper_svg(PaperSpec {
            kicker: Some(&kicker),
            title: &ocr.title,
            correspondent: &corr_s,
            date: &date,
            r#ref: None,
            lines: &lines,
            footer: Some(&footer),
            stamp: Some(stamp),
            accent: None,
        }))
    };
    // Raster images keep a small data-URL fallback; never store SVG/paper as file_data.
    // original_bytes is always the raw upload (PDF/PNG/JPEG), not this data-URL.
    // Huge scans stay in original_bytes only — embedding 100+ Mo base64 freezes INSERT/UI.
    let file_data = if raw.len() > MAX_EMBED_FILE_DATA {
        None
    } else if crate::preview::looks_like_png(raw.as_ref()) {
        Some(format!(
            "data:image/png;base64,{}",
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, raw.as_ref())
        ))
    } else if crate::preview::looks_like_jpeg(raw.as_ref()) {
        Some(format!(
            "data:image/jpeg;base64,{}",
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, raw.as_ref())
        ))
    } else {
        None
    };
    let page_count = rendered_pages.len().max(1) as i32;
    let ocr_status = if used_llm {
        "done"
    } else if error.is_some() {
        "failed"
    } else if skip || (key.is_none() && can_vision) {
        "skipped"
    } else {
        "pending"
    };
    let ocr_provider: Option<&str> = if used_llm { Some(&input.provider) } else { None };
    let checksum = format!("sha-{asn}-{}", raw.len());
    let document_id: i32 = match sqlx::query_scalar(
        r#"insert into documents (
      title, content, created_date, asn, checksum, mime_type, original_filename,
      page_count, language, ocr_status, ocr_provider, thumbnail_svg, file_data, file_size, summary,
      original_bytes, owner_id
    ) values (
      $1,$2,$3::date,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17
    ) returning id"#,
    )
    .bind(&ocr.title)
    .bind(&ocr.content)
    .bind(&ocr.created_date)
    .bind(asn)
    .bind(&checksum)
    .bind(&input.mime_type)
    .bind(&input.filename)
    .bind(page_count)
    .bind(&ocr.language)
    .bind(ocr_status)
    .bind(ocr_provider)
    .bind(&thumbnail)
    .bind(&file_data)
    .bind(raw.len() as i32)
    .bind(&ocr.summary)
    .bind(raw.as_ref()) // original_bytes: raw PDF/PNG/JPEG bytes, never the paper SVG / data-URL
    .bind(owner_id)
    .fetch_one(&mut *tx)
    .await
    {
        Ok(id) => id,
        Err(e) => {
            let _ = tx.rollback().await;
            // OCR may already have been billed — surface insert failure on the job.
            let msg = format!("Échec enregistrement document: {e}");
            let _ = sqlx::query(
                "update consume_jobs set status = 'failed', message = $1, finished_at = now() where id = $2",
            )
            .bind(&msg)
            .bind(job_id)
            .execute(pool)
            .await;
            return Err(e.into());
        }
    };
    tx.commit().await?;

    if !rendered_pages.is_empty() {
        if let Err(e) = crate::preview::store_pages(pool, document_id, owner_id, &rendered_pages).await {
            tracing::warn!("store preview pages: {e}");
        }
    }

    auto_classify(pool, document_id, &ocr.content, &ocr, owner_id).await?;
    let detail = if used_llm {
        format!("OCR {} · {}", input.provider, input.filename)
    } else {
        format!(
            "Import {}{}",
            input.filename,
            if error.is_some() { " · OCR échoué" } else { "" }.to_string()
        )
    };
    log_audit(pool, document_id, "consumed", Some(&detail)).await?;
    let job_status = if error.is_some() && !used_llm { "failed" } else { "success" };
    sqlx::query("update consume_jobs set status = $1, message = $2, document_id = $3, finished_at = now() where id = $4")
        .bind(job_status)
        .bind(&ocr.summary)
        .bind(document_id)
        .bind(job_id)
        .execute(pool)
        .await?;
    Ok(ConsumeResult {
        document_id,
        ocr,
        used_llm,
        error,
    })
}

pub async fn re_ocr_document(pool: &PgPool, mut opts: OcrInput, owner_id: i32) -> anyhow::Result<serde_json::Value> {
    // refuse kimi/mistral / unknown (would otherwise hit x.ai with wrong key).
    let Some(p) = crate::llm_keys::normalize_consume_provider(&opts.provider) else {
        return Ok(serde_json::json!({
            "ok": false,
            "error": "ocr_unsupported_provider"
        }));
    };
    opts.provider = p.to_string();
    let row = sqlx::query("select original_filename, mime_type, original_bytes from documents where id = $1 and owner_id = $2")
        .bind(opts.id)
        .bind(owner_id)
        .fetch_optional(pool)
        .await?;
    let Some(row) = row else {
        return Ok(serde_json::json!({"ok": false, "error": "Introuvable"}));
    };
    let orig: Option<String> = row.get("original_filename");
    let mime: String = row.get("mime_type");
    let original_bytes: Option<Vec<u8>> = row.get("original_bytes");
    let Some(bytes) = original_bytes.filter(|b| !b.is_empty()) else {
        return Ok(serde_json::json!({
            "ok": false,
            "error": "Fichier source absent — réimportez le document pour relancer l'OCR."
        }));
    };
    let mut jpegs = crate::preview::load_cached_pages(pool, opts.id).await.unwrap_or_default();
    if jpegs.is_empty() {
        let mime_c = mime.clone();
        jpegs = match crate::limits::run_heavy_blocking(
            std::time::Duration::from_secs(crate::preview::RENDER_TIMEOUT_SECS),
            move || crate::preview::render_all(&bytes, &mime_c),
        )
        .await
        {
            Ok(pages) => pages,
            Err(e) => {
                tracing::warn!(error = %e, "preview render_all failed");
                Vec::new()
            }
        };
        if !jpegs.is_empty() {
            let _ = crate::preview::store_pages(pool, opts.id, owner_id, &jpegs).await;
        }
    }
    if jpegs.is_empty() {
        return Ok(serde_json::json!({
            "ok": false,
            "error": "Impossible de générer une image pour l'OCR (fichier trop grand ou format non pris en charge)."
        }));
    }
    let user_key = crate::llm_keys::get_decrypted(pool, owner_id, &opts.provider)
        .await
        .ok()
        .flatten();
    let key = resolve_key(
        &opts.provider,
        user_key.as_deref(),
        opts.api_key.as_deref(),
    );
    let base_url = load_ocr_base_url(pool, owner_id).await;
    if opts.provider == "selfhosted" {
        if base_url.as_deref().unwrap_or("").is_empty() {
            return Ok(serde_json::json!({
                "ok": false,
                "error": "URL de base Auto-hébergé manquante — renseignez-la dans Réglages → OCR."
            }));
        }
    } else if key.is_none() {
        return Ok(serde_json::json!({
            "ok": false,
            "error": "Clé API absente pour ce fournisseur. Ajoutez-la dans Réglages → OCR."
        }));
    }
    let owner_tag_names: Vec<String> = list_tags(pool, owner_id)
        .await?
        .into_iter()
        .map(|t| t.name)
        .collect();
    let ocr_allowlist = owner_tag_allowlist(owner_tag_names.iter().map(String::as_str));
    match run_llm_ocr(
        &opts.provider,
        key.as_deref().unwrap_or(""),
        orig.as_deref().unwrap_or("document"),
        &jpegs,
        base_url.as_deref(),
        &ocr_allowlist,
    )
    .await
    {
        Ok(ocr) => {
            sqlx::query(
                r#"update documents set title = $1, content = $2, created_date = coalesce($3::date, created_date),
                    language = $4, ocr_status = 'done', ocr_provider = $5, summary = $6, modified_at = now()
                   where id = $7 and owner_id = $8"#,
            )
            .bind(crate::limits::clamp_title(&ocr.title))
            .bind(&ocr.content)
            .bind(&ocr.created_date)
            .bind(&ocr.language)
            .bind(&opts.provider)
            .bind(&ocr.summary)
            .bind(opts.id)
            .bind(owner_id)
            .execute(pool)
            .await?;
            auto_classify(pool, opts.id, &ocr.content, &ocr, owner_id).await?;
            log_audit(pool, opts.id, "ocr", Some(&format!("Relance OCR {}", opts.provider))).await?;
            Ok(serde_json::json!({"ok": true}))
        }
        Err(err) => {
            tracing::warn!(error = %crate::ocr::redact_for_log(&err.to_string()), "re-ocr failed");
            let code = opaque_ocr_provider_error(&err);
            sqlx::query(
                "update documents set ocr_status = 'failed', summary = $2, modified_at = now() where id = $1 and owner_id = $3",
            )
            .bind(opts.id)
            .bind(code)
            .bind(owner_id)
            .execute(pool)
            .await?;
            Ok(serde_json::json!({"ok": false, "error": code}))
        }
    }
}

#[cfg(test)]
mod classify_tag_tests {
    use super::classify_tag_names;
    use crate::models::Tag;
    use crate::ocr::{may_ensure_ocr_tag, OcrResult};

    fn tag(id: i32, name: &str, algo: &str, match_text: &str, inbox: bool) -> Tag {
        Tag {
            id,
            name: name.into(),
            color: "#57534e".into(),
            is_inbox_tag: inbox,
            match_text: match_text.into(),
            matching_algorithm: algo.into(),
            is_insensitive: true,
        }
    }

    fn ocr(tags: &[&str], plate: Option<&str>, created: Option<&str>, year: Option<&str>) -> OcrResult {
        OcrResult {
            title: "Doc".into(),
            content: "facture CHF 120".into(),
            correspondent: None,
            document_type: None,
            created_date: created.map(str::to_string),
            tags: tags.iter().map(|s| (*s).to_string()).collect(),
            language: "fr".into(),
            summary: String::new(),
            amount: None,
            currency: None,
            reference: None,
            paid: None,
            license_plate: plate.map(str::to_string),
            year: year.map(str::to_string),
            provider: "xai".into(),
        }
    }

    #[test]
    fn llm_inventions_dropped_owner_and_year_kept() {
        let owner = vec![
            tag(1, "Finance", "auto", "", false),
            tag(2, "Santé", "none", "", false),
            tag(3, "ai-done", "none", "", false),
        ];
        let result = ocr(
            &["finance", "Inventé", "ai-pending"],
            None,
            Some("2024-03-01"),
            None,
        );
        let hay = "facture CHF 120 Doc";
        let names = classify_tag_names(&owner, &result, &hay.to_lowercase(), hay, hay, "2024");
        assert!(names.iter().any(|n| n == "Finance"));
        assert!(names.iter().any(|n| n == "2024"));
        assert!(!names.iter().any(|n| n == "Inventé"));
        assert!(!names.iter().any(|n| n.eq_ignore_ascii_case("ai-done")));
        assert!(!names.iter().any(|n| n.eq_ignore_ascii_case("ai-pending")));
        assert!(!may_ensure_ocr_tag("Inventé", false));
    }

    #[test]
    fn auto_text_match_supplements_owner_labels() {
        let owner = vec![tag(1, "Finance", "auto", "", false)];
        let result = ocr(&[], None, None, None);
        let hay = "facture CHF finance 120";
        let names = classify_tag_names(&owner, &result, &hay.to_lowercase(), hay, hay, "2026");
        assert!(names.iter().any(|n| n == "Finance"));
    }

    #[test]
    fn plate_forces_vehicule_and_year() {
        let owner = vec![tag(1, "Finance", "none", "", false)];
        let result = ocr(&["Finance"], Some("VD 123456"), Some("2023-01-02"), None);
        let hay = "immat VD 123456";
        let names = classify_tag_names(&owner, &result, &hay.to_lowercase(), hay, hay, "2023");
        assert!(names.iter().any(|n| n == "Véhicule"));
        assert!(names.iter().any(|n| n == "2023"));
        assert!(names.iter().any(|n| n == "Finance"));
        assert!(may_ensure_ocr_tag("Véhicule", true));
        assert!(!may_ensure_ocr_tag("Véhicule", false));
    }

    #[test]
    fn inbox_from_ocr_is_special_cased() {
        let owner = vec![tag(1, "Finance", "none", "", false)];
        let result = ocr(&["Inbox"], None, None, None);
        let hay = "x";
        let names = classify_tag_names(&owner, &result, hay, hay, hay, "2026");
        assert!(names.iter().any(|n| n == "Inbox"));
        assert!(may_ensure_ocr_tag("Inbox", false));
    }
}
