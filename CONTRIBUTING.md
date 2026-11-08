# Contributing to Veil

Thanks for helping. Veil handles the most sensitive moment of someone's day (what they are about to
send), so the bar is: correct, small, tested, and never leaking content.

## Ground rules

- **Never paste real secrets or personal data** into issues, PRs, tests or fixtures. Use the fake values in
  [`tests/fixtures/`](tests/fixtures/README.md) (test card `4111 1111 1111 1111`, `example.com` addresses,
  `AKIAIOSFODNN7EXAMPLE`, checksum-valid but invented national IDs).
- Read [`CONTEXT.md`](CONTEXT.md) first and use its words (hold, replay, channel, finding, backstop…) in
  code, docs and issues.
- Changes to the decisions table in [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) need a decision record:
  `docs/decisions/NNNN-short-title.md` (context, decision, consequences).
- Be kind: [Code of Conduct](CODE_OF_CONDUCT.md).

## Developer Certificate of Origin

Every commit must be signed off (DCO 1.1, <https://developercertificate.org/>):

```bash
git commit -s -m "veil-detect: add PT NIF recognizer"
```

This adds `Signed-off-by: Your Name <you@example.com>`, certifying you wrote the change or have the right
to submit it under Apache-2.0.

## Setup

See [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md). Short version:

```bash
rustup update stable                     # Rust 1.92+
cargo test --workspace
cd apps/desktop && npm install && npm run check
```

## Before you open a PR

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
# if you touched the UI
cd apps/desktop && npm run check && npm run build
```

PR checklist:

- [ ] One logical change; the title says which crate or area (`veil-core: …`, `desktop: …`).
- [ ] Tests: new behavior has a failing-first test (red → green → refactor). Platform-independent logic is
      tested with the mock platform/scanner from `veil-core`'s `testing` feature.
- [ ] No content can reach logs, the activity log, IPC or disk (check every new `tracing::` call and every
      new `ActivityEntry` field).
- [ ] The hook path stays non-blocking (no scanning, file I/O, IPC or locks held across platform calls on
      the hook thread).
- [ ] Docs updated: `CONTEXT.md` for new terms, `docs/LIMITATIONS.md` for new known gaps, README tables if
      platform support changed.
- [ ] Commits signed off.

## Adding a personal-data recognizer

Recognizers live in `crates/veil-detect` (layer 2), grouped by region (`universal`, `us`, `uk`, `eu`,
`apac`).

1. Prefer a pattern **plus a validator** (checksum, mod-11/23/97, Luhn, Verhoeff, date sanity). A bare
   regex for "9 digits" produces false positives on every invoice number; without a checksum, require
   context words nearby and use a lower score.
2. Use an upper-snake entity name with the country prefix (`PT_NIF`, `BR_CPF`) and add a human phrase for
   popups in `veil-core/src/findings.rs` (`describe_pii`), e.g. "a Portuguese tax number (NIF)".
3. Tests: at least three valid examples (invented, checksum-valid), three near misses (wrong check digit,
   wrong length, embedded in a longer number) and one multilingual context sentence.
4. Add a fixture line to `tests/fixtures/make_fixtures.py` and its expectation to
   `tests/fixtures/expected.json`, then regenerate the fixtures.
5. If you adapt a pattern from another project, keep its license notice and add it to [`NOTICE`](NOTICE).
   We accept MIT, Apache-2.0, BSD and similar; not GPL.

## Adding or changing a platform provider

Providers implement `veil_core::Platform` (`crates/veil-core/src/platform.rs`). Read its module docs:

- `run` owns the main thread and delivers every input event to the `InputHandler` before any app sees it.
- Replays are tagged and **must not** come back through the handler.
- `foreground`, `app_at` and `element_at` are called from the hook: cache, never block.
- Pick the right `DropStrategy` and be honest about it: if drops can't be stopped, return `Unsupported`
  and Veil arms the backstop instead.
- Gate the whole crate with `#![cfg(target_os = "…")]` so the workspace builds everywhere, and export
  `pub fn create() -> anyhow::Result<Arc<dyn Platform>>`.
- Document permissions in `permissions()` with one or two sentences the setup wizard can show verbatim.
- Add a section to [`tests/MANUAL_TEST_PLAN.md`](tests/MANUAL_TEST_PLAN.md) and to
  [`docs/LIMITATIONS.md`](docs/LIMITATIONS.md).

## Reporting false positives and negatives

Use the "Detection report" issue template. Describe the *shape* of the data (e.g. "Spanish NIE written
with dots: X.1234567.L") and paste a **fake** example with the same shape. Issues containing real data
will be deleted.

## UI changes

Screens are designed with the Hallmark skill (`nutlope/hallmark`). Run `/hallmark audit` on changed screens
and include before/after screenshots at narrow and wide widths in the PR.

## License

By contributing you agree your contributions are licensed under the [Apache License 2.0](LICENSE).
