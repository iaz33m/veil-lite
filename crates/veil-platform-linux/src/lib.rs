//! Veil platform provider for Linux (X11 and Wayland).
//!
//! | Need | X11 | Wayland |
//! |---|---|---|
//! | Hold input | evdev `EVIOCGRAB` + uinput mirror | same |
//! | Replay | uinput "Veil replay" device | same |
//! | Foreground app | `_NET_ACTIVE_WINDOW` (watcher thread) | AT-SPI focus events |
//! | App / element under pointer | X11 window tree + AT-SPI `GetAccessibleAtPoint` | focused app; element lookup approximate |
//! | Composer text | AT-SPI Text / EditableText | same |
//! | Clipboard | arboard | arboard (data-control) |
//! | Drag data / drop blocking | XDND selection, swallow + Escape | not possible (`Unsupported`) |
//! | OCR | none (by design, no fallback) | none |
//! | Prompts | zenity / kdialog | same |
//!
//! Setup: input access needs the `veil` group and `packaging/linux/99-veil.rules`.

#[cfg(target_os = "linux")]
mod atspi;
#[cfg(target_os = "linux")]
mod clipboard;
#[cfg(target_os = "linux")]
mod dialog;
#[cfg(target_os = "linux")]
mod input;
#[cfg(target_os = "linux")]
#[allow(dead_code)] // full code table kept for reference and future xkb keymap
mod keymap;
#[cfg(target_os = "linux")]
mod platform;
#[cfg(target_os = "linux")]
mod proc;
#[cfg(target_os = "linux")]
mod translate;
#[cfg(target_os = "linux")]
mod x11;

#[cfg(target_os = "linux")]
pub use platform::{LinuxPlatform, Session};

/// Create the Linux platform provider (X11 or Wayland, from the session environment).
#[cfg(target_os = "linux")]
pub fn create() -> anyhow::Result<std::sync::Arc<dyn veil_core::Platform>> {
    Ok(std::sync::Arc::new(platform::LinuxPlatform::new()?))
}
