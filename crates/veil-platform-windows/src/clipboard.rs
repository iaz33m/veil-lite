//! Clipboard read/write: Unicode text, file lists (`CF_HDROP`) and images (`PNG`, `CF_DIBV5`, `CF_DIB`).

use crate::util::wide;
use std::path::PathBuf;
use veil_core::ClipboardContent;
use windows::core::w;
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND, POINT};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Ole::{CF_DIB, CF_DIBV5, CF_HDROP, CF_UNICODETEXT};
use windows::Win32::UI::Shell::{DragQueryFileW, DROPFILES, HDROP};

const MAX_IMAGE_BYTES: usize = 64 * 1024 * 1024;

struct Open;

impl Drop for Open {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseClipboard();
        }
    }
}

/// Open the clipboard, retrying while another process holds it.
fn open(owner: Option<HWND>) -> Option<Open> {
    for _ in 0..10 {
        if unsafe { OpenClipboard(owner) }.is_ok() {
            return Some(Open);
        }
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
    tracing::debug!("clipboard busy");
    None
}

fn png_format() -> u32 {
    unsafe { RegisterClipboardFormatW(w!("PNG")) }
}

fn available(fmt: u32) -> bool {
    unsafe { IsClipboardFormatAvailable(fmt) }.is_ok()
}

/// Copy the bytes of a clipboard handle.
fn bytes_of(fmt: u32) -> Option<Vec<u8>> {
    unsafe {
        let h = GetClipboardData(fmt).ok()?;
        let g = HGLOBAL(h.0);
        let size = GlobalSize(g);
        if size == 0 || size > MAX_IMAGE_BYTES {
            return None;
        }
        let p = GlobalLock(g) as *const u8;
        if p.is_null() {
            return None;
        }
        let v = std::slice::from_raw_parts(p, size).to_vec();
        let _ = GlobalUnlock(g);
        Some(v)
    }
}

fn read_text() -> Option<String> {
    let fmt = CF_UNICODETEXT.0 as u32;
    if !available(fmt) {
        return None;
    }
    let bytes = bytes_of(fmt)?;
    let units: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    let s = crate::util::from_wide(&units);
    (!s.is_empty()).then_some(s)
}

fn read_files() -> Vec<PathBuf> {
    let fmt = CF_HDROP.0 as u32;
    if !available(fmt) {
        return Vec::new();
    }
    unsafe {
        let Ok(h) = GetClipboardData(fmt) else { return Vec::new() };
        let drop = HDROP(h.0);
        let count = DragQueryFileW(drop, 0xFFFF_FFFF, None).min(1000);
        let mut out = Vec::with_capacity(count as usize);
        for i in 0..count {
            let len = DragQueryFileW(drop, i, None) as usize;
            let mut buf = vec![0u16; len + 1];
            DragQueryFileW(drop, i, Some(&mut buf));
            let s = crate::util::from_wide(&buf);
            if !s.is_empty() {
                out.push(PathBuf::from(s));
            }
        }
        out
    }
}

/// Packed DIB (BITMAPINFOHEADER/V5 + palette/masks + pixels) → BMP file bytes.
pub fn dib_to_bmp(dib: &[u8]) -> Option<Vec<u8>> {
    if dib.len() < 40 {
        return None;
    }
    let u32_at = |o: usize| u32::from_le_bytes([dib[o], dib[o + 1], dib[o + 2], dib[o + 3]]);
    let header_size = u32_at(0) as usize;
    let bit_count = u16::from_le_bytes([dib[14], dib[15]]);
    let compression = u32_at(16);
    let colors_used = u32_at(32) as usize;
    let mut palette = colors_used * 4;
    if palette == 0 && bit_count <= 8 {
        palette = (1usize << bit_count) * 4;
    }
    // BI_BITFIELDS with a plain BITMAPINFOHEADER stores three DWORD masks after the header.
    let masks = if compression == 3 && header_size == 40 { 12 } else { 0 };
    let offset = 14 + header_size + masks + palette;
    let total = 14 + dib.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(offset as u32).to_le_bytes());
    out.extend_from_slice(dib);
    Some(out)
}

fn read_image() -> Option<Vec<u8>> {
    let png = png_format();
    if png != 0 && available(png) {
        if let Some(b) = bytes_of(png) {
            return Some(b);
        }
    }
    for fmt in [CF_DIBV5.0 as u32, CF_DIB.0 as u32] {
        if available(fmt) {
            if let Some(dib) = bytes_of(fmt) {
                return dib_to_bmp(&dib);
            }
        }
    }
    None
}

pub fn read() -> ClipboardContent {
    let Some(_open) = open(None) else { return ClipboardContent::default() };
    let files = read_files();
    let image = if files.is_empty() { read_image() } else { None };
    ClipboardContent { text: read_text(), files, image }
}

/// Global memory block holding `bytes` (owned by the system after a successful `SetClipboardData`).
fn alloc(bytes: &[u8]) -> Option<HGLOBAL> {
    unsafe {
        let g = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1)).ok()?;
        let p = GlobalLock(g) as *mut u8;
        if p.is_null() {
            let _ = GlobalFree(Some(g));
            return None;
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
        let _ = GlobalUnlock(g);
        Some(g)
    }
}

fn put(fmt: u32, bytes: &[u8]) -> bool {
    let Some(g) = alloc(bytes) else { return false };
    match unsafe { SetClipboardData(fmt, Some(HANDLE(g.0))) } {
        Ok(_) => true,
        Err(e) => {
            tracing::debug!("SetClipboardData({fmt}) failed: {e}");
            unsafe {
                let _ = GlobalFree(Some(g));
            }
            false
        }
    }
}

fn hdrop_bytes(files: &[PathBuf]) -> Vec<u8> {
    let header = DROPFILES {
        pFiles: std::mem::size_of::<DROPFILES>() as u32,
        pt: POINT { x: 0, y: 0 },
        fNC: false.into(),
        fWide: true.into(),
    };
    let mut out = Vec::new();
    let raw = unsafe {
        std::slice::from_raw_parts(&header as *const DROPFILES as *const u8, std::mem::size_of::<DROPFILES>())
    };
    out.extend_from_slice(raw);
    for f in files {
        for u in wide(&f.to_string_lossy()) {
            out.extend_from_slice(&u.to_le_bytes());
        }
    }
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

/// Replace the clipboard with `content`. `owner` must be a window of this process (required for
/// `SetClipboardData` after `EmptyClipboard`).
pub fn write(owner: HWND, content: &ClipboardContent) -> bool {
    let Some(_open) = open(Some(owner)) else { return false };
    if unsafe { EmptyClipboard() }.is_err() {
        return false;
    }
    let mut ok = true;
    if let Some(text) = &content.text {
        let bytes: Vec<u8> = wide(text).iter().flat_map(|u| u.to_le_bytes()).collect();
        ok &= put(CF_UNICODETEXT.0 as u32, &bytes);
    }
    if !content.files.is_empty() {
        ok &= put(CF_HDROP.0 as u32, &hdrop_bytes(&content.files));
    }
    if let Some(img) = &content.image {
        if img.starts_with(b"\x89PNG") {
            ok &= put(png_format(), img);
        } else if img.starts_with(b"BM") && img.len() > 14 {
            ok &= put(CF_DIB.0 as u32, &img[14..]);
        }
    }
    ok
}
