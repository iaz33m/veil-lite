//! Platform used when no OS provider is available (unsupported OS, or the provider failed to start).
//! It installs no hooks; the engine still serves IPC so the desktop app can explain what is wrong.

use parking_lot::{Condvar, Mutex};
use std::path::PathBuf;
use std::sync::Arc;
use veil_core::input::Replay;
use veil_core::platform::{Choice, InputHandler, Platform, Prompt};
use veil_core::types::*;

pub struct UnsupportedPlatform {
    reason: String,
    stopped: Mutex<bool>,
    wake: Condvar,
}

impl UnsupportedPlatform {
    pub fn new(reason: impl Into<String>) -> Self {
        UnsupportedPlatform { reason: reason.into(), stopped: Mutex::new(false), wake: Condvar::new() }
    }
}

impl Platform for UnsupportedPlatform {
    fn name(&self) -> &'static str {
        "unsupported"
    }

    fn run(self: Arc<Self>, _handler: Arc<dyn InputHandler>) -> anyhow::Result<()> {
        let mut stopped = self.stopped.lock();
        while !*stopped {
            self.wake.wait(&mut stopped);
        }
        Ok(())
    }

    fn stop(&self) {
        *self.stopped.lock() = true;
        self.wake.notify_all();
    }

    fn replay(&self, _replay: &Replay) {}

    fn foreground(&self) -> Option<AppContext> {
        None
    }

    fn app_at(&self, _at: Point) -> Option<AppContext> {
        None
    }

    fn element_at(&self, _at: Point) -> Option<UiElement> {
        None
    }

    fn focused_text(&self, _app: &AppContext) -> FocusedText {
        FocusedText::NotExposed { role: String::new() }
    }

    fn clipboard(&self) -> ClipboardContent {
        ClipboardContent::default()
    }

    fn set_clipboard(&self, _content: &ClipboardContent) -> bool {
        false
    }

    fn drag_session(&self) -> Option<DragSession> {
        None
    }

    fn drop_strategy(&self) -> DropStrategy {
        DropStrategy::Unsupported
    }

    fn picker_selection(&self, _app: &AppContext) -> Vec<PathBuf> {
        Vec::new()
    }

    fn ask(&self, prompt: &Prompt) -> Choice {
        prompt.cancel_choice()
    }

    fn permissions(&self) -> Vec<Permission> {
        vec![Permission {
            id: "platform".into(),
            title: "Protection on this computer".into(),
            granted: false,
            help: self.reason.clone(),
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_returns_after_stop() {
        struct Nop;
        impl InputHandler for Nop {
            fn on_input(&self, _ev: &veil_core::InputEvent) -> veil_core::HookVerdict {
                veil_core::HookVerdict::Pass
            }
        }
        let p = Arc::new(UnsupportedPlatform::new("test"));
        let runner = {
            let p = Arc::clone(&p);
            std::thread::spawn(move || p.run(Arc::new(Nop)))
        };
        std::thread::sleep(std::time::Duration::from_millis(50));
        p.stop();
        runner.join().unwrap().unwrap();
        assert!(!p.permissions()[0].granted);
    }
}
