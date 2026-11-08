//! Veil platform provider for macOS 12+.
//!
//! A port of the proof-of-concept agent (`dlp-agent.swift` v0.3.0):
//! * a session [CGEventTap](tap) holds Enter / Send clicks / Cmd+V / file-picker "Open" / drops,
//! * the Accessibility API ([`ax`]) reads the composer text and the element under the pointer
//!   (Chromium and Electron are switched to full accessibility with `AXManualAccessibility`),
//! * held input is re-injected with a tagged event source so the tap lets it through,
//! * drops are stopped by withholding the drag pasteboard while it hovers over a protected app,
//! * prompts are system alerts (`CFUserNotificationDisplayAlert`), OCR is Vision.
//!
//! The only permission needed is Accessibility.
//!
//! On every other OS this crate compiles to an empty library.

#![cfg(target_os = "macos")]
#![allow(clippy::missing_safety_doc)]

mod apps;
mod ax;
mod ffi;
mod ocr;
mod pasteboard;
mod picker;
mod platform;
mod tap;
mod ui;

use std::sync::Arc;

pub use platform::MacPlatform;

/// Create the macOS provider.
pub fn create() -> anyhow::Result<Arc<dyn veil_core::Platform>> {
    Ok(Arc::new(MacPlatform::new()?))
}
