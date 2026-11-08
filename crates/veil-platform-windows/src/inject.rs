//! Tagged input injection with `SendInput`.

use crate::keys::{self, ModTracker};
use std::mem::size_of;
use veil_core::{Modifiers, Point, Replay};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    MapVirtualKeyW, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, MAPVK_VK_TO_VSC, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEINPUT,
    VIRTUAL_KEY, VK_CONTROL, VK_ESCAPE, VK_MENU, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::SetCursorPos;

/// `dwExtraInfo` of every event Veil injects ("VEIL"). The hooks pass these straight through.
pub const MAGIC: usize = 0x5645_494C;

fn key_input(vk: u16, up: bool, extended: bool) -> INPUT {
    let mut flags = KEYBD_EVENT_FLAGS(0);
    if up {
        flags |= KEYEVENTF_KEYUP;
    }
    if extended {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    let scan = unsafe { MapVirtualKeyW(vk as u32, MAPVK_VK_TO_VSC) } as u16;
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: VIRTUAL_KEY(vk), wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: MAGIC },
        },
    }
}

fn mouse_input(flags: windows::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT { dx: 0, dy: 0, mouseData: 0, dwFlags: flags, time: 0, dwExtraInfo: MAGIC },
        },
    }
}

fn send(inputs: &[INPUT]) {
    if inputs.is_empty() {
        return;
    }
    let sent = unsafe { SendInput(inputs, size_of::<INPUT>() as i32) };
    if sent as usize != inputs.len() {
        tracing::warn!("SendInput injected {sent} of {} events (UIPI or desktop switch?)", inputs.len());
    }
}

/// Press/release modifiers so the system's logical state matches `want` around a key press, then restore.
fn wrap_modifiers(want: Modifiers, have: Modifiers, body: &mut Vec<INPUT>) -> Vec<INPUT> {
    let pairs =
        [(want.shift, have.shift, VK_SHIFT.0), (want.ctrl, have.ctrl, VK_CONTROL.0), (want.alt, have.alt, VK_MENU.0)];
    let mut out = Vec::new();
    let mut restore = Vec::new();
    for (w, h, vk) in pairs {
        if w && !h {
            out.push(key_input(vk, false, false));
            restore.push(key_input(vk, true, false));
        } else if !w && h {
            out.push(key_input(vk, true, false));
            restore.push(key_input(vk, false, false));
        }
    }
    out.append(body);
    restore.reverse();
    out.extend(restore);
    out
}

/// Inject a replay. `logical` is the modifier state the system currently believes in.
pub fn replay(r: &Replay, logical: &ModTracker) {
    match r {
        Replay::Key { key, mods, .. } => {
            let (vk, ext) = keys::vk_from_key(*key);
            let mut body = vec![key_input(vk, false, ext), key_input(vk, true, ext)];
            let inputs = wrap_modifiers(*mods, logical.mods(), &mut body);
            send(&inputs);
        }
        Replay::KeyDown { key, .. } => {
            let (vk, ext) = keys::vk_from_key(*key);
            send(&[key_input(vk, false, ext)]);
        }
        Replay::KeyUp { key, .. } => {
            let (vk, ext) = keys::vk_from_key(*key);
            send(&[key_input(vk, true, ext)]);
        }
        Replay::Click { at, clicks } => click(*at, *clicks),
    }
}

pub fn click(at: Point, clicks: u32) {
    unsafe {
        let _ = SetCursorPos(at.x.round() as i32, at.y.round() as i32);
    }
    let mut inputs = Vec::new();
    for _ in 0..clicks.max(1) {
        inputs.push(mouse_input(MOUSEEVENTF_LEFTDOWN));
        inputs.push(mouse_input(MOUSEEVENTF_LEFTUP));
    }
    send(&inputs);
}

pub fn escape() {
    send(&[key_input(VK_ESCAPE.0, false, false), key_input(VK_ESCAPE.0, true, false)]);
}

/// A lone tagged left-button release at a point (resyncs the button state after a swallowed drop).
pub fn left_up_at(at: Point) {
    unsafe {
        let _ = SetCursorPos(at.x.round() as i32, at.y.round() as i32);
    }
    send(&[mouse_input(MOUSEEVENTF_LEFTUP)]);
}

pub fn move_cursor(at: Point) {
    unsafe {
        let _ = SetCursorPos(at.x.round() as i32, at.y.round() as i32);
    }
}
