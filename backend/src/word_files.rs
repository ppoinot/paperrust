//! Word-processor bytes Paperrust stores as the original file.
//!
//! Magic decides. The extension is used only where OOXML packages share the
//! `PK` header with spreadsheets and presentations, and where an OLE compound
//! file is Word 97–2003 only when the name is `.doc` or `.dot`.

use crate::preview::{looks_like_jpeg, looks_like_png};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WordKind {
    Docx,
    Docm,
    Dotx,
    Dotm,
    Odt,
    Ott,
    Fodt,
    Rtf,
    Doc,
    Dot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IncomingSave {
    /// PDF magic. Stored beside the Word original. The original is not replaced.
    Rendition,
    /// A format WordCraft rewrites in place (or a `.doc` that came back as `.docx`).
    Replace(WordKind),
    /// Page image export. Not archived.
    Ignore,
    /// HTML, Markdown, text, TeX, JSON, or anything else.
    Reject,
}

pub fn mime_for(kind: WordKind) -> &'static str {
    match kind {
        WordKind::Docx => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        WordKind::Dotx => "application/vnd.openxmlformats-officedocument.wordprocessingml.template",
        WordKind::Docm => "application/vnd.ms-word.document.macroEnabled.12",
        WordKind::Dotm => "application/vnd.ms-word.template.macroEnabled.12",
        WordKind::Odt | WordKind::Fodt => "application/vnd.oasis.opendocument.text",
        WordKind::Ott => "application/vnd.oasis.opendocument.text-template",
        WordKind::Rtf => "application/rtf",
        WordKind::Doc | WordKind::Dot => "application/msword",
    }
}

pub fn extension_for(kind: WordKind) -> &'static str {
    match kind {
        WordKind::Docx => ".docx",
        WordKind::Docm => ".docm",
        WordKind::Dotx => ".dotx",
        WordKind::Dotm => ".dotm",
        WordKind::Odt => ".odt",
        WordKind::Ott => ".ott",
        WordKind::Fodt => ".fodt",
        WordKind::Rtf => ".rtf",
        WordKind::Doc => ".doc",
        WordKind::Dot => ".dot",
    }
}

pub fn is_word_mime(mime: &str) -> bool {
    let m = mime
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    matches!(
        m.as_str(),
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
            | "application/vnd.openxmlformats-officedocument.wordprocessingml.template"
            | "application/vnd.ms-word.document.macroenabled.12"
            | "application/vnd.ms-word.template.macroenabled.12"
            | "application/msword"
            | "application/rtf"
            | "text/rtf"
            | "application/vnd.oasis.opendocument.text"
            | "application/vnd.oasis.opendocument.text-template"
    )
}

/// Save of DOCX/DOCM/DOTX/DOTM/ODT/OTT/FODT/RTF replaces the original.
/// OLE `.doc` / `.dot` is not rewritten in place: WordCraft sends a `.docx`.
pub fn is_rewritable(kind: WordKind) -> bool {
    !matches!(kind, WordKind::Doc | WordKind::Dot)
}

pub fn classify(bytes: &[u8], filename: &str) -> Option<WordKind> {
    if bytes.starts_with(b"%PDF") || looks_like_png(bytes) || looks_like_jpeg(bytes) {
        return None;
    }
    if bytes.starts_with(b"PK\x03\x04") {
        return classify_zip(bytes, filename);
    }
    if looks_like_rtf(bytes) {
        return Some(WordKind::Rtf);
    }
    if looks_like_ole(bytes) {
        return classify_ole(bytes, filename);
    }
    if looks_like_fodt(bytes) {
        return Some(WordKind::Fodt);
    }
    None
}

pub fn classify_save(bytes: &[u8], filename: &str) -> IncomingSave {
    if bytes.is_empty() {
        return IncomingSave::Reject;
    }
    if looks_like_png(bytes) || looks_like_jpeg(bytes) {
        return IncomingSave::Ignore;
    }
    if bytes.starts_with(b"%PDF") {
        return IncomingSave::Rendition;
    }
    if let Some(kind) = classify(bytes, filename) {
        if is_rewritable(kind) {
            return IncomingSave::Replace(kind);
        }
    }
    IncomingSave::Reject
}

/// Keep a matching extension. A `.doc` that came back as DOCX bytes is renamed `.docx`.
pub fn saved_filename(raw_name: &str, kind: WordKind) -> String {
    let clamped = crate::limits::clamp_filename(raw_name);
    let ext = extension_for(kind);
    let lower = clamped.to_ascii_lowercase();
    if lower.ends_with(ext) && clamped.len() > ext.len() {
        return clamped;
    }
    let stem = clamped
        .rsplit_once('.')
        .map(|(stem, _)| stem)
        .unwrap_or(clamped.as_str())
        .trim()
        .trim_end_matches('.');
    if stem.is_empty() {
        format!("document{ext}")
    } else {
        format!("{stem}{ext}")
    }
}

fn file_ext(filename: &str) -> Option<String> {
    let name = filename.rsplit(['/', '\\']).next().unwrap_or(filename).trim();
    let (_, ext) = name.rsplit_once('.')?;
    if ext.is_empty() || ext.len() > 8 || ext.contains(' ') {
        return None;
    }
    Some(ext.to_ascii_lowercase())
}

fn looks_like_rtf(bytes: &[u8]) -> bool {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    bytes.starts_with(b"{\\rtf")
}

fn looks_like_ole(bytes: &[u8]) -> bool {
    bytes.len() >= 8
        && bytes[..8] == [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]
}

fn has_word_document_stream(bytes: &[u8]) -> bool {
    const NEEDLE: &[u8] = b"W\x00o\x00r\x00d\x00D\x00o\x00c\x00u\x00m\x00e\x00n\x00t\x00";
    bytes.windows(NEEDLE.len()).any(|window| window == NEEDLE)
}

fn classify_ole(bytes: &[u8], filename: &str) -> Option<WordKind> {
    if !has_word_document_stream(bytes) {
        return None;
    }
    match file_ext(filename)?.as_str() {
        "doc" => Some(WordKind::Doc),
        "dot" => Some(WordKind::Dot),
        _ => None,
    }
}

fn looks_like_fodt(bytes: &[u8]) -> bool {
    let n = bytes.len().min(8192);
    let head = String::from_utf8_lossy(&bytes[..n]).to_ascii_lowercase();
    head.contains("application/vnd.oasis.opendocument.text") && head.contains("<office:document")
}

struct ZipEntry {
    name: String,
    data_start: usize,
    data_len: usize,
    method: u16,
}

struct ZipScan {
    entries: Vec<ZipEntry>,
    saw_word: bool,
    saw_sheet_or_deck: bool,
}

/// Walk local headers. Stop when a data-descriptor flag leaves the next header unreachable.
fn scan_zip(bytes: &[u8]) -> Option<ZipScan> {
    if bytes.len() < 30 || &bytes[..4] != b"PK\x03\x04" {
        return None;
    }
    let mut entries = Vec::new();
    let mut saw_word = false;
    let mut saw_sheet_or_deck = false;
    let mut i = 0usize;
    for _ in 0..64 {
        if i + 30 > bytes.len() || &bytes[i..i + 4] != b"PK\x03\x04" {
            break;
        }
        let flags = u16::from_le_bytes([bytes[i + 6], bytes[i + 7]]);
        let method = u16::from_le_bytes([bytes[i + 8], bytes[i + 9]]);
        let compressed = u32::from_le_bytes([
            bytes[i + 18],
            bytes[i + 19],
            bytes[i + 20],
            bytes[i + 21],
        ]) as usize;
        let name_len = u16::from_le_bytes([bytes[i + 26], bytes[i + 27]]) as usize;
        let extra_len = u16::from_le_bytes([bytes[i + 28], bytes[i + 29]]) as usize;
        if name_len > 512 {
            break;
        }
        let name_start = i + 30;
        let name_end = name_start.saturating_add(name_len);
        if name_end > bytes.len() {
            break;
        }
        let name = String::from_utf8_lossy(&bytes[name_start..name_end]).into_owned();
        let data_start = name_end.saturating_add(extra_len);
        if data_start > bytes.len() {
            break;
        }
        if name.starts_with("word/") {
            saw_word = true;
        }
        if name.starts_with("xl/") || name.starts_with("ppt/") {
            saw_sheet_or_deck = true;
        }
        let descriptor = flags & 0x0008 != 0;
        if descriptor && compressed == 0 {
            entries.push(ZipEntry {
                name,
                data_start,
                data_len: 0,
                method,
            });
            break;
        }
        let data_end = data_start.saturating_add(compressed);
        if data_end > bytes.len() {
            break;
        }
        entries.push(ZipEntry {
            name,
            data_start,
            data_len: compressed,
            method,
        });
        i = data_end;
    }
    if entries.is_empty() {
        return None;
    }
    Some(ZipScan {
        entries,
        saw_word,
        saw_sheet_or_deck,
    })
}

fn classify_zip(bytes: &[u8], filename: &str) -> Option<WordKind> {
    let scan = scan_zip(bytes)?;
    let first = &scan.entries[0];
    if first.name == "mimetype" && first.method == 0 && first.data_len > 0 {
        let end = first.data_start.saturating_add(first.data_len).min(bytes.len());
        let mime = std::str::from_utf8(&bytes[first.data_start..end])
            .unwrap_or("")
            .trim();
        if mime.starts_with("application/vnd.oasis.opendocument.text-template") {
            return Some(WordKind::Ott);
        }
        if mime.starts_with("application/vnd.oasis.opendocument.text") {
            return Some(WordKind::Odt);
        }
        return None;
    }
    if scan.saw_sheet_or_deck || !scan.saw_word {
        return None;
    }
    Some(match file_ext(filename).as_deref() {
        Some("docm") => WordKind::Docm,
        Some("dotm") => WordKind::Dotm,
        Some("dotx") => WordKind::Dotx,
        _ => WordKind::Docx,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zip_stored(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        for (name, data) in entries {
            let name_b = name.as_bytes();
            out.extend_from_slice(b"PK\x03\x04");
            out.extend_from_slice(&20u16.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&0u32.to_le_bytes());
            let n = u32::try_from(data.len()).unwrap();
            out.extend_from_slice(&n.to_le_bytes());
            out.extend_from_slice(&n.to_le_bytes());
            out.extend_from_slice(&u16::try_from(name_b.len()).unwrap().to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(name_b);
            out.extend_from_slice(data);
        }
        out
    }

    fn ole(with_stream: bool) -> Vec<u8> {
        let mut bytes = vec![0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
        bytes.extend(std::iter::repeat(0).take(32));
        if with_stream {
            for unit in "WordDocument".encode_utf16() {
                bytes.extend_from_slice(&unit.to_le_bytes());
            }
        }
        bytes
    }

    #[test]
    fn docx_with_word_entry_is_word_and_xlsx_is_not() {
        let docx = zip_stored(&[("word/document.xml", b"<w:document/>")]);
        assert_eq!(classify(&docx, "contrat.docx"), Some(WordKind::Docx));
        assert_eq!(classify(&docx, "renamed.bin"), Some(WordKind::Docx));
        assert_eq!(classify(&docx, "macro.docm"), Some(WordKind::Docm));
        assert_eq!(classify(&docx, "modele.dotx"), Some(WordKind::Dotx));
        let xlsx = zip_stored(&[("xl/workbook.xml", b"<x/>")]);
        assert_eq!(classify(&xlsx, "table.xlsx"), None);
        assert_eq!(classify(&xlsx, "renamed.docx"), None);
        let both = zip_stored(&[
            ("word/document.xml", b"<w/>"),
            ("xl/workbook.xml", b"<x/>"),
        ]);
        assert_eq!(classify(&both, "mix.docx"), None);
        let pptx = zip_stored(&[("ppt/slides/slide1.xml", b"<p/>")]);
        assert_eq!(classify(&pptx, "deck.pptx"), None);
    }

    #[test]
    fn odt_requires_the_first_mimetype_entry() {
        let odt = zip_stored(&[
            ("mimetype", b"application/vnd.oasis.opendocument.text"),
            ("content.xml", b"<office:document/>"),
        ]);
        assert_eq!(classify(&odt, "note.odt"), Some(WordKind::Odt));
        let ott = zip_stored(&[(
            "mimetype",
            b"application/vnd.oasis.opendocument.text-template",
        )]);
        assert_eq!(classify(&ott, "modele.ott"), Some(WordKind::Ott));
        let ods = zip_stored(&[(
            "mimetype",
            b"application/vnd.oasis.opendocument.spreadsheet",
        )]);
        assert_eq!(classify(&ods, "table.ods"), None);
        let late = zip_stored(&[
            ("content.xml", b"<office:document/>"),
            ("mimetype", b"application/vnd.oasis.opendocument.text"),
        ]);
        assert_eq!(classify(&late, "late.odt"), None);
    }

    #[test]
    fn rtf_ole_and_flat_odt() {
        assert_eq!(classify(b"{\\rtf1 hello}", "a.rtf"), Some(WordKind::Rtf));
        assert_eq!(classify(b"not {\\rtf1", "a.rtf"), None);
        let fodt = b"<?xml version=\"1.0\"?><office:document office:mimetype=\"application/vnd.oasis.opendocument.text\">";
        assert_eq!(classify(fodt, "a.fodt"), Some(WordKind::Fodt));
        assert_eq!(classify(&ole(true), "ancien.doc"), Some(WordKind::Doc));
        assert_eq!(classify(&ole(true), "ANCIEN.DOC"), Some(WordKind::Doc));
        assert_eq!(classify(&ole(true), "modele.dot"), Some(WordKind::Dot));
        assert_eq!(classify(&ole(false), "ancien.doc"), None);
        assert_eq!(classify(&ole(true), "table.xls"), None);
        assert_eq!(classify(&ole(true), "ancien.docx"), None);
    }

    #[test]
    fn data_descriptor_without_sizes_does_not_walk_forever() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"PK\x03\x04");
        bytes.extend_from_slice(&20u16.to_le_bytes());
        bytes.extend_from_slice(&0x0008u16.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        let name = b"word/document.xml";
        bytes.extend_from_slice(&u16::try_from(name.len()).unwrap().to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(name);
        assert_eq!(classify(&bytes, "a.docx"), Some(WordKind::Docx));
    }

    #[test]
    fn save_routes_pdf_png_and_rewritten_word() {
        assert_eq!(
            classify_save(b"%PDF-1.7", "export.pdf"),
            IncomingSave::Rendition
        );
        let png: &[u8] = b"\x89PNG\r\n\x1a\nrest";
        assert_eq!(classify_save(png, "page.pdf"), IncomingSave::Ignore);
        let docx = zip_stored(&[("word/document.xml", b"<w/>")]);
        assert_eq!(
            classify_save(&docx, "contrat.docx"),
            IncomingSave::Replace(WordKind::Docx)
        );
        assert_eq!(
            classify_save(&ole(true), "ancien.doc"),
            IncomingSave::Reject
        );
        assert_eq!(classify_save(b"<html></html>", "a.html"), IncomingSave::Reject);
        assert_eq!(classify_save(b"# titre", "a.md"), IncomingSave::Reject);
        assert_eq!(saved_filename("ancien.doc", WordKind::Docx), "ancien.docx");
        assert_eq!(saved_filename("contrat.docx", WordKind::Docx), "contrat.docx");
        assert_eq!(saved_filename("document.pdf", WordKind::Docx), "document.docx");
    }
}
