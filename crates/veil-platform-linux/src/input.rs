//! Kernel-level input: grab every keyboard and pointer (`EVIOCGRAB`), mirror each through its own
//! uinput device, and ask the core about every interesting event before re-emitting it.
//!
//! * One thread per grabbed device; frames (events up to `SYN_REPORT`) are translated, judged, filtered
//!   ([`filter_frame`]) and written to that device's mirror.
//! * Replays go through a separate "Veil replay" uinput device that is never grabbed, so they can't
//!   re-enter the handler.
//! * Button and key state must stay consistent per device: a key whose press went through a mirror is
//!   released on that mirror, and a swallowed left-button release (drop cancel) is released on the mirror
//!   that pressed it.

use crate::keymap::*;
use crate::translate::{filter_frame, Accumulator, Framer, Raw, Step, Translator};
use crate::x11::X11;
use evdev::raw_stream::RawDevice;
use evdev::uinput::VirtualDevice;
use evdev::{AttributeSet, InputEvent as EvEvent, KeyCode, RelativeAxisCode, UinputAbsSetup};
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use veil_core::input::{HookVerdict, InputEvent, Key, Replay};
use veil_core::platform::InputHandler;
use veil_core::types::Point;

/// Name prefix of Veil's own virtual devices (never grabbed).
pub const VIRTUAL_PREFIX: &str = "Veil ";
const POLL_MS: i32 = 250;
const GRAB_WAIT: Duration = Duration::from_secs(2);
const KEY_GAP: Duration = Duration::from_millis(4);

type Mirror = Arc<Mutex<VirtualDevice>>;

/// Where pointer coordinates come from.
pub enum PointerSource {
    X11(Arc<X11>),
    /// Wayland: accumulated relative motion (approximate).
    Accumulated(Mutex<Accumulator>),
}

impl PointerSource {
    fn position(&self) -> Point {
        match self {
            PointerSource::X11(x) => x.pointer().unwrap_or_default(),
            PointerSource::Accumulated(a) => a.lock().pos,
        }
    }

    fn feed(&self, ev: &Raw) {
        if let PointerSource::Accumulated(a) = self {
            a.lock().apply(ev);
        }
    }

    fn warp(&self, p: Point) {
        match self {
            PointerSource::X11(x) => x.warp(p),
            PointerSource::Accumulated(a) => a.lock().pos = p,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Keyboard,
    Pointer,
}

/// Is this device one we grab?
fn classify(dev: &RawDevice) -> Option<Kind> {
    if dev.name().is_some_and(|n| n.starts_with(VIRTUAL_PREFIX)) {
        return None;
    }
    let keys = dev.supported_keys()?;
    let keyboard = keys.contains(KeyCode(KEY_A)) && keys.contains(KeyCode(KEY_ENTER));
    let pointer = keys.contains(KeyCode(BTN_LEFT))
        && (dev.supported_relative_axes().is_some_and(|r| r.contains(RelativeAxisCode(REL_X)))
            || dev.supported_absolute_axes().is_some());
    if keyboard {
        Some(Kind::Keyboard)
    } else if pointer {
        Some(Kind::Pointer)
    } else {
        None
    }
}

/// A uinput device with the same capabilities as `dev`.
fn build_mirror(dev: &RawDevice) -> std::io::Result<VirtualDevice> {
    let name = format!("{VIRTUAL_PREFIX}mirror: {}", dev.name().unwrap_or("input"));
    let mut b = VirtualDevice::builder()?.name(name.as_bytes()).input_id(dev.input_id());
    if let Some(keys) = dev.supported_keys() {
        b = b.with_keys(keys)?;
    }
    if let Some(rel) = dev.supported_relative_axes() {
        b = b.with_relative_axes(rel)?;
    }
    if dev.supported_absolute_axes().is_some() {
        for (axis, info) in dev.get_absinfo()? {
            b = b.with_absolute_axis(&UinputAbsSetup::new(axis, info))?;
        }
    }
    if let Some(msc) = dev.misc_properties() {
        b = b.with_msc(msc)?;
    }
    b = b.with_properties(dev.properties())?;
    b.build()
}

/// The replay device: every keyboard key, the mouse buttons, relative motion and wheel.
fn build_replay_device() -> std::io::Result<VirtualDevice> {
    let mut keys = AttributeSet::<KeyCode>::new();
    for code in 1..=248u16 {
        keys.insert(KeyCode(code));
    }
    for code in [BTN_LEFT, BTN_RIGHT, BTN_MIDDLE] {
        keys.insert(KeyCode(code));
    }
    let mut rel = AttributeSet::<RelativeAxisCode>::new();
    for code in [REL_X, REL_Y, REL_WHEEL, REL_HWHEEL] {
        rel.insert(RelativeAxisCode(code));
    }
    VirtualDevice::builder()?
        .name(format!("{VIRTUAL_PREFIX}replay").as_bytes())
        .with_keys(&keys)?
        .with_relative_axes(&rel)?
        .build()
}

fn set_nonblocking(fd: i32) {
    // SAFETY: fcntl on a valid, owned descriptor.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        if flags >= 0 {
            libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
    }
}

/// Wait until `fd` is readable or the timeout passes. Returns false on timeout/error.
fn poll_readable(fd: i32, timeout_ms: i32) -> bool {
    let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
    // SAFETY: one valid pollfd.
    let n = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
    n > 0 && pfd.revents & libc::POLLIN != 0
}

fn ev(r: &Raw) -> EvEvent {
    EvEvent::new(r.ty, r.code, r.value)
}

pub struct Input {
    handler: Arc<dyn InputHandler>,
    translator: Mutex<Translator>,
    pointer: PointerSource,
    replay: Mutex<VirtualDevice>,
    grabbed: Mutex<HashSet<PathBuf>>,
    /// Keys currently held through a mirror (code → mirror).
    down_on_mirror: Mutex<HashMap<u16, Mirror>>,
    /// Mirror whose left button is still down because its release was swallowed (drop cancel).
    pending_left_release: Mutex<Option<Mirror>>,
    stop: Arc<AtomicBool>,
    /// Errors from the last device scan (permissions etc.).
    pub last_error: Mutex<Option<String>>,
}

impl Input {
    pub fn new(
        handler: Arc<dyn InputHandler>,
        pointer: PointerSource,
        stop: Arc<AtomicBool>,
    ) -> anyhow::Result<Arc<Input>> {
        let replay = build_replay_device().map_err(|e| {
            anyhow::anyhow!(
                "can't create a virtual input device (/dev/uinput): {e}. Add your user to the `veil` group and install packaging/linux/99-veil.rules"
            )
        })?;
        Ok(Arc::new(Input {
            handler,
            translator: Mutex::new(Translator::new(Arc::new(UsKeymap))),
            pointer,
            replay: Mutex::new(replay),
            grabbed: Mutex::new(HashSet::new()),
            down_on_mirror: Mutex::new(HashMap::new()),
            pending_left_release: Mutex::new(None),
            stop,
            last_error: Mutex::new(None),
        }))
    }

    /// Grab every keyboard/pointer not yet grabbed. Returns how many are grabbed in total.
    pub fn scan(self: &Arc<Self>) -> usize {
        let mut errors = Vec::new();
        let entries = match std::fs::read_dir("/dev/input") {
            Ok(e) => e,
            Err(e) => {
                *self.last_error.lock() = Some(format!("can't list /dev/input: {e}"));
                return 0;
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let is_event = path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("event"));
            if !is_event || self.grabbed.lock().contains(&path) {
                continue;
            }
            match RawDevice::open(&path) {
                Ok(dev) => {
                    if let Some(kind) = classify(&dev) {
                        if let Err(e) = self.start_device(path.clone(), dev, kind) {
                            errors.push(format!("{}: {e}", path.display()));
                        }
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                    errors.push(format!("{}: permission denied", path.display()));
                }
                Err(_) => {}
            }
        }
        let n = self.grabbed.lock().len();
        *self.last_error.lock() = if n == 0 && !errors.is_empty() { Some(errors.join("; ")) } else { None };
        if n == 0 && !errors.is_empty() {
            tracing::warn!("no input devices grabbed: {}", errors.join("; "));
        }
        n
    }

    fn start_device(self: &Arc<Self>, path: PathBuf, mut dev: RawDevice, kind: Kind) -> anyhow::Result<()> {
        let mirror: Mirror = Arc::new(Mutex::new(build_mirror(&dev)?));
        // Don't grab a keyboard with keys held (their releases would be lost and keys would stick).
        if kind == Kind::Keyboard {
            let until = Instant::now() + GRAB_WAIT;
            while Instant::now() < until && dev.get_key_state().map(|s| s.iter().next().is_some()).unwrap_or(false) {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        dev.grab()?;
        set_nonblocking(dev.as_raw_fd());
        self.grabbed.lock().insert(path.clone());
        tracing::info!("grabbed {:?} {}", kind, dev.name().unwrap_or("?"));
        let me = Arc::clone(self);
        std::thread::Builder::new().name("veil-evdev".into()).spawn(move || {
            me.device_loop(&path, dev, mirror);
            me.grabbed.lock().remove(&path);
        })?;
        Ok(())
    }

    fn device_loop(&self, path: &Path, mut dev: RawDevice, mirror: Mirror) {
        let fd = dev.as_raw_fd();
        let mut framer = Framer::default();
        loop {
            if self.stop.load(Ordering::Relaxed) {
                break;
            }
            if !poll_readable(fd, POLL_MS) {
                continue;
            }
            let raws: Vec<Raw> = match dev.fetch_events() {
                Ok(events) => events.map(|e| Raw::new(e.event_type().0, e.code(), e.value())).collect(),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
                Err(e) => {
                    tracing::info!("input device {} gone: {e}", path.display());
                    break;
                }
            };
            for raw in raws {
                if let Some(frame) = framer.push(raw) {
                    self.process_frame(&frame, &mirror);
                }
            }
        }
        let _ = dev.ungrab();
    }

    fn process_frame(&self, frame: &[Raw], mirror: &Mirror) {
        let now = Instant::now();
        for r in frame {
            self.pointer.feed(r);
        }
        let mut swallowed = vec![false; frame.len()];
        let mut asks: Vec<(usize, InputEvent)> = Vec::new();
        {
            let mut tr = self.translator.lock();
            let mut pos = || self.pointer.position();
            for (i, r) in frame.iter().enumerate() {
                match tr.step(r, now, &mut pos) {
                    Step::Pass => {}
                    Step::Drop => swallowed[i] = true,
                    Step::Ask(e) => asks.push((i, e)),
                }
            }
        }
        for (i, e) in &asks {
            if self.handler.on_input(e) == HookVerdict::Swallow {
                swallowed[*i] = true;
            }
        }
        let drag = {
            let mut tr = self.translator.lock();
            for (i, _) in &asks {
                tr.verdict(&frame[*i], swallowed[*i]);
            }
            let mut pos = || self.pointer.position();
            tr.end_frame(&mut pos)
        };
        if let Some(d) = drag {
            // Drags are observed, never swallowed.
            let _ = self.handler.on_input(&d);
        }

        // Book-keeping for per-device key/button consistency.
        for (i, r) in frame.iter().enumerate() {
            if r.ty != EV_KEY {
                continue;
            }
            if r.code == BTN_LEFT && r.value == 0 && swallowed[i] {
                *self.pending_left_release.lock() = Some(Arc::clone(mirror));
            } else if !is_button(r.code) && !swallowed[i] {
                match r.value {
                    1 => {
                        self.down_on_mirror.lock().insert(r.code, Arc::clone(mirror));
                    }
                    0 => {
                        self.down_on_mirror.lock().remove(&r.code);
                    }
                    _ => {}
                }
            }
        }

        let out = filter_frame(frame, &swallowed);
        if out.is_empty() {
            return;
        }
        let events: Vec<EvEvent> = out.iter().filter(|r| !r.is_syn_report()).map(ev).collect();
        if let Err(e) = mirror.lock().emit(&events) {
            tracing::warn!("mirror write failed: {e}");
        }
    }

    // ----- replays (worker thread) -----

    fn emit_replay(&self, events: &[Raw]) {
        let evs: Vec<EvEvent> = events.iter().map(ev).collect();
        if let Err(e) = self.replay.lock().emit(&evs) {
            tracing::warn!("replay write failed: {e}");
        }
    }

    fn key_on_replay(&self, code: u16, value: i32) {
        self.emit_replay(&[Raw::new(EV_KEY, code, value)]);
        std::thread::sleep(KEY_GAP);
    }

    pub fn replay(&self, r: &Replay) {
        match r {
            Replay::Key { key, mods, .. } => {
                let code = evdev_code(*key);
                let mods = modifier_codes(*mods);
                for m in &mods {
                    self.key_on_replay(*m, 1);
                }
                self.key_on_replay(code, 1);
                self.key_on_replay(code, 0);
                for m in mods.iter().rev() {
                    self.key_on_replay(*m, 0);
                }
            }
            Replay::KeyDown { key, .. } => self.key_on_replay(evdev_code(*key), 1),
            Replay::KeyUp { key, .. } => {
                let code = evdev_code(*key);
                let held = self.down_on_mirror.lock().remove(&code);
                match held {
                    Some(m) => {
                        let _ = m.lock().emit(&[EvEvent::new(EV_KEY, code, 0)]);
                    }
                    None => self.key_on_replay(code, 0),
                }
            }
            Replay::Click { at, clicks } => {
                self.pointer.warp(*at);
                std::thread::sleep(Duration::from_millis(15));
                for _ in 0..(*clicks).max(1) {
                    self.key_on_replay(BTN_LEFT, 1);
                    std::thread::sleep(Duration::from_millis(20));
                    self.key_on_replay(BTN_LEFT, 0);
                    std::thread::sleep(Duration::from_millis(30));
                }
            }
        }
    }

    /// Cancel an X11 drag whose button-up was swallowed: Escape while the button is still down (the
    /// drag source aborts), then release the button on the device that pressed it.
    pub fn cancel_drop(&self) {
        self.replay(&Replay::Key { key: Key::Escape, mods: Default::default(), text: None });
        std::thread::sleep(Duration::from_millis(40));
        let mirror = self.pending_left_release.lock().take();
        match mirror {
            Some(m) => {
                let _ = m.lock().emit(&[EvEvent::new(EV_KEY, BTN_LEFT, 0)]);
            }
            None => self.key_on_replay(BTN_LEFT, 0),
        }
    }
}

/// Can we read input devices and write uinput?
pub fn devices_accessible() -> (bool, String) {
    let uinput = std::fs::OpenOptions::new().write(true).open("/dev/uinput").is_ok();
    let mut readable = 0;
    let mut total = 0;
    if let Ok(entries) = std::fs::read_dir("/dev/input") {
        for e in entries.flatten() {
            let p = e.path();
            if p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("event")) {
                total += 1;
                if std::fs::File::open(&p).is_ok() {
                    readable += 1;
                }
            }
        }
    }
    let ok = uinput && readable > 0;
    let detail = format!("/dev/uinput writable: {uinput}; readable input devices: {readable}/{total}");
    (ok, detail)
}
