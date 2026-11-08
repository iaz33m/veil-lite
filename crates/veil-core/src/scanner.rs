use crate::findings::Finding;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Which detection layers to run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScanLayers {
    pub keywords: bool,
    pub pii: bool,
    pub model: bool,
}

impl ScanLayers {
    /// Keywords and regex only: microseconds to a millisecond; used on every keystroke.
    pub const FAST: ScanLayers = ScanLayers { keywords: true, pii: true, model: false };
    pub const ALL: ScanLayers = ScanLayers { keywords: true, pii: true, model: true };
    pub const MODEL_ONLY: ScanLayers = ScanLayers { keywords: false, pii: false, model: true };
}

/// Text pulled out of a file, or the reason there is none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Extracted {
    Text(String),
    /// An image (or image-only document page) that needs OCR.
    Image(Vec<u8>),
    /// Several parts, e.g. the entries of a zip archive.
    Many(Vec<(String, Extracted)>),
    /// Too large; only the file name was checked.
    TooLarge,
    /// Encrypted or password-protected.
    Encrypted,
    Unsupported,
    Unreadable(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ModelStatus {
    /// Not downloaded yet; layers 1–2 protect alone.
    Missing,
    Downloading { percent: u8 },
    /// On disk and verified, not loaded.
    Ready,
    Loaded,
    Failed { reason: String },
    /// Built without model support.
    Disabled,
}

/// Detection pipeline (implemented by `veil-detect`).
pub trait Scanner: Send + Sync {
    /// Scan text with the given layers. `source` names where the text came from (for findings).
    fn scan_text(&self, text: &str, layers: ScanLayers, source: &str) -> Vec<Finding>;

    /// Extract the text of a file for scanning (file-name checks are done by the caller).
    fn extract(&self, path: &Path) -> Extracted;

    /// Current state of the layer-3 model.
    fn model_status(&self) -> ModelStatus;

    /// Release the model from memory (called after idle time).
    fn unload_model(&self) {}

    /// Replace user keywords (policy hot-reload).
    fn set_keywords(&self, _keywords: &[String]) {}
}
