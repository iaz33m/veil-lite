//! Veil core.
//!
//! Everything here is platform-independent: the [`platform::Platform`] contract that each OS provider
//! implements, the hook-thread [`interceptor::Interceptor`] (hold first, inspect second), the decision
//! [`engine::Engine`] that runs on a worker thread, the [`policy::Policy`], redaction and the
//! metadata-only [`activity::ActivityLog`].

pub mod activity;
pub mod buffer;
pub mod engine;
pub mod findings;
pub mod input;
pub mod interceptor;
pub mod paths;
pub mod platform;
pub mod policy;
pub mod redact;
pub mod scanner;
pub mod shared;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod types;

#[cfg(test)]
mod flow_tests;

pub use engine::{Engine, EngineEvent, EventBus, Inspection, Job, Trigger};
pub use findings::{CheckId, Finding, FindingKind};
pub use input::{HookVerdict, InputEvent, Key, Modifiers, Replay};
pub use interceptor::Interceptor;
pub use platform::{Choice, InputHandler, Platform, Prompt, PromptKind};
pub use policy::{Action, Policy};
pub use scanner::{Extracted, ModelStatus, ScanLayers, Scanner};
pub use types::*;
