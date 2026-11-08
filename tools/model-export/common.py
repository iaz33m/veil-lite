"""Shared helpers for export.py and parity.py.

Everything that depends on GLiNER2 internals is isolated here and marked `# VERIFY:`. When an attribute is
missing, the helpers print the model's module tree and stop, so the names can be fixed quickly.
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path
from typing import Any

import torch

HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent
SPEC_PATH = REPO / "crates" / "veil-detect" / "model-spec.json"
CHECKS_PATH = HERE / "checks.json"

# VERIFY: Hugging Face id of the GLiNER2.5 multilingual Decide checkpoint (Apache-2.0).
DEFAULT_MODEL = "fastino/gliner2.5-multi-decide"

# Must match `split_words` in crates/veil-detect/src/model/chunk.rs.
WORD_RE = re.compile(r"\w+(?:[-_]\w+)*|\S")

MAX_LEN = 512
CHUNK_OVERLAP = 64


def split_words(text: str) -> list[str]:
    return WORD_RE.findall(text)


def load_checks(path: Path = CHECKS_PATH) -> dict[str, Any]:
    return json.loads(path.read_text(encoding="utf-8"))


def load_model(name: str):
    try:
        from gliner2 import GLiNER2  # VERIFY: package / class name
    except ImportError:
        sys.exit("gliner2 is not installed: pip install -r requirements.txt")
    model = GLiNER2.from_pretrained(name)
    model.eval()
    return model


def schema_for(checks: dict[str, Any]) -> dict[str, Any]:
    """Classification schema with one multi-label task whose labels are the four checks.

    VERIFY: GLiNER2 accepts `{"task": {"labels": {label: description}, "multi_label": True,
    "cls_threshold": 0.0}}` in `classify_text`. If descriptions are not supported, use a plain list of
    labels instead (the label text then carries the meaning).
    """
    labels = {c["label"]: c["description"] for c in checks["checks"]}
    return {checks["task"]: {"labels": labels, "multi_label": bool(checks.get("multi_label", True)), "cls_threshold": 0.0}}


def dump_tree(model: torch.nn.Module, depth: int = 3) -> None:
    print("\n--- model module tree (depth %d) ---" % depth, file=sys.stderr)
    for name, module in model.named_modules():
        if name.count(".") < depth:
            n = sum(p.numel() for p in module.parameters(recurse=True))
            print(f"{name or '<root>'}: {type(module).__name__} ({n:,} params)", file=sys.stderr)
    print("--- attributes ---", file=sys.stderr)
    print(", ".join(a for a in dir(model) if not a.startswith("_")), file=sys.stderr)


def find_encoder(model) -> torch.nn.Module:
    """The transformer encoder (DeBERTa-v3 family)."""
    from transformers import PreTrainedModel

    # VERIFY: attribute name of the encoder in GLiNER2.
    for attr in ("encoder", "token_rep_layer", "bert_layer", "model"):
        m = getattr(model, attr, None)
        if isinstance(m, PreTrainedModel):
            return m
        inner = getattr(m, "model", None) if m is not None else None
        if isinstance(inner, PreTrainedModel):
            return inner
    candidates = [m for _, m in model.named_modules() if isinstance(m, PreTrainedModel)]
    if candidates:
        return max(candidates, key=lambda m: sum(p.numel() for p in m.parameters()))
    dump_tree(model)
    sys.exit("could not find the transformer encoder; set it in common.find_encoder")


def find_classifier(model) -> torch.nn.Module:
    """The classification head applied to label-token embeddings."""
    # VERIFY: attribute name of the classification head in GLiNER2.
    for attr in ("classifier", "classification_head", "cls_head", "label_classifier", "classification_layer"):
        m = getattr(model, attr, None)
        if isinstance(m, torch.nn.Module):
            return m
    for name, m in model.named_modules():
        if "classif" in name.lower() and isinstance(m, torch.nn.Module) and any(True for _ in m.parameters()):
            return m
    dump_tree(model)
    sys.exit("could not find the classification head; set it in common.find_classifier")


def find_tokenizer(model):
    """The Hugging Face fast tokenizer used for the text."""
    # VERIFY: where GLiNER2 keeps its tokenizer.
    for path in ("processor.tokenizer", "tokenizer", "data_processor.tokenizer", "schema_processor.tokenizer"):
        obj = model
        for part in path.split("."):
            obj = getattr(obj, part, None)
            if obj is None:
                break
        if obj is not None and hasattr(obj, "backend_tokenizer"):
            return obj
    dump_tree(model)
    sys.exit("could not find a fast tokenizer; set it in common.find_tokenizer")


def classify(model, text: str, schema: dict[str, Any]):
    """Run the library's own classification (the reference)."""
    # VERIFY: `classify_text(text, schema, include_confidence=True)` signature.
    try:
        return model.classify_text(text, schema, include_confidence=True)
    except TypeError:
        return model.classify_text(text, schema)


def parse_scores(result: Any, checks: dict[str, Any]) -> list[float]:
    """Scores per check (in checks.json order) from `classify_text` output.

    VERIFY: handles `{"task": [{"label": l, "confidence": c}, ...]}`, `{"task": {l: c}}` and
    `{"task": [(l, c), ...]}`. Labels missing from the output (below threshold) score 0.
    """
    task = checks["task"]
    value = result.get(task, result) if isinstance(result, dict) else result
    found: dict[str, float] = {}
    items = value.items() if isinstance(value, dict) else value
    for item in items or []:
        if isinstance(item, dict):
            label, conf = item.get("label"), item.get("confidence", item.get("score", 1.0))
        elif isinstance(item, (tuple, list)) and len(item) == 2:
            label, conf = item
        else:
            label, conf = item, 1.0
        found[str(label)] = float(conf)
    return [found.get(c["label"], 0.0) for c in checks["checks"]]


class Capture:
    """Records the encoder's inputs/outputs and the classifier's inputs/outputs during one library call."""

    def __init__(self, encoder: torch.nn.Module, classifier: torch.nn.Module):
        self.encoder_kwargs: dict[str, Any] | None = None
        self.hidden: torch.Tensor | None = None
        self.cls_in: torch.Tensor | None = None
        self.cls_out: torch.Tensor | None = None
        self._handles = [
            encoder.register_forward_hook(self._on_encoder, with_kwargs=True),
            classifier.register_forward_hook(self._on_classifier),
        ]

    def _on_encoder(self, _module, args, kwargs, output):
        if self.encoder_kwargs is None:
            kw = dict(kwargs)
            if args:
                kw.setdefault("input_ids", args[0])
                if len(args) > 1:
                    kw.setdefault("attention_mask", args[1])
            self.encoder_kwargs = kw
            self.hidden = output.last_hidden_state if hasattr(output, "last_hidden_state") else output[0]

    def _on_classifier(self, _module, args, output):
        if self.cls_in is None:
            self.cls_in = args[0]
            self.cls_out = output

    def close(self) -> None:
        for h in self._handles:
            h.remove()


class DecideWrapper(torch.nn.Module):
    """input_ids, attention_mask → sigmoid scores [batch, checks].

    The schema (with one label token per check) is a constant prefix, so the label tokens sit at fixed
    positions: take their hidden states and run the classification head on them.
    """

    def __init__(self, encoder: torch.nn.Module, classifier: torch.nn.Module, positions: list[int], sigmoid: bool = True):
        super().__init__()
        self.encoder = encoder
        self.classifier = classifier
        self.register_buffer("positions", torch.tensor(positions, dtype=torch.long), persistent=False)
        self.sigmoid = sigmoid

    def forward(self, input_ids: torch.Tensor, attention_mask: torch.Tensor) -> torch.Tensor:
        out = self.encoder(input_ids=input_ids, attention_mask=attention_mask)
        hidden = out.last_hidden_state if hasattr(out, "last_hidden_state") else out[0]
        labels = hidden.index_select(1, self.positions)  # [B, L, H]
        logits = self.classifier(labels)  # [B, L, 1] or [B, L]
        logits = logits.reshape(logits.shape[0], -1)
        return torch.sigmoid(logits) if self.sigmoid else logits


def windows(n: int, window: int, overlap: int, max_chunks: int) -> list[range]:
    """Mirror of `windows` in crates/veil-detect/src/model/chunk.rs."""
    if n == 0 or window == 0:
        return []
    step = max(window - overlap, 1)
    out, start = [], 0
    while len(out) < max_chunks:
        end = min(start + window, n)
        out.append(range(start, end))
        if end >= n:
            break
        start += step
    return out
