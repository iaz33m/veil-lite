//! Running applications, the on-screen window list and browser URLs.

use crate::ax::Element;
use crate::ffi::{
    cf_array_items, cf_dict_get, cf_f64, cf_i64, kCGNullWindowID, kCGWindowListExcludeDesktopElements,
    kCGWindowListOptionOnScreenOnly, CGRect, CGRectMakeWithDictionaryRepresentation, CGWindowListCopyWindowInfo,
    OwnedCf,
};
use core_foundation_sys::base::CFTypeRef;
use objc2::rc::autoreleasepool;
use objc2_app_kit::{NSRunningApplication, NSWorkspace};
use objc2_foundation::NSString;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Out-of-process host of NSOpenPanel / NSSavePanel (macOS 10.15+ for sandboxed and many other apps).
pub const PANEL_SERVICE: &str = "com.apple.appkit.xpc.openAndSavePanelService";

pub const CHROMIUM_BROWSERS: &[&str] = &[
    "com.google.Chrome",
    "com.google.Chrome.beta",
    "com.google.Chrome.dev",
    "com.google.Chrome.canary",
    "org.chromium.Chromium",
    "com.microsoft.edgemac",
    "com.microsoft.edgemac.Beta",
    "com.brave.Browser",
    "company.thebrowser.Browser",
    "company.thebrowser.dia",
    "com.operasoftware.Opera",
    "ai.perplexity.comet",
    "com.vivaldi.Vivaldi",
];

pub const OTHER_BROWSERS: &[&str] = &[
    "com.apple.Safari",
    "com.apple.SafariTechnologyPreview",
    "org.mozilla.firefox",
    "org.mozilla.firefoxdeveloperedition",
    "app.zen-browser.zen",
];

pub fn is_browser(bundle_id: &str) -> bool {
    CHROMIUM_BROWSERS.contains(&bundle_id) || OTHER_BROWSERS.contains(&bundle_id)
}

#[derive(Clone, Debug)]
pub struct AppInfo {
    pub pid: u32,
    pub bundle_id: Option<String>,
    pub name: String,
    pub exe: Option<String>,
    seen: Instant,
}

impl AppInfo {
    /// Stable identifier: bundle id, else executable name.
    pub fn app_id(&self) -> String {
        self.bundle_id.clone().or_else(|| self.exe.clone()).unwrap_or_else(|| format!("pid:{}", self.pid))
    }

    pub fn is_browser(&self) -> bool {
        self.bundle_id.as_deref().is_some_and(is_browser)
    }

    pub fn is_panel_service(&self) -> bool {
        self.bundle_id.as_deref() == Some(PANEL_SERVICE)
    }
}

fn info_from(app: &NSRunningApplication) -> AppInfo {
    let pid = app.processIdentifier() as u32;
    let bundle_id = app.bundleIdentifier().map(|s| s.to_string());
    let exe = app.executableURL().and_then(|u| u.lastPathComponent()).map(|s| s.to_string());
    let name = app
        .localizedName()
        .map(|s| s.to_string())
        .or_else(|| exe.clone())
        .or_else(|| bundle_id.clone())
        .unwrap_or_else(|| "App".into());
    AppInfo { pid, bundle_id, name, exe, seen: Instant::now() }
}

pub fn lookup(pid: u32) -> Option<AppInfo> {
    autoreleasepool(|_| {
        NSRunningApplication::runningApplicationWithProcessIdentifier(pid as i32).map(|a| info_from(&a))
    })
}

/// The frontmost (active) application.
pub fn frontmost() -> Option<AppInfo> {
    autoreleasepool(|_| NSWorkspace::sharedWorkspace().frontmostApplication().map(|a| info_from(&a)))
}

pub fn frontmost_pid() -> Option<u32> {
    autoreleasepool(|_| NSWorkspace::sharedWorkspace().frontmostApplication().map(|a| a.processIdentifier() as u32))
}

pub fn pids_with_bundle(bundle_id: &str) -> Vec<u32> {
    autoreleasepool(|_| {
        let id = NSString::from_str(bundle_id);
        let apps = NSRunningApplication::runningApplicationsWithBundleIdentifier(&id);
        apps.iter().map(|a| a.processIdentifier() as u32).collect()
    })
}

/// pid → app info, refreshed every 30 s (pids are rarely reused that fast).
#[derive(Default)]
pub struct AppCache {
    map: Mutex<HashMap<u32, AppInfo>>,
}

impl AppCache {
    pub fn get(&self, pid: u32) -> Option<AppInfo> {
        if let Some(i) = self.map.lock().get(&pid) {
            if i.seen.elapsed() < Duration::from_secs(30) {
                return Some(i.clone());
            }
        }
        let info = lookup(pid)?;
        let mut map = self.map.lock();
        if map.len() > 512 {
            map.retain(|_, v| v.seen.elapsed() < Duration::from_secs(300));
        }
        map.insert(pid, info.clone());
        Some(info)
    }

    pub fn put(&self, info: &AppInfo) {
        self.map.lock().insert(info.pid, info.clone());
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Win {
    pub pid: u32,
    pub bounds: CGRect,
    pub layer: i64,
    pub alpha: f64,
}

/// The on-screen window list, front to back, cached briefly (it is read on every drag move).
pub struct WindowCache {
    inner: Mutex<(Option<Instant>, Vec<Win>)>,
}

impl Default for WindowCache {
    fn default() -> Self {
        WindowCache { inner: Mutex::new((None, Vec::new())) }
    }
}

impl WindowCache {
    pub fn windows(&self, max_age: Duration) -> Vec<Win> {
        let mut g = self.inner.lock();
        let fresh = g.0.is_some_and(|t| t.elapsed() < max_age);
        if !fresh {
            g.1 = snapshot_windows();
            g.0 = Some(Instant::now());
        }
        g.1.clone()
    }

    /// Owner of the top-most normal (layer 0), visible window under the point. Skips drag images, menus,
    /// the Dock, alerts and Veil's own banner.
    pub fn owner_at(&self, x: f64, y: f64, max_age: Duration) -> Option<u32> {
        self.windows(max_age)
            .into_iter()
            .find(|w| w.layer == 0 && w.alpha > 0.05 && w.bounds.contains(x, y))
            .map(|w| w.pid)
    }
}

fn snapshot_windows() -> Vec<Win> {
    let list = unsafe {
        CGWindowListCopyWindowInfo(
            kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
            kCGNullWindowID,
        )
    };
    let Some(list) = OwnedCf::from_create(list as CFTypeRef) else { return Vec::new() };
    let mut out = Vec::new();
    for dict in cf_array_items(list.as_ptr()) {
        let pid = cf_i64(cf_dict_get(dict, "kCGWindowOwnerPID"));
        let layer = cf_i64(cf_dict_get(dict, "kCGWindowLayer")).unwrap_or(0);
        let alpha = cf_f64(cf_dict_get(dict, "kCGWindowAlpha")).unwrap_or(1.0);
        let bounds_dict = cf_dict_get(dict, "kCGWindowBounds");
        let mut bounds = CGRect::default();
        let ok =
            !bounds_dict.is_null() && unsafe { CGRectMakeWithDictionaryRepresentation(bounds_dict as _, &mut bounds) };
        if let (Some(pid), true) = (pid, ok) {
            out.push(Win { pid: pid as u32, bounds, layer, alpha });
        }
    }
    out
}

/// URL shown by a browser window. `cache` keeps the web-area element between calls so the expensive
/// tree search runs only when the page/window changes.
pub fn browser_url(pid: u32, cache: &mut Option<Element>) -> Option<String> {
    if let Some(wa) = cache.as_ref() {
        if let Some(u) = wa.url("AXURL").filter(|u| !u.is_empty()) {
            return Some(u);
        }
        *cache = None;
    }
    let app = Element::application(pid, 0.3);
    let win = app.element("AXFocusedWindow")?;
    if let Some(wa) = win.find(600, 18, &mut |e| e.role() == "AXWebArea") {
        if let Some(u) = wa.url("AXURL").filter(|u| !u.is_empty()) {
            *cache = Some(wa);
            return Some(u);
        }
    }
    // Chromium's address bar ("Address and search bar").
    let bar = win.find(400, 12, &mut |e| {
        e.role() == "AXTextField" && {
            let l = e.label();
            l.contains("address") || l.contains("location") || l.contains("search bar") || l.contains("url")
        }
    })?;
    let v = bar.string("AXValue")?.trim().to_string();
    if v.is_empty() || v.contains(' ') {
        return None;
    }
    Some(if v.contains("://") { v } else { format!("https://{v}") })
}
