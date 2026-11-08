# Test harness

`test-chat.html` is a fake chat app for testing Veil safely. Open it straight from disk (`file://`) in any
browser, or load it in an Electron/Chromium app. It has no network access (a strict Content Security
Policy blocks every request) and stores nothing.

What it offers:

| Area | Exercises |
|---|---|
| Textarea composer + **Send** button (`aria-label="Send message"`) | Enter, Shift+Enter, Send click, Redact & send through the accessibility API |
| Rich-text composer (`contenteditable`) | Apps like Slack/ChatGPT whose text is exposed differently |
| Icon-only send button (accessible name "Send prompt", no visible text) | Send-control detection by accessible name |
| "Send with Ctrl/⌘+Enter only" toggle | Modifier-Enter sends |
| File input | File-picker uploads (Open / double-click held) |
| Drop zone | Drag-and-drop of files and dragged text; an empty drop is logged as "data was withheld" |
| Paste area | Pasted text, copied files, screenshots (OCR) |

The right-hand **Arrived** panel shows exactly what the page received, newest first. With Veil running,
sensitive content must only appear there after you choose *Send anyway* (or masked, after *Redact &
send*).

Use it with the fake-data files in [`../fixtures`](../fixtures/README.md) and the checklists in
[`../MANUAL_TEST_PLAN.md`](../MANUAL_TEST_PLAN.md).
