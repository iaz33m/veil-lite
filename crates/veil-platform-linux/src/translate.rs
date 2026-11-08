//! Pure input logic: raw evdev frames → core [`InputEvent`]s, and which raw events to re-emit.
//!
//! A *frame* is the events a device reports between two `SYN_REPORT`s. The router feeds each frame
//! through [`Translator`], asks the core about the events it cares about, then re-emits the frame minus
//! swallowed events with [`filter_frame`] (which keeps `SYN_REPORT` framing correct).

use crate::keymap::*;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};
use veil_core::input::InputEvent;
use veil_core::types::Point;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Raw {
    pub ty: u16,
    pub code: u16,
    pub value: i32,
}

impl Raw {
    pub const fn new(ty: u16, code: u16, value: i32) -> Self {
        Raw { ty, code, value }
    }

    pub const fn syn() -> Self {
        Raw { ty: EV_SYN, code: SYN_REPORT, value: 0 }
    }

    pub fn is_syn_report(&self) -> bool {
        self.ty == EV_SYN && self.code == SYN_REPORT
    }
}

/// The events to write to the virtual device for a frame, given which ones were swallowed.
///
/// * swallowed events are dropped, along with the `MSC_SCAN` that announces a swallowed key;
/// * incoming `EV_SYN` events are dropped and exactly one `SYN_REPORT` closes the output;
/// * a frame left with nothing but `MSC_SCAN` produces no output at all.
pub fn filter_frame(events: &[Raw], swallowed: &[bool]) -> Vec<Raw> {
    let mut out = Vec::with_capacity(events.len() + 1);
    for (i, ev) in events.iter().enumerate() {
        if swallowed.get(i).copied().unwrap_or(false) || ev.ty == EV_SYN {
            continue;
        }
        if ev.ty == EV_MSC && ev.code == MSC_SCAN {
            let next_key = events[i + 1..].iter().position(|e| e.ty == EV_KEY).map(|p| i + 1 + p);
            if next_key.is_some_and(|k| swallowed.get(k).copied().unwrap_or(false)) {
                continue;
            }
        }
        out.push(*ev);
    }
    if out.iter().all(|e| e.ty == EV_MSC) {
        return Vec::new();
    }
    out.push(Raw::syn());
    out
}

/// What the router does with one raw event.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    /// Not interesting to the core: re-emit.
    Pass,
    /// Swallow without asking (auto-repeat of a key whose press was swallowed).
    Drop,
    /// Ask the core.
    Ask(InputEvent),
}

const DOUBLE_CLICK: Duration = Duration::from_millis(400);
const DOUBLE_CLICK_SLOP: f64 = 4.0;

pub struct Translator {
    keymap: Arc<dyn Keymap>,
    pub mods: ModState,
    swallowed_keys: HashSet<u16>,
    last_click: Option<(Instant, Point, u32)>,
    pub left_down: bool,
    moved: bool,
}

impl Translator {
    pub fn new(keymap: Arc<dyn Keymap>) -> Self {
        Translator { keymap, mods: ModState::default(), swallowed_keys: HashSet::new(), last_click: None, left_down: false, moved: false }
    }

    /// Classify one raw event. `pos` gives the pointer position (only called for button events).
    pub fn step(&mut self, ev: &Raw, now: Instant, pos: &mut dyn FnMut() -> Point) -> Step {
        match ev.ty {
            EV_KEY if ev.code == BTN_LEFT => {
                let at = pos();
                if ev.value == 1 {
                    self.left_down = true;
                    self.moved = false;
                    let clicks = match self.last_click {
                        Some((t, p, n)) if now.duration_since(t) < DOUBLE_CLICK && p.distance(&at) <= DOUBLE_CLICK_SLOP => n + 1,
                        _ => 1,
                    };
                    self.last_click = Some((now, at, clicks));
                    Step::Ask(InputEvent::MouseDown { at, clicks })
                } else if ev.value == 0 {
                    self.left_down = false;
                    Step::Ask(InputEvent::MouseUp { at })
                } else {
                    Step::Pass
                }
            }
            EV_KEY if is_button(ev.code) => Step::Pass,
            EV_KEY => {
                // Track physical modifier state whether or not the event is swallowed.
                let before = self.mods;
                self.mods.update(ev.code, ev.value);
                let key = veil_key(ev.code);
                match ev.value {
                    0 => Step::Ask(InputEvent::KeyUp { key, mods: self.mods.modifiers() }),
                    2 if self.swallowed_keys.contains(&ev.code) => Step::Drop,
                    _ => {
                        let text = if is_modifier(ev.code) { None } else { self.keymap.text(ev.code, &before) };
                        Step::Ask(InputEvent::KeyDown { key, mods: before.modifiers(), text })
                    }
                }
            }
            EV_REL if ev.code == REL_X || ev.code == REL_Y => {
                if self.left_down && ev.value != 0 {
                    self.moved = true;
                }
                Step::Pass
            }
            _ => Step::Pass,
        }
    }

    /// Record the core's verdict for a key event so its auto-repeats are dropped too.
    pub fn verdict(&mut self, ev: &Raw, swallowed: bool) {
        if ev.ty != EV_KEY || is_button(ev.code) {
            return;
        }
        match ev.value {
            1 if swallowed => {
                self.swallowed_keys.insert(ev.code);
            }
            1 => {
                self.swallowed_keys.remove(&ev.code);
            }
            0 => {
                self.swallowed_keys.remove(&ev.code);
            }
            _ => {}
        }
    }

    /// After a frame: a drag event if the pointer moved with the left button held.
    pub fn end_frame(&mut self, pos: &mut dyn FnMut() -> Point) -> Option<InputEvent> {
        if self.left_down && std::mem::take(&mut self.moved) {
            return Some(InputEvent::MouseDrag { at: pos() });
        }
        None
    }
}

/// Pointer position on sessions where it can't be queried (Wayland): relative motion accumulated
/// and clamped to the screen. Pointer acceleration is not modelled, so this is approximate.
#[derive(Clone, Copy, Debug)]
pub struct Accumulator {
    pub pos: Point,
    pub width: f64,
    pub height: f64,
}

impl Accumulator {
    pub fn new(width: f64, height: f64) -> Self {
        Accumulator { pos: Point::new(width / 2.0, height / 2.0), width, height }
    }

    pub fn apply(&mut self, ev: &Raw) {
        if ev.ty != EV_REL {
            return;
        }
        match ev.code {
            REL_X => self.pos.x = (self.pos.x + ev.value as f64).clamp(0.0, self.width - 1.0),
            REL_Y => self.pos.y = (self.pos.y + ev.value as f64).clamp(0.0, self.height - 1.0),
            _ => {}
        }
    }
}

/// Split a stream of raw events into frames at `SYN_REPORT`.
#[derive(Default)]
pub struct Framer {
    pending: Vec<Raw>,
}

impl Framer {
    pub fn push(&mut self, ev: Raw) -> Option<Vec<Raw>> {
        if ev.is_syn_report() {
            return Some(std::mem::take(&mut self.pending));
        }
        self.pending.push(ev);
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use veil_core::input::{Key, Modifiers};

    fn key(code: u16, value: i32) -> Raw {
        Raw::new(EV_KEY, code, value)
    }

    fn scan(v: i32) -> Raw {
        Raw::new(EV_MSC, MSC_SCAN, v)
    }

    #[test]
    fn passing_frame_is_closed_with_one_syn() {
        let f = [scan(0x1c), key(KEY_ENTER, 1)];
        assert_eq!(filter_frame(&f, &[false, false]), vec![scan(0x1c), key(KEY_ENTER, 1), Raw::syn()]);
    }

    #[test]
    fn swallowed_key_takes_its_scan_code_with_it() {
        let f = [scan(0x1c), key(KEY_ENTER, 1)];
        assert!(filter_frame(&f, &[false, true]).is_empty());
        let f = [scan(0x1e), key(KEY_A, 1), scan(0x1c), key(KEY_ENTER, 1)];
        assert_eq!(filter_frame(&f, &[false, false, false, true]), vec![scan(0x1e), key(KEY_A, 1), Raw::syn()]);
    }

    #[test]
    fn swallowed_button_keeps_motion_in_the_same_frame() {
        let f = [Raw::new(EV_REL, REL_X, 3), key(BTN_LEFT, 1)];
        assert_eq!(filter_frame(&f, &[false, true]), vec![Raw::new(EV_REL, REL_X, 3), Raw::syn()]);
    }

    #[test]
    fn framer_splits_on_syn_report() {
        let mut fr = Framer::default();
        assert_eq!(fr.push(key(KEY_A, 1)), None);
        assert_eq!(fr.push(Raw::syn()), Some(vec![key(KEY_A, 1)]));
        assert_eq!(fr.push(Raw::syn()), Some(vec![]));
    }

    fn tr() -> Translator {
        Translator::new(Arc::new(UsKeymap))
    }

    #[test]
    fn keys_become_core_events_with_text_and_mods() {
        let mut t = tr();
        let now = Instant::now();
        let mut pos = || Point::default();
        assert_eq!(
            t.step(&key(KEY_LEFTCTRL, 1), now, &mut pos),
            Step::Ask(InputEvent::KeyDown { key: Key::Other(KEY_LEFTCTRL as u32), mods: Modifiers::NONE, text: None })
        );
        assert_eq!(
            t.step(&key(KEY_V, 1), now, &mut pos),
            Step::Ask(InputEvent::KeyDown { key: Key::V, mods: Modifiers { ctrl: true, ..Modifiers::NONE }, text: Some("v".into()) })
        );
        t.step(&key(KEY_LEFTCTRL, 0), now, &mut pos);
        t.step(&key(KEY_LEFTSHIFT, 1), now, &mut pos);
        assert_eq!(
            t.step(&key(KEY_A, 1), now, &mut pos),
            Step::Ask(InputEvent::KeyDown { key: Key::A, mods: Modifiers { shift: true, ..Modifiers::NONE }, text: Some("A".into()) })
        );
    }

    #[test]
    fn repeats_of_swallowed_keys_are_dropped_until_release() {
        let mut t = tr();
        let now = Instant::now();
        let mut pos = || Point::default();
        let down = key(KEY_ENTER, 1);
        assert!(matches!(t.step(&down, now, &mut pos), Step::Ask(_)));
        t.verdict(&down, true);
        assert_eq!(t.step(&key(KEY_ENTER, 2), now, &mut pos), Step::Drop);
        let up = key(KEY_ENTER, 0);
        assert!(matches!(t.step(&up, now, &mut pos), Step::Ask(InputEvent::KeyUp { key: Key::Enter, .. })));
        t.verdict(&up, true);
        // A passed press: repeats are asked about normally.
        let down2 = key(KEY_ENTER, 1);
        t.step(&down2, now, &mut pos);
        t.verdict(&down2, false);
        assert!(matches!(t.step(&key(KEY_ENTER, 2), now, &mut pos), Step::Ask(_)));
    }

    #[test]
    fn clicks_count_and_drags_are_reported() {
        let mut t = tr();
        let t0 = Instant::now();
        let mut pos = || Point::new(10.0, 10.0);
        assert_eq!(t.step(&key(BTN_LEFT, 1), t0, &mut pos), Step::Ask(InputEvent::MouseDown { at: Point::new(10.0, 10.0), clicks: 1 }));
        assert_eq!(t.end_frame(&mut pos), None, "no motion yet");
        t.step(&Raw::new(EV_REL, REL_X, 5), t0, &mut pos);
        assert_eq!(t.end_frame(&mut pos), Some(InputEvent::MouseDrag { at: Point::new(10.0, 10.0) }));
        assert!(matches!(t.step(&key(BTN_LEFT, 0), t0, &mut pos), Step::Ask(InputEvent::MouseUp { .. })));
        let t1 = t0 + Duration::from_millis(150);
        assert_eq!(t.step(&key(BTN_LEFT, 1), t1, &mut pos), Step::Ask(InputEvent::MouseDown { at: Point::new(10.0, 10.0), clicks: 2 }));
        t.step(&key(BTN_LEFT, 0), t1, &mut pos);
        let t2 = t1 + Duration::from_secs(1);
        assert!(matches!(t.step(&key(BTN_LEFT, 1), t2, &mut pos), Step::Ask(InputEvent::MouseDown { clicks: 1, .. })));
        assert_eq!(t.step(&key(BTN_RIGHT, 1), t2, &mut pos), Step::Pass);
    }

    #[test]
    fn accumulator_clamps_to_screen() {
        let mut a = Accumulator::new(100.0, 50.0);
        a.apply(&Raw::new(EV_REL, REL_X, 500));
        a.apply(&Raw::new(EV_REL, REL_Y, -500));
        assert_eq!(a.pos, Point::new(99.0, 0.0));
    }
}
