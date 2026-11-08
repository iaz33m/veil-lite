//! Veil platform provider for Windows 10/11.
//!
//! * Input: `WH_KEYBOARD_LL` + `WH_MOUSE_LL` on the thread that calls [`veil_core::Platform::run`].
//! * Replays: `SendInput` tagged with [`inject::MAGIC`] in `dwExtraInfo`; tagged events skip the handler.
//! * Accessibility: UI Automation on two COM (MTA) worker threads — a *fast* lane used from the hook on
//!   mouse-down (short timeout, skipped when busy) and a *slow* lane for text, URLs, file dialogs and the
//!   Explorer selection.
//! * Drops: `SwallowAndCancel` — the button-up is swallowed and the OLE drag is cancelled with Escape.
//! * OCR: `Windows.Media.Ocr`. Prompts: `TaskDialogIndirect` (loaded dynamically; falls back to a
//!   relabelled `MessageBoxW` when comctl32 v6 is not activated).
//!
//! Limitations: windows of elevated (admin) processes are out of reach unless Veil runs elevated or as a
//! signed `uiAccess` binary (UIPI); Windows silently removes low-level hooks whose callback repeatedly
//! exceeds `LowLevelHooksTimeout`, so the hook path never blocks for long.

#![cfg(windows)]

mod apps;
mod clipboard;
mod com;
mod hooks;
mod inject;
mod keys;
mod ocr;
mod platform;
mod shell;
mod ui;
mod uia;
mod util;

pub use platform::WindowsPlatform;

use std::sync::Arc;

/// Create the Windows platform provider.
pub fn create() -> anyhow::Result<Arc<dyn veil_core::Platform>> {
    let platform: Arc<dyn veil_core::Platform> = WindowsPlatform::new()?;
    Ok(platform)
}
