<script lang="ts">
  import { api, errorText } from "../lib/api";
  import { engine } from "../lib/engine.svelte";
  import { platformLabel } from "../lib/format";
  import { router } from "../lib/router.svelte";
  import EngineDown from "../components/EngineDown.svelte";
  import ModelPanel from "../components/ModelPanel.svelte";
  import PermissionList from "../components/PermissionList.svelte";

  let { onfinish }: { onfinish: (autostart: boolean) => Promise<void> } = $props();

  const steps = [
    { id: "welcome", label: "Welcome" },
    { id: "permissions", label: "Permissions" },
    { id: "model", label: "AI checks" },
    { id: "done", label: "Done" },
  ] as const;
  type Step = (typeof steps)[number]["id"];

  const step = $derived<Step>((steps.find((s) => s.id === router.sub)?.id ?? "welcome") as Step);
  const index = $derived(steps.findIndex((s) => s.id === step));

  let allGranted = $state(false);
  let autostart = $state(true);
  let finishing = $state(false);
  let finishError = $state<string | null>(null);
  let heading = $state<HTMLHeadingElement | null>(null);

  const platform = $derived(engine.status?.platform ?? "");
  const model = $derived(engine.model);
  const modelBusy = $derived(model?.state === "downloading");
  const modelReady = $derived(model?.state === "ready" || model?.state === "loaded");

  function go(to: Step) {
    router.go("setup", to);
  }

  // Move focus to the new step's heading so keyboard and screen-reader users land on it.
  $effect(() => {
    void step;
    heading?.focus();
  });

  async function finish() {
    finishing = true;
    finishError = null;
    try {
      await onfinish(autostart);
    } catch (e) {
      finishError = errorText(e);
    } finally {
      finishing = false;
    }
  }
</script>

<div class="setup">
  <header class="head">
    <p class="wordmark" aria-hidden="true">Veil</p>
    <nav aria-label="Setup steps">
      <ol class="steps">
        {#each steps as s, i (s.id)}
          <li class:done={i < index} aria-current={s.id === step ? "step" : undefined}>
            <span class="num" aria-hidden="true">{i + 1}</span>
            <span class="label">{s.label}</span>
            {#if i < index}<span class="visually-hidden">(done)</span>{/if}
          </li>
        {/each}
      </ol>
    </nav>
  </header>

  {#if engine.error && !engine.loading}
    <EngineDown message={engine.error} />
  {/if}

  <section class="panel" aria-labelledby="setup-title">
    {#if step === "welcome"}
      <h1 id="setup-title" tabindex="-1" bind:this={heading}>Keep secrets out of what you send</h1>
      <p class="lead">
        Veil looks at what you’re about to send, paste, drop or upload, in AI chats and any other app. If it spots a
        password, an API key or personal data, it holds it and asks you first.
      </p>
      <div class="tray">
        <div class="label-card" aria-label="How Veil works">
          <dl class="facts">
            <dt>Checks</dt>
            <dd>Keywords you choose, personal-data patterns, and optional on-device AI.</dd>
            <dt>Asks</dt>
            <dd>Send anyway, don’t send, or redact and send. You decide every time.</dd>
            <dt>Keeps</dt>
            <dd>Everything on this computer. No account, no telemetry, nothing uploaded.</dd>
          </dl>
          <p class="stub">The activity log stores what kind of thing was found and where, never the content.</p>
        </div>
      </div>
      <div class="row actions">
        <button type="button" class="btn btn--primary" onclick={() => go("permissions")}>Get started</button>
        <button type="button" class="btn btn--text" onclick={() => void api.openLink("privacy")}>
          Read the privacy notes
        </button>
      </div>
    {:else if step === "permissions"}
      <h1 id="setup-title" tabindex="-1" bind:this={heading}>Let Veil see what you send</h1>
      <p class="lead">
        To check a message before it leaves, Veil needs to read the text box you’re typing in and notice when you press
        Send. It reads only at those moments and keeps nothing.
      </p>

      {#if platform === "macos"}
        <p class="note">macOS asks for this as <strong>Accessibility</strong>. After you grant it, Veil picks it up by itself.</p>
      {:else if platform.startsWith("linux")}
        <p class="note">
          On Linux Veil reads keyboard and mouse events through a <code>veil</code> group. The installer adds a rule for it;
          log out and back in after adding yourself to the group.
        </p>
      {:else if platform === "windows"}
        <p class="note">Windows needs no extra permission. Apps running as administrator can only be checked if Veil runs as administrator too.</p>
      {/if}

      <PermissionList onchange={(ok) => (allGranted = ok)} />

      {#if platform === "linux-wayland"}
        <div class="notice">
          <p>On Wayland, Veil can’t stop a drag-and-drop as it happens. It warns you before the next send instead.</p>
        </div>
      {/if}

      <div class="row actions">
        <button type="button" class="btn btn--primary" onclick={() => go("model")}>
          {allGranted ? "Continue" : "Continue without it"}
        </button>
        <button type="button" class="btn btn--text" onclick={() => go("welcome")}>Back</button>
      </div>
      {#if !allGranted}
        <p class="helper">Without this permission Veil can’t check messages in most apps. You can grant it later from Status.</p>
      {/if}
    {:else if step === "model"}
      <h1 id="setup-title" tabindex="-1" bind:this={heading}>Add on-device AI checks?</h1>
      <p class="lead">
        A small AI model spots things patterns miss: confidential business details, source code, and secrets that don’t
        follow a known format. It runs on your computer only.
      </p>
      <ModelPanel variant="setup" />
      <div class="row actions">
        <button type="button" class="btn btn--primary" onclick={() => go("done")}>
          {modelReady ? "Continue" : modelBusy ? "Continue while it downloads" : "Skip for now"}
        </button>
        <button type="button" class="btn btn--text" onclick={() => go("permissions")}>Back</button>
      </div>
      {#if !modelReady && !modelBusy}
        <p class="helper">You can download it later in Settings, under AI checks.</p>
      {/if}
    {:else}
      <h1 id="setup-title" tabindex="-1" bind:this={heading}>Veil is ready</h1>
      <p class="lead">
        It runs in the {platform === "macos" ? "menu bar" : "system tray"}. Closing this window keeps protection on; use
        “Quit Veil” in the tray menu to stop it.
      </p>
      <dl class="facts summary">
        <dt>Computer</dt>
        <dd>{platform ? platformLabel(platform) : "Checking…"}</dd>
        <dt>Permissions</dt>
        <dd>{engine.status ? (engine.status.permissions_ok ? "Granted" : "Still missing, protection is limited") : "Checking…"}</dd>
        <dt>AI checks</dt>
        <dd>{modelReady ? "On" : modelBusy ? "Downloading" : "Off, keywords and patterns only"}</dd>
      </dl>
      <label class="choice">
        <input type="checkbox" bind:checked={autostart} />
        <span class="choice-title">Start Veil when you log in</span>
        <span class="choice-detail">Protection only works while Veil is running.</span>
      </label>
      <div class="row actions">
        <button type="button" class="btn btn--primary" onclick={finish} disabled={finishing} aria-busy={finishing}>
          {finishing ? "Finishing…" : "Open Veil"}
        </button>
        <button type="button" class="btn btn--text" onclick={() => go("model")}>Back</button>
      </div>
      <p class="error-text" aria-live="polite">{finishError ?? ""}</p>
    {/if}
  </section>
</div>

<style>
  .setup { width: 100%; max-width: 44rem; margin-inline: auto; padding: var(--space-xl) var(--gutter) var(--space-2xl); display: grid; gap: var(--space-xl); }
  .head { display: grid; gap: var(--space-md); }
  .wordmark { font-family: var(--font-display); font-weight: 700; font-size: var(--text-lg); letter-spacing: var(--tracking-display); }
  .steps { list-style: none; margin: 0; padding: 0; display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); gap: var(--space-xs); }
  .steps li { display: flex; align-items: center; gap: var(--space-xs); padding-top: var(--space-xs); border-top: 3px solid var(--color-rule); color: var(--color-muted); font-size: var(--text-sm); min-width: 0; }
  .steps li.done { border-top-color: var(--color-accent-soft); color: var(--color-ink-2); }
  .steps li[aria-current="step"] { border-top-color: var(--color-accent); color: var(--color-ink); font-weight: 500; }
  .num { font-family: var(--font-outlier); font-variant-numeric: tabular-nums; }
  .label { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .panel { display: grid; gap: var(--space-lg); align-content: start; }
  .panel h1 { outline: none; }
  .lead { color: var(--color-ink-2); }
  .note { color: var(--color-ink-2); font-size: var(--text-sm); }
  .actions { margin-top: var(--space-xs); }
  .summary { font-size: var(--text-sm); }
  @media (max-width: 30rem) { .steps .label { display: none; } }
</style>
