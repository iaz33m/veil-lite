# AGENTS.md

Instructions for AI coding agents (and humans) working in this repository. Read this, then
[CONTEXT.md](CONTEXT.md) (vocabulary) and [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) (design).

## What Veil is

An on-device DLP agent: it holds outgoing input (Enter, Send click, paste, drop, file-picker Open) in any
desktop app, checks it with three detection layers, and replays it, drops it, or redacts it after asking
the user. Rust workspace + Tauri v2 app. macOS, Windows, Linux.

## Repository map

| Path | What | Builds on |
|---|---|---|
| `crates/veil-core` | `Platform` trait, `Interceptor` (hook-thread state machine), `Engine` (worker: decide/ask/replay), `Policy`, redaction, `ActivityLog`, `Paths`, keystroke buffer. No OS code | all |
| `crates/veil-core/src/testing.rs` | `MockPlatform`, `MockScanner` (feature `testing`) | all |
| `crates/veil-core/src/flow_tests.rs` | End-to-end flows through interceptor + engine with the mocks | all |
| `crates/veil-detect` | `VeilScanner`: L1 keywords, L2 recognizers (redact-core + Veil EU/APAC), file extraction, chunking, L3 model (`ort` load-dynamic + `tokenizers`), model download/verify | all |
| `crates/veil-ipc` | `protocol.rs` (the contract: `Request`/`Response`/`StatusInfo`), framing, client, server | all |
| `crates/veil-engine` | The binary: wiring, threads, IPC server, policy hot-reload, `check` subcommand | all |
| `crates/veil-platform-macos` | CGEventTap, AX, NSPasteboard, Vision, alerts/NSPanel | macOS only (empty elsewhere) |
| `crates/veil-platform-windows` | LL hooks, SendInput, UIA, clipboard, OLE drag, Media.Ocr, TaskDialog | Windows only |
| `crates/veil-platform-linux` | evdev grab + uinput, AT-SPI, arboard, X11 XDND, notifications | Linux only |
| `apps/desktop` | Tauri v2 app (Svelte + TS UI in `src/`, Rust in `src-tauri/`, excluded from the workspace) | all |
| `tools/model-export` | Python: GLiNER2.5-multi-Decide → ONNX int8 + manifest + parity test (run on a Mac/PC with HF access) | — |
| `tests/harness/test-chat.html` | Fake chat page for safe manual tests | — |
| `tests/fixtures` | Fake-data files + `expected.json` | — |

## Commands

```bash
cargo build --workspace
cargo test --workspace                       # unit + flow tests (mock platform)
cargo test -p veil-core                      # fastest loop for interceptor/engine/policy work
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all                              # rustfmt.toml: max_width 120
cargo run -p veil-engine -- check <file>     # run the detectors on a file, print findings (types only)
python3 tests/fixtures/make_fixtures.py      # regenerate fixtures after editing the script

cd apps/desktop
npm install && npm run check && npm run build
npm run tauri dev                            # needs the sidecar: ../../scripts/prepare-sidecar.sh
```

Platform crates only compile their real code on their own OS. To check another OS's provider from a
machine that has the target's std installed:
`cargo check -p veil-platform-windows --target x86_64-pc-windows-msvc`.

## Invariants (do not break)

1. **The hook thread never scans and never blocks.** `Interceptor::on_input` decides Pass/Swallow in
   microseconds and hands work to the engine through the job queue (`try_send`). No file I/O, no
   detection, no IPC, no prompts on the hook thread.
2. **Never call the platform while holding the state lock** (`Shared::state`). Read what you need, drop the
   guard, then call the platform. Hooks on Windows/macOS re-enter; holding the lock deadlocks input.
3. **Replays never re-enter the handler.** Providers tag injected events (CGEventSource user data,
   `dwExtraInfo`, the uinput device) and drop them before `on_input`.
4. **Only replay what was swallowed**, in original order: the trigger first, then input queued during the
   hold. Swallowed key-downs register their key-up in `pending_keyups`; held clicks set `swallow_up`.
5. **No content anywhere persistent.** The activity log (`ActivityEntry`) stores detection *types*, app
   id/name, site host, channel, outcome and file count. Never text, matched values, file names, paths,
   window titles or full URLs. Same for `tracing` output and IPC. `activity.rs` has a test for this; extend
   it when adding fields.
6. **Fail modes are policy, not accidents.** Text fails open by default, files fail closed
   (`actions.fail_mode`). A held action with no prompt on screen is released after `HOLD_WATCHDOG` (4 s).
7. **Elements of other processes are never Send buttons.** `element.pid` must equal the target app's pid
   (stops Veil's own popup buttons from triggering holds).
8. **Redact & send only when the composer text was read through accessibility** (`FocusedText::Exposed`);
   the keystroke buffer is a best guess and select-all + paste of a wrong guess would destroy text.
9. Drop handling follows the provider's `DropStrategy`; when a drop may have landed (`Unsupported`, or
   `Withhold` that wasn't withheld) the engine arms the **backstop** for the next send in that app.

## Testing approach

- Platform-independent logic is developed test-first (red → green → refactor) against `MockPlatform` and
  `MockScanner` (`veil-core` feature `testing`). See `flow_tests.rs` for the harness pattern: feed
  `InputEvent`s to the interceptor, drain jobs into `Engine::handle`, assert on replays, prompts, banners
  and activity outcomes.
- Detectors: table tests with valid, near-miss and multilingual cases; fixture expectations in
  `tests/fixtures/expected.json`.
- Platform providers: unit-test pure helpers (key mapping, label parsing); everything else is verified by
  hand with `tests/MANUAL_TEST_PLAN.md` on the real OS (Phase B).
- Never use real secrets or personal data in tests.

## Workflow skills

The maintainers chose two skill sets. Install them into the repo (project-local) before starting work:

```bash
npx skills@latest add mattpocock/skills     # select the engineering skills
npx skills add nutlope/hallmark             # UI design
```

Then run `/setup-matt-pocock-skills` once (issue tracker: GitHub Issues; domain docs: `CONTEXT.md` and
`docs/decisions/`).

| Skill | Use it for |
|---|---|
| `setup-matt-pocock-skills` | One-time configuration |
| `grill-with-docs` | Resolve open questions one at a time before a milestone; record terms in `CONTEXT.md`, decisions in `docs/decisions/` |
| `to-prd` → `to-issues` | One PRD per milestone, split into thin vertical-slice issues |
| `tdd` | All platform-independent code: interceptor, buffer, policy, detectors, extractors, IPC |
| `prototype` | Throwaway proofs: GLiNER → ONNX → Rust spike, Windows drag-cancel proof, Wayland input checks |
| `diagnose` | Platform bugs found during Phase B testing |
| `improve-codebase-architecture` / `zoom-out` | Architecture reviews (after providers land, before v0.1) |
| `triage` | Incoming issues |
| `review` / `handoff` | Review against `main` before merging; handoff notes between long sessions |
| `hallmark` | Design and audit every UI screen (setup wizard, settings, activity, tray copy, popup copy); floor checks + `sloplint` must pass |

## Conventions

- Words from `CONTEXT.md`, not synonyms (say *hold*, not "block" or "intercept", for the temporary state).
- Commit titles: `<crate or area>: <imperative summary>`; sign off (`git commit -s`).
- User-facing strings: short, plain, name the *kind* of data, never the value.
- New dependency? Prefer pure Rust, permissive licenses, and add it to the crate's `Cargo.toml` (shared
  versions go in `[workspace.dependencies]`).
