<script lang="ts">
  import { onMount } from "svelte";
  import { api, errorText } from "../lib/api";
  import type { Permission } from "../lib/types";

  /** Re-check every couple of seconds while shown, so a grant in System Settings appears without a click. */
  let { poll = true, onchange }: { poll?: boolean; onchange?: (allGranted: boolean) => void } = $props();

  let list = $state<Permission[] | null>(null);
  let failed = $state<string | null>(null);
  let pending = $state<string | null>(null);

  async function load() {
    try {
      list = await api.permissions();
      failed = null;
      onchange?.(list.every((p) => p.granted));
    } catch (e) {
      failed = errorText(e);
    }
  }

  async function grant(id: string) {
    pending = id;
    try {
      await api.requestPermission(id);
    } catch (e) {
      failed = errorText(e);
    } finally {
      pending = null;
      await load();
    }
  }

  onMount(() => {
    void load();
    if (!poll) return;
    const t = setInterval(() => void load(), 2000);
    return () => clearInterval(t);
  });
</script>

{#if failed && list === null}
  <div class="notice notice--error" role="alert">
    <p>Veil couldn’t read its permissions. {failed}</p>
    <div><button type="button" class="btn" onclick={load}>Check again</button></div>
  </div>
{:else if list === null}
  <div class="skeleton" style="height: 4.5rem" aria-label="Checking permissions"></div>
{:else if list.length === 0}
  <p>Nothing to grant on this computer. Veil already has what it needs.</p>
{:else}
  <ul class="perms">
    {#each list as p (p.id)}
      <li>
        <div class="text">
          <p class="title">{p.title}</p>
          <p class="help">{p.help}</p>
        </div>
        {#if p.granted}
          <span class="granted">Granted</span>
        {:else}
          <button type="button" class="btn btn--primary" onclick={() => grant(p.id)} disabled={pending !== null} aria-busy={pending === p.id}>
            {pending === p.id ? "Opening…" : "Grant access"}
          </button>
        {/if}
      </li>
    {/each}
  </ul>
  <p class="error-text" aria-live="polite">{failed ?? ""}</p>
{/if}

<style>
  .perms { list-style: none; margin: 0; padding: 0; display: grid; }
  li { display: flex; align-items: center; justify-content: space-between; gap: var(--space-lg); padding: var(--space-md) 0; border-bottom: 1px solid var(--color-rule); }
  li:first-child { border-top: 1px solid var(--color-rule); }
  .text { display: grid; gap: var(--space-2xs); min-width: 0; }
  .title { font-weight: 500; }
  .help { color: var(--color-muted); font-size: var(--text-sm); }
  .granted { font-family: var(--font-outlier); font-size: var(--text-sm); color: var(--color-accent-deep); padding: var(--space-2xs) var(--space-sm); border: 1px solid var(--color-accent); border-radius: var(--radius-input); white-space: nowrap; }
</style>
