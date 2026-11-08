//! Keystroke shadow buffer: mirrors what the user types into each app, so detection works even when an
//! app exposes no text to accessibility APIs.

use crate::input::{Key, Modifiers};
use std::collections::{HashMap, HashSet};

const MAX_CHARS: usize = 4000;

#[derive(Default, Debug)]
pub struct KeystrokeBuffer {
    text: HashMap<u32, String>,
    select_all: HashSet<u32>,
}

impl KeystrokeBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply a key-down event. Returns true if the buffer changed.
    pub fn apply(&mut self, pid: u32, key: Key, mods: Modifiers, text: Option<&str>) -> bool {
        // Select all (primary+A): the next edit replaces everything.
        if key == Key::A && mods.only_primary() {
            self.select_all.insert(pid);
            return false;
        }
        let replace_all = self.select_all.remove(&pid);
        let buf = self.text.entry(pid).or_default();

        if key == Key::Backspace || key == Key::Delete {
            let word_delete = if cfg!(target_os = "macos") { mods.alt } else { mods.ctrl };
            let line_delete = cfg!(target_os = "macos") && mods.meta;
            if replace_all || line_delete {
                buf.clear();
            } else if word_delete {
                while buf.ends_with(char::is_whitespace) {
                    buf.pop();
                }
                while buf.chars().last().is_some_and(|c| !c.is_whitespace()) {
                    buf.pop();
                }
            } else {
                buf.pop();
            }
            return true;
        }

        // Shortcuts (other than plain shift typing) don't type text.
        let typing_mods = mods.meta || mods.ctrl;
        if typing_mods {
            if replace_all {
                self.select_all.insert(pid); // e.g. Cmd+A then Cmd+C keeps the selection
            }
            return false;
        }

        let produced: String = if key.is_enter() {
            if mods.shift {
                "\n".into()
            } else {
                return false; // a send, handled by the interceptor
            }
        } else if key == Key::Tab {
            return false;
        } else {
            match text {
                Some(t) => t.chars().filter(|c| is_typed_char(*c)).collect(),
                None => return false,
            }
        };
        if produced.is_empty() {
            return false;
        }
        if replace_all {
            buf.clear();
        }
        buf.push_str(&produced);
        trim_front(buf);
        true
    }

    pub fn append(&mut self, pid: u32, text: &str) {
        let replace_all = self.select_all.remove(&pid);
        let buf = self.text.entry(pid).or_default();
        if replace_all {
            buf.clear();
        }
        buf.push_str(text);
        trim_front(buf);
    }

    /// Replace with the text the app actually shows (accessibility is the source of truth when exposed).
    pub fn set(&mut self, pid: u32, text: &str) {
        let mut s = text.to_string();
        trim_front(&mut s);
        self.text.insert(pid, s);
        self.select_all.remove(&pid);
    }

    pub fn get(&self, pid: u32) -> &str {
        self.text.get(&pid).map(String::as_str).unwrap_or("")
    }

    pub fn clear(&mut self, pid: u32) {
        self.text.remove(&pid);
        self.select_all.remove(&pid);
    }
}

fn is_typed_char(c: char) -> bool {
    // Printable characters; drop control characters and the private-use range used for arrow/function keys.
    (!c.is_control() || c == '\n' || c == '\t') && !('\u{F700}'..='\u{F8FF}').contains(&c)
}

fn trim_front(s: &mut String) {
    let count = s.chars().count();
    if count > MAX_CHARS {
        let cut = s.char_indices().nth(count - MAX_CHARS).map(|(i, _)| i).unwrap_or(0);
        s.drain(..cut);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(buf: &mut KeystrokeBuffer, s: &str) {
        for c in s.chars() {
            buf.apply(1, Key::Other(0), Modifiers::NONE, Some(&c.to_string()));
        }
    }

    #[test]
    fn typing_and_backspace() {
        let mut b = KeystrokeBuffer::new();
        typed(&mut b, "confidentiax");
        b.apply(1, Key::Backspace, Modifiers::NONE, None);
        typed(&mut b, "l");
        assert_eq!(b.get(1), "confidential");
    }

    #[test]
    fn word_delete_removes_last_word() {
        let mut b = KeystrokeBuffer::new();
        typed(&mut b, "this is secret  ");
        let word = if cfg!(target_os = "macos") {
            Modifiers { alt: true, ..Modifiers::NONE }
        } else {
            Modifiers { ctrl: true, ..Modifiers::NONE }
        };
        b.apply(1, Key::Backspace, word, None);
        assert_eq!(b.get(1), "this is ");
    }

    #[test]
    fn select_all_then_typing_replaces() {
        let mut b = KeystrokeBuffer::new();
        typed(&mut b, "old text");
        b.apply(1, Key::A, Modifiers::primary_only(), None);
        typed(&mut b, "n");
        assert_eq!(b.get(1), "n");
    }

    #[test]
    fn select_all_then_backspace_clears() {
        let mut b = KeystrokeBuffer::new();
        typed(&mut b, "old text");
        b.apply(1, Key::A, Modifiers::primary_only(), None);
        b.apply(1, Key::Backspace, Modifiers::NONE, None);
        assert_eq!(b.get(1), "");
    }

    #[test]
    fn shift_enter_is_newline_and_enter_is_not_text() {
        let mut b = KeystrokeBuffer::new();
        typed(&mut b, "a");
        b.apply(1, Key::Enter, Modifiers { shift: true, ..Modifiers::NONE }, Some("\r"));
        b.apply(1, Key::Enter, Modifiers::NONE, Some("\r"));
        assert_eq!(b.get(1), "a\n");
    }

    #[test]
    fn shortcuts_and_arrow_keys_do_not_type() {
        let mut b = KeystrokeBuffer::new();
        b.apply(1, Key::Other(9), Modifiers::primary_only(), Some("c"));
        b.apply(1, Key::Other(123), Modifiers::NONE, Some("\u{F702}"));
        assert_eq!(b.get(1), "");
    }

    #[test]
    fn buffers_are_per_app_and_bounded() {
        let mut b = KeystrokeBuffer::new();
        b.append(1, &"x".repeat(5000));
        b.append(2, "y");
        assert_eq!(b.get(1).chars().count(), MAX_CHARS);
        assert_eq!(b.get(2), "y");
        b.clear(1);
        assert_eq!(b.get(1), "");
    }
}
