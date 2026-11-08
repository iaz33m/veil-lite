/// Every command the web view may call. tauri-build generates `allow-<command>` permissions for them,
/// and capabilities/default.json grants exactly this set to the main window.
const COMMANDS: &[&str] = &[
    "status",
    "get_policy",
    "set_policy",
    "pause",
    "resume",
    "activity",
    "clear_activity",
    "model_status",
    "download_model",
    "cancel_download",
    "remove_model",
    "permissions",
    "request_permission",
    "engine_restart",
    "app_state",
    "complete_setup",
    "set_autostart",
    "open_link",
    "check_for_update",
];

fn main() {
    tauri_build::try_build(
        tauri_build::Attributes::new().app_manifest(tauri_build::AppManifest::new().commands(COMMANDS)),
    )
    .expect("failed to run tauri-build");
}
