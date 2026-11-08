# Development

How to build, run and verify each part of Veil. Agent-specific notes are in [AGENTS.md](../AGENTS.md).

## Prerequisites

| Tool | Version | Notes |
|---|---|---|
| Rust | 1.92+ | `rustup update stable`; `rustfmt` and `clippy` components |
| Node.js | 22 | for `apps/desktop` |
| Tauri v2 prerequisites | — | <https://v2.tauri.app/start/prerequisites/> |
| Python | 3.11+ | only for `tools/model-export` and fixtures |

Linux system packages (Debian/Ubuntu):

```bash
sudo apt install build-essential pkg-config libudev-dev libdbus-1-dev libxkbcommon-dev \
  libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev patchelf
```

## Workspace layout

`crates/*` form one Cargo workspace. `apps/desktop/src-tauri` is a separate crate (excluded from the
workspace) that depends on `veil-ipc` and `veil-core` by path.

## Core and detectors

```bash
cargo test -p veil-core           # interceptor + engine flows with the mock platform (fast)
cargo test -p veil-detect         # recognizers, extraction, chunking (model tests skip without a model)
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

## The engine

```bash
cargo run -p veil-engine -- --help
cargo run -p veil-engine -- check tests/fixtures/message-sensitive.txt   # detection only, no hooks
cargo run -p veil-engine -- check -                                      # read text from stdin
VEIL_HOME=/tmp/veil-dev RUST_LOG=veil=debug cargo run -p veil-engine       # run with hooks, isolated state
```

`check` prints finding *types* and scores for a file or stdin; it is the quickest way to test recognizers
and extraction. Running the engine for real needs the platform permissions below.

### Permissions while developing

| OS | What to grant |
|---|---|
| macOS | System Settings → Privacy & Security → Accessibility: add the binary you run (`target/debug/veil-engine`) or, when started from a terminal, the terminal app. Re-grant after the binary changes path |
| Windows | None. Run unelevated (elevated windows are not reachable anyway) |
| Linux | `sudo cp packaging/linux/70-veil.rules /etc/udev/rules.d/ && sudo groupadd -f veil && sudo usermod -aG veil $USER`, reload udev, log out and in; make sure accessibility is on (`gsettings set org.gnome.desktop.interface toolkit-accessibility true` on GNOME) |

(The udev rule's exact path is defined by the Linux provider/packaging; check the platform crate's README
if it differs.)

## The desktop app

The app bundles the engine as a Tauri sidecar (`externalBin`), which must exist as
`apps/desktop/src-tauri/binaries/veil-engine-<target-triple>[.exe]` before `tauri dev`/`tauri build`:

```bash
./scripts/prepare-sidecar.sh                 # debug build for the host
./scripts/prepare-sidecar.sh --release       # release build
./scripts/prepare-sidecar.sh --release --target universal-apple-darwin   # macOS universal (lipo)
```

Then:

```bash
cd apps/desktop
npm install
npm run check        # svelte-check / TypeScript
npm run tauri dev
npm run tauri build  # local installer for the host OS
```

The AI layer needs the ONNX Runtime shared library at runtime (`ort` is built with `load-dynamic`).
`./scripts/fetch-onnxruntime.sh` downloads the official ONNX Runtime release for the host into
`apps/desktop/src-tauri/resources/onnxruntime/`, where the app bundles it. For a bare engine run, point
`VEIL_ORT_DYLIB` at the library instead.

### UI work

Design and audit screens with the Hallmark skill (`npx skills add nutlope/hallmark`, then `/hallmark`).
Floor checks (contrast, focus states, reduced motion, narrow widths) and `sloplint` must pass before a UI
PR merges.

## The AI model (`tools/model-export`)

The L3 model is an ONNX int8 export of GLiNER2.5-multi-Decide's classification path, produced once per
model version on a machine that can reach Hugging Face (a Mac works; the CI and dev containers can't).

```bash
cd tools/model-export
python3 -m venv .venv && source .venv/bin/activate
pip install -r requirements.txt
python export.py --out out/          # ONNX + int8 + tokenizer.json + manifest.json
python parity.py --out out/          # compares ONNX vs PyTorch scores on the multilingual sample set
```

Publish `out/` to the model repository, then record each file's URL, size and SHA-256 in the
compiled-in model spec in `crates/veil-detect` (the exporter prints the snippet). Releases must never ship
a spec with unpinned hashes.

## Fixtures and the harness

```bash
python3 tests/fixtures/make_fixtures.py      # regenerate fake-data fixtures (deterministic)
open tests/harness/test-chat.html            # or double-click; works from file://, no network
```

The harness imitates a chat composer (Enter to send, Send button, file input, drop zone, paste area) and
logs what "arrived". Point Veil at it to test every channel without sending anything real.

## Verification plan

Phase A (code for all platforms) ends with CI green on all three runners. Phase B tests one platform at a
time with [tests/MANUAL_TEST_PLAN.md](../tests/MANUAL_TEST_PLAN.md):

| Phase | Platform | Exit criteria |
|---|---|---|
| T1 | macOS 12+ (Apple silicon and Intel) | All channel tests pass on harness, Slack/Cursor (Electron), Chrome on chatgpt.com/claude.ai, a native app; budgets met; clean-machine install from the DMG |
| T2 | Windows 10 and 11 | Same, including the drag-and-drop cancel proof and OCR with a language pack |
| T3 | Linux: GNOME Wayland + an X11 session | Same, with Wayland limits documented; deb/rpm install sets up permissions |
| R1 | All | Signing, notarization, updater, LIMITATIONS and THREAT_MODEL reviewed → v0.1 |

Budgets (from ARCHITECTURE §12): engine idle RSS < 40 MB; with model < 500 MB peak; hook callback < 1 ms
typical, < 5 ms max; send decision with cached verdict < 50 ms p95; L3 < 300 ms p95 per 512-token chunk on a
4-core laptop; idle CPU ≈ 0.

## Releasing

Tagged `v*` commits run `.github/workflows/release.yml`: build the sidecar per target, fetch ONNX Runtime,
build installers with `tauri-action`, sign/notarize (secrets), upload to a draft GitHub release with an
SBOM. A maintainer reviews and publishes the draft.
