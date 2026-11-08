//! `veil_core::Platform` for Linux.

use crate::atspi::{self, Atspi};
use crate::clipboard::Clipboard;
use crate::dialog;
use crate::input::{self, Input, PointerSource};
use crate::translate::Accumulator;
use crate::x11::X11;
use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use veil_core::input::{HookVerdict, InputEvent, Replay};
use veil_core::platform::{Choice, InputHandler, Platform, Prompt};
use veil_core::types::*;

const RESCAN_EVERY: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Session {
    X11,
    Wayland,
}

impl Session {
    pub fn detect() -> Session {
        let ty = std::env::var("XDG_SESSION_TYPE").unwrap_or_default().to_ascii_lowercase();
        if ty == "wayland" || (ty.is_empty() && std::env::var_os("WAYLAND_DISPLAY").is_some()) {
            Session::Wayland
        } else {
            Session::X11
        }
    }
}

pub struct LinuxPlatform {
    session: Session,
    x11: Option<Arc<X11>>,
    atspi: Mutex<Option<Arc<Atspi>>>,
    clipboard: Clipboard,
    input: Mutex<Option<Arc<Input>>>,
    stop: Arc<AtomicBool>,
    /// Last left-button press (drag data must be newer than this).
    last_mouse_down: Arc<Mutex<Option<Instant>>>,
    a11y_enabled_once: AtomicBool,
}

/// Records mouse-down times, then hands the event to the core.
struct Recorder {
    inner: Arc<dyn InputHandler>,
    last_mouse_down: Arc<Mutex<Option<Instant>>>,
}

impl InputHandler for Recorder {
    fn on_input(&self, ev: &InputEvent) -> HookVerdict {
        if matches!(ev, InputEvent::MouseDown { .. }) {
            *self.last_mouse_down.lock() = Some(Instant::now());
        }
        self.inner.on_input(ev)
    }
}

impl LinuxPlatform {
    pub fn new() -> anyhow::Result<LinuxPlatform> {
        let mut session = Session::detect();
        let x11 = if session == Session::X11 {
            match X11::connect() {
                Ok(x) => Some(x),
                Err(e) => {
                    tracing::warn!("X11 unavailable ({e}); falling back to Wayland behaviour");
                    session = Session::Wayland;
                    None
                }
            }
        } else {
            None
        };
        let atspi = match Atspi::connect() {
            Ok(a) => Some(a),
            Err(e) => {
                tracing::warn!("AT-SPI unavailable: {e}");
                None
            }
        };
        Ok(LinuxPlatform {
            session,
            x11,
            atspi: Mutex::new(atspi),
            clipboard: Clipboard::default(),
            input: Mutex::new(None),
            stop: Arc::new(AtomicBool::new(false)),
            last_mouse_down: Arc::new(Mutex::new(None)),
            a11y_enabled_once: AtomicBool::new(false),
        })
    }

    fn atspi(&self) -> Option<Arc<Atspi>> {
        self.atspi.lock().clone()
    }

    fn input(&self) -> Option<Arc<Input>> {
        self.input.lock().clone()
    }

    fn with_url(&self, mut app: AppContext) -> AppContext {
        if app.url.is_none() {
            if let Some(a) = self.atspi() {
                app.url = a.url_for(app.pid);
            }
        }
        app
    }

    fn screen_size(&self) -> (f64, f64) {
        if let Some(x) = &self.x11 {
            return x.screen;
        }
        // Wayland: no global coordinates; a large virtual screen keeps the accumulator unclamped.
        (7680.0, 4320.0)
    }
}

impl Platform for LinuxPlatform {
    fn name(&self) -> &'static str {
        match self.session {
            Session::X11 => "linux-x11",
            Session::Wayland => "linux-wayland",
        }
    }

    fn run(self: Arc<Self>, handler: Arc<dyn InputHandler>) -> anyhow::Result<()> {
        if let Some(x) = &self.x11 {
            x.spawn_watcher(Arc::new(|app: &AppContext| tracing::debug!("focus: {}", app.display())));
        }
        if let Some(a) = self.atspi() {
            a.spawn_listener();
        }
        if !self.a11y_enabled_once.swap(true, Ordering::Relaxed) {
            if let Err(e) = atspi::set_enabled(true) {
                tracing::debug!("could not enable toolkit accessibility: {e}");
            }
        }

        let pointer = match &self.x11 {
            Some(x) => PointerSource::X11(Arc::clone(x)),
            None => {
                let (w, h) = self.screen_size();
                PointerSource::Accumulated(Mutex::new(Accumulator::new(w, h)))
            }
        };
        let recorder: Arc<dyn InputHandler> =
            Arc::new(Recorder { inner: handler, last_mouse_down: Arc::clone(&self.last_mouse_down) });
        let input = Input::new(recorder, pointer, Arc::clone(&self.stop))?;
        *self.input.lock() = Some(Arc::clone(&input));

        if input.scan() == 0 {
            let detail = input.last_error.lock().clone().unwrap_or_else(|| "no keyboards or mice found".into());
            tracing::warn!("Veil is not intercepting input yet: {detail}");
        }

        let mut last_scan = Instant::now();
        while !self.stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(200));
            if last_scan.elapsed() >= RESCAN_EVERY {
                input.scan();
                last_scan = Instant::now();
            }
            // AT-SPI may come up after us (session start): retry the connection.
            if self.atspi.lock().is_none() {
                if let Ok(a) = Atspi::connect() {
                    a.spawn_listener();
                    *self.atspi.lock() = Some(a);
                }
            }
        }
        // Device threads see the flag, ungrab and exit.
        std::thread::sleep(Duration::from_millis(300));
        Ok(())
    }

    fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    fn replay(&self, replay: &Replay) {
        match self.input() {
            Some(i) => i.replay(replay),
            None => tracing::warn!("replay before input started: {replay:?}"),
        }
    }

    fn foreground(&self) -> Option<AppContext> {
        match &self.x11 {
            Some(x) => x.foreground().map(|a| self.with_url(a)),
            None => self.atspi().and_then(|a| a.focused_app()),
        }
    }

    fn app_at(&self, at: Point) -> Option<AppContext> {
        match &self.x11 {
            Some(x) => x.app_at(at).map(|a| self.with_url(a)),
            None => self.foreground(),
        }
    }

    fn element_at(&self, at: Point) -> Option<UiElement> {
        let pid = self.app_at(at)?.pid;
        self.atspi()?.element_at(pid, at)
    }

    fn focused_text(&self, app: &AppContext) -> FocusedText {
        match self.atspi() {
            Some(a) => a.focused_text(app.pid),
            None => FocusedText::NotExposed { role: "unknown".into() },
        }
    }

    fn enable_accessibility(&self, _app: &AppContext) {
        if !self.a11y_enabled_once.swap(true, Ordering::Relaxed) {
            let _ = atspi::set_enabled(true);
        }
    }

    fn replace_composer_text(&self, app: &AppContext, text: &str) -> bool {
        self.atspi().is_some_and(|a| a.replace_focused_text(app.pid, text))
    }

    fn clipboard(&self) -> ClipboardContent {
        self.clipboard.read()
    }

    fn set_clipboard(&self, content: &ClipboardContent) -> bool {
        self.clipboard.write(content)
    }

    fn drag_session(&self) -> Option<DragSession> {
        let x = self.x11.as_ref()?;
        let down = (*self.last_mouse_down.lock())?;
        let changed = (*x.watched.xdnd_changed.lock())?;
        // Only data of a drag that started after the last press (never a stale selection).
        if changed < down {
            return None;
        }
        x.xdnd_session()
    }

    fn drop_strategy(&self) -> DropStrategy {
        match self.session {
            Session::X11 => DropStrategy::SwallowAndCancel,
            // Wayland compositors own drag-and-drop; a client can't cancel another client's drop.
            Session::Wayland => DropStrategy::Unsupported,
        }
    }

    fn cancel_drop(&self, _at: Point) {
        if let Some(i) = self.input() {
            i.cancel_drop();
        }
    }

    fn picker_selection(&self, app: &AppContext) -> Vec<PathBuf> {
        self.atspi().map(|a| a.picker_selection(app.pid)).unwrap_or_default()
    }

    fn ask(&self, prompt: &Prompt) -> Choice {
        dialog::ask(prompt)
    }

    fn banner(&self, text: &str) {
        dialog::notify(text);
    }

    fn refocus(&self, app: &AppContext) {
        if let Some(x) = &self.x11 {
            if let Some(w) = x.window_for_pid(app.pid) {
                x.activate(w);
                std::thread::sleep(Duration::from_millis(60));
            }
        }
    }

    fn permissions(&self) -> Vec<Permission> {
        let (devices_ok, detail) = input::devices_accessible();
        let a11y_ok = atspi::bus_reachable() && atspi::is_enabled().unwrap_or(false);
        vec![
            Permission {
                id: "input_devices".into(),
                title: "Keyboard and mouse access".into(),
                granted: devices_ok,
                help: format!(
                    "Veil reads input devices and creates a virtual one so it can hold a message before it is sent. \
                     Run once: `sudo groupadd -f veil && sudo usermod -aG veil $USER && sudo cp packaging/linux/99-veil.rules /etc/udev/rules.d/ && sudo udevadm control --reload && sudo udevadm trigger`, then log out and back in. ({detail})"
                ),
            },
            Permission {
                id: "accessibility".into(),
                title: "Accessibility (AT-SPI)".into(),
                granted: a11y_ok,
                help: "Lets Veil read the message you're about to send and find Send buttons. Veil turns it on for your \
                       session; on GNOME you can also run `gsettings set org.gnome.desktop.interface toolkit-accessibility true`. \
                       Restart Chrome/Electron apps afterwards."
                    .into(),
            },
        ]
    }

    fn request_permission(&self, id: &str) -> bool {
        match id {
            "accessibility" => {
                if let Err(e) = atspi::set_enabled(true) {
                    tracing::warn!("enabling AT-SPI failed: {e}");
                }
                if dialog::find_program("gsettings").is_some() {
                    let _ = std::process::Command::new("gsettings")
                        .args(["set", "org.gnome.desktop.interface", "toolkit-accessibility", "true"])
                        .status();
                }
                if self.atspi.lock().is_none() {
                    if let Ok(a) = Atspi::connect() {
                        a.spawn_listener();
                        *self.atspi.lock() = Some(a);
                    }
                }
                atspi::is_enabled().unwrap_or(false)
            }
            "input_devices" => {
                tracing::info!(
                    "input access needs a one-time setup: sudo groupadd -f veil; sudo usermod -aG veil $USER; \
                     sudo cp packaging/linux/99-veil.rules /etc/udev/rules.d/; sudo udevadm control --reload; \
                     sudo udevadm trigger; then log out and back in"
                );
                let ok = input::devices_accessible().0;
                if ok {
                    if let Some(i) = self.input() {
                        i.scan();
                    }
                }
                ok
            }
            _ => false,
        }
    }
}
