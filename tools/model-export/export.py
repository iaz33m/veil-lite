#!/usr/bin/env python3
"""Export GLiNER2.5-multi-Decide to the ONNX model Veil runs on-device (layer 3).

Output (in --out/<model id>/):
  model.int8.onnx   input_ids, attention_mask (int64, [batch, seq]) → scores (float32, [batch, 4], sigmoid)
  tokenizer.json    the text tokenizer (Hugging Face `tokenizers` format, loaded by the Rust runner)
  manifest.json     how the Rust runner builds inputs: prefix_ids + text ids + suffix_ids, check order, …
and updates crates/veil-detect/model-spec.json with the files' SHA-256 and sizes.

How it works without depending on GLiNER2 internals more than necessary:
  1. Run the library's own `classify_text` once on a sentinel text with forward hooks on the encoder and
     the classification head. That yields the exact input ids the library builds for our fixed schema and
     the hidden states that feed the head.
  2. Locate the sentinel's token ids inside those input ids → everything before is the constant schema
     prefix, everything after is the suffix.
  3. Match each classification-head input row to a position in the encoder output → the label-token
     positions (inside the prefix, so fixed for every text).
  4. Wrap encoder + head into one module, check it reproduces the library's scores, export to ONNX
     (opset 17, dynamic batch/sequence), quantize weights to int8, and check the ONNX outputs again.

Usage (on a Mac or Linux box with internet access to Hugging Face):
  python -m venv .venv && . .venv/bin/activate && pip install -r requirements.txt
  python export.py                       # writes out/ and updates model-spec.json (published stays false)
  python export.py --install ~/Library/Application\\ Support/Veil/models   # also copy for local testing
  python parity.py                       # compare PyTorch vs ONNX and measure latency
After uploading the three files to the URLs in model-spec.json, run `python export.py --publish-only` to
set "published": true (hashes must already be filled in).
"""

from __future__ import annotations

import argparse
import hashlib
import json
import shutil
import sys
import time
from pathlib import Path

import numpy as np
import torch

import common

SENTINEL = "veil sentinel alpha bravo charlie delta echo foxtrot"


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def find_subsequence(haystack: list[int], needle: list[int]) -> int:
    for i in range(len(haystack) - len(needle) + 1):
        if haystack[i : i + len(needle)] == needle:
            return i
    return -1


def capture_call(model, encoder, classifier, text: str, schema):
    cap = common.Capture(encoder, classifier)
    try:
        with torch.no_grad():
            result = common.classify(model, text, schema)
    finally:
        cap.close()
    if cap.encoder_kwargs is None or cap.cls_in is None:
        common.dump_tree(model)
        sys.exit("hooks did not fire: the encoder/classifier found are not the ones classify_text uses")
    return cap, result


def text_ids(tokenizer_json: Path, text: str, mode: str) -> list[int]:
    """Text ids exactly as the Rust runner computes them (`tokenizers` crate semantics)."""
    from tokenizers import Tokenizer

    tok = Tokenizer.from_file(str(tokenizer_json))
    if mode == "words":
        return tok.encode(common.split_words(text), is_pretokenized=True, add_special_tokens=False).ids
    return tok.encode(text, add_special_tokens=False).ids


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--model", default=common.DEFAULT_MODEL, help="Hugging Face id or local path of the checkpoint")
    ap.add_argument("--checks", type=Path, default=common.CHECKS_PATH)
    ap.add_argument("--out", type=Path, default=common.HERE / "out")
    ap.add_argument("--opset", type=int, default=17)
    ap.add_argument("--install", type=Path, help="also copy the files to <dir>/<model id>/ (Veil's models directory)")
    ap.add_argument("--base-url", help="URL prefix for the files in model-spec.json (default: keep existing)")
    ap.add_argument("--publish-only", action="store_true", help="only set published=true in model-spec.json")
    ap.add_argument("--tolerance", type=float, default=2e-3, help="max |wrapper - library| score difference")
    args = ap.parse_args()

    spec = json.loads(common.SPEC_PATH.read_text())
    if args.publish_only:
        missing = [f["name"] for f in spec["files"] if len(f["sha256"]) != 64 or f["size"] <= 0]
        if missing:
            sys.exit(f"hashes/sizes missing for {missing}: run the export first")
        spec["published"] = True
        common.SPEC_PATH.write_text(json.dumps(spec, indent=2) + "\n")
        print("model-spec.json: published = true")
        return

    checks = common.load_checks(args.checks)
    keys = [c["key"] for c in checks["checks"]]
    schema = common.schema_for(checks)
    out_dir = args.out / spec["id"]
    out_dir.mkdir(parents=True, exist_ok=True)

    print(f"loading {args.model} …")
    model = common.load_model(args.model)
    encoder = common.find_encoder(model)
    classifier = common.find_classifier(model)
    tokenizer = common.find_tokenizer(model)
    print(f"encoder: {type(encoder).__name__}, head: {type(classifier).__name__}, tokenizer: {type(tokenizer).__name__}")

    # 1. Capture the library's inputs for the sentinel text.
    cap, _ = capture_call(model, encoder, classifier, SENTINEL, schema)
    extra = set(cap.encoder_kwargs) - {"input_ids", "attention_mask", "return_dict", "output_hidden_states"}
    if extra:
        # VERIFY: if the library passes token_type_ids or position ids, add them to DecideWrapper.
        print(f"warning: encoder also receives {sorted(extra)}; the export ignores them", file=sys.stderr)
    ids = cap.encoder_kwargs["input_ids"]
    mask = cap.encoder_kwargs.get("attention_mask")
    if ids.shape[0] != 1:
        sys.exit(f"expected one sequence, got batch {ids.shape[0]}")
    n = int(mask[0].sum()) if mask is not None else ids.shape[1]
    full = ids[0, :n].tolist()

    # 2. Prefix / suffix around the sentinel, and which tokenization mode matches the library.
    tok_json = out_dir / "tokenizer.json"
    tokenizer.backend_tokenizer.save(str(tok_json))
    mode, start, sent = None, -1, []
    for candidate in ("words", "plain"):
        sent = text_ids(tok_json, SENTINEL, candidate)
        start = find_subsequence(full, sent)
        if start >= 0:
            mode = candidate
            break
    if mode is None:
        print(f"library ids: {full}", file=sys.stderr)
        sys.exit("sentinel tokens not found in the library's input ids; check common.split_words / tokenizer")
    prefix, suffix = full[:start], full[start + len(sent) :]
    print(f"text mode: {mode}; prefix {len(prefix)} tokens, suffix {len(suffix)} tokens")

    # 3. Label-token positions: rows of the head input that equal encoder hidden states.
    hidden = cap.hidden[0, :n]
    cls_in = cap.cls_in.reshape(-1, cap.cls_in.shape[-1])
    if cls_in.shape[0] != len(keys):
        # VERIFY: the head may see extra rows (e.g. a task token); keep the last len(keys) rows.
        print(f"warning: head input has {cls_in.shape[0]} rows for {len(keys)} checks", file=sys.stderr)
        cls_in = cls_in[-len(keys) :]
    positions = []
    for row in cls_in:
        diff = (hidden - row).abs().max(dim=-1).values
        p = int(diff.argmin())
        if float(diff[p]) > 1e-4:
            # VERIFY: a projection sits between the encoder and the head; extend DecideWrapper with it.
            sys.exit("classification-head inputs are not raw encoder states; see common.DecideWrapper")
        positions.append(p)
    if max(positions) >= len(prefix):
        sys.exit(f"label tokens {positions} are not inside the schema prefix ({len(prefix)} tokens); unsupported layout")
    print(f"label positions: {positions}")

    # 4. Wrapper and its parity with the library.
    wrapper = common.DecideWrapper(encoder, classifier, positions, sigmoid=True).eval()
    worst = 0.0
    for text in checks["samples"]:
        _, result = capture_call(model, encoder, classifier, text, schema)
        ref = common.parse_scores(result, checks)
        row = prefix + text_ids(tok_json, text, mode)[: common.MAX_LEN - len(prefix) - len(suffix)] + suffix
        with torch.no_grad():
            got = wrapper(torch.tensor([row]), torch.ones(1, len(row), dtype=torch.long))[0].tolist()
        d = max(abs(a - b) for a, b in zip(got, ref) if b > 0) if any(r > 0 for r in ref) else 0.0
        worst = max(worst, d)
        print(f"  {text[:50]!r:55} lib={np.round(ref, 3)} wrap={np.round(got, 3)}")
    if worst > args.tolerance:
        # VERIFY: softmax instead of sigmoid (single-label), or a calibration step after the head.
        sys.exit(f"wrapper differs from the library by {worst:.4f} (> {args.tolerance}); see DecideWrapper")
    print(f"wrapper matches the library (max diff {worst:.5f})")

    # 5. ONNX export (fp32) and int8 dynamic quantization.
    fp32 = out_dir / "model.fp32.onnx"
    int8 = out_dir / "model.int8.onnx"
    sample = [prefix + text_ids(tok_json, t, mode)[:64] + suffix for t in checks["samples"][:2]]
    width = max(len(r) for r in sample)
    pad_id = int(tokenizer.pad_token_id if tokenizer.pad_token_id is not None else 0)
    dummy_ids = torch.tensor([r + [pad_id] * (width - len(r)) for r in sample])
    dummy_mask = torch.tensor([[1] * len(r) + [0] * (width - len(r)) for r in sample])
    print("exporting ONNX …")
    export_kwargs = dict(
        input_names=["input_ids", "attention_mask"],
        output_names=["scores"],
        dynamic_axes={"input_ids": {0: "batch", 1: "seq"}, "attention_mask": {0: "batch", 1: "seq"}, "scores": {0: "batch"}},
        opset_version=args.opset,
        do_constant_folding=True,
    )
    try:
        torch.onnx.export(wrapper, (dummy_ids, dummy_mask), str(fp32), dynamo=False, **export_kwargs)
    except TypeError:  # older torch without the `dynamo` argument
        torch.onnx.export(wrapper, (dummy_ids, dummy_mask), str(fp32), **export_kwargs)

    from onnxruntime.quantization import QuantType, quantize_dynamic

    print("quantizing to int8 …")
    quantize_dynamic(str(fp32), str(int8), weight_type=QuantType.QInt8, op_types_to_quantize=["MatMul", "Gemm"])

    # 6. Check the ONNX models against the wrapper (padded batch, like the Rust runner).
    import onnxruntime as ort

    rows = [prefix + text_ids(tok_json, t, mode)[: common.MAX_LEN - len(prefix) - len(suffix)] + suffix for t in checks["samples"]]
    width = max(len(r) for r in rows)
    b_ids = np.array([r + [pad_id] * (width - len(r)) for r in rows], dtype=np.int64)
    b_mask = np.array([[1] * len(r) + [0] * (width - len(r)) for r in rows], dtype=np.int64)
    with torch.no_grad():
        ref = np.stack([wrapper(torch.tensor([r]), torch.ones(1, len(r), dtype=torch.long))[0].numpy() for r in rows])
    for path in (fp32, int8):
        sess = ort.InferenceSession(str(path), providers=["CPUExecutionProvider"])
        t0 = time.perf_counter()
        got = sess.run(["scores"], {"input_ids": b_ids, "attention_mask": b_mask})[0]
        ms = (time.perf_counter() - t0) * 1000
        print(f"{path.name}: max diff vs PyTorch {np.abs(got - ref).max():.4f}, batch of {len(rows)} in {ms:.0f} ms")

    # 7. Manifest + spec.
    manifest = {
        "format": 1,
        "model_id": spec["id"],
        "onnx": int8.name,
        "tokenizer": tok_json.name,
        "max_len": common.MAX_LEN,
        "chunk_overlap": common.CHUNK_OVERLAP,
        "prefix_ids": prefix,
        "suffix_ids": suffix,
        "pad_id": pad_id,
        "checks": keys,
        "inputs": {"input_ids": "input_ids", "attention_mask": "attention_mask"},
        "output": "scores",
        "apply_sigmoid": False,
        "text_mode": mode,
        "lowercase": False,
        "source_model": args.model,
        "label_positions": positions,
        "exported_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
    }
    (out_dir / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    fp32.unlink(missing_ok=True)

    for f in spec["files"]:
        path = out_dir / f["name"]
        f["sha256"] = sha256(path)
        f["size"] = path.stat().st_size
        if args.base_url:
            f["url"] = args.base_url.rstrip("/") + "/" + f["name"]
    spec["published"] = False
    common.SPEC_PATH.write_text(json.dumps(spec, indent=2) + "\n")
    total = sum(f["size"] for f in spec["files"])
    print(f"wrote {out_dir} ({total / 1e6:.1f} MB) and updated {common.SPEC_PATH.relative_to(common.REPO)}")

    if args.install:
        dest = args.install.expanduser() / spec["id"]
        dest.mkdir(parents=True, exist_ok=True)
        for f in spec["files"]:
            shutil.copy2(out_dir / f["name"], dest / f["name"])
        print(f"installed to {dest}")


if __name__ == "__main__":
    main()
