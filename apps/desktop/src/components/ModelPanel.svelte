<script lang="ts">
  import { api, errorText } from "../lib/api";
  import { engine } from "../lib/engine.svelte";
  import { modelSummary } from "../lib/format";
  import ConfirmDialog from "./ConfirmDialog.svelte";

  /** `setup` shows the full explanation and leaves the choice to the user; `settings` is compact. */
  let { variant = "settings" }: { variant?: "setup" | "settings" } = $props();

  let busy = $state(false);
  let failed = $state<string | null>(null);
  let confirmRemove = $state(false);

  const model = $derived(engine.model);
  const downloading = $derived(model?.state === "downloading");
  const installed = $derived(model?.state === "ready" || model?.state === "loaded");
  const pausedAt = $derived(!downloading && !installed && engine.downloadedPercent !== null ? engine.downloadedPercent : null);
  const percent = $derived(model?.state === "downloading" ? model.percent : (pausedAt ?? 0));

  async function run(fn: () => Promise<void>) {
    busy = true;
    failed = null;
    try {
      await fn();
      await engine.refresh();
    } catch (e) {
      failed = errorText(e);
    } finally {
      busy = false;
    }
  }

  const download = () => run(() => api.downloadModel());
  const pause = () => run(() => api.cancelDownload());
  const remove = () => run(() => api.removeModel());
</script>

<div class="model" aria-busy={busy}>
  {#if variant === "setup"}
    <ul class="facts-list">
      <li>About 300 MB, downloaded once from Veil’s model release and checked against a pinned fingerprint.</li>
      <li>Runs on your processor only. Nothing you type is sent anywhere.</li>
      <li>Optional. Without it, keyword and personal-data pattern checks still protect you.</li>
    </ul>
  {/if}

  {#if model === null}
    <div class="skeleton" style="height: 3.5rem"></div>
  {:else}
    <div class="state">
      <p><span class="name">On-device AI checks</span> <span class="summary">{modelSummary(model)}</span></p>
      {#if downloading || pausedAt !== null}
        <div class="progress" role="progressbar" aria-label="Model download" aria-valuemin={0} aria-valuemax={100} aria-valuenow={percent}>
          <span style={`transform: scaleX(${percent / 100})`}></span>
        </div>
        {#if pausedAt !== null}
          <p class="helper">Paused at {pausedAt}%. Resuming continues from there.</p>
        {/if}
      {/if}
    </div>

    <div class="row">
      {#if downloading}
        <button type="button" class="btn" onclick={pause} disabled={busy}>Pause download</button>
      {:else if pausedAt !== null}
        <button type="button" class="btn btn--primary" onclick={download} disabled={busy} aria-busy={busy}>Resume download</button>
        <button type="button" class="btn btn--text" onclick={() => run(async () => { await api.removeModel(); engine.downloadedPercent = null; })} disabled={busy}>Discard partial download</button>
      {:else if model.state === "missing" || model.state === "failed"}
        <button type="button" class="btn btn--primary" onclick={download} disabled={busy} aria-busy={busy}>
          {model.state === "failed" ? "Try the download again" : "Download the AI model"}
        </button>
      {:else if installed && variant === "settings"}
        <button type="button" class="btn btn--danger" onclick={() => (confirmRemove = true)} disabled={busy}>Remove the AI model</button>
      {/if}
    </div>
    <p class="error-text" aria-live="polite">{failed ?? ""}</p>
  {/if}
</div>

<ConfirmDialog
  open={confirmRemove}
  title="Remove the AI model?"
  body="Veil keeps checking keywords and personal-data patterns. You can download the model again later."
  confirm="Remove the model"
  onconfirm={() => { confirmRemove = false; void remove(); }}
  oncancel={() => (confirmRemove = false)}
/>

<style>
  .model { display: grid; gap: var(--space-md); }
  .facts-list { margin: 0; padding-left: var(--space-lg); display: grid; gap: var(--space-2xs); color: var(--color-ink-2); }
  .state { display: grid; gap: var(--space-xs); }
  .name { font-weight: 500; margin-right: var(--space-xs); }
  .summary { color: var(--color-muted); }
</style>
