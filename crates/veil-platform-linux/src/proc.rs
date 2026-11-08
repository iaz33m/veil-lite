//! Process helpers (`/proc`) and app naming.

use std::path::Path;

/// Executable file name of a process, e.g. `slack`, `chrome`, `gnome-terminal-server`.
pub fn exe_name(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    let exe = std::fs::read_link(format!("/proc/{pid}/exe")).ok();
    let from_exe = exe.as_deref().and_then(Path::file_name).map(|n| n.to_string_lossy().trim_end_matches(" (deleted)").to_string());
    from_exe.filter(|s| !s.is_empty()).or_else(|| comm(pid))
}

/// Short process name from `/proc/<pid>/comm`.
pub fn comm(pid: u32) -> Option<String> {
    let s = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

/// Working directory of a process (used to resolve relative picker selections).
pub fn cwd(pid: u32) -> Option<std::path::PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
}

/// Human-readable app name from the executable and the X11 `WM_CLASS` class (or AT-SPI app name).
pub fn display_name(app_id: &str, class: Option<&str>) -> String {
    let known = match app_id {
        "chrome" | "google-chrome" | "google-chrome-stable" => Some("Google Chrome"),
        "chromium" | "chromium-browser" => Some("Chromium"),
        "brave" | "brave-browser" => Some("Brave"),
        "msedge" | "microsoft-edge" => Some("Microsoft Edge"),
        "firefox" | "firefox-bin" | "firefox-esr" => Some("Firefox"),
        "vivaldi-bin" => Some("Vivaldi"),
        "code" => Some("Visual Studio Code"),
        "slack" => Some("Slack"),
        "Discord" | "discord" => Some("Discord"),
        "teams-for-linux" => Some("Microsoft Teams"),
        _ => None,
    };
    if let Some(k) = known {
        return k.to_string();
    }
    let base = class.filter(|c| !c.is_empty()).unwrap_or(app_id);
    base.split(['-', '_', '.'])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut cs = w.chars();
            match cs.next() {
                Some(f) => f.to_uppercase().collect::<String>() + cs.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn is_browser(app_id: &str) -> bool {
    matches!(
        app_id,
        "chrome"
            | "google-chrome"
            | "google-chrome-stable"
            | "chromium"
            | "chromium-browser"
            | "brave"
            | "brave-browser"
            | "msedge"
            | "microsoft-edge"
            | "firefox"
            | "firefox-bin"
            | "firefox-esr"
            | "librewolf"
            | "zen"
            | "zen-bin"
            | "vivaldi-bin"
            | "opera"
            | "epiphany"
            | "falkon"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(display_name("chrome", Some("Google-chrome")), "Google Chrome");
        assert_eq!(display_name("echova-studio", None), "Echova Studio");
        assert_eq!(display_name("obsidian", Some("obsidian")), "Obsidian");
        assert!(is_browser("firefox") && !is_browser("slack"));
    }

    #[test]
    fn own_process_is_readable() {
        let pid = std::process::id();
        assert!(exe_name(pid).is_some());
        assert!(comm(pid).is_some());
        assert_eq!(exe_name(0), None);
    }
}
