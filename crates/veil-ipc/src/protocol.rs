//! Wire protocol between the desktop app (client) and `veil-engine` (server). **This file is the
//! contract**: the engine and the Tauri app both build against it.
//!
//! Transport: a local socket (Unix domain socket `0600` in a `0700` directory on macOS/Linux, a named pipe
//! on Windows), name from [`veil_core::paths::Paths::socket_name`].
//!
//! Framing: every message is a 4-byte little-endian length followed by that many bytes of UTF-8 JSON.
//! Messages larger than [`MAX_FRAME`] are rejected and the connection is closed.
//!
//! Session: the first request on every connection must be [`Request::Hello`] carrying the token read from
//! [`veil_core::paths::Paths::token_file`] (random 32 bytes, hex, rewritten each time the engine starts,
//! mode `0600`). Any other first message, or a wrong token, closes the connection.
//!
//! Each request gets exactly one response, except [`Request::Subscribe`]: after an `Ok` response the
//! server streams [`Response::Event`] frames on that connection until it closes. Use a dedicated
//! connection for subscriptions.

use serde::{Deserialize, Serialize};
use veil_core::activity::ActivityEntry;
use veil_core::scanner::ModelStatus;
use veil_core::types::Permission;
use veil_core::EngineEvent;

pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_FRAME: usize = 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum Request {
    Hello { token: String, protocol: u32 },
    Status,
    /// Current policy as YAML.
    GetPolicy,
    /// Replace the policy. The engine validates it, writes `policy.yaml` and applies it immediately.
    SetPolicy { yaml: String },
    Pause { minutes: u32 },
    Resume,
    /// Most recent activity entries, newest first.
    Activity { limit: usize },
    ClearActivity,
    ModelStatus,
    /// Start downloading the AI model (no-op if present). Progress arrives as `EngineEvent::Model`.
    DownloadModel,
    CancelDownload,
    /// Delete the downloaded model files.
    RemoveModel,
    Permissions,
    /// Trigger the OS permission request / open the right settings page.
    RequestPermission { id: String },
    Subscribe,
    /// Stop the engine (used by "Quit Veil").
    Shutdown,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum Response {
    Ok,
    Hello { engine_version: String, protocol: u32 },
    Status(StatusInfo),
    Policy { yaml: String },
    Activity { entries: Vec<ActivityEntry> },
    Model { status: ModelStatus },
    Permissions { list: Vec<Permission> },
    PermissionResult { granted: bool },
    Event(EngineEvent),
    Error { message: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StatusInfo {
    pub engine_version: String,
    /// Platform provider name, e.g. `macos`, `windows`, `linux-x11`, `linux-wayland`, `unsupported`.
    pub platform: String,
    /// Protection is running (hooks installed).
    pub active: bool,
    pub paused_remaining_secs: Option<u64>,
    pub model: ModelStatus,
    /// All required permissions granted.
    pub permissions_ok: bool,
    /// Drops can be blocked on this platform/session.
    pub drops_blockable: bool,
    /// OS OCR available for images.
    pub ocr_available: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_have_a_stable_json_shape() {
        let j = serde_json::to_string(&Request::Pause { minutes: 15 }).unwrap();
        assert_eq!(j, r#"{"method":"pause","params":{"minutes":15}}"#);
        assert_eq!(serde_json::to_string(&Request::Status).unwrap(), r#"{"method":"status"}"#);
        let back: Request = serde_json::from_str(r#"{"method":"set_policy","params":{"yaml":"version: 1"}}"#).unwrap();
        assert_eq!(back, Request::SetPolicy { yaml: "version: 1".into() });
    }

    #[test]
    fn responses_round_trip() {
        let r = Response::Model { status: ModelStatus::Downloading { percent: 40 } };
        let j = serde_json::to_string(&r).unwrap();
        assert_eq!(j, r#"{"type":"model","data":{"status":{"state":"downloading","percent":40}}}"#);
        assert_eq!(serde_json::from_str::<Response>(&j).unwrap(), r);
        let e = Response::Event(EngineEvent::Resumed);
        assert_eq!(serde_json::from_str::<Response>(&serde_json::to_string(&e).unwrap()).unwrap(), e);
    }
}
