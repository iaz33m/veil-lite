# Veil

Veil stops sensitive data from leaving through the apps people type into. It catches the moment of
sending in any desktop app, checks the content on the device, and lets the user decide. This file is the
shared language for code, docs, issues and conversations; it is maintained with the `grill-with-docs`
skill.

## Language

### Interception

**Protected app**:
An app (and, for browsers, a site) whose outgoing input Veil inspects. Decided by the policy **scope**.
_Avoid_: monitored app, target (except "drop target"), watched app

**Scope**:
The policy rule that decides which apps are protected: `all_apps` minus an exclusion list (default), or
`listed_apps` plus listed sites.
_Avoid_: allowlist, coverage

**Channel**:
The way content leaves: *message* (Enter or Send click), *paste*, *drop*, *upload* (file picker),
*typing* (live warning only).
_Avoid_: vector, path, route

**Trigger**:
The single input that starts sending: an Enter key-down, a click on a **Send control**, a paste shortcut,
or the Open click in a file picker.
_Avoid_: action (reserved for policy actions), event

**Send control**:
A button whose accessible label matches the policy's send labels (`send`, `submit`, `generate`, …) and
that belongs to the protected app's own process.
_Avoid_: send button (fine in prose, but the rule includes the pid check)

**Hold**:
The temporary state in which Veil has swallowed a trigger and is deciding. Only one hold exists at a time.
Input typed into the same app during a hold is queued.
_Avoid_: block (that is a policy action), intercept, freeze

**Replay**:
Re-injecting swallowed input (the trigger, then anything queued) after the user or policy allows it.
Replays are tagged as Veil's own and never inspected again.
_Avoid_: resend, re-emit, forward

**Drop (verb, of input)**:
Discarding a held trigger: nothing is sent; typed text stays in the composer.
_Avoid_: cancel (reserved for the user's "Don't send" choice), discard

**Keystroke buffer**:
Veil's per-app mirror of what the user typed, used when an app does not expose its text.
_Avoid_: shadow text, keylog (never store or call it a log)

**Composer**:
The text field a message is typed into. Its text is **exposed** when the accessibility API returns it,
**not exposed** otherwise.

### Drag and drop

**Drag verdict**:
The result of checking a drag's files and text when the drag starts: *pending*, *clean* or *dirty*.

**Withhold**:
macOS drop strategy: while a dirty or pending drag hovers over a protected app, its data is hidden so the
drop lands empty; the data is restored afterwards.
_Avoid_: block the drop

**Swallow and cancel**:
Windows / X11 drop strategy: the button-up over the protected app is swallowed and the drag is cancelled
(Esc).

**Unblockable drop**:
A drop the platform cannot stop (Wayland). Veil warns and arms the **backstop**.

**Backstop**:
A second chance: after a risky drop that may have landed, the next send in that app (within 10 minutes)
asks the user to remove the attachment first.
_Avoid_: fallback, retry

**Allow drops for 30 s**:
The drop popup's override: drops into that app pass untouched for 30 seconds so the user can drop again
on purpose.

### Detection

**Layer**:
One stage of detection. **L1** keywords, **L2** personal-data recognizers, **L3** AI checks.
_Avoid_: tier, stage (in code)

**Keyword**:
A user word or phrase from the policy, matched case- and accent-insensitively (L1).

**Recognizer**:
An L2 pattern for one kind of personal data or secret (`EMAIL_ADDRESS`, `ES_DNI`), usually with a
checksum **validator**.
_Avoid_: regex, rule

**Check**:
One of the four L3 yes/no questions: *secrets*, *personal_info*, *confidential*, *source_code*. Each has a
threshold and an action in the policy.
_Avoid_: category (ambiguous), label, classifier

**Finding**:
One detection: what kind (keyword, PII entity or check), where (byte span, when known), score, and the
source ("message", a file name, "image"). A finding never carries the matched value outside memory.
_Avoid_: hit, match, alert

**Scan**:
Running layers over text. **Inspection** is the engine's whole look at something leaving: text, files and
images, plus what could not be checked.

**Extraction**:
Turning a file into text (or an image for OCR, or parts for archives). Results that can't be checked:
too large, encrypted, unreadable. Unsupported types are only checked by file name.

**Prescore**:
Running L3 in the background 400 ms after typing pauses, so the verdict is cached when the user sends.
_Avoid_: precompute, warm-up

**Model status**:
missing → downloading → ready (on disk, verified) → loaded (in memory) → back to ready after idle unload;
or failed / disabled.

### Decisions

**Action**:
What the policy says to do with findings: *allow*, *ask* (popup) or *block* (notice, no override). The
strictest action among findings wins.
_Avoid_: verdict (reserved for drags), decision (fine in prose)

**Prompt**:
The native popup that asks the user. Kinds: send, paste, upload, drop, backstop, block notice.
_Avoid_: dialog (in code), alert, modal

**Choice**:
The user's answer: *Send anyway*, *Don't send*, *Redact & send*, *Allow drops for 30 s*, *OK*.

**Redact & send**:
Replace each finding's span with a mask in the composer (or the pasted text), then replay. Text only,
and only when the text was exposed.
_Avoid_: sanitize, scrub

**Fail mode**:
What happens when content can't be checked in time or at all: *open* (let it through) or *closed* (ask).
Defaults: text open, files closed.

**Pause**:
User-chosen time (15 min / 1 h) during which nothing is held.
_Avoid_: disable, snooze

**Activity log**:
Metadata-only record of outcomes: time, app, site host, channel, detection types, outcome, file count.
Outcomes: allowed, sent_anyway, redacted, cancelled, blocked, drop_blocked, unchecked.
_Avoid_: audit log (implies tamper evidence), history

### Processes

**Engine**:
The headless `veil-engine` process: hooks, detection, prompts. Works without the app.

**App**:
The Tauri desktop app: tray, setup wizard, settings, activity view, model download. Talks to the engine
only over IPC.
_Avoid_: UI process, client (except in IPC code), agent

**Platform provider**:
The OS-specific implementation of the `Platform` trait, chosen at startup.
_Avoid_: driver, backend, adapter

## Relationships

- A **trigger** in a **protected app** starts a **hold**; the hold ends in a **replay** or a **drop**.
- An **inspection** runs **layers** and produces **findings**; the **policy** turns findings into an
  **action**; *ask* shows a **prompt**; the **choice** decides replay, drop or **redact & send**.
- A **drag** gets a **drag verdict** at start; at drop time the provider's strategy (**withhold**,
  **swallow and cancel**, or **unblockable**) decides how it is stopped; an unblockable or leaked drop
  arms the **backstop**.
- **Prescore** fills the L3 cache that the **message** channel reads at send time.
- Every prompt outcome and block becomes one **activity log** entry; clean text sends are not logged.

## Example dialogue

> **Dev:** "If the user pastes a screenshot into Slack, is that a **hold** on the **paste** channel?"
> **Maintainer:** "Yes. The paste shortcut is the **trigger**; the engine OCRs the image, and if a
> **check** crosses its threshold the **prompt** offers *Don't send* and *Send anyway*. No *Redact & send*:
> images are never modified."
> **Dev:** "And if OCR isn't available, like on Linux?"
> **Maintainer:** "Then the image produces no **findings**; it is not an unchecked item, so the files **fail
> mode** doesn't apply. That's a documented limitation, not a fallback."

## Flagged ambiguities

- "Block" was used for both the temporary **hold** and the policy **action** *block* — resolved: *hold*
  is temporary; *block* means no override.
- "Cancel" meant both the user's *Don't send* and cancelling a drag — resolved: the user **cancels**
  (outcome `cancelled`); the provider **cancels the drag** only in the *swallow and cancel* strategy.
- "Verdict" vs "decision" — resolved: **drag verdict** for drags only; policy output is an **action**.
- "Category" was used for both L2 entity types and L3 checks — resolved: L2 produces **recognizer**
  entities, L3 answers **checks**.
