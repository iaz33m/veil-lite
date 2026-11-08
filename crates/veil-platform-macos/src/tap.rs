//! The session event tap (macOS's WH_KEYBOARD_LL / WH_MOUSE_LL) and input re-injection.
//!
//! Re-injected events carry [`MAGIC`] in `kCGEventSourceUserData` (set on the source and on each event).
//! As in the proof of concept, every injected event is also remembered for 0.5 s so the tap recognises it
//! even if the field is rewritten on the way back in. Recognised events never reach the handler.

use crate::ffi::*;
use core_foundation_sys::base::{CFRelease, CFTypeRef};
use core_foundation_sys::mach_port::{CFMachPortCreateRunLoopSource, CFMachPortInvalidate, CFMachPortRef};
use core_foundation_sys::runloop::{kCFRunLoopCommonModes, CFRunLoopAddSource, CFRunLoopGetMain};
use parking_lot::Mutex;
use std::ffi::{c_ulong, c_void};
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use veil_core::input::{HookVerdict, InputEvent, Key, Modifiers, Replay};
use veil_core::platform::InputHandler;
use veil_core::types::Point;

/// "VEIL" — marks events Veil injects.
pub const MAGIC: i64 = 0x5645_494C;
const EXPECT_FOR: Duration = Duration::from_millis(500);

/// macOS virtual key codes.
pub mod keycode {
    pub const RETURN: u16 = 36;
    pub const KEYPAD_ENTER: u16 = 76;
    pub const A: u16 = 0;
    pub const V: u16 = 9;
    pub const HELP_INSERT: u16 = 114;
    pub const DELETE: u16 = 51;
    pub const FORWARD_DELETE: u16 = 117;
    pub const ESCAPE: u16 = 53;
    pub const TAB: u16 = 48;
}

pub fn key_from_code(code: u16) -> Key {
    match code {
        keycode::RETURN => Key::Enter,
        keycode::KEYPAD_ENTER => Key::KeypadEnter,
        keycode::A => Key::A,
        keycode::V => Key::V,
        keycode::HELP_INSERT => Key::Insert,
        keycode::DELETE => Key::Backspace,
        keycode::FORWARD_DELETE => Key::Delete,
        keycode::ESCAPE => Key::Escape,
        keycode::TAB => Key::Tab,
        other => Key::Other(other as u32),
    }
}

pub fn code_from_key(key: Key) -> u16 {
    match key {
        Key::Enter => keycode::RETURN,
        Key::KeypadEnter => keycode::KEYPAD_ENTER,
        Key::A => keycode::A,
        Key::V => keycode::V,
        Key::Insert => keycode::HELP_INSERT,
        Key::Backspace => keycode::DELETE,
        Key::Delete => keycode::FORWARD_DELETE,
        Key::Escape => keycode::ESCAPE,
        Key::Tab => keycode::TAB,
        Key::Other(c) => c as u16,
    }
}

pub fn mods_from_flags(flags: CGEventFlags) -> Modifiers {
    Modifiers {
        shift: flags & kCGEventFlagMaskShift != 0,
        ctrl: flags & kCGEventFlagMaskControl != 0,
        alt: flags & kCGEventFlagMaskAlternate != 0,
        meta: flags & kCGEventFlagMaskCommand != 0,
    }
}

pub fn flags_from_mods(m: Modifiers) -> CGEventFlags {
    let mut f = 0;
    if m.shift {
        f |= kCGEventFlagMaskShift;
    }
    if m.ctrl {
        f |= kCGEventFlagMaskControl;
    }
    if m.alt {
        f |= kCGEventFlagMaskAlternate;
    }
    if m.meta {
        f |= kCGEventFlagMaskCommand;
    }
    f
}

struct Expected {
    etype: CGEventType,
    code: i64,
    until: Instant,
}

/// Posts tagged events.
pub struct Injector {
    source: AtomicPtr<c_void>,
    expected: Mutex<Vec<Expected>>,
}

impl Injector {
    pub fn new() -> Injector {
        let source = unsafe { CGEventSourceCreate(kCGEventSourceStateHIDSystemState) };
        if !source.is_null() {
            unsafe { CGEventSourceSetUserData(source, MAGIC) };
        }
        Injector { source: AtomicPtr::new(source), expected: Mutex::new(Vec::new()) }
    }

    fn source(&self) -> CGEventSourceRef {
        self.source.load(Ordering::Relaxed)
    }

    /// True (and consumed) when the event is one Veil injected.
    pub fn is_ours(&self, event: CGEventRef, etype: CGEventType) -> bool {
        let tagged = unsafe { CGEventGetIntegerValueField(event, kCGEventSourceUserData) } == MAGIC;
        let code = if etype == kCGEventKeyDown || etype == kCGEventKeyUp {
            unsafe { CGEventGetIntegerValueField(event, kCGKeyboardEventKeycode) }
        } else {
            -1
        };
        let now = Instant::now();
        let mut exp = self.expected.lock();
        exp.retain(|e| e.until > now);
        if let Some(i) = exp.iter().position(|e| e.etype == etype && e.code == code) {
            exp.remove(i);
            return true;
        }
        tagged
    }

    /// Tag, remember and post an event we created (consumes the +1 reference).
    fn post(&self, event: CGEventRef, etype: CGEventType, code: i64) {
        if event.is_null() {
            return;
        }
        unsafe { CGEventSetIntegerValueField(event, kCGEventSourceUserData, MAGIC) };
        self.expected.lock().push(Expected { etype, code, until: Instant::now() + EXPECT_FOR });
        unsafe {
            CGEventPost(kCGHIDEventTap, event);
            CFRelease(event as CFTypeRef);
        }
    }

    fn key_event(&self, key: Key, mods: Modifiers, text: Option<&str>, down: bool) {
        let code = code_from_key(key);
        let ev = unsafe { CGEventCreateKeyboardEvent(self.source(), code, down) };
        if ev.is_null() {
            return;
        }
        unsafe { CGEventSetFlags(ev, flags_from_mods(mods)) };
        // Keep the exact characters for ordinary keys (dead keys, layouts with option-characters).
        if let (Key::Other(_), Some(t)) = (key, text) {
            if !t.is_empty() && t.chars().all(|c| !c.is_control()) {
                let utf16: Vec<u16> = t.encode_utf16().collect();
                unsafe { CGEventKeyboardSetUnicodeString(ev, utf16.len() as c_ulong, utf16.as_ptr()) };
            }
        }
        let etype = if down { kCGEventKeyDown } else { kCGEventKeyUp };
        self.post(ev, etype, code as i64);
    }

    fn click(&self, at: Point, clicks: u32) {
        let pos = CGPoint { x: at.x, y: at.y };
        for etype in [kCGEventLeftMouseDown, kCGEventLeftMouseUp] {
            let ev = unsafe { CGEventCreateMouseEvent(self.source(), etype, pos, kCGMouseButtonLeft) };
            if ev.is_null() {
                continue;
            }
            unsafe {
                CGEventSetIntegerValueField(ev, kCGMouseEventClickState, clicks.max(1) as i64);
                CGEventSetFlags(ev, 0);
            }
            self.post(ev, etype, -1);
        }
    }

    pub fn replay(&self, r: &Replay) {
        match r {
            Replay::Key { key, mods, text } => {
                self.key_event(*key, *mods, text.as_deref(), true);
                self.key_event(*key, *mods, text.as_deref(), false);
            }
            Replay::KeyDown { key, mods, text } => self.key_event(*key, *mods, text.as_deref(), true),
            Replay::KeyUp { key, mods } => self.key_event(*key, *mods, None, false),
            Replay::Click { at, clicks } => self.click(*at, *clicks),
        }
    }
}

impl Drop for Injector {
    fn drop(&mut self) {
        let s = self.source();
        if !s.is_null() {
            unsafe { CFRelease(s as CFTypeRef) };
        }
    }
}

/// Called on the main thread for every mouse-down before the handler (drag pasteboard bookkeeping).
pub type MouseDownHook = Box<dyn Fn() + Send + Sync>;

/// State the C callback needs. Leaked into the tap's `user_info` for the tap's lifetime.
pub struct TapContext {
    pub handler: Arc<dyn InputHandler>,
    pub injector: Arc<Injector>,
    pub on_mouse_down: MouseDownHook,
    pub port: AtomicPtr<c_void>,
}

/// An installed tap. Dropping it disables and invalidates the tap and frees the context.
pub struct Tap {
    port: CFMachPortRef,
    ctx: *mut TapContext,
}

impl Tap {
    /// Create the session tap and add it to the main run loop. Fails without Accessibility permission.
    pub fn install(ctx: TapContext) -> anyhow::Result<Tap> {
        let mask: CGEventMask =
            [kCGEventKeyDown, kCGEventKeyUp, kCGEventLeftMouseDown, kCGEventLeftMouseDragged, kCGEventLeftMouseUp]
                .iter()
                .fold(0, |m, t| m | (1u64 << t));
        let ctx = Box::into_raw(Box::new(ctx));
        let port = unsafe {
            CGEventTapCreate(
                kCGSessionEventTap,
                kCGHeadInsertEventTap,
                kCGEventTapOptionDefault,
                mask,
                tap_callback,
                ctx as *mut c_void,
            )
        };
        if port.is_null() {
            drop(unsafe { Box::from_raw(ctx) });
            anyhow::bail!("could not create the event tap (Accessibility permission missing?)");
        }
        unsafe {
            (*ctx).port.store(port as *mut c_void, Ordering::SeqCst);
            let source = CFMachPortCreateRunLoopSource(std::ptr::null(), port, 0);
            if source.is_null() {
                CFMachPortInvalidate(port);
                CFRelease(port as CFTypeRef);
                drop(Box::from_raw(ctx));
                anyhow::bail!("could not create a run-loop source for the event tap");
            }
            CFRunLoopAddSource(CFRunLoopGetMain(), source, kCFRunLoopCommonModes);
            CFRelease(source as CFTypeRef);
            CGEventTapEnable(port, true);
        }
        Ok(Tap { port, ctx })
    }
}

impl Drop for Tap {
    fn drop(&mut self) {
        unsafe {
            CGEventTapEnable(self.port, false);
            CFMachPortInvalidate(self.port);
            CFRelease(self.port as CFTypeRef);
            drop(Box::from_raw(self.ctx));
        }
    }
}

fn translate(event: CGEventRef, etype: CGEventType) -> Option<InputEvent> {
    unsafe {
        match etype {
            kCGEventKeyDown | kCGEventKeyUp => {
                let code = CGEventGetIntegerValueField(event, kCGKeyboardEventKeycode) as u16;
                let key = key_from_code(code);
                let mods = mods_from_flags(CGEventGetFlags(event));
                if etype == kCGEventKeyUp {
                    return Some(InputEvent::KeyUp { key, mods });
                }
                let mut buf = [0u16; 32];
                let mut len: c_ulong = 0;
                CGEventKeyboardGetUnicodeString(event, buf.len() as c_ulong, &mut len, buf.as_mut_ptr());
                let len = (len as usize).min(buf.len());
                let text = (len > 0).then(|| String::from_utf16_lossy(&buf[..len]));
                Some(InputEvent::KeyDown { key, mods, text })
            }
            kCGEventLeftMouseDown => {
                let p = CGEventGetLocation(event);
                let clicks = CGEventGetIntegerValueField(event, kCGMouseEventClickState).max(1) as u32;
                Some(InputEvent::MouseDown { at: Point::new(p.x, p.y), clicks })
            }
            kCGEventLeftMouseDragged => {
                let p = CGEventGetLocation(event);
                Some(InputEvent::MouseDrag { at: Point::new(p.x, p.y) })
            }
            kCGEventLeftMouseUp => {
                let p = CGEventGetLocation(event);
                Some(InputEvent::MouseUp { at: Point::new(p.x, p.y) })
            }
            _ => None,
        }
    }
}

extern "C" fn tap_callback(
    _proxy: CGEventTapProxy,
    etype: CGEventType,
    event: CGEventRef,
    user_info: *mut c_void,
) -> CGEventRef {
    if user_info.is_null() {
        return event;
    }
    let ctx = unsafe { &*(user_info as *const TapContext) };

    if etype == kCGEventTapDisabledByTimeout || etype == kCGEventTapDisabledByUserInput {
        let port = ctx.port.load(Ordering::SeqCst);
        if !port.is_null() {
            unsafe { CGEventTapEnable(port as CFMachPortRef, true) };
        }
        tracing::warn!("event tap was disabled by the system ({etype:#x}); re-enabled");
        return event;
    }
    if event.is_null() || ctx.injector.is_ours(event, etype) {
        return event;
    }
    let Some(input) = translate(event, etype) else { return event };

    let verdict = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        objc2::rc::autoreleasepool(|_| {
            if etype == kCGEventLeftMouseDown {
                (ctx.on_mouse_down)();
            }
            ctx.handler.on_input(&input)
        })
    }))
    .unwrap_or(HookVerdict::Pass);

    match verdict {
        HookVerdict::Pass => event,
        HookVerdict::Swallow => std::ptr::null_mut(),
    }
}
