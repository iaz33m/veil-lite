# Privacy

Veil reads the most private thing on your computer: what you are about to send. This page lists exactly
what it keeps, where, and what (little) touches the network.

## The promise

1. **Content stays in memory.** Message text, clipboard contents, file contents and OCR text are checked
   in memory and dropped. They are never written to disk, logged, cached on disk or sent anywhere.
2. **No telemetry.** No analytics, no crash upload, no usage pings, no account.
3. **Network only when you say so:** the AI model download (after you approve it) and update checks.

## What is stored, and where

All files belong to your user account. `VEIL_HOME` relocates everything to one folder (portable use,
tests).

| File | macOS | Windows | Linux | Contents |
|---|---|---|---|---|
| `policy.yaml` | `~/Library/Application Support/Veil/` | `%APPDATA%\Veil\` | `$XDG_CONFIG_HOME/veil/` (`~/.config/veil/`) | Your settings: scope, exclusions, keywords, regions, thresholds, actions. **Your keywords are stored here in plain text**, since they are what Veil looks for |
| `activity.jsonl`, `activity.1.jsonl` | `~/Library/Application Support/Veil/` | `%LOCALAPPDATA%\Veil\` | `$XDG_DATA_HOME/veil/` (`~/.local/share/veil/`) | Metadata-only activity log (below). Rotated at 5 MB; one old file kept |
| `models/` | same as above | `%LOCALAPPDATA%\Veil\models\` | `~/.local/share/veil/models/` | The downloaded AI model (ONNX file, tokenizer, manifest). Public data, no personal content |
| `engine.log` | same as above | `%LOCALAPPDATA%\Veil\` | `~/.local/share/veil/` | Diagnostics: startup, permissions, errors, timings. Never content, values, file names or URLs |
| `engine.token` | same folder | `%LOCALAPPDATA%\Veil\` | `$XDG_RUNTIME_DIR/veil/` | Random session token for the app ↔ engine connection, rewritten every engine start, readable only by you |
| `engine.sock` | same folder | — (named pipe `\\.\pipe\veil-engine-<user>`) | `$XDG_RUNTIME_DIR/veil/` | Local socket; nothing stored |

### Activity log fields

One JSON line per event that involved a prompt, a block, or a file check:

| Field | Example | Notes |
|---|---|---|
| `ts_ms` | `1762588800000` | Time (Unix ms) |
| `app_id` | `com.tinyspeck.slackmacgap`, `slack.exe` | Bundle id / executable name |
| `app_name` | `Slack` | |
| `site` | `chatgpt.com` | **Host only**, for browsers. Never the path, query or full URL |
| `channel` | `message`, `paste`, `drop`, `upload` | |
| `detections` | `["email_address", "keyword", "ai:secrets"]` | Detection **types**. Never the matched value, never which keyword |
| `outcome` | `cancelled`, `sent_anyway`, `redacted`, `blocked`, `drop_blocked`, `allowed`, `unchecked` | |
| `files` | `1` | A count. Never file names or paths |

Not stored: message text, matched values, keyword text, file names, file paths, window titles, full URLs,
clipboard contents, screenshots. Clean messages sent without a prompt are not logged at all.

You can turn the log off (`activity_log.enabled: false`) or clear it from the Activity screen.

### What prompts show

Prompts appear on your screen only. They name the *kind* of data ("a payment card number", "the keyword
“confidential”") and, for files, the file name. They never show the matched value.

## Network

| When | Where to | What is sent | Verification |
|---|---|---|---|
| You approve the model download in the setup wizard (or later in Settings) | The pinned model URL (Hugging Face) compiled into the release | A normal HTTPS GET (with a `Range` header when resuming). No identifiers beyond what any HTTPS client sends | Every file's SHA-256 is pinned in the release; files are verified before use |
| Update checks (if enabled) | The release manifest URL | HTTPS GET | Signed manifests (Tauri updater) |

Nothing else. The engine has no other network code; the IPC socket is local only.

## Memory

- The L3 verdict cache holds SHA-256 hashes of checked text and the resulting check scores, in memory
  only, at most 256 entries; it is cleared when the policy changes and when the engine exits.
- The keystroke buffer holds up to 4,000 characters per app, in memory only, and is cleared after each
  send.

## Removing Veil's data

Quit Veil, then delete the folders in the table above. Uninstallers remove the app; your policy and
activity log are left in place in case you reinstall, so delete them yourself if you want a clean slate.
