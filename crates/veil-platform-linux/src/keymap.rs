//! evdev key codes ↔ Veil keys, modifier tracking and key → text.
//!
//! Text uses a US-layout fallback ([`UsKeymap`]). The [`Keymap`] trait is the seam for a proper
//! xkbcommon-based implementation (reading the session's layout) later; the core only needs text for
//! the typed-message buffer, which is a fallback for apps that don't expose their composer over AT-SPI.

use veil_core::input::{Key, Modifiers};

// linux/input-event-codes.h
pub const EV_SYN: u16 = 0x00;
pub const EV_KEY: u16 = 0x01;
pub const EV_REL: u16 = 0x02;
pub const EV_ABS: u16 = 0x03;
pub const EV_MSC: u16 = 0x04;
pub const SYN_REPORT: u16 = 0;
pub const MSC_SCAN: u16 = 4;
pub const REL_X: u16 = 0x00;
pub const REL_Y: u16 = 0x01;
pub const REL_HWHEEL: u16 = 0x06;
pub const REL_WHEEL: u16 = 0x08;
pub const REL_WHEEL_HI_RES: u16 = 0x0b;
pub const REL_HWHEEL_HI_RES: u16 = 0x0c;

pub const KEY_ESC: u16 = 1;
pub const KEY_BACKSPACE: u16 = 14;
pub const KEY_TAB: u16 = 15;
pub const KEY_ENTER: u16 = 28;
pub const KEY_LEFTCTRL: u16 = 29;
pub const KEY_A: u16 = 30;
pub const KEY_LEFTSHIFT: u16 = 42;
pub const KEY_V: u16 = 47;
pub const KEY_RIGHTSHIFT: u16 = 54;
pub const KEY_LEFTALT: u16 = 56;
pub const KEY_SPACE: u16 = 57;
pub const KEY_CAPSLOCK: u16 = 58;
pub const KEY_KPENTER: u16 = 96;
pub const KEY_RIGHTCTRL: u16 = 97;
pub const KEY_RIGHTALT: u16 = 100;
pub const KEY_INSERT: u16 = 110;
pub const KEY_DELETE: u16 = 111;
pub const KEY_LEFTMETA: u16 = 125;
pub const KEY_RIGHTMETA: u16 = 126;
pub const BTN_LEFT: u16 = 0x110;
pub const BTN_RIGHT: u16 = 0x111;
pub const BTN_MIDDLE: u16 = 0x112;
pub const BTN_TOUCH: u16 = 0x14a;
pub const BTN_MISC_FIRST: u16 = 0x100;
pub const BTN_LAST: u16 = 0x15f;

/// Pointer buttons and other `BTN_*` codes live in 0x100..=0x15f (and joystick/gamepad ranges above
/// that we never grab); everything else in EV_KEY is a keyboard key.
pub fn is_button(code: u16) -> bool {
    (BTN_MISC_FIRST..=BTN_LAST).contains(&code) || (0x2c0..=0x2e7).contains(&code)
}

pub fn is_modifier(code: u16) -> bool {
    matches!(
        code,
        KEY_LEFTCTRL
            | KEY_RIGHTCTRL
            | KEY_LEFTSHIFT
            | KEY_RIGHTSHIFT
            | KEY_LEFTALT
            | KEY_RIGHTALT
            | KEY_LEFTMETA
            | KEY_RIGHTMETA
            | KEY_CAPSLOCK
    )
}

/// evdev code → the keys the core cares about.
pub fn veil_key(code: u16) -> Key {
    match code {
        KEY_ENTER => Key::Enter,
        KEY_KPENTER => Key::KeypadEnter,
        KEY_A => Key::A,
        KEY_V => Key::V,
        KEY_INSERT => Key::Insert,
        KEY_BACKSPACE => Key::Backspace,
        KEY_DELETE => Key::Delete,
        KEY_ESC => Key::Escape,
        KEY_TAB => Key::Tab,
        other => Key::Other(other as u32),
    }
}

/// Veil key → evdev code (for replays). `Key::Other` always carries an evdev code on Linux.
pub fn evdev_code(key: Key) -> u16 {
    match key {
        Key::Enter => KEY_ENTER,
        Key::KeypadEnter => KEY_KPENTER,
        Key::A => KEY_A,
        Key::V => KEY_V,
        Key::Insert => KEY_INSERT,
        Key::Backspace => KEY_BACKSPACE,
        Key::Delete => KEY_DELETE,
        Key::Escape => KEY_ESC,
        Key::Tab => KEY_TAB,
        Key::Other(c) => c as u16,
    }
}

/// Codes to press for a modifier set (left-hand keys).
pub fn modifier_codes(mods: Modifiers) -> Vec<u16> {
    let mut v = Vec::new();
    if mods.ctrl {
        v.push(KEY_LEFTCTRL);
    }
    if mods.shift {
        v.push(KEY_LEFTSHIFT);
    }
    if mods.alt {
        v.push(KEY_LEFTALT);
    }
    if mods.meta {
        v.push(KEY_LEFTMETA);
    }
    v
}

/// Modifier state tracked from key events of one logical keyboard.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ModState {
    pub lshift: bool,
    pub rshift: bool,
    pub lctrl: bool,
    pub rctrl: bool,
    pub alt: bool,
    pub altgr: bool,
    pub lmeta: bool,
    pub rmeta: bool,
    pub caps_lock: bool,
}

impl ModState {
    /// Update from a key event (`value`: 1 press, 0 release, 2 repeat).
    pub fn update(&mut self, code: u16, value: i32) {
        let down = value != 0;
        match code {
            KEY_LEFTSHIFT => self.lshift = down,
            KEY_RIGHTSHIFT => self.rshift = down,
            KEY_LEFTCTRL => self.lctrl = down,
            KEY_RIGHTCTRL => self.rctrl = down,
            KEY_LEFTALT => self.alt = down,
            KEY_RIGHTALT => self.altgr = down,
            KEY_LEFTMETA => self.lmeta = down,
            KEY_RIGHTMETA => self.rmeta = down,
            KEY_CAPSLOCK if value == 1 => self.caps_lock = !self.caps_lock,
            _ => {}
        }
    }

    pub fn shift(&self) -> bool {
        self.lshift || self.rshift
    }

    pub fn ctrl(&self) -> bool {
        self.lctrl || self.rctrl
    }

    pub fn meta(&self) -> bool {
        self.lmeta || self.rmeta
    }

    /// Modifiers as the core sees them. AltGr is a level shift for typing, not Alt.
    pub fn modifiers(&self) -> Modifiers {
        Modifiers { shift: self.shift(), ctrl: self.ctrl(), alt: self.alt, meta: self.meta() }
    }
}

/// Key → produced text.
pub trait Keymap: Send + Sync {
    fn text(&self, code: u16, state: &ModState) -> Option<String>;
}

/// US QWERTY fallback. Num Lock is assumed on for the keypad.
#[derive(Default)]
pub struct UsKeymap;

const LETTERS: &[(u16, char)] = &[
    (16, 'q'),
    (17, 'w'),
    (18, 'e'),
    (19, 'r'),
    (20, 't'),
    (21, 'y'),
    (22, 'u'),
    (23, 'i'),
    (24, 'o'),
    (25, 'p'),
    (30, 'a'),
    (31, 's'),
    (32, 'd'),
    (33, 'f'),
    (34, 'g'),
    (35, 'h'),
    (36, 'j'),
    (37, 'k'),
    (38, 'l'),
    (44, 'z'),
    (45, 'x'),
    (46, 'c'),
    (47, 'v'),
    (48, 'b'),
    (49, 'n'),
    (50, 'm'),
];

/// (code, unshifted, shifted)
const SYMBOLS: &[(u16, char, char)] = &[
    (2, '1', '!'),
    (3, '2', '@'),
    (4, '3', '#'),
    (5, '4', '$'),
    (6, '5', '%'),
    (7, '6', '^'),
    (8, '7', '&'),
    (9, '8', '*'),
    (10, '9', '('),
    (11, '0', ')'),
    (12, '-', '_'),
    (13, '=', '+'),
    (26, '[', '{'),
    (27, ']', '}'),
    (39, ';', ':'),
    (40, '\'', '"'),
    (41, '`', '~'),
    (43, '\\', '|'),
    (51, ',', '<'),
    (52, '.', '>'),
    (53, '/', '?'),
    (86, '\\', '|'),
];

const KEYPAD: &[(u16, char)] = &[
    (71, '7'),
    (72, '8'),
    (73, '9'),
    (74, '-'),
    (75, '4'),
    (76, '5'),
    (77, '6'),
    (78, '+'),
    (79, '1'),
    (80, '2'),
    (81, '3'),
    (82, '0'),
    (83, '.'),
    (55, '*'),
    (98, '/'),
];

impl UsKeymap {
    /// Every code this keymap can type (used to give the virtual device all the keys replays need).
    pub fn codes() -> impl Iterator<Item = u16> {
        LETTERS
            .iter()
            .map(|(c, _)| *c)
            .chain(SYMBOLS.iter().map(|(c, _, _)| *c))
            .chain(KEYPAD.iter().map(|(c, _)| *c))
            .chain([KEY_SPACE])
    }
}

impl Keymap for UsKeymap {
    fn text(&self, code: u16, state: &ModState) -> Option<String> {
        if code == KEY_SPACE {
            return Some(" ".into());
        }
        if let Some((_, ch)) = LETTERS.iter().find(|(c, _)| *c == code) {
            let upper = state.shift() ^ state.caps_lock;
            return Some(if upper { ch.to_ascii_uppercase() } else { *ch }.to_string());
        }
        if let Some((_, lo, hi)) = SYMBOLS.iter().find(|(c, _, _)| *c == code) {
            return Some(if state.shift() { *hi } else { *lo }.to_string());
        }
        KEYPAD.iter().find(|(c, _)| *c == code).map(|(_, ch)| ch.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_round_trip() {
        for code in
            [KEY_ENTER, KEY_KPENTER, KEY_A, KEY_V, KEY_INSERT, KEY_BACKSPACE, KEY_DELETE, KEY_ESC, KEY_TAB, 44, 200]
        {
            assert_eq!(evdev_code(veil_key(code)), code);
        }
        assert_eq!(veil_key(KEY_ENTER), Key::Enter);
        assert_eq!(veil_key(45), Key::Other(45));
    }

    #[test]
    fn modifiers_are_tracked() {
        let mut s = ModState::default();
        s.update(KEY_RIGHTCTRL, 1);
        s.update(KEY_LEFTSHIFT, 1);
        assert_eq!(s.modifiers(), Modifiers { ctrl: true, shift: true, ..Modifiers::NONE });
        s.update(KEY_RIGHTCTRL, 0);
        s.update(KEY_RIGHTALT, 1);
        let m = s.modifiers();
        assert!(!m.ctrl && !m.alt && m.shift, "AltGr is not Alt");
        s.update(KEY_CAPSLOCK, 1);
        s.update(KEY_CAPSLOCK, 2);
        s.update(KEY_CAPSLOCK, 0);
        assert!(s.caps_lock);
        s.update(KEY_CAPSLOCK, 1);
        assert!(!s.caps_lock);
    }

    #[test]
    fn us_layout_text() {
        let km = UsKeymap;
        let mut s = ModState::default();
        assert_eq!(km.text(KEY_A, &s).as_deref(), Some("a"));
        assert_eq!(km.text(2, &s).as_deref(), Some("1"));
        assert_eq!(km.text(KEY_SPACE, &s).as_deref(), Some(" "));
        assert_eq!(km.text(79, &s).as_deref(), Some("1"));
        assert_eq!(km.text(KEY_ENTER, &s), None);
        assert_eq!(km.text(KEY_LEFTCTRL, &s), None);
        s.update(KEY_LEFTSHIFT, 1);
        assert_eq!(km.text(KEY_A, &s).as_deref(), Some("A"));
        assert_eq!(km.text(3, &s).as_deref(), Some("@"));
        s.update(KEY_CAPSLOCK, 1);
        assert_eq!(km.text(KEY_A, &s).as_deref(), Some("a"), "shift cancels caps lock for letters");
        s.update(KEY_LEFTSHIFT, 0);
        assert_eq!(km.text(KEY_A, &s).as_deref(), Some("A"));
        assert_eq!(km.text(3, &s).as_deref(), Some("2"), "caps lock doesn't shift symbols");
    }

    #[test]
    fn modifier_codes_for_replays() {
        assert_eq!(modifier_codes(Modifiers::primary_only()), vec![KEY_LEFTCTRL]);
        assert!(modifier_codes(Modifiers::NONE).is_empty());
        assert!(is_button(BTN_LEFT) && !is_button(KEY_A));
        assert!(is_modifier(KEY_RIGHTMETA) && !is_modifier(KEY_V));
    }
}
