//! Forward engine events to the web view (`veil://event`) and report whether the engine is reachable
//! (`veil://engine-link`). Reconnects whenever the engine restarts.

use crate::tray;
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use veil_core::paths::Paths;
use veil_core::EngineEvent;
use veil_ipc::Client;

const RETRY: Duration = Duration::from_secs(2);

pub fn start(app: AppHandle, paths: Paths) {
    std::thread::Builder::new()
        .name("veil-events".into())
        .spawn(move || {
            let mut up: Option<bool> = None;
            let mut set_up = |app: &AppHandle, v: bool| {
                if up != Some(v) {
                    up = Some(v);
                    let _ = app.emit("veil://engine-link", v);
                    tray::refresh(app);
                }
            };
            loop {
                match Client::subscribe(&paths) {
                    Ok(subscription) => {
                        set_up(&app, true);
                        for event in subscription {
                            if matches!(event, EngineEvent::Paused { .. } | EngineEvent::Resumed) {
                                let app = app.clone();
                                std::thread::spawn(move || tray::refresh(&app));
                            }
                            let _ = app.emit("veil://event", &event);
                        }
                        set_up(&app, false);
                    }
                    Err(_) => set_up(&app, false),
                }
                std::thread::sleep(RETRY);
            }
        })
        .expect("spawn event thread");
}
