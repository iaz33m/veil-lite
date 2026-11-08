<script lang="ts">
  import { onMount } from "svelte";
  import { parse } from "yaml";
  import { api, errorText } from "../lib/api";
  import { engine } from "../lib/engine.svelte";
  import { colourMode, type Mode } from "../lib/mode.svelte";
  import { CHECKS, linesToList, parsePolicy, REGIONS, stringifyPolicy } from "../lib/policy";
  import { router } from "../lib/router.svelte";
  import type { Policy, Region } from "../lib/types";
  import ModelPanel from "../components/ModelPanel.svelte";

  let { autostart }: { autostart: boolean } = $props();

  const tabs = [
    { id: "apps", label: "Apps" },
    { id: "detection", label: "What to look for" },
    { id: "ai", label: "AI checks" },
    { id: "actions", label: "When something is found" },
    { id: "general", label: "General" },
    { id: "advanced", label: "Policy file" },
  ] as const;
  type Tab = (typeof tabs)[number]["id"];

  const tab = $derived<Tab>((tabs.find((t) => t.id === router.sub)?.id ?? "apps") as Tab);

  let loadState = $state<"loading" | "ready" | "error">("loading");
  let loadError = $state<string | null>(null);
  let saved = $state<string>("");
  let draft = $state<Policy | null>(null);
  let yamlText = $state("");
  // Lists are edited as text (one entry per line) and folded into the policy on save.
  let keywordsText = $state("");
  let excludeText = $state("");
  let includeAppsText = $state("");
  let includeSitesText = $state("");

  let saving = $state(false);
  let saveError = $state<string | null>(null);
  let savedNote = $state<string | null>(null);
  let autostartOn = $state(false);
  let autostartError = $state<string | null>(null);

  $effect(() => {
    autostartOn = autostart;
  });

  function fill(yaml: string) {
    saved = yaml;
    yamlText = yaml;
    const p = parsePolicy(yaml);
    draft = p;
    keywordsText = p.keywords.join("\n");
    excludeText = p.scope.exclude_apps.join("\n");
    includeAppsText = p.scope.include_apps.join("\n");
    includeSitesText = p.scope.include_sites.join("\n");
  }

  async function load() {
    loadState = "loading";
    try {
      fill(await api.getPolicy());
      loadState = "ready";
      loadError = null;
    } catch (e) {
      loadError = errorText(e);
      loadState = "error";
    }
  }

  onMount(() => void load());

  function composed(): string {
    if (!draft) return saved;
    const p: Policy = JSON.parse(JSON.stringify(draft));
    p.keywords = linesToList(keywordsText);
    p.scope.exclude_apps = linesToList(excludeText);
    p.scope.include_apps = linesToList(includeAppsText);
    p.scope.include_sites = linesToList(includeSitesText).map((s) => s.toLowerCase());
    return stringifyPolicy(p);
  }

  const dirty = $derived.by(() => {
    if (!draft) return false;
    if (tab === "advanced") return yamlText !== saved;
    return JSON.stringify(parsePolicy(composed())) !== JSON.stringify(parsePolicy(saved));
  });

  // Reload when the policy changes elsewhere (tray, another window), unless there are unsaved edits.
  let seenVersion = engine.policyVersion;
  $effect(() => {
    const v = engine.policyVersion;
    if (v !== seenVersion) {
      seenVersion = v;
      if (!dirty) void load();
    }
  });

  async function save() {
    saving = true;
    saveError = null;
    savedNote = null;
    const yaml = tab === "advanced" ? yamlText : composed();
    if (tab === "advanced") {
      try {
        parse(yaml);
      } catch (e) {
        saveError = `The file isn’t valid YAML: ${errorText(e)}`;
        saving = false;
        return;
      }
    }
    try {
      await api.setPolicy(yaml);
      fill(await api.getPolicy());
      savedNote = "Saved. Veil is using the new settings.";
    } catch (e) {
      saveError = errorText(e);
    } finally {
      saving = false;
    }
  }

  function revert() {
    fill(saved);
    saveError = null;
  }

  function toggleRegion(r: Region, on: boolean) {
    if (!draft) return;
    const set = new Set(draft.pii.regions);
    if (on) set.add(r);
    else set.delete(r);
    draft.pii.regions = REGIONS.map((x) => x.key).filter((k) => set.has(k));
  }

  async function setAutostart(on: boolean) {
    autostartError = null;
    try {
      await api.setAutostart(on);
      autostartOn = on;
    } catch (e) {
      autostartError = errorText(e);
    }
  }

  const modes: { id: Mode; label: string }[] = [
    { id: "system", label: "Same as the system" },
    { id: "light", label: "Light" },
    { id: "dark", label: "Dark" },
  ];
</script>

<div class="page">
  <header class="page-head">
    <h1>Settings</h1>
    <p>Changes apply as soon as you save. They’re stored in Veil’s policy file on this computer.</p>
  </header>

  <nav class="tabs" aria-label="Settings sections">
    {#each tabs as t (t.id)}
      <a href={router.href("settings", t.id)} aria-current={tab === t.id ? "page" : undefined}>{t.label}</a>
    {/each}
  </nav>

  {#if tab === "general"}
    <section class="section" aria-labelledby="s-general">
      <h2 id="s-general">General</h2>
      <label class="choice">
        <input type="checkbox" checked={autostartOn} onchange={(e) => setAutostart(e.currentTarget.checked)} />
        <span class="choice-title">Start Veil when you log in</span>
        <span class="choice-detail">Protection only works while Veil is running.</span>
      </label>
      <p class="error-text" aria-live="polite">{autostartError ?? ""}</p>
      <fieldset class="group">
        <legend class="field-label">Colours</legend>
        <div class="choices">
          {#each modes as m (m.id)}
            <label class="choice">
              <input type="radio" name="mode" value={m.id} checked={colourMode.mode === m.id} onchange={() => colourMode.set(m.id)} />
              <span class="choice-title">{m.label}</span>
            </label>
          {/each}
        </div>
      </fieldset>
    </section>
  {:else if loadState === "loading"}
    <div class="loading" aria-label="Loading settings">
      {#each [0, 1, 2] as i (i)}<div class="skeleton" style="height: 3.5rem"></div>{/each}
    </div>
  {:else if loadState === "error" || !draft}
    <div class="notice notice--error" role="alert">
      <p>Settings couldn’t be loaded from the engine. {loadError}</p>
      <div><button type="button" class="btn" onclick={load}>Try again</button></div>
    </div>
  {:else}
    <form
      class="form"
      onsubmit={(e) => {
        e.preventDefault();
        void save();
      }}
    >
      {#if tab === "apps"}
        <section class="section" aria-labelledby="s-apps">
          <h2 id="s-apps">Which apps Veil watches</h2>
          <div class="choices">
            <label class="choice">
              <input type="radio" name="scope" value="all_apps" bind:group={draft.scope.mode} />
              <span class="choice-title">Every app, except the ones listed below</span>
              <span class="choice-detail">Recommended. Covers chat apps, browsers, email and AI tools you add later.</span>
            </label>
            <label class="choice">
              <input type="radio" name="scope" value="listed_apps" bind:group={draft.scope.mode} />
              <span class="choice-title">Only the apps and sites I list</span>
              <span class="choice-detail">For a narrower setup, for example AI chat sites only.</span>
            </label>
          </div>
          <div class="field">
            <label for="exclude">Never watch these apps</label>
            <textarea id="exclude" class="textarea" rows="6" bind:value={excludeText} aria-describedby="exclude-help"></textarea>
            <p class="helper" id="exclude-help">One per line: a macOS bundle id (com.apple.Terminal) or a program name (WindowsTerminal.exe). Terminals and password managers are listed by default.</p>
          </div>
          {#if draft.scope.mode === "listed_apps"}
            <div class="field">
              <label for="include-apps">Watch these apps</label>
              <textarea id="include-apps" class="textarea" rows="4" bind:value={includeAppsText}></textarea>
              <p class="helper">One per line, in the same form as above.</p>
            </div>
            <div class="field">
              <label for="include-sites">Watch these sites in any browser</label>
              <textarea id="include-sites" class="textarea" rows="5" bind:value={includeSitesText}></textarea>
              <p class="helper">One host per line, for example chatgpt.com. Subdomains are included.</p>
            </div>
          {/if}
        </section>
      {:else if tab === "detection"}
        <section class="section" aria-labelledby="s-keywords">
          <h2 id="s-keywords">Your keywords</h2>
          <div class="field">
            <label for="keywords">Hold anything containing these words or phrases</label>
            <textarea id="keywords" class="textarea" rows="6" bind:value={keywordsText} aria-describedby="kw-help"></textarea>
            <p class="helper" id="kw-help">One per line. Matching ignores capitals and accents, for example a project name or “internal only”.</p>
          </div>
        </section>
        <section class="section" aria-labelledby="s-pii">
          <h2 id="s-pii">Personal data</h2>
          <label class="choice">
            <input type="checkbox" bind:checked={draft.pii.enabled} />
            <span class="choice-title">Look for personal data</span>
            <span class="choice-detail">Pattern checks with validation, so a random number isn’t mistaken for a card.</span>
          </label>
          <fieldset class="group" disabled={!draft.pii.enabled}>
            <legend class="field-label">Formats from</legend>
            <div class="choices">
              {#each REGIONS as r (r.key)}
                <label class="choice">
                  <input type="checkbox" checked={draft.pii.regions.includes(r.key)} onchange={(e) => toggleRegion(r.key, e.currentTarget.checked)} />
                  <span class="choice-title">{r.title}</span>
                  <span class="choice-detail">{r.detail}</span>
                </label>
              {/each}
            </div>
          </fieldset>
        </section>
      {:else if tab === "ai"}
        <section class="section" aria-labelledby="s-model">
          <h2 id="s-model">The AI model</h2>
          <p class="intro">A small model on this computer reads messages and files for four kinds of sensitive content that patterns miss.</p>
          <label class="choice">
            <input type="checkbox" bind:checked={draft.model.enabled} />
            <span class="choice-title">Use AI checks when the model is installed</span>
            <span class="choice-detail">Turn off to rely on keywords and personal-data patterns only.</span>
          </label>
          <ModelPanel variant="settings" />
        </section>
        <section class="section" aria-labelledby="s-checks">
          <h2 id="s-checks">What the model checks</h2>
          <div class="checks">
            {#each CHECKS as c (c.key)}
              {@const rule = draft.checks[c.key]}
              <fieldset class="check" disabled={!draft.model.enabled}>
                <legend class="visually-hidden">{c.title}</legend>
                <label class="check-head">
                  <input type="checkbox" bind:checked={rule.enabled} />
                  <span><span class="choice-title">{c.title}</span><span class="choice-detail block">{c.detail}</span></span>
                </label>
                <div class="check-controls" class:off={!rule.enabled}>
                  <div class="field">
                    <label for={`t-${c.key}`}>Sensitivity <span class="mono">{Math.round((1 - rule.threshold) * 100)}</span></label>
                    <input
                      id={`t-${c.key}`}
                      type="range"
                      min="0.3"
                      max="0.95"
                      step="0.05"
                      value={1.25 - rule.threshold}
                      oninput={(e) => (rule.threshold = Math.round((1.25 - Number(e.currentTarget.value)) * 100) / 100)}
                      disabled={!rule.enabled}
                      aria-describedby={`th-${c.key}`}
                    />
                    <p class="helper" id={`th-${c.key}`}>Higher catches more, with more false alarms. Model score needed: <span class="mono">{rule.threshold.toFixed(2)}</span></p>
                  </div>
                  <div class="field">
                    <label for={`a-${c.key}`}>When found</label>
                    <select id={`a-${c.key}`} class="select" bind:value={rule.action} disabled={!rule.enabled}>
                      <option value="ask">Ask me before sending</option>
                      <option value="block">Block, no override</option>
                    </select>
                  </div>
                </div>
              </fieldset>
            {/each}
          </div>
        </section>
      {:else if tab === "actions"}
        <section class="section" aria-labelledby="s-actions">
          <h2 id="s-actions">When a keyword or personal data is found</h2>
          <div class="choices">
            <label class="choice">
              <input type="radio" name="default" value="ask" bind:group={draft.actions.default} />
              <span class="choice-title">Ask me</span>
              <span class="choice-detail">Send anyway, don’t send, or redact and send.</span>
            </label>
            <label class="choice">
              <input type="radio" name="default" value="block" bind:group={draft.actions.default} />
              <span class="choice-title">Block it</span>
              <span class="choice-detail">You’re told why; there’s no way to send it anyway.</span>
            </label>
          </div>
          <label class="choice">
            <input type="checkbox" bind:checked={draft.actions.allow_redact} />
            <span class="choice-title">Offer “Redact & send” for text</span>
            <span class="choice-detail">Replaces what was found with a placeholder such as [EMAIL_ADDRESS], then sends.</span>
          </label>
        </section>
        <section class="section" aria-labelledby="s-fail">
          <h2 id="s-fail">If a check can’t finish</h2>
          <div class="grid-2">
            <div class="field">
              <label for="fail-text">For messages and pasted text</label>
              <select id="fail-text" class="select" bind:value={draft.actions.fail_mode.text}>
                <option value="open">Let it through</option>
                <option value="closed">Ask me first</option>
              </select>
              <p class="helper">Keyword and pattern checks always finish; this covers the AI check running late.</p>
            </div>
            <div class="field">
              <label for="fail-files">For files and images</label>
              <select id="fail-files" class="select" bind:value={draft.actions.fail_mode.files}>
                <option value="closed">Ask me first</option>
                <option value="open">Let it through</option>
              </select>
              <p class="helper">Applies to files that are too large, encrypted or unreadable.</p>
            </div>
          </div>
        </section>
        <section class="section" aria-labelledby="s-other">
          <h2 id="s-other">Pausing and the log</h2>
          <label class="choice">
            <input type="checkbox" bind:checked={draft.pause.allowed} />
            <span class="choice-title">Allow pausing protection</span>
            <span class="choice-detail">For 15 minutes or an hour, from the tray or the Status screen.</span>
          </label>
          <label class="choice">
            <input type="checkbox" bind:checked={draft.activity_log.enabled} />
            <span class="choice-title">Keep an activity log</span>
            <span class="choice-detail">The app, the kind of data found and what happened. Never the content.</span>
          </label>
        </section>
      {:else if tab === "advanced"}
        <section class="section" aria-labelledby="s-yaml">
          <h2 id="s-yaml">Policy file</h2>
          <div class="field">
            <label for="yaml">policy.yaml</label>
            <textarea
              id="yaml"
              class="textarea yaml"
              rows="22"
              spellcheck="false"
              bind:value={yamlText}
              aria-invalid={saveError ? "true" : undefined}
              aria-describedby="yaml-help"
            ></textarea>
            <p class="helper" id="yaml-help">Every setting, including the labels Veil treats as Send and Open buttons. The engine checks the file before using it.</p>
          </div>
        </section>
      {/if}

      <div class="savebar">
        <button type="submit" class="btn btn--primary" disabled={saving || !dirty} aria-busy={saving}>{saving ? "Saving…" : "Save changes"}</button>
        <button type="button" class="btn btn--text" onclick={revert} disabled={saving || !dirty}>Discard changes</button>
        <p class="status" aria-live="polite">
          {#if saveError}<span class="error-text">{saveError}</span>{:else if savedNote && !dirty}{savedNote}{:else if dirty}Unsaved changes{/if}
        </p>
      </div>
    </form>
  {/if}
</div>

<style>
  .tabs { display: flex; gap: var(--space-2xs); overflow-x: auto; border-bottom: 1px solid var(--color-rule); }
  .tabs a {
    display: inline-flex; align-items: center; min-height: 44px; padding: 0 var(--space-sm); color: var(--color-muted); text-decoration: none;
    font-size: var(--text-sm); font-weight: 500; white-space: nowrap; border-bottom: 2px solid transparent; margin-bottom: -1px;
    transition: color var(--dur-short) var(--ease-out), border-color var(--dur-short) var(--ease-out);
  }
  .tabs a:hover { color: var(--color-ink); }
  .tabs a[aria-current="page"] { color: var(--color-ink); border-bottom-color: var(--color-accent); }
  .form { display: grid; gap: var(--space-xl); }
  .loading { display: grid; gap: var(--space-sm); }
  .choices { display: grid; gap: var(--space-xs); }
  .group { border: 0; margin: 0; padding: 0; display: grid; gap: var(--space-xs); min-width: 0; }
  .group:disabled { opacity: 0.55; }
  .intro { color: var(--color-ink-2); }
  .checks { display: grid; gap: var(--space-md); }
  .check { border: 0; margin: 0; padding: 0 0 var(--space-md); border-bottom: 1px solid var(--color-rule); display: grid; gap: var(--space-sm); min-width: 0; }
  .check:last-child { border-bottom: 0; }
  .check:disabled { opacity: 0.55; }
  .check-head { display: flex; gap: var(--space-sm); align-items: flex-start; cursor: pointer; }
  .check-head input { margin-top: 3px; }
  .block { display: block; }
  .check-controls { display: grid; grid-template-columns: minmax(0, 2fr) minmax(0, 1fr); gap: var(--space-lg); padding-left: calc(18px + var(--space-sm)); }
  .check-controls.off { opacity: 0.55; }
  .grid-2 { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: var(--space-lg); }
  .yaml { min-height: 24rem; }
  .savebar {
    position: sticky; bottom: 0; display: flex; align-items: center; gap: var(--space-sm); flex-wrap: wrap;
    padding: var(--space-md) 0; background: var(--color-paper); border-top: 1px solid var(--color-rule);
  }
  .status { color: var(--color-muted); font-size: var(--text-sm); }
  @media (max-width: 40rem) { .check-controls, .grid-2 { grid-template-columns: minmax(0, 1fr); padding-left: 0; } }
</style>
