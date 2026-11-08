//! Text extraction from files: plain text (UTF-8/UTF-16), Office Open XML and OpenDocument, PDF, legacy
//! binary Office (printable strings), zip archives (one level, bomb-safe) and images (handed to OS OCR).

use quick_xml::events::Event;
use quick_xml::Reader;
use std::io::{Cursor, Read};
use std::path::Path;
use veil_core::Extracted;

/// Extracted text is capped at this many bytes (enough for every detection layer).
pub const MAX_TEXT_BYTES: usize = 4 * 1024 * 1024;
/// Archive limits.
const MAX_ARCHIVE_ENTRIES: usize = 200;
const MAX_ARCHIVE_TOTAL: u64 = 100 * 1024 * 1024;
/// Max bytes decompressed from a single zip member (office parts or archive entries).
const MAX_MEMBER_BYTES: u64 = 50 * 1024 * 1024;

pub fn extract(path: &Path, max_bytes: u64) -> Extracted {
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) => return Extracted::Unreadable(e.to_string()),
    };
    if meta.is_dir() {
        return Extracted::Unsupported;
    }
    if meta.len() > max_bytes {
        return Extracted::TooLarge;
    }
    match std::fs::read(path) {
        Ok(bytes) => {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            extract_bytes(&name, &bytes, 0, max_bytes)
        }
        Err(e) => Extracted::Unreadable(e.to_string()),
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Kind {
    Text,
    Office,
    OpenDocument,
    Archive,
    Pdf,
    Image,
    LegacyOffice,
    Binary,
}

fn extension(name: &str) -> String {
    Path::new(name).extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default()
}

fn classify(name: &str, bytes: &[u8]) -> Kind {
    let ext = extension(name);
    if bytes.starts_with(b"PK\x03\x04") || bytes.starts_with(b"PK\x05\x06") {
        return match ext.as_str() {
            "docx" | "docm" | "dotx" | "xlsx" | "xlsm" | "xltx" | "pptx" | "pptm" | "ppsx" | "potx" => Kind::Office,
            "odt" | "ods" | "odp" | "odg" | "ott" | "ots" | "otp" => Kind::OpenDocument,
            _ => Kind::Archive,
        };
    }
    if bytes.starts_with(b"%PDF") {
        return Kind::Pdf;
    }
    if is_image(bytes) || matches!(ext.as_str(), "heic" | "heif") && bytes.len() > 12 {
        return Kind::Image;
    }
    if bytes.starts_with(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]) {
        return Kind::LegacyOffice;
    }
    if decode_text(bytes).is_some() {
        return Kind::Text;
    }
    Kind::Binary
}

fn is_image(b: &[u8]) -> bool {
    b.starts_with(b"\x89PNG\r\n\x1a\n")
        || b.starts_with(&[0xFF, 0xD8, 0xFF])
        || b.starts_with(b"GIF87a")
        || b.starts_with(b"GIF89a")
        || (b.starts_with(b"BM") && b.len() > 26)
        || b.starts_with(b"II*\0")
        || b.starts_with(b"MM\0*")
        || (b.len() > 12 && &b[..4] == b"RIFF" && &b[8..12] == b"WEBP")
        || (b.len() > 12
            && &b[4..8] == b"ftyp"
            && matches!(&b[8..12], b"heic" | b"heix" | b"mif1" | b"msf1" | b"hevc" | b"avif"))
}

pub fn extract_bytes(name: &str, bytes: &[u8], depth: usize, max_bytes: u64) -> Extracted {
    match classify(name, bytes) {
        Kind::Text => Extracted::Text(cap(decode_text(bytes).unwrap_or_default())),
        Kind::Office => office(bytes),
        Kind::OpenDocument => open_document(bytes),
        Kind::Archive if depth == 0 => archive(bytes, max_bytes),
        Kind::Archive => Extracted::Unsupported,
        Kind::Pdf => pdf(bytes),
        Kind::Image => Extracted::Image(bytes.to_vec()),
        Kind::LegacyOffice => legacy_office(bytes),
        Kind::Binary => Extracted::Unsupported,
    }
}

fn cap(mut s: String) -> String {
    if s.len() > MAX_TEXT_BYTES {
        let mut end = MAX_TEXT_BYTES;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s.truncate(end);
    }
    s
}

/// Decode text with a BOM (UTF-8/UTF-16) or plain UTF-8. `None` for binary data.
pub fn decode_text(bytes: &[u8]) -> Option<String> {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return Some(String::from_utf8_lossy(rest).into_owned());
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        let units: Vec<u16> = rest.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        return Some(String::from_utf16_lossy(&units));
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        let units: Vec<u16> = rest.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
        return Some(String::from_utf16_lossy(&units));
    }
    let head = &bytes[..bytes.len().min(8192)];
    if head.contains(&0) {
        return None;
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => Some(s.to_string()),
        Err(e) => {
            // Allow a cut-off multi-byte sequence at the end, or mostly-UTF-8 text in a legacy encoding.
            let lossy = String::from_utf8_lossy(bytes);
            let bad = lossy.chars().filter(|&c| c == char::REPLACEMENT_CHARACTER).count();
            let total = lossy.chars().count().max(1);
            (e.error_len().is_none() || bad * 20 < total).then(|| lossy.into_owned())
        }
    }
}

fn open_zip(bytes: &[u8]) -> Result<zip::ZipArchive<Cursor<&[u8]>>, Extracted> {
    zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| Extracted::Unreadable(e.to_string()))
}

/// Read one zip member, bounded. `Err(Encrypted)` for encrypted members.
fn read_member(zip: &mut zip::ZipArchive<Cursor<&[u8]>>, index: usize, limit: u64) -> Result<Vec<u8>, Extracted> {
    let file = match zip.by_index(index) {
        Ok(f) => f,
        Err(zip::result::ZipError::UnsupportedArchive(msg)) if msg.to_ascii_lowercase().contains("password") => {
            return Err(Extracted::Encrypted)
        }
        Err(e) => return Err(Extracted::Unreadable(e.to_string())),
    };
    if file.encrypted() {
        return Err(Extracted::Encrypted);
    }
    let mut out = Vec::new();
    file.take(limit).read_to_end(&mut out).map_err(|e| Extracted::Unreadable(e.to_string()))?;
    Ok(out)
}

fn member_names(zip: &zip::ZipArchive<Cursor<&[u8]>>) -> Vec<String> {
    zip.file_names().map(String::from).collect()
}

/// Text of an XML part. Paragraph-like end tags become newlines, cells become tabs; runs are joined as-is
/// so words split across formatting runs stay whole.
fn xml_text(xml: &[u8], skip_shared_string_values: bool) -> String {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut out = String::new();
    let mut buf = Vec::new();
    let mut in_value_of_shared_cell = false;
    let mut shared_cell = false;
    let mut in_text_elem = 0usize;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = e.local_name();
                let name = name.as_ref();
                if name == b"c" && skip_shared_string_values {
                    shared_cell = e
                        .attributes()
                        .flatten()
                        .any(|a| a.key.local_name().as_ref() == b"t" && a.value.as_ref() == b"s");
                }
                if name == b"v" && shared_cell {
                    in_value_of_shared_cell = true;
                }
                if is_text_elem(name) {
                    in_text_elem += 1;
                }
            }
            Ok(Event::End(e)) => {
                let name = e.local_name();
                if is_text_elem(name.as_ref()) {
                    in_text_elem = in_text_elem.saturating_sub(1);
                }
                match name.as_ref() {
                    b"p" | b"h" | b"si" | b"row" | b"tr" | b"br" | b"list-item" => out.push('\n'),
                    b"c" | b"tc" | b"table-cell" => {
                        out.push('\t');
                        shared_cell = false;
                    }
                    b"v" => in_value_of_shared_cell = false,
                    _ => {}
                }
            }
            Ok(Event::Empty(e)) => match e.local_name().as_ref() {
                b"tab" => out.push('\t'),
                b"br" | b"cr" | b"line-break" => out.push('\n'),
                b"s" => out.push(' '),
                _ => {}
            },
            Ok(Event::Text(t)) => {
                if in_value_of_shared_cell {
                    // Shared-string index, not text.
                } else if let Ok(s) = t.unescape() {
                    if in_text_elem > 0 || !s.trim().is_empty() {
                        out.push_str(&s);
                    }
                }
            }
            Ok(Event::CData(t)) => out.push_str(&String::from_utf8_lossy(&t)),
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
        if out.len() > MAX_TEXT_BYTES {
            break;
        }
    }
    out
}

fn is_text_elem(name: &[u8]) -> bool {
    matches!(name, b"t" | b"v" | b"p" | b"h" | b"span")
}

fn office(bytes: &[u8]) -> Extracted {
    let mut zip = match open_zip(bytes) {
        Ok(z) => z,
        Err(e) => return e,
    };
    let names = member_names(&zip);
    let wanted = |n: &str| {
        (n.starts_with("word/") && n.ends_with(".xml") && !n.contains("/_rels/") && !n.ends_with("styles.xml")
            && !n.ends_with("settings.xml") && !n.ends_with("fontTable.xml") && !n.ends_with("webSettings.xml")
            && !n.contains("theme"))
            || n == "xl/sharedStrings.xml"
            || (n.starts_with("xl/worksheets/") && n.ends_with(".xml"))
            || (n.starts_with("xl/comments") && n.ends_with(".xml"))
            || ((n.starts_with("ppt/slides/") || n.starts_with("ppt/notesSlides/")) && n.ends_with(".xml"))
            || n == "docProps/core.xml"
    };
    let mut parts: Vec<(usize, &String)> = names.iter().enumerate().filter(|(_, n)| wanted(n)).collect();
    parts.sort_by(|a, b| natural_key(a.1).cmp(&natural_key(b.1)));
    let mut text = String::new();
    for (i, name) in parts {
        let data = match read_member(&mut zip, i, MAX_MEMBER_BYTES) {
            Ok(d) => d,
            Err(e @ Extracted::Encrypted) => return e,
            Err(_) => continue,
        };
        text.push_str(&xml_text(&data, name.starts_with("xl/worksheets/")));
        text.push('\n');
        if text.len() > MAX_TEXT_BYTES {
            break;
        }
    }
    Extracted::Text(cap(text))
}

/// "slide10.xml" sorts after "slide2.xml".
fn natural_key(s: &str) -> (String, u64) {
    let digits: String = s.chars().rev().skip_while(|c| !c.is_ascii_digit()).take_while(|c| c.is_ascii_digit()).collect();
    let n = digits.chars().rev().collect::<String>().parse().unwrap_or(0);
    let prefix: String = s.chars().take_while(|c| !c.is_ascii_digit()).collect();
    (prefix, n)
}

fn open_document(bytes: &[u8]) -> Extracted {
    let mut zip = match open_zip(bytes) {
        Ok(z) => z,
        Err(e) => return e,
    };
    let names = member_names(&zip);
    if let Some(i) = names.iter().position(|n| n == "META-INF/manifest.xml") {
        if let Ok(m) = read_member(&mut zip, i, MAX_MEMBER_BYTES) {
            if String::from_utf8_lossy(&m).contains("encryption-data") {
                return Extracted::Encrypted;
            }
        }
    }
    let mut text = String::new();
    for part in ["content.xml", "styles.xml", "meta.xml"] {
        if let Some(i) = names.iter().position(|n| n == part) {
            if let Ok(data) = read_member(&mut zip, i, MAX_MEMBER_BYTES) {
                text.push_str(&xml_text(&data, false));
                text.push('\n');
            }
        }
    }
    Extracted::Text(cap(text))
}

fn archive(bytes: &[u8], max_bytes: u64) -> Extracted {
    let mut zip = match open_zip(bytes) {
        Ok(z) => z,
        Err(e) => return e,
    };
    let mut parts = Vec::new();
    let mut total = 0u64;
    let mut encrypted = 0;
    for i in 0..zip.len().min(MAX_ARCHIVE_ENTRIES) {
        let (name, size, compressed, is_dir) = match zip.by_index_raw(i) {
            Ok(f) => (f.name().to_string(), f.size(), f.compressed_size(), f.is_dir()),
            Err(_) => continue,
        };
        if is_dir || name.starts_with("__MACOSX/") || name.ends_with(".DS_Store") {
            continue;
        }
        let ratio_bomb = size > 10 * 1024 * 1024 && size > compressed.saturating_mul(100);
        if size > max_bytes || ratio_bomb || total + size > MAX_ARCHIVE_TOTAL {
            parts.push((name, Extracted::TooLarge));
            continue;
        }
        match read_member(&mut zip, i, max_bytes.min(MAX_MEMBER_BYTES)) {
            Ok(data) => {
                total += data.len() as u64;
                let ex = extract_bytes(&name, &data, 1, max_bytes);
                parts.push((name, ex));
            }
            Err(Extracted::Encrypted) => {
                encrypted += 1;
                parts.push((name, Extracted::Encrypted));
            }
            Err(e) => parts.push((name, e)),
        }
    }
    if zip.len() > MAX_ARCHIVE_ENTRIES {
        parts.push((format!("({} more entries)", zip.len() - MAX_ARCHIVE_ENTRIES), Extracted::TooLarge));
    }
    if !parts.is_empty() && encrypted == parts.len() {
        return Extracted::Encrypted;
    }
    Extracted::Many(parts)
}

fn pdf(bytes: &[u8]) -> Extracted {
    let encrypted = memchr_find(bytes, b"/Encrypt").is_some();
    let owned = bytes.to_vec();
    let result = std::panic::catch_unwind(move || pdf_extract::extract_text_from_mem(&owned));
    match result {
        Ok(Ok(text)) if !text.trim().is_empty() => Extracted::Text(cap(text)),
        Ok(Ok(_)) if encrypted => Extracted::Encrypted,
        Ok(Ok(_)) => Extracted::Unreadable("PDF has no text layer (scanned pages)".into()),
        Ok(Err(_)) | Err(_) if encrypted => Extracted::Encrypted,
        Ok(Err(e)) => Extracted::Unreadable(format!("PDF: {e}")),
        Err(_) => Extracted::Unreadable("PDF could not be parsed".into()),
    }
}

fn memchr_find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Legacy .doc/.xls/.ppt (OLE compound files): pull printable runs of ASCII and UTF-16LE text.
/// Password-protected OOXML files are also OLE containers holding an `EncryptedPackage` stream.
fn legacy_office(bytes: &[u8]) -> Extracted {
    let marker: Vec<u8> = "EncryptedPackage".encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    if memchr_find(bytes, &marker).is_some() {
        return Extracted::Encrypted;
    }
    let mut text = String::new();
    // UTF-16LE runs.
    let mut run = String::new();
    for pair in bytes.chunks_exact(2) {
        let u = u16::from_le_bytes([pair[0], pair[1]]);
        match char::from_u32(u as u32) {
            Some(c) if (c.is_alphanumeric() || c.is_ascii_punctuation() || c == ' ') && u >= 0x20 => run.push(c),
            _ => flush_run(&mut run, &mut text, 6),
        }
    }
    flush_run(&mut run, &mut text, 6);
    // 8-bit runs.
    for &b in bytes {
        if (0x20..0x7f).contains(&b) {
            run.push(b as char);
        } else {
            flush_run(&mut run, &mut text, 6);
        }
    }
    flush_run(&mut run, &mut text, 6);
    Extracted::Text(cap(text))
}

fn flush_run(run: &mut String, out: &mut String, min: usize) {
    if run.chars().count() >= min && run.chars().any(|c| c.is_alphabetic()) {
        out.push_str(run.trim());
        out.push('\n');
    }
    run.clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn zip_with(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buf = Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
            for (name, data) in files {
                w.start_file(*name, opts).unwrap();
                w.write_all(data).unwrap();
            }
            w.finish().unwrap();
        }
        buf.into_inner()
    }

    fn text_of(e: Extracted) -> String {
        match e {
            Extracted::Text(t) => t,
            other => panic!("expected text, got {other:?}"),
        }
    }

    #[test]
    fn plain_text_and_utf16() {
        assert_eq!(text_of(extract_bytes("a.txt", "héllo".as_bytes(), 0, u64::MAX)), "héllo");
        let mut utf16 = vec![0xFF, 0xFE];
        utf16.extend("secret".encode_utf16().flat_map(|u| u.to_le_bytes()));
        assert_eq!(text_of(extract_bytes("a.txt", &utf16, 0, u64::MAX)), "secret");
        assert_eq!(extract_bytes("a.bin", &[0, 1, 2, 3, 0, 9], 0, u64::MAX), Extracted::Unsupported);
    }

    #[test]
    fn docx_runs_are_joined_and_paragraphs_split() {
        let doc = br#"<?xml version="1.0"?><w:document xmlns:w="x"><w:body>
            <w:p><w:r><w:t>Strictly conf</w:t></w:r><w:r><w:t>idential</w:t></w:r></w:p>
            <w:p><w:r><w:t xml:space="preserve">Card 4111 1111 1111 1111 &amp; more</w:t></w:r></w:p>
            </w:body></w:document>"#;
        let bytes = zip_with(&[("[Content_Types].xml", b"<Types/>"), ("word/document.xml", doc)]);
        let t = text_of(extract_bytes("plan.docx", &bytes, 0, u64::MAX));
        assert!(t.contains("Strictly confidential\n"), "{t:?}");
        assert!(t.contains("Card 4111 1111 1111 1111 & more"), "{t:?}");
    }

    #[test]
    fn xlsx_reads_shared_strings_and_numbers_but_not_indices() {
        let shared = br#"<sst><si><t>Salary list</t></si><si><t>jane@example.com</t></si></sst>"#;
        let sheet = br#"<worksheet><sheetData><row><c r="A1" t="s"><v>0</v></c><c r="B1"><v>4111111111111111</v></c></row>
            <row><c r="A2" t="s"><v>1</v></c><c r="B2" t="inlineStr"><is><t>inline note</t></is></c></row></sheetData></worksheet>"#;
        let bytes = zip_with(&[("xl/sharedStrings.xml", shared), ("xl/worksheets/sheet1.xml", sheet)]);
        let t = text_of(extract_bytes("pay.xlsx", &bytes, 0, u64::MAX));
        assert!(t.contains("Salary list") && t.contains("jane@example.com"), "{t:?}");
        assert!(t.contains("4111111111111111") && t.contains("inline note"), "{t:?}");
        assert!(!t.contains("\t0\t") && !t.starts_with('0'), "shared-string indices leaked: {t:?}");
    }

    #[test]
    fn pptx_slides_in_order() {
        let s1 = br#"<p:sld><a:p><a:r><a:t>first</a:t></a:r></a:p></p:sld>"#;
        let s10 = br#"<p:sld><a:p><a:r><a:t>tenth</a:t></a:r></a:p></p:sld>"#;
        let s2 = br#"<p:sld><a:p><a:r><a:t>second</a:t></a:r></a:p></p:sld>"#;
        let bytes = zip_with(&[("ppt/slides/slide10.xml", s10), ("ppt/slides/slide1.xml", s1), ("ppt/slides/slide2.xml", s2)]);
        let t = text_of(extract_bytes("deck.pptx", &bytes, 0, u64::MAX));
        let (a, b, c) = (t.find("first").unwrap(), t.find("second").unwrap(), t.find("tenth").unwrap());
        assert!(a < b && b < c, "{t:?}");
    }

    #[test]
    fn odt_and_encrypted_odt() {
        let content = br#"<office:document-content><office:body><office:text><text:p>Proyecto<text:s/>secreto</text:p></office:text></office:body></office:document-content>"#;
        let bytes = zip_with(&[("mimetype", b"application/vnd.oasis.opendocument.text"), ("content.xml", content)]);
        let t = text_of(extract_bytes("a.odt", &bytes, 0, u64::MAX));
        assert!(t.contains("Proyecto secreto"), "{t:?}");
        let manifest = br#"<manifest:manifest><manifest:file-entry><manifest:encryption-data/></manifest:file-entry></manifest:manifest>"#;
        let enc = zip_with(&[("META-INF/manifest.xml", manifest), ("content.xml", b"garbage")]);
        assert_eq!(extract_bytes("a.odt", &enc, 0, u64::MAX), Extracted::Encrypted);
    }

    #[test]
    fn archives_recurse_one_level() {
        let inner = zip_with(&[("deep.txt", b"deep secret")]);
        let bytes = zip_with(&[("notes.txt", b"top secret"), ("nested.zip", &inner), ("dir/pic.png", b"\x89PNG\r\n\x1a\nxxxx")]);
        match extract_bytes("bundle.zip", &bytes, 0, u64::MAX) {
            Extracted::Many(parts) => {
                assert_eq!(parts.len(), 3);
                assert_eq!(parts[0], ("notes.txt".into(), Extracted::Text("top secret".into())));
                assert_eq!(parts[1].1, Extracted::Unsupported, "no second level");
                assert!(matches!(parts[2].1, Extracted::Image(_)));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn zip_bombs_are_not_inflated() {
        let big = vec![b'a'; 30 * 1024 * 1024];
        let bytes = zip_with(&[("bomb.txt", &big)]);
        assert!(bytes.len() < 1024 * 1024);
        match extract_bytes("bomb.zip", &bytes, 0, 25 * 1024 * 1024) {
            Extracted::Many(parts) => assert_eq!(parts[0].1, Extracted::TooLarge),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn images_and_size_cap() {
        assert!(matches!(extract_bytes("x.png", b"\x89PNG\r\n\x1a\n0000", 0, u64::MAX), Extracted::Image(_)));
        assert!(matches!(extract_bytes("x.jpg", &[0xFF, 0xD8, 0xFF, 0xE0, 1, 2], 0, u64::MAX), Extracted::Image(_)));
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("big.txt");
        std::fs::write(&p, vec![b'a'; 2048]).unwrap();
        assert_eq!(extract(&p, 1024), Extracted::TooLarge);
        assert!(matches!(extract(&dir.path().join("missing.txt"), 1024), Extracted::Unreadable(_)));
        assert_eq!(extract(dir.path(), 1024), Extracted::Unsupported);
    }

    #[test]
    fn broken_pdf_is_unreadable_not_a_panic() {
        let e = extract_bytes("x.pdf", b"%PDF-1.4\n garbage without objects", 0, u64::MAX);
        assert!(matches!(e, Extracted::Unreadable(_)), "{e:?}");
        let e = extract_bytes("x.pdf", b"%PDF-1.4\n/Encrypt 5 0 R garbage", 0, u64::MAX);
        assert_eq!(e, Extracted::Encrypted);
    }

    #[test]
    fn minimal_pdf_text_is_extracted() {
        let pdf = minimal_pdf("Quarterly numbers are confidential");
        let t = text_of(extract_bytes("q.pdf", &pdf, 0, u64::MAX));
        assert!(t.contains("confidential"), "{t:?}");
    }

    #[test]
    fn legacy_office_strings_and_encrypted_ooxml() {
        let mut doc = vec![0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1, 0, 0];
        doc.extend("Board minutes confidential".encode_utf16().flat_map(|u| u.to_le_bytes()));
        doc.extend([0, 0, 0, 0]);
        let t = text_of(extract_bytes("old.doc", &doc, 0, u64::MAX));
        assert!(t.contains("Board minutes confidential"), "{t:?}");
        let mut enc = vec![0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1, 0, 0];
        enc.extend("EncryptedPackage".encode_utf16().flat_map(|u| u.to_le_bytes()));
        assert_eq!(extract_bytes("locked.docx", &enc, 0, u64::MAX), Extracted::Encrypted);
    }

    /// A one-page PDF with a single line of text (Helvetica), with a correct xref table.
    pub fn minimal_pdf(text: &str) -> Vec<u8> {
        let stream = format!("BT /F1 12 Tf 72 712 Td ({text}) Tj ET");
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>".to_string(),
            format!("<< /Length {} >>\nstream\n{stream}\nendstream", stream.len()),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_string(),
        ];
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend(format!("{} 0 obj\n{o}\nendobj\n", i + 1).bytes());
        }
        let xref = out.len();
        out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).bytes());
        for off in offsets {
            out.extend(format!("{off:010} 00000 n \n").bytes());
        }
        out.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).bytes());
        out
    }
}
