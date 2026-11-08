//! Commands the web view calls (see src/lib/api.ts). Engine calls run on the blocking pool.

use crate::link::{expect_ok, unexpected, EngineLink};
use crate::prefs::Prefs;
use crate::supervisor::Supervisor;
use serde::Serialize;
use std::sync::Arc;
use tauri::{AppHandle, State};
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_opener::OpenerExt as _;
use veil_core::activity::ActivityEntry;
use veil_core::scanner::ModelStatus;
use veil_core::types::Permission;
use veil_ipc::{Request, Response, StatusInfo};

type Link<'a> = State<'a, Arc<EngineLink>>;
type CmdResult<T> = Result<T, String>;

const REPOSITORY: &str = "https://github.com/veil-dlp/veil";

/// Run a blocking engine call off the async runtime.
async fn engine<T, F>(link: &Link<'_>, f: F) -> CmdResult<T>
where
    T: Send + 'static,
    F: FnOnce(&EngineLink) -> CmdResult<T> + Send + 'static,
{
    let link = link.inner().clone();
    tauri::async_runtime::spawn_blocking(move || f(&link)).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn status(link: Link<'_>) -> CmdResult<StatusInfo> {
    engine(&link, |l| match l.call(Request::Status)? {
        Response::Status(s) => Ok(s),
        other => Err(unexpected(&other)),
    })
    .await
}

#[tauri::command]
pub async fn get_policy(link: Link<'_>) -> CmdResult<String> {
    engine(&link, |l| match l.call(Request::GetPolicy)? {
        Response::Policy { yaml } => Ok(yaml),
        other => Err(unexpected(&other)),
    })
    .await
}

#[tauri::command]
pub async fn set_policy(link: Link<'_>, yaml: String) -> CmdResult<()> {
    engine(&link, move |l| expect_ok(l.call(Request::SetPolicy { yaml })?)).await
}

#[tauri::command]
pub async fn pause(link: Link<'_>, minutes: u32) -> CmdResult<()> {
    engine(&link, move |l| expect_ok(l.call(Request::Pause { minutes })?)).await
}

#[tauri::command]
pub async fn resume(link: Link<'_>) -> CmdResult<()> {
    engine(&link, |l| expect_ok(l.call(Request::Resume)?)).await
}

#[tauri::command]
pub async fn activity(link: Link<'_>, limit: usize) -> CmdResult<Vec<ActivityEntry>> {
    let limit = limit.clamp(1, 5000);
    engine(&link, move |l| match l.call(Request::Activity { limit })? {
        Response::Activity { entries } => Ok(entries),
        other => Err(unexpected(&other)),
    })
    .await
}

#[tauri::command]
pub async fn clear_activity(link: Link<'_>) -> CmdResult<()> {
    engine(&link, |l| expect_ok(l.call(Request::ClearActivity)?)).await
}

#[tauri::command]
pub async fn model_status(link: Link<'_>) -> CmdResult<ModelStatus> {
    engine(&link, |l| match l.call(Request::ModelStatus)? {
        Response::Model { status } => Ok(status),
        other => Err(unexpected(&other)),
    })
    .await
}

#[tauri::command]
pub async fn download_model(link: Link<'_>) -> CmdResult<()> {
    engine(&link, |l| expect_ok(l.call(Request::DownloadModel)?)).await
}

#[tauri::command]
pub async fn cancel_download(link: Link<'_>) -> CmdResult<()> {
    engine(&link, |l| expect_ok(l.call(Request::CancelDownload)?)).await
}

#[tauri::command]
pub async fn remove_model(link: Link<'_>) -> CmdResult<()> {
    engine(&link, |l| expect_ok(l.call(Request::RemoveModel)?)).await
}

#[tauri::command]
pub async fn permissions(link: Link<'_>) -> CmdResult<Vec<Permission>> {
    engine(&link, |l| match l.call(Request::Permissions)? {
        Response::Permissions { list } => Ok(list),
        other => Err(unexpected(&other)),
    })
    .await
}

#[tauri::command]
pub async fn request_permission(link: Link<'_>, id: String) -> CmdResult<bool> {
    // The OS prompt can stay open for a while: use a connection of its own.
    engine(&link, move |l| match l.call_alone(Request::RequestPermission { id })? {
        Response::PermissionResult { granted } => Ok(granted),
        Response::Ok => Ok(false),
        other => Err(unexpected(&other)),
    })
    .await
}

#[tauri::command]
pub async fn engine_restart(link: Link<'_>, supervisor: State<'_, Arc<Supervisor>>) -> CmdResult<()> {
    let supervisor = supervisor.inner().clone();
    engine(&link, move |l| {
        supervisor.restart(l);
        Ok(())
    })
    .await
}

#[derive(Serialize)]
pub struct AppState {
    setup_complete: bool,
    autostart: bool,
    version: String,
}

#[tauri::command]
pub fn app_state(app: AppHandle, prefs: State<'_, Prefs>) -> AppState {
    AppState {
        setup_complete: prefs.get().setup_complete,
        autostart: app.autolaunch().is_enabled().unwrap_or(false),
        version: app.package_info().version.to_string(),
    }
}

#[tauri::command]
pub fn complete_setup(app: AppHandle, prefs: State<'_, Prefs>, autostart: bool) -> CmdResult<()> {
    apply_autostart(&app, autostart)?;
    prefs.update(|p| p.setup_complete = true)
}

#[tauri::command]
pub fn set_autostart(app: AppHandle, enabled: bool) -> CmdResult<()> {
    apply_autostart(&app, enabled)
}

fn apply_autostart(app: &AppHandle, enabled: bool) -> CmdResult<()> {
    let manager = app.autolaunch();
    let result = if enabled { manager.enable() } else { manager.disable() };
    result.map_err(|e| format!("Couldn’t change the start-at-login setting: {e}"))
}

/// Opens one of a fixed set of pages; the web view can't pass an arbitrary URL.
#[tauri::command]
pub fn open_link(app: AppHandle, which: String) -> CmdResult<()> {
    let url = match which.as_str() {
        "repository" => REPOSITORY.to_string(),
        "privacy" => format!("{REPOSITORY}/blob/main/docs/PRIVACY.md"),
        "security" => format!("{REPOSITORY}/blob/main/SECURITY.md"),
        "license" => format!("{REPOSITORY}/blob/main/LICENSE"),
        _ => return Err("Unknown link.".into()),
    };
    app.opener().open_url(url, None::<&str>).map_err(|e| e.to_string())
}

/// The newer version available, if any.
#[tauri::command]
pub async fn check_for_update(app: AppHandle) -> CmdResult<Option<String>> {
    #[cfg(feature = "updater")]
    {
        use tauri_plugin_updater::UpdaterExt as _;
        let update = app.updater().map_err(|e| e.to_string())?.check().await.map_err(|e| e.to_string())?;
        Ok(update.map(|u| u.version))
    }
    #[cfg(not(feature = "updater"))]
    {
        let _ = app;
        Err("Automatic updates aren’t set up in this build yet. New versions are published on GitHub.".into())
    }
}
