# Threat model (v0.1)

Status: draft for v0.1, reviewed before every release. Vocabulary: [CONTEXT.md](../CONTEXT.md).

## 1. What Veil protects against

The **careless or hurried insider**: a person who means well and is about to paste, type, drop or upload
something sensitive into an app that sends it off the device (an AI chatbot, a chat app, a web form).

Veil's job is to make that moment visible and reversible. It is a safety net for honest mistakes, not a
control against someone determined to exfiltrate data.

## 2. Assets

| Asset | Why it matters |
|---|---|
| A1 Content being sent (text, files, images) | The thing we protect; also the most sensitive thing Veil touches |
| A2 The user's input stream | Veil sits in it; a bug could drop, reorder or invent keystrokes |
| A3 The policy | Decides what is protected |
| A4 The activity log | Metadata about what the user did; private even without content |
| A5 Model and update artifacts | Code and weights executed on the device |
| A6 The engine IPC | Can pause protection, change policy, read activity |

## 3. Trust boundaries

```
user ──input──▶ [OS input layer] ──▶ veil-engine hook ──▶ target app ──▶ network
                                         │
                          worker: detect, prompt (in-process, memory only)
                                         │
                     local IPC (token) ──┴── Veil app (Tauri, web view)
                                         │
                    network: model download (pinned SHA-256), updates (signed)
```

- Everything runs as the logged-in user. Veil does not cross into other users' sessions.
- The web view never sees content: the app only exchanges policy, status and metadata with the engine.
- The only inbound data from the network is the model (hash-pinned) and app updates (signed manifests).

## 4. Threats and mitigations

STRIDE-style, by asset.

### A1 Content

| # | Threat | Mitigation | Residual risk |
|---|---|---|---|
| T1 | Veil writes content to disk (logs, cache, crash dumps) | Activity entries hold types only (unit-tested); `tracing` calls never format content; model cache is in-memory, keyed by SHA-256 | A future contributor logging a `Finding` span or text; guarded by review checklist and the activity-log test |
| T2 | Veil sends content over the network | No telemetry, no crash upload; network code exists only in the model downloader and the updater | Third-party dependency behavior; audit with SBOM per release |
| T3 | Content exposed over IPC | Protocol has no content-bearing messages (`Request`/`Response` in `veil-ipc/src/protocol.rs`) | — |
| T4 | Redaction leaves data in the clipboard | Original clipboard is restored after the swap (~180 ms) | Clipboard managers may record the temporary redacted text (not the original) |
| T5 | Popup shows the matched value over a shoulder | Prompts name the kind of data only ("a payment card number"); file names may appear for files | File names are visible in the prompt by design |

### A2 Input stream

| # | Threat | Mitigation | Residual risk |
|---|---|---|---|
| T6 | Engine hangs and freezes typing | Hook thread never scans or blocks; hold watchdog releases after 4 s without a prompt; text fails open by default | A provider bug in the hook itself (Phase B testing, hook budget < 1 ms) |
| T7 | Keys lost or reordered during a hold | Same-app input is queued and replayed after the trigger, in order; matching key-ups are swallowed | Input to *other* apps during a hold passes immediately (by design) |
| T8 | Replay loops (Veil re-inspecting its own replay) | Replays are tagged and never delivered to the handler | — |
| T9 | Veil's own popup buttons treated as Send controls | Send controls must belong to the target app's pid | — |
| T10 | Replay into the wrong window after a prompt | Engine refocuses the target app before replaying | User switches windows during the prompt; documented |

### A3 Policy and A6 IPC

| # | Threat | Mitigation | Residual risk |
|---|---|---|---|
| T11 | Another local user connects to the engine | Unix socket in a `0700` directory with mode `0600`; per-user named pipe on Windows; per-boot 256-bit token on every connection | Root/admin can do anything |
| T12 | Same-user malware pauses Veil or edits the policy | Out of scope (see §5) | Accepted |
| T13 | Malformed policy disables protection | Policy is validated before applying; invalid YAML is rejected and the previous policy stays | — |
| T14 | Oversized IPC frames exhaust memory | 1 MiB frame cap; connection closed on violation | — |

### A4 Activity log

| # | Threat | Mitigation | Residual risk |
|---|---|---|---|
| T15 | Log reveals what the user works on | Stores app, site host, channel, types, outcome, file count; never window titles or full URLs; can be disabled or cleared | The site host (e.g. `chatgpt.com`) is recorded |

### A5 Model and updates

| # | Threat | Mitigation | Residual risk |
|---|---|---|---|
| T16 | Tampered or swapped model file | SHA-256 for every file compiled into the release; partial downloads kept as `.part` and verified before rename; engine refuses an unverified model | Compromise of the release build itself |
| T17 | Malicious update | Tauri updater verifies signed manifests; macOS notarization; Windows Authenticode | Key compromise |
| T18 | Malicious ONNX graph | Only the pinned file loads; ONNX Runtime runs it on CPU without custom ops | ONNX Runtime vulnerabilities; keep it updated |

### Malicious inputs to detection

| # | Threat | Mitigation | Residual risk |
|---|---|---|---|
| T19 | Zip bomb / huge file | 25 MB size cap, entry count and depth limits on archives, extraction off the hook thread; unreadable → files fail closed | CPU spent before the cap trips |
| T20 | Crafted PDF/Office file crashes the extractor | Extraction errors become *unreadable* (fail closed for files) | A crash in a dependency takes the engine down; the app restarts it |
| T21 | Regex denial of service | Rust `regex` (linear time) for L2; `fancy-regex` only inside the tokenizer | — |

## 5. Out of scope

- Malware or scripts running as the same user (they can quit Veil, edit the policy, inject input through
  accessibility APIs, or upload files directly).
- A user who deliberately chooses *Send anyway*, pauses protection, or edits the policy. Overrides are
  logged as metadata; that is the whole control.
- Kernel-level or admin attackers.
- Exfiltration that never passes through user input in a desktop app: background sync, CLI tools, email
  clients sending queued mail, screenshots shared by other means, phones.

## 6. Bypasses we know about

Listed per platform in [LIMITATIONS.md](LIMITATIONS.md): context-menu paste, accessibility-API injection,
apps that expose neither text nor keystrokes, elevated windows on Windows, Wayland drops, images on Linux,
OCR without language packs.

## 7. Review triggers

Update this document when: a new channel or platform provider is added, any network code is added, the
activity-log schema or IPC protocol changes, or a security report is resolved.
