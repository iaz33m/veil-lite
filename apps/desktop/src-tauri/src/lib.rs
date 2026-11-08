//! Veil desktop app: tray icon, setup wizard, settings and activity on top of the `veil-engine`
//! sidecar. The app holds no detection logic; everything goes through the engine's local IPC.

mod commands;
mod events;
mod link;
mod prefs;
mod supervisor;
mod tray;
mod window;

use link::EngineLink;
use prefs::Prefs;
use std::sync::Arc;
use supervisor::Supervisor;
use tauri::{AppHandle, Manager, RunEvent, WindowEvent};
use tauri_plugin_autostart::MacosLauncher;
use veil_core::paths::Paths;

/// Argument the login item passes, so Veil starts in the tray without opening its window.
const MINIMIZED_ARG: &str = "--minimized";

pub fn run() {
    let paths = match Paths::detect() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("veil: can't find a home directory: {e}");
            std::process::exit(1);
        }
    };
    let link = Arc::new(EngineLink::new(paths.clone()));
    let supervisor = Supervisor::new();

    let mut builder = tauri::Builder::default()
        // Must be first: a second launch focuses the running app instead.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| window::show_main(app)))
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, Some(vec![MINIMIZED_ARG])));
    #[cfg(feature = "updater")]
    {
        builder = builder.plugin(tauri_plugin_updater::Builder::new().build());
    }
    builder = builder
        .manage(Arc::clone(&link))
        .manage(Arc::clone(&supervisor))
        .manage(tray::Tray::default())
        .invoke_handler(tauri::generate_handler![
            commands::status,
            commands::get_policy,
            commands::set_policy,
            commands::pause,
            commands::resume,
            commands::activity,
            commands::clear_activity,
            commands::model_status,
            commands::download_model,
            commands::cancel_download,
            commands::remove_model,
            commands::permissions,
            commands::request_permission,
            commands::engine_restart,
            commands::app_state,
            commands::complete_setup,
            commands::set_autostart,
            commands::open_link,
            commands::check_for_update,
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            let prefs = Prefs::load(app.path().app_config_dir().ok());
            let setup_complete = prefs.get().setup_complete;
            app.manage(prefs);

            tray::build(&handle)?;
            supervisor.start(handle.clone(), Arc::clone(&link));
            events::start(handle.clone(), paths.clone());
            tray::start_refresher(handle.clone());

            let minimized = std::env::args().any(|a| a == MINIMIZED_ARG);
            if setup_complete && minimized {
                #[cfg(target_os = "macos")]
                app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            } else {
                window::show_main(&handle);
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the window keeps Veil running in the tray.
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == window::MAIN {
                    api.prevent_close();
                    if let Some(w) = window.app_handle().get_webview_window(window::MAIN) {
                        window::hide(&w);
                    }
                }
            }
        });

    let app = builder.build(tauri::generate_context!()).expect("error while building the Veil app");
    app.run(|_app, event| {
        // Only an explicit quit (exit code set) ends the app; losing the last window doesn't.
        if let RunEvent::ExitRequested { api, code: None, .. } = event {
            api.prevent_exit();
        }
    });
}

/// "Quit Veil": stop the engine (protection ends) and exit.
pub fn quit(app: &AppHandle) {
    let link = app.state::<Arc<EngineLink>>().inner().clone();
    let supervisor = app.state::<Arc<Supervisor>>().inner().clone();
    let app = app.clone();
    std::thread::spawn(move || {
        supervisor.shutdown(&link);
        app.exit(0);
    });
}
