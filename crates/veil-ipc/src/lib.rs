//! Veil local IPC: protocol and transport between the desktop app and the engine.
//!
//! Blocking, thread-based API on top of `interprocess` local sockets (Unix domain sockets on macOS and
//! Linux, named pipes on Windows). See [`protocol`] for the wire format.

mod client;
mod framing;
pub mod protocol;
mod server;
mod transport;

pub use client::{Client, Subscription};
pub use framing::{read_frame, write_frame};
pub use protocol::{Request, Response, StatusInfo, MAX_FRAME, PROTOCOL_VERSION};
pub use server::{Handler, Server};

/// Version reported in `Hello` responses (the workspace version).
pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");
