//! Low-level keyboard and mouse hooks.
//!
//! Hook procedures run on the thread that installed them, inside its message loop. They only translate the
//! event, call the core's handler (which decides in microseconds) and return. Events carrying
//! [`crate::inject::MAGIC`] are Veil's own replays and pass straight through without reaching the handler.

use crate::inject::MAGIC;
use crate::keys;
use crate::platform::{Inner, INNER};
use crate::util::{hwnd_val, root_at};
use std::sync::atomic::Ordering;
use veil_core::{HookVerdict, InputEvent, Point};
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{
    GetCurrentThread, GetCurrentThreadId, SetThreadPriority, THREAD_PRIORITY_HIGHEST,
};
use windows::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime;
use windows::Win32::UI::WindowsAndMessaging::*;

const HC_ACTION: i32 = 0;

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION {
        if let Some(inner) = INNER.get() {
            let kb = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
            let swallow = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| on_key(inner, wparam.0 as u32, kb)))
                .unwrap_or(false);
            if swallow {
                return LRESULT(1);
            }
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION {
        if let Some(inner) = INNER.get() {
            let ms = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
            let swallow =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| on_mouse(inner, wparam.0 as u32, ms)))
                    .unwrap_or(false);
            if swallow {
                return LRESULT(1);
            }
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

fn dispatch(inner: &Inner, ev: &InputEvent) -> bool {
    let handler = inner.handler.read().clone();
    match handler {
        Some(h) => h.on_input(ev) == HookVerdict::Swallow,
        None => false,
    }
}

/// Returns true to swallow.
fn on_key(inner: &Inner, msg: u32, kb: &KBDLLHOOKSTRUCT) -> bool {
    let down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
    let up = msg == WM_KEYUP || msg == WM_SYSKEYUP;
    if !down && !up {
        return false;
    }
    let vk = kb.vkCode;
    if kb.dwExtraInfo == MAGIC {
        inner.logical.update(vk, down);
        return false;
    }
    inner.physical.update(vk, down);
    let mods = inner.physical.mods();
    let extended = kb.flags.contains(LLKHF_EXTENDED);
    let key = keys::key_from_vk(vk, extended);
    let ev = if down {
        InputEvent::KeyDown { key, mods, text: keys::char_for(vk, kb.scanCode, mods, inner.physical.alt_gr()) }
    } else {
        InputEvent::KeyUp { key, mods }
    };
    let swallow = dispatch(inner, &ev);
    if !swallow {
        inner.logical.update(vk, down);
    }
    swallow
}

fn on_mouse(inner: &Inner, msg: u32, ms: &MSLLHOOKSTRUCT) -> bool {
    if ms.dwExtraInfo == MAGIC {
        return false;
    }
    let at = Point::new(ms.pt.x as f64, ms.pt.y as f64);
    let ev = match msg {
        WM_LBUTTONDOWN => {
            let clicks = {
                let mut m = inner.mouse.lock();
                let dbl_time = unsafe { GetDoubleClickTime() };
                let (dx, dy) = unsafe { (GetSystemMetrics(SM_CXDOUBLECLK) / 2, GetSystemMetrics(SM_CYDOUBLECLK) / 2) };
                let close =
                    (ms.pt.x - m.last_down_pt.0).abs() <= dx.max(2) && (ms.pt.y - m.last_down_pt.1).abs() <= dy.max(2);
                m.clicks = if close && ms.time.wrapping_sub(m.last_down_time) <= dbl_time { m.clicks + 1 } else { 1 };
                m.last_down_time = ms.time;
                m.last_down_pt = (ms.pt.x, ms.pt.y);
                m.left_down = true;
                m.down_seq += 1;
                m.down_at = at;
                m.down_root = hwnd_val(root_at(ms.pt.x, ms.pt.y));
                m.clicks
            };
            InputEvent::MouseDown { at, clicks }
        }
        WM_MOUSEMOVE => {
            if !inner.mouse.lock().left_down {
                return false;
            }
            InputEvent::MouseDrag { at }
        }
        WM_LBUTTONUP => {
            inner.mouse.lock().left_down = false;
            InputEvent::MouseUp { at }
        }
        _ => return false,
    };
    dispatch(inner, &ev)
}

/// Install the hooks on the current thread and pump messages until `WM_QUIT`.
pub fn run_loop(inner: &Inner) -> anyhow::Result<()> {
    unsafe {
        let tid = GetCurrentThreadId();
        // Make sure the thread has a message queue before anyone posts WM_QUIT to it.
        let mut msg = MSG::default();
        let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);
        inner.hook_thread.store(tid, Ordering::Release);
        if inner.stopping.load(Ordering::Acquire) {
            return Ok(());
        }
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST);
        let module = GetModuleHandleW(None)?;
        let kb = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), Some(module.into()), 0)?;
        let mouse = match SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), Some(module.into()), 0) {
            Ok(h) => h,
            Err(e) => {
                let _ = UnhookWindowsHookEx(kb);
                return Err(e.into());
            }
        };
        tracing::info!("Windows input hooks installed");
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let _ = UnhookWindowsHookEx(kb);
        let _ = UnhookWindowsHookEx(mouse);
        inner.hook_thread.store(0, Ordering::Release);
        tracing::info!("Windows input hooks removed");
        Ok(())
    }
}

/// Ask the hook thread to leave its loop.
pub fn stop(inner: &Inner) {
    inner.stopping.store(true, Ordering::Release);
    let tid = inner.hook_thread.load(Ordering::Acquire);
    if tid != 0 {
        unsafe {
            let _ = PostThreadMessageW(tid, WM_QUIT, WPARAM(0), LPARAM(0));
        }
    }
}
