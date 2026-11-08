<script lang="ts">
  import { onMount } from "svelte";
  import { api, errorText } from "../lib/api";
  import { engine } from "../lib/engine.svelte";
  import { appLabel, CHANNEL_LABEL, detectionLabel, OUTCOME_LABEL, when } from "../lib/format";
  import { router } from "../lib/router.svelte";
  import type { Channel, Outcome } from "../lib/types";
  import ConfirmDialog from "../components/ConfirmDialog.svelte";

  let confirmClear = $state(false);
  let clearing = $state(false);
  let failed = $state<string | null>(null);

  onMount(() => void engine.loadActivity(true));

  const channel = $derived((router.query.channel ?? "") as Channel | "");
  const outcome = $derived((router.query.outcome ?? "") as Outcome | "");
  const rows = $derived(
    engine.activity.filter((e) => (!channel || e.channel === channel) && (!outcome || e.outcome === outcome)),
  );

  function setFilter(key: "channel" | "outcome", value: string) {
    router.replaceQuery({ ...router.query, [key]: value });
  }

  async function clear() {
    confirmClear = false;
    clearing = true;
    failed = null;
    try {
      await api.clearActivity();
      engine.activity = [];
    } catch (e) {
      failed = errorText(e);
    } finally {
      clearing = false;
    }
  }
</script>

<div class="page">
  <header class="page-head">
    <h1>Activity</h1>
    <p>What Veil checked and what you decided. Only the app, the kind of data and the outcome are kept, never what you wrote or the files themselves.</p>
  </header>

  <div class="toolbar">
    <div class="field">
      <label for="f-channel">How it was sent</label>
      <select id="f-channel" class="select" value={channel} onchange={(e) => setFilter("channel", e.currentTarget.value)}>
        <option value="">Any way</option>
        {#each Object.entries(CHANNEL_LABEL) as [key, label] (key)}
          <option value={key}>{label}</option>
        {/each}
      </select>
    </div>
    <div class="field">
      <label for="f-outcome">What happened</label>
      <select id="f-outcome" class="select" value={outcome} onchange={(e) => setFilter("outcome", e.currentTarget.value)}>
        <option value="">Anything</option>
        {#each Object.entries(OUTCOME_LABEL) as [key, label] (key)}
          <option value={key}>{label}</option>
        {/each}
      </select>
    </div>
    <div class="end">
      <button type="button" class="btn btn--danger" onclick={() => (confirmClear = true)} disabled={clearing || engine.activity.length === 0} aria-busy={clearing}>
        {clearing ? "Clearing…" : "Clear the log"}
      </button>
    </div>
  </div>
  <p class="error-text" aria-live="polite">{failed ?? ""}</p>

  {#if engine.activityState === "loading" || engine.activityState === "idle"}
    <div class="loading" aria-label="Loading activity">
      {#each [0, 1, 2, 3] as i (i)}<div class="skeleton" style="height: 2.75rem"></div>{/each}
    </div>
  {:else if engine.activityState === "error"}
    <div class="notice notice--error" role="alert">
      <p>The activity log couldn’t be read. {engine.activityError}</p>
      <div><button type="button" class="btn" onclick={() => engine.loadActivity(true)}>Try again</button></div>
    </div>
  {:else if engine.activity.length === 0}
    <div class="notice">
      <p><strong>No activity yet.</strong> Each time Veil checks a file or holds a message, it’s listed here so you can see what was caught.</p>
      <div><a class="btn" href={router.href("settings", "detection")}>Choose what to look for</a></div>
    </div>
  {:else if rows.length === 0}
    <div class="notice">
      <p>Nothing matches these filters.</p>
      <div><button type="button" class="btn" onclick={() => router.replaceQuery({})}>Show everything</button></div>
    </div>
  {:else}
    <table class="table">
      <caption class="visually-hidden">Activity, newest first</caption>
      <thead>
        <tr><th scope="col">When</th><th scope="col">Where</th><th scope="col">How</th><th scope="col">Found</th><th scope="col">What happened</th></tr>
      </thead>
      <tbody>
        {#each rows as e, i (`${e.ts_ms}-${i}`)}
          <tr>
            <td class="mono nowrap">{when(e.ts_ms)}</td>
            <th scope="row" class="where">{appLabel(e)}</th>
            <td>{CHANNEL_LABEL[e.channel]}{e.files > 0 ? `, ${e.files} ${e.files === 1 ? "file" : "files"}` : ""}</td>
            <td>
              {#if e.detections.length}
                <span class="tags">{#each e.detections as d (d)}<span class="tag">{detectionLabel(d)}</span>{/each}</span>
              {:else}
                <span class="muted">Nothing</span>
              {/if}
            </td>
            <td>{OUTCOME_LABEL[e.outcome]}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}
</div>

<ConfirmDialog
  open={confirmClear}
  title="Clear the activity log?"
  body="Every entry is deleted from this computer. This can’t be undone."
  confirm="Clear the log"
  onconfirm={clear}
  oncancel={() => (confirmClear = false)}
/>

<style>
  .toolbar { display: flex; align-items: flex-end; gap: var(--space-md); flex-wrap: wrap; }
  .toolbar .field { width: min(16rem, 100%); }
  .end { margin-left: auto; }
  .loading { display: grid; gap: var(--space-xs); }
  .nowrap { white-space: nowrap; }
  .where { font-weight: 500; text-align: left; }
  .muted { color: var(--color-muted); }
  @media (max-width: 40rem) {
    .table thead { display: none; }
    .table tr { display: grid; gap: var(--space-2xs); padding: var(--space-sm) 0; border-bottom: 1px solid var(--color-rule); }
    .table td, .table th { border: 0; padding: 0; }
  }
</style>
