<script lang="ts">
  import { api, errorText } from "../lib/api";
  import { engine } from "../lib/engine.svelte";
  import { platformLabel } from "../lib/format";
  import { router } from "../lib/router.svelte";

  let { version }: { version: string } = $props();

  let checking = $state(false);
  let updateNote = $state<string | null>(null);
  let updateError = $state<string | null>(null);

  async function checkUpdate() {
    checking = true;
    updateNote = null;
    updateError = null;
    try {
      const newer = await api.checkForUpdate();
      updateNote = newer ? `Version ${newer} is available.` : "You have the latest version.";
    } catch (e) {
      updateError = errorText(e);
    } finally {
      checking = false;
    }
  }

  const links = [
    { id: "repository", label: "Source code and releases" },
    { id: "privacy", label: "What Veil stores, and where" },
    { id: "security", label: "Report a security problem" },
    { id: "license", label: "Apache License 2.0" },
  ] as const;
</script>

<div class="page">
  <header class="page-head">
    <h1>About Veil</h1>
    <p>Open-source protection that checks what you send before it leaves your computer.</p>
  </header>

  <section class="section" aria-labelledby="a-version">
    <h2 id="a-version">This copy</h2>
    <dl class="facts">
      <dt>App</dt>
      <dd class="mono">{version || "unknown"}</dd>
      <dt>Engine</dt>
      <dd class="mono">{engine.status?.engine_version ?? (engine.error ? "not running" : "checking…")}</dd>
      <dt>System</dt>
      <dd>{engine.status ? platformLabel(engine.status.platform) : "unknown"}</dd>
      <dt>Text in images</dt>
      <dd>{engine.status ? (engine.status.ocr_available ? "Checked with the system’s text recognition" : "Not checked on this system") : "unknown"}</dd>
    </dl>
    <div class="row">
      <button type="button" class="btn" onclick={checkUpdate} disabled={checking} aria-busy={checking}>
        {checking ? "Checking…" : "Check for updates"}
      </button>
      <a class="btn btn--text" href={router.href("setup", "welcome")}>Run setup again</a>
    </div>
    <p class="helper" aria-live="polite">{updateNote ?? ""}</p>
    <p class="error-text" aria-live="polite">{updateError ?? ""}</p>
  </section>

  <section class="section" aria-labelledby="a-privacy">
    <h2 id="a-privacy">Privacy</h2>
    <p>
      Everything Veil checks stays on this computer. It has no account and sends no telemetry. The only download it ever
      makes is the optional AI model, when you ask for it, from a pinned address with a checked fingerprint. The activity
      log records the app, the kind of thing found and what happened: never the text, the file or the full web address.
    </p>
  </section>

  <section class="section" aria-labelledby="a-links">
    <h2 id="a-links">Links</h2>
    <ul class="links">
      {#each links as l (l.id)}
        <li><button type="button" class="btn btn--text" onclick={() => void api.openLink(l.id)}>{l.label}</button></li>
      {/each}
    </ul>
    <p class="helper">Veil is licensed under Apache-2.0. It builds on redact-core, ONNX Runtime, Tauri and the GLiNER2.5 model.</p>
  </section>
</div>

<style>
  .links { list-style: none; margin: 0; padding: 0; display: grid; gap: var(--space-2xs); justify-items: start; }
  .links .btn { padding-inline: 0; }
</style>
