<script lang="ts">
  import { engine } from "../lib/engine.svelte";
  import { appLabel, clock, countdown, detectionLabel, HELD, isToday, platformLabel } from "../lib/format";

  const status = $derived(engine.status);
  const off = $derived(engine.error !== null || !status?.active);
  const paused = $derived(!off && engine.pausedLeft !== null);

  const today = $derived(engine.activity.filter((e) => isToday(e.ts_ms)));
  const heldToday = $derived(today.filter((e) => HELD.has(e.outcome)).length);
  const checkedToday = $derived(today.length);
  const lastHeld = $derived(engine.activity.find((e) => HELD.has(e.outcome)) ?? null);

  const aiLine = $derived.by(() => {
    const m = engine.model;
    if (!m) return "AI checks: checking…";
    if (m.state === "ready" || m.state === "loaded") return "AI checks: on";
    if (m.state === "downloading") return `AI checks: downloading, ${m.percent}%`;
    return "AI checks: not installed";
  });
</script>

<div class="tray">
  <article class="label-card" aria-label="Protection status">
    <div class="top">
      <div class="lines">
        <p class="kind">Veil{status ? `, ${platformLabel(status.platform)}` : ""}</p>
        <p class="state">{off ? "Not protecting" : paused ? "Paused" : "On guard"}</p>
      </div>
      {#if off}
        <div class="stamp stamp--off" aria-hidden="true"><span>Off</span></div>
      {:else if paused}
        <div class="stamp stamp--off" aria-hidden="true"><span>Paused</span><span class="small">{countdown(engine.pausedLeft ?? 0)} left</span></div>
      {:else}
        <div class="stamp" aria-hidden="true"><span>Checked</span><span class="small">before it leaves</span></div>
      {/if}
    </div>

    {#if engine.activityState === "ready"}
      <dl class="facts">
        <dt>Held today</dt>
        <dd>{heldToday}</dd>
        <dt>Checked today</dt>
        <dd>{checkedToday}</dd>
        <dt>Last held</dt>
        <dd>
          {#if lastHeld}
            {clock(lastHeld.ts_ms)}, {appLabel(lastHeld)}{lastHeld.detections[0] ? `, ${detectionLabel(lastHeld.detections[0])}` : ""}
          {:else}
            Nothing yet
          {/if}
        </dd>
      </dl>
    {:else if engine.activityState === "error"}
      <p class="muted">Today’s figures aren’t available right now.</p>
    {:else}
      <div class="skeleton" style="height: 4.5rem" aria-label="Loading today’s figures"></div>
    {/if}

    <p class="stub">Keywords and personal data: on. {aiLine}.</p>
  </article>
</div>

<style>
  .top { display: flex; align-items: flex-start; justify-content: space-between; gap: var(--space-lg); }
  .lines { display: grid; gap: var(--space-2xs); min-width: 0; }
  .kind { color: var(--color-muted); font-size: var(--text-sm); }
  .state { font-size: var(--text-xl); font-weight: 500; letter-spacing: -0.01em; }
  .stamp { flex-shrink: 0; margin-top: var(--space-2xs); }
  .small { font-size: var(--text-xs); font-weight: 400; }
  .facts { font-size: var(--text-sm); }
  .facts dt { font-weight: 400; color: var(--color-muted); }
  .facts dd { color: var(--color-ink); }
  .muted { color: var(--color-muted); font-size: var(--text-sm); }
</style>
