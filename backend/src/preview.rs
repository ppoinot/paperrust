use std::path::{Path, PathBuf};

use base64::Engine;
use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, ExtendedColorType, GenericImageView};
use sqlx::{PgPool, Row};

pub const PREVIEW_MAX_WIDTH: u32 = 900;
pub const JPEG_QUALITY: u8 = 70;
pub const MAX_PAGES: i32 = 50;
/// Tall phone/scan PNGs (e.g. 6k×12k) need headroom, but unbounded 90s stacks ().
pub const RENDER_TIMEOUT_SECS: u64 = 60;

pub fn pdfium_library_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("PAPERRUST_PDFIUM") {
        let pb = PathBuf::from(&p);
        if pb.is_file() {
            return Some(pb);
        }
        let nested = pb.join("libpdfium.so");
        if nested.is_file() {
            return Some(nested);
        }
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("libpdfium.so"));
            candidates.push(dir.join("lib/libpdfium.so"));
            if let Some(root) = dir.parent().and_then(|p| p.parent()).and_then(|p| p.parent()) {
                candidates.push(root.join("lib/libpdfium.so"));
            }
        }
    }
    candidates.push(PathBuf::from("/var/home/grok-agent/work/paperrust/lib/libpdfium.so"));
    candidates.push(PathBuf::from("lib/libpdfium.so"));
    candidates.into_iter().find(|p| p.is_file())
}

fn is_pdf(mime: &str, bytes: &[u8]) -> bool {
    mime == "application/pdf"
        || mime.ends_with("/pdf")
        || bytes.starts_with(b"%PDF")
}

pub fn looks_like_pdf(bytes: &[u8]) -> bool {
    bytes.starts_with(b"%PDF")
}

pub fn looks_like_png(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\x89PNG\r\n\x1a\n")
}

pub fn looks_like_jpeg(bytes: &[u8]) -> bool {
    bytes.len() >= 3 && bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF
}

pub fn looks_like_svg(bytes: &[u8]) -> bool {
    let n = bytes.len().min(512);
    let s = String::from_utf8_lossy(&bytes[..n]).to_ascii_lowercase();
    s.contains("<svg") || s.contains("image/svg")
}

fn is_raster_image(mime: &str) -> bool {
    mime.starts_with("image/") && !mime.contains("svg")
}

pub fn decode_image_data_url(file_data: &str) -> Option<(Vec<u8>, String)> {
    let s = file_data.trim();
    let rest = s.strip_prefix("data:")?;
    let (meta, b64) = rest.split_once(',')?;
    let meta_lc = meta.to_ascii_lowercase();
    if !meta_lc.contains("base64") {
        return None;
    }
    let mime = meta.split(';').next()?.trim();
    if !is_raster_image(mime) {
        return None;
    }
    let cleaned: String = b64.chars().filter(|c| !c.is_whitespace()).collect();
    let raw = Engine::decode(&base64::engine::general_purpose::STANDARD, &cleaned)
        .or_else(|_| Engine::decode(&base64::engine::general_purpose::STANDARD_NO_PAD, &cleaned))
        .ok()?;
    if looks_like_svg(&raw) || looks_like_pdf(&raw) {
        return None;
    }
    if !looks_like_png(&raw) && !looks_like_jpeg(&raw) {
        return None;
    }
    Some((raw, mime.to_string()))
}

fn encode_jpeg(img: DynamicImage) -> anyhow::Result<Vec<u8>> {
    let rgb = img.to_rgb8();
    let (w, h) = rgb.dimensions();
    let mut out = Vec::new();
    let mut enc = JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY);
    enc.encode(rgb.as_raw(), w, h, ExtendedColorType::Rgb8)?;
    Ok(out)
}

fn downscale(img: DynamicImage) -> DynamicImage {
    let (w, h) = img.dimensions();
    if w <= PREVIEW_MAX_WIDTH {
        return img;
    }
    let nh = ((h as u64 * PREVIEW_MAX_WIDTH as u64) / w.max(1) as u64).max(1) as u32;
    img.resize(PREVIEW_MAX_WIDTH, nh, image::imageops::FilterType::Triangle)
}

fn render_image_bytes(bytes: &[u8]) -> anyhow::Result<Vec<u8>> {
    // bound decode dimensions / allocations (zip-bomb / decompression DoS).
    // Allow tall A4 phone scans (~6k×12k ≈ 70 MP) then downscale to PREVIEW_MAX_WIDTH.
    const MAX_DIM: u32 = 20_000;
    const MAX_PIXELS: u64 = 120_000_000;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DIM);
    limits.max_image_height = Some(MAX_DIM);
    limits.max_alloc = Some(MAX_PIXELS.saturating_mul(4));
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes));
    reader.limits(limits);
    let reader = reader.with_guessed_format()?;
    let img = reader.decode()?;
    let (w, h) = img.dimensions();
    if (w as u64).saturating_mul(h as u64) > MAX_PIXELS {
        anyhow::bail!("image exceeds pixel limit");
    }
    encode_jpeg(downscale(img))
}

fn pdfium() -> anyhow::Result<pdfium_render::prelude::Pdfium> {
    use pdfium_render::prelude::*;
    let path = pdfium_library_path()
        .ok_or_else(|| anyhow::anyhow!("libpdfium.so not found (set PAPERRUST_PDFIUM)"))?;
    let bindings = Pdfium::bind_to_library(&path)
        .map_err(|e| anyhow::anyhow!("bind pdfium at {}: {e}", path.display()))?;
    Ok(Pdfium::new(bindings))
}

fn render_pdf_page_in_doc(
    doc: &pdfium_render::prelude::PdfDocument<'_>,
    page_1based: i32,
) -> anyhow::Result<Vec<u8>> {
    use pdfium_render::prelude::*;
    let n = doc.pages().len();
    if page_1based < 1 || (page_1based as u16) > n {
        anyhow::bail!("pdf page out of range");
    }
    let page = doc
        .pages()
        .get((page_1based as u16) - 1)
        .map_err(|e| anyhow::anyhow!("pdf page: {e}"))?;
    // Target width already set — skip a second downscale pass.
    let cfg = PdfRenderConfig::new().set_target_width(PREVIEW_MAX_WIDTH as i32);
    let img = page
        .render_with_config(&cfg)
        .map_err(|e| anyhow::anyhow!("pdf render: {e}"))?
        .as_image();
    encode_jpeg(img)
}

fn with_open_pdf<T>(
    bytes: &[u8],
    f: impl FnOnce(&pdfium_render::prelude::PdfDocument<'_>) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let pdfium = pdfium()?;
    let doc = pdfium
        .load_pdf_from_byte_slice(bytes, None)
        .map_err(|e| anyhow::anyhow!("pdf open: {e}"))?;
    f(&doc)
}

fn render_pdf_page(bytes: &[u8], page_1based: i32) -> anyhow::Result<Vec<u8>> {
    with_open_pdf(bytes, |doc| render_pdf_page_in_doc(doc, page_1based))
}

fn pdf_page_count(bytes: &[u8]) -> anyhow::Result<i32> {
    with_open_pdf(bytes, |doc| Ok(i32::from(doc.pages().len())))
}

/// Uncapped page count (OCR limit checks).
pub fn count_pages_total(bytes: &[u8], mime: &str) -> anyhow::Result<i32> {
    if is_pdf(mime, bytes) {
        return pdf_page_count(bytes);
    }
    if is_raster_image(mime) {
        return Ok(1);
    }
    anyhow::bail!("no raster preview for {mime}")
}

pub fn count_pages(bytes: &[u8], mime: &str) -> anyhow::Result<i32> {
    Ok(count_pages_total(bytes, mime)?.min(MAX_PAGES))
}

pub fn render_page(bytes: &[u8], mime: &str, page_1based: i32) -> anyhow::Result<Vec<u8>> {
    if page_1based < 1 || page_1based > MAX_PAGES {
        anyhow::bail!("page out of range");
    }
    if is_pdf(mime, bytes) {
        return render_pdf_page(bytes, page_1based);
    }
    if is_raster_image(mime) {
        if page_1based != 1 {
            anyhow::bail!("image has a single page");
        }
        return render_image_bytes(bytes);
    }
    anyhow::bail!("no raster preview for {mime}")
}

pub fn render_all(bytes: &[u8], mime: &str) -> anyhow::Result<Vec<Vec<u8>>> {
    if is_pdf(mime, bytes) {
        return with_open_pdf(bytes, |doc| {
            let n = (i32::from(doc.pages().len())).min(MAX_PAGES).max(1);
            let mut out = Vec::with_capacity(n as usize);
            for i in 1..=n {
                out.push(render_pdf_page_in_doc(doc, i)?);
            }
            Ok(out)
        });
    }
    if is_raster_image(mime) {
        return Ok(vec![render_image_bytes(bytes)?]);
    }
    anyhow::bail!("no raster preview for {mime}")
}

pub async fn ensure_schema(pool: &PgPool) -> anyhow::Result<()> {
    if let Err(e) = sqlx::query("alter table documents add column if not exists original_bytes bytea")
        .execute(pool)
        .await
    {
        tracing::warn!("add original_bytes (if not exists): {e}");
        if let Err(e2) = sqlx::query("alter table documents add column original_bytes bytea")
            .execute(pool)
            .await
        {
            tracing::warn!("add original_bytes: {e2}");
        }
    }
    sqlx::query(
        r#"create table if not exists document_preview_pages (
            document_id int not null references documents(id) on delete cascade,
            page int not null,
            jpeg bytea not null,
            primary key (document_id, page)
        )"#,
    )
    .execute(pool)
    .await?;

    let rows = sqlx::query(
        "select id, file_data from documents where (original_bytes is null or octet_length(original_bytes) = 0) and file_data like 'data:image%' and file_data not like '%svg%'",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    for row in rows {
        let id: i32 = row.get("id");
        let fd: Option<String> = row.get("file_data");
        if let Some(fd) = fd {
            if let Some((raw, _)) = decode_image_data_url(&fd) {
                let b64 = Engine::encode(&base64::engine::general_purpose::STANDARD, &raw);
                let _ = sqlx::query("update documents set original_bytes = decode($1, 'base64') where id = $2")
                    .bind(&b64)
                    .bind(id)
                    .execute(pool)
                    .await;
            }
        }
    }
    tracing::info!(
        "preview schema ready; pdfium={}",
        pdfium_library_path()
            .as_deref()
            .unwrap_or(Path::new("missing"))
            .display()
    );
    Ok(())
}

pub async fn cached_jpeg(pool: &PgPool, id: i32, page: i32) -> sqlx::Result<Option<Vec<u8>>> {
    sqlx::query_scalar("select jpeg from document_preview_pages where document_id = $1 and page = $2")
        .bind(id)
        .bind(page)
        .fetch_optional(pool)
        .await
}

pub async fn load_cached_pages(pool: &PgPool, id: i32) -> sqlx::Result<Vec<Vec<u8>>> {
    let rows = sqlx::query(
        "select jpeg from document_preview_pages where document_id = $1 order by page",
    )
    .bind(id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|r| r.get::<Vec<u8>, _>("jpeg")).collect())
}

pub async fn store_jpeg(pool: &PgPool, id: i32, page: i32, jpeg: &[u8]) -> sqlx::Result<()> {
    sqlx::query(
        r#"insert into document_preview_pages (document_id, page, jpeg) values ($1,$2,$3)
           on conflict (document_id, page) do update set jpeg = excluded.jpeg"#,
    )
    .bind(id)
    .bind(page)
    .bind(jpeg)
    .execute(pool)
    .await?;
    Ok(())
}

/// Drop cached preview JPEGs so the next OCR / preview pass re-renders from new bytes.
pub async fn clear_pages(pool: &PgPool, id: i32) -> sqlx::Result<()> {
    sqlx::query("delete from document_preview_pages where document_id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn store_pages(pool: &PgPool, id: i32, owner_id: i32, pages: &[Vec<u8>]) -> sqlx::Result<()> {
    if pages.is_empty() {
        return Ok(());
    }
    let page_nos: Vec<i32> = (1..=pages.len() as i32).collect();
    // One round-trip: unnest pages + jpeg payloads.
    sqlx::query(
        r#"insert into document_preview_pages (document_id, page, jpeg)
           select $1, page, jpeg
           from unnest($2::int[], $3::bytea[]) as t(page, jpeg)
           on conflict (document_id, page) do update set jpeg = excluded.jpeg"#,
    )
    .bind(id)
    .bind(&page_nos)
    .bind(pages)
    .execute(pool)
    .await?;
    sqlx::query("update documents set page_count = $1 where id = $2 and owner_id = $3")
        .bind(pages.len() as i32)
        .bind(id)
        .bind(owner_id)
        .execute(pool)
        .await?;
    Ok(())
}

#[derive(Debug)]
pub struct OriginalDoc {
    pub id: i32,
    pub mime_type: String,
    pub original_filename: Option<String>,
    pub page_count: i32,
    pub bytes: Option<Vec<u8>>,
}


pub async fn load_original_meta(
    pool: &PgPool,
    id: i32,
    owner_id: i32,
) -> sqlx::Result<Option<(i32, String, Option<String>, i32, bool, i64)>> {
    let row = sqlx::query(
        r#"select id, mime_type, original_filename, page_count,
                  (original_bytes is not null and octet_length(original_bytes) > 0) as has_bytes,
                  coalesce(octet_length(original_bytes), 0)::bigint as byte_length
           from documents where id = $1 and owner_id = $2"#,
    )
    .bind(id)
    .bind(owner_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| {
        (
            r.get("id"),
            r.get("mime_type"),
            r.get("original_filename"),
            r.get("page_count"),
            r.get::<bool, _>("has_bytes"),
            r.get("byte_length"),
        )
    }))
}

pub async fn load_original(pool: &PgPool, id: i32, owner_id: i32) -> sqlx::Result<Option<OriginalDoc>> {
    // Native bytea — consume already binds/reads Vec<u8> successfully on pgrust.
    let row = sqlx::query(
        "select id, mime_type, original_filename, page_count, original_bytes from documents where id = $1 and owner_id = $2",
    )
    .bind(id)
    .bind(owner_id)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let mut bytes: Option<Vec<u8>> = row.get("original_bytes");
    if bytes.as_ref().map(|b| b.is_empty()).unwrap_or(true) {
        bytes = None;
    }
    let mime_type: String = row.get("mime_type");
    // Never serve or persist the paper SVG as original_bytes.
    if bytes.as_ref().is_some_and(|b| looks_like_svg(b)) {
        bytes = None;
    }
    // If original_bytes is missing, backfill from a raster data:image/png|jpeg URL only.
    if bytes.is_none() {
        let fd: Option<String> = sqlx::query_scalar(
            "select file_data from documents where id = $1 and owner_id = $2",
        )
        .bind(id)
        .bind(owner_id)
        .fetch_optional(pool)
        .await?
        .flatten();
        if let Some(fd) = fd {
            let looks_svg = fd.contains("image/svg") || fd.contains("<svg");
            if !looks_svg {
                if let Some((raw, decoded_mime)) = decode_image_data_url(&fd) {
                    if !looks_like_svg(&raw)
                        && !decoded_mime.contains("svg")
                        && (looks_like_png(&raw) || looks_like_jpeg(&raw))
                    {
                        let _ = sqlx::query(
                            "update documents set original_bytes = $1 where id = $2 and owner_id = $3",
                        )
                        .bind(&raw)
                        .bind(id)
                        .bind(owner_id)
                        .execute(pool)
                        .await;
                        bytes = Some(raw);
                    }
                }
            }
        }
    }
    Ok(Some(OriginalDoc {
        id: row.get("id"),
        mime_type,
        original_filename: row.get("original_filename"),
        page_count: row.get("page_count"),
        bytes,
    }))
}

pub async fn cached_max_page(pool: &PgPool, id: i32) -> sqlx::Result<i32> {
    let n: Option<i32> = sqlx::query_scalar(
        "select coalesce(max(page),0)::int from document_preview_pages where document_id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(n.unwrap_or(0))
}
