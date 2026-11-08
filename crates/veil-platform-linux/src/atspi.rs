//! AT-SPI2 over D-Bus (pure-Rust zbus, no libatspi).
//!
//! * The accessibility bus address comes from `org.a11y.Bus.GetAddress` on the session bus (or
//!   `AT_SPI_BUS_ADDRESS`).
//! * Focus is tracked from `object:state-changed:focused` events on a listener thread, so the hook path
//!   (`foreground()` on Wayland) never makes a D-Bus call.
//! * Two connections: `fast` (short method timeout) for `element_at` on the hook path, `conn` for the
//!   worker (text, file chooser, URL lookups).

use crate::proc;
use parking_lot::{Mutex, RwLock};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use veil_core::types::{AppContext, FocusedText, Point, UiElement};
use zbus::blocking::Connection;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

const ACCESSIBLE: &str = "org.a11y.atspi.Accessible";
const COMPONENT: &str = "org.a11y.atspi.Component";
const TEXT: &str = "org.a11y.atspi.Text";
const EDITABLE_TEXT: &str = "org.a11y.atspi.EditableText";
const DOCUMENT: &str = "org.a11y.atspi.Document";
const SELECTION: &str = "org.a11y.atspi.Selection";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";
const NULL_PATH: &str = "/org/a11y/atspi/null";
const REGISTRY: &str = "org.a11y.atspi.Registry";
const ROOT_PATH: &str = "/org/a11y/atspi/accessible/root";

// AtspiStateType bit positions.
const STATE_CHECKED: u32 = 4;
const STATE_EDITABLE: u32 = 7;
const STATE_PRESSED: u32 = 20;
const STATE_SHOWING: u32 = 25;

/// Hook-path budget for `element_at`.
const ELEMENT_AT_BUDGET: Duration = Duration::from_millis(60);
const FAST_TIMEOUT: Duration = Duration::from_millis(40);
const SLOW_TIMEOUT: Duration = Duration::from_millis(1500);
const APP_CACHE_TTL: Duration = Duration::from_secs(5);
const PICKER_MAX_NODES: usize = 4000;
const PICKER_BUDGET: Duration = Duration::from_millis(1500);

/// An accessible object: D-Bus name of its application + object path.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Obj {
    pub bus: String,
    pub path: String,
}

impl Obj {
    fn from_pair((bus, path): (String, OwnedObjectPath)) -> Option<Obj> {
        let path = path.as_str().to_string();
        (!bus.is_empty() && path != NULL_PATH).then_some(Obj { bus, path })
    }
}

/// Address of the accessibility bus.
fn a11y_address() -> anyhow::Result<String> {
    if let Ok(addr) = std::env::var("AT_SPI_BUS_ADDRESS") {
        if !addr.is_empty() {
            return Ok(addr);
        }
    }
    let session = Connection::session()?;
    let reply = session.call_method(Some("org.a11y.Bus"), "/org/a11y/bus", Some("org.a11y.Bus"), "GetAddress", &())?;
    Ok(reply.body().deserialize::<String>()?)
}

/// Turn on toolkit accessibility for the session (`org.a11y.Status.IsEnabled`). Chromium/Electron and
/// GTK apps start exposing their trees when this is set.
pub fn set_enabled(enabled: bool) -> anyhow::Result<()> {
    let session = Connection::session()?;
    session.call_method(
        Some("org.a11y.Bus"),
        "/org/a11y/bus",
        Some(PROPERTIES),
        "Set",
        &("org.a11y.Status", "IsEnabled", Value::from(enabled)),
    )?;
    Ok(())
}

/// Whether toolkit accessibility is enabled for the session.
pub fn is_enabled() -> Option<bool> {
    let session = Connection::session().ok()?;
    let reply = session
        .call_method(Some("org.a11y.Bus"), "/org/a11y/bus", Some(PROPERTIES), "Get", &("org.a11y.Status", "IsEnabled"))
        .ok()?;
    let v: OwnedValue = reply.body().deserialize().ok()?;
    bool::try_from(v).ok()
}

/// Whether the accessibility bus answers at all.
pub fn bus_reachable() -> bool {
    a11y_address()
        .ok()
        .and_then(|a| zbus::blocking::connection::Builder::address(a.as_str()).ok()?.build().ok())
        .is_some()
}

/// Focus as tracked by the listener.
#[derive(Clone, Debug)]
struct Focus {
    obj: Obj,
    app: AppContext,
}

pub struct Atspi {
    conn: Connection,
    fast: Connection,
    focus: RwLock<Option<Focus>>,
    /// Last document URL seen per browser pid.
    urls: Mutex<HashMap<u32, String>>,
    /// Application bus name → pid.
    pids: Mutex<HashMap<String, Option<u32>>>,
    /// pid → application root objects.
    apps: Mutex<HashMap<u32, (Instant, Vec<Obj>)>>,
}

impl Atspi {
    pub fn connect() -> anyhow::Result<Arc<Atspi>> {
        let addr = a11y_address()?;
        let conn = zbus::blocking::connection::Builder::address(addr.as_str())?.method_timeout(SLOW_TIMEOUT).build()?;
        let fast = zbus::blocking::connection::Builder::address(addr.as_str())?.method_timeout(FAST_TIMEOUT).build()?;
        Ok(Arc::new(Atspi {
            conn,
            fast,
            focus: RwLock::new(None),
            urls: Mutex::new(HashMap::new()),
            pids: Mutex::new(HashMap::new()),
            apps: Mutex::new(HashMap::new()),
        }))
    }

    // ----- raw calls -----

    fn call<B, R>(&self, conn: &Connection, obj: &Obj, iface: &str, method: &str, body: &B) -> Option<R>
    where
        B: serde::Serialize + zbus::zvariant::DynamicType,
        R: for<'d> serde::Deserialize<'d> + zbus::zvariant::Type,
    {
        let reply = conn.call_method(Some(obj.bus.as_str()), obj.path.as_str(), Some(iface), method, body).ok()?;
        reply.body().deserialize::<R>().ok()
    }

    fn prop(&self, conn: &Connection, obj: &Obj, iface: &str, name: &str) -> Option<OwnedValue> {
        self.call::<_, OwnedValue>(conn, obj, PROPERTIES, "Get", &(iface, name))
    }

    fn name(&self, conn: &Connection, obj: &Obj) -> String {
        self.prop(conn, obj, ACCESSIBLE, "Name").and_then(|v| String::try_from(v).ok()).unwrap_or_default()
    }

    fn description(&self, conn: &Connection, obj: &Obj) -> String {
        self.prop(conn, obj, ACCESSIBLE, "Description").and_then(|v| String::try_from(v).ok()).unwrap_or_default()
    }

    fn role_name(&self, conn: &Connection, obj: &Obj) -> String {
        self.call::<_, String>(conn, obj, ACCESSIBLE, "GetRoleName", &()).unwrap_or_default()
    }

    fn parent(&self, conn: &Connection, obj: &Obj) -> Option<Obj> {
        let v = self.prop(conn, obj, ACCESSIBLE, "Parent")?;
        let pair = <(String, OwnedObjectPath)>::try_from(v).ok()?;
        Obj::from_pair(pair).filter(|p| p != obj)
    }

    fn children(&self, conn: &Connection, obj: &Obj) -> Vec<Obj> {
        self.call::<_, Vec<(String, OwnedObjectPath)>>(conn, obj, ACCESSIBLE, "GetChildren", &())
            .unwrap_or_default()
            .into_iter()
            .filter_map(Obj::from_pair)
            .collect()
    }

    fn states(&self, conn: &Connection, obj: &Obj) -> u64 {
        let v = self.call::<_, Vec<u32>>(conn, obj, ACCESSIBLE, "GetState", &()).unwrap_or_default();
        let lo = v.first().copied().unwrap_or(0) as u64;
        let hi = v.get(1).copied().unwrap_or(0) as u64;
        lo | (hi << 32)
    }

    fn has_state(states: u64, bit: u32) -> bool {
        states & (1u64 << bit) != 0
    }

    fn text_of(&self, conn: &Connection, obj: &Obj) -> Option<String> {
        self.call::<_, String>(conn, obj, TEXT, "GetText", &(0i32, -1i32))
    }

    /// Process id behind an application's bus name.
    fn pid_of(&self, bus: &str) -> Option<u32> {
        if let Some(pid) = self.pids.lock().get(bus) {
            return *pid;
        }
        let pid = self
            .conn
            .call_method(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                Some("org.freedesktop.DBus"),
                "GetConnectionUnixProcessID",
                &(bus,),
            )
            .ok()
            .and_then(|m| m.body().deserialize::<u32>().ok());
        let mut pids = self.pids.lock();
        if pids.len() > 512 {
            pids.clear();
        }
        pids.insert(bus.to_string(), pid);
        pid
    }

    fn app_context(&self, pid: u32, window_title: Option<String>) -> Option<AppContext> {
        let app_id = proc::exe_name(pid)?;
        let name = proc::display_name(&app_id, None);
        let url = self.urls.lock().get(&pid).cloned();
        Some(AppContext { app_id, name, pid, window_title, url })
    }

    /// Application root objects (children of the registry root) for a pid.
    fn apps_for_pid(&self, conn: &Connection, pid: u32) -> Vec<Obj> {
        if let Some((t, objs)) = self.apps.lock().get(&pid) {
            if t.elapsed() < APP_CACHE_TTL {
                return objs.clone();
            }
        }
        let root = Obj { bus: REGISTRY.into(), path: ROOT_PATH.into() };
        let mut found = Vec::new();
        for app in self.children(conn, &root) {
            if self.pid_of(&app.bus) == Some(pid) {
                found.push(app);
            }
        }
        self.apps.lock().insert(pid, (Instant::now(), found.clone()));
        found
    }

    // ----- focus tracking -----

    /// Register for focus events and track them on a background thread.
    pub fn spawn_listener(self: &Arc<Self>) {
        let me = Arc::clone(self);
        let _ = std::thread::Builder::new().name("veil-atspi".into()).spawn(move || {
            if let Err(e) = me.listen() {
                tracing::warn!("AT-SPI listener stopped: {e}");
            }
        });
    }

    fn register_event(&self, event: &str) {
        let reg = Obj { bus: REGISTRY.into(), path: "/org/a11y/atspi/registry".into() };
        // at-spi2-core ≥ 2.46 takes (event, properties, app_bus_name); older versions take (event).
        let newer = self
            .conn
            .call_method(
                Some(reg.bus.as_str()),
                reg.path.as_str(),
                Some(REGISTRY),
                "RegisterEvent",
                &(event, Vec::<String>::new(), ""),
            )
            .is_ok();
        if !newer {
            let _ = self.conn.call_method(
                Some(reg.bus.as_str()),
                reg.path.as_str(),
                Some(REGISTRY),
                "RegisterEvent",
                &(event,),
            );
        }
    }

    fn listen(&self) -> anyhow::Result<()> {
        self.register_event("object:state-changed:focused");
        let rule = zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .interface("org.a11y.atspi.Event.Object")?
            .member("StateChanged")?
            .arg(0, "focused")?
            .build();
        let iter = zbus::blocking::MessageIterator::for_match_rule(rule, &self.conn, Some(256))?;
        for msg in iter {
            let Ok(msg) = msg else { continue };
            let header = msg.header();
            let (Some(sender), Some(path)) = (header.sender(), header.path()) else { continue };
            let gained = parse_state_event(&msg).is_some_and(|(detail, d1)| detail == "focused" && d1 == 1);
            if !gained {
                continue;
            }
            let obj = Obj { bus: sender.to_string(), path: path.to_string() };
            self.on_focus(obj);
        }
        Ok(())
    }

    fn on_focus(&self, obj: Obj) {
        let Some(pid) = self.pid_of(&obj.bus) else { return };
        // Window title and, for browsers, the document URL: walk up the ancestors once.
        let mut title = None;
        let mut url = None;
        let browser = proc::exe_name(pid).is_some_and(|e| proc::is_browser(&e));
        let mut cur = Some(obj.clone());
        for _ in 0..48 {
            let Some(o) = cur else { break };
            let role = self.role_name(&self.conn, &o);
            if browser && url.is_none() && role == "document web" {
                url = self.document_url(&o);
            }
            if role == "frame" || role == "window" || role == "dialog" {
                title = Some(self.name(&self.conn, &o)).filter(|t| !t.is_empty());
                break;
            }
            cur = self.parent(&self.conn, &o);
        }
        if let Some(u) = url {
            self.urls.lock().insert(pid, u);
        }
        if let Some(app) = self.app_context(pid, title) {
            *self.focus.write() = Some(Focus { obj, app });
        }
    }

    fn document_url(&self, doc: &Obj) -> Option<String> {
        if let Some(u) = self
            .call::<_, String>(&self.conn, doc, DOCUMENT, "GetAttributeValue", &("DocURL",))
            .filter(|u| !u.is_empty())
        {
            return Some(u);
        }
        let attrs = self.call::<_, HashMap<String, String>>(&self.conn, doc, DOCUMENT, "GetAttributes", &())?;
        attrs.get("DocURL").or_else(|| attrs.get("URL")).cloned().filter(|u| !u.is_empty())
    }

    /// App that has keyboard focus (from events; no D-Bus call).
    pub fn focused_app(&self) -> Option<AppContext> {
        self.focus.read().as_ref().map(|f| {
            let mut app = f.app.clone();
            app.url = self.urls.lock().get(&app.pid).cloned();
            app
        })
    }

    /// Last known document URL of a browser process.
    pub fn url_for(&self, pid: u32) -> Option<String> {
        self.urls.lock().get(&pid).cloned()
    }

    fn focused_obj(&self, pid: u32) -> Option<Obj> {
        self.focus.read().as_ref().filter(|f| f.app.pid == pid).map(|f| f.obj.clone())
    }

    // ----- worker-thread queries -----

    /// Text of the focused editable element of `pid`.
    pub fn focused_text(&self, pid: u32) -> FocusedText {
        let Some(obj) = self.focused_obj(pid) else { return FocusedText::NotExposed { role: "unknown".into() } };
        let role = self.role_name(&self.conn, &obj);
        // Prefer the element itself; rich editors sometimes focus a child paragraph, so try the parent too.
        if let Some(text) = self.text_of(&self.conn, &obj) {
            let states = self.states(&self.conn, &obj);
            if Self::has_state(states, STATE_EDITABLE) || !text.is_empty() {
                return FocusedText::Exposed { text, role };
            }
        }
        if let Some(parent) = self.parent(&self.conn, &obj) {
            if Self::has_state(self.states(&self.conn, &parent), STATE_EDITABLE) {
                if let Some(text) = self.text_of(&self.conn, &parent) {
                    return FocusedText::Exposed { text, role: self.role_name(&self.conn, &parent) };
                }
            }
        }
        FocusedText::NotExposed { role }
    }

    /// Replace the focused editable element's text. Verified by reading it back.
    pub fn replace_focused_text(&self, pid: u32, text: &str) -> bool {
        let Some(obj) = self.focused_obj(pid) else { return false };
        if !Self::has_state(self.states(&self.conn, &obj), STATE_EDITABLE) {
            return false;
        }
        let ok = self.call::<_, bool>(&self.conn, &obj, EDITABLE_TEXT, "SetTextContents", &(text,)).unwrap_or(false);
        ok && self.text_of(&self.conn, &obj).is_some_and(|t| t == text)
    }

    /// Accessible element under a screen point in `pid`'s windows. Hook path: bounded by a short budget.
    pub fn element_at(&self, pid: u32, p: Point) -> Option<UiElement> {
        let deadline = Instant::now() + ELEMENT_AT_BUDGET;
        let conn = &self.fast;
        let mut hit: Option<Obj> = None;
        'apps: for app in self.apps_for_pid(conn, pid) {
            for frame in self.children(conn, &app) {
                if Instant::now() >= deadline {
                    break 'apps;
                }
                if !Self::has_state(self.states(conn, &frame), STATE_SHOWING) {
                    continue;
                }
                let mut cur = frame.clone();
                let mut found = false;
                for _ in 0..32 {
                    if Instant::now() >= deadline {
                        break;
                    }
                    let next = self
                        .call::<_, (String, OwnedObjectPath)>(
                            conn,
                            &cur,
                            COMPONENT,
                            "GetAccessibleAtPoint",
                            &(p.x as i32, p.y as i32, 0u32),
                        )
                        .and_then(Obj::from_pair);
                    match next {
                        Some(n) if n != cur => {
                            found = true;
                            cur = n;
                        }
                        _ => break,
                    }
                }
                if found {
                    hit = Some(cur);
                    break 'apps;
                }
            }
        }
        let obj = hit?;
        let role = self.role_name(conn, &obj);
        let mut label = self.name(conn, &obj);
        if label.is_empty() {
            label = self.description(conn, &obj);
        }
        let label = label.to_lowercase();
        let is_button = matches!(role.as_str(), "push button" | "button" | "toggle button" | "menu button");
        let is_generic = label.is_empty()
            && matches!(
                role.as_str(),
                "section" | "panel" | "filler" | "unknown" | "redundant object" | "static" | "image"
            );
        // File dialog ancestor (bounded by the remaining budget).
        let mut in_file_dialog = false;
        let mut cur = self.parent(conn, &obj);
        for _ in 0..24 {
            if Instant::now() >= deadline + Duration::from_millis(20) {
                break;
            }
            let Some(o) = cur else { break };
            let r = self.role_name(conn, &o);
            if r == "file chooser" {
                in_file_dialog = true;
                break;
            }
            if r == "dialog" {
                let n = self.name(conn, &o).to_lowercase();
                in_file_dialog = ["open", "upload", "choose", "select file", "attach"].iter().any(|w| n.contains(w));
                break;
            }
            if r == "frame" || r == "application" {
                break;
            }
            cur = self.parent(conn, &o);
        }
        Some(UiElement { role, label, pid, is_button, in_file_dialog, is_generic })
    }

    /// Files selected in a GTK/Qt/portal file chooser shown by `pid` (best effort).
    pub fn picker_selection(&self, pid: u32) -> Vec<PathBuf> {
        let deadline = Instant::now() + PICKER_BUDGET;
        let conn = &self.conn;
        // 1. Find the chooser: role "file chooser", else a showing dialog.
        let mut chooser: Option<Obj> = None;
        let mut dialog: Option<Obj> = None;
        let mut queue: VecDeque<Obj> = self.apps_for_pid(conn, pid).into_iter().collect();
        let mut seen = HashSet::new();
        while let Some(o) = queue.pop_front() {
            if seen.len() > PICKER_MAX_NODES || Instant::now() >= deadline {
                break;
            }
            if !seen.insert(o.clone()) {
                continue;
            }
            let role = self.role_name(conn, &o);
            if role == "file chooser" {
                chooser = Some(o);
                break;
            }
            if role == "dialog" && dialog.is_none() && Self::has_state(self.states(conn, &o), STATE_SHOWING) {
                dialog = Some(o.clone());
            }
            queue.extend(self.children(conn, &o));
        }
        let Some(root) = chooser.or(dialog) else { return Vec::new() };

        // 2. Collect entry text, selected row names and path-bar buttons inside it.
        let mut entries: Vec<String> = Vec::new();
        let mut selected: Vec<String> = Vec::new();
        let mut path_bar: Vec<(String, bool)> = Vec::new();
        let mut queue = VecDeque::from([root]);
        let mut visited = 0usize;
        while let Some(o) = queue.pop_front() {
            visited += 1;
            if visited > PICKER_MAX_NODES || Instant::now() >= deadline {
                break;
            }
            let role = self.role_name(conn, &o);
            match role.as_str() {
                "text" | "entry" | "editbar" => {
                    if let Some(t) = self.text_of(conn, &o).filter(|t| !t.trim().is_empty()) {
                        entries.push(t);
                    }
                }
                "toggle button" => {
                    let s = self.states(conn, &o);
                    let name = self.name(conn, &o);
                    if !name.is_empty() {
                        path_bar.push((name, Self::has_state(s, STATE_CHECKED) || Self::has_state(s, STATE_PRESSED)));
                    }
                }
                "table" | "tree table" | "list" | "tree" | "list box" | "grid" => {
                    let n = self
                        .prop(conn, &o, SELECTION, "NSelectedChildren")
                        .and_then(|v| i32::try_from(v).ok())
                        .unwrap_or(0);
                    for i in 0..n.min(64) {
                        if let Some(cell) = self
                            .call::<_, (String, OwnedObjectPath)>(conn, &o, SELECTION, "GetSelectedChild", &(i,))
                            .and_then(Obj::from_pair)
                        {
                            let mut name = self.name(conn, &cell);
                            if name.is_empty() {
                                if let Some(first) = self.children(conn, &cell).first() {
                                    name = self.name(conn, first);
                                }
                            }
                            if !name.is_empty() && !selected.contains(&name) {
                                selected.push(name);
                            }
                        }
                    }
                    // Rows are many; don't descend into the table.
                    continue;
                }
                _ => {}
            }
            queue.extend(self.children(conn, &o));
        }
        resolve_selection(&entries, &selected, &path_bar, proc::cwd(pid).as_deref())
    }
}

/// Body of an `org.a11y.atspi.Event.Object` signal: `(detail, detail1)`. Newer at-spi2-core sends
/// `(siiva{sv})`, older `(siiv(so))`.
fn parse_state_event(msg: &zbus::Message) -> Option<(String, i32)> {
    let body = msg.body();
    if let Ok((detail, d1, _d2, _any, _props)) =
        body.deserialize::<(String, i32, i32, OwnedValue, HashMap<String, OwnedValue>)>()
    {
        return Some((detail, d1));
    }
    if let Ok((detail, d1, _d2, _any, _app)) =
        body.deserialize::<(String, i32, i32, OwnedValue, (String, OwnedObjectPath))>()
    {
        return Some((detail, d1));
    }
    None
}

/// Turn what the chooser shows into existing paths.
///
/// * an absolute path typed in the location entry wins;
/// * otherwise selected row names (or the entry's file names) are joined to the current folder, which is
///   reconstructed from the path bar (buttons up to the checked one), trying `/` and `$HOME` as roots.
fn resolve_selection(
    entries: &[String],
    selected: &[String],
    path_bar: &[(String, bool)],
    cwd: Option<&Path>,
) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in entries {
        let e = e.trim();
        if e.starts_with('/') {
            let p = PathBuf::from(e);
            if p.is_file() {
                out.push(p);
            }
        }
    }
    if !out.is_empty() {
        return out;
    }

    let mut names: Vec<String> = selected.to_vec();
    if names.is_empty() {
        for e in entries {
            names.extend(split_quoted(e));
        }
    }
    if names.is_empty() {
        return out;
    }

    let mut folders: Vec<PathBuf> = Vec::new();
    for e in entries {
        let e = e.trim();
        if e.starts_with('/') && Path::new(e).is_dir() {
            folders.push(PathBuf::from(e));
        }
    }
    let upto = path_bar.iter().position(|(_, checked)| *checked).map(|i| i + 1).unwrap_or(path_bar.len());
    let parts: Vec<&str> = path_bar[..upto].iter().map(|(n, _)| n.as_str()).collect();
    if !parts.is_empty() {
        // Rooted at the file system.
        let start = parts
            .iter()
            .position(|p| *p == "/" || p.eq_ignore_ascii_case("file system") || p.eq_ignore_ascii_case("computer"));
        if let Some(s) = start {
            let mut p = PathBuf::from("/");
            p.extend(&parts[s + 1..]);
            folders.push(p);
        }
        // Rooted at home (first button is often the user name or "Home").
        if let Ok(home) = std::env::var("HOME") {
            let user = Path::new(&home).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let h = parts.iter().position(|p| p.eq_ignore_ascii_case("home") || *p == user);
            if let Some(h) = h {
                let mut p = PathBuf::from(&home);
                p.extend(&parts[h + 1..]);
                folders.push(p);
            }
            let mut p = PathBuf::from(&home);
            p.extend(&parts);
            folders.push(p);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        for d in ["", "Downloads", "Documents", "Desktop", "Pictures"] {
            folders.push(Path::new(&home).join(d));
        }
    }
    if let Some(c) = cwd {
        folders.push(c.to_path_buf());
    }
    for folder in folders.iter().filter(|f| f.is_dir()) {
        let found: Vec<PathBuf> = names.iter().map(|n| folder.join(n)).filter(|p| p.exists()).collect();
        if found.len() == names.len() {
            return found;
        }
    }
    out
}

/// `"a b.txt" "c.pdf"` → `["a b.txt", "c.pdf"]`; a plain name stays as is.
fn split_quoted(s: &str) -> Vec<String> {
    let s = s.trim();
    if !s.contains('"') {
        return if s.is_empty() { Vec::new() } else { vec![s.to_string()] };
    }
    s.split('"').enumerate().filter(|(i, part)| i % 2 == 1 && !part.is_empty()).map(|(_, p)| p.to_string()).collect()
}
