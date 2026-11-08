//! The main window: shown on demand, hidden (not closed) so Veil keeps running in the tray.

use tauri::{AppHandle, Manager, Runtime, WebviewWindow};

pub const MAIN: &str = "main";

pub fn show_main<R: Runtime>(app: &AppHandle<R>) {
    #[cfg(target_os = "macos")]
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
    if let Some(w) = app.get_webview_window(MAIN) {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

pub fn hide<R: Runtime>(window: &WebviewWindow<R>) {
    let _ = window.hide();
    // No Dock icon while only the menu bar item is left.
    #[cfg(target_os = "macos")]
    let _ = window.app_handle().set_activation_policy(tauri::ActivationPolicy::Accessory);
}
