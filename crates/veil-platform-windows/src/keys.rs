//! Virtual-key mapping, modifier tracking and typed-character translation.

use std::sync::atomic::{AtomicU32, Ordering};
use veil_core::{Key, Modifiers};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, GetKeyboardLayout, ToUnicodeEx, VK_A, VK_BACK, VK_CAPITAL, VK_CONTROL, VK_DELETE, VK_ESCAPE,
    VK_INSERT, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_MENU, VK_RCONTROL, VK_RETURN, VK_RMENU, VK_RSHIFT,
    VK_RWIN, VK_SHIFT, VK_TAB, VK_V,
};
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

/// Bit set in `Key::Other` codes for keys with the extended flag (arrows, right Ctrl/Alt, navigation
/// cluster, keypad divide, …) so replays restore it.
pub const EXTENDED_BIT: u32 = 0x100;

const SHIFT_L: u32 = 1;
const SHIFT_R: u32 = 2;
const CTRL_L: u32 = 4;
const CTRL_R: u32 = 8;
const ALT_L: u32 = 16;
const ALT_R: u32 = 32;
const WIN_L: u32 = 64;
const WIN_R: u32 = 128;

/// Which modifier keys are down, as a bit set.
#[derive(Default)]
pub struct ModTracker(AtomicU32);

impl ModTracker {
    pub fn update(&self, vk: u32, down: bool) {
        let bit = match vk as u16 {
            v if v == VK_LSHIFT.0 || v == VK_SHIFT.0 => SHIFT_L,
            v if v == VK_RSHIFT.0 => SHIFT_R,
            v if v == VK_LCONTROL.0 || v == VK_CONTROL.0 => CTRL_L,
            v if v == VK_RCONTROL.0 => CTRL_R,
            v if v == VK_LMENU.0 || v == VK_MENU.0 => ALT_L,
            v if v == VK_RMENU.0 => ALT_R,
            v if v == VK_LWIN.0 => WIN_L,
            v if v == VK_RWIN.0 => WIN_R,
            _ => return,
        };
        if down {
            self.0.fetch_or(bit, Ordering::Relaxed);
        } else {
            self.0.fetch_and(!bit, Ordering::Relaxed);
        }
    }

    pub fn mods(&self) -> Modifiers {
        let b = self.0.load(Ordering::Relaxed);
        Modifiers {
            shift: b & (SHIFT_L | SHIFT_R) != 0,
            ctrl: b & (CTRL_L | CTRL_R) != 0,
            alt: b & (ALT_L | ALT_R) != 0,
            meta: b & (WIN_L | WIN_R) != 0,
        }
    }

    /// Right Alt is held (AltGr on many layouts: the system reports LCtrl + RAlt).
    pub fn alt_gr(&self) -> bool {
        self.0.load(Ordering::Relaxed) & ALT_R != 0
    }
}

pub fn is_modifier(vk: u32) -> bool {
    matches!(vk, 0x10..=0x12 | 0xA0..=0xA5 | 0x5B | 0x5C)
}

/// Map a virtual key to the core's `Key`.
pub fn key_from_vk(vk: u32, extended: bool) -> Key {
    let v = vk as u16;
    match v {
        _ if v == VK_RETURN.0 => {
            if extended {
                Key::KeypadEnter
            } else {
                Key::Enter
            }
        }
        _ if v == VK_A.0 => Key::A,
        _ if v == VK_V.0 => Key::V,
        _ if v == VK_INSERT.0 => Key::Insert,
        _ if v == VK_BACK.0 => Key::Backspace,
        _ if v == VK_DELETE.0 => Key::Delete,
        _ if v == VK_ESCAPE.0 => Key::Escape,
        _ if v == VK_TAB.0 => Key::Tab,
        _ => Key::Other(vk | if extended { EXTENDED_BIT } else { 0 }),
    }
}

/// Virtual key and extended flag for replaying a `Key`.
pub fn vk_from_key(key: Key) -> (u16, bool) {
    match key {
        Key::Enter => (VK_RETURN.0, false),
        Key::KeypadEnter => (VK_RETURN.0, true),
        Key::A => (VK_A.0, false),
        Key::V => (VK_V.0, false),
        Key::Insert => (VK_INSERT.0, true),
        Key::Backspace => (VK_BACK.0, false),
        Key::Delete => (VK_DELETE.0, true),
        Key::Escape => (VK_ESCAPE.0, false),
        Key::Tab => (VK_TAB.0, false),
        Key::Other(code) => ((code & 0xFF) as u16, code & EXTENDED_BIT != 0),
    }
}

/// Characters a key-down produces with the foreground thread's keyboard layout.
///
/// Uses `ToUnicodeEx` flag `0x4` (Windows 10 1607+: do not change the keyboard state) so dead keys typed
/// by the user keep working. The key state is rebuilt from the tracked modifiers because the hook thread's
/// own key state does not follow the foreground thread.
pub fn char_for(vk: u32, scan: u32, mods: Modifiers, alt_gr: bool) -> Option<String> {
    if is_modifier(vk) || mods.meta || (mods.ctrl && !alt_gr) || (mods.alt && !alt_gr) {
        return None;
    }
    let mut state = [0u8; 256];
    if mods.shift {
        state[VK_SHIFT.0 as usize] = 0x80;
        state[VK_LSHIFT.0 as usize] = 0x80;
    }
    if alt_gr {
        state[VK_CONTROL.0 as usize] = 0x80;
        state[VK_LCONTROL.0 as usize] = 0x80;
        state[VK_MENU.0 as usize] = 0x80;
        state[VK_RMENU.0 as usize] = 0x80;
    }
    unsafe {
        if GetKeyState(VK_CAPITAL.0 as i32) & 1 != 0 {
            state[VK_CAPITAL.0 as usize] = 0x01;
        }
        let fg = GetForegroundWindow();
        let tid = windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(fg, None);
        let hkl = GetKeyboardLayout(tid);
        let mut buf = [0u16; 8];
        let n = ToUnicodeEx(vk, scan, &state, &mut buf, 0x4, Some(hkl));
        if n <= 0 {
            return None;
        }
        let s = String::from_utf16_lossy(&buf[..n as usize]);
        if s.chars().all(|c| c.is_control()) {
            return None;
        }
        Some(s)
    }
}
