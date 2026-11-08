<script lang="ts">
  import { router, type Page } from "../lib/router.svelte";
  import { colourMode } from "../lib/mode.svelte";
  import { engine } from "../lib/engine.svelte";

  const tabs: { page: Page; label: string }[] = [
    { page: "status", label: "Status" },
    { page: "activity", label: "Activity" },
    { page: "settings", label: "Settings" },
    { page: "about", label: "About" },
  ];

  const modeLabel = $derived(
    colourMode.mode === "system" ? "Colours follow the system" : colourMode.mode === "light" ? "Light colours" : "Dark colours",
  );
  const state = $derived(
    engine.error ? "Engine not running" : engine.pausedLeft !== null ? "Paused" : engine.status?.active ? "On guard" : "Starting",
  );
</script>

<header class="bar">
  <div class="inner">
    <a class="wordmark" href={router.href("status")} aria-label="Veil, status">Veil</a>
    <nav aria-label="Main">
      <ul>
        {#each tabs as tab (tab.page)}
          <li>
            <a href={router.href(tab.page)} aria-current={router.page === tab.page ? "page" : undefined}>{tab.label}</a>
          </li>
        {/each}
      </ul>
    </nav>
    <div class="end">
      <span class="state" class:off={state !== "On guard"} aria-live="polite">{state}</span>
      <button type="button" class="btn btn--text mode" onclick={() => colourMode.next()} title={modeLabel} aria-label={`${modeLabel}. Change colours`}>
        {#if colourMode.mode === "dark"}
          <svg width="18" height="18" viewBox="0 0 24 24" aria-hidden="true"><path d="M20 14.5A8 8 0 0 1 9.5 4a8 8 0 1 0 10.5 10.5Z" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round" /></svg>
        {:else if colourMode.mode === "light"}
          <svg width="18" height="18" viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="4.5" fill="none" stroke="currentColor" stroke-width="1.8" /><path d="M12 2.5v2.5M12 19v2.5M2.5 12H5M19 12h2.5M5.3 5.3l1.8 1.8M16.9 16.9l1.8 1.8M5.3 18.7l1.8-1.8M16.9 7.1l1.8-1.8" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" /></svg>
        {:else}
          <svg width="18" height="18" viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="8.5" fill="none" stroke="currentColor" stroke-width="1.8" /><path d="M12 3.5a8.5 8.5 0 0 1 0 17Z" fill="currentColor" /></svg>
        {/if}
      </button>
    </div>
  </div>
</header>

<style>
  .bar { position: sticky; top: 0; z-index: var(--z-sticky-nav); background: var(--color-paper); border-bottom: var(--rule-hair) solid var(--color-rule); }
  .inner { max-width: var(--container); margin-inline: auto; padding-inline: var(--gutter); min-height: var(--bar-height); display: flex; align-items: center; gap: var(--space-lg); }
  .wordmark { font-family: var(--font-display); font-weight: 700; font-size: var(--text-lg); letter-spacing: var(--tracking-display); text-decoration: none; color: var(--color-ink); }
  nav { min-width: 0; overflow-x: auto; }
  ul { list-style: none; margin: 0; padding: 0; display: flex; gap: var(--space-2xs); }
  nav a {
    display: inline-flex; align-items: center; min-height: 44px; padding: 0 var(--space-sm); border-radius: var(--radius-input);
    color: var(--color-muted); text-decoration: none; font-size: var(--text-sm); font-weight: 500; white-space: nowrap;
    transition: background-color var(--dur-short) var(--ease-out), color var(--dur-short) var(--ease-out);
  }
  nav a:hover { color: var(--color-ink); background: var(--color-paper-2); }
  nav a[aria-current="page"] { color: var(--color-ink); background: var(--color-accent-soft); }
  .end { margin-left: auto; display: flex; align-items: center; gap: var(--space-sm); }
  .state { font-size: var(--text-sm); color: var(--color-accent-deep); font-weight: 500; white-space: nowrap; }
  .state.off { color: var(--color-muted); }
  .mode { min-width: 44px; min-height: 44px; color: var(--color-ink-2); }
  @media (max-width: 40rem) { .state { display: none; } }
</style>
