//! Veil detection layers.
//!
//! * Layer 1 — [`keywords`]: user keywords and phrases, case- and accent-insensitive.
//! * Layer 2 — [`pii`]: personal-data and secret patterns with validators (redact-core + Veil's EU/APAC
//!   recognizers).
//! * Layer 3 — [`model`]: GLiNER2.5-multi-Decide exported to ONNX (int8), four yes/no checks, CPU only,
//!   downloaded on first run with the user's approval (feature `model`).
//!
//! [`VeilScanner`] puts them together behind [`veil_core::Scanner`], plus file text extraction ([`extract`]).

pub mod extract;
pub mod keywords;
pub mod model;
pub mod pii;

use keywords::KeywordMatcher;
use parking_lot::{Mutex, RwLock};
use pii::{parse_regions, PiiDetector, Region};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use veil_core::{Extracted, Finding, ModelStatus, Policy, ScanLayers, Scanner};

/// Files larger than this are not read (only their names are checked).
pub const DEFAULT_MAX_FILE_BYTES: u64 = 25 * 1024 * 1024;
/// ONNX Runtime threads for the model (CPU only, kept small so the machine stays responsive).
#[cfg(feature = "model")]
const MODEL_THREADS: usize = 2;

/// Everything the scanner needs, usually derived from the policy.
#[derive(Clone, Debug, PartialEq)]
pub struct DetectConfig {
    pub keywords: Vec<String>,
    pub pii_enabled: bool,
    /// `universal`, `us`, `uk`, `eu`, `apac`.
    pub pii_regions: Vec<String>,
    pub pii_min_score: f32,
    pub max_file_bytes: u64,
    pub model_enabled: bool,
    pub models_dir: PathBuf,
    /// ONNX Runtime library. `None` searches the default locations (see [`model::ort_dylib_candidates`]).
    pub ort_dylib: Option<PathBuf>,
    /// Max 512-token chunks scored per text.
    pub max_chunks: usize,
}

impl Default for DetectConfig {
    fn default() -> Self {
        DetectConfig {
            keywords: Vec::new(),
            pii_enabled: true,
            pii_regions: ["universal", "us", "uk", "eu", "apac"].map(String::from).to_vec(),
            pii_min_score: 0.5,
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
            model_enabled: true,
            models_dir: PathBuf::from("models"),
            ort_dylib: None,
            max_chunks: 32,
        }
    }
}

impl DetectConfig {
    pub fn from_policy(policy: &Policy, models_dir: &Path) -> Self {
        DetectConfig {
            keywords: policy.keywords.clone(),
            pii_enabled: policy.pii.enabled,
            pii_regions: policy.pii.regions.clone(),
            pii_min_score: policy.pii.min_score,
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
            model_enabled: policy.model.enabled,
            models_dir: models_dir.to_path_buf(),
            ort_dylib: std::env::var_os("VEIL_ORT_DYLIB").map(PathBuf::from),
            max_chunks: policy.model.max_chunks.max(1),
        }
    }
}

/// Settings read on every scan.
struct Settings {
    pii_enabled: bool,
    regions: HashSet<Region>,
    pii_min_score: f32,
    max_file_bytes: u64,
    model_enabled: bool,
    models_dir: PathBuf,
    #[cfg_attr(not(feature = "model"), allow(dead_code))]
    ort_dylib: Option<PathBuf>,
    max_chunks: usize,
}

impl Settings {
    fn from_config(cfg: &DetectConfig) -> Self {
        Settings {
            pii_enabled: cfg.pii_enabled,
            regions: parse_regions(&cfg.pii_regions),
            pii_min_score: cfg.pii_min_score,
            max_file_bytes: cfg.max_file_bytes,
            model_enabled: cfg.model_enabled,
            models_dir: cfg.models_dir.clone(),
            ort_dylib: cfg.ort_dylib.clone(),
            max_chunks: cfg.max_chunks.max(1),
        }
    }
}

/// Layer-3 state. The loaded session lives here; inference runs under this lock (one at a time).
struct ModelState {
    /// Missing / Ready / Loaded / Failed (Disabled when built without the `model` feature).
    status: ModelStatus,
    downloading: Option<u8>,
    #[cfg(feature = "model")]
    loaded: Option<model::runner::DecideModel>,
}

pub struct VeilScanner {
    settings: RwLock<Settings>,
    keywords: RwLock<KeywordMatcher>,
    pii: PiiDetector,
    model: Mutex<ModelState>,
}

impl VeilScanner {
    /// Build the scanner. Never fails: model problems show up in [`Scanner::model_status`].
    pub fn new(cfg: DetectConfig) -> Self {
        let settings = Settings::from_config(&cfg);
        let status = disk_status(&settings.models_dir);
        VeilScanner {
            keywords: RwLock::new(KeywordMatcher::new(&cfg.keywords)),
            settings: RwLock::new(settings),
            pii: PiiDetector::new(),
            model: Mutex::new(ModelState {
                status,
                downloading: None,
                #[cfg(feature = "model")]
                loaded: None,
            }),
        }
    }

    /// Apply a new policy (keywords, regions, thresholds, model on/off).
    pub fn apply_policy(&self, policy: &Policy) {
        self.set_keywords(&policy.keywords);
        let model_enabled = {
            let mut s = self.settings.write();
            s.pii_enabled = policy.pii.enabled;
            s.regions = parse_regions(&policy.pii.regions);
            s.pii_min_score = policy.pii.min_score;
            s.model_enabled = policy.model.enabled;
            s.max_chunks = policy.model.max_chunks.max(1);
            s.model_enabled
        };
        if !model_enabled {
            self.unload_model();
        }
    }

    /// Re-check the model files on disk (after a download finished or the model was removed).
    pub fn refresh_model(&self) {
        let models_dir = self.settings.read().models_dir.clone();
        let mut m = self.model.lock();
        m.downloading = None;
        #[cfg(feature = "model")]
        if !model::is_installed(&models_dir) {
            m.loaded = None;
        } else if m.loaded.is_some() {
            m.status = ModelStatus::Loaded;
            return;
        }
        m.status = disk_status(&models_dir);
    }

    /// Download progress for [`Scanner::model_status`]; `None` when no download is running.
    pub fn set_download_progress(&self, percent: Option<u8>) {
        self.model.lock().downloading = percent.map(|p| p.min(100));
    }

    pub fn set_model_failed(&self, reason: String) {
        let mut m = self.model.lock();
        m.downloading = None;
        #[cfg(feature = "model")]
        {
            m.loaded = None;
        }
        m.status = ModelStatus::Failed { reason };
    }

    /// Run layer 3 on `text`. Loads the model on first use.
    #[cfg(feature = "model")]
    fn model_findings(&self, text: &str, source: &str) -> Vec<Finding> {
        let (enabled, models_dir, explicit, max_chunks) = {
            let s = self.settings.read();
            (s.model_enabled, s.models_dir.clone(), s.ort_dylib.clone(), s.max_chunks)
        };
        if !enabled || text.trim().is_empty() {
            return Vec::new();
        }
        let mut m = self.model.lock();
        if m.loaded.is_none() {
            if !matches!(m.status, ModelStatus::Ready) {
                return Vec::new();
            }
            let Some(dylib) = model::find_ort_dylib(explicit.as_deref(), &models_dir) else {
                tracing::warn!("ONNX Runtime library not found; the AI checks are off");
                m.status = ModelStatus::Failed { reason: "ONNX Runtime library not found".into() };
                return Vec::new();
            };
            let dir = model::spec().dir(&models_dir);
            let started = std::time::Instant::now();
            match model::runner::DecideModel::load(&dir, &dylib, MODEL_THREADS) {
                Ok(loaded) => {
                    tracing::info!("AI model loaded in {:?}", started.elapsed());
                    m.loaded = Some(loaded);
                    m.status = ModelStatus::Loaded;
                }
                Err(e) => {
                    tracing::warn!("AI model failed to load: {e:#}");
                    m.status = ModelStatus::Failed { reason: format!("{e:#}") };
                    return Vec::new();
                }
            }
        }
        let Some(loaded) = m.loaded.as_mut() else { return Vec::new() };
        match loaded.score(text, max_chunks) {
            Ok(scores) => scores.into_iter().map(|(check, score)| Finding::check(check, score, source)).collect(),
            Err(e) => {
                tracing::warn!("AI check failed: {e:#}");
                Vec::new()
            }
        }
    }

    #[cfg(not(feature = "model"))]
    fn model_findings(&self, _text: &str, _source: &str) -> Vec<Finding> {
        Vec::new()
    }
}

/// Model state from what is on disk.
fn disk_status(models_dir: &Path) -> ModelStatus {
    if !cfg!(feature = "model") {
        ModelStatus::Disabled
    } else if model::is_installed(models_dir) {
        ModelStatus::Ready
    } else {
        ModelStatus::Missing
    }
}

impl Scanner for VeilScanner {
    fn scan_text(&self, text: &str, layers: ScanLayers, source: &str) -> Vec<Finding> {
        let mut out = Vec::new();
        if text.is_empty() {
            return out;
        }
        if layers.keywords {
            out.extend(self.keywords.read().scan(text, source));
        }
        if layers.pii {
            let s = self.settings.read();
            if s.pii_enabled && !s.regions.is_empty() {
                out.extend(self.pii.scan(text, &s.regions, s.pii_min_score, source));
            }
        }
        if layers.model {
            out.extend(self.model_findings(text, source));
        }
        out
    }

    fn extract(&self, path: &Path) -> Extracted {
        let max = self.settings.read().max_file_bytes;
        extract::extract(path, max)
    }

    fn model_status(&self) -> ModelStatus {
        let m = self.model.lock();
        match m.downloading {
            Some(percent) => ModelStatus::Downloading { percent },
            None => m.status.clone(),
        }
    }

    fn unload_model(&self) {
        #[cfg(feature = "model")]
        {
            let mut m = self.model.lock();
            if m.loaded.take().is_some() {
                tracing::info!("AI model unloaded");
                m.status = ModelStatus::Ready;
            }
        }
    }

    fn set_keywords(&self, keywords: &[String]) {
        *self.keywords.write() = KeywordMatcher::new(keywords);
    }
}
