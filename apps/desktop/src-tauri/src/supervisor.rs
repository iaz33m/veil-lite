//! Keeps `veil-engine` running as a sidecar: starts it when no engine answers, restarts it with
//! backoff when it exits, and stops it when Veil quits. An engine that is already running (for example
//! one started by hand during development) is used as it is.

use crate::link::EngineLink;
use parking_lot::{Condvar, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Runtime};
use tauri_plugin_shell::process::{CommandChild, CommandEvent};
use tauri_plugin_shell::ShellExt;
use veil_ipc::Request;

const FIRST_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(30);
/// An engine that ran this long resets the backoff.
const HEALTHY_RUN: Duration = Duration::from_secs(60);
/// How often an engine we don't own is checked.
const WATCH_INTERVAL: Duration = Duration::from_secs(3);

#[derive(Default)]
pub struct Supervisor {
    child: Mutex<Option<CommandChild>>,
    quitting: AtomicBool,
    wake: Mutex<bool>,
    wake_cv: Condvar,
}

impl Supervisor {
    pub fn new() -> Arc<Self> {
        Arc::new(Supervisor::default())
    }

    /// Start the supervision thread.
    pub fn start<R: Runtime>(self: &Arc<Self>, app: AppHandle<R>, link: Arc<EngineLink>) {
        let this = Arc::clone(self);
        std::thread::Builder::new()
            .name("veil-engine-supervisor".into())
            .spawn(move || this.supervise(app, link))
            .expect("spawn supervisor thread");
    }

    fn supervise<R: Runtime>(&self, app: AppHandle<R>, link: Arc<EngineLink>) {
        let mut backoff = FIRST_BACKOFF;
        while !self.quitting.load(Ordering::SeqCst) {
            if link.is_up() {
                self.sleep(WATCH_INTERVAL);
                continue;
            }
            let started = Instant::now();
            match spawn_engine(&app) {
                Ok((mut rx, child)) => {
                    eprintln!("veil: started the engine (pid {})", child.pid());
                    *self.child.lock() = Some(child);
                    link.reset();
                    tauri::async_runtime::block_on(async {
                        while let Some(event) = rx.recv().await {
                            match event {
                                CommandEvent::Terminated(p) => {
                                    eprintln!("veil: the engine exited (code {:?}, signal {:?})", p.code, p.signal);
                                    break;
                                }
                                CommandEvent::Error(e) => eprintln!("veil: engine error: {e}"),
                                CommandEvent::Stderr(line) if cfg!(debug_assertions) => {
                                    eprintln!("engine | {}", String::from_utf8_lossy(&line).trim_end());
                                }
                                _ => {}
                            }
                        }
                    });
                    self.child.lock().take();
                    link.reset();
                }
                Err(e) => eprintln!("veil: couldn't start the engine: {e}"),
            }
            if self.quitting.load(Ordering::SeqCst) {
                break;
            }
            if started.elapsed() >= HEALTHY_RUN {
                backoff = FIRST_BACKOFF;
            }
            if self.sleep(backoff) {
                backoff = FIRST_BACKOFF;
            } else {
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
        }
    }

    /// Sleep until the timeout or a wake-up. Returns true when woken early.
    fn sleep(&self, d: Duration) -> bool {
        let mut woken = self.wake.lock();
        if !*woken {
            self.wake_cv.wait_for(&mut woken, d);
        }
        std::mem::take(&mut *woken)
    }

    fn kick(&self) {
        *self.wake.lock() = true;
        self.wake_cv.notify_all();
    }

    /// Stop the running engine (ours or not) and start a fresh one straight away.
    pub fn restart(&self, link: &EngineLink) {
        let _ = link.call(Request::Shutdown);
        link.reset();
        if let Some(child) = self.child.lock().take() {
            let _ = child.kill();
        }
        self.kick();
    }

    /// Stop supervising and stop the engine (when Veil quits).
    pub fn shutdown(&self, link: &EngineLink) {
        self.quitting.store(true, Ordering::SeqCst);
        let _ = link.call(Request::Shutdown);
        if let Some(child) = self.child.lock().take() {
            let _ = child.kill();
        }
        self.kick();
    }
}

fn spawn_engine<R: Runtime>(
    app: &AppHandle<R>,
) -> Result<(tauri::async_runtime::Receiver<CommandEvent>, CommandChild), String> {
    app.shell()
        .sidecar("veil-engine")
        .map_err(|e| e.to_string())?
        .args(["run"])
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| e.to_string())
}
