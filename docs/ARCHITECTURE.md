# Veil: Architecture (v0.1)

Veil is an open-source (Apache-2.0) desktop agent that stops sensitive data from leaving through the apps
people type into: AI tools, chat apps, browsers, anything. It intercepts the moment of sending (Enter, Send
click, paste, file drop, file upload), checks the content locally, and lets the user send anyway, block,
or redact. Everything runs on the device; content is never stored or transmitted.

Status: **final for v0.1**. Changes go through a short decision record (section 14).

---

## 1. Decisions

| Area | Decision |
|---|---|
| License | Apache-2.0 |
| Language | Rust (engine, core, detectors, platform providers); TypeScript for the UI |
| UI shell | Tauri v2 (tray, onboarding, settings, activity, updater, autostart) |
| Platforms in v0.1 | macOS 12+, Windows 10/11 and Linux (X11 and Wayland). Code is written for all three now; testing on each platform is a separate, later phase (section 14) |
| Platform layer | One `Platform` trait; the provider is chosen at startup from the OS (runtime polymorphism, `Box<dyn Platform>`) |
| Channels | Typing (live warning), Enter / Send click, paste, drag-and-drop, file picker, images (OCR) |
| Default scope | All apps, minus an exclusion list (terminals, Finder/Explorer, password managers, system prompts, Veil itself) |
| Actions | **Send anyway**, **Block**, **Redact & send**, **Pause protection** (15 min / 1 h) |
| Default action | Popup offering Send anyway / Don't send / Redact & send (where redaction applies). Per category, policy can switch to **Block** (no override) |
| Layer 1 | User keywords and phrases |
| Layer 2 | Regex + validators for personal data, built on **`redact-core`** (Rust, Apache-2.0, Presidio-style recognizers with Luhn/IBAN/checksum validation) plus Veil's own recognizers for missing country formats (patterns may be adapted from `pii-vault`, MIT): universal (email, cards, IBAN, IPs, API keys, secrets) + US, UK, EU (ES, DE, FR, IT, …), Asia-Pacific (IN, PK, AU, SG, …) |
| Layer 3 | **GLiNER2.5-multi-Decide** (287M, multilingual, Apache-2.0), int8 ONNX, CPU only, via `ort` + `tokenizers` |
| Layer 3 checks | Secrets & credentials; personal information; confidential business info; source code |
| Model scope | Messages, file contents and OCR text (long content chunked, with caps) |
| Model delivery | A **setup wizard** on first launch first walks the user through every permission Veil needs (macOS Accessibility; Linux input group and accessibility bus; anything else a platform requires), then offers the AI model download (size shown). On the user's go-ahead the app downloads it from Hugging Face with progress and resume, and verifies a pinned SHA-256. Layers 1–2 protect while it downloads |
| OCR | OS-native only: Apple Vision (macOS), `Windows.Media.Ocr` (Windows). Linux has no OS-native OCR, so images are not inspected there (no fallback, by decision) |
| Activity log | Metadata only: time, app/site, detection type, action. Never content, never matched values |
| Telemetry | None. Network use: model download and update checks only |
| UI design | Designed with the **Hallmark** skill (`nutlope/hallmark`, MIT): theme, accessibility floor checks, `sloplint` at eight widths, fresh-context review |
| Engineering workflow | Matt Pocock's **engineering skills** (`mattpocock/skills`, MIT): grilling + docs, PRD → vertical-slice issues, TDD, diagnose, architecture review, handoff (section 15) |

---

## 2. System overview

```
┌────────────────────────── user session ──────────────────────────────────────────┐
│                                                                                   │
│  Veil.app / Veil.exe (Tauri v2)              veil-engine (headless, always on)    │
│  ┌───────────────────────────┐   local IPC   ┌─────────────────────────────────┐  │
│  │ tray · onboarding ·       │◀────────────▶│ platform provider (hooks, a11y,  │  │
│  │ settings · activity ·     │ (UDS / named │   clipboard, drag, OCR, popups)  │  │
│  │ model download · updater  │   pipe)       │ interceptor (hold/queue/replay)  │  │
│  │ window closed by default  │               │ detection pipeline (L1 L2 L3)    │  │
│  └───────────────────────────┘               │ policy engine · activity log     │  │
│              │ spawns + watches              └─────────────────────────────────┘  │
│              └────────────────────────────────────────▲                           │
└───────────────────────────────────────────────────────┼───────────────────────────┘
                                                        │ OS input / accessibility APIs
                                         apps: Chrome, Slack, Teams, ChatGPT, …
```

**Why two processes**
- The engine must never stall: a UI freeze or crash cannot touch the input hooks.
- The web view (WebView2 / WKWebView) costs tens of MB; the window stays closed unless opened, and the
  engine idles at a few MB.
- Other programs (enterprise agents, scripts) can drive the engine through the same documented IPC API.

**Process lifecycle**
- The Tauri app starts at login (autostart plugin) in tray mode and launches `veil-engine` (bundled as a
  Tauri `externalBin`, named `veil-engine-{target_triple}`).
- The engine holds a single-instance lock; the app restarts it if it exits (watchdog with backoff).
- macOS: the engine lives inside `Veil.app/Contents/MacOS/`, signed with the same identity, so the
  Accessibility permission is granted once to "Veil".
- Linux: the packages install a udev rule and add the user to the `veil` group so the engine can read
  input devices and create a virtual one (section 4); started through XDG autostart.
- Popups and the live-warning banner are drawn **natively by the engine** (not by the web UI), so
  enforcement keeps working when the UI process is not running.

---

## 3. Repository layout

```
veil/
  Cargo.toml                    workspace
  crates/
    veil-core/                  types, Platform trait, interceptor state machine, keystroke buffer,
                                policy engine, activity log (no OS code)
    veil-detect/                L1 keywords, L2 regex (redact-core + Veil recognizers),
                                L3 decide (ort + tokenizers), file text extraction, chunking, cache
    veil-ipc/                   protocol types (serde) + transport (UDS / named pipe), versioned
    veil-platform-macos/        CGEventTap, Accessibility, NSPasteboard, Vision OCR, NSPanel/alerts
    veil-platform-windows/      LL hooks, SendInput, UI Automation, clipboard, OLE drag, Shell,
                                Windows.Media.Ocr, banner window, TaskDialog
    veil-platform-linux/        evdev grab + uinput replay, AT-SPI (focus, text, element at point),
                                X11/Wayland clipboard, XDND drag handling (X11), GTK dialogs, notifications
    veil-engine/                binary: wiring, threads, lifecycle, single instance
  apps/desktop/                 Tauri v2 app (src-tauri + Svelte/Vite UI)
  tools/model-export/           Python: export GLiNER2.5-multi-Decide → ONNX, int8, parity test
  tests/harness/                local test chat page (records what it "receives"; no network)
  tests/fixtures/               sample files per format and language
  docs/                         ARCHITECTURE.md, THREAT_MODEL.md, LIMITATIONS.md, PRIVACY.md,
                                decisions/ (ADRs), agents/ (skill config)
  .agents/skills/               project-local agent skills (engineering skills, Hallmark)
  CONTEXT.md                    shared domain language (maintained with grill-with-docs)
  AGENTS.md / CLAUDE.md         agent instructions, incl. the "Agent skills" block
  LICENSE  SECURITY.md  CONTRIBUTING.md
```

---

## 4. The platform contract

Every OS-specific capability sits behind one trait. The core never calls an OS API directly.

```rust
pub trait Platform: Send + Sync {
    fn name(&self) -> &'static str;

    // input
    fn start_input(&self, sink: InputSink) -> Result<()>;         // events → core; core returns Pass/Hold
    fn replay(&self, ev: &HeldInput) -> Result<()>;               // re-inject, tagged as Veil's own
    fn inject_text(&self, text: &str) -> Result<()>;              // for Redact & send (clipboard swap)

    // context
    fn foreground(&self) -> AppContext;                           // app id, pid, window title, browser URL
    fn app_at(&self, p: Point) -> Option<AppContext>;
    fn element_at(&self, p: Point) -> Option<UiElement>;          // role, name, pid (Send / Open buttons)
    fn focused_text(&self, pid: u32) -> FocusedText;              // composer text, or NotExposed
    fn enable_accessibility(&self, app: &AppContext);             // Chromium/Electron tree switch

    // transfer
    fn clipboard(&self) -> ClipboardContent;                      // text, files, image
    fn drag_files(&self) -> Option<DragSession>;                  // files of the drag in progress
    fn block_drop(&self, s: &DragSession) -> Result<DropBlock>;   // macOS: withhold data; Windows: cancel
    fn picker_selection(&self, pid: u32) -> Option<Vec<PathBuf>>; // file dialog → paths

    // OCR
    fn ocr(&self, image: &ImageData, langs: &[Lang]) -> Result<Option<String>>; // None = unavailable

    // user interaction (native, works without the UI process)
    fn ask(&self, prompt: &Prompt) -> Decision;                   // Send anyway / Don't send / Redact
    fn banner(&self, text: &str);                                 // non-activating warning
    fn refocus(&self, app: &AppContext);

    // permissions
    fn permissions(&self) -> PermissionState;                     // e.g. macOS Accessibility
}

pub fn current_platform() -> Box<dyn Platform> {
    #[cfg(target_os = "macos")]   { return Box::new(veil_platform_macos::MacPlatform::new()); }
    #[cfg(target_os = "windows")] { return Box::new(veil_platform_windows::WinPlatform::new()); }
    #[cfg(target_os = "linux")]   { return Box::new(veil_platform_linux::LinuxPlatform::new()); }
    #[allow(unreachable_code)]    Box::new(Unsupported)
}
```

| Capability | macOS provider | Windows provider | Linux provider |
|---|---|---|---|
| Input hold / replay | `CGEventTap` (session, active); replays tagged via event-source user data + expected list | `WH_KEYBOARD_LL` / `WH_MOUSE_LL`; `SendInput` with `dwExtraInfo` tag | `evdev` exclusive grab of keyboard/mouse devices (`EVIOCGRAB`), events re-emitted through a `uinput` virtual device; works on X11 and Wayland |
| Composer text | AX: `AXFocusedUIElement`, `AXValue`, `AXEditableAncestor` | UIA: focused element, ValuePattern / TextPattern | AT-SPI (`atspi` crate over D-Bus): focused object, `Text` / `EditableText` interfaces |
| Chromium / Electron tree | `AXManualAccessibility`, `AXEnhancedUserInterface` | UIA client listening; `SPI_SETSCREENREADER` fallback (restored on exit) | Set `org.a11y.Status.IsEnabled` / `ScreenReaderEnabled` on the session bus |
| Focused app | `NSWorkspace` frontmost app | `SetWinEventHook(EVENT_SYSTEM_FOREGROUND)` | AT-SPI focus events (app name, pid); X11: `_NET_ACTIVE_WINDOW` |
| Element under cursor | `AXUIElementCopyElementAtPosition` | `IUIAutomation::ElementFromPoint` | AT-SPI `Component.GetAccessibleAtPoint` |
| Browser URL | AX read of the address bar | UIA read of the address bar | AT-SPI read of the address bar |
| Drag-and-drop block | Withhold the drag pasteboard while over a protected app; restore when leaving | Swallow button-up over the target, inject Esc, resync; verified by test (L4) | X11: hold the button-up at the device grab, inject Esc, resync. Wayland: not blockable in v0.1 (documented) |
| Files being dragged | Drag pasteboard file URLs | Explorer selection via Shell COM | X11: `XdndSelection` `text/uri-list`. Wayland: unavailable |
| Clipboard | `NSPasteboard` | Win32 clipboard (`CF_UNICODETEXT`, `CF_HDROP`, images) | `arboard` (X11 + Wayland data-control) incl. `text/uri-list` |
| OCR | Vision `VNRecognizeTextRequest` | `Windows.Media.Ocr` (needs language packs) | None (images not inspected) |
| Popup / banner | `CFUserNotificationDisplayAlert` / non-activating `NSPanel` | `TaskDialogIndirect` / layered `WS_EX_NOACTIVATE` window | GTK4 dialog / desktop notification (`notify-rust`) |
| Permissions | Accessibility (TCC) | none (signed `uiAccess` build later for elevated windows) | membership of the `veil` group (udev rule for `/dev/input` and `/dev/uinput`), AT-SPI enabled |

---|---|---|
| Input hold / replay | `CGEventTap` (session, active); replays tagged via event-source user data + expected list | `WH_KEYBOARD_LL` / `WH_MOUSE_LL`; `SendInput` with `dwExtraInfo` tag |
| Composer text | AX: `AXFocusedUIElement`, `AXValue`, `AXEditableAncestor` | UIA: focused element, ValuePattern / TextPattern |
| Chromium / Electron tree | `AXManualAccessibility`, `AXEnhancedUserInterface` | UIA client listening; `SPI_SETSCREENREADER` fallback (restored on exit) |
| Element under cursor | `AXUIElementCopyElementAtPosition` | `IUIAutomation::ElementFromPoint` |
| Browser URL | AX read of the address bar | UIA read of the address bar |
| Drag-and-drop block | Withhold the drag pasteboard while over a protected app; restore when leaving | Swallow button-up over the target, inject Esc, resync; verified by test (L4) |
| Files being dragged | Drag pasteboard file URLs | Explorer selection via Shell COM |
| OCR | Vision `VNRecognizeTextRequest` | `Windows.Media.Ocr` (needs language packs) |
| Popup / banner | `CFUserNotificationDisplayAlert` / non-activating `NSPanel` | `TaskDialogIndirect` / layered `WS_EX_NOACTIVATE` window |

---

## 5. Interception (the core state machine)

**Principle: hold first, inspect second, then replay, drop or redact.**

1. The provider's hook thread classifies each event in microseconds and never blocks:
   - Enter / Ctrl·Cmd+Enter in a protected app → **Hold**.
   - Click on a Send-type control (`send, submit, generate, post, reply, share, ask`, any language list in
     policy) whose element belongs to the target process → **Hold**.
   - Ctrl·Cmd+V → **Hold** if the clipboard has files, an image, or layer-1/2 hits; else pass and record
     the pasted text into the keystroke buffer.
   - File drop over a protected app → handled at drag start (scan) and drop (block or pass).
   - Open / Upload / double-click in a file dialog of a protected app → **Hold**.
2. While a key is held, further keys to the same app are queued and replayed in order afterwards.
3. The decision runs on the worker; the hook thread only receives Pass / Replay / Drop.
4. Replayed input is tagged so it is never re-inspected. Elements belonging to Veil's own popup never count
   as Send controls.
5. Timeouts: a hold that cannot be decided within **1.5 s** follows the channel's policy (`fail_open` or
   `fail_closed`; default: fail closed for files, fail open for text with a warning banner).
6. Backstop: after a blocked drop, the next send in that app within 10 min asks the user to remove the
   attachment first.

**Typing:** a per-app keystroke buffer mirrors typed text (Backspace, word delete, select-all-replace).
Layers 1–2 run on every keystroke (microseconds); the banner appears on the first hit. Layer 3 runs
**in the background** 400 ms after typing pauses; its verdict is cached by content hash, so pressing Enter
on unchanged text is decided instantly. Accessibility text, when exposed, replaces the buffer as the
source of truth.

---

## 6. Detection pipeline

```
content ──▶ normalize ──▶ L1 keywords ──▶ L2 regex+validators ──▶ L3 GLiNER2.5-multi-Decide ──▶ findings
   ▲                                                                   (lazy-loaded, int8, CPU)
   ├─ text: composer / buffer / clipboard
   ├─ files: extract text (txt, csv, json, code, docx, xlsx, pptx, odt, ods, odp, pdf, zip contents)
   └─ images: OS-native OCR → text
```

| Layer | What | Cost | Notes |
|---|---|---|---|
| L1 | Keywords and phrases from policy | µs | Case- and accent-insensitive |
| L2 | `redact-core` recognizers + Veil recognizers for US, UK, EU, APAC | < 1 ms per KB | Validators (Luhn, IBAN, mod-11/23, Verhoeff for Aadhaar…) to cut false positives |
| L3 | Four yes/no checks: secrets, personal information, confidential business, source code | ~150–300 ms per 512-token chunk on CPU | Probability per check; thresholds per check in policy |
| OCR | Vision / Windows.Media.Ocr | ~50–300 ms per image | Only for pasted, dropped or picked images and image-only PDF pages |

**Layer 3 runtime (no Python):**
- `tools/model-export` converts `fastino/GLiNER2.5-multi-Decide` to ONNX (classification path only),
  quantizes the encoder to int8, and runs a parity test against the PyTorch model on ~200 samples per
  supported language. Releases ship the ONNX file's SHA-256.
- At runtime: `tokenizers` builds the model input (the four check labels + text), `ort` (ONNX Runtime)
  runs it on 2 threads, the result is a probability per check.
- Loaded on first need, unloaded after 5 idle minutes. Until the model is downloaded, L1–L2 protect alone
  and the tray shows "AI checks: downloading".
- Long content: 512-token chunks with 64-token overlap, max 32 chunks per file (configurable). The first
  chunk that crosses a threshold stops the scan.

**Caching:** findings are cached by content hash (text) and by path + size + mtime (files).

---

## 7. Policy

One YAML file, hot-reloaded:
macOS `~/Library/Application Support/Veil/policy.yaml`, Windows `%APPDATA%\Veil\policy.yaml`.

```yaml
version: 1
scope:
  mode: all_apps                # all_apps | listed_apps
  exclude_apps: [com.apple.Terminal, WindowsTerminal.exe, com.1password.1password, ...]
keywords: [confidential, "project falcon"]
pii:
  regions: [universal, us, uk, eu, apac]
checks:                         # layer 3; action: ask (popup) or block (no override)
  secrets:          { threshold: 0.60, action: ask }
  personal_info:    { threshold: 0.70, action: ask }
  confidential:     { threshold: 0.70, action: ask }
  source_code:      { threshold: 0.75, action: ask }
actions:
  default: ask                  # ask = popup: Send anyway / Don't send / Redact & send
  allow_redact: true
  fail_mode: { text: open, files: closed }
pause:
  allowed: true                 # tray: 15 min / 1 h
activity_log:
  enabled: true                 # metadata only
```

The settings screen edits this file; advanced users can edit it directly.

---

## 8. Actions

| Action | Text (typed / pasted) | Files and images |
|---|---|---|
| **Send anyway** | Replay the held input | Replay the paste / drop / Open |
| **Block** (don't send) | Drop the held input; text stays in the box for editing | Drop it; drag data withheld; picker cancelled |
| **Redact & send** | Replace each finding with a mask (`[EMAIL]`, `****1234`), write the redacted text into the composer (clipboard swap: select-all + paste, original clipboard restored), then replay Enter | Not offered in v0.1 (files are not modified) |
| **Pause protection** | Tray: 15 min / 1 h; banner shows the remaining time; logged | same |

---

## 9. Desktop app (Tauri v2)

- **Tray:** status (protected / paused / AI checks downloading / permission missing), pause 15 min / 1 h,
  open Veil, quit.
- **Setup wizard (first launch, re-openable from the tray):**
  1. Welcome and what Veil does / never does (privacy summary).
  2. Permissions, one step per permission the platform needs: macOS Accessibility (opens the right
     System Settings pane and waits until granted); Linux `veil` group and accessibility bus (shows the
     exact command if the package could not set it up); Windows needs none in v0.1. Each step shows a
     live "granted / not yet" status.
  3. AI model: shows the size and asks to download; on confirmation the app downloads it (progress,
     pause/resume, retry), verifies the SHA-256 and hands it to the engine.
  4. Keywords and regions (pre-filled defaults), then "Protection is on".
- **Settings:** checks on/off and sensitivity, keywords, regions, scope and exclusions, action per check.
- **Activity:** metadata-only list with filters; export CSV.
- **Plugins:** autostart, single-instance, updater (signed), shell (engine sidecar), notification, store, log.
- The UI talks only to the engine IPC; it never touches hooks or content.

---

## 10. IPC

- Transport: Unix domain socket (`~/Library/Application Support/Veil/engine.sock`, mode 0600) on macOS;
  named pipe `\\.\pipe\veil-engine-<user SID>` with a per-user ACL on Windows.
- Framing: length-prefixed JSON (versioned schema in `veil-ipc`); every connection presents a token
  from a 0600 token file.
- Methods: `status`, `get_policy`/`set_policy`, `pause(minutes)`, `resume`, `activity(query)`,
  `model_status`, `subscribe(events)`. Content is never sent over IPC.

---

## 11. Privacy and security

- No content is stored, logged or sent. Activity entries hold detection *types*, never values.
- Network: model download (Hugging Face, pinned hash) and update checks (signed manifests). No telemetry.
- Model file and updates are integrity-checked; the engine refuses an unverified model.
- Known bypasses are documented in `LIMITATIONS.md`: input injected through accessibility APIs,
  context-menu paste, apps exposing no text and no keystrokes (canvas, remote desktops),
  elevated windows on Windows without a signed `uiAccess` build, OCR-less images on systems without
  language packs, images on Linux (no native OCR), file drops on Wayland, and Linux apps that do not
  expose AT-SPI (keystroke buffer only).
- `SECURITY.md` with a private disclosure address; `THREAT_MODEL.md` kept current.

---

## 12. Budgets (enforced in CI benchmarks)

| Metric | Target |
|---|---|
| Engine RSS idle (model unloaded) | < 40 MB |
| Engine RSS with model loaded | < 500 MB peak |
| Idle CPU | ≈ 0 (event-driven) |
| Hook callback | < 1 ms typical, < 5 ms max |
| Send decision with cached verdict | < 50 ms p95 |
| Layer 3 per 512-token chunk (CPU) | < 300 ms p95 on a 4-core laptop |
| Tauri app | window closed by default |

---

## 13. Packaging and release

- **macOS:** `.app` + DMG, hardened runtime, Developer ID signed and notarized; engine signed with the
  same identity; universal binary (arm64 + x86_64).
- **Windows:** MSI (WiX) and NSIS, Authenticode signed (open-source signing programme where available).
- **Linux:** `.deb` and `.rpm` (with the udev rule, group setup and XDG autostart entry) and AppImage
  (manual permission setup documented).
- **Updater:** Tauri updater with signed manifests; model updates through the same channel.
- **CI:** GitHub Actions matrix (macos-latest, windows-latest, ubuntu-latest): build, unit tests,
  detector accuracy suite, SBOM, signed artifacts. Harness end-to-end tests and budgets run in the
  platform testing phase.

---

## 14. Milestones (v0.1 = macOS + Windows + Linux)

Code for all three platforms is written first; each platform is then tested and hardened in its own phase.

**Phase A: build (all platforms)**

| | Milestone | Exit criteria |
|---|---|---|
| M0 | Workspace, CI, agent skills installed and configured (section 15), `CONTEXT.md` from a grilling session, `veil-core` types and `Platform` trait, IPC skeleton. **Spike** (via `prototype`): GLiNER2.5-multi-Decide → ONNX → Rust | Workspace builds on all three CI runners; spike parity within tolerance on 200 samples |
| M1 | Interceptor state machine, keystroke buffer, policy engine, activity log, L1 (platform-independent, unit-tested) | Unit tests green on all runners |
| M2 | macOS provider (port of the tested Swift PoC) | Compiles; unit tests for pure logic |
| M3 | Windows provider (per the Windows plan) | Compiles; unit tests for pure logic |
| M4 | Linux provider (evdev/uinput, AT-SPI, clipboard, X11 drag) | Compiles; unit tests for pure logic |
| M5 | L2 regex (all regions), file extraction, native OCR (macOS, Windows) | Detector accuracy suite green |
| M6 | L3 with background pre-scoring, thresholds, chunking | Accuracy measured per language |
| M7 | Actions (Redact & send, pause), backstop, policy hot-reload | Logic tests green |
| M8 | Tauri app: tray, setup wizard, settings, activity, model download; screens designed with Hallmark | Builds on all three runners; Hallmark floor checks and `sloplint` clean |

**Phase B: test and harden (one platform at a time)**

| | Milestone | Exit criteria |
|---|---|---|
| T1 | macOS: harness end-to-end tests, budgets, clean-machine install | All channel tests pass; budgets met |
| T2 | Windows: same, including the drag-and-drop proof | same |
| T3 | Linux: same on GNOME (Wayland) and an X11 session | same, with documented Wayland limits |
| R1 | Signing, notarization, updater, docs → **v0.1** | Signed releases; LIMITATIONS and THREAT_MODEL published |

**Decision records:** changes to section 1 are proposed as `docs/decisions/NNNN-title.md`
(context, decision, consequences) and linked here.

---

## 15. Development workflow and agent skills

Coding agents working on this repository use two skill sets, installed into the repo so every agent
and contributor gets the same workflow.

**Engineering: `mattpocock/skills` (MIT)**

Install: `npx skills@latest add mattpocock/skills` (select the engineering skills), then run
`/setup-matt-pocock-skills` once to configure the issue tracker (GitHub Issues), triage labels and the
domain-doc layout. The relevant skills and where each is used:

| Skill | Used for |
|---|---|
| `setup-matt-pocock-skills` | One-time repo setup (M0) |
| `grill-with-docs` | Resolve open questions one at a time before each milestone; record outcomes in `CONTEXT.md` and `docs/decisions/` |
| `to-prd` | One PRD per milestone, derived from this document |
| `to-issues` | Split each PRD into thin vertical-slice GitHub issues |
| `tdd` | Red-green-refactor for all platform-independent code: interceptor state machine, keystroke buffer, policy engine, detectors, extractors, IPC |
| `prototype` | Throwaway proofs before committing to a design: the GLiNER → ONNX → Rust spike (M0), the Windows drag-and-drop proof, Wayland input checks |
| `diagnose` | Platform bugs found in the testing phase (T1–T3) |
| `improve-codebase-architecture` / `zoom-out` | Architecture review after M4 and before R1 |
| `triage` | Incoming issues once the repository is public |
| `review` / `handoff` | Review against `main` before merging; handoff notes between long agent sessions |

The exact skill list is confirmed at install time, since the collection evolves.

**UI design: `nutlope/hallmark` (MIT)**

Install: `npx skills add nutlope/hallmark` (project-local). Requires Node.js 18+; browser checks and
screenshots also need `puppeteer-core` and Chrome.

- The setup wizard, tray menu content, settings, activity view, decision popup copy and the project
  website are designed with `/hallmark`; existing screens are checked with `/hallmark audit <path>`.
- One theme is chosen for the whole product and recorded (Hallmark writes its design decisions into the
  first CSS comment); the Svelte components implement that theme.
- Release gate for UI work: Hallmark's **floor** checks pass (contrast, focus states, reduced motion,
  no invented facts, mobile/narrow layouts) and `sloplint` is clean at all widths.
- Hallmark targets web pages; for the desktop app we use its themes, checks and review, not its
  photo library or conversion-page patterns.

---

## 16. Out of scope for v0.1

Central management and fleet policy, SIEM export, Windows minifilter, tamper protection,
redaction of files, fine-tuning UI, GPU/Neural Engine acceleration, MDM/GPO deployment packages.
