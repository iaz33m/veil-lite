<script lang="ts">
  import { onMount } from "svelte";
  import { api } from "./lib/api";
  import { engine } from "./lib/engine.svelte";
  import { router } from "./lib/router.svelte";
  import type { AppState } from "./lib/types";
  import TopBar from "./components/TopBar.svelte";
  import EngineDown from "./components/EngineDown.svelte";
  import Setup from "./routes/Setup.svelte";
  import Status from "./routes/Status.svelte";
  import Activity from "./routes/Activity.svelte";
  import Settings from "./routes/Settings.svelte";
  import About from "./routes/About.svelte";

  let appState = $state<AppState | null>(null);

  onMount(() => {
    engine.start();
    api
      .appState()
      .then((s) => {
        appState = s;
        if (!s.setup_complete && router.page !== "setup") router.go("setup");
      })
      .catch(() => {
        appState = { setup_complete: true, autostart: false, version: "" };
      });
  });

  const pageTitle: Record<string, string> = {
    setup: "Set up Veil",
    status: "Veil",
    activity: "Activity, Veil",
    settings: "Settings, Veil",
    about: "About Veil",
  };

  $effect(() => {
    document.title = pageTitle[router.page] ?? "Veil";
  });

  async function finishSetup(autostart: boolean) {
    await api.completeSetup(autostart);
    if (appState) appState = { ...appState, setup_complete: true, autostart };
    router.go("status");
  }
</script>

<a class="skip" href="#main">Skip to content</a>

{#if router.page === "setup"}
  <main id="main" tabindex="-1">
    <Setup onfinish={finishSetup} />
  </main>
{:else}
  <TopBar />
  <main id="main" tabindex="-1">
    {#if engine.error && !engine.loading && router.page !== "about"}
      <EngineDown message={engine.error} />
    {/if}
    {#if router.page === "status"}
      <Status />
    {:else if router.page === "activity"}
      <Activity />
    {:else if router.page === "settings"}
      <Settings autostart={appState?.autostart ?? false} />
    {:else if router.page === "about"}
      <About version={appState?.version ?? ""} />
    {/if}
  </main>
{/if}

<style>
  .skip { position: absolute; left: var(--space-md); top: calc(-1 * var(--space-3xl)); z-index: var(--z-tooltip); padding: var(--space-xs) var(--space-md); border-radius: var(--radius-input); background: var(--color-ink); color: var(--color-paper); }
  .skip:focus-visible { top: var(--space-md); }
  main { outline: none; }
</style>
