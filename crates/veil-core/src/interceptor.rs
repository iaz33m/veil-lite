//! The hook-thread state machine: hold first, inspect second, replay or drop.
//!
//! Runs inside the OS input hook, so it never scans and never blocks: it decides in microseconds whether
//! to pass an event or swallow it, and hands the real work to the [`Engine`](crate::engine::Engine)
//! through a job queue.

use crate::engine::{Job, Trigger};
use crate::input::{HookVerdict, InputEvent, Key, Modifiers, Replay};
use crate::platform::{InputHandler, Platform};
use crate::shared::{DragState, DragVerdict, Hold, Shared, HOLD_WATCHDOG};
use crate::types::{AppContext, DropStrategy, Point};
use crossbeam_channel::Sender;
use std::sync::Arc;
use std::time::Instant;

/// Movement (in points/pixels) before a press-and-move counts as a drag.
const DRAG_THRESHOLD: f64 = 5.0;

pub struct Interceptor {
    platform: Arc<dyn Platform>,
    shared: Arc<Shared>,
    jobs: Sender<Job>,
}

impl Interceptor {
    pub fn new(platform: Arc<dyn Platform>, shared: Arc<Shared>, jobs: Sender<Job>) -> Self {
        Interceptor { platform, shared, jobs }
    }

    fn protected(&self, app: &AppContext) -> bool {
        self.shared.policy.read().protects(app)
    }

    fn submit(&self, job: Job) {
        if self.jobs.try_send(job).is_err() {
            tracing::warn!("job queue full or closed");
        }
    }

    /// Start holding an action in `app`. Returns false if something else is already held.
    fn begin_hold(&self, app: &AppContext) -> bool {
        let mut s = self.shared.state.lock();
        if s.hold.is_some() {
            return false;
        }
        s.hold = Some(Hold { pid: app.pid, app: app.clone(), queued: Vec::new(), since: Instant::now() });
        true
    }

    /// Drop a hold that has been stuck with no prompt on screen (the worker died or hung).
    fn expire_stuck_hold(&self) {
        let mut s = self.shared.state.lock();
        let stuck = s.hold.as_ref().is_some_and(|h| h.since.elapsed() > HOLD_WATCHDOG) && !s.prompt_open;
        if stuck {
            tracing::warn!("releasing a hold that timed out");
            s.hold = None;
        }
    }

    fn on_key_down(&self, key: Key, mods: Modifiers, text: Option<&str>, ev: &InputEvent) -> HookVerdict {
        let Some(app) = self.platform.foreground() else { return HookVerdict::Pass };
        if !self.protected(&app) {
            return HookVerdict::Pass;
        }

        {
            let mut s = self.shared.state.lock();
            if let Some(hold) = s.hold.as_mut() {
                if hold.pid == app.pid {
                    if let Some(r) = Replay::from_event(ev) {
                        hold.queued.push(r);
                    }
                    return HookVerdict::Swallow;
                }
            }
        }

        let is_send = key.is_enter() && !mods.shift && !mods.alt;
        let is_paste = (key == Key::V && mods.only_primary()) || (key == Key::Insert && mods.shift && !mods.ctrl);

        if is_send || is_paste {
            if !self.begin_hold(&app) {
                return HookVerdict::Swallow;
            }
            self.shared.state.lock().pending_keyups.insert(key);
            let trigger = Replay::Key { key, mods, text: text.map(String::from) };
            if is_send {
                self.submit(Job::Send { app, trigger: Trigger::Key(trigger) });
            } else {
                self.submit(Job::Paste { app, trigger });
            }
            return HookVerdict::Swallow;
        }

        let changed = self.shared.state.lock().buffer.apply(app.pid, key, mods, text);
        if changed {
            self.submit(Job::Typed { app });
        }
        HookVerdict::Pass
    }

    fn on_key_up(&self, key: Key, ev: &InputEvent) -> HookVerdict {
        let mut s = self.shared.state.lock();
        if s.pending_keyups.remove(&key) {
            return HookVerdict::Swallow;
        }
        if let Some(hold) = s.hold.as_mut() {
            let fg_pid = hold.pid;
            drop(s);
            // Only queue key-ups that belong to the held app.
            if self.platform.foreground().is_some_and(|a| a.pid == fg_pid) {
                let mut s = self.shared.state.lock();
                if let Some(hold) = s.hold.as_mut() {
                    if let Some(r) = Replay::from_event(ev) {
                        hold.queued.push(r);
                    }
                    return HookVerdict::Swallow;
                }
            }
        }
        HookVerdict::Pass
    }

    fn on_mouse_down(&self, at: Point, clicks: u32) -> HookVerdict {
        {
            let mut s = self.shared.state.lock();
            s.drag_generation += 1;
            let generation = s.drag_generation;
            s.drag = Some(DragState { generation, down_at: at, started: false, withheld: false });
            if s.hold.is_some() {
                return HookVerdict::Pass;
            }
        }

        let Some(app) = self.platform.app_at(at) else { return HookVerdict::Pass };
        if !self.protected(&app) {
            return HookVerdict::Pass;
        }
        let Some(el) = self.platform.element_at(at) else { return HookVerdict::Pass };

        let (is_send_button, is_pick_button) = {
            let policy = self.shared.policy.read();
            (
                el.is_button && el.pid == app.pid && policy.is_send_label(&el.label),
                el.in_file_dialog && ((el.is_button && policy.is_pick_label(&el.label)) || clicks >= 2),
            )
        };

        let generic_while_dirty = el.is_generic
            && el.pid == app.pid
            && app.url.is_some()
            && self.shared.state.lock().typed_dirty.get(&app.pid).copied().unwrap_or(false);

        if is_pick_button {
            if !self.begin_hold(&app) {
                return HookVerdict::Pass;
            }
            self.shared.state.lock().swallow_up = true;
            self.submit(Job::Pick { app, at, clicks });
            return HookVerdict::Swallow;
        }

        if is_send_button || generic_while_dirty {
            if !self.begin_hold(&app) {
                return HookVerdict::Pass;
            }
            self.shared.state.lock().swallow_up = true;
            self.submit(Job::Send { app, trigger: Trigger::Click { at, clicks, generic: !is_send_button } });
            return HookVerdict::Swallow;
        }
        HookVerdict::Pass
    }

    fn on_mouse_drag(&self, at: Point) -> HookVerdict {
        let (generation, just_started, withheld) = {
            let mut s = self.shared.state.lock();
            let Some(drag) = s.drag.as_mut() else { return HookVerdict::Pass };
            let just_started = !drag.started && drag.down_at.distance(&at) >= DRAG_THRESHOLD;
            if just_started {
                drag.started = true;
            }
            if !drag.started {
                return HookVerdict::Pass;
            }
            (drag.generation, just_started, drag.withheld)
        };

        if just_started {
            self.submit(Job::DragStart { generation });
        }

        if self.platform.drop_strategy() != DropStrategy::Withhold {
            return HookVerdict::Pass;
        }
        // Withhold the drag data while it hovers over a protected app, unless it is known to be clean.
        let over_protected = self
            .platform
            .app_at(at)
            .filter(|a| self.protected(a) && !self.shared.drops_allowed(&a.app_id))
            .is_some();
        let clean = matches!(self.shared.drag_verdict(generation), DragVerdict::Clean | DragVerdict::NoData);
        let want = over_protected && !clean;
        if want != withheld {
            self.platform.set_drag_withheld(want);
            if let Some(d) = self.shared.state.lock().drag.as_mut() {
                d.withheld = want;
            }
        }
        HookVerdict::Pass
    }

    fn on_mouse_up(&self, at: Point) -> HookVerdict {
        let (swallow, drag) = {
            let mut s = self.shared.state.lock();
            (std::mem::take(&mut s.swallow_up), s.drag.take())
        };
        if swallow {
            return HookVerdict::Swallow;
        }
        let Some(drag) = drag.filter(|d| d.started) else { return HookVerdict::Pass };

        let target = self.platform.app_at(at).filter(|a| self.protected(a));
        let Some(app) = target else {
            if drag.withheld {
                self.platform.set_drag_withheld(false);
            }
            return HookVerdict::Pass;
        };
        if self.shared.drops_allowed(&app.app_id) {
            if drag.withheld {
                self.platform.set_drag_withheld(false);
            }
            return HookVerdict::Pass;
        }
        let verdict = self.shared.drag_verdict(drag.generation);
        if matches!(verdict, DragVerdict::Clean | DragVerdict::NoData) {
            if drag.withheld {
                self.platform.set_drag_withheld(false);
            }
            return HookVerdict::Pass;
        }

        let strategy = self.platform.drop_strategy();
        self.submit(Job::Drop { app, generation: drag.generation, at, strategy, withheld: drag.withheld });
        match strategy {
            // The drop lands with the data withheld (empty), then the engine restores the data.
            DropStrategy::Withhold => HookVerdict::Pass,
            DropStrategy::SwallowAndCancel => HookVerdict::Swallow,
            // Cannot stop it: the engine warns and arms the backstop for the next send.
            DropStrategy::Unsupported => HookVerdict::Pass,
        }
    }
}

impl InputHandler for Interceptor {
    fn on_input(&self, ev: &InputEvent) -> HookVerdict {
        if self.shared.is_paused() {
            return HookVerdict::Pass;
        }
        self.expire_stuck_hold();
        match ev {
            InputEvent::KeyDown { key, mods, text } => self.on_key_down(*key, *mods, text.as_deref(), ev),
            InputEvent::KeyUp { key, .. } => self.on_key_up(*key, ev),
            InputEvent::MouseDown { at, clicks } => self.on_mouse_down(*at, *clicks),
            InputEvent::MouseDrag { at } => self.on_mouse_drag(*at),
            InputEvent::MouseUp { at } => self.on_mouse_up(*at),
        }
    }
}
