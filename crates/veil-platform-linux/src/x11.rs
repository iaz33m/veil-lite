//! X11 (also used for XWayland-only bits is avoided: on Wayland sessions this module is not used).
//!
//! * a query connection shared by the hook and worker threads (x11rb's `RustConnection` is `Sync`);
//! * a watcher thread with its own connection that tracks `_NET_ACTIVE_WINDOW`, title changes and
//!   `XdndSelection` ownership changes (XFixes), so `foreground()` never does a round trip;
//! * short-lived connections for selection transfers (they need their own event queue).

use crate::clipboard::parse_uri_list;
use crate::proc;
use parking_lot::{Mutex, RwLock};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use veil_core::types::{AppContext, DragSession, Point};
use x11rb::connection::Connection;
use x11rb::protocol::xfixes::{ConnectionExt as _, SelectionEventMask};
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ChangeWindowAttributesAux, ClientMessageEvent, ConnectionExt as _, CreateWindowAux, EventMask,
    PropMode, Window, WindowClass,
};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::{COPY_DEPTH_FROM_PARENT, CURRENT_TIME, NONE};

x11rb::atom_manager! {
    pub Atoms: AtomsCookie {
        _NET_ACTIVE_WINDOW,
        _NET_WM_PID,
        _NET_WM_NAME,
        WM_STATE,
        WM_CLASS,
        WM_NAME,
        UTF8_STRING,
        XdndSelection,
        TEXT_URI_LIST: b"text/uri-list",
        TEXT_PLAIN_UTF8: b"text/plain;charset=utf-8",
        VEIL_SEL,
    }
}

/// State maintained by the watcher thread.
#[derive(Default)]
pub struct Watched {
    pub foreground: RwLock<Option<(Window, AppContext)>>,
    /// Last time any client took ownership of `XdndSelection` (a drag started).
    pub xdnd_changed: Mutex<Option<Instant>>,
    /// Top-level client window per pid (for refocus).
    pub windows: Mutex<HashMap<u32, Window>>,
}

pub struct X11 {
    conn: RustConnection,
    root: Window,
    atoms: Atoms,
    pub screen: (f64, f64),
    /// Window under the pointer → app, cached briefly (hook path).
    at_cache: Mutex<HashMap<Window, (Instant, Option<AppContext>)>>,
    pub watched: Arc<Watched>,
}

const AT_CACHE_TTL: Duration = Duration::from_millis(150);

impl X11 {
    pub fn connect() -> anyhow::Result<Arc<X11>> {
        let (conn, screen_num) = x11rb::connect(None)?;
        let screen = &conn.setup().roots[screen_num];
        let root = screen.root;
        let size = (screen.width_in_pixels as f64, screen.height_in_pixels as f64);
        let atoms = Atoms::new(&conn)?.reply()?;
        let x = Arc::new(X11 { conn, root, atoms, screen: size, at_cache: Mutex::new(HashMap::new()), watched: Arc::new(Watched::default()) });
        x.refresh_foreground();
        Ok(x)
    }

    fn prop_bytes(&self, w: Window, atom: Atom, ty: impl Into<Atom>) -> Option<Vec<u8>> {
        let r = self.conn.get_property(false, w, atom, ty, 0, 1 << 16).ok()?.reply().ok()?;
        (r.format == 8 && !r.value.is_empty()).then_some(r.value)
    }

    fn prop_u32(&self, w: Window, atom: Atom, ty: impl Into<Atom>) -> Option<u32> {
        let r = self.conn.get_property(false, w, atom, ty, 0, 1).ok()?.reply().ok()?;
        r.value32()?.next()
    }

    pub fn active_window(&self) -> Option<Window> {
        self.prop_u32(self.root, self.atoms._NET_ACTIVE_WINDOW, AtomEnum::WINDOW).filter(|w| *w != NONE)
    }

    pub fn window_pid(&self, w: Window) -> Option<u32> {
        self.prop_u32(w, self.atoms._NET_WM_PID, AtomEnum::CARDINAL).filter(|p| *p != 0)
    }

    /// `(instance, class)` from `WM_CLASS`.
    pub fn wm_class(&self, w: Window) -> Option<(String, String)> {
        let v = self.prop_bytes(w, self.atoms.WM_CLASS, AtomEnum::STRING)?;
        let mut parts = v.split(|b| *b == 0).map(|s| String::from_utf8_lossy(s).into_owned());
        Some((parts.next().unwrap_or_default(), parts.next().unwrap_or_default()))
    }

    pub fn title(&self, w: Window) -> Option<String> {
        self.prop_bytes(w, self.atoms._NET_WM_NAME, self.atoms.UTF8_STRING)
            .or_else(|| self.prop_bytes(w, self.atoms.WM_NAME, AtomEnum::ANY))
            .map(|b| String::from_utf8_lossy(&b).into_owned())
    }

    fn has_wm_state(&self, w: Window) -> bool {
        self.conn
            .get_property(false, w, self.atoms.WM_STATE, AtomEnum::ANY, 0, 0)
            .ok()
            .and_then(|c| c.reply().ok())
            .is_some_and(|r| r.type_ != NONE)
    }

    /// The client window (the one with `WM_STATE`) inside a top-level frame. Breadth-first, bounded.
    pub fn client_window(&self, frame: Window) -> Window {
        if self.has_wm_state(frame) {
            return frame;
        }
        let mut queue = std::collections::VecDeque::from([(frame, 0)]);
        while let Some((w, depth)) = queue.pop_front() {
            if depth > 3 {
                break;
            }
            let Ok(Ok(tree)) = self.conn.query_tree(w).map(|c| c.reply()) else { continue };
            for child in tree.children.iter().rev() {
                if self.has_wm_state(*child) {
                    return *child;
                }
                queue.push_back((*child, depth + 1));
            }
        }
        frame
    }

    pub fn app_for_window(&self, w: Window) -> Option<AppContext> {
        let pid = self.window_pid(w)?;
        let class = self.wm_class(w);
        let app_id = proc::exe_name(pid).or_else(|| class.as_ref().map(|c| c.0.clone()))?;
        let name = proc::display_name(&app_id, class.as_ref().map(|c| c.1.as_str()));
        self.watched.windows.lock().insert(pid, w);
        Some(AppContext { app_id, name, pid, window_title: self.title(w), url: None })
    }

    pub fn refresh_foreground(&self) {
        let fg = self.active_window().and_then(|w| self.app_for_window(w).map(|a| (w, a)));
        *self.watched.foreground.write() = fg;
    }

    pub fn foreground(&self) -> Option<AppContext> {
        self.watched.foreground.read().as_ref().map(|(_, a)| a.clone())
    }

    pub fn pointer(&self) -> Option<Point> {
        let r = self.conn.query_pointer(self.root).ok()?.reply().ok()?;
        Some(Point::new(r.root_x as f64, r.root_y as f64))
    }

    /// Top-level window under a screen point.
    pub fn toplevel_at(&self, p: Point) -> Option<Window> {
        let r = self.conn.translate_coordinates(self.root, self.root, p.x as i16, p.y as i16).ok()?.reply().ok()?;
        (r.child != NONE).then_some(r.child)
    }

    pub fn app_at(&self, p: Point) -> Option<AppContext> {
        let frame = self.toplevel_at(p)?;
        if let Some((t, app)) = self.at_cache.lock().get(&frame) {
            if t.elapsed() < AT_CACHE_TTL {
                return app.clone();
            }
        }
        let client = self.client_window(frame);
        let app = self.app_for_window(client);
        let mut cache = self.at_cache.lock();
        if cache.len() > 64 {
            cache.clear();
        }
        cache.insert(frame, (Instant::now(), app.clone()));
        app
    }

    pub fn warp(&self, p: Point) {
        let _ = self.conn.warp_pointer(NONE, self.root, 0, 0, 0, 0, p.x as i16, p.y as i16);
        let _ = self.conn.flush();
    }

    /// Ask the window manager to activate a window (EWMH, source indication 2 = pager).
    pub fn activate(&self, w: Window) {
        let ev = ClientMessageEvent::new(32, w, self.atoms._NET_ACTIVE_WINDOW, [2, CURRENT_TIME, 0, 0, 0]);
        let mask = EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY;
        let _ = self.conn.send_event(false, self.root, mask, ev);
        let _ = self.conn.flush();
    }

    pub fn window_for_pid(&self, pid: u32) -> Option<Window> {
        self.watched.windows.lock().get(&pid).copied()
    }

    /// Start the watcher thread (own connection).
    pub fn spawn_watcher(self: &Arc<Self>, on_focus: Arc<dyn Fn(&AppContext) + Send + Sync>) {
        let me = Arc::clone(self);
        let _ = std::thread::Builder::new().name("veil-x11-watch".into()).spawn(move || {
            if let Err(e) = me.watch(on_focus) {
                tracing::warn!("X11 watcher stopped: {e}");
            }
        });
    }

    fn watch(&self, on_focus: Arc<dyn Fn(&AppContext) + Send + Sync>) -> anyhow::Result<()> {
        let (conn, _) = x11rb::connect(None)?;
        conn.change_window_attributes(self.root, &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE))?;
        let xfixes = conn.xfixes_query_version(5, 0).ok().and_then(|c| c.reply().ok()).is_some();
        if xfixes {
            conn.xfixes_select_selection_input(self.root, self.atoms.XdndSelection, SelectionEventMask::SET_SELECTION_OWNER)?;
        } else {
            tracing::warn!("XFixes unavailable: drag data can't be tied to new drags");
        }
        conn.flush()?;
        let mut watched_window: Option<Window> = None;
        loop {
            match conn.wait_for_event()? {
                Event::PropertyNotify(e) if e.window == self.root && e.atom == self.atoms._NET_ACTIVE_WINDOW => {
                    self.refresh_foreground();
                    let fg = self.watched.foreground.read().clone();
                    if let Some((w, app)) = fg {
                        if watched_window != Some(w) {
                            let _ = conn.change_window_attributes(w, &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE));
                            let _ = conn.flush();
                            watched_window = Some(w);
                        }
                        on_focus(&app);
                    }
                }
                Event::PropertyNotify(e)
                    if Some(e.window) == watched_window && (e.atom == self.atoms._NET_WM_NAME || e.atom == self.atoms.WM_NAME) =>
                {
                    let title = self.title(e.window);
                    if let Some((w, app)) = self.watched.foreground.write().as_mut() {
                        if *w == e.window {
                            app.window_title = title;
                        }
                    }
                }
                Event::XfixesSelectionNotify(e) if e.selection == self.atoms.XdndSelection => {
                    *self.watched.xdnd_changed.lock() = Some(Instant::now());
                }
                _ => {}
            }
        }
    }

    /// Contents of the drag in progress (`XdndSelection`), files first, else UTF-8 text.
    pub fn xdnd_session(&self) -> Option<DragSession> {
        if let Some(list) = read_selection(self.atoms.XdndSelection, self.atoms.TEXT_URI_LIST, &self.atoms) {
            let files = parse_uri_list(&String::from_utf8_lossy(&list));
            if !files.is_empty() {
                return Some(DragSession { files, text: None, has_image: false });
            }
        }
        for target in [self.atoms.UTF8_STRING, self.atoms.TEXT_PLAIN_UTF8] {
            if let Some(text) = read_selection(self.atoms.XdndSelection, target, &self.atoms) {
                let text = String::from_utf8_lossy(&text).into_owned();
                if !text.trim().is_empty() {
                    return Some(DragSession { files: Vec::new(), text: Some(text), has_image: false });
                }
            }
        }
        None
    }
}

const SELECTION_TIMEOUT: Duration = Duration::from_millis(200);

/// Convert a selection on a private connection and window, waiting briefly for the owner's answer.
/// INCR (chunked) transfers are not supported; uri-lists and short texts don't use them.
fn read_selection(selection: Atom, target: Atom, atoms: &Atoms) -> Option<Vec<u8>> {
    let (conn, screen_num) = x11rb::connect(None).ok()?;
    let root = conn.setup().roots[screen_num].root;
    if conn.get_selection_owner(selection).ok()?.reply().ok()?.owner == NONE {
        return None;
    }
    let win = conn.generate_id().ok()?;
    conn.create_window(COPY_DEPTH_FROM_PARENT, win, root, -10, -10, 1, 1, 0, WindowClass::INPUT_OUTPUT, 0, &CreateWindowAux::new())
        .ok()?;
    conn.delete_property(win, atoms.VEIL_SEL).ok()?;
    conn.convert_selection(win, selection, target, atoms.VEIL_SEL, CURRENT_TIME).ok()?;
    conn.flush().ok()?;
    let deadline = Instant::now() + SELECTION_TIMEOUT;
    let result = loop {
        match conn.poll_for_event() {
            Ok(Some(Event::SelectionNotify(e))) if e.requestor == win => {
                if e.property == NONE {
                    break None;
                }
                let r = conn.get_property(true, win, atoms.VEIL_SEL, AtomEnum::ANY, 0, 1 << 22).ok()?.reply().ok()?;
                break (!r.value.is_empty()).then_some(r.value);
            }
            Ok(Some(_)) => {}
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(3)),
            _ => break None,
        }
    };
    let _ = conn.destroy_window(win);
    let _ = conn.flush();
    result
}

/// Unused-but-kept helper to mark Veil's own windows (none today); avoids an unused import warning
/// when PropMode is needed by future helpers.
#[allow(dead_code)]
fn set_utf8(conn: &RustConnection, w: Window, prop: Atom, ty: Atom, value: &str) {
    let _ = conn.change_property8(PropMode::REPLACE, w, prop, ty, value.as_bytes());
}
