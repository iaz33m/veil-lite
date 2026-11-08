//! Layer-3 model files: the compiled-in spec (what to download, pinned SHA-256), the manifest shipped with
//! the model (how to build inputs), download/verify/remove, and locating the ONNX Runtime library.

pub mod chunk;
mod download;
#[cfg(feature = "model")]
pub mod runner;

pub use download::{download, download_spec, http_agent};

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// One downloadable file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileSpec {
    pub name: String,
    pub url: String,
    /// Lower-case hex SHA-256. Empty until published.
    pub sha256: String,
    pub size: u64,
}

/// What the app downloads on first run (`model-spec.json`, compiled in).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelSpec {
    pub id: String,
    pub version: String,
    /// False until the exported model is uploaded and the hashes below are filled in.
    pub published: bool,
    pub license: String,
    /// Upstream checkpoint the export was made from.
    pub source: String,
    pub files: Vec<FileSpec>,
}

impl ModelSpec {
    pub fn total_size(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }

    /// Directory holding this model's files.
    pub fn dir(&self, models_dir: &Path) -> PathBuf {
        models_dir.join(&self.id)
    }
}

pub fn spec() -> &'static ModelSpec {
    static SPEC: OnceLock<ModelSpec> = OnceLock::new();
    SPEC.get_or_init(|| serde_json::from_str(include_str!("../../model-spec.json")).expect("valid model-spec.json"))
}

/// Model files present with the expected sizes (hashes are verified when downloading).
pub fn is_installed(models_dir: &Path) -> bool {
    is_installed_spec(spec(), models_dir)
}

pub fn is_installed_spec(spec: &ModelSpec, models_dir: &Path) -> bool {
    let dir = spec.dir(models_dir);
    !spec.files.is_empty()
        && spec.files.iter().all(|f| {
            std::fs::metadata(dir.join(&f.name))
                .map(|m| m.is_file() && (f.size == 0 || m.len() == f.size))
                .unwrap_or(false)
        })
}

/// Delete the downloaded model (including partial downloads).
pub fn remove(models_dir: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(spec().dir(models_dir)) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        r => r,
    }
}

/// How to feed the exported model (`manifest.json`, written by `tools/model-export`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub format: u32,
    #[serde(default)]
    pub model_id: String,
    /// ONNX file name inside the model directory.
    pub onnx: String,
    /// `tokenizer.json` file name.
    pub tokenizer: String,
    /// Max sequence length including prefix and suffix.
    pub max_len: usize,
    #[serde(default = "default_overlap")]
    pub chunk_overlap: usize,
    /// Schema/prompt token ids placed before the text.
    pub prefix_ids: Vec<i64>,
    /// Token ids placed after the text.
    pub suffix_ids: Vec<i64>,
    pub pad_id: i64,
    /// Output columns, in order: check keys (`secrets`, `personal_info`, `confidential`, `source_code`).
    pub checks: Vec<String>,
    #[serde(default)]
    pub inputs: InputNames,
    #[serde(default = "default_output")]
    pub output: String,
    /// Apply a sigmoid to the output (false when the graph already outputs probabilities).
    #[serde(default)]
    pub apply_sigmoid: bool,
    /// `words`: split text GLiNER-style and tokenize pre-split words; `plain`: tokenize the raw text.
    #[serde(default = "default_text_mode")]
    pub text_mode: String,
    #[serde(default)]
    pub lowercase: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InputNames {
    pub input_ids: String,
    pub attention_mask: String,
}

impl Default for InputNames {
    fn default() -> Self {
        InputNames { input_ids: "input_ids".into(), attention_mask: "attention_mask".into() }
    }
}

fn default_overlap() -> usize {
    64
}
fn default_output() -> String {
    "scores".into()
}
fn default_text_mode() -> String {
    "words".into()
}

impl Manifest {
    pub fn load(dir: &Path) -> anyhow::Result<Manifest> {
        let m: Manifest = serde_json::from_slice(&std::fs::read(dir.join("manifest.json"))?)?;
        anyhow::ensure!(m.format == 1, "unsupported model manifest format {}", m.format);
        anyhow::ensure!(!m.checks.is_empty(), "manifest lists no checks");
        anyhow::ensure!(m.max_len > m.prefix_ids.len() + m.suffix_ids.len() + 16, "max_len too small for the schema");
        Ok(m)
    }

    /// Text tokens per window.
    pub fn window(&self) -> usize {
        self.max_len - self.prefix_ids.len() - self.suffix_ids.len()
    }
}

/// File name of the ONNX Runtime shared library on this OS.
pub fn ort_library_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "libonnxruntime.dylib"
    } else if cfg!(windows) {
        "onnxruntime.dll"
    } else {
        "libonnxruntime.so"
    }
}

/// Where to look for ONNX Runtime: `$VEIL_ORT_DYLIB`, next to the executable, the macOS app bundle's
/// `Resources`/`Frameworks`, `../lib/veil` (Linux packages), then the models directory.
pub fn ort_dylib_candidates(models_dir: &Path) -> Vec<PathBuf> {
    let name = ort_library_name();
    let mut out = Vec::new();
    if let Some(p) = std::env::var_os("VEIL_ORT_DYLIB") {
        out.push(PathBuf::from(p));
    }
    if let Some(exe_dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
        out.push(exe_dir.join(name));
        if cfg!(target_os = "macos") {
            out.push(exe_dir.join("../Resources").join(name));
            out.push(exe_dir.join("../Resources/resources").join(name));
            out.push(exe_dir.join("../Frameworks").join(name));
        }
        if cfg!(target_os = "linux") {
            out.push(exe_dir.join("../lib/veil").join(name));
            out.push(exe_dir.join(format!("{name}.1")));
        }
        out.push(exe_dir.join("resources").join(name));
    }
    if cfg!(target_os = "linux") {
        out.push(PathBuf::from("/usr/lib/veil").join(name));
    }
    out.push(models_dir.join(name));
    out.push(models_dir.join("runtime").join(name));
    out
}

pub fn find_ort_dylib(explicit: Option<&Path>, models_dir: &Path) -> Option<PathBuf> {
    if let Some(p) = explicit {
        return p.is_file().then(|| p.to_path_buf());
    }
    ort_dylib_candidates(models_dir).into_iter().find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_spec_parses_and_is_not_published_yet() {
        let s = spec();
        assert_eq!(s.id, "gliner2.5-multi-decide-int8");
        let names: Vec<_> = s.files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, vec!["manifest.json", "tokenizer.json", "model.int8.onnx"]);
        if s.published {
            assert!(s.files.iter().all(|f| f.sha256.len() == 64 && f.size > 0));
        }
    }

    #[test]
    fn installed_means_files_with_expected_sizes() {
        let dir = tempfile::tempdir().unwrap();
        let spec = ModelSpec {
            id: "m".into(),
            version: "1".into(),
            published: true,
            license: String::new(),
            source: String::new(),
            files: vec![FileSpec { name: "a.bin".into(), url: String::new(), sha256: String::new(), size: 3 }],
        };
        assert!(!is_installed_spec(&spec, dir.path()));
        std::fs::create_dir_all(spec.dir(dir.path())).unwrap();
        std::fs::write(spec.dir(dir.path()).join("a.bin"), b"ab").unwrap();
        assert!(!is_installed_spec(&spec, dir.path()));
        std::fs::write(spec.dir(dir.path()).join("a.bin"), b"abc").unwrap();
        assert!(is_installed_spec(&spec, dir.path()));
    }

    #[test]
    fn manifest_defaults_and_validation() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("manifest.json"),
            r#"{"format":1,"onnx":"m.onnx","tokenizer":"tokenizer.json","max_len":512,"prefix_ids":[1,2],"suffix_ids":[3],"pad_id":0,"checks":["secrets"]}"#,
        )
        .unwrap();
        let m = Manifest::load(dir.path()).unwrap();
        assert_eq!(m.window(), 509);
        assert_eq!(m.output, "scores");
        assert_eq!(m.text_mode, "words");
        assert_eq!(m.inputs.input_ids, "input_ids");
        std::fs::write(dir.path().join("manifest.json"), r#"{"format":2,"onnx":"m","tokenizer":"t","max_len":512,"prefix_ids":[],"suffix_ids":[],"pad_id":0,"checks":["x"]}"#).unwrap();
        assert!(Manifest::load(dir.path()).is_err());
    }

    #[test]
    fn ort_candidates_include_env_and_models_dir() {
        let c = ort_dylib_candidates(Path::new("/m"));
        assert!(c.contains(&Path::new("/m").join(ort_library_name())));
        assert_eq!(find_ort_dylib(Some(Path::new("/definitely/missing.so")), Path::new("/m")), None);
    }
}
