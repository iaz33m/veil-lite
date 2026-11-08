//! Layer 3 runner: GLiNER2.5-multi-Decide exported to ONNX (int8), driven by `manifest.json`.
//!
//! Input per chunk: `prefix_ids + text_ids + suffix_ids` (the prefix/suffix encode the fixed four-check
//! schema and were computed by the model's own processor at export time). Output: `[batch, checks]`
//! scores. A text's score per check is the max over its chunks.

use super::chunk::{assemble, max_per_column, pad_batch, sigmoid, split_words, windows};
use super::Manifest;
use anyhow::{anyhow, Context};
use ort::session::{builder::GraphOptimizationLevel, Session};
use ort::value::Tensor;
use std::path::Path;
use std::sync::OnceLock;
use tokenizers::Tokenizer;
use veil_core::CheckId;

const BATCH: usize = 8;

static RUNTIME: OnceLock<Result<(), String>> = OnceLock::new();

/// Load ONNX Runtime from `dylib` once per process.
pub fn init_runtime(dylib: &Path) -> anyhow::Result<()> {
    RUNTIME
        .get_or_init(|| {
            let builder = ort::init_from(dylib).map_err(|e| format!("loading ONNX Runtime from {}: {e}", dylib.display()))?;
            builder.with_name("veil").build().map(|_| ()).map_err(|e| format!("initialising ONNX Runtime: {e}"))
        })
        .clone()
        .map_err(|e| anyhow!(e))
}

pub struct DecideModel {
    session: Session,
    tokenizer: Tokenizer,
    manifest: Manifest,
    checks: Vec<Option<CheckId>>,
}

impl DecideModel {
    pub fn load(dir: &Path, dylib: &Path, threads: usize) -> anyhow::Result<DecideModel> {
        let manifest = Manifest::load(dir)?;
        let tokenizer = load_tokenizer(&dir.join(&manifest.tokenizer))?;
        init_runtime(dylib)?;
        let session = Session::builder()
            .map_err(|e| anyhow!("{e}"))?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|e| anyhow!("{e}"))?
            .with_intra_threads(threads.clamp(1, 2))
            .map_err(|e| anyhow!("{e}"))?
            .commit_from_file(dir.join(&manifest.onnx))
            .map_err(|e| anyhow!("loading {}: {e}", manifest.onnx))?;
        let checks = manifest.checks.iter().map(|k| CheckId::from_key(k)).collect();
        Ok(DecideModel { session, tokenizer, manifest, checks })
    }

    /// Score `text` for every check the model knows.
    pub fn score(&mut self, text: &str, max_chunks: usize) -> anyhow::Result<Vec<(CheckId, f32)>> {
        let ids = text_token_ids(&self.tokenizer, &self.manifest, text, max_chunks)?;
        let rows = build_rows(&self.manifest, &ids, max_chunks);
        if rows.is_empty() {
            return Ok(Vec::new());
        }
        let n = self.manifest.checks.len();
        let mut all = Vec::with_capacity(rows.len() * n);
        for batch_rows in rows.chunks(BATCH) {
            let batch = pad_batch(batch_rows, self.manifest.pad_id);
            let ids = Tensor::from_array(([batch.rows, batch.cols], batch.ids)).map_err(|e| anyhow!("{e}"))?;
            let mask = Tensor::from_array(([batch.rows, batch.cols], batch.mask)).map_err(|e| anyhow!("{e}"))?;
            let inputs = ort::inputs![
                self.manifest.inputs.input_ids.as_str() => ids,
                self.manifest.inputs.attention_mask.as_str() => mask,
            ];
            let outputs = self.session.run(inputs).map_err(|e| anyhow!("model run failed: {e}"))?;
            let value = outputs
                .get(&self.manifest.output)
                .ok_or_else(|| anyhow!("model has no output named {}", self.manifest.output))?;
            let (_shape, data) = value.try_extract_tensor::<f32>().map_err(|e| anyhow!("{e}"))?;
            anyhow::ensure!(data.len() == batch.rows * n, "unexpected output size {}", data.len());
            all.extend_from_slice(data);
        }
        let mut scores = max_per_column(&all, n);
        if self.manifest.apply_sigmoid {
            scores.iter_mut().for_each(|s| *s = sigmoid(*s));
        }
        Ok(self.checks.iter().zip(scores).filter_map(|(c, s)| c.map(|c| (c, s.clamp(0.0, 1.0)))).collect())
    }
}

pub fn load_tokenizer(path: &Path) -> anyhow::Result<Tokenizer> {
    Tokenizer::from_file(path).map_err(|e| anyhow!("loading tokenizer {}: {e}", path.display()))
}

/// Text token ids (no special tokens), limited to what `max_chunks` windows can hold.
pub fn text_token_ids(tokenizer: &Tokenizer, m: &Manifest, text: &str, max_chunks: usize) -> anyhow::Result<Vec<u32>> {
    let window = m.window();
    let budget = window + (window.saturating_sub(m.chunk_overlap)).saturating_mul(max_chunks.saturating_sub(1));
    let lowered;
    let text = if m.lowercase {
        lowered = text.to_lowercase();
        lowered.as_str()
    } else {
        text
    };
    let encoding = if m.text_mode == "plain" {
        // Bound the work on huge texts: ~4 bytes per token is generous.
        let mut end = text.len().min(budget.saturating_mul(8));
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        tokenizer.encode(&text[..end], false)
    } else {
        let mut words = split_words(text);
        words.truncate(budget);
        tokenizer.encode(words, false)
    }
    .map_err(|e| anyhow!("tokenizing: {e}"))
    .context("tokenizer")?;
    let mut ids = encoding.get_ids().to_vec();
    ids.truncate(budget);
    Ok(ids)
}

/// Model input rows for the text ids.
pub fn build_rows(m: &Manifest, ids: &[u32], max_chunks: usize) -> Vec<Vec<i64>> {
    windows(ids.len(), m.window(), m.chunk_overlap, max_chunks)
        .into_iter()
        .map(|r| assemble(&m.prefix_ids, &ids[r], &m.suffix_ids))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::InputNames;

    /// A word-level tokenizer over a tiny vocabulary, whitespace pre-tokenizer.
    fn tiny_tokenizer(dir: &Path) -> Tokenizer {
        let json = r#"{
          "version": "1.0", "truncation": null, "padding": null, "added_tokens": [],
          "normalizer": null,
          "pre_tokenizer": {"type": "Whitespace"},
          "post_processor": null, "decoder": null,
          "model": {"type": "WordLevel", "unk_token": "[UNK]",
            "vocab": {"[UNK]": 0, "the": 1, "plan": 2, "is": 3, "confidential": 4, ",": 5, "ok": 6}}
        }"#;
        let p = dir.join("tokenizer.json");
        std::fs::write(&p, json).unwrap();
        load_tokenizer(&p).unwrap()
    }

    fn manifest(max_len: usize, overlap: usize, mode: &str) -> Manifest {
        Manifest {
            format: 1,
            model_id: "t".into(),
            onnx: "m.onnx".into(),
            tokenizer: "tokenizer.json".into(),
            max_len,
            chunk_overlap: overlap,
            prefix_ids: vec![100, 101],
            suffix_ids: vec![102],
            pad_id: 0,
            checks: vec!["secrets".into(), "confidential".into()],
            inputs: InputNames::default(),
            output: "scores".into(),
            apply_sigmoid: false,
            text_mode: mode.into(),
            lowercase: true,
        }
    }

    #[test]
    fn tokenizes_words_and_builds_overlapping_rows() {
        let dir = tempfile::tempdir().unwrap();
        let tok = tiny_tokenizer(dir.path());
        // window = 7 - 3 = 4 tokens, overlap 1.
        let m = manifest(7, 1, "words");
        let ids = text_token_ids(&tok, &m, "The plan is CONFIDENTIAL, ok", 32).unwrap();
        assert_eq!(ids, vec![1, 2, 3, 4, 5, 6]);
        let rows = build_rows(&m, &ids, 32);
        assert_eq!(rows, vec![vec![100, 101, 1, 2, 3, 4, 102], vec![100, 101, 4, 5, 6, 102]]);
        let plain = manifest(7, 1, "plain");
        assert_eq!(text_token_ids(&tok, &plain, "the plan", 32).unwrap(), vec![1, 2]);
    }

    #[test]
    fn long_texts_are_capped_at_max_chunks() {
        let dir = tempfile::tempdir().unwrap();
        let tok = tiny_tokenizer(dir.path());
        let m = manifest(7, 1, "words");
        let text = "the plan ".repeat(1000);
        let ids = text_token_ids(&tok, &m, &text, 3).unwrap();
        // 4 + 3 * 2 = 10 tokens is all three windows can hold.
        assert_eq!(ids.len(), 10);
        assert_eq!(build_rows(&m, &ids, 3).len(), 3);
    }

    /// Runs the real exported model when `VEIL_TEST_MODEL_DIR` (model files) and `VEIL_ORT_DYLIB` are set.
    #[test]
    fn real_model_when_available() {
        let Some(dir) = std::env::var_os("VEIL_TEST_MODEL_DIR") else { return };
        let dir = std::path::PathBuf::from(dir);
        let dylib = crate::model::find_ort_dylib(None, &dir).expect("set VEIL_ORT_DYLIB");
        let mut model = DecideModel::load(&dir, &dylib, 2).unwrap();
        let code = model.score("fn main() { let key = std::env::var(\"AWS_SECRET\").unwrap(); }", 32).unwrap();
        let chat = model.score("What a lovely day for a walk in the park!", 32).unwrap();
        let get = |v: &[(CheckId, f32)], c| v.iter().find(|(k, _)| *k == c).map(|(_, s)| *s).unwrap();
        eprintln!("code: {code:?}\nchat: {chat:?}");
        assert!(get(&code, CheckId::SourceCode) > get(&chat, CheckId::SourceCode));
    }
}
