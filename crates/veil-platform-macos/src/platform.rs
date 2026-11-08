//! `MacPlatform`: the `veil_core::Platform` implementation.

use crate::apps::{self, AppCache, WindowCache, CHROMIUM_BROWSERS};
use crate::ax::{self, Element};
use crate::pasteboard::{self, Items};
use crate::picker::{self, ENTRY_ROLES};
use crate::tap::{Injector, Tap, TapContext};
use crate::{ocr, ui};
use anyhow::anyhow;
use core_foundation_sys::runloop::{kCFRunLoopDefaultMode, CFRunLoopRunInMode};
use dispatch2::DispatchQueue;
use objc2::rc::autoreleasepool;
use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSEvent, NSEventModifierFlags, NSEventType, NSWorkspace,
};
use objc2_foundation::{NSPoint, NSString, NSURL};
use parking_lot::Mutex;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use veil_core::input::Replay;
use veil_core::platform::{Choice, InputHandler, Platform, Prompt};
use veil_core::types::*;

/// AX messaging timeout for hit-testing under the pointer (also the global default for elements that
/// don't set their own).
const HIT_TEST_TIMEOUT: f32 = 0.15;
/// Upper bound for the work done in the tap on a mouse-down.
const HIT_TEST_BUDGET: Duration = Duration::from_millis(120);
/// The window list is re-read at most this often (drag moves query it continuously).
const WINDOW_LIST_MAX_AGE: Duration = Duration::from_millis(150);
const REFRESH_EVERY: Duration = Duration::from_millis(400);

const GENERIC_ROLES: &[&str] = &["AXWebArea", "AXGroup", "AXUnknown", "AXScrollArea"];

const ACCESSIBILITY_SETTINGS: &str = "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility";

/// Title and URL of the frontmost app, refreshed off the hook thread.
#[derive(Default)]
struct FrontDetails {
    pid: u32,
    title: Option<String>,
    url: Option<String>,
}

#[derive(Default)]
struct DragState {
    /// Drag pasteboard change count at the last mouse-down: a drag-and-drop session exists only once it
    /// changes, so data from an earlier drag is never reported.
    count_at_down: isize,
    /// The drag data while it is withheld.
    saved: Option<Items>,
    withheld: bool,
}

pub struct MacPlatform {
    injector: Arc<Injector>,
    apps: AppCache,
    windows: WindowCache,
    system: Element,
    front: Mutex<FrontDetails>,
    clipboard_snapshot: Mutex<Option<(ClipboardContent, Items)>>,
    drag: Mutex<DragState>,
    ax_enabled: Mutex<HashSet<u32>>,
    stopped: AtomicBool,
    own_pid: u32,
}

impl MacPlatform {
    pub fn new() -> anyhow::Result<MacPlatform> {
        let system = Element::system_wide(HIT_TEST_TIMEOUT);
        Ok(MacPlatform {
            injector: Arc::new(Injector::new()),
            apps: AppCache::default(),
            windows: WindowCache::default(),
            system,
            front: Mutex::new(FrontDetails::default()),
            clipboard_snapshot: Mutex::new(None),
            drag: Mutex::new(DragState::default()),
            ax_enabled: Mutex::new(HashSet::new()),
            stopped: AtomicBool::new(false),
            own_pid: std::process::id(),
        })
    }

    /// App context for a pid. The open/save panel service is mapped back to the app it serves.
    fn context(&self, pid: u32) -> Option<AppContext> {
        if pid == 0 || pid == self.own_pid {
            return None;
        }
        let info = self.apps.get(pid)?;
        if info.is_panel_service() {
            let host = apps::frontmost_pid().filter(|p| *p != pid && *p != self.own_pid)?;
            let host_info = self.apps.get(host)?;
            if host_info.is_panel_service() {
                return None;
            }
            return self.context(host);
        }
        let (title, url) = {
            let f = self.front.lock();
            if f.pid == pid {
                (f.title.clone(), f.url.clone())
            } else {
                (None, None)
            }
        };
        Some(AppContext { app_id: info.app_id(), name: info.name.clone(), pid, window_title: title, url })
    }

    /// Ask Chromium / Electron to build their accessibility tree (once per process).
    fn enable_ax(&self, pid: u32, bundle_id: Option<&str>) {
        if pid == self.own_pid || !self.ax_enabled.lock().insert(pid) {
            return;
        }
        let app = Element::application(pid, 0.3);
        let mut ok = app.set_bool("AXManualAccessibility", true);
        let mut how = "AXManualAccessibility";
        if !ok && bundle_id.is_some_and(|b| CHROMIUM_BROWSERS.contains(&b)) {
            ok = app.set_bool("AXEnhancedUserInterface", true);
            how = "AXEnhancedUserInterface";
        }
        tracing::debug!(pid, bundle_id, how, ok, "requested accessibility tree");
    }

    /// Called from the tap on every mouse-down (main thread).
    fn note_mouse_down(&self) {
        let count = pasteboard::change_count(&pasteboard::drag());
        let mut st = self.drag.lock();
        st.count_at_down = count;
        st.saved = None;
        st.withheld = false;
    }

    /// Background refresh of the frontmost app's window title and browser URL (AX calls stay off the
    /// hook thread), and accessibility enabling for newly activated apps.
    fn refresh_loop(self: Arc<Self>) {
        let running: Vec<(u32, Option<String>)> = autoreleasepool(|_| {
            NSWorkspace::sharedWorkspace()
                .runningApplications()
                .iter()
                .filter(|a| a.activationPolicy() == NSApplicationActivationPolicy::Regular)
                .map(|a| (a.processIdentifier() as u32, a.bundleIdentifier().map(|s| s.to_string())))
                .collect()
        });
        for (pid, bundle) in running {
            self.enable_ax(pid, bundle.as_deref());
        }

        let mut last_pid = 0;
        let mut web_area: Option<Element> = None;
        while !self.stopped.load(Ordering::SeqCst) {
            std::thread::sleep(REFRESH_EVERY);
            let Some(info) = apps::frontmost() else { continue };
            if info.pid == self.own_pid || info.is_panel_service() {
                continue;
            }
            self.apps.put(&info);
            if info.pid != last_pid {
                last_pid = info.pid;
                web_area = None;
                self.enable_ax(info.pid, info.bundle_id.as_deref());
            }
            let title = ax::window_title(info.pid);
            let url = if info.is_browser() { apps::browser_url(info.pid, &mut web_area) } else { None };
            *self.front.lock() = FrontDetails { pid: info.pid, title, url };
        }
    }

    fn hit_test(&self, at: Point) -> Option<UiElement> {
        let start = Instant::now();
        let hit = self.system.element_at(at.x, at.y)?;
        // An icon or label inside a button: walk up to the button.
        let mut button = None;
        let mut cur = hit.clone();
        for _ in 0..4 {
            if cur.role() == "AXButton" {
                button = Some(cur.clone());
                break;
            }
            if start.elapsed() > HIT_TEST_BUDGET {
                break;
            }
            match cur.parent() {
                Some(p) => cur = p,
                None => break,
            }
        }
        let el = button.unwrap_or(hit);
        let role = el.role();
        let is_button = role == "AXButton";
        let pid = el.pid().unwrap_or(0);
        if start.elapsed() > HIT_TEST_BUDGET {
            return Some(UiElement { role, pid, is_button, ..Default::default() });
        }
        let label = el.label();
        let candidate =
            (is_button && picker::is_pick_label(&el.title())) || (!is_button && ENTRY_ROLES.contains(&role.as_str()));
        let in_file_dialog = candidate && start.elapsed() < HIT_TEST_BUDGET && {
            let from_panel_service = self.apps.get(pid).is_some_and(|i| i.is_panel_service());
            picker::in_file_dialog(&el, from_panel_service)
        };
        let is_generic = !is_button && label.trim().is_empty() && GENERIC_ROLES.contains(&role.as_str());
        Some(UiElement { role, label, pid, is_button, in_file_dialog, is_generic })
    }
}

impl Platform for MacPlatform {
    fn name(&self) -> &'static str {
        "macos"
    }

    fn run(self: Arc<Self>, handler: Arc<dyn InputHandler>) -> anyhow::Result<()> {
        let mtm = MainThreadMarker::new().ok_or_else(|| anyhow!("the macOS provider must run on the main thread"))?;
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

        // The setup wizard asks for Accessibility; keep the run loop alive (banners, IPC-driven work)
        // and install the tap as soon as permission is granted.
        let mut waiting_logged = false;
        while !ax::is_trusted() {
            if self.stopped.load(Ordering::SeqCst) {
                return Ok(());
            }
            if !waiting_logged {
                tracing::warn!(
                    "waiting for Accessibility permission (System Settings › Privacy & Security › Accessibility)"
                );
                waiting_logged = true;
            }
            unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.5, 0) };
        }

        let me = Arc::clone(&self);
        let tap = Tap::install(TapContext {
            handler,
            injector: Arc::clone(&self.injector),
            on_mouse_down: Box::new(move || me.note_mouse_down()),
            port: AtomicPtr::new(std::ptr::null_mut()),
        })?;
        tracing::info!("macOS event tap installed");

        let refresher = Arc::clone(&self);
        std::thread::Builder::new().name("veil-mac-refresh".into()).spawn(move || refresher.refresh_loop())?;

        if !self.stopped.load(Ordering::SeqCst) {
            app.run();
        }
        drop(tap);
        tracing::info!("macOS event loop stopped");
        Ok(())
    }

    fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        DispatchQueue::main().exec_async(|| {
            let Some(mtm) = MainThreadMarker::new() else { return };
            autoreleasepool(|_| {
                let app = NSApplication::sharedApplication(mtm);
                app.stop(None);
                // -stop: takes effect after the next event: post an empty one.
                #[allow(unused_unsafe)]
                let ev = unsafe {
                    NSEvent::otherEventWithType_location_modifierFlags_timestamp_windowNumber_context_subtype_data1_data2(
                        NSEventType::ApplicationDefined,
                        NSPoint::new(0.0, 0.0),
                        NSEventModifierFlags::empty(),
                        0.0,
                        0,
                        None,
                        0,
                        0,
                        0,
                    )
                };
                if let Some(ev) = ev {
                    app.postEvent_atStart(&ev, true);
                }
            });
        });
    }

    fn replay(&self, replay: &Replay) {
        self.injector.replay(replay);
    }

    fn foreground(&self) -> Option<AppContext> {
        let pid = apps::frontmost_pid()?;
        self.context(pid)
    }

    fn app_at(&self, at: Point) -> Option<AppContext> {
        let pid = self.windows.owner_at(at.x, at.y, WINDOW_LIST_MAX_AGE)?;
        self.context(pid)
    }

    fn element_at(&self, at: Point) -> Option<UiElement> {
        self.hit_test(at)
    }

    fn focused_text(&self, app: &AppContext) -> FocusedText {
        if !self.ax_enabled.lock().contains(&app.pid) {
            let bundle = self.apps.get(app.pid).and_then(|i| i.bundle_id);
            self.enable_ax(app.pid, bundle.as_deref());
        }
        let (text, role, exposed) = ax::composer_text(app.pid);
        if exposed {
            FocusedText::Exposed { text, role }
        } else {
            FocusedText::NotExposed { role }
        }
    }

    fn enable_accessibility(&self, app: &AppContext) {
        let bundle = self.apps.get(app.pid).and_then(|i| i.bundle_id);
        self.enable_ax(app.pid, bundle.as_deref());
    }

    fn replace_composer_text(&self, app: &AppContext, text: &str) -> bool {
        let Some(el) = ax::focused_editable(app.pid) else { return false };
        if !el.is_settable("AXValue") || !el.set_string("AXValue", text) {
            return false;
        }
        // Web composers often accept the call but ignore it: verify.
        el.string("AXValue").is_some_and(|v| v.trim() == text.trim())
    }

    fn clipboard(&self) -> ClipboardContent {
        let content = pasteboard::read_clipboard();
        let snapshot = autoreleasepool(|_| pasteboard::snapshot(&pasteboard::general()));
        *self.clipboard_snapshot.lock() = snapshot.map(|items| (content.clone(), items));
        content
    }

    fn set_clipboard(&self, content: &ClipboardContent) -> bool {
        // Restoring what `clipboard()` returned: put back every type (rich text, HTML…), not just the summary.
        let snapshot = self.clipboard_snapshot.lock().clone();
        if let Some((saved, items)) = snapshot {
            if saved == *content {
                autoreleasepool(|_| pasteboard::restore(&pasteboard::general(), &items));
                return true;
            }
        }
        pasteboard::write_clipboard(content)
    }

    fn drag_session(&self) -> Option<DragSession> {
        autoreleasepool(|_| {
            let st = self.drag.lock();
            if st.withheld {
                return st.saved.as_ref().map(pasteboard::session_from_items);
            }
            let pb = pasteboard::drag();
            if pasteboard::change_count(&pb) == st.count_at_down {
                return None;
            }
            drop(st);
            Some(pasteboard::session_from_pasteboard(&pb))
        })
    }

    fn set_drag_withheld(&self, withheld: bool) {
        autoreleasepool(|_| {
            let mut st = self.drag.lock();
            if st.withheld == withheld {
                return;
            }
            let pb = pasteboard::drag();
            if withheld {
                if pasteboard::change_count(&pb) == st.count_at_down {
                    return; // no drag-and-drop session (window move, text selection)
                }
                let Some(items) = pasteboard::snapshot(&pb) else {
                    tracing::warn!("drag data too large to withhold");
                    return;
                };
                pb.clearContents();
                st.saved = Some(items);
                st.withheld = true;
            } else {
                if let Some(items) = st.saved.take() {
                    pasteboard::restore(&pb, &items);
                }
                st.withheld = false;
            }
        })
    }

    fn drop_strategy(&self) -> DropStrategy {
        // Swallowing the button-up does not stop a drop on macOS: the data is withheld instead.
        DropStrategy::Withhold
    }

    fn picker_selection(&self, app: &AppContext) -> Vec<PathBuf> {
        picker::selection(app.pid)
    }

    fn ocr(&self, image: &[u8]) -> Option<String> {
        ocr::recognize(image)
    }

    fn ask(&self, prompt: &Prompt) -> Choice {
        ui::ask(prompt)
    }

    fn banner(&self, text: &str) {
        ui::banner(text);
    }

    fn refocus(&self, app: &AppContext) {
        ui::refocus(app.pid);
    }

    fn permissions(&self) -> Vec<Permission> {
        vec![Permission {
            id: "accessibility".into(),
            title: "Accessibility".into(),
            granted: ax::is_trusted(),
            help: "Lets Veil read what you are about to send and hold it until you decide. Turn Veil on in \
                   System Settings › Privacy & Security › Accessibility."
                .into(),
        }]
    }

    fn request_permission(&self, id: &str) -> bool {
        if id != "accessibility" {
            return false;
        }
        if ax::prompt_for_trust() {
            return true;
        }
        autoreleasepool(|_| {
            if let Some(url) = NSURL::URLWithString(&NSString::from_str(ACCESSIBILITY_SETTINGS)) {
                NSWorkspace::sharedWorkspace().openURL(&url);
            }
        });
        ax::is_trusted()
    }
}
