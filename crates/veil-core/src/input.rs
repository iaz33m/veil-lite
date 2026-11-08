use crate::types::Point;

/// Keys the core cares about. Everything else is `Other` with the platform's key code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    Enter,
    KeypadEnter,
    A,
    V,
    Insert,
    Backspace,
    Delete,
    Escape,
    Tab,
    Other(u32),
}

impl Key {
    pub fn is_enter(&self) -> bool {
        matches!(self, Key::Enter | Key::KeypadEnter)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    /// Command on macOS, Windows/Super key elsewhere.
    pub meta: bool,
}

impl Modifiers {
    pub const NONE: Modifiers = Modifiers { shift: false, ctrl: false, alt: false, meta: false };

    /// The platform's shortcut modifier: Command on macOS, Control elsewhere.
    pub fn primary(&self) -> bool {
        if cfg!(target_os = "macos") {
            self.meta
        } else {
            self.ctrl
        }
    }

    /// Only the primary modifier is held.
    pub fn only_primary(&self) -> bool {
        let others = if cfg!(target_os = "macos") {
            self.ctrl || self.alt || self.shift
        } else {
            self.meta || self.alt || self.shift
        };
        self.primary() && !others
    }

    pub fn any(&self) -> bool {
        self.shift || self.ctrl || self.alt || self.meta
    }

    /// Modifiers for the platform's primary shortcut (e.g. Cmd+V / Ctrl+V).
    pub fn primary_only() -> Modifiers {
        if cfg!(target_os = "macos") {
            Modifiers { meta: true, ..Modifiers::NONE }
        } else {
            Modifiers { ctrl: true, ..Modifiers::NONE }
        }
    }
}

/// A user input event as seen by the platform hook, before it reaches any app.
#[derive(Clone, Debug, PartialEq)]
pub enum InputEvent {
    KeyDown {
        key: Key,
        mods: Modifiers,
        /// Characters the key produces with the current layout, if any.
        text: Option<String>,
    },
    KeyUp {
        key: Key,
        mods: Modifiers,
    },
    MouseDown {
        at: Point,
        clicks: u32,
    },
    MouseDrag {
        at: Point,
    },
    MouseUp {
        at: Point,
    },
}

/// What the hook does with the event right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookVerdict {
    /// Deliver the event normally.
    Pass,
    /// Swallow it; the core will replay it (or not) later.
    Swallow,
}

/// Input the platform re-injects, tagged as Veil's own so the hook lets it through.
#[derive(Clone, Debug, PartialEq)]
pub enum Replay {
    /// Key press (down + up).
    Key { key: Key, mods: Modifiers, text: Option<String> },
    /// A single key-down (queued typing replays down and up separately to keep ordering).
    KeyDown { key: Key, mods: Modifiers, text: Option<String> },
    KeyUp { key: Key, mods: Modifiers },
    /// Click (down + up) at a point.
    Click { at: Point, clicks: u32 },
}

impl Replay {
    /// The replay equivalent of a queued event, if it can be replayed.
    pub fn from_event(ev: &InputEvent) -> Option<Replay> {
        match ev {
            InputEvent::KeyDown { key, mods, text } => {
                Some(Replay::KeyDown { key: *key, mods: *mods, text: text.clone() })
            }
            InputEvent::KeyUp { key, mods } => Some(Replay::KeyUp { key: *key, mods: *mods }),
            _ => None,
        }
    }

    /// Paste shortcut for the platform.
    pub fn paste() -> Replay {
        Replay::Key { key: Key::V, mods: Modifiers::primary_only(), text: None }
    }

    pub fn escape() -> Replay {
        Replay::Key { key: Key::Escape, mods: Modifiers::NONE, text: None }
    }

    pub fn select_all() -> Replay {
        Replay::Key { key: Key::A, mods: Modifiers::primary_only(), text: None }
    }
}
