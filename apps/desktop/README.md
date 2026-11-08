# Veil desktop app

Tauri v2 app (Svelte 5 + TypeScript front end, Rust back end) around the headless `veil-engine`. It holds no
detection logic: every action goes to the engine over local IPC (`crates/veil-ipc`).

- **Tray / menu bar:** status line, Pause for 15 minutes, Pause for 1 hour, Resume, Open Veil…, Quit Veil.
- **Setup wizard:** Welcome → Permissions → AI model (optional, user-approved download) → Done (start at login).
- **Window:** Status, Activity (metadata only), Settings (structured editor over `policy.yaml` plus the raw file),
  About. Closing the window hides it; Veil keeps running in the tray.
- **Engine sidecar:** `src-tauri/binaries/veil-engine-<target-triple>`, started when no engine answers, restarted
  with backoff (1 s → 30 s) if it exits, stopped on Quit. An engine you started yourself is used as it is.

## Develop

```sh
cd apps/desktop
npm install
npm run tauri dev          # builds veil-engine, copies it into src-tauri/binaries/, starts Vite and the app
```

Other scripts:

| Command | What it does |
|---|---|
| `npm run check` | svelte-check (types, a11y), warnings fail |
| `npm run build` | front end into `dist/` |
| `npm run sidecar -- --build [--release] [--target <triple>]` | build `veil-engine` and copy it next to the app |
| `npm run tauri build` | release bundles (dmg / msi + nsis / deb, rpm, AppImage) |
| `npm run icons` | regenerate icons from `src-tauri/icons/source/app-icon.svg` |

The tray uses `icons/tray@2x.png` (a monochrome template image) on macOS and `icons/32x32.png` elsewhere; its
source is `icons/source/tray-icon.svg`.

## Security

- The web view can only call the app's own commands, listed in `src-tauri/build.rs` and granted in
  `src-tauri/capabilities/default.json`, plus listening to events. No shell, file system, HTTP or opener access
  from JavaScript.
- `open_link` opens one of four fixed GitHub pages; the web view can't pass a URL.
- Strict CSP: no remote scripts, styles, fonts or connections.

## Updates

`tauri-plugin-updater` is behind the `updater` Cargo feature and off by default. To turn it on: generate a signing
key (`npm run tauri signer generate`), put the public key in `tauri.conf.json` → `plugins.updater.pubkey`, set
`bundle.createUpdaterArtifacts` to `true`, and build with `--features updater`.
