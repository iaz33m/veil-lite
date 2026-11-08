//! The decision engine. Runs on a worker thread: reads what is being sent, runs the detection layers,
//! applies the policy, asks the user when needed, then replays or drops the held input.

use crate::activity::{ActivityEntry, ActivityLog, Outcome};
use crate::findings::{summarize, Finding};
use crate::input::Replay;
use crate::platform::{Choice, Platform, Prompt, PromptKind};
use crate::policy::{Action, FailMode, Policy};
use crate::redact::{can_redact, redact};
use crate::scanner::{Extracted, ModelStatus, ScanLayers, Scanner};
use crate::shared::{content_hash, DragVerdict, Shared};
use crate::types::*;
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Budget for the AI layer while a message is held. Past it, the fail mode applies.
pub const MESSAGE_MODEL_BUDGET: Duration = Duration::from_millis(1500);
/// Budget for checking files (drops, uploads, pasted files).
pub const FILE_BUDGET: Duration = Duration::from_secs(8);
const DRAG_SESSION_RETRIES: u32 = 6;
const DRAG_SESSION_RETRY_DELAY: Duration = Duration::from_millis(25);
const DROP_RESTORE_DELAY: Duration = Duration::from_millis(350);
const DROP_VERDICT_WAIT: Duration = Duration::from_secs(10);
const CLIPBOARD_SETTLE: Duration = Duration::from_millis(180);
const MODEL_CACHE_SIZE: usize = 256;

/// What started a held send.
#[derive(Clone, Debug, PartialEq)]
pub enum Trigger {
    /// Enter (or similar) key.
    Key(Replay),
    /// Click on a Send button, or on an unlabelled web element while the text looked sensitive.
    Click { at: Point, clicks: u32, generic: bool },
}

impl Trigger {
    pub fn replay(&self) -> Replay {
        match self {
            Trigger::Key(r) => r.clone(),
            Trigger::Click { at, clicks, .. } => Replay::Click { at: *at, clicks: *clicks },
        }
    }
}

/// Work handed from the hook thread to the worker.
#[derive(Clone, Debug)]
pub enum Job {
    Send { app: AppContext, trigger: Trigger },
    Paste { app: AppContext, trigger: Replay },
    Pick { app: AppContext, at: Point, clicks: u32 },
    Typed { app: AppContext },
    DragStart { generation: u64 },
    Drop { app: AppContext, generation: u64, at: Point, strategy: DropStrategy, withheld: bool },
    Shutdown,
}

/// Events for the desktop app (over IPC `subscribe`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum EngineEvent {
    Activity { entry: ActivityEntry },
    Paused { remaining_secs: u64 },
    Resumed,
    PolicyChanged,
    Model { status: ModelStatus },
    Warning { message: String },
}

/// Fan-out of engine events to any number of subscribers.
#[derive(Default)]
pub struct EventBus {
    subscribers: Mutex<Vec<Sender<EngineEvent>>>,
}

impl EventBus {
    pub fn subscribe(&self) -> Receiver<EngineEvent> {
        let (tx, rx) = crossbeam_channel::bounded(256);
        self.subscribers.lock().push(tx);
        rx
    }

    pub fn publish(&self, ev: EngineEvent) {
        self.subscribers.lock().retain(|s| match s.try_send(ev.clone()) {
            Ok(()) => true,
            Err(crossbeam_channel::TrySendError::Full(_)) => true,
            Err(crossbeam_channel::TrySendError::Disconnected(_)) => false,
        });
    }
}

/// Result of inspecting content.
#[derive(Clone, Debug, Default)]
pub struct Inspection {
    pub findings: Vec<Finding>,
    /// Files that could not be checked (too large, encrypted, unreadable).
    pub unchecked: Vec<String>,
    pub files: usize,
    /// False when the AI layer ran out of time.
    pub complete: bool,
}

#[derive(Default)]
struct ModelCache {
    done: HashMap<[u8; 32], Vec<Finding>>,
    order: VecDeque<[u8; 32]>,
    inflight: HashSet<[u8; 32]>,
}

impl ModelCache {
    fn insert(&mut self, key: [u8; 32], findings: Vec<Finding>) {
        self.inflight.remove(&key);
        if self.done.insert(key, findings).is_none() {
            self.order.push_back(key);
            while self.order.len() > MODEL_CACHE_SIZE {
                if let Some(old) = self.order.pop_front() {
                    self.done.remove(&old);
                }
            }
        }
    }
}

struct Prescore {
    due: Instant,
    app: AppContext,
}

pub struct Engine {
    platform: Arc<dyn Platform>,
    scanner: Arc<dyn Scanner>,
    shared: Arc<Shared>,
    activity: Arc<ActivityLog>,
    pub events: Arc<EventBus>,
    cache: Arc<Mutex<ModelCache>>,
    prescore: Mutex<Option<Prescore>>,
    last_model_use: Mutex<Instant>,
    message_budget: Duration,
}

impl Engine {
    pub fn new(
        platform: Arc<dyn Platform>,
        scanner: Arc<dyn Scanner>,
        shared: Arc<Shared>,
        activity: Arc<ActivityLog>,
    ) -> Self {
        Engine {
            platform,
            scanner,
            shared,
            activity,
            events: Arc::new(EventBus::default()),
            cache: Arc::new(Mutex::new(ModelCache::default())),
            prescore: Mutex::new(None),
            last_model_use: Mutex::new(Instant::now()),
            message_budget: MESSAGE_MODEL_BUDGET,
        }
    }

    pub fn with_message_budget(mut self, budget: Duration) -> Self {
        self.message_budget = budget;
        self
    }

    pub fn shared(&self) -> &Arc<Shared> {
        &self.shared
    }

    pub fn platform(&self) -> &Arc<dyn Platform> {
        &self.platform
    }

    pub fn scanner(&self) -> &Arc<dyn Scanner> {
        &self.scanner
    }

    pub fn activity(&self) -> &Arc<ActivityLog> {
        &self.activity
    }

    // ----- control (called from IPC threads) -----

    pub fn apply_policy(&self, policy: Policy) {
        self.scanner.set_keywords(&policy.keywords);
        *self.shared.policy.write() = policy;
        self.cache.lock().done.clear();
        self.events.publish(EngineEvent::PolicyChanged);
    }

    pub fn pause(&self, d: Duration) -> bool {
        if !self.shared.policy.read().pause.allowed {
            return false;
        }
        self.shared.pause(d);
        self.events.publish(EngineEvent::Paused { remaining_secs: d.as_secs() });
        true
    }

    pub fn resume(&self) {
        self.shared.resume();
        self.events.publish(EngineEvent::Resumed);
    }

    // ----- worker loop -----

    /// Process jobs until `Job::Shutdown` or the channel closes.
    pub fn run(self: Arc<Self>, jobs: Receiver<Job>) {
        loop {
            let wait = self.next_timer().map(|t| t.saturating_duration_since(Instant::now()));
            let job = match wait {
                Some(w) => match jobs.recv_timeout(w) {
                    Ok(job) => Some(job),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => break,
                },
                None => match jobs.recv() {
                    Ok(job) => Some(job),
                    Err(_) => break,
                },
            };
            match job {
                Some(Job::Shutdown) => break,
                Some(job) => self.handle(job),
                None => self.on_timer(),
            }
        }
    }

    pub fn spawn(self: Arc<Self>, jobs: Receiver<Job>) -> std::thread::JoinHandle<()> {
        std::thread::Builder::new().name("veil-engine".into()).spawn(move || self.run(jobs)).expect("spawn engine")
    }

    fn next_timer(&self) -> Option<Instant> {
        let prescore = self.prescore.lock().as_ref().map(|p| p.due);
        let unload = matches!(self.scanner.model_status(), ModelStatus::Loaded).then(|| {
            *self.last_model_use.lock() + Duration::from_secs(self.shared.policy.read().model.idle_unload_secs)
        });
        match (prescore, unload) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    fn on_timer(&self) {
        let due = {
            let mut p = self.prescore.lock();
            match p.as_ref() {
                Some(ps) if ps.due <= Instant::now() => p.take(),
                _ => None,
            }
        };
        if let Some(ps) = due {
            self.prescore(&ps.app);
        }
        let idle = Duration::from_secs(self.shared.policy.read().model.idle_unload_secs);
        if matches!(self.scanner.model_status(), ModelStatus::Loaded) && self.last_model_use.lock().elapsed() >= idle {
            tracing::info!("unloading model after {idle:?} idle");
            self.scanner.unload_model();
            self.events.publish(EngineEvent::Model { status: self.scanner.model_status() });
        }
    }

    pub fn handle(&self, job: Job) {
        match job {
            Job::Send { app, trigger } => self.on_send(&app, trigger),
            Job::Paste { app, trigger } => self.on_paste(&app, trigger),
            Job::Pick { app, at, clicks } => self.on_pick(&app, at, clicks),
            Job::Typed { app } => self.on_typed(&app),
            Job::DragStart { generation } => self.on_drag_start(generation),
            Job::Drop { app, generation, at, strategy, withheld } => {
                self.on_drop(&app, generation, at, strategy, withheld)
            }
            Job::Shutdown => {}
        }
    }

    // ----- detection helpers -----

    fn model_usable(&self) -> bool {
        self.shared.policy.read().model.enabled
            && matches!(self.scanner.model_status(), ModelStatus::Ready | ModelStatus::Loaded)
    }

    /// Fast layers synchronously, then the AI layer (cached) within `budget`.
    pub fn scan_text(&self, text: &str, source: &str, budget: Duration) -> (Vec<Finding>, bool) {
        let mut findings = self.scanner.scan_text(text, ScanLayers::FAST, source);
        if text.trim().is_empty() || !self.model_usable() {
            return (findings, true);
        }
        match self.model_findings(text, source, budget) {
            Some(m) => {
                findings.extend(m);
                (findings, true)
            }
            None => (findings, false),
        }
    }

    fn model_findings(&self, text: &str, source: &str, budget: Duration) -> Option<Vec<Finding>> {
        let key = content_hash(text);
        let deadline = Instant::now() + budget;
        let with_source = |v: &Vec<Finding>| {
            v.iter().cloned().map(|mut f| {
                f.source = source.to_string();
                f
            }).collect::<Vec<_>>()
        };
        let already_running = {
            let mut c = self.cache.lock();
            if let Some(hit) = c.done.get(&key) {
                return Some(with_source(hit));
            }
            !c.inflight.insert(key)
        };
        *self.last_model_use.lock() = Instant::now();
        if !already_running {
            self.spawn_model_scan(text.to_string(), key);
        }
        // Wait for this scan (or an identical background pre-score) to land in the cache.
        loop {
            if let Some(hit) = self.cache.lock().done.get(&key) {
                return Some(with_source(hit));
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn spawn_model_scan(&self, text: String, key: [u8; 32]) {
        let scanner = Arc::clone(&self.scanner);
        let cache = Arc::clone(&self.cache);
        let spawned = std::thread::Builder::new().name("veil-model".into()).spawn(move || {
            let findings = scanner.scan_text(&text, ScanLayers::MODEL_ONLY, "");
            cache.lock().insert(key, findings);
        });
        if spawned.is_err() {
            self.cache.lock().inflight.remove(&key);
        }
    }

    /// Inspect message text and/or files and/or an image.
    pub fn inspect(&self, text: Option<(&str, &str)>, files: &[PathBuf], image: Option<&[u8]>, budget: Duration) -> Inspection {
        let deadline = Instant::now() + budget;
        let mut ins = Inspection { complete: true, files: files.len(), ..Default::default() };
        if let Some((text, source)) = text {
            let (f, complete) = self.scan_text(text, source, budget);
            ins.findings.extend(f);
            ins.complete &= complete;
        }
        for path in files {
            self.inspect_file(path, &mut ins, deadline);
        }
        if let Some(bytes) = image {
            if let Some(text) = self.platform.ocr(bytes) {
                let (f, complete) = self.scan_text(&text, "image", remaining(deadline));
                ins.findings.extend(f);
                ins.complete &= complete;
            }
        }
        ins
    }

    fn inspect_file(&self, path: &Path, ins: &mut Inspection, deadline: Instant) {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
        let name_layers = ScanLayers { keywords: true, pii: false, model: false };
        ins.findings.extend(self.scanner.scan_text(&name, name_layers, &name));
        let extracted = self.scanner.extract(path);
        self.inspect_extracted(&name, extracted, ins, deadline);
    }

    fn inspect_extracted(&self, name: &str, ex: Extracted, ins: &mut Inspection, deadline: Instant) {
        match ex {
            Extracted::Text(text) => {
                let (f, complete) = self.scan_text(&text, name, remaining(deadline));
                ins.findings.extend(f);
                ins.complete &= complete;
            }
            Extracted::Image(bytes) => {
                if let Some(text) = self.platform.ocr(&bytes) {
                    let (f, complete) = self.scan_text(&text, name, remaining(deadline));
                    ins.findings.extend(f);
                    ins.complete &= complete;
                }
            }
            Extracted::Many(parts) => {
                for (part, ex) in parts {
                    self.inspect_extracted(&format!("{name}/{part}"), ex, ins, deadline);
                }
            }
            Extracted::TooLarge | Extracted::Encrypted | Extracted::Unreadable(_) => ins.unchecked.push(name.to_string()),
            Extracted::Unsupported => {}
        }
    }

    /// Policy action for an inspection, including the fail mode for content that could not be checked.
    pub fn decide(&self, ins: &Inspection, channel: Channel) -> (Action, Vec<Finding>) {
        let policy = self.shared.policy.read();
        let d = policy.decide(&ins.findings, channel);
        let fail = if channel.is_file_channel() || ins.files > 0 {
            policy.actions.fail_mode.files
        } else {
            policy.actions.fail_mode.text
        };
        let incomplete = !ins.unchecked.is_empty() || !ins.complete;
        let action = if incomplete && fail == FailMode::Closed { d.action.max(Action::Ask) } else { d.action };
        (action, d.findings)
    }

    // ----- prompts -----

    fn ask(&self, prompt: &Prompt) -> Choice {
        self.shared.state.lock().prompt_open = true;
        let choice = self.platform.ask(prompt);
        self.shared.state.lock().prompt_open = false;
        if prompt.options.contains(&choice) {
            choice
        } else {
            prompt.cancel_choice()
        }
    }

    fn message_for(kind: PromptKind, app: &AppContext, findings: &[Finding], ins: &Inspection) -> String {
        let what = summarize(findings);
        let target = app.display();
        let mut msg = match kind {
            PromptKind::Send => format!("This message to {target} contains {what}."),
            PromptKind::Paste => format!("What you're pasting into {target} contains {what}."),
            PromptKind::Upload => format!("The file you're uploading to {target} contains {what}."),
            PromptKind::Drop => format!(
                "Veil stopped a drop into {target}: it contains {what}. To drop it anyway, choose “Allow drops for 30 s” and drop again."
            ),
            PromptKind::Backstop => format!(
                "A file you dropped into {target} a moment ago contains {what}. If it is attached, remove it before sending."
            ),
            PromptKind::BlockNotice => format!("Veil's policy doesn't allow sending {what} to {target}."),
        };
        if findings.is_empty() && !ins.complete {
            msg = format!("Veil couldn't finish checking this content for {target} in time.");
        }
        let mut sources: Vec<&str> = Vec::new();
        for f in findings {
            if f.source != "message" && !f.source.is_empty() && !sources.contains(&f.source.as_str()) {
                sources.push(&f.source);
            }
        }
        if !sources.is_empty() && kind != PromptKind::Send {
            msg.push_str(&format!("\n\nFound in: {}", sources.iter().take(3).copied().collect::<Vec<_>>().join(", ")));
        }
        if !ins.unchecked.is_empty() {
            msg.push_str(&format!(
                "\n\nCouldn't be checked (too large, encrypted or unreadable): {}",
                ins.unchecked.iter().take(3).cloned().collect::<Vec<_>>().join(", ")
            ));
        }
        msg
    }

    fn log(&self, app: &AppContext, channel: Channel, findings: &[Finding], outcome: Outcome, files: usize) {
        if !self.shared.policy.read().activity_log.enabled {
            return;
        }
        let entry = ActivityEntry::new(app, channel, findings, outcome, files);
        self.activity.append(&entry);
        self.events.publish(EngineEvent::Activity { entry });
    }

    /// Release the hold, then replay the trigger (if any) and anything typed meanwhile.
    fn release(&self, app: &AppContext, trigger: Option<Replay>, prompted: bool) {
        let queued = self.shared.release_hold();
        if prompted {
            self.platform.refocus(app);
        }
        if let Some(t) = trigger {
            self.platform.replay(&t);
        }
        for r in queued {
            self.platform.replay(&r);
        }
    }

    fn message_sent(&self, pid: u32) {
        let mut s = self.shared.state.lock();
        s.buffer.clear(pid);
        s.typed_dirty.remove(&pid);
        s.warned.remove(&pid);
    }

    // ----- channels -----

    fn on_send(&self, app: &AppContext, trigger: Trigger) {
        let (text, exposed) = match self.platform.focused_text(app) {
            FocusedText::Exposed { text, .. } if !text.trim().is_empty() => (text, true),
            _ => (self.shared.buffer_text(app.pid), false),
        };
        let backstop = self.shared.take_dirty_drop(app.pid);

        if text.trim().is_empty() && backstop.is_none() {
            self.release(app, Some(trigger.replay()), false);
            return;
        }

        let ins = self.inspect(Some((&text, "message")), &[], None, self.message_budget);
        let (mut action, mut findings) = self.decide(&ins, Channel::Message);
        let mut kind = PromptKind::Send;
        if let Some(dropped) = backstop {
            findings.extend(dropped);
            action = action.max(Action::Ask);
            kind = PromptKind::Backstop;
        }

        match action {
            Action::Allow => {
                self.release(app, Some(trigger.replay()), false);
                self.message_sent(app.pid);
            }
            Action::Block => {
                let prompt = self.prompt(PromptKind::BlockNotice, app, &findings, &ins, vec![Choice::Ok]);
                self.ask(&prompt);
                self.release(app, None, true);
                self.log(app, Channel::Message, &findings, Outcome::Blocked, 0);
            }
            Action::Ask => {
                let redactable = kind == PromptKind::Send
                    && exposed
                    && self.shared.policy.read().actions.allow_redact
                    && can_redact(&findings);
                let mut options = vec![Choice::DontSend, Choice::SendAnyway];
                if redactable {
                    options.push(Choice::Redact);
                }
                let prompt = self.prompt(kind, app, &findings, &ins, options);
                match self.ask(&prompt) {
                    Choice::SendAnyway => {
                        self.release(app, Some(trigger.replay()), true);
                        self.message_sent(app.pid);
                        self.log(app, Channel::Message, &findings, Outcome::SentAnyway, 0);
                    }
                    Choice::Redact => {
                        self.platform.refocus(app);
                        let ok = self.replace_text(app, &redact(&text, &findings));
                        if ok {
                            self.release(app, Some(trigger.replay()), true);
                            self.message_sent(app.pid);
                            self.log(app, Channel::Message, &findings, Outcome::Redacted, 0);
                        } else {
                            self.release(app, None, true);
                            self.log(app, Channel::Message, &findings, Outcome::Cancelled, 0);
                        }
                    }
                    _ => {
                        self.release(app, None, true);
                        if matches!(trigger, Trigger::Click { generic: true, .. }) {
                            // Don't ask again on every click in the page until the text changes.
                            self.shared.state.lock().typed_dirty.insert(app.pid, false);
                        }
                        self.log(app, Channel::Message, &findings, Outcome::Cancelled, 0);
                    }
                }
            }
        }
    }

    fn prompt(&self, kind: PromptKind, app: &AppContext, findings: &[Finding], ins: &Inspection, options: Vec<Choice>) -> Prompt {
        Prompt {
            title: "Veil".into(),
            message: Self::message_for(kind, app, findings, ins),
            app: app.clone(),
            kind,
            options,
        }
    }

    /// Replace the composer text: directly when the platform can, else select-all + paste.
    fn replace_text(&self, app: &AppContext, text: &str) -> bool {
        if self.platform.replace_composer_text(app, text) {
            return true;
        }
        let saved = self.platform.clipboard();
        let temp = ClipboardContent { text: Some(text.to_string()), ..Default::default() };
        if !self.platform.set_clipboard(&temp) {
            return false;
        }
        self.platform.replay(&Replay::select_all());
        std::thread::sleep(Duration::from_millis(40));
        self.platform.replay(&Replay::paste());
        std::thread::sleep(CLIPBOARD_SETTLE);
        self.platform.set_clipboard(&saved);
        true
    }

    fn on_paste(&self, app: &AppContext, trigger: Replay) {
        let clip = self.platform.clipboard();
        let text = clip.text.as_deref().filter(|t| !t.trim().is_empty());
        if text.is_none() && clip.files.is_empty() && clip.image.is_none() {
            self.release(app, Some(trigger), false);
            return;
        }
        let budget = if clip.files.is_empty() && clip.image.is_none() { self.message_budget } else { FILE_BUDGET };
        let ins = self.inspect(text.map(|t| (t, "message")), &clip.files, clip.image.as_deref(), budget);
        let (action, findings) = self.decide(&ins, Channel::Paste);
        let files = clip.files.len();

        let pasted_text = |this: &Self| {
            if let Some(t) = text {
                this.shared.state.lock().buffer.append(app.pid, t);
                this.on_typed(app);
            }
        };

        match action {
            Action::Allow => {
                self.release(app, Some(trigger), false);
                pasted_text(self);
                if files > 0 || clip.image.is_some() {
                    self.log(app, Channel::Paste, &[], Outcome::Allowed, files);
                }
            }
            Action::Block => {
                let prompt = self.prompt(PromptKind::BlockNotice, app, &findings, &ins, vec![Choice::Ok]);
                self.ask(&prompt);
                self.release(app, None, true);
                self.log(app, Channel::Paste, &findings, Outcome::Blocked, files);
            }
            Action::Ask => {
                let redactable = files == 0
                    && clip.image.is_none()
                    && text.is_some()
                    && self.shared.policy.read().actions.allow_redact
                    && can_redact(&findings);
                let mut options = vec![Choice::DontSend, Choice::SendAnyway];
                if redactable {
                    options.push(Choice::Redact);
                }
                let prompt = self.prompt(PromptKind::Paste, app, &findings, &ins, options);
                match self.ask(&prompt) {
                    Choice::SendAnyway => {
                        self.release(app, Some(trigger), true);
                        pasted_text(self);
                        self.log(app, Channel::Paste, &findings, Outcome::SentAnyway, files);
                    }
                    Choice::Redact => {
                        let original = text.unwrap_or_default();
                        let redacted = redact(original, &findings);
                        let temp = ClipboardContent { text: Some(redacted.clone()), ..Default::default() };
                        if self.platform.set_clipboard(&temp) {
                            self.release(app, Some(trigger), true);
                            std::thread::sleep(CLIPBOARD_SETTLE);
                            self.platform.set_clipboard(&clip);
                            self.shared.state.lock().buffer.append(app.pid, &redacted);
                            self.log(app, Channel::Paste, &findings, Outcome::Redacted, 0);
                        } else {
                            self.release(app, None, true);
                            self.log(app, Channel::Paste, &findings, Outcome::Cancelled, 0);
                        }
                    }
                    _ => {
                        self.release(app, None, true);
                        self.log(app, Channel::Paste, &findings, Outcome::Cancelled, files);
                    }
                }
            }
        }
    }

    fn on_pick(&self, app: &AppContext, at: Point, clicks: u32) {
        let click = Replay::Click { at, clicks };
        let files = self.platform.picker_selection(app);
        if files.is_empty() {
            self.release(app, Some(click), false);
            return;
        }
        let ins = self.inspect(None, &files, None, FILE_BUDGET);
        let (action, findings) = self.decide(&ins, Channel::Upload);
        match action {
            Action::Allow => {
                self.release(app, Some(click), false);
                self.log(app, Channel::Upload, &[], Outcome::Allowed, files.len());
            }
            Action::Block => {
                let prompt = self.prompt(PromptKind::BlockNotice, app, &findings, &ins, vec![Choice::Ok]);
                self.ask(&prompt);
                // The picker stays open so another file can be chosen.
                self.release(app, None, true);
                self.log(app, Channel::Upload, &findings, Outcome::Blocked, files.len());
            }
            Action::Ask => {
                let prompt = self.prompt(PromptKind::Upload, app, &findings, &ins, vec![Choice::DontSend, Choice::SendAnyway]);
                if self.ask(&prompt) == Choice::SendAnyway {
                    self.release(app, Some(click), true);
                    self.log(app, Channel::Upload, &findings, Outcome::SentAnyway, files.len());
                } else {
                    self.release(app, None, true);
                    self.log(app, Channel::Upload, &findings, Outcome::Cancelled, files.len());
                }
            }
        }
    }

    fn on_typed(&self, app: &AppContext) {
        let text = self.shared.buffer_text(app.pid);
        self.update_typed_state(app, &text);
        let delay = self.shared.policy.read().model.prescore_delay_ms;
        *self.prescore.lock() = Some(Prescore { due: Instant::now() + Duration::from_millis(delay), app: app.clone() });
    }

    fn update_typed_state(&self, app: &AppContext, text: &str) {
        let findings = self.scanner.scan_text(text, ScanLayers::FAST, "message");
        let decision = self.shared.policy.read().decide(&findings, Channel::Typing);
        let dirty = decision.action != Action::Allow;
        let warn = {
            let mut s = self.shared.state.lock();
            s.typed_dirty.insert(app.pid, dirty);
            if dirty {
                s.warned.insert(app.pid)
            } else {
                s.warned.remove(&app.pid);
                false
            }
        };
        if warn {
            let message = format!("Veil: this message contains {}. You'll be asked before it is sent.", summarize(&decision.findings));
            self.platform.banner(&message);
            self.events.publish(EngineEvent::Warning { message });
        }
    }

    /// Background work after typing pauses: refresh from the real composer text and warm the AI cache.
    fn prescore(&self, app: &AppContext) {
        let text = match self.platform.focused_text(app) {
            FocusedText::Exposed { text, .. } if !text.trim().is_empty() => {
                self.update_typed_state(app, &text);
                text
            }
            _ => self.shared.buffer_text(app.pid),
        };
        if text.trim().is_empty() || !self.model_usable() {
            return;
        }
        let key = content_hash(&text);
        {
            let mut c = self.cache.lock();
            if c.done.contains_key(&key) || !c.inflight.insert(key) {
                return;
            }
        }
        *self.last_model_use.lock() = Instant::now();
        self.spawn_model_scan(text, key);
    }

    fn on_drag_start(&self, generation: u64) {
        let mut session = None;
        for _ in 0..DRAG_SESSION_RETRIES {
            session = self.platform.drag_session().filter(|s| !s.files.is_empty() || s.text.is_some());
            if session.is_some() {
                break;
            }
            std::thread::sleep(DRAG_SESSION_RETRY_DELAY);
        }
        let Some(session) = session else {
            self.shared.set_drag_verdict(generation, DragVerdict::Clean);
            return;
        };
        let text = session.text.as_deref().filter(|t| !t.trim().is_empty());
        let ins = self.inspect(text.map(|t| (t, "dragged text")), &session.files, None, FILE_BUDGET);
        let (action, findings) = self.decide(&ins, Channel::Drop);
        let verdict = if action == Action::Allow {
            DragVerdict::Clean
        } else {
            DragVerdict::Dirty { findings, action, files: session.files.len() }
        };
        self.shared.set_drag_verdict(generation, verdict);
    }

    fn on_drop(&self, app: &AppContext, generation: u64, at: Point, strategy: DropStrategy, withheld: bool) {
        match strategy {
            DropStrategy::SwallowAndCancel => self.platform.cancel_drop(at),
            DropStrategy::Withhold => {
                std::thread::sleep(DROP_RESTORE_DELAY);
                if withheld {
                    self.platform.set_drag_withheld(false);
                }
            }
            DropStrategy::Unsupported => {}
        }
        let may_have_landed = strategy == DropStrategy::Unsupported || (strategy == DropStrategy::Withhold && !withheld);

        let deadline = Instant::now() + DROP_VERDICT_WAIT;
        let verdict = loop {
            let v = self.shared.drag_verdict(generation);
            if v != DragVerdict::Pending || Instant::now() >= deadline {
                break v;
            }
            std::thread::sleep(Duration::from_millis(25));
        };

        match verdict {
            DragVerdict::Clean => {
                if !may_have_landed {
                    // The drop was stopped only because the check had not finished: let the retry through.
                    self.shared.allow_drops(&app.app_id);
                    self.platform.banner("Veil checked it: nothing sensitive. Drop it again.");
                }
            }
            DragVerdict::Pending => {
                let ins = Inspection { complete: false, files: 1, ..Default::default() };
                if may_have_landed {
                    self.shared.state.lock().dirty_drops.insert(app.pid, (Instant::now(), Vec::new()));
                }
                let prompt = self.prompt(PromptKind::Drop, app, &[], &ins, vec![Choice::Ok, Choice::AllowFor30s]);
                if self.ask(&prompt) == Choice::AllowFor30s {
                    self.shared.allow_drops(&app.app_id);
                }
                self.log(app, Channel::Drop, &[], Outcome::Unchecked, 1);
            }
            DragVerdict::Dirty { findings, action, files } => {
                let ins = Inspection { complete: true, files, ..Default::default() };
                if may_have_landed {
                    self.shared.state.lock().dirty_drops.insert(app.pid, (Instant::now(), findings.clone()));
                }
                if action == Action::Block {
                    let prompt = self.prompt(PromptKind::BlockNotice, app, &findings, &ins, vec![Choice::Ok]);
                    self.ask(&prompt);
                    self.log(app, Channel::Drop, &findings, Outcome::Blocked, files);
                } else {
                    let prompt = self.prompt(PromptKind::Drop, app, &findings, &ins, vec![Choice::Ok, Choice::AllowFor30s]);
                    if self.ask(&prompt) == Choice::AllowFor30s {
                        self.shared.allow_drops(&app.app_id);
                        self.log(app, Channel::Drop, &findings, Outcome::SentAnyway, files);
                    } else {
                        self.log(app, Channel::Drop, &findings, Outcome::DropBlocked, files);
                    }
                }
            }
        }
    }
}

fn remaining(deadline: Instant) -> Duration {
    deadline.saturating_duration_since(Instant::now())
}
