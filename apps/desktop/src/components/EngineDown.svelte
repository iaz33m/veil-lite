<script lang="ts">
  import { api, errorText } from "../lib/api";
  import { engine } from "../lib/engine.svelte";

  let { message }: { message: string } = $props();
  let busy = $state<"retry" | "restart" | null>(null);
  let failed = $state<string | null>(null);

  async function retry() {
    busy = "retry";
    failed = null;
    await engine.refresh();
    busy = null;
  }

  async function restart() {
    busy = "restart";
    failed = null;
    try {
      await api.engineRestart();
      await new Promise((r) => setTimeout(r, 1200));
      await engine.refresh();
    } catch (e) {
      failed = errorText(e);
    } finally {
      busy = null;
    }
  }
</script>

<div class="wrap">
  <div class="notice notice--error" role="alert">
    <h2>Veil’s engine isn’t running</h2>
    <p>Protection is off: nothing you send is checked until it starts again.</p>
    <p class="helper">{message}</p>
    <div class="row">
      <button type="button" class="btn btn--primary" onclick={restart} disabled={busy !== null} aria-busy={busy === "restart"}>
        {busy === "restart" ? "Starting the engine…" : "Restart the engine"}
      </button>
      <button type="button" class="btn" onclick={retry} disabled={busy !== null} aria-busy={busy === "retry"}>
        {busy === "retry" ? "Checking…" : "Check again"}
      </button>
    </div>
    <p class="error-text" aria-live="polite">{failed ?? ""}</p>
  </div>
</div>

<style>
  .wrap { max-width: var(--container); margin-inline: auto; padding: var(--space-lg) var(--gutter) 0; }
</style>
