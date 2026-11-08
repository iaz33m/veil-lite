# Security policy

## Reporting a vulnerability

Please **do not open a public issue** for security problems.

Report privately through GitHub's **"Report a vulnerability"** button on the repository's Security tab
(private vulnerability reporting). If that is unavailable, email the maintainers at the address listed in
the repository profile with the subject `VEIL SECURITY`.

Include: affected version or commit, OS and version, the app involved, steps to reproduce, and impact.
Never include real secrets or personal data; use the fake values in `tests/fixtures/`.

What to expect:

| | Target |
|---|---|
| Acknowledgement | 3 working days |
| First assessment | 10 working days |
| Fix or mitigation for confirmed high-severity issues | 30 days, coordinated disclosure afterwards |

We credit reporters in the release notes unless you ask us not to.

## Supported versions

Veil is pre-release. Only the latest commit on `main` (and, after v0.1, the latest release) receives
security fixes.

## Scope

In scope:

- Content leaving the device, being written to disk or logged by Veil (message text, matched values, file
  contents, file paths, full URLs).
- The engine IPC accepting connections or commands from other users, or without the session token.
- Model or update downloads being accepted without hash/signature verification.
- Privilege problems in the Linux packaging (udev rule, `veil` group) beyond what is documented.
- Crashes or hangs of the engine that freeze user input (the hook must never block).
- A way to make Veil replay input the user did not type.

Out of scope (documented limitations, see [docs/LIMITATIONS.md](docs/LIMITATIONS.md)): bypasses that
need the user to act deliberately (Send anyway, Pause, editing the policy, quitting Veil), malware running
as the same user, and channels listed as not covered.

## What Veil is, and is not

Veil is a **guard rail for well-meaning people**: it catches the accidental paste of a customer list into
a chatbot, the API key left in a code snippet, the confidential deck dropped into the wrong window.

Veil is **not**:

- **a malware barrier.** Anything running as the same user can quit it, edit its policy, inject input
  through accessibility APIs, or read the clipboard itself.
- **tamper-proof.** v0.1 has no tamper protection, no admin-enforced policy and no central management.
- **mandatory.** Every popup can be overridden by design (except categories the policy sets to Block),
  and protection can be paused from the tray. Overrides are recorded in the metadata-only activity log.
- **a network DLP.** It watches the moment of input in desktop apps, not network traffic, cloud sync,
  email clients sending in the background, or command-line uploads.

## Threat model summary

Assets: the content being sent, the user's trust that Veil itself leaks nothing, and the integrity of the
user's input stream.

| Threat | Mitigation |
|---|---|
| Veil leaks what it inspects | Content is processed in memory only; activity log holds detection types only; no telemetry; IPC never carries content |
| Another local user drives the engine | Socket in a `0700` directory, mode `0600` (Unix); per-user named pipe (Windows); per-boot random token required on every connection |
| Tampered model or update | Model files pinned by SHA-256 compiled into the release; Tauri updater manifests are signed |
| Engine bug freezes typing | Hook thread never scans or blocks; holds expire after 4 s without a prompt; text fails open by default |
| Replay injects unintended input | Only input Veil itself swallowed is replayed, in original order; replays are tagged and never re-enter the hook |
| Malicious file crashes the extractor | Size caps (25 MB), archive depth and entry limits, extraction in the worker thread, failures treated per fail mode |

The full model is in [docs/THREAT_MODEL.md](docs/THREAT_MODEL.md).
