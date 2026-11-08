//! COM apartments and the UI Automation worker threads.
//!
//! UIA objects are not `Send`, so each worker thread owns its own `IUIAutomation` and runs closures sent
//! to it. Callers wait with a timeout; a stuck provider (hung app) can never block the hook or the engine
//! for longer than the timeout.

use crossbeam_channel::{bounded, unbounded, Sender};
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
use windows::Win32::UI::Accessibility::{CUIAutomation, IUIAutomation, IUIAutomationTreeWalker};

thread_local! {
    static COM_READY: Cell<bool> = const { Cell::new(false) };
}

/// Join the multithreaded apartment on the current thread (once). Needed for WinRT (OCR) and Shell COM.
pub fn ensure_mta() {
    COM_READY.with(|ready| {
        if !ready.get() {
            let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
            // S_OK, S_FALSE (already initialised) and RPC_E_CHANGED_MODE (STA thread) are all usable.
            if hr.is_err() && hr.0 != 0x8001_0106_u32 as i32 {
                tracing::warn!("CoInitializeEx failed: {hr:?}");
            }
            ready.set(true);
        }
    });
}

/// What a worker closure gets.
pub struct UiaCtx {
    pub uia: IUIAutomation,
    pub walker: Option<IUIAutomationTreeWalker>,
}

type Job = Box<dyn FnOnce(&UiaCtx) + Send>;

pub struct ComWorker {
    name: &'static str,
    tx: Sender<Job>,
    busy: Arc<AtomicBool>,
    queued: Arc<std::sync::atomic::AtomicUsize>,
}

impl ComWorker {
    pub fn spawn(name: &'static str) -> anyhow::Result<Arc<ComWorker>> {
        let (tx, rx) = unbounded::<Job>();
        let (ready_tx, ready_rx) = bounded::<Result<(), String>>(1);
        let busy = Arc::new(AtomicBool::new(false));
        let queued = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (busy2, queued2) = (busy.clone(), queued.clone());
        std::thread::Builder::new().name(format!("veil-uia-{name}")).spawn(move || {
            ensure_mta();
            let uia: IUIAutomation = match unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) } {
                Ok(u) => u,
                Err(e) => {
                    let _ = ready_tx.send(Err(format!("UI Automation unavailable: {e}")));
                    return;
                }
            };
            let walker = unsafe { uia.ControlViewWalker() }.ok();
            let ctx = UiaCtx { uia, walker };
            let _ = ready_tx.send(Ok(()));
            for job in rx {
                queued2.fetch_sub(1, Ordering::Relaxed);
                busy2.store(true, Ordering::Release);
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(&ctx)));
                busy2.store(false, Ordering::Release);
            }
        })?;
        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => Ok(Arc::new(ComWorker { name, tx, busy, queued })),
            Ok(Err(e)) => anyhow::bail!(e),
            Err(_) => anyhow::bail!("UI Automation worker `{name}` did not start"),
        }
    }

    /// Idle and nothing queued.
    pub fn is_idle(&self) -> bool {
        !self.busy.load(Ordering::Acquire) && self.queued.load(Ordering::Relaxed) == 0
    }

    /// Run `f` on the worker and wait up to `timeout` for its result.
    pub fn call<R, F>(&self, timeout: Duration, f: F) -> Option<R>
    where
        R: Send + 'static,
        F: FnOnce(&UiaCtx) -> Option<R> + Send + 'static,
    {
        let (rtx, rrx) = bounded(1);
        self.queued.fetch_add(1, Ordering::Relaxed);
        let job: Job = Box::new(move |ctx| {
            let _ = rtx.send(f(ctx));
        });
        if self.tx.send(job).is_err() {
            self.queued.fetch_sub(1, Ordering::Relaxed);
            return None;
        }
        match rrx.recv_timeout(timeout) {
            Ok(r) => r,
            Err(_) => {
                tracing::debug!("UIA worker `{}` timed out after {timeout:?}", self.name);
                None
            }
        }
    }

    /// Like [`ComWorker::call`] but gives up immediately when the worker is busy (hook path).
    pub fn call_if_idle<R, F>(&self, timeout: Duration, f: F) -> Option<R>
    where
        R: Send + 'static,
        F: FnOnce(&UiaCtx) -> Option<R> + Send + 'static,
    {
        if !self.is_idle() {
            return None;
        }
        self.call(timeout, f)
    }

    /// Fire and forget.
    pub fn spawn_job<F>(&self, f: F)
    where
        F: FnOnce(&UiaCtx) + Send + 'static,
    {
        self.queued.fetch_add(1, Ordering::Relaxed);
        if self.tx.send(Box::new(f)).is_err() {
            self.queued.fetch_sub(1, Ordering::Relaxed);
        }
    }
}
