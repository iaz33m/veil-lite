# Veil

**On-device data-loss prevention for AI chats and every app you type into.**

Veil sits between you and the apps you send things through: ChatGPT, Claude, Gemini, Slack, Teams, a
browser tab, an Electron app, anything. At the moment something is about to leave (you press Enter,
click Send, paste, drop a file, pick a file to upload), Veil holds it, checks it on your machine, and only
lets it through if it's clean or you say so.

- Open source, Apache-2.0.
- Runs entirely on the device. No account, no cloud, no telemetry.
- Rust engine, Tauri v2 desktop app, macOS / Windows / Linux.

> **Status: pre-release.** Phase A (code for all three platforms) is complete. Phase B (testing and
> hardening on each platform, then signed releases) has not started. Expect rough edges and do not rely on
> Veil as your only control yet. See [docs/LIMITATIONS.md](docs/LIMITATIONS.md).

---

## How it works

**Hold first, inspect second, then replay or drop.**

1. The platform provider hooks keyboard and mouse input before any app sees it.
2. When you send (Enter, a Send-type button), paste, drop or upload in a protected app, Veil **holds**
   that input. Anything you type meanwhile is queued, not lost.
3. The engine reads what is about to leave (the composer text through the accessibility API, or Veil's
   own keystroke buffer when the app exposes nothing; the clipboard; the dragged or picked files) and runs
   the detection layers.
4. Clean → the held input is **replayed** exactly as you typed it. Sensitive → a native popup asks what to
   do. The replayed input is tagged so Veil never inspects it twice.

### Detection layers

| Layer | What it finds | Cost |
|---|---|---|
| **L1 keywords** | Your words and phrases (`confidential`, `project falcon`), case- and accent-insensitive | microseconds |
| **L2 personal data** | Emails, payment cards, IBANs, phone numbers, IPs, API keys and secrets, national IDs for the US, UK, EU (ES, DE, FR, IT, NL, VAT) and Asia-Pacific (IN, PK, AU, SG), with checksum validators. Built on [`redact-core`](https://crates.io/crates/redact-core) plus Veil's own recognizers | < 1 ms per KB |
| **L3 AI checks** | Four yes/no questions answered by [GLiNER2.5-multi-Decide](https://huggingface.co/fastino) (287M, multilingual, int8 ONNX, CPU only): *secrets & credentials*, *personal information*, *confidential business information*, *source code* | ~150–300 ms per 512-token chunk; pre-scored in the background while you type |

Files are opened and their text extracted (txt, csv, json, code, docx, xlsx, pptx, odt, ods, odp, pdf, zip
contents). Images are read with the OS's own OCR (Apple Vision on macOS, `Windows.Media.Ocr` on Windows).

### What you see

When something is found, a native popup names *what kind* of data it is (never the value) and offers:

| Button | Effect |
|---|---|
| **Don't send** (default) | The input is dropped; your text stays in the box so you can edit it |
| **Send anyway** | The held input is replayed |
| **Redact & send** | Each finding is replaced with a mask (`[EMAIL_ADDRESS]`, `****1111`, `[REDACTED]`) in the composer, then sent. Text only; offered when Veil could read the composer |

The policy can turn any category into **Block** (a notice with no override). For drops, the popup offers
**Allow drops for 30 s** so you can drop again deliberately. From the tray you can **pause protection** for
15 minutes or an hour.

While you type, a small non-activating banner warns as soon as the message contains something sensitive,
so the popup at send time is never a surprise.

### Channels

| Channel | How it is caught |
|---|---|
| Typing | Live warning banner (L1–L2 on every keystroke, L3 after a 400 ms pause) |
| Enter / Send click | Held and checked before the app receives it |
| Paste (Cmd/Ctrl+V, Shift+Insert) | Held; clipboard text, files and images are checked |
| Drag and drop | Files are scanned when the drag starts; the drop is withheld or cancelled over a protected app |
| File picker | The Open / Upload click (or double-click) is held while the selected files are checked |
| Images | OCR text goes through the same layers (macOS and Windows) |

## Platform support

| | macOS 12+ | Windows 10/11 | Linux X11 | Linux Wayland |
|---|---|---|---|---|
| Enter / Send / paste | ✓ | ✓ | ✓ | ✓ |
| Composer text via accessibility | AX | UI Automation | AT-SPI | AT-SPI |
| Block drag-and-drop | ✓ (data withheld) | ✓ (drop cancelled) | ✓ (drop cancelled) | ✗ warns and asks again on the next send |
| File picker uploads | ✓ | ✓ | ✓ | ✓ (where the dialog exposes AT-SPI) |
| Images (OCR) | ✓ Vision | ✓ needs an OCR language pack | ✗ no OS-native OCR | ✗ |
| Permissions needed | Accessibility | none | `veil` group + accessibility bus | `veil` group + accessibility bus |

Known gaps, honestly stated: elevated (admin) windows on Windows are not visible to a normal process until
a signed `uiAccess` build exists; apps that expose neither text nor keystrokes (remote desktops, canvas
editors) can't be read; context-menu paste and input injected by other software bypass the hook. The full
list is in [docs/LIMITATIONS.md](docs/LIMITATIONS.md).

## Privacy

- Content is checked in memory and then forgotten. It is never written to disk, logged, or sent anywhere.
- The **activity log** is metadata only: time, app, site host, channel, detection *types*, outcome and
  file count. Never message text, matched values, file names, paths or full URLs.
- The only network traffic: the AI model download (after you approve it in the setup wizard, from a pinned
  URL, verified against a pinned SHA-256) and update checks. No telemetry, no analytics, no crash upload.

Details: [docs/PRIVACY.md](docs/PRIVACY.md).

## Install

Signed installers will be published with v0.1 (DMG for macOS, MSI/NSIS for Windows, deb/rpm/AppImage for
Linux). Until then, build from source.

### Build from source

Prerequisites:

- Rust **1.92+** (`rustup update stable`)
- Node.js **22** and npm
- Tauri v2 prerequisites for your OS: <https://v2.tauri.app/start/prerequisites/>
  (Xcode command-line tools on macOS; WebView2 + MSVC build tools on Windows;
  `libwebkit2gtk-4.1-dev`, `libayatana-appindicator3-dev`, `librsvg2-dev` on Debian/Ubuntu)
- Linux engine: `libudev-dev`, `libdbus-1-dev`, `pkg-config`

```bash
# engine and libraries
cargo build --release -p veil-engine
cargo test --workspace

# try the detectors on a file without installing anything
cargo run -p veil-engine -- check tests/fixtures/message-sensitive.txt

# desktop app (bundles the engine as a sidecar)
./scripts/prepare-sidecar.sh          # builds veil-engine and copies it to apps/desktop/src-tauri/binaries/
cd apps/desktop
npm install
npm run tauri dev
```

On first launch the setup wizard walks you through the permissions your OS needs and then offers the AI
model download. Layers 1–2 protect from the first second; the AI checks switch on when the download is
verified.

More: [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md).

## Configuration

Everything lives in one YAML policy, edited through Settings or by hand (hot-reloaded):

| OS | Policy file |
|---|---|
| macOS | `~/Library/Application Support/Veil/policy.yaml` |
| Windows | `%APPDATA%\Veil\policy.yaml` |
| Linux | `$XDG_CONFIG_HOME/veil/policy.yaml` (usually `~/.config/veil/policy.yaml`) |

```yaml
version: 1
scope: { mode: all_apps }          # minus a built-in exclusion list (terminals, file managers, password managers…)
keywords: [confidential, "project falcon"]
pii: { enabled: true, regions: [universal, us, uk, eu, apac], min_score: 0.5 }
checks:                            # AI checks: action ask (popup) or block (no override)
  secrets:       { threshold: 0.60, action: ask }
  personal_info: { threshold: 0.70, action: ask }
  confidential:  { threshold: 0.70, action: ask }
  source_code:   { threshold: 0.75, action: ask }
actions: { default: ask, allow_redact: true, fail_mode: { text: open, files: closed } }
pause: { allowed: true }
activity_log: { enabled: true }
```

## Repository layout

```
crates/
  veil-core/              platform contract, interceptor (hold/queue/replay), engine, policy, redaction, activity log
  veil-detect/            L1 keywords, L2 personal-data recognizers, file text extraction, L3 AI checks (ort + tokenizers), model download
  veil-ipc/               protocol and local-socket transport between app and engine
  veil-engine/            the headless background process
  veil-platform-macos/    CGEventTap, Accessibility, NSPasteboard, Vision, native alerts
  veil-platform-windows/  low-level hooks, SendInput, UI Automation, OLE drag, Windows.Media.Ocr, TaskDialog
  veil-platform-linux/    evdev grab + uinput replay, AT-SPI, X11 drag, notifications
apps/desktop/             Tauri v2 app: tray, setup wizard, settings, activity, model download
tools/model-export/       GLiNER2.5-multi-Decide → ONNX int8 export and parity test
tests/harness/            local fake chat page for safe end-to-end testing
tests/fixtures/           sample files (fake data only) and expected detections
docs/                     architecture, threat model, limitations, privacy, development
```

Start with [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) and [CONTEXT.md](CONTEXT.md) (the project's
vocabulary).

## Contributing

Bug reports, false positives/negatives (with *fake* data only), recognizers for more countries and
platform fixes are all welcome. Read [CONTRIBUTING.md](CONTRIBUTING.md). Security issues: see
[SECURITY.md](SECURITY.md), please don't open public issues for them.

## License

[Apache License 2.0](LICENSE). Third-party notices in [NOTICE](NOTICE).
