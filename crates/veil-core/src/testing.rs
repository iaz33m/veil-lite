//! Test doubles: a scriptable platform and a tiny scanner. Enabled for tests and with the `testing`
//! feature (used by the engine binary's `--simulate` mode and by other crates' tests).

use crate::findings::{CheckId, Finding};
use crate::input::Replay;
use crate::platform::{Choice, InputHandler, Platform, Prompt};
use crate::scanner::{Extracted, ModelStatus, ScanLayers, Scanner};
use crate::types::*;
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Default)]
pub struct MockPlatform {
    pub foreground: Mutex<Option<AppContext>>,
    /// Overrides `app_at` (else the foreground app).
    pub app_at: Mutex<Option<AppContext>>,
    pub element: Mutex<Option<UiElement>>,
    pub focused: Mutex<Option<FocusedText>>,
    pub clipboard: Mutex<ClipboardContent>,
    pub replays: Mutex<Vec<Replay>>,
    pub answers: Mutex<VecDeque<Choice>>,
    pub prompts: Mutex<Vec<Prompt>>,
    pub banners: Mutex<Vec<String>>,
    pub drag: Mutex<Option<DragSession>>,
    pub withheld: Mutex<Vec<bool>>,
    pub strategy: Mutex<Option<DropStrategy>>,
    pub cancelled_drops: Mutex<u32>,
    pub picker: Mutex<Vec<PathBuf>>,
    pub supports_replace: bool,
    pub replaced: Mutex<Option<String>>,
    pub ocr_text: Mutex<Option<String>>,
}

impl MockPlatform {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn with_replace() -> Arc<Self> {
        Arc::new(MockPlatform { supports_replace: true, ..Default::default() })
    }

    pub fn focus(&self, app: &AppContext) {
        *self.foreground.lock() = Some(app.clone());
    }

    pub fn answer(&self, choice: Choice) {
        self.answers.lock().push_back(choice);
    }

    pub fn set_text(&self, text: &str) {
        *self.focused.lock() = Some(FocusedText::Exposed { text: text.into(), role: "AXTextArea".into() });
    }

    pub fn take_replays(&self) -> Vec<Replay> {
        std::mem::take(&mut *self.replays.lock())
    }
}

impl Platform for MockPlatform {
    fn name(&self) -> &'static str {
        "mock"
    }

    fn run(self: Arc<Self>, _handler: Arc<dyn InputHandler>) -> anyhow::Result<()> {
        Ok(())
    }

    fn stop(&self) {}

    fn replay(&self, replay: &Replay) {
        self.replays.lock().push(replay.clone());
    }

    fn foreground(&self) -> Option<AppContext> {
        self.foreground.lock().clone()
    }

    fn app_at(&self, _at: Point) -> Option<AppContext> {
        self.app_at.lock().clone().or_else(|| self.foreground())
    }

    fn element_at(&self, _at: Point) -> Option<UiElement> {
        self.element.lock().clone()
    }

    fn focused_text(&self, _app: &AppContext) -> FocusedText {
        self.focused.lock().clone().unwrap_or(FocusedText::NotExposed { role: "unknown".into() })
    }

    fn replace_composer_text(&self, _app: &AppContext, text: &str) -> bool {
        if self.supports_replace {
            *self.replaced.lock() = Some(text.into());
        }
        self.supports_replace
    }

    fn clipboard(&self) -> ClipboardContent {
        self.clipboard.lock().clone()
    }

    fn set_clipboard(&self, content: &ClipboardContent) -> bool {
        *self.clipboard.lock() = content.clone();
        true
    }

    fn drag_session(&self) -> Option<DragSession> {
        self.drag.lock().clone()
    }

    fn set_drag_withheld(&self, withheld: bool) {
        self.withheld.lock().push(withheld);
    }

    fn drop_strategy(&self) -> DropStrategy {
        self.strategy.lock().unwrap_or(DropStrategy::Withhold)
    }

    fn cancel_drop(&self, _at: Point) {
        *self.cancelled_drops.lock() += 1;
    }

    fn picker_selection(&self, _app: &AppContext) -> Vec<PathBuf> {
        self.picker.lock().clone()
    }

    fn ocr(&self, _image: &[u8]) -> Option<String> {
        self.ocr_text.lock().clone()
    }

    fn ask(&self, prompt: &Prompt) -> Choice {
        self.prompts.lock().push(prompt.clone());
        self.answers.lock().pop_front().unwrap_or(prompt.cancel_choice())
    }

    fn banner(&self, text: &str) {
        self.banners.lock().push(text.into());
    }

    fn permissions(&self) -> Vec<Permission> {
        vec![Permission { id: "mock".into(), title: "Mock".into(), granted: true, help: String::new() }]
    }
}

/// Finds the keyword `confidential`, `SECRET-…` tokens (as `GENERIC_SECRET`), and — when the model is
/// "ready" — flags text containing `fn ` or `def ` as source code.
pub struct MockScanner {
    pub model: Mutex<ModelStatus>,
    pub model_delay: std::time::Duration,
    pub model_calls: Mutex<u32>,
}

impl Default for MockScanner {
    fn default() -> Self {
        MockScanner { model: Mutex::new(ModelStatus::Missing), model_delay: std::time::Duration::ZERO, model_calls: Mutex::new(0) }
    }
}

impl MockScanner {
    pub fn with_model() -> Self {
        MockScanner { model: Mutex::new(ModelStatus::Ready), ..Default::default() }
    }
}

impl Scanner for MockScanner {
    fn scan_text(&self, text: &str, layers: ScanLayers, source: &str) -> Vec<Finding> {
        let mut out = Vec::new();
        let lower = text.to_lowercase();
        if layers.keywords {
            if let Some(i) = lower.find("confidential") {
                out.push(Finding::keyword("confidential", i..i + 12, source));
            }
        }
        if layers.pii {
            if let Some(i) = text.find("SECRET-") {
                let end = text[i..].find(char::is_whitespace).map(|e| i + e).unwrap_or(text.len());
                out.push(Finding::pii("GENERIC_SECRET", i..end, 0.9, source));
            }
        }
        if layers.model && matches!(*self.model.lock(), ModelStatus::Ready | ModelStatus::Loaded) {
            *self.model_calls.lock() += 1;
            std::thread::sleep(self.model_delay);
            *self.model.lock() = ModelStatus::Loaded;
            if text.contains("fn ") || text.contains("def ") {
                out.push(Finding::check(CheckId::SourceCode, 0.92, source));
            }
        }
        out
    }

    fn extract(&self, path: &Path) -> Extracted {
        match std::fs::read(path) {
            Ok(bytes) if bytes.len() > 1024 * 1024 => Extracted::TooLarge,
            Ok(bytes) if path.extension().is_some_and(|e| e == "png") => Extracted::Image(bytes),
            Ok(bytes) => Extracted::Text(String::from_utf8_lossy(&bytes).into_owned()),
            Err(e) => Extracted::Unreadable(e.to_string()),
        }
    }

    fn model_status(&self) -> ModelStatus {
        self.model.lock().clone()
    }

    fn unload_model(&self) {
        let mut m = self.model.lock();
        if *m == ModelStatus::Loaded {
            *m = ModelStatus::Ready;
        }
    }
}

/// A chat app to point tests at.
pub fn chat_app() -> AppContext {
    AppContext {
        app_id: "com.example.chat".into(),
        name: "Chat".into(),
        pid: 42,
        window_title: Some("Chat".into()),
        url: None,
    }
}

pub fn browser_app(url: &str) -> AppContext {
    AppContext { app_id: "com.google.Chrome".into(), name: "Google Chrome".into(), pid: 77, window_title: None, url: Some(url.into()) }
}
