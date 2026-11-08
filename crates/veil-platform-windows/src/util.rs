//! Small Win32 helpers. Window handles cross threads as `isize` because `HWND` is not `Send`.

use std::ffi::c_void;
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetClassNameW, GetWindowTextW, GetWindowThreadProcessId, WindowFromPoint, GA_ROOT,
};

/// NUL-terminated UTF-16.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// UTF-16 buffer (up to the first NUL) to a `String`.
pub fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

pub fn hwnd(v: isize) -> HWND {
    HWND(v as *mut c_void)
}

pub fn hwnd_val(h: HWND) -> isize {
    h.0 as isize
}

pub fn class_name(h: HWND) -> String {
    if h.0.is_null() {
        return String::new();
    }
    let mut buf = [0u16; 256];
    let n = unsafe { GetClassNameW(h, &mut buf) };
    if n <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buf[..n as usize])
}

pub fn window_text(h: HWND) -> String {
    if h.0.is_null() {
        return String::new();
    }
    let mut buf = [0u16; 512];
    let n = unsafe { GetWindowTextW(h, &mut buf) };
    if n <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buf[..n as usize])
}

/// (process id, thread id) owning a window.
pub fn window_pid(h: HWND) -> (u32, u32) {
    let mut pid = 0u32;
    let tid = unsafe { GetWindowThreadProcessId(h, Some(&mut pid)) };
    (pid, tid)
}

/// Top-level window under a screen point (physical pixels; the process is per-monitor DPI aware).
pub fn root_at(x: i32, y: i32) -> HWND {
    unsafe {
        let h = WindowFromPoint(POINT { x, y });
        if h.0.is_null() {
            return h;
        }
        let root = GetAncestor(h, GA_ROOT);
        if root.0.is_null() {
            h
        } else {
            root
        }
    }
}

/// Window classes of Chromium/Electron and Firefox top-level windows. Their web content ignores UIA
/// `ValuePattern.SetValue` for frameworks such as React, so composer text is replaced via the clipboard.
pub fn is_web_class(class: &str) -> bool {
    class.starts_with("Chrome_WidgetWin") || class == "MozillaWindowClass"
}

/// Common file dialogs (`#32770` with the file-name control, id 1148).
pub fn is_file_dialog(root: HWND) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::GetDlgItem;
    if class_name(root) != "#32770" {
        return false;
    }
    unsafe { GetDlgItem(Some(root), 1148).is_ok() }
}
