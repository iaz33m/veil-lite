<script lang="ts">
  let {
    open,
    title,
    body,
    confirm,
    onconfirm,
    oncancel,
  }: { open: boolean; title: string; body: string; confirm: string; onconfirm: () => void; oncancel: () => void } = $props();

  let dialog = $state<HTMLDialogElement | null>(null);

  $effect(() => {
    if (!dialog) return;
    if (open && !dialog.open) dialog.showModal();
    if (!open && dialog.open) dialog.close();
  });
</script>

<dialog class="confirm" bind:this={dialog} aria-labelledby="confirm-title" onclose={oncancel}>
  <div class="body">
    <h2 id="confirm-title">{title}</h2>
    <p>{body}</p>
    <div class="row actions">
      <button type="button" class="btn" onclick={oncancel}>Keep it</button>
      <button type="button" class="btn btn--danger" onclick={onconfirm}>{confirm}</button>
    </div>
  </div>
</dialog>

<style>
  .body { display: grid; gap: var(--space-md); }
  .actions { justify-content: flex-end; }
</style>
