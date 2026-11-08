//! End-to-end flows through the interceptor and engine with the mock platform and scanner.

use crate::activity::{ActivityLog, Outcome};
use crate::engine::{Engine, Job};
use crate::input::{HookVerdict, InputEvent, Key, Modifiers, Replay};
use crate::interceptor::Interceptor;
use crate::platform::{Choice, InputHandler, PromptKind};
use crate::policy::{Action, FailMode, Policy};
use crate::scanner::ModelStatus;
use crate::shared::Shared;
use crate::testing::*;
use crate::types::*;
use crossbeam_channel::Receiver;
use std::sync::Arc;
use std::time::Duration;

struct H {
    platform: Arc<MockPlatform>,
    scanner: Arc<MockScanner>,
    shared: Arc<Shared>,
    engine: Arc<Engine>,
    hook: Interceptor,
    jobs: Receiver<Job>,
    _dir: tempfile::TempDir,
}

impl H {
    fn new(platform: Arc<MockPlatform>, scanner: MockScanner) -> H {
        Self::with(platform, scanner, Policy::default(), Duration::from_millis(1500))
    }

    fn with(platform: Arc<MockPlatform>, scanner: MockScanner, policy: Policy, budget: Duration) -> H {
        let dir = tempfile::tempdir().unwrap();
        let scanner = Arc::new(scanner);
        let shared = Arc::new(Shared::new(policy));
        let activity = Arc::new(ActivityLog::new(&dir.path().join("activity.jsonl")));
        let engine = Arc::new(
            Engine::new(platform.clone(), scanner.clone(), shared.clone(), activity).with_message_budget(budget),
        );
        let (tx, rx) = crossbeam_channel::unbounded();
        let hook = Interceptor::new(platform.clone(), shared.clone(), tx);
        platform.focus(&chat_app());
        H { platform, scanner, shared, engine, hook, jobs: rx, _dir: dir }
    }

    fn input(&self, ev: InputEvent) -> HookVerdict {
        self.hook.on_input(&ev)
    }

    fn drain(&self) {
        while let Ok(job) = self.jobs.try_recv() {
            self.engine.handle(job);
        }
    }

    fn enter(&self) -> HookVerdict {
        self.input(InputEvent::KeyDown { key: Key::Enter, mods: Modifiers::NONE, text: Some("\r".into()) })
    }

    fn type_str(&self, s: &str) {
        for ch in s.chars() {
            let v = self.input(InputEvent::KeyDown { key: Key::Other(0), mods: Modifiers::NONE, text: Some(ch.to_string()) });
            assert_eq!(v, HookVerdict::Pass);
        }
    }

    fn paste(&self) -> HookVerdict {
        self.input(InputEvent::KeyDown { key: Key::V, mods: Modifiers::primary_only(), text: None })
    }

    fn outcomes(&self) -> Vec<Outcome> {
        self.engine.activity().recent(50).into_iter().map(|e| e.outcome).collect()
    }
}

fn enter_replay() -> Replay {
    Replay::Key { key: Key::Enter, mods: Modifiers::NONE, text: Some("\r".into()) }
}

fn file_with(dir: &std::path::Path, name: &str, content: &[u8]) -> std::path::PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, content).unwrap();
    p
}

#[test]
fn clean_message_is_replayed() {
    let h = H::new(MockPlatform::new(), MockScanner::default());
    h.platform.set_text("hello there");
    assert_eq!(h.enter(), HookVerdict::Swallow);
    h.drain();
    assert_eq!(h.platform.take_replays(), vec![enter_replay()]);
    assert!(h.platform.prompts.lock().is_empty());
    // The swallowed Enter's key-up is swallowed too (the replay sends its own).
    assert_eq!(h.input(InputEvent::KeyUp { key: Key::Enter, mods: Modifiers::NONE }), HookVerdict::Swallow);
    assert!(h.shared.state.lock().hold.is_none());
}

#[test]
fn empty_composer_passes_without_scanning() {
    let h = H::new(MockPlatform::new(), MockScanner::default());
    h.platform.set_text("   ");
    h.enter();
    h.drain();
    assert_eq!(h.platform.take_replays(), vec![enter_replay()]);
}

#[test]
fn shift_enter_is_a_newline_not_a_send() {
    let h = H::new(MockPlatform::new(), MockScanner::default());
    let v = h.input(InputEvent::KeyDown { key: Key::Enter, mods: Modifiers { shift: true, ..Modifiers::NONE }, text: None });
    assert_eq!(v, HookVerdict::Pass);
    assert!(h.jobs.try_recv().is_ok_and(|j| matches!(j, Job::Typed { .. })));
}

#[test]
fn sensitive_message_dont_send_drops_it() {
    let h = H::new(MockPlatform::new(), MockScanner::default());
    h.platform.set_text("this is confidential");
    h.platform.answer(Choice::DontSend);
    h.enter();
    h.drain();
    assert!(h.platform.take_replays().is_empty());
    let prompts = h.platform.prompts.lock();
    assert_eq!(prompts[0].kind, PromptKind::Send);
    assert_eq!(prompts[0].options, vec![Choice::DontSend, Choice::SendAnyway, Choice::Redact]);
    assert!(prompts[0].message.contains("the keyword “confidential”"), "{}", prompts[0].message);
    drop(prompts);
    assert_eq!(h.outcomes(), vec![Outcome::Cancelled]);
    assert!(h.shared.state.lock().hold.is_none());
}

#[test]
fn send_anyway_replays_trigger_then_keys_typed_meanwhile() {
    let h = H::new(MockPlatform::new(), MockScanner::default());
    h.platform.set_text("confidential plan");
    h.platform.answer(Choice::SendAnyway);
    h.enter();
    let x = InputEvent::KeyDown { key: Key::Other(7), mods: Modifiers::NONE, text: Some("x".into()) };
    assert_eq!(h.input(x), HookVerdict::Swallow, "typing during a hold is queued");
    h.drain();
    assert_eq!(
        h.platform.take_replays(),
        vec![enter_replay(), Replay::KeyDown { key: Key::Other(7), mods: Modifiers::NONE, text: Some("x".into()) }]
    );
    assert_eq!(h.outcomes(), vec![Outcome::SentAnyway]);
}

#[test]
fn redact_replaces_text_through_the_platform() {
    let h = H::new(MockPlatform::with_replace(), MockScanner::default());
    h.platform.set_text("token SECRET-abc123 here");
    h.platform.answer(Choice::Redact);
    h.enter();
    h.drain();
    assert_eq!(h.platform.replaced.lock().as_deref(), Some("token [GENERIC_SECRET] here"));
    assert_eq!(h.platform.take_replays(), vec![enter_replay()]);
    assert_eq!(h.outcomes(), vec![Outcome::Redacted]);
}

#[test]
fn redact_falls_back_to_select_all_and_paste_and_restores_clipboard() {
    let h = H::new(MockPlatform::new(), MockScanner::default());
    *h.platform.clipboard.lock() = ClipboardContent { text: Some("user clipboard".into()), ..Default::default() };
    h.platform.set_text("token SECRET-abc123");
    h.platform.answer(Choice::Redact);
    h.enter();
    h.drain();
    assert_eq!(h.platform.take_replays(), vec![Replay::select_all(), Replay::paste(), enter_replay()]);
    assert_eq!(h.platform.clipboard.lock().text.as_deref(), Some("user clipboard"));
}

#[test]
fn typed_buffer_is_used_when_the_app_hides_its_text_and_redact_is_not_offered() {
    let h = H::new(MockPlatform::new(), MockScanner::default());
    h.type_str("this is confidential");
    h.drain();
    h.platform.answer(Choice::DontSend);
    h.enter();
    h.drain();
    let prompts = h.platform.prompts.lock();
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0].options, vec![Choice::DontSend, Choice::SendAnyway]);
}

#[test]
fn typing_warning_is_shown_once_per_message() {
    let h = H::new(MockPlatform::new(), MockScanner::default());
    h.type_str("so confidential");
    h.drain();
    h.type_str(" and more");
    h.drain();
    assert_eq!(h.platform.banners.lock().len(), 1);
    // After sending, the next sensitive message warns again.
    h.platform.answer(Choice::SendAnyway);
    h.enter();
    h.drain();
    h.type_str("confidential again");
    h.drain();
    assert_eq!(h.platform.banners.lock().len(), 2);
}

#[test]
fn excluded_apps_are_not_touched() {
    let h = H::new(MockPlatform::new(), MockScanner::default());
    h.platform.focus(&AppContext { app_id: "com.apple.Terminal".into(), name: "Terminal".into(), pid: 9, ..Default::default() });
    h.platform.set_text("confidential");
    assert_eq!(h.enter(), HookVerdict::Pass);
    assert!(h.jobs.try_recv().is_err());
}

#[test]
fn pause_lets_everything_through() {
    let h = H::new(MockPlatform::new(), MockScanner::default());
    assert!(h.engine.pause(Duration::from_secs(60)));
    h.platform.set_text("confidential");
    assert_eq!(h.enter(), HookVerdict::Pass);
    h.engine.resume();
    assert_eq!(h.enter(), HookVerdict::Swallow);
}

#[test]
fn block_policy_shows_a_notice_and_never_sends() {
    let mut policy = Policy::default();
    policy.actions.default = Action::Block;
    let h = H::with(MockPlatform::new(), MockScanner::default(), policy, Duration::from_secs(1));
    h.platform.set_text("confidential");
    h.enter();
    h.drain();
    assert!(h.platform.take_replays().is_empty());
    let p = h.platform.prompts.lock();
    assert_eq!(p[0].kind, PromptKind::BlockNotice);
    assert_eq!(p[0].options, vec![Choice::Ok]);
}

#[test]
fn sensitive_paste_can_be_redacted_and_clipboard_is_restored() {
    let h = H::new(MockPlatform::new(), MockScanner::default());
    let original = ClipboardContent { text: Some("key SECRET-zzz ok".into()), ..Default::default() };
    *h.platform.clipboard.lock() = original.clone();
    h.platform.answer(Choice::Redact);
    assert_eq!(h.paste(), HookVerdict::Swallow);
    h.drain();
    assert_eq!(h.platform.take_replays(), vec![Replay::Key { key: Key::V, mods: Modifiers::primary_only(), text: None }]);
    assert_eq!(*h.platform.clipboard.lock(), original);
    assert_eq!(h.shared.buffer_text(42), "key [GENERIC_SECRET] ok");
    assert_eq!(h.outcomes(), vec![Outcome::Redacted]);
}

#[test]
fn clean_paste_goes_through_and_feeds_the_buffer() {
    let h = H::new(MockPlatform::new(), MockScanner::default());
    *h.platform.clipboard.lock() = ClipboardContent { text: Some("hello".into()), ..Default::default() };
    h.paste();
    h.drain();
    assert_eq!(h.platform.take_replays().len(), 1);
    assert_eq!(h.shared.buffer_text(42), "hello");
}

#[test]
fn pasted_image_is_checked_with_ocr_and_cannot_be_redacted() {
    let h = H::new(MockPlatform::new(), MockScanner::default());
    *h.platform.clipboard.lock() = ClipboardContent { image: Some(vec![1, 2, 3]), ..Default::default() };
    *h.platform.ocr_text.lock() = Some("CONFIDENTIAL numbers".into());
    h.platform.answer(Choice::DontSend);
    h.paste();
    h.drain();
    let p = h.platform.prompts.lock();
    assert_eq!(p[0].kind, PromptKind::Paste);
    assert_eq!(p[0].options, vec![Choice::DontSend, Choice::SendAnyway]);
    drop(p);
    assert!(h.platform.take_replays().is_empty());
}

#[test]
fn send_button_click_is_held_and_replayed() {
    let h = H::new(MockPlatform::new(), MockScanner::default());
    *h.platform.element.lock() =
        Some(UiElement { role: "AXButton".into(), label: "send message".into(), pid: 42, is_button: true, ..Default::default() });
    h.platform.set_text("all good");
    let at = Point::new(10.0, 10.0);
    assert_eq!(h.input(InputEvent::MouseDown { at, clicks: 1 }), HookVerdict::Swallow);
    assert_eq!(h.input(InputEvent::MouseUp { at }), HookVerdict::Swallow);
    h.drain();
    assert_eq!(h.platform.take_replays(), vec![Replay::Click { at, clicks: 1 }]);
}

#[test]
fn buttons_of_other_processes_are_ignored() {
    // e.g. Veil's own popup button "Send anyway" floating over the app.
    let h = H::new(MockPlatform::new(), MockScanner::default());
    *h.platform.element.lock() =
        Some(UiElement { role: "AXButton".into(), label: "send".into(), pid: 999, is_button: true, ..Default::default() });
    assert_eq!(h.input(InputEvent::MouseDown { at: Point::new(1.0, 1.0), clicks: 1 }), HookVerdict::Pass);
}

#[test]
fn unlabelled_web_click_is_held_only_while_the_text_looks_sensitive() {
    let h = H::new(MockPlatform::new(), MockScanner::default());
    let app = browser_app("https://chatgpt.com/");
    h.platform.focus(&app);
    *h.platform.element.lock() = Some(UiElement { role: "AXGroup".into(), pid: 77, is_generic: true, ..Default::default() });
    let at = Point::new(5.0, 5.0);
    assert_eq!(h.input(InputEvent::MouseDown { at, clicks: 1 }), HookVerdict::Pass);
    h.input(InputEvent::MouseUp { at });
    h.type_str("confidential");
    h.drain();
    h.platform.answer(Choice::DontSend);
    assert_eq!(h.input(InputEvent::MouseDown { at, clicks: 1 }), HookVerdict::Swallow);
    h.input(InputEvent::MouseUp { at });
    h.drain();
    assert_eq!(h.platform.prompts.lock().len(), 1);
    // Not asked again for the next click until the text changes.
    assert_eq!(h.input(InputEvent::MouseDown { at, clicks: 1 }), HookVerdict::Pass);
}

#[test]
fn file_picker_selection_is_checked_before_open() {
    let dir = tempfile::tempdir().unwrap();
    let h = H::new(MockPlatform::new(), MockScanner::default());
    *h.platform.picker.lock() = vec![file_with(dir.path(), "plan.txt", b"strictly confidential numbers")];
    *h.platform.element.lock() = Some(UiElement {
        role: "AXButton".into(),
        label: "open".into(),
        pid: 42,
        is_button: true,
        in_file_dialog: true,
        ..Default::default()
    });
    let at = Point::new(3.0, 3.0);
    h.platform.answer(Choice::SendAnyway);
    assert_eq!(h.input(InputEvent::MouseDown { at, clicks: 1 }), HookVerdict::Swallow);
    h.drain();
    let p = h.platform.prompts.lock().clone();
    assert_eq!(p[0].kind, PromptKind::Upload);
    assert!(p[0].message.contains("plan.txt"));
    assert_eq!(h.platform.take_replays(), vec![Replay::Click { at, clicks: 1 }]);
}

#[test]
fn files_that_cannot_be_checked_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let h = H::new(MockPlatform::new(), MockScanner::default());
    *h.platform.picker.lock() = vec![file_with(dir.path(), "huge.bin", &vec![b'a'; 2 * 1024 * 1024])];
    *h.platform.element.lock() =
        Some(UiElement { label: "open".into(), pid: 42, is_button: true, in_file_dialog: true, ..Default::default() });
    h.input(InputEvent::MouseDown { at: Point::default(), clicks: 1 });
    h.drain();
    let p = h.platform.prompts.lock();
    assert!(p[0].message.contains("Couldn't be checked"), "{}", p[0].message);
    assert!(h.platform.replays.lock().is_empty());
}

fn drag(h: &H, from: Point) {
    h.input(InputEvent::MouseDown { at: from, clicks: 1 });
    h.input(InputEvent::MouseDrag { at: Point::new(from.x + 10.0, from.y) });
}

#[test]
fn sensitive_drop_is_withheld_then_can_be_allowed_for_30s() {
    let dir = tempfile::tempdir().unwrap();
    let h = H::new(MockPlatform::new(), MockScanner::default());
    *h.platform.drag.lock() =
        Some(DragSession { files: vec![file_with(dir.path(), "a.txt", b"confidential")], ..Default::default() });
    let to = Point::new(100.0, 100.0);
    drag(&h, Point::default());
    h.drain(); // DragStart → dirty verdict
    h.input(InputEvent::MouseDrag { at: to });
    assert_eq!(*h.platform.withheld.lock(), vec![true]);
    h.platform.answer(Choice::AllowFor30s);
    assert_eq!(h.input(InputEvent::MouseUp { at: to }), HookVerdict::Pass);
    h.drain();
    assert_eq!(*h.platform.withheld.lock(), vec![true, false], "data restored after the drop");
    assert_eq!(h.platform.prompts.lock()[0].kind, PromptKind::Drop);
    assert!(h.shared.drops_allowed("com.example.chat"));
    // The retry passes untouched.
    drag(&h, Point::default());
    h.input(InputEvent::MouseDrag { at: to });
    assert_eq!(h.input(InputEvent::MouseUp { at: to }), HookVerdict::Pass);
    assert!(!h.jobs.try_iter().any(|j| matches!(j, Job::Drop { .. })));
}

#[test]
fn drop_before_the_check_finishes_is_stopped_then_allowed_if_clean() {
    let dir = tempfile::tempdir().unwrap();
    let h = H::new(MockPlatform::new(), MockScanner::default());
    *h.platform.drag.lock() =
        Some(DragSession { files: vec![file_with(dir.path(), "a.txt", b"harmless")], ..Default::default() });
    let to = Point::new(50.0, 50.0);
    drag(&h, Point::default());
    h.input(InputEvent::MouseDrag { at: to });
    h.input(InputEvent::MouseUp { at: to });
    h.drain();
    assert!(h.platform.prompts.lock().is_empty());
    assert!(h.shared.drops_allowed("com.example.chat"));
    assert_eq!(h.platform.banners.lock().len(), 1);
}

#[test]
fn swallow_and_cancel_platforms_cancel_the_drop() {
    let dir = tempfile::tempdir().unwrap();
    let h = H::new(MockPlatform::new(), MockScanner::default());
    *h.platform.strategy.lock() = Some(DropStrategy::SwallowAndCancel);
    *h.platform.drag.lock() =
        Some(DragSession { files: vec![file_with(dir.path(), "a.txt", b"confidential")], ..Default::default() });
    let to = Point::new(50.0, 50.0);
    drag(&h, Point::default());
    h.drain();
    assert_eq!(h.input(InputEvent::MouseUp { at: to }), HookVerdict::Swallow);
    h.drain();
    assert_eq!(*h.platform.cancelled_drops.lock(), 1);
    assert!(h.platform.withheld.lock().is_empty());
}

#[test]
fn unblockable_drop_arms_the_backstop_for_the_next_send() {
    let dir = tempfile::tempdir().unwrap();
    let h = H::new(MockPlatform::new(), MockScanner::default());
    *h.platform.strategy.lock() = Some(DropStrategy::Unsupported);
    *h.platform.drag.lock() =
        Some(DragSession { files: vec![file_with(dir.path(), "a.txt", b"confidential")], ..Default::default() });
    let to = Point::new(50.0, 50.0);
    drag(&h, Point::default());
    h.drain();
    h.input(InputEvent::MouseUp { at: to });
    h.drain();
    h.platform.set_text("here you go");
    h.platform.answer(Choice::DontSend);
    h.enter();
    h.drain();
    let kinds: Vec<_> = h.platform.prompts.lock().iter().map(|p| p.kind).collect();
    assert_eq!(kinds, vec![PromptKind::Drop, PromptKind::Backstop]);
    assert!(h.platform.take_replays().is_empty());
}

#[test]
fn ai_layer_runs_once_per_text_thanks_to_the_cache() {
    let h = H::new(MockPlatform::new(), MockScanner::with_model());
    h.platform.set_text("def leak(): return 1");
    h.platform.answer(Choice::DontSend);
    h.platform.answer(Choice::DontSend);
    h.enter();
    h.drain();
    h.enter();
    h.drain();
    assert_eq!(h.platform.prompts.lock().len(), 2);
    assert!(h.platform.prompts.lock()[0].message.contains("source code"));
    assert_eq!(*h.scanner.model_calls.lock(), 1);
}

#[test]
fn slow_ai_layer_fails_open_for_text_by_default() {
    let scanner = MockScanner { model: parking_lot::Mutex::new(ModelStatus::Ready), model_delay: Duration::from_millis(300), ..Default::default() };
    let h = H::with(MockPlatform::new(), scanner, Policy::default(), Duration::from_millis(30));
    h.platform.set_text("def leak(): pass");
    h.enter();
    h.drain();
    assert!(h.platform.prompts.lock().is_empty());
    assert_eq!(h.platform.take_replays(), vec![enter_replay()]);
}

#[test]
fn slow_ai_layer_can_fail_closed() {
    let mut policy = Policy::default();
    policy.actions.fail_mode.text = FailMode::Closed;
    let scanner = MockScanner { model: parking_lot::Mutex::new(ModelStatus::Ready), model_delay: Duration::from_millis(300), ..Default::default() };
    let h = H::with(MockPlatform::new(), scanner, policy, Duration::from_millis(30));
    h.platform.set_text("plain words");
    h.enter();
    h.drain();
    let p = h.platform.prompts.lock();
    assert!(p[0].message.contains("couldn't finish"), "{}", p[0].message);
}
