//! State shared by the hook thread (interceptor) and the worker thread (engine).
//!
//! Rule: never call into the platform while holding the state lock.

use crate::buffer::KeystrokeBuffer;
use crate::findings::Finding;
use crate::input::{Key, Replay};
use crate::policy::{Action, Policy};
use crate::types::{AppContext, Point};
use parking_lot::{Mutex, RwLock};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

/// A send/paste/upload held while it is inspected.
#[derive(Clone, Debug)]
pub struct Hold {
    pub pid: u32,
    pub app: AppContext,
    /// Input typed into the same app while held, replayed in order afterwards.
    pub queued: Vec<Replay>,
    pub since: Instant,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DragVerdict {
    Pending,
    /// The press-and-move carried no readable drag data (e.g. a text selection): nothing to stop.
    NoData,
    Clean,
    Dirty { findings: Vec<Finding>, action: Action, files: usize },
}

#[derive(Clone, Debug)]
pub struct DragState {
    pub generation: u64,
    pub down_at: Point,
    pub started: bool,
    /// Drag data is currently withheld by the platform.
    pub withheld: bool,
}

#[derive(Debug, Default)]
pub struct State {
    pub hold: Option<Hold>,
    /// Key-downs we swallowed whose key-ups must be swallowed too.
    pub pending_keyups: HashSet<Key>,
    /// Swallow the next mouse-up (its mouse-down was held).
    pub swallow_up: bool,
    pub drag: Option<DragState>,
    pub drag_generation: u64,
    pub drag_verdicts: HashMap<u64, DragVerdict>,
    pub buffer: KeystrokeBuffer,
    /// Fast layers found something in what is being typed (per pid).
    pub typed_dirty: HashMap<u32, bool>,
    /// Typing warning already shown for the current message (per pid).
    pub warned: HashSet<u32>,
    pub paused_until: Option<Instant>,
    /// Risky drops that may have landed (per pid): asked again on the next send.
    pub dirty_drops: HashMap<u32, (Instant, Vec<Finding>)>,
    /// Drops into these apps pass until the deadline (per app id).
    pub allow_drops_until: HashMap<String, Instant>,
    /// A prompt is on screen.
    pub prompt_open: bool,
}

pub struct Shared {
    pub state: Mutex<State>,
    pub policy: RwLock<Policy>,
}

/// A held action with no prompt on screen is released after this long (worker stuck or crashed).
pub const HOLD_WATCHDOG: Duration = Duration::from_secs(4);
pub const DIRTY_DROP_TTL: Duration = Duration::from_secs(600);
pub const ALLOW_DROPS_FOR: Duration = Duration::from_secs(30);

impl Shared {
    pub fn new(policy: Policy) -> Self {
        Shared { state: Mutex::new(State::default()), policy: RwLock::new(policy) }
    }

    pub fn is_paused(&self) -> bool {
        let mut s = self.state.lock();
        match s.paused_until {
            Some(t) if Instant::now() < t => true,
            Some(_) => {
                s.paused_until = None;
                false
            }
            None => false,
        }
    }

    pub fn pause(&self, d: Duration) {
        self.state.lock().paused_until = Some(Instant::now() + d);
    }

    pub fn resume(&self) {
        self.state.lock().paused_until = None;
    }

    pub fn paused_remaining(&self) -> Option<Duration> {
        let s = self.state.lock();
        s.paused_until.and_then(|t| t.checked_duration_since(Instant::now()))
    }

    /// Release the hold and return input queued during it.
    pub fn release_hold(&self) -> Vec<Replay> {
        let mut s = self.state.lock();
        s.hold.take().map(|h| h.queued).unwrap_or_default()
    }

    pub fn buffer_text(&self, pid: u32) -> String {
        self.state.lock().buffer.get(pid).to_string()
    }

    pub fn set_drag_verdict(&self, generation: u64, verdict: DragVerdict) {
        let mut s = self.state.lock();
        s.drag_verdicts.insert(generation, verdict);
        let keep_from = generation.saturating_sub(8);
        s.drag_verdicts.retain(|g, _| *g >= keep_from);
    }

    pub fn drag_verdict(&self, generation: u64) -> DragVerdict {
        self.state.lock().drag_verdicts.get(&generation).cloned().unwrap_or(DragVerdict::Pending)
    }

    pub fn drops_allowed(&self, app_id: &str) -> bool {
        let s = self.state.lock();
        s.allow_drops_until.get(app_id).is_some_and(|t| Instant::now() < *t)
    }

    pub fn allow_drops(&self, app_id: &str) {
        self.state.lock().allow_drops_until.insert(app_id.to_string(), Instant::now() + ALLOW_DROPS_FOR);
    }

    pub fn take_dirty_drop(&self, pid: u32) -> Option<Vec<Finding>> {
        let mut s = self.state.lock();
        let (at, findings) = s.dirty_drops.remove(&pid)?;
        (at.elapsed() < DIRTY_DROP_TTL).then_some(findings)
    }
}

/// Content hash used for the model cache.
pub fn content_hash(text: &str) -> [u8; 32] {
    Sha256::digest(text.as_bytes()).into()
}

/// Short hex tag (8 chars) for names.
pub fn short_hash(bytes: &[u8]) -> String {
    hex::encode(&Sha256::digest(bytes)[..4])
}
