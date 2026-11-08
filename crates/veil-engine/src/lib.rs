//! Veil engine library: the IPC service, logging and the fallback platform. The binary (`main.rs`) wires
//! them to the real detection pipeline (`veil-detect`) and the OS provider for the current platform.

pub mod logging;
pub mod service;
pub mod unsupported;

pub use service::{ModelStore, ScannerControl, Service};
pub use unsupported::UnsupportedPlatform;

#[cfg(test)]
mod tests;
