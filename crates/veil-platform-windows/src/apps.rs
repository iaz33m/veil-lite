//! Which application owns a window: cached process info, the foreground app (hook path: must be fast),
//! the app under a point, and browser URLs fetched asynchronously through UI Automation.

use crate::com::ComWorker;
use crate::util::{hwnd, hwnd_val, root_at, window_pid, window_text};
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use veil_core::AppContext;
use windows::core::PWSTR;
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

const PROC_TTL: Duration = Duration::from_secs(30);
const TITLE_TTL: Duration = Duration::from_millis(400);
const URL_TTL: Duration = Duration::from_secs(3);

const BROWSERS: &[&str] = &[
    "chrome.exe",
    "msedge.exe",
    "brave.exe",
    "firefox.exe",
    "opera.exe",
    "opera_gx.exe",
    "vivaldi.exe",
    "arc.exe",
    "chromium.exe",
    "zen.exe",
];

#[derive(Clone)]
struct ProcInfo {
    app_id: String,
    name: String,
    at: Instant,
}

struct Fg {
    hwnd: isize,
    ctx: AppContext,
    at: Instant,
}

#[derive(Clone)]
struct UrlEntry {
    title: String,
    url: Option<String>,
    at: Instant,
}

pub struct Apps {
    procs: Mutex<HashMap<u32, ProcInfo>>,
    fg: Mutex<Option<Fg>>,
    /// Last top-level window seen in the foreground per process (for refocus).
    last_window: Mutex<HashMap<u32, isize>>,
    urls: Arc<Mutex<HashMap<isize, UrlEntry>>>,
    pending_urls: Arc<Mutex<HashSet<isize>>>,
    slow: Arc<ComWorker>,
}

impl Apps {
    pub fn new(slow: Arc<ComWorker>) -> Self {
        Apps {
            procs: Mutex::new(HashMap::new()),
            fg: Mutex::new(None),
            last_window: Mutex::new(HashMap::new()),
            urls: Arc::new(Mutex::new(HashMap::new())),
            pending_urls: Arc::new(Mutex::new(HashSet::new())),
            slow,
        }
    }

    fn proc_info(&self, pid: u32) -> ProcInfo {
        if let Some(p) = self.procs.lock().get(&pid) {
            if p.at.elapsed() < PROC_TTL {
                return p.clone();
            }
        }
        let path = exe_path(pid);
        let app_id = path
            .as_deref()
            .and_then(|p| std::path::Path::new(p).file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let name = path
            .as_deref()
            .and_then(file_description)
            .filter(|d| !d.trim().is_empty())
            .unwrap_or_else(|| app_id.trim_end_matches(".exe").trim_end_matches(".EXE").to_string());
        let info = ProcInfo { app_id, name, at: Instant::now() };
        let mut procs = self.procs.lock();
        if procs.len() > 512 {
            procs.retain(|_, p| p.at.elapsed() < PROC_TTL);
        }
        procs.insert(pid, info.clone());
        info
    }

    fn context_for(&self, root: isize) -> Option<AppContext> {
        let h = hwnd(root);
        let (pid, _) = window_pid(h);
        if pid == 0 {
            return None;
        }
        let p = self.proc_info(pid);
        let title = window_text(h);
        let url = self.cached_url(root, &p.app_id, &title);
        Some(AppContext {
            app_id: p.app_id,
            name: p.name,
            pid,
            window_title: (!title.is_empty()).then_some(title),
            url,
        })
    }

    /// Foreground application. Called from the keyboard hook: a pointer compare when nothing changed.
    pub fn foreground(&self) -> Option<AppContext> {
        let h = unsafe { GetForegroundWindow() };
        if h.0.is_null() {
            return None;
        }
        let root = hwnd_val(h);
        {
            let fg = self.fg.lock();
            if let Some(f) = fg.as_ref() {
                if f.hwnd == root && f.at.elapsed() < TITLE_TTL {
                    return Some(f.ctx.clone());
                }
            }
        }
        let ctx = self.context_for(root)?;
        self.last_window.lock().insert(ctx.pid, root);
        *self.fg.lock() = Some(Fg { hwnd: root, ctx: ctx.clone(), at: Instant::now() });
        Some(ctx)
    }

    /// Foreground top-level window handle.
    pub fn foreground_hwnd(&self) -> isize {
        hwnd_val(unsafe { GetForegroundWindow() })
    }

    /// Application owning the top-level window under a point.
    pub fn app_at(&self, x: i32, y: i32) -> Option<AppContext> {
        let root = root_at(x, y);
        if root.0.is_null() {
            return None;
        }
        self.context_for(hwnd_val(root))
    }

    /// Last foreground window of a process (for refocus after a prompt).
    pub fn window_of(&self, pid: u32) -> Option<isize> {
        self.last_window.lock().get(&pid).copied()
    }

    fn cached_url(&self, root: isize, app_id: &str, title: &str) -> Option<String> {
        let lower = app_id.to_ascii_lowercase();
        if !BROWSERS.contains(&lower.as_str()) {
            return None;
        }
        let entry = self.urls.lock().get(&root).cloned();
        let fresh = entry.as_ref().is_some_and(|e| e.title == title && e.at.elapsed() < URL_TTL);
        if !fresh {
            self.refresh_url(root, lower == "firefox.exe" || lower == "zen.exe", title.to_string());
        }
        // A stale URL is better than none while the refresh runs, unless the tab changed.
        entry.filter(|e| e.title == title).and_then(|e| e.url)
    }

    fn refresh_url(&self, root: isize, firefox: bool, title: String) {
        if !self.pending_urls.lock().insert(root) {
            return;
        }
        let (urls, pending) = (self.urls.clone(), self.pending_urls.clone());
        self.slow.spawn_job(move |ctx| {
            let url = crate::uia::browser_url(ctx, root, firefox);
            let mut map = urls.lock();
            if map.len() > 256 {
                map.clear();
            }
            map.insert(root, UrlEntry { title, url, at: Instant::now() });
            pending.lock().remove(&root);
        });
    }
}

fn exe_path(pid: u32) -> Option<String> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(handle);
        ok.then(|| String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// `FileDescription` (e.g. "Google Chrome", "Slack") from the executable's version resource.
fn file_description(path: &str) -> Option<String> {
    let wpath = crate::util::wide(path);
    unsafe {
        let size = GetFileVersionInfoSizeW(windows::core::PCWSTR(wpath.as_ptr()), None);
        if size == 0 || size > 4 * 1024 * 1024 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        GetFileVersionInfoW(windows::core::PCWSTR(wpath.as_ptr()), None, size, data.as_mut_ptr().cast()).ok()?;

        // First translation (language, code page).
        let mut ptr: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut len = 0u32;
        let q = crate::util::wide("\\VarFileInfo\\Translation");
        let (lang, cp) = if VerQueryValueW(data.as_ptr().cast(), windows::core::PCWSTR(q.as_ptr()), &mut ptr, &mut len)
            .as_bool()
            && len >= 4
            && !ptr.is_null()
        {
            let words = std::slice::from_raw_parts(ptr as *const u16, 2);
            (words[0], words[1])
        } else {
            (0x0409, 0x04B0)
        };
        for key in ["FileDescription", "ProductName"] {
            let q = crate::util::wide(&format!("\\StringFileInfo\\{lang:04x}{cp:04x}\\{key}"));
            let mut p: *mut core::ffi::c_void = std::ptr::null_mut();
            let mut n = 0u32;
            if VerQueryValueW(data.as_ptr().cast(), windows::core::PCWSTR(q.as_ptr()), &mut p, &mut n).as_bool()
                && n > 1
                && !p.is_null()
            {
                let s = std::slice::from_raw_parts(p as *const u16, n as usize);
                let v = crate::util::from_wide(s);
                if !v.trim().is_empty() {
                    return Some(v.trim().to_string());
                }
            }
        }
        None
    }
}
