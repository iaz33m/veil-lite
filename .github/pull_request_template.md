## What and why

<!-- One logical change. Link the issue. -->

## How it was tested

<!-- cargo test, flow tests added, manual test plan items (OS/app), screenshots for UI. -->

## Checklist

- [ ] `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`
- [ ] New behavior has tests (mock platform/scanner for core logic)
- [ ] No content can reach logs, the activity log, IPC or disk
- [ ] Hook path stays non-blocking; no platform calls under the state lock
- [ ] Docs updated (CONTEXT.md terms, LIMITATIONS.md, README tables) where relevant
- [ ] No real secrets or personal data anywhere in the diff
- [ ] Commits signed off (`git commit -s`)
