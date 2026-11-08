use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// The application (and, for browsers, the site) that receives the user's input.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppContext {
    /// Stable identifier: bundle id on macOS, executable name on Windows/Linux.
    pub app_id: String,
    /// Human-readable name.
    pub name: String,
    pub pid: u32,
    pub window_title: Option<String>,
    /// Address-bar URL when the app is a browser and the URL could be read.
    pub url: Option<String>,
}

impl AppContext {
    /// Host of the URL (e.g. `chatgpt.com`), used in the activity log instead of the full URL.
    pub fn site(&self) -> Option<String> {
        let url = self.url.as_deref()?;
        let rest = url.split("://").nth(1).unwrap_or(url);
        let host = rest.split(['/', '?', '#']).next()?.trim();
        let host = host.rsplit('@').next()?.split(':').next()?;
        (!host.is_empty()).then(|| host.to_ascii_lowercase())
    }

    /// Short label for popups and logs: "Slack" or "Google Chrome — chatgpt.com".
    pub fn display(&self) -> String {
        match self.site() {
            Some(site) => format!("{} — {}", self.name, site),
            None => self.name.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
    pub fn distance(&self, other: &Point) -> f64 {
        ((self.x - other.x).powi(2) + (self.y - other.y).powi(2)).sqrt()
    }
}

/// An accessibility element under the pointer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UiElement {
    /// Platform role, e.g. `AXButton`, `Button`, `push button`.
    pub role: String,
    /// Lower-cased accessible name / title / description.
    pub label: String,
    /// Process that owns the element.
    pub pid: u32,
    pub is_button: bool,
    /// The element belongs to a file open/upload dialog.
    pub in_file_dialog: bool,
    /// Generic container with no name (web content the app does not expose).
    pub is_generic: bool,
}

/// Text of the focused composer, or the fact that the app does not expose it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FocusedText {
    Exposed { text: String, role: String },
    NotExposed { role: String },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClipboardContent {
    pub text: Option<String>,
    pub files: Vec<PathBuf>,
    /// Encoded image bytes (PNG/TIFF/BMP) when the clipboard holds an image.
    pub image: Option<Vec<u8>>,
}

/// Files (and text) carried by a drag-and-drop session in progress.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DragSession {
    pub files: Vec<PathBuf>,
    pub text: Option<String>,
    pub has_image: bool,
}

/// How the platform stops a drop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropStrategy {
    /// The drag data is withheld while over the target; the button-up is allowed and the drop lands empty
    /// (macOS).
    Withhold,
    /// The button-up is swallowed and the drag cancelled with Escape (Windows, Linux X11).
    SwallowAndCancel,
    /// Drops cannot be blocked on this platform/session (e.g. Wayland).
    Unsupported,
}

/// A permission the platform needs, shown in the setup wizard.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Permission {
    pub id: String,
    pub title: String,
    pub granted: bool,
    /// What the user has to do, in one or two sentences.
    pub help: String,
}

/// The channel through which content is leaving.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    Message,
    Paste,
    Drop,
    Upload,
    Typing,
}

impl Channel {
    pub fn is_file_channel(&self) -> bool {
        matches!(self, Channel::Drop | Channel::Upload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn site_is_extracted_from_url() {
        let app = AppContext {
            name: "Google Chrome".into(),
            url: Some("https://user:pw@ChatGPT.com:443/c/abc?x=1".into()),
            ..Default::default()
        };
        assert_eq!(app.site().as_deref(), Some("chatgpt.com"));
        assert_eq!(app.display(), "Google Chrome — chatgpt.com");
    }

    #[test]
    fn site_handles_bare_hosts_and_missing_urls() {
        let mut app = AppContext { name: "Arc".into(), ..Default::default() };
        assert_eq!(app.site(), None);
        app.url = Some("claude.ai/new".into());
        assert_eq!(app.site().as_deref(), Some("claude.ai"));
    }
}
