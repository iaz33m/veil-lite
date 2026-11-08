//! The engine's IPC service: answers the desktop app, applies policy changes (from the app or from edits
//! to `policy.yaml`), and runs the model download.

use crossbeam_channel::Receiver;
use parking_lot::Mutex;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, SystemTime};
use veil_core::paths::Paths;
use veil_core::policy::Policy;
use veil_core::scanner::ModelStatus;
use veil_core::types::DropStrategy;
use veil_core::{Engine, EngineEvent, Platform};
use veil_ipc::{Handler, Request, Response, StatusInfo, ENGINE_VERSION};

const POLICY_POLL: Duration = Duration::from_secs(2);
const MAX_ACTIVITY: usize = 1000;

/// Control surface of the detection pipeline beyond the `Scanner` trait (implemented for `VeilScanner`).
pub trait ScannerControl: Send + Sync {
    /// Keywords, personal-data regions, model on/off.
    fn apply_policy(&self, policy: &Policy);
    /// Re-check the model files on disk (after a download or removal).
    fn refresh_model(&self);
    /// `Some(percent)` while downloading; `None` when done.
    fn set_download_progress(&self, percent: Option<u8>);
    fn set_model_failed(&self, reason: String);
}

/// Where the AI model lives (implemented with `veil_detect::model`).
pub trait ModelStore: Send + Sync {
    fn is_installed(&self) -> bool;
    fn download(&self, cancel: &AtomicBool, progress: &mut dyn FnMut(u64, u64)) -> anyhow::Result<()>;
    fn remove(&self) -> std::io::Result<()>;
}

/// File stamp used to notice edits to `policy.yaml`.
type Stamp = Option<(SystemTime, u64)>;

pub struct Service {
    me: Weak<Service>,
    paths: Paths,
    engine: Arc<Engine>,
    platform: Arc<dyn Platform>,
    scanner: Arc<dyn ScannerControl>,
    models: Arc<dyn ModelStore>,
    download: Mutex<Option<Arc<AtomicBool>>>,
    policy_stamp: Mutex<Stamp>,
    policy_io: Mutex<()>,
    stopping: AtomicBool,
    /// OS hooks are installed and running.
    active: AtomicBool,
}

impl Service {
    pub fn new(
        paths: Paths,
        engine: Arc<Engine>,
        platform: Arc<dyn Platform>,
        scanner: Arc<dyn ScannerControl>,
        models: Arc<dyn ModelStore>,
    ) -> Arc<Service> {
        let stamp = stamp(&paths.policy_file());
        let active = AtomicBool::new(platform.name() != "unsupported");
        Arc::new_cyclic(|me| Service {
            me: me.clone(),
            paths,
            engine,
            platform,
            scanner,
            models,
            download: Mutex::new(None),
            policy_stamp: Mutex::new(stamp),
            policy_io: Mutex::new(()),
            stopping: AtomicBool::new(false),
            active,
        })
    }

    pub fn engine(&self) -> &Arc<Engine> {
        &self.engine
    }

    /// Record whether the OS hooks are running (reported in `Status`).
    pub fn set_active(&self, active: bool) {
        self.active.store(active, Ordering::SeqCst);
    }

    pub fn is_stopping(&self) -> bool {
        self.stopping.load(Ordering::SeqCst)
    }

    /// Poll `policy.yaml` for edits made outside the app.
    pub fn start_policy_watcher(&self) {
        let me = self.me.clone();
        let spawned = std::thread::Builder::new().name("veil-policy".into()).spawn(move || loop {
            std::thread::sleep(POLICY_POLL);
            let Some(service) = me.upgrade() else { return };
            if service.stopping.load(Ordering::SeqCst) {
                return;
            }
            service.check_policy_file();
        });
        if let Err(e) = spawned {
            tracing::error!("cannot start the policy watcher: {e}");
        }
    }

    /// Reload `policy.yaml` if it changed since it was last read or written. Invalid files are ignored
    /// (the previous policy stays) and reported once per change.
    pub fn check_policy_file(&self) {
        let path = self.paths.policy_file();
        let _io = self.policy_io.lock();
        let now = stamp(&path);
        {
            let mut last = self.policy_stamp.lock();
            if *last == now {
                return;
            }
            *last = now;
        }
        if now.is_none() {
            return;
        }
        match Policy::load(&path) {
            Ok(policy) => {
                if policy != *self.engine.shared().policy.read() {
                    tracing::info!("policy.yaml changed; applying");
                    self.apply(policy);
                }
            }
            Err(e) => {
                let message = format!("policy.yaml is invalid, keeping the previous policy: {e}");
                tracing::warn!("{message}");
                self.engine.events.publish(EngineEvent::Warning { message });
            }
        }
    }

    fn apply(&self, policy: Policy) {
        self.scanner.apply_policy(&policy);
        self.engine.apply_policy(policy);
    }

    fn set_policy(&self, yaml: &str) -> Response {
        let policy = match Policy::from_yaml(yaml) {
            Ok(p) => p,
            Err(e) => return Response::Error { message: format!("invalid policy: {e}") },
        };
        let path = self.paths.policy_file();
        {
            let _io = self.policy_io.lock();
            if let Err(e) = write_atomic(&path, yaml.as_bytes()) {
                return Response::Error { message: format!("cannot save the policy: {e}") };
            }
            *self.policy_stamp.lock() = stamp(&path);
        }
        self.apply(policy);
        Response::Ok
    }

    fn get_policy(&self) -> Response {
        // Prefer the file as written (keeps the user's comments) when it is the policy in force.
        let current = self.engine.shared().policy.read().clone();
        if let Ok(text) = std::fs::read_to_string(self.paths.policy_file()) {
            if Policy::from_yaml(&text).is_ok_and(|p| p == current) {
                return Response::Policy { yaml: text };
            }
        }
        Response::Policy { yaml: current.to_yaml() }
    }

    pub fn status(&self) -> StatusInfo {
        let name = self.platform.name();
        StatusInfo {
            engine_version: ENGINE_VERSION.into(),
            platform: name.into(),
            active: name != "unsupported" && self.active.load(Ordering::SeqCst),
            paused_remaining_secs: self.engine.shared().paused_remaining().map(|d| d.as_secs().max(1)),
            model: self.engine.scanner().model_status(),
            permissions_ok: self.platform.permissions().iter().all(|p| p.granted),
            drops_blockable: self.platform.drop_strategy() != DropStrategy::Unsupported,
            ocr_available: matches!(name, "macos" | "windows"),
        }
    }

    fn publish_model(&self) {
        self.engine.events.publish(EngineEvent::Model { status: self.engine.scanner().model_status() });
    }

    fn start_download(&self) -> Response {
        if self.models.is_installed() {
            self.scanner.refresh_model();
            self.publish_model();
            return Response::Ok;
        }
        let cancel = {
            let mut slot = self.download.lock();
            if slot.is_some() {
                return Response::Ok;
            }
            let cancel = Arc::new(AtomicBool::new(false));
            *slot = Some(Arc::clone(&cancel));
            cancel
        };
        let Some(me) = self.me.upgrade() else { return Response::Error { message: "engine is stopping".into() } };
        self.scanner.set_download_progress(Some(0));
        self.publish_model();
        let spawned = std::thread::Builder::new().name("veil-download".into()).spawn(move || me.run_download(&cancel));
        match spawned {
            Ok(_) => Response::Ok,
            Err(e) => {
                *self.download.lock() = None;
                self.scanner.set_download_progress(None);
                Response::Error { message: format!("cannot start the download: {e}") }
            }
        }
    }

    fn run_download(&self, cancel: &AtomicBool) {
        tracing::info!("model download started");
        let mut last = 0u8;
        let result = self.models.download(cancel, &mut |done, total| {
            let pct = if total == 0 { 0 } else { (done.saturating_mul(100) / total).min(99) as u8 };
            if pct != last {
                last = pct;
                self.scanner.set_download_progress(Some(pct));
                self.publish_model();
            }
        });
        self.scanner.set_download_progress(None);
        match result {
            Ok(()) => tracing::info!("model download finished"),
            Err(_) if cancel.load(Ordering::SeqCst) => tracing::info!("model download cancelled"),
            Err(e) => {
                tracing::warn!("model download failed: {e:#}");
                self.scanner.set_model_failed(format!("{e:#}"));
            }
        }
        if !matches!(self.engine.scanner().model_status(), ModelStatus::Failed { .. }) {
            self.scanner.refresh_model();
        }
        *self.download.lock() = None;
        self.publish_model();
    }

    fn cancel_download(&self) -> bool {
        match self.download.lock().as_ref() {
            Some(flag) => {
                flag.store(true, Ordering::SeqCst);
                true
            }
            None => false,
        }
    }

    fn remove_model(&self) -> Response {
        if self.cancel_download() {
            for _ in 0..100 {
                if self.download.lock().is_none() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        self.engine.scanner().unload_model();
        let result = self.models.remove();
        self.scanner.refresh_model();
        self.publish_model();
        match result {
            Ok(()) => Response::Ok,
            Err(e) => Response::Error { message: format!("cannot remove the model: {e}") },
        }
    }

    fn pause(&self, minutes: u32) -> Response {
        if minutes == 0 {
            return Response::Error { message: "pause needs a duration".into() };
        }
        let minutes = if minutes <= 15 { 15 } else { 60 };
        if self.engine.pause(Duration::from_secs(minutes * 60)) {
            Response::Ok
        } else {
            Response::Error { message: "pausing is turned off by the policy".into() }
        }
    }

    /// Stop background work (download, watcher) and ask the platform loop to exit.
    pub fn shutdown(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        self.cancel_download();
        self.platform.stop();
    }
}

impl Handler for Service {
    fn handle(&self, req: Request) -> Response {
        match req {
            Request::Hello { .. } | Request::Subscribe => {
                Response::Error { message: "handled by the transport".into() }
            }
            Request::Status => Response::Status(self.status()),
            Request::GetPolicy => self.get_policy(),
            Request::SetPolicy { yaml } => self.set_policy(&yaml),
            Request::Pause { minutes } => self.pause(minutes),
            Request::Resume => {
                self.engine.resume();
                Response::Ok
            }
            Request::Activity { limit } => {
                Response::Activity { entries: self.engine.activity().recent(limit.min(MAX_ACTIVITY)) }
            }
            Request::ClearActivity => {
                self.engine.activity().clear();
                Response::Ok
            }
            Request::ModelStatus => Response::Model { status: self.engine.scanner().model_status() },
            Request::DownloadModel => self.start_download(),
            Request::CancelDownload => {
                self.cancel_download();
                Response::Ok
            }
            Request::RemoveModel => self.remove_model(),
            Request::Permissions => Response::Permissions { list: self.platform.permissions() },
            Request::RequestPermission { id } => {
                Response::PermissionResult { granted: self.platform.request_permission(&id) }
            }
            Request::Shutdown => {
                tracing::info!("shutdown requested over IPC");
                self.shutdown();
                Response::Ok
            }
        }
    }

    fn subscribe(&self) -> Receiver<EngineEvent> {
        self.engine.events.subscribe()
    }
}

fn stamp(path: &Path) -> Stamp {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

/// Write via a temporary file in the same directory, then rename over the target.
fn write_atomic(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("yaml.tmp");
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, path)
}
