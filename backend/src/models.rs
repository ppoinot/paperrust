use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::sync::LazyLock;

/// Present-or-absent: missing field → None, JSON null/value → Some(inner).
pub fn deserialize_present<'de, T, D>(deserializer: D) -> Result<Option<T>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    T::deserialize(deserializer).map(Some)
}

fn default_tag_color() -> String {
    "#57534e".into()
}

fn default_algo_none() -> String {
    "none".into()
}

fn default_algo_auto() -> String {
    "auto".into()
}

fn default_provider() -> String {
    "xai".into()
}

fn default_data_type() -> String {
    "string".into()
}

fn default_sort_field() -> String {
    "created_date".into()
}

fn default_true() -> bool {
    true
}

fn default_json_obj() -> Value {
    Value::Object(Default::default())
}

fn default_json_arr() -> Value {
    Value::Array(Vec::new())
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Tag {
    pub id: i32,
    pub name: String,
    pub color: String,
    pub is_inbox_tag: bool,
    #[serde(rename = "match")]
    #[sqlx(rename = "match")]
    pub match_text: String,
    pub matching_algorithm: String,
    pub is_insensitive: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Correspondent {
    pub id: i32,
    pub name: String,
    #[serde(rename = "match")]
    #[sqlx(rename = "match")]
    pub match_text: String,
    pub matching_algorithm: String,
    pub is_insensitive: bool,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct DocumentType {
    pub id: i32,
    pub name: String,
    #[serde(rename = "match")]
    #[sqlx(rename = "match")]
    pub match_text: String,
    pub matching_algorithm: String,
    pub is_insensitive: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct StoragePath {
    pub id: i32,
    pub name: String,
    pub path: String,
    #[serde(rename = "match")]
    #[sqlx(rename = "match")]
    pub match_text: String,
    pub matching_algorithm: String,
    pub is_insensitive: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomField {
    pub id: i32,
    pub name: String,
    pub data_type: String,
    pub extra_data: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentNote {
    pub id: i32,
    pub body: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentListItem {
    pub id: i32,
    pub title: String,
    pub content: String,
    pub correspondent_id: Option<i32>,
    pub correspondent_name: Option<String>,
    pub document_type_id: Option<i32>,
    pub document_type_name: Option<String>,
    pub storage_path_id: Option<i32>,
    pub storage_path_name: Option<String>,
    pub created_date: Option<String>,
    pub added_at: String,
    pub modified_at: String,
    pub asn: Option<i32>,
    pub mime_type: String,
    pub original_filename: Option<String>,
    pub page_count: i32,
    pub language: String,
    pub ocr_status: String,
    pub ocr_provider: Option<String>,
    pub thumbnail_svg: Option<String>,
    pub file_size: i32,
    pub summary: Option<String>,
    pub tags: Vec<Tag>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomValue {
    pub field_id: i32,
    pub name: String,
    pub data_type: String,
    pub value: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryItem {
    pub id: i32,
    pub action: String,
    pub detail: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentDetail {
    #[serde(flatten)]
    pub item: DocumentListItem,
    pub file_data: Option<String>,
    /// True when downloadable original bytes exist (PDF/PNG/JPEG via `/file`).
    #[serde(default)]
    pub has_original: bool,
    pub notes: Vec<DocumentNote>,
    pub custom_values: Vec<CustomValue>,
    pub history: Vec<HistoryItem>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentFilter {
    pub q: Option<String>,
    pub tag_ids: Option<Vec<i32>>,
    pub correspondent_id: Option<i32>,
    pub document_type_id: Option<i32>,
    pub storage_path_id: Option<i32>,
    pub inbox_only: Option<bool>,
    /// Soft-deleted documents only (Corbeille).
    pub trash_only: Option<bool>,
    pub ocr_status: Option<String>,
    pub date_from: Option<String>,
    pub date_to: Option<String>,
    pub view_id: Option<i32>,
    pub sort: Option<String>,
    pub sort_reverse: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedView {
    pub id: i32,
    pub name: String,
    pub show_on_dashboard: bool,
    pub show_in_sidebar: bool,
    pub sort_field: String,
    pub sort_reverse: bool,
    pub filter: DocumentFilter,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowFilters {
    pub content_contains: Option<String>,
    pub has_correspondent: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowAction {
    #[serde(rename = "type")]
    pub type_: String,
    pub tag_id: Option<i32>,
    pub correspondent_id: Option<i32>,
    pub document_type_id: Option<i32>,
    pub storage_path_id: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Workflow {
    pub id: i32,
    pub name: String,
    pub enabled: bool,
    pub trigger_type: String,
    pub filters: WorkflowFilters,
    pub actions: Vec<WorkflowAction>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailAccount {
    pub id: i32,
    pub name: String,
    pub imap_server: Option<String>,
    pub username: Option<String>,
    pub imap_port: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailRule {
    pub id: i32,
    pub name: String,
    pub account_id: Option<i32>,
    pub folder: Option<String>,
    pub filter_from: Option<String>,
    pub filter_subject: Option<String>,
    pub enabled: bool,
    pub correspondent_id: Option<i32>,
    pub document_type_id: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsumeJob {
    pub id: i32,
    pub filename: String,
    pub status: String,
    pub message: Option<String>,
    pub document_id: Option<i32>,
    pub created_at: String,
    pub finished_at: Option<String>,
}

/// /api/jobs payload: camelCase for types.ts plus snake_case aliases for the current logs page.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobOut {
    pub id: i32,
    pub filename: String,
    pub status: String,
    pub message: Option<String>,
    pub document_id: Option<i32>,
    #[serde(rename = "documentId")]
    pub document_id_camel: Option<i32>,
    pub created_at: String,
    #[serde(rename = "createdAt")]
    pub created_at_camel: String,
    pub finished_at: Option<String>,
    #[serde(rename = "finishedAt")]
    pub finished_at_camel: Option<String>,
}

impl JobOut {
    pub fn new(
        id: i32,
        filename: String,
        status: String,
        message: Option<String>,
        document_id: Option<i32>,
        created_at: String,
        finished_at: Option<String>,
    ) -> Self {
        Self {
            id,
            filename,
            status,
            message,
            document_id,
            document_id_camel: document_id,
            created_at: created_at.clone(),
            created_at_camel: created_at,
            finished_at: finished_at.clone(),
            finished_at_camel: finished_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MonthCount {
    pub month: String,
    pub count: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TypeCount {
    pub name: String,
    pub count: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardStats {
    pub documents: i32,
    pub inbox: i32,
    pub correspondents: i32,
    pub tags: i32,
    pub pages: i32,
    pub storage_bytes: i32,
    pub ocr_done: i32,
    pub ocr_pending: i32,
    pub recent: Vec<DocumentListItem>,
    pub by_month: Vec<MonthCount>,
    pub by_type: Vec<TypeCount>,
    pub jobs: Vec<ConsumeJob>,
    pub views: Vec<SavedView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    pub tags: Vec<Tag>,
    pub correspondents: Vec<Correspondent>,
    pub document_types: Vec<DocumentType>,
    pub storage_paths: Vec<StoragePath>,
    pub custom_fields: Vec<CustomField>,
    pub saved_views: Vec<SavedView>,
    pub workflows: Vec<Workflow>,
    pub mail_accounts: Vec<MailAccount>,
    pub mail_rules: Vec<MailRule>,
    pub ai_available: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsumeInput {
    pub filename: String,
    pub mime_type: String,
    #[serde(default)]
    pub data_base64: String,
    /// Multipart uploads set this and skip the base64 round-trip.
    #[serde(skip)]
    pub raw_bytes: Option<Vec<u8>>,
    pub provider: String,
    pub api_key: Option<String>,
    pub skip_ocr: Option<bool>,
    /// Ignored (). Selfhosted OCR base URL is settings-only (`ocrBaseUrl`).
    #[serde(default)]
    pub base_url: Option<String>,
    /// Optional title override (ngx `post_document` / clients).
    #[serde(default)]
    pub title_override: Option<String>,
    /// Optional created date override `YYYY-MM-DD` (ngx `created`).
    #[serde(default)]
    pub created_override: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsumeResult {
    pub document_id: i32,
    pub ocr: crate::ocr::OcrResult,
    pub used_llm: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetaUpdate {
    #[serde(default)]
    pub id: i32,
    pub title: Option<String>,
    pub content: Option<String>,
    #[serde(default, deserialize_with = "deserialize_present")]
    pub correspondent_id: Option<Option<i32>>,
    #[serde(default, deserialize_with = "deserialize_present")]
    pub document_type_id: Option<Option<i32>>,
    #[serde(default, deserialize_with = "deserialize_present")]
    pub storage_path_id: Option<Option<i32>>,
    #[serde(default, deserialize_with = "deserialize_present")]
    pub created_date: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_present")]
    pub asn: Option<Option<i32>>,
    pub tag_ids: Option<Vec<i32>>,
    pub custom_values: Option<Vec<CustomValuePatch>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomValuePatch {
    pub field_id: i32,
    pub value: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BulkUpdate {
    pub ids: Vec<i32>,
    pub add_tag_ids: Option<Vec<i32>>,
    pub remove_tag_ids: Option<Vec<i32>>,
    pub correspondent_id: Option<i32>,
    pub document_type_id: Option<i32>,
    /// Soft-delete → Corbeille (sets deleted_at).
    pub delete_selected: Option<bool>,
    /// Restore from Corbeille (clears deleted_at).
    pub restore_selected: Option<bool>,
    /// Permanent delete from Corbeille (row + bytes).
    pub purge_selected: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteInput {
    pub document_id: i32,
    pub body: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrInput {
    #[serde(default)]
    pub id: i32,
    #[serde(default = "default_provider")]
    pub provider: String,
    pub api_key: Option<String>,
    pub data_base64: Option<String>,
    pub mime_type: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagInput {
    pub id: Option<i32>,
    pub name: String,
    #[serde(default = "default_tag_color")]
    pub color: String,
    #[serde(default)]
    pub is_inbox_tag: bool,
    #[serde(rename = "match", default)]
    pub match_text: String,
    #[serde(default = "default_algo_none")]
    pub matching_algorithm: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CorrespondentInput {
    pub id: Option<i32>,
    pub name: String,
    #[serde(rename = "match", default)]
    pub match_text: String,
    #[serde(default = "default_algo_auto")]
    pub matching_algorithm: String,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TypeInput {
    pub id: Option<i32>,
    pub name: String,
    #[serde(rename = "match", default)]
    pub match_text: String,
    #[serde(default = "default_algo_auto")]
    pub matching_algorithm: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PathInput {
    pub id: Option<i32>,
    pub name: String,
    pub path: String,
    #[serde(rename = "match", default)]
    pub match_text: String,
    #[serde(default = "default_algo_none")]
    pub matching_algorithm: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldInput {
    pub id: Option<i32>,
    pub name: String,
    #[serde(default = "default_data_type")]
    pub data_type: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewInput {
    pub id: Option<i32>,
    pub name: String,
    #[serde(default)]
    pub show_on_dashboard: bool,
    #[serde(default = "default_true")]
    pub show_in_sidebar: bool,
    #[serde(default = "default_sort_field")]
    pub sort_field: String,
    #[serde(default = "default_true")]
    pub sort_reverse: bool,
    #[serde(default)]
    pub filter: DocumentFilter,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowInput {
    pub id: Option<i32>,
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_trigger")]
    pub trigger_type: String,
    #[serde(default = "default_json_obj")]
    pub filters: Value,
    #[serde(default = "default_json_arr")]
    pub actions: Value,
}

fn default_trigger() -> String {
    "document_added".into()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsInput {
    pub ocr_provider: String,
    pub ocr_language: String,
    pub auto_match: bool,
    /// OpenAI-compatible base URL when provider is `selfhosted`.
    #[serde(default)]
    pub ocr_base_url: Option<String>,
    /// Corbeille auto-purge after N days (30 | 60 | 90).
    #[serde(default)]
    pub trash_retention_days: Option<i32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsOut {
    pub ocr_provider: String,
    pub ocr_language: String,
    pub auto_match: bool,
    pub ai_available: bool,
    pub llm_keys: Vec<crate::llm_keys::LlmKeyStatus>,
    /// Self-hosted OpenAI-compatible base URL (may be empty).
    #[serde(default)]
    pub ocr_base_url: String,
    /// Corbeille auto-purge after N days (30 | 60 | 90).
    pub trash_retention_days: i32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmKeyPatch {
    pub provider: String,
    pub api_key: Option<String>,
    #[serde(default)]
    pub clear: bool,
}

#[derive(Debug, sqlx::FromRow)]
pub struct DocRow {
    pub id: i32,
    pub title: String,
    pub content: String,
    pub correspondent_id: Option<i32>,
    pub correspondent_name: Option<String>,
    pub document_type_id: Option<i32>,
    pub document_type_name: Option<String>,
    pub storage_path_id: Option<i32>,
    pub storage_path_name: Option<String>,
    pub created_date: Option<String>,
    pub added_at: String,
    pub modified_at: String,
    pub asn: Option<i32>,
    pub mime_type: String,
    pub original_filename: Option<String>,
    pub page_count: i32,
    pub language: String,
    pub ocr_status: String,
    pub ocr_provider: Option<String>,
    pub thumbnail_svg: Option<String>,
    pub file_size: i32,
    pub summary: Option<String>,
}

static RE_SVG_DEFS: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?s)<defs>.*?</defs>").expect("svg defs regex")
});
static RE_SVG_GRAIN: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"<rect[^>]*filter="url\(#grain\)"[^>]*/?>"#).expect("svg grain regex")
});
static RE_SVG_ON_ATTR: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"(?i)(?:^|[\s>'"/])on[a-z0-9_-]+\s*="#).expect("svg on* attr regex")
});

/// Thumbnail-only SVG gate. **Not** a general XSS sanitizer — refuses active
/// content (`<script`, `javascript:`, `on*=`, `foreignObject`, `iframe`, `<use`, `xlink:href`)
/// and optionally strips presentation-only `defs` / grain rects.
pub fn sanitize_svg(svg: Option<String>) -> Option<String> {
    let svg = svg?;
    let lower = svg.to_ascii_lowercase();
    if lower.contains("<script")
        || lower.contains("javascript:")
        || RE_SVG_ON_ATTR.is_match(&svg)
        || lower.contains("foreignobject")
        || lower.contains("<iframe")
        || lower.contains("<use")
        || lower.contains("xlink:href")
    {
        return None;
    }
    let out = RE_SVG_DEFS.replace_all(&svg, "");
    let out = RE_SVG_GRAIN.replace_all(&out, "");
    Some(out.into_owned())
}

pub fn map_doc(row: DocRow, tags: Vec<Tag>) -> DocumentListItem {
    DocumentListItem {
        id: row.id,
        title: row.title,
        content: row.content,
        correspondent_id: row.correspondent_id,
        correspondent_name: row.correspondent_name,
        document_type_id: row.document_type_id,
        document_type_name: row.document_type_name,
        storage_path_id: row.storage_path_id,
        storage_path_name: row.storage_path_name,
        created_date: row.created_date,
        added_at: row.added_at,
        modified_at: row.modified_at,
        asn: row.asn,
        mime_type: row.mime_type,
        original_filename: row.original_filename,
        page_count: row.page_count,
        language: row.language,
        ocr_status: row.ocr_status,
        ocr_provider: row.ocr_provider,
        thumbnail_svg: sanitize_svg(row.thumbnail_svg),
        file_size: row.file_size,
        summary: row.summary,
        tags,
    }
}

/// List projection: keep searching full OCR in WHERE, but only ship a short snippet.
pub const DOC_SELECT: &str = r#"
  select
    d.id, d.title, left(d.content, 480) as content, d.correspondent_id, c.name as correspondent_name,
    d.document_type_id, dt.name as document_type_name,
    d.storage_path_id, sp.name as storage_path_name,
    d.created_date::text as created_date, d.added_at::text as added_at,
    d.modified_at::text as modified_at, d.asn, d.mime_type, d.original_filename,
    d.page_count, d.language, d.ocr_status, d.ocr_provider, d.thumbnail_svg,
    d.file_size, d.summary
  from documents d
  left join correspondents c on c.id = d.correspondent_id and c.owner_id = d.owner_id
  left join document_types dt on dt.id = d.document_type_id and dt.owner_id = d.owner_id
  left join storage_paths sp on sp.id = d.storage_path_id and sp.owner_id = d.owner_id
"#;

/// Org-env OCR readiness for catalogue only.
/// do not claim AI is ready from ambient env keys unless
/// `PAPERRUST_LLM_ENV_FALLBACK=1` (same gate as bootstrap seed). Per-user
/// readiness lives on settings (`hasKey` / `aiAvailable`).
pub fn ai_available() -> bool {
    if std::env::var("PAPERRUST_LLM_ENV_FALLBACK").ok().as_deref() != Some("1") {
        return false;
    }
    crate::ocr::env_key_configured("xai")
        || crate::ocr::env_key_configured("openai")
        || crate::ocr::env_key_configured("anthropic")
}


#[cfg(test)]
mod syn_svg_tests {
    use super::sanitize_svg;

    #[test]
    fn rejects_tight_onload_without_leading_space() {
        // Must match `onload=` even when glued after a quote / tag (no required whitespace).
        assert!(sanitize_svg(Some(r#"<svg onload="alert(1)"></svg>"#.into())).is_none());
        assert!(sanitize_svg(Some(r#"<svg/onload=alert(1)>"#.into())).is_none());
        assert!(sanitize_svg(Some(r#"<svg width="1"onload="alert(1)">"#.into())).is_none());
    }

    #[test]
    fn allows_inert_svg() {
        let s = sanitize_svg(Some(
            r#"<svg xmlns="http://www.w3.org/2000/svg"><rect width="1" height="1"/></svg>"#.into(),
        ));
        assert!(s.is_some(), "{s:?}");
    }
}
