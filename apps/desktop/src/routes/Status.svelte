<script lang="ts">
  import { onMount } from "svelte";
  import { errorText } from "../lib/api";
  import { engine } from "../lib/engine.svelte";
  import { appLabel, CHANNEL_LABEL, detectionLabel, HELD, OUTCOME_LABEL, when } from "../lib/format";
  import { router } from "../lib/router.svelte";
  import GuardLabel from "../components/GuardLabel.svelte";

  let busy = $state(false);
  let failed = $state<string | null>(null);

  onMount(() => void engine.loadActivity());

  const status = $derived(engine.status);
  const off = $derived(engine.error !== null || !status?.active);
  const paused = $derived(!off && engine.pausedLeft !== null);
  const recent = $derived(engine.activity.filter((e) => HELD.has(e.outcome) || e.outcome === "sent_anyway").slice(0, 5));

  const heading = $derived(off ? "Protection is off" : paused ? "Protection is paused" : "Veil is on guard");
  const lead = $derived(
    off
      ? "Veil can’t check what you send until its engine runs."
      : paused
        ? "Messages, pastes and files go out unchecked until the pause ends."
        : "Messages, pastes, drops and uploads are checked on this computer before they leave.",
  );

  async function act(fn: () => Promise<void>) {
    busy = true;
    failed = null;
    try {
      await fn();
    } catch (e) {
      failed = errorText(e);
    } finally {
      busy = false;
    }
  }

  const modelMissing = $derived(engine.model?.state === "missing" || engine.model?.state === "failed");
</script>

<div class="page">
  <header class="page-head">
    <h1>{heading}</h1>
    <p>{lead}</p>
  </header>

  <section class="guard" aria-label="Protection">
    <GuardLabel />
    {#if !off}
      <div class="row">
        {#if paused}
          <button type="button" class="btn btn--primary" onclick={() => act(() => engine.resume())} disabled={busy}>Resume protection</button>
        {:else}
          <button type="button" class="btn" onclick={() => act(() => engine.pause(15))} disabled={busy}>Pause for 15 minutes</button>
          <button type="button" class="btn" onclick={() => act(() => engine.pause(60))} disabled={busy}>Pause for 1 hour</button>
        {/if}
      </div>
      <p class="error-text" aria-live="polite">{failed ?? ""}</p>
    {/if}
  </section>

  {#if status && (!status.permissions_ok || modelMissing || !status.drops_blockable || !status.ocr_available || engine.warning)}
    <section class="section" aria-labelledby="attention">
      <h2 id="attention">Worth knowing</h2>
      <div class="notes">
        {#if !status.permissions_ok}
          <div class="notice">
            <p><strong>Veil is missing a permission.</strong> Without it, it can’t see what you send in some apps.</p>
            <div><a class="btn btn--primary" href={router.href("setup", "permissions")}>Review permissions</a></div>
          </div>
        {/if}
        {#if engine.warning}
          <div class="notice"><p>{engine.warning}</p></div>
        {/if}
        {#if modelMissing}
          <div class="notice">
            <p>The on-device AI model isn’t installed, so Veil checks keywords and personal-data patterns only.</p>
            <div><a class="btn" href={router.href("settings", "ai")}>Set up AI checks</a></div>
          </div>
        {/if}
        {#if !status.drops_blockable}
          <div class="notice"><p>On this desktop session Veil can’t stop a drag-and-drop. It warns you before the next send instead.</p></div>
        {/if}
        {#if !status.ocr_available}
          <div class="notice"><p>Text inside images isn’t checked on this system: it has no built-in text recognition.</p></div>
        {/if}
      </div>
    </section>
  {/if}

  <section class="section" aria-labelledby="recent">
    <div class="section-head">
      <h2 id="recent">Recently held</h2>
      <a class="btn btn--text" href={router.href("activity")}>See all activity</a>
    </div>
    {#if engine.activityState === "loading" || engine.activityState === "idle"}
      <div class="skeleton" style="height: 6rem" aria-label="Loading recent activity"></div>
    {:else if engine.activityState === "error"}
      <div class="notice notice--error" role="alert">
        <p>Recent activity couldn’t be loaded. {engine.activityError}</p>
        <div><button type="button" class="btn" onclick={() => engine.loadActivity(true)}>Try again</button></div>
      </div>
    {:else if recent.length === 0}
      <p class="empty">Nothing held yet. When Veil stops a message, paste or file, it shows here with what it found.</p>
    {:else}
      <table class="table">
        <thead>
          <tr><th scope="col">When</th><th scope="col">Where</th><th scope="col">Found</th><th scope="col">What happened</th></tr>
        </thead>
        <tbody>
          {#each recent as e (e.ts_ms + e.app_id + e.outcome)}
            <tr>
              <td class="mono">{when(e.ts_ms)}</td>
              <td>{appLabel(e)}<span class="sub">{CHANNEL_LABEL[e.channel]}</span></td>
              <td>{e.detections.map(detectionLabel).join(", ") || "Not checked"}</td>
              <td>{OUTCOME_LABEL[e.outcome]}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    {/if}
  </section>
</div>

<style>
  .guard { display: grid; gap: var(--space-md); max-width: 40rem; }
  .notes { display: grid; gap: var(--space-sm); }
  .empty { color: var(--color-muted); }
  .sub { display: block; color: var(--color-muted); font-size: var(--text-xs); }
</style>
