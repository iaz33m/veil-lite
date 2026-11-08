# Known limitations (v0.1)

Veil is honest about what it can't see. Each item says what happens and whether it is planned.

> v0.1 has not yet been through its platform testing phase (Phase B). Items marked **(untested)** are
> written but not yet verified on real machines.

## Everywhere

| Limitation | What happens | Plan |
|---|---|---|
| Context-menu paste (right-click → Paste), Edit → Paste menu, drag of text from another app inside the same window | Not a shortcut, so the paste is not held. Text pasted this way still reaches the keystroke buffer only if it was typed; the send-time check still reads exposed composer text | Menu paste via accessibility notifications later |
| Input injected by other software (accessibility APIs, automation tools, remote control) | Not seen by the hook | Out of scope |
| Apps exposing neither composer text nor real keystrokes (remote desktops, VMs, games, canvas editors, some terminals) | Nothing to read; the keystroke buffer is empty or wrong | Out of scope |
| Apps that expose no text to accessibility | The keystroke buffer is used. It follows typing, Backspace, word delete and select-all, but not cursor moves, undo, autocomplete or spell-correction. *Redact & send* is not offered | Better buffer heuristics |
| Sending with a key other than Enter / Ctrl/Cmd+Enter (custom shortcuts) or a button with an unrecognized label | Not held at send time; the typing banner still warns | Add labels/shortcuts in policy |
| Unlabelled buttons in web pages | Clicks on unlabelled elements are held only in browsers and only while the typed text already looks sensitive | — |
| Long content | AI checks read at most 32 chunks of 512 tokens per file; layers 1–2 read everything up to the 25 MB file cap | Configurable caps |
| Files over 25 MB, encrypted or unreadable | Reported as "couldn't be checked"; files fail closed (ask) by default | — |
| Unsupported file types (video, audio, binaries) | Only the file name is checked | — |
| Images in documents | Images embedded in docx/pdf are not OCR'd in v0.1 (image-only PDF pages may be) | Later |
| AI checks before the model is downloaded, or if it fails to load | Layers 1–2 only; the tray says so | — |
| Slow machines | If the AI check doesn't finish in 1.5 s at send time, text fails open (sends) unless the policy says closed | Prescore usually hides this |
| A file dropped while its check is still running | Stopped; if it turns out clean, drops into that app are allowed for 30 s and Veil asks you to drop again | — |
| Typing during a prompt into another app | Passes immediately (only the held app's input is queued) | By design |
| Switching windows while a prompt is open | Veil refocuses the original app before replaying | — |

## macOS (untested)

| Limitation | What happens |
|---|---|
| Accessibility permission required | Without it no hooks are installed; the wizard and tray say so |
| Secure input (password fields, some terminals, apps that enable `EnableSecureEventInput`) | macOS hides keystrokes from all event taps; nothing is held while secure input is on |
| Some Electron/Chromium apps need their accessibility tree switched on | Veil sets `AXManualAccessibility` / `AXEnhancedUserInterface`; a few apps ignore it and fall back to the keystroke buffer |
| Drops are stopped by withholding the drag data | Some apps read drag data before the drop; when Veil can't be sure the data was withheld it arms the backstop for the next send |
| OCR languages | Whatever Apple Vision supports on that macOS version |
| Apps in other Spaces / full-screen | Supported, but popups appear on the active Space |

## Windows (untested)

| Limitation | What happens |
|---|---|
| Elevated (Run as administrator) windows | A normal process can't hook or read them (UIPI). Nothing is held in elevated apps until a signed `uiAccess` build ships |
| UWP / WinUI apps with sandboxed input | Composer text via UI Automation where exposed; otherwise keystroke buffer |
| Drag-and-drop | The button-up over a protected app is swallowed and the drag cancelled with Esc. Drops started from apps that don't use OLE drag-and-drop may behave differently |
| Files being dragged from apps other than Explorer | Read from the OLE data object when possible; otherwise only Explorer selections are known |
| OCR | `Windows.Media.Ocr` needs an OCR language pack for the text's language (Settings → Time & language → Language → Optional features). Without one, images are not checked |
| Screen-reader flag | If an app only builds its UI Automation tree for screen readers, Veil may set `SPI_SETSCREENREADER` and restores it on exit |

## Linux (untested)

| Limitation | What happens |
|---|---|
| Permissions | The engine needs read access to `/dev/input/event*` and write access to `/dev/uinput`: the packages add a udev rule and a `veil` group; you must log out and back in after being added. AppImage users run the documented commands themselves |
| Input grab | Veil grabs keyboards and mice exclusively and re-emits events through a virtual device. Devices hot-plugged later are picked up; exotic input devices (tablets, game controllers) are left alone |
| Keyboard layouts | Characters are derived from the active XKB layout; dead keys and IME (Chinese/Japanese/Korean input) are not mirrored in the keystroke buffer. Exposed composer text (AT-SPI) is unaffected |
| AT-SPI | Apps must expose AT-SPI (GTK, Qt, Chromium/Electron with accessibility enabled). Veil turns on the session's accessibility flag; others fall back to the keystroke buffer |
| **Wayland: drag-and-drop** | Drops can't be seen or stopped. Veil can't read the dragged files; after a drop into a protected app it can't verify, the next send asks again (backstop) only when it knows a drop happened |
| Wayland: window under the pointer | Compositors don't reveal which window is under the pointer; Veil uses AT-SPI and the focused app instead, which can be wrong for clicks into unfocused windows |
| **Images** | Linux has no OS-native OCR, so pasted, dropped and uploaded images are not checked (no bundled fallback, by decision) |
| Popups | Shown with the desktop's dialog tool (zenity/kdialog) or a notification; appearance varies by desktop |
| Clipboard on Wayland | Needs a compositor with the data-control protocol (wlroots, KDE); GNOME Wayland falls back to the X11 clipboard via XWayland apps |

## Not in v0.1

Central management, admin-enforced policy, tamper protection, SIEM export, redaction of files,
GPU/Neural Engine acceleration, MDM/GPO packages, network-level DLP.
