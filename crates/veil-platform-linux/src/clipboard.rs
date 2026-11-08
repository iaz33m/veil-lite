//! Clipboard through arboard (X11, or wlr/ext data-control on Wayland with an X11 fallback), and the
//! pure helpers for `text/uri-list` and PNG.

use parking_lot::Mutex;
use percent_encoding::percent_decode_str;
use std::borrow::Cow;
use std::path::PathBuf;
use veil_core::types::ClipboardContent;

/// Local paths from a `text/uri-list` (RFC 2483): comments skipped, only `file:` URIs on this host.
pub fn parse_uri_list(list: &str) -> Vec<PathBuf> {
    list.lines()
        .map(|l| l.trim_end_matches('\r').trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let rest = l.strip_prefix("file://")?;
            // file:///path or file://localhost/path; other hosts are not local files.
            let path = if rest.starts_with('/') {
                rest
            } else {
                let (host, path) = rest.split_at(rest.find('/')?);
                if host != "localhost" {
                    return None;
                }
                path
            };
            let decoded = percent_decode_str(path).decode_utf8().ok()?;
            Some(PathBuf::from(decoded.into_owned()))
        })
        .collect()
}

/// Plain-text bodies sometimes carry paths (e.g. `x-special/gnome-copied-files`: `copy\nfile:///a`).
pub fn parse_gnome_copied_files(body: &str) -> Vec<PathBuf> {
    let mut lines = body.lines();
    match lines.next() {
        Some("copy") | Some("cut") => parse_uri_list(&lines.collect::<Vec<_>>().join("\n")),
        _ => Vec::new(),
    }
}

pub fn encode_png(width: usize, height: usize, rgba: &[u8]) -> Option<Vec<u8>> {
    if width == 0 || height == 0 || rgba.len() != width * height * 4 {
        return None;
    }
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, width as u32, height as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().ok()?;
        writer.write_image_data(rgba).ok()?;
    }
    Some(out)
}

pub fn decode_png(bytes: &[u8]) -> Option<(usize, usize, Vec<u8>)> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::ALPHA);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    buf.truncate(info.buffer_size());
    let (w, h) = (info.width as usize, info.height as usize);
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::GrayscaleAlpha => buf.chunks(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        _ => return None,
    };
    (rgba.len() == w * h * 4).then_some((w, h, rgba))
}

/// One long-lived arboard handle (on X11 the clipboard content we set lives as long as it does).
#[derive(Default)]
pub struct Clipboard {
    inner: Mutex<Option<arboard::Clipboard>>,
}

impl Clipboard {
    fn with<R>(&self, f: impl FnOnce(&mut arboard::Clipboard) -> R) -> Option<R> {
        let mut guard = self.inner.lock();
        if guard.is_none() {
            match arboard::Clipboard::new() {
                Ok(c) => *guard = Some(c),
                Err(e) => {
                    tracing::warn!("clipboard unavailable: {e}");
                    return None;
                }
            }
        }
        guard.as_mut().map(f)
    }

    pub fn read(&self) -> ClipboardContent {
        self.with(|c| {
            let files = c.get().file_list().unwrap_or_default();
            let text = if files.is_empty() { c.get_text().ok() } else { None };
            let image = if files.is_empty() && text.as_deref().is_none_or(|t| t.is_empty()) {
                c.get_image().ok().and_then(|img| encode_png(img.width, img.height, &img.bytes))
            } else {
                None
            };
            ClipboardContent { text, files, image }
        })
        .unwrap_or_default()
    }

    pub fn write(&self, content: &ClipboardContent) -> bool {
        self.with(|c| {
            if !content.files.is_empty() {
                return c.set().file_list(&content.files).is_ok();
            }
            if let Some(png) = &content.image {
                if let Some((width, height, rgba)) = decode_png(png) {
                    return c.set_image(arboard::ImageData { width, height, bytes: Cow::Owned(rgba) }).is_ok();
                }
            }
            match &content.text {
                Some(t) => c.set_text(t.clone()).is_ok(),
                None => c.clear().is_ok(),
            }
        })
        .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_lists_become_local_paths() {
        let list = "# comment\r\nfile:///home/me/My%20Plan.docx\r\nfile://localhost/tmp/a.txt\nhttps://example.com/x\nfile://otherhost/etc/passwd\n\n";
        assert_eq!(parse_uri_list(list), vec![PathBuf::from("/home/me/My Plan.docx"), PathBuf::from("/tmp/a.txt")]);
        assert_eq!(parse_uri_list("file:///caf%C3%A9.pdf"), vec![PathBuf::from("/café.pdf")]);
        assert!(parse_uri_list("not a uri").is_empty());
    }

    #[test]
    fn gnome_copied_files() {
        assert_eq!(parse_gnome_copied_files("copy\nfile:///a/b.txt"), vec![PathBuf::from("/a/b.txt")]);
        assert!(parse_gnome_copied_files("hello\nfile:///a").is_empty());
    }

    #[test]
    fn png_round_trip() {
        let rgba: Vec<u8> = (0..2 * 3 * 4).map(|i| i as u8).collect();
        let png = encode_png(2, 3, &rgba).unwrap();
        assert_eq!(&png[1..4], b"PNG");
        assert_eq!(decode_png(&png), Some((2, 3, rgba)));
        assert!(encode_png(2, 2, &[0; 3]).is_none());
        assert!(decode_png(b"nope").is_none());
    }
}
