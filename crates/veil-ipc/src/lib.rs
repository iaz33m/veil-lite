//! Veil local IPC: protocol and transport between the desktop app and the engine.

pub mod protocol;

pub use protocol::{Request, Response, StatusInfo, MAX_FRAME, PROTOCOL_VERSION};
