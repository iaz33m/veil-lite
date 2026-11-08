//! Tray (menu bar) icon: status line, pause/resume, open, quit.

use crate::link::EngineLink;
use crate::window;
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::Duration;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};
use veil_ipc::{Request, Response};

const TRAY_ID: &str = "veil";

pub struct TrayItems {
    status: MenuItem<Wry>,
    pause15: MenuItem<Wry>,
    pause60: MenuItem<Wry>,
    resume: MenuItem<Wry>,
}

/// Managed state: the menu items whose text and state change.
#[derive(Default)]
pub struct Tray(Mutex<Option<TrayItems>>);

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let status = MenuItem::with_id(app, "status", "Veil: starting…", false, None::<&str>)?;
    let pause15 = MenuItem::with_id(app, "pause15", "Pause for 15 minutes", false, None::<&str>)?;
    let pause60 = MenuItem::with_id(app, "pause60", "Pause for 1 hour", false, None::<&str>)?;
    let resume = MenuItem::with_id(app, "resume", "Resume protection", false, None::<&str>)?;
    let open = MenuItem::with_id(app, "open", "Open Veil…", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Veil", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &status,
            &PredefinedMenuItem::separator(app)?,
            &pause15,
            &pause60,
            &resume,
            &PredefinedMenuItem::separator(app)?,
            &open,
            &quit,
        ],
    )?;

    // macOS shows a monochrome template image in the menu bar; elsewhere the coloured mark reads better.
    #[cfg(target_os = "macos")]
    let icon = tauri::include_image!("icons/tray@2x.png");
    #[cfg(not(target_os = "macos"))]
    let icon = tauri::include_image!("icons/32x32.png");

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .icon_as_template(cfg!(target_os = "macos"))
        .tooltip("Veil")
        .menu(&menu)
        .show_menu_on_left_click(cfg!(target_os = "macos"))
        .on_menu_event(|app, event| on_menu(app, event.id().as_ref()))
        .on_tray_icon_event(|tray, event| {
            // Windows and Linux: a left click opens the window; the menu is on right click.
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                if !cfg!(target_os = "macos") {
                    window::show_main(tray.app_handle());
                }
            }
        })
        .build(app)?;

    *app.state::<Tray>().0.lock() = Some(TrayItems { status, pause15, pause60, resume });
    Ok(())
}

fn on_menu(app: &AppHandle, id: &str) {
    match id {
        "open" => window::show_main(app),
        "quit" => crate::quit(app),
        "pause15" | "pause60" | "resume" => {
            let req = match id {
                "pause15" => Request::Pause { minutes: 15 },
                "pause60" => Request::Pause { minutes: 60 },
                _ => Request::Resume,
            };
            let app = app.clone();
            std::thread::spawn(move || {
                let link = app.state::<Arc<EngineLink>>().inner().clone();
                if let Err(e) = link.call(req) {
                    eprintln!("veil: tray action failed: {e}");
                }
                refresh(&app);
            });
        }
        _ => {}
    }
}

/// Re-read the engine status and update the menu (blocking; call off the main thread).
pub fn refresh(app: &AppHandle) {
    let link = app.state::<Arc<EngineLink>>().inner().clone();
    let status = match link.call(Request::Status) {
        Ok(Response::Status(s)) => Some(s),
        _ => None,
    };
    let (text, can_pause, can_resume) = match &status {
        None => ("Veil: engine not running".to_string(), false, false),
        Some(s) if !s.active => ("Veil: not protecting".to_string(), false, false),
        Some(s) => match s.paused_remaining_secs {
            Some(secs) => (format!("Veil: paused, {} left", minutes_left(secs)), false, true),
            None => ("Veil: on guard".to_string(), true, false),
        },
    };
    if let Some(items) = app.state::<Tray>().0.lock().as_ref() {
        let _ = items.status.set_text(&text);
        let _ = items.pause15.set_enabled(can_pause);
        let _ = items.pause60.set_enabled(can_pause);
        let _ = items.resume.set_enabled(can_resume);
    }
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_tooltip(Some(text.as_str()));
    }
}

fn minutes_left(secs: u64) -> String {
    let m = secs.div_ceil(60);
    if m <= 1 {
        "1 minute".into()
    } else if m < 60 {
        format!("{m} minutes")
    } else {
        let h = m / 60;
        let r = m % 60;
        if r == 0 {
            format!("{h} h")
        } else {
            format!("{h} h {r} min")
        }
    }
}

/// Keep the pause countdown in the menu fresh.
pub fn start_refresher(app: AppHandle) {
    std::thread::Builder::new()
        .name("veil-tray".into())
        .spawn(move || loop {
            refresh(&app);
            std::thread::sleep(Duration::from_secs(30));
        })
        .expect("spawn tray refresher");
}
