//! Desktop-app preferences (not the engine policy): whether first-run setup is done.

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DesktopPrefs {
    pub setup_complete: bool,
}

pub struct Prefs {
    path: Option<PathBuf>,
    inner: Mutex<DesktopPrefs>,
}

impl Prefs {
    /// Load from `<app config dir>/desktop.json`; missing or unreadable means defaults.
    pub fn load(dir: Option<PathBuf>) -> Self {
        let path = dir.map(|d| d.join("desktop.json"));
        let inner = path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Prefs { path, inner: Mutex::new(inner) }
    }

    pub fn get(&self) -> DesktopPrefs {
        self.inner.lock().clone()
    }

    pub fn update(&self, f: impl FnOnce(&mut DesktopPrefs)) -> Result<(), String> {
        let mut prefs = self.inner.lock();
        f(&mut prefs);
        let Some(path) = &self.path else { return Ok(()) };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("Couldn’t save settings: {e}"))?;
        }
        let json = serde_json::to_string_pretty(&*prefs).map_err(|e| e.to_string())?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)
            .and_then(|_| std::fs::rename(&tmp, path))
            .map_err(|e| format!("Couldn’t save settings: {e}"))
    }
}
