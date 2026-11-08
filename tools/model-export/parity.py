#!/usr/bin/env python3
"""Compare the exported ONNX model with the original GLiNER2 model, and measure CPU latency.

The ONNX side mirrors the Rust runner (crates/veil-detect/src/model/runner.rs) step by step: split words,
tokenize with tokenizer.json, cap tokens to what max_chunks windows hold, windows of `max_len - prefix -
suffix` tokens with `chunk_overlap` overlap, rows = prefix + window + suffix, padded batches of 8, score per
check = max over windows.

  python parity.py                          # samples from checks.json
  python parity.py --text-file notes.txt    # plus your own texts (one per paragraph)
  python parity.py --threads 2 --runs 50    # latency with the thread count Veil uses
Exits non-zero when any score differs by more than --tolerance.
"""

from __future__ import annotations

import argparse
import json
import statistics
import sys
import time
from pathlib import Path

import numpy as np

import common

BATCH = 8


class OnnxDecide:
    def __init__(self, model_dir: Path, threads: int):
        import onnxruntime as ort
        from tokenizers import Tokenizer

        self.m = json.loads((model_dir / "manifest.json").read_text())
        self.tok = Tokenizer.from_file(str(model_dir / self.m["tokenizer"]))
        opts = ort.SessionOptions()
        opts.intra_op_num_threads = threads
        opts.graph_optimization_level = ort.GraphOptimizationLevel.ORT_ENABLE_ALL
        self.sess = ort.InferenceSession(str(model_dir / self.m["onnx"]), opts, providers=["CPUExecutionProvider"])

    def window(self) -> int:
        return self.m["max_len"] - len(self.m["prefix_ids"]) - len(self.m["suffix_ids"])

    def text_ids(self, text: str, max_chunks: int) -> list[int]:
        m = self.m
        window = self.window()
        budget = window + max(window - m["chunk_overlap"], 0) * max(max_chunks - 1, 0)
        if m.get("lowercase"):
            text = text.lower()
        if m.get("text_mode", "words") == "plain":
            ids = self.tok.encode(text[: budget * 8], add_special_tokens=False).ids
        else:
            words = common.split_words(text)[:budget]
            ids = self.tok.encode(words, is_pretokenized=True, add_special_tokens=False).ids
        return ids[:budget]

    def score(self, text: str, max_chunks: int = 32) -> list[float]:
        m = self.m
        ids = self.text_ids(text, max_chunks)
        rows = [m["prefix_ids"] + ids[r.start : r.stop] + m["suffix_ids"] for r in common.windows(len(ids), self.window(), m["chunk_overlap"], max_chunks)]
        if not rows:
            return [0.0] * len(m["checks"])
        scores = []
        for i in range(0, len(rows), BATCH):
            batch = rows[i : i + BATCH]
            width = max(len(r) for r in batch)
            b_ids = np.array([r + [m["pad_id"]] * (width - len(r)) for r in batch], dtype=np.int64)
            b_mask = np.array([[1] * len(r) + [0] * (width - len(r)) for r in batch], dtype=np.int64)
            out = self.sess.run([m["output"]], {m["inputs"]["input_ids"]: b_ids, m["inputs"]["attention_mask"]: b_mask})[0]
            scores.append(out)
        s = np.concatenate(scores).max(axis=0)
        if m.get("apply_sigmoid"):
            s = 1 / (1 + np.exp(-s))
        return np.clip(s, 0, 1).tolist()


def percentile(values: list[float], q: float) -> float:
    v = sorted(values)
    return v[min(len(v) - 1, int(round(q * (len(v) - 1))))]


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--model", default=common.DEFAULT_MODEL)
    ap.add_argument("--dir", type=Path, help="exported model directory (default: out/<model id>)")
    ap.add_argument("--checks", type=Path, default=common.CHECKS_PATH)
    ap.add_argument("--text-file", type=Path, action="append", default=[])
    ap.add_argument("--threads", type=int, default=2)
    ap.add_argument("--runs", type=int, default=30)
    ap.add_argument("--tolerance", type=float, default=0.08, help="int8 vs PyTorch max abs score difference")
    ap.add_argument("--skip-torch", action="store_true", help="only measure the ONNX model")
    args = ap.parse_args()

    spec = json.loads(common.SPEC_PATH.read_text())
    model_dir = args.dir or (common.HERE / "out" / spec["id"])
    checks = common.load_checks(args.checks)
    keys = [c["key"] for c in checks["checks"]]
    texts = list(checks["samples"])
    for f in args.text_file:
        texts += [p.strip() for p in f.read_text(encoding="utf-8").split("\n\n") if p.strip()]

    onnx = OnnxDecide(model_dir, args.threads)
    if onnx.m["checks"] != keys:
        sys.exit(f"manifest checks {onnx.m['checks']} differ from checks.json {keys}")

    worst = 0.0
    if not args.skip_torch:
        model = common.load_model(args.model)
        schema = common.schema_for(checks)
        print(f"{'text':52} {'check':14} {'torch':>6} {'onnx':>6} {'diff':>6}")
        for text in texts:
            ref = common.parse_scores(common.classify(model, text, schema), checks)
            got = onnx.score(text)
            for k, a, b in zip(keys, ref, got):
                d = abs(a - b)
                worst = max(worst, d)
                flag = "  <-- " if d > args.tolerance else ""
                print(f"{text[:50]!r:52} {k:14} {a:6.3f} {b:6.3f} {d:6.3f}{flag}")
        print(f"\nmax |torch - onnx| = {worst:.4f} (tolerance {args.tolerance})")

    # Latency: a chat message, a paragraph, and a long document (several chunks).
    short = "Can you summarise this for me? The Q3 numbers are confidential until Friday."
    para = " ".join(texts[:4])
    long_doc = " ".join(texts) * 40
    print(f"\nlatency with {args.threads} thread(s), {args.runs} runs:")
    for name, text in (("message", short), ("paragraph", para), ("document", long_doc)):
        onnx.score(text)  # warm-up
        times = []
        for _ in range(args.runs):
            t0 = time.perf_counter()
            onnx.score(text)
            times.append((time.perf_counter() - t0) * 1000)
        n_chunks = len(common.windows(len(onnx.text_ids(text, 32)), onnx.window(), onnx.m["chunk_overlap"], 32))
        print(f"  {name:10} chunks={n_chunks:2}  p50={statistics.median(times):7.1f} ms  p95={percentile(times, 0.95):7.1f} ms")

    try:
        import resource

        rss = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
        rss_mb = rss / (1024 * 1024) if sys.platform == "darwin" else rss / 1024
        print(f"peak RSS (whole process, incl. PyTorch if loaded): {rss_mb:.0f} MB")
    except ImportError:
        pass

    if worst > args.tolerance:
        sys.exit(1)


if __name__ == "__main__":
    main()
