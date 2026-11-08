//! The contract every OS provider implements. One implementation is chosen at runtime
//! (`Box<dyn Platform>` / `Arc<dyn Platform>`); the core never calls OS APIs directly.
//!
//! Threading rules:
//! * [`Platform::run`] blocks the calling (main) thread, installs the input hooks and delivers every user
//!   input event to the [`InputHandler`] *before* any application sees it.
//! * Events produced by [`Platform::replay`] are tagged as Veil's own and are **never** delivered to the
//!   handler.
//! * Everything the hook calls (`foreground`, `app_at`, `element_at`) must be fast (well under a
//!   millisecond typical) — cache where needed. Everything else is called from the worker thread.

use crate::input::{HookVerdict, InputEvent, Replay};
use crate::types::*;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

/// Receives raw input from the platform hook.
pub trait InputHandler: Send + Sync {
    fn on_input(&self, ev: &InputEvent) -> HookVerdict;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptKind {
    /// A message is about to be sent.
    Send,
    Paste,
    /// Files chosen in a file picker.
    Upload,
    /// A drag-and-drop was stopped.
    Drop,
    /// A risky drop may already have landed; asked on the next send.
    Backstop,
    /// The policy blocks this with no override.
    BlockNotice,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Choice {
    SendAnyway,
    DontSend,
    Redact,
    /// Let drops into this app through for 30 seconds (the user then drops again).
    AllowFor30s,
    Ok,
}

impl Choice {
    pub fn key(&self) -> &'static str {
        match self {
            Choice::SendAnyway => "send_anyway",
            Choice::DontSend => "dont_send",
            Choice::Redact => "redact",
            Choice::AllowFor30s => "allow_30s",
            Choice::Ok => "ok",
        }
    }
}

/// A question for the user. Providers render it natively (dialog, task dialog, GTK/zenity).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Prompt {
    pub title: String,
    pub message: String,
    pub app: AppContext,
    pub kind: PromptKind,
    /// Buttons in order; the first is the default (safe) button.
    pub options: Vec<Choice>,
}

impl Prompt {
    /// Button label for a choice, worded for this prompt.
    pub fn label(&self, choice: Choice) -> &'static str {
        match (self.kind, choice) {
            (_, Choice::Ok) => "OK",
            (_, Choice::AllowFor30s) => "Allow drops for 30 s",
            (PromptKind::Paste, Choice::SendAnyway) => "Paste anyway",
            (PromptKind::Paste, Choice::DontSend) => "Don't paste",
            (PromptKind::Paste, Choice::Redact) => "Paste redacted",
            (PromptKind::Upload, Choice::SendAnyway) => "Upload anyway",
            (PromptKind::Upload, Choice::DontSend) => "Don't upload",
            (_, Choice::SendAnyway) => "Send anyway",
            (_, Choice::DontSend) => "Don't send",
            (_, Choice::Redact) => "Redact & send",
        }
    }

    /// The choice used when the dialog is dismissed (Esc / closed / failed to show).
    pub fn cancel_choice(&self) -> Choice {
        if self.options.contains(&Choice::DontSend) {
            Choice::DontSend
        } else {
            Choice::Ok
        }
    }
}

/// Everything Veil needs from an operating system.
pub trait Platform: Send + Sync + 'static {
    /// Short name, e.g. `macos`, `windows`, `linux-x11`, `linux-wayland`.
    fn name(&self) -> &'static str;

    /// Install hooks and run the platform event loop on the current thread until [`Platform::stop`].
    fn run(self: Arc<Self>, handler: Arc<dyn InputHandler>) -> anyhow::Result<()>;

    /// Ask the event loop to exit.
    fn stop(&self);

    /// Re-inject input, tagged so the hook lets it through untouched.
    fn replay(&self, replay: &Replay);

    /// Application that has keyboard focus. Called from the hook: must be fast.
    fn foreground(&self) -> Option<AppContext>;

    /// Application owning the top-most window under a screen point. Called from the hook: must be fast.
    fn app_at(&self, at: Point) -> Option<AppContext>;

    /// Accessibility element under a screen point. Called from the hook on mouse-down: must be fast.
    fn element_at(&self, at: Point) -> Option<UiElement>;

    /// Text of the focused composer in `app` (worker thread).
    fn focused_text(&self, app: &AppContext) -> FocusedText;

    /// Ask the app (Chromium/Electron) to build its accessibility tree.
    fn enable_accessibility(&self, _app: &AppContext) {}

    /// Replace the focused composer's text directly. Returns false if unsupported; the core then uses
    /// select-all + paste through the clipboard.
    fn replace_composer_text(&self, _app: &AppContext, _text: &str) -> bool {
        false
    }

    fn clipboard(&self) -> ClipboardContent;

    /// Put content on the clipboard (used for redaction, then to restore the user's clipboard).
    fn set_clipboard(&self, content: &ClipboardContent) -> bool;

    /// The drag-and-drop session in progress, if its data can be read.
    fn drag_session(&self) -> Option<DragSession>;

    /// Withhold (true) or restore (false) the drag data while it is over a protected target.
    fn set_drag_withheld(&self, _withheld: bool) {}

    fn drop_strategy(&self) -> DropStrategy;

    /// Abort the drag in progress after its button-up was swallowed (`SwallowAndCancel` platforms).
    fn cancel_drop(&self, _at: Point) {}

    /// Files currently selected in the file-picker dialog shown by `app`.
    fn picker_selection(&self, app: &AppContext) -> Vec<PathBuf>;

    /// OS-native OCR. `None` when unavailable (no fallback by design).
    fn ocr(&self, _image: &[u8]) -> Option<String> {
        None
    }

    /// Show a prompt and wait for the answer (worker thread).
    fn ask(&self, prompt: &Prompt) -> Choice;

    /// Small non-activating notice near the pointer or screen corner (typing warnings).
    fn banner(&self, _text: &str) {}

    /// Give focus back to the app after a prompt.
    fn refocus(&self, _app: &AppContext) {}

    /// Permissions for the setup wizard.
    fn permissions(&self) -> Vec<Permission>;

    /// Trigger the OS request / open the right settings page. Returns whether it is granted now.
    fn request_permission(&self, _id: &str) -> bool {
        false
    }
}
