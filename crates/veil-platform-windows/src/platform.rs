//! `veil_core::Platform` for Windows.

use crate::apps::Apps;
use crate::com::ComWorker;
use crate::keys::ModTracker;
use crate::ui::UiThread;
use crate::util::{class_name, hwnd, is_file_dialog, is_web_class, root_at, window_pid};
use crate::{clipboard, hooks, inject, ocr, shell, ui, uia};
use parking_lot::{Mutex, RwLock};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use veil_core::*;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
use windows::Win32::System::Threading::{GetCurrentProcess, GetCurrentProcessId, OpenProcessToken};
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, IsWindowVisible};

/// Hook-path budget for UI Automation hit-testing on mouse-down.
const ELEMENT_AT_TIMEOUT: Duration = Duration::from_millis(60);
const FOCUSED_TEXT_TIMEOUT: Duration = Duration::from_millis(400);
const SET_VALUE_TIMEOUT: Duration = Duration::from_millis(500);
const PICKER_TIMEOUT: Duration = Duration::from_millis(1500);
const SELECTION_TIMEOUT: Duration = Duration::from_millis(1500);
/// Time for the OLE drag loop to see Escape before the button state is resynchronised.
const CANCEL_SETTLE: Duration = Duration::from_millis(120);
/// Time for the target window to accept focus before replays (see the PoC lesson L12).
const REFOCUS_SETTLE: Duration = Duration::from_millis(120);

/// Hook-thread state shared with the platform object.
pub(crate) static INNER: OnceLock<Arc<Inner>> = OnceLock::new();

#[derive(Default)]
pub(crate) struct MouseState {
    pub left_down: bool,
    pub last_down_time: u32,
    pub last_down_pt: (i32, i32),
    pub clicks: u32,
    /// Increments on every real left-button press.
    pub down_seq: u64,
    pub down_at: Point,
    /// Top-level window under the last press (the drag source).
    pub down_root: isize,
}

pub(crate) struct Inner {
    pub handler: RwLock<Option<Arc<dyn InputHandler>>>,
    /// Modifiers as physically pressed (every real event).
    pub physical: ModTracker,
    /// Modifiers as the system believes them (delivered real events + Veil's own injections).
    pub logical: ModTracker,
    pub mouse: Mutex<MouseState>,
    pub hook_thread: AtomicU32,
    pub stopping: AtomicBool,
    pub apps: Apps,
    pub fast: Arc<ComWorker>,
    pub slow: Arc<ComWorker>,
    pub ui: UiThread,
    /// Drag data resolved for a given press (`down_seq`).
    pub drag_cache: Mutex<Option<(u64, Option<DragSession>)>>,
    pub woken: Mutex<HashSet<u32>>,
    pub own_pid: u32,
}

pub struct WindowsPlatform {
    inner: Arc<Inner>,
}

impl WindowsPlatform {
    pub fn new() -> anyhow::Result<Arc<WindowsPlatform>> {
        if let Some(inner) = INNER.get() {
            return Ok(Arc::new(WindowsPlatform { inner: inner.clone() }));
        }
        unsafe {
            // Hook coordinates, SetCursorPos and UIA hit-testing all in physical pixels.
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }
        let fast = ComWorker::spawn("fast")?;
        let slow = ComWorker::spawn("slow")?;
        let ui = UiThread::spawn()?;
        let inner = Arc::new(Inner {
            handler: RwLock::new(None),
            physical: ModTracker::default(),
            logical: ModTracker::default(),
            mouse: Mutex::new(MouseState::default()),
            hook_thread: AtomicU32::new(0),
            stopping: AtomicBool::new(false),
            apps: Apps::new(slow.clone()),
            fast,
            slow,
            ui,
            drag_cache: Mutex::new(None),
            woken: Mutex::new(HashSet::new()),
            own_pid: unsafe { GetCurrentProcessId() },
        });
        let inner = INNER.get_or_init(|| inner).clone();
        Ok(Arc::new(WindowsPlatform { inner }))
    }

    /// A visible file dialog (`#32770` with the file-name control) of the process, preferring the
    /// foreground window.
    fn file_dialog_of(&self, pid: u32) -> Option<isize> {
        let fg = self.inner.apps.foreground_hwnd();
        if fg != 0 && window_pid(hwnd(fg)).0 == pid && is_file_dialog(hwnd(fg)) {
            return Some(fg);
        }
        struct Search {
            pid: u32,
            found: isize,
        }
        unsafe extern "system" fn each(
            h: windows::Win32::Foundation::HWND,
            lp: windows::Win32::Foundation::LPARAM,
        ) -> windows::core::BOOL {
            let s = unsafe { &mut *(lp.0 as *mut Search) };
            if window_pid(h).0 == s.pid && unsafe { IsWindowVisible(h) }.as_bool() && is_file_dialog(h) {
                s.found = h.0 as isize;
                return false.into();
            }
            true.into()
        }
        let mut s = Search { pid, found: 0 };
        unsafe {
            let _ = EnumWindows(Some(each), windows::Win32::Foundation::LPARAM(&mut s as *mut Search as isize));
        }
        (s.found != 0).then_some(s.found)
    }

    fn is_elevated() -> bool {
        unsafe {
            let mut token = HANDLE::default();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
                return false;
            }
            let mut elevation = TOKEN_ELEVATION::default();
            let mut len = 0u32;
            let ok = GetTokenInformation(
                token,
                TokenElevation,
                Some(&mut elevation as *mut TOKEN_ELEVATION as *mut _),
                std::mem::size_of::<TOKEN_ELEVATION>() as u32,
                &mut len,
            )
            .is_ok();
            let _ = windows::Win32::Foundation::CloseHandle(token);
            ok && elevation.TokenIsElevated != 0
        }
    }
}

impl Platform for WindowsPlatform {
    fn name(&self) -> &'static str {
        "windows"
    }

    fn run(self: Arc<Self>, handler: Arc<dyn InputHandler>) -> anyhow::Result<()> {
        *self.inner.handler.write() = Some(handler);
        let result = hooks::run_loop(&self.inner);
        *self.inner.handler.write() = None;
        result
    }

    fn stop(&self) {
        hooks::stop(&self.inner);
    }

    fn replay(&self, replay: &Replay) {
        inject::replay(replay, &self.inner.logical);
    }

    fn foreground(&self) -> Option<AppContext> {
        self.inner.apps.foreground()
    }

    fn app_at(&self, at: Point) -> Option<AppContext> {
        self.inner.apps.app_at(at.x.round() as i32, at.y.round() as i32)
    }

    fn element_at(&self, at: Point) -> Option<UiElement> {
        let (x, y) = (at.x.round() as i32, at.y.round() as i32);
        let in_dialog = is_file_dialog(root_at(x, y));
        self.inner.fast.call_if_idle(ELEMENT_AT_TIMEOUT, move |ctx| uia::element_at(ctx, x, y, in_dialog))
    }

    fn focused_text(&self, _app: &AppContext) -> FocusedText {
        let own = self.inner.own_pid;
        self.inner
            .slow
            .call(FOCUSED_TEXT_TIMEOUT, move |ctx| Some(uia::focused_text(ctx, own)))
            .unwrap_or(FocusedText::NotExposed { role: "timeout".into() })
    }

    fn enable_accessibility(&self, app: &AppContext) {
        if !self.inner.woken.lock().insert(app.pid) {
            return;
        }
        if let Some(h) = self.inner.apps.window_of(app.pid) {
            self.inner.slow.spawn_job(move |ctx| uia::wake(ctx, h));
        }
    }

    fn replace_composer_text(&self, app: &AppContext, text: &str) -> bool {
        // Web composers (Chromium, Electron, Firefox) keep their own state; setting the accessibility
        // value would show text the app never sends. The core falls back to select-all + paste.
        let class = self.inner.apps.window_of(app.pid).map(|h| class_name(hwnd(h))).unwrap_or_default();
        if is_web_class(&class) || app.url.is_some() {
            return false;
        }
        let text = text.to_string();
        self.inner.slow.call(SET_VALUE_TIMEOUT, move |ctx| uia::set_focused_value(ctx, &text)).unwrap_or(false)
    }

    fn clipboard(&self) -> ClipboardContent {
        clipboard::read()
    }

    fn set_clipboard(&self, content: &ClipboardContent) -> bool {
        clipboard::write(self.inner.ui.owner(), content)
    }

    fn drag_session(&self) -> Option<DragSession> {
        let (seq, root) = {
            let m = self.inner.mouse.lock();
            (m.down_seq, m.down_root)
        };
        if let Some((cached_seq, session)) = self.inner.drag_cache.lock().as_ref() {
            if *cached_seq == seq {
                return session.clone();
            }
        }
        let class = class_name(hwnd(root));
        let desktop = class == "Progman" || class == "WorkerW";
        let explorer = class == "CabinetWClass" || class == "ExploreWClass";
        if !desktop && !explorer {
            // Drags from other apps (browser downloads, mail attachments…) can't be read from outside.
            *self.inner.drag_cache.lock() = Some((seq, None));
            return None;
        }
        let files = self.inner.slow.call(SELECTION_TIMEOUT, move |_ctx| Some(shell::selection(root, desktop)));
        let Some(files) = files else {
            // Timed out: don't cache, the engine retries.
            return None;
        };
        let session = (!files.is_empty()).then_some(DragSession { files, text: None, has_image: false });
        *self.inner.drag_cache.lock() = Some((seq, session.clone()));
        session
    }

    fn drop_strategy(&self) -> DropStrategy {
        DropStrategy::SwallowAndCancel
    }

    /// The button-up was swallowed, so the OLE `DoDragDrop` loop in the source is still running. Escape
    /// makes it return `DRAGDROP_S_CANCEL`; then a tagged button-up at the drag origin resynchronises
    /// the button state. If Escape was not processed in time, that release lands on the source itself,
    /// which is a no-op (dropping a file onto its own folder). The engine's backstop covers the rest.
    fn cancel_drop(&self, at: Point) {
        let origin = self.inner.mouse.lock().down_at;
        inject::escape();
        std::thread::sleep(CANCEL_SETTLE);
        inject::left_up_at(origin);
        std::thread::sleep(Duration::from_millis(30));
        inject::move_cursor(at);
    }

    fn picker_selection(&self, app: &AppContext) -> Vec<PathBuf> {
        let Some(dialog) = self.file_dialog_of(app.pid) else { return Vec::new() };
        self.inner.slow.call(PICKER_TIMEOUT, move |ctx| Some(uia::picker_selection(ctx, dialog))).unwrap_or_default()
    }

    fn ocr(&self, image: &[u8]) -> Option<String> {
        ocr::recognize(image)
    }

    fn ask(&self, prompt: &Prompt) -> Choice {
        ui::ask(prompt)
    }

    fn banner(&self, text: &str) {
        self.inner.ui.banner(text);
    }

    fn refocus(&self, app: &AppContext) {
        if let Some(h) = self.inner.apps.window_of(app.pid) {
            ui::force_foreground(hwnd(h));
            std::thread::sleep(REFOCUS_SETTLE);
        }
    }

    fn permissions(&self) -> Vec<Permission> {
        let mut list = vec![Permission {
            id: "elevated_apps".into(),
            title: "Protect apps running as administrator".into(),
            granted: Self::is_elevated(),
            help: "Optional. Windows keeps a normal app from watching windows of apps started with \"Run as \
                   administrator\". To protect those too, run Veil elevated or install the signed build with UI \
                   Access."
                .into(),
        }];
        if !ocr::available() {
            list.push(Permission {
                id: "ocr_language".into(),
                title: "Text recognition for images".into(),
                granted: false,
                help: "Optional. Add an OCR language in Settings › Time & language › Language & region (language \
                       options › Optical character recognition) so Veil can read text in pasted images."
                    .into(),
            });
        }
        list
    }

    fn request_permission(&self, _id: &str) -> bool {
        false
    }
}
