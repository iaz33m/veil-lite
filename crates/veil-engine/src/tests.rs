//! The engine service behind a real IPC server, with the mock platform and scanner (no OS hooks).

use crate::service::{ModelStore, ScannerControl, Service};
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use veil_core::activity::{ActivityEntry, ActivityLog, Outcome};
use veil_core::paths::Paths;
use veil_core::policy::Policy;
use veil_core::scanner::ModelStatus;
use veil_core::shared::Shared;
use veil_core::testing::{chat_app, MockPlatform, MockScanner};
use veil_core::types::Channel;
use veil_core::{Engine, EngineEvent};
use veil_ipc::{Client, Request, Response, Server, Subscription};

struct Control {
    scanner: Arc<MockScanner>,
    installed: Arc<AtomicBool>,
    applied: Mutex<Vec<Policy>>,
}

impl ScannerControl for Control {
    fn apply_policy(&self, policy: &Policy) {
        self.applied.lock().push(policy.clone());
    }
    fn refresh_model(&self) {
        *self.scanner.model.lock() =
            if self.installed.load(Ordering::SeqCst) { ModelStatus::Ready } else { ModelStatus::Missing };
    }
    fn set_download_progress(&self, percent: Option<u8>) {
        if let Some(percent) = percent {
            *self.scanner.model.lock() = ModelStatus::Downloading { percent };
        }
    }
    fn set_model_failed(&self, reason: String) {
        *self.scanner.model.lock() = ModelStatus::Failed { reason };
    }
}

struct Models {
    installed: Arc<AtomicBool>,
    fail: bool,
}

impl ModelStore for Models {
    fn is_installed(&self) -> bool {
        self.installed.load(Ordering::SeqCst)
    }
    fn download(&self, cancel: &AtomicBool, progress: &mut dyn FnMut(u64, u64)) -> anyhow::Result<()> {
        for done in [0u64, 250, 500, 1000] {
            if cancel.load(Ordering::SeqCst) {
                anyhow::bail!("cancelled");
            }
            progress(done, 1000);
            std::thread::sleep(Duration::from_millis(5));
        }
        if self.fail {
            anyhow::bail!("checksum mismatch");
        }
        self.installed.store(true, Ordering::SeqCst);
        Ok(())
    }
    fn remove(&self) -> std::io::Result<()> {
        self.installed.store(false, Ordering::SeqCst);
        Ok(())
    }
}

struct Rig {
    _dir: tempfile::TempDir,
    paths: Paths,
    service: Arc<Service>,
    control: Arc<Control>,
    _server: Server,
}

fn rig(fail_download: bool) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::rooted(dir.path());
    paths.ensure().unwrap();
    let policy = Policy::load_or_create(&paths.policy_file()).unwrap();
    let platform = MockPlatform::new();
    let scanner = Arc::new(MockScanner::default());
    let shared = Arc::new(Shared::new(policy));
    let activity = Arc::new(ActivityLog::new(&paths.activity_file()));
    let engine = Arc::new(Engine::new(platform.clone(), scanner.clone(), shared, activity));
    let installed = Arc::new(AtomicBool::new(false));
    let control = Arc::new(Control { scanner, installed: installed.clone(), applied: Mutex::new(Vec::new()) });
    let models = Arc::new(Models { installed, fail: fail_download });
    let service = Service::new(paths.clone(), engine, platform, control.clone(), models);
    let server = Server::start(&paths, service.clone()).unwrap();
    Rig { _dir: dir, paths, service, control, _server: server }
}

fn next_event(sub: &mut Subscription, want: impl Fn(&EngineEvent) -> bool) -> EngineEvent {
    for ev in sub.by_ref() {
        if want(&ev) {
            return ev;
        }
    }
    panic!("subscription ended");
}

#[test]
fn status_reports_platform_model_and_permissions() {
    let r = rig(false);
    let mut c = Client::connect(&r.paths).unwrap();
    let Response::Status(s) = c.call(&Request::Status).unwrap() else { panic!() };
    assert_eq!(s.platform, "mock");
    assert!(s.active);
    assert!(s.permissions_ok);
    assert!(s.drops_blockable);
    assert!(!s.ocr_available);
    assert_eq!(s.model, ModelStatus::Missing);
    assert_eq!(s.paused_remaining_secs, None);
}

#[test]
fn set_policy_validates_saves_applies_and_notifies() {
    let r = rig(false);
    let mut sub = Client::subscribe(&r.paths).unwrap();
    let mut c = Client::connect(&r.paths).unwrap();

    let Response::Policy { yaml } = c.call(&Request::GetPolicy).unwrap() else { panic!() };
    let mut policy = Policy::from_yaml(&yaml).unwrap();
    policy.keywords = vec!["project falcon".into()];
    let new_yaml = format!("# my notes\n{}", policy.to_yaml());
    assert_eq!(c.call(&Request::SetPolicy { yaml: new_yaml.clone() }).unwrap(), Response::Ok);

    assert_eq!(r.service.engine().shared().policy.read().keywords, vec!["project falcon"]);
    assert_eq!(std::fs::read_to_string(r.paths.policy_file()).unwrap(), new_yaml);
    assert_eq!(r.control.applied.lock().last().unwrap().keywords, vec!["project falcon"]);
    next_event(&mut sub, |e| *e == EngineEvent::PolicyChanged);
    // The file as written (with comments) is returned.
    assert_eq!(c.call(&Request::GetPolicy).unwrap(), Response::Policy { yaml: new_yaml.clone() });

    let bad = c.call(&Request::SetPolicy { yaml: "checks:\n  nope: {threshold: 2, action: ask}\n".into() }).unwrap();
    assert!(matches!(bad, Response::Error { .. }), "{bad:?}");
    assert_eq!(r.service.engine().shared().policy.read().keywords, vec!["project falcon"]);
    assert_eq!(std::fs::read_to_string(r.paths.policy_file()).unwrap(), new_yaml);
    // Our own write is not seen as an external edit.
    let applied = r.control.applied.lock().len();
    r.service.check_policy_file();
    assert_eq!(r.control.applied.lock().len(), applied);
}

#[test]
fn edits_to_the_policy_file_are_picked_up_and_bad_edits_are_reported() {
    let r = rig(false);
    let mut sub = Client::subscribe(&r.paths).unwrap();
    let mut p = Policy::default();
    p.keywords = vec!["edited".into()];
    std::fs::write(r.paths.policy_file(), p.to_yaml()).unwrap();
    r.service.check_policy_file();
    assert_eq!(r.service.engine().shared().policy.read().keywords, vec!["edited"]);
    next_event(&mut sub, |e| *e == EngineEvent::PolicyChanged);

    std::fs::write(r.paths.policy_file(), "pii: {regions: [mars]}\n").unwrap();
    r.service.check_policy_file();
    assert_eq!(r.service.engine().shared().policy.read().keywords, vec!["edited"]);
    let ev = next_event(&mut sub, |e| matches!(e, EngineEvent::Warning { .. }));
    let EngineEvent::Warning { message } = ev else { unreachable!() };
    assert!(message.contains("invalid"), "{message}");
}

#[test]
fn pause_and_resume() {
    let r = rig(false);
    let mut sub = Client::subscribe(&r.paths).unwrap();
    let mut c = Client::connect(&r.paths).unwrap();
    assert_eq!(c.call(&Request::Pause { minutes: 5 }).unwrap(), Response::Ok);
    assert_eq!(next_event(&mut sub, |_| true), EngineEvent::Paused { remaining_secs: 900 });
    let Response::Status(s) = c.call(&Request::Status).unwrap() else { panic!() };
    assert!(s.paused_remaining_secs.is_some_and(|secs| (890..=900).contains(&secs)));
    assert_eq!(c.call(&Request::Pause { minutes: 45 }).unwrap(), Response::Ok);
    assert_eq!(next_event(&mut sub, |_| true), EngineEvent::Paused { remaining_secs: 3600 });
    assert_eq!(c.call(&Request::Resume).unwrap(), Response::Ok);
    assert_eq!(next_event(&mut sub, |_| true), EngineEvent::Resumed);
    let Response::Status(s) = c.call(&Request::Status).unwrap() else { panic!() };
    assert_eq!(s.paused_remaining_secs, None);
    assert!(matches!(c.call(&Request::Pause { minutes: 0 }).unwrap(), Response::Error { .. }));

    let mut p = Policy::default();
    p.pause.allowed = false;
    assert_eq!(c.call(&Request::SetPolicy { yaml: p.to_yaml() }).unwrap(), Response::Ok);
    assert!(matches!(c.call(&Request::Pause { minutes: 15 }).unwrap(), Response::Error { .. }));
}

#[test]
fn activity_is_listed_newest_first_and_can_be_cleared() {
    let r = rig(false);
    let log = r.service.engine().activity();
    for outcome in [Outcome::Cancelled, Outcome::SentAnyway] {
        log.append(&ActivityEntry::new(&chat_app(), Channel::Message, &[], outcome, 0));
    }
    let mut c = Client::connect(&r.paths).unwrap();
    let Response::Activity { entries } = c.call(&Request::Activity { limit: 10 }).unwrap() else { panic!() };
    assert_eq!(entries.iter().map(|e| e.outcome).collect::<Vec<_>>(), vec![Outcome::SentAnyway, Outcome::Cancelled]);
    assert_eq!(c.call(&Request::ClearActivity).unwrap(), Response::Ok);
    assert_eq!(c.call(&Request::Activity { limit: 10 }).unwrap(), Response::Activity { entries: vec![] });
}

#[test]
fn model_download_reports_progress_then_ready_and_can_be_removed() {
    let r = rig(false);
    let mut sub = Client::subscribe(&r.paths).unwrap();
    let mut c = Client::connect(&r.paths).unwrap();
    assert_eq!(c.call(&Request::DownloadModel).unwrap(), Response::Ok);
    let mut seen = Vec::new();
    loop {
        let EngineEvent::Model { status } = next_event(&mut sub, |e| matches!(e, EngineEvent::Model { .. })) else {
            unreachable!()
        };
        seen.push(status.clone());
        if status == ModelStatus::Ready {
            break;
        }
    }
    assert!(seen.contains(&ModelStatus::Downloading { percent: 50 }), "{seen:?}");
    assert_eq!(c.call(&Request::ModelStatus).unwrap(), Response::Model { status: ModelStatus::Ready });
    // Already installed: no second download.
    assert_eq!(c.call(&Request::DownloadModel).unwrap(), Response::Ok);
    assert_eq!(c.call(&Request::RemoveModel).unwrap(), Response::Ok);
    assert_eq!(c.call(&Request::ModelStatus).unwrap(), Response::Model { status: ModelStatus::Missing });
}

#[test]
fn failed_download_is_reported() {
    let r = rig(true);
    let mut c = Client::connect(&r.paths).unwrap();
    c.call(&Request::DownloadModel).unwrap();
    for _ in 0..200 {
        if let Response::Model { status: ModelStatus::Failed { reason } } = c.call(&Request::ModelStatus).unwrap() {
            assert!(reason.contains("checksum"), "{reason}");
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("download never failed");
}

#[test]
fn permissions_and_shutdown() {
    let r = rig(false);
    let mut c = Client::connect(&r.paths).unwrap();
    let Response::Permissions { list } = c.call(&Request::Permissions).unwrap() else { panic!() };
    assert_eq!(list.len(), 1);
    assert_eq!(
        c.call(&Request::RequestPermission { id: "mock".into() }).unwrap(),
        Response::PermissionResult { granted: false }
    );
    assert_eq!(c.call(&Request::Shutdown).unwrap(), Response::Ok);
}
