# Model export (layer 3)

Turns the **GLiNER2.5-multi-Decide** checkpoint (Apache-2.0) into the files Veil downloads on first run:

| File | What |
|---|---|
| `model.int8.onnx` | `input_ids`, `attention_mask` (int64 `[batch, seq]`) → `scores` (float32 `[batch, 4]`, already sigmoid) |
| `tokenizer.json` | text tokenizer, loaded by the Rust `tokenizers` crate |
| `manifest.json` | constant schema prefix/suffix token ids, check order, window size, text mode |

The four checks (`checks.json`) are one multi-label classification task: `secrets`, `personal_info`,
`confidential`, `source_code`. Because the schema is constant, its tokens are a fixed **prefix**; the Rust
runner only tokenizes the text and builds `prefix + text + suffix` per 512-token window
(`crates/veil-detect/src/model/runner.rs`).

Hugging Face is not reachable from the build container, so this runs on a developer Mac (or any machine
with internet access). CPU is enough.

## Steps

```bash
cd tools/model-export
python3 -m venv .venv && . .venv/bin/activate
pip install -r requirements.txt

# 1. Export: writes out/<model id>/ and fills sha256/size in crates/veil-detect/model-spec.json
python export.py
#    optional: copy into Veil's models dir to try it locally
python export.py --install ~/Library/Application\ Support/Veil/models

# 2. Parity and latency (PyTorch vs ONNX int8, mirrors the Rust input building)
python parity.py --threads 2

# 3. Upload the three files to the URLs in model-spec.json (or pass --base-url to export.py),
#    then mark the spec as published and commit it:
python export.py --publish-only
```

Veil refuses to download while `"published": false` and rejects any file whose SHA-256 differs from the spec.

To run the Rust runner against the exported files:

```bash
VEIL_TEST_MODEL_DIR=out/gliner2.5-multi-decide-int8 VEIL_ORT_DYLIB=/path/to/libonnxruntime.dylib \
  cargo test -p veil-detect real_model -- --nocapture
```

## If the export stops

The script relies on a few GLiNER2 internals, all in `common.py` and marked `# VERIFY:`:

- `DEFAULT_MODEL` — the Hugging Face id of the multilingual Decide checkpoint.
- `schema_for` — the classification schema format (label descriptions, `multi_label`, `cls_threshold`).
- `find_encoder` / `find_classifier` / `find_tokenizer` — attribute names; on failure the module tree is
  printed so the right names can be filled in.
- `classify` / `parse_scores` — `classify_text` signature and output shape.
- `DecideWrapper` — assumes the head is applied directly to the label tokens' encoder states with a
  sigmoid; `export.py` checks this against the library's own scores and stops if they differ (e.g. an
  extra projection layer or softmax).

`export.py` also verifies that the text tokens the Rust runner will produce (`split_words` +
`tokenizer.json`) appear verbatim inside the library's input ids, and picks `text_mode` (`words` or
`plain`) accordingly.

## Budgets to check (docs/ARCHITECTURE.md §12)

- Download ≈ 300 MB or less (int8).
- Model peak memory < 500 MB in the engine.
- p95 < 300 ms per 512-token chunk on 2 threads.
