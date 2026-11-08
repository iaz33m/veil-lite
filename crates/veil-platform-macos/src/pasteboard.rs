//! NSPasteboard: the general clipboard (read, write, full snapshot/restore) and the drag pasteboard
//! (session data, withholding while a drag hovers over a protected app).

use objc2::rc::{autoreleasepool, Retained};
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{
    NSPasteboard, NSPasteboardItem, NSPasteboardNameDrag, NSPasteboardTypeFileURL, NSPasteboardTypePNG,
    NSPasteboardTypeString, NSPasteboardTypeTIFF, NSPasteboardWriting,
};
use objc2_foundation::{NSArray, NSData, NSString, NSURL};
use std::path::PathBuf;
use veil_core::types::{ClipboardContent, DragSession};

/// Every item of a pasteboard with every type's raw data.
pub type Items = Vec<Vec<(String, Vec<u8>)>>;

/// Snapshots larger than this are not kept (restoring then falls back to the summarised content).
const MAX_SNAPSHOT_BYTES: usize = 64 * 1024 * 1024;

pub fn general() -> Retained<NSPasteboard> {
    NSPasteboard::generalPasteboard()
}

pub fn drag() -> Retained<NSPasteboard> {
    NSPasteboard::pasteboardWithName(unsafe { NSPasteboardNameDrag })
}

pub fn change_count(pb: &NSPasteboard) -> isize {
    pb.changeCount()
}

/// Copy every item and type (for a faithful restore). `None` if larger than the cap.
pub fn snapshot(pb: &NSPasteboard) -> Option<Items> {
    autoreleasepool(|_| {
        let mut total = 0usize;
        let mut out = Vec::new();
        let items = pb.pasteboardItems()?;
        for item in items.iter() {
            let mut pairs = Vec::new();
            for t in item.types().iter() {
                if let Some(d) = item.dataForType(&t) {
                    total += d.len();
                    if total > MAX_SNAPSHOT_BYTES {
                        return None;
                    }
                    pairs.push((t.to_string(), d.to_vec()));
                }
            }
            out.push(pairs);
        }
        Some(out)
    })
}

/// Replace the pasteboard contents with previously snapshotted items.
pub fn restore(pb: &NSPasteboard, items: &Items) {
    autoreleasepool(|_| {
        pb.clearContents();
        if items.is_empty() {
            return;
        }
        let objects: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> = items
            .iter()
            .map(|pairs| {
                let item = NSPasteboardItem::new();
                for (t, d) in pairs {
                    item.setData_forType(&NSData::with_bytes(d), &NSString::from_str(t));
                }
                ProtocolObject::from_retained(item)
            })
            .collect();
        pb.writeObjects(&NSArray::from_retained_slice(&objects));
    })
}

fn file_urls(pb: &NSPasteboard) -> Vec<PathBuf> {
    let Some(items) = pb.pasteboardItems() else { return Vec::new() };
    let mut out = Vec::new();
    for item in items.iter() {
        let Some(s) = item.stringForType(unsafe { NSPasteboardTypeFileURL }) else { continue };
        let Some(url) = NSURL::URLWithString(&s) else { continue };
        // File reference URLs (file:///.file/id=…) resolve to a path through `path`.
        if let Some(path) = url.path() {
            let p = path.to_string();
            if !p.is_empty() {
                out.push(PathBuf::from(p));
            }
        }
    }
    out
}

fn has_type(pb: &NSPasteboard, wanted: &[&str]) -> bool {
    pb.types().is_some_and(|types| types.iter().any(|t| wanted.contains(&t.to_string().as_str())))
}

fn image_bytes(pb: &NSPasteboard) -> Option<Vec<u8>> {
    let png = pb.dataForType(unsafe { NSPasteboardTypePNG });
    let data = png.or_else(|| pb.dataForType(unsafe { NSPasteboardTypeTIFF }))?;
    let bytes = data.to_vec();
    (!bytes.is_empty()).then_some(bytes)
}

/// Read the general clipboard. Images are only reported when no files are present (Finder puts file
/// icons on the clipboard next to the file URLs).
pub fn read_clipboard() -> ClipboardContent {
    autoreleasepool(|_| {
        let pb = general();
        let text = pb.stringForType(unsafe { NSPasteboardTypeString }).map(|s| s.to_string());
        let files = file_urls(&pb);
        let image = if files.is_empty() { image_bytes(&pb) } else { None };
        ClipboardContent { text, files, image }
    })
}

/// Write simple content (text / files / image) to the general clipboard.
pub fn write_clipboard(content: &ClipboardContent) -> bool {
    autoreleasepool(|_| {
        let pb = general();
        pb.clearContents();
        let mut objects: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> = Vec::new();
        if let Some(text) = &content.text {
            let item = NSPasteboardItem::new();
            item.setString_forType(&NSString::from_str(text), unsafe { NSPasteboardTypeString });
            objects.push(ProtocolObject::from_retained(item));
        }
        for f in &content.files {
            let url = NSURL::fileURLWithPath(&NSString::from_str(&f.to_string_lossy()));
            if let Some(s) = url.absoluteString() {
                let item = NSPasteboardItem::new();
                item.setString_forType(&s, unsafe { NSPasteboardTypeFileURL });
                objects.push(ProtocolObject::from_retained(item));
            }
        }
        if let Some(img) = &content.image {
            let item = NSPasteboardItem::new();
            let t = if img.starts_with(&[0x89, b'P', b'N', b'G']) {
                unsafe { NSPasteboardTypePNG }
            } else {
                unsafe { NSPasteboardTypeTIFF }
            };
            item.setData_forType(&NSData::with_bytes(img), t);
            objects.push(ProtocolObject::from_retained(item));
        }
        if objects.is_empty() {
            return true;
        }
        pb.writeObjects(&NSArray::from_retained_slice(&objects))
    })
}

/// What a drag pasteboard (or a snapshot of it) carries.
pub fn session_from_pasteboard(pb: &NSPasteboard) -> DragSession {
    autoreleasepool(|_| DragSession {
        files: file_urls(pb),
        text: pb.stringForType(unsafe { NSPasteboardTypeString }).map(|s| s.to_string()).filter(|s| !s.is_empty()),
        has_image: has_type(pb, &["public.png", "public.tiff", "public.jpeg", "public.heic"]),
    })
}

/// The same, from snapshotted items (used while the drag data is withheld).
pub fn session_from_items(items: &Items) -> DragSession {
    let mut s = DragSession::default();
    for pairs in items {
        for (t, d) in pairs {
            match t.as_str() {
                "public.file-url" => {
                    let url = String::from_utf8_lossy(d).trim_end_matches('\0').to_string();
                    if let Some(p) = file_url_to_path(&url) {
                        s.files.push(p);
                    }
                }
                "public.utf8-plain-text" | "NSStringPboardType" => {
                    let text = String::from_utf8_lossy(d).to_string();
                    if !text.is_empty() && s.text.is_none() {
                        s.text = Some(text);
                    }
                }
                "public.png" | "public.tiff" | "public.jpeg" | "public.heic" => s.has_image = true,
                _ => {}
            }
        }
    }
    s
}

fn file_url_to_path(url: &str) -> Option<PathBuf> {
    autoreleasepool(|_| {
        let u = NSURL::URLWithString(&NSString::from_str(url))?;
        u.path().map(|p| PathBuf::from(p.to_string()))
    })
}
