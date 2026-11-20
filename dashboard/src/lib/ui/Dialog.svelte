<script lang="ts">
  // A modal dialog on the native <dialog> element: focus stays inside,
  // Escape closes it, and focus returns to where it was.
  import type { Snippet } from 'svelte';

  let {
    open = $bindable(false),
    title,
    description,
    wide = false,
    children,
    footer,
    onclose,
  }: {
    open?: boolean;
    title: string;
    description?: string;
    wide?: boolean;
    children: Snippet;
    footer?: Snippet;
    onclose?: () => void;
  } = $props();

  let dialog: HTMLDialogElement | undefined = $state();
  const uid = $props.id();

  $effect(() => {
    if (!dialog) return;
    if (open && !dialog.open) dialog.showModal();
    if (!open && dialog.open) dialog.close();
  });
</script>

<dialog
  bind:this={dialog}
  class:wide
  aria-labelledby="{uid}-title"
  aria-describedby={description ? `${uid}-desc` : undefined}
  onclose={() => {
    open = false;
    onclose?.();
  }}
>
  <div class="head">
    <h2 id="{uid}-title">{title}</h2>
    {#if description}<p id="{uid}-desc" class="secondary">{description}</p>{/if}
  </div>
  {#if open}
    <div class="body">{@render children()}</div>
    {#if footer}<div class="footer">{@render footer()}</div>{/if}
  {/if}
</dialog>

<style>
  dialog {
    width: min(560px, calc(100vw - 32px));
    max-height: calc(100vh - 64px);
    padding: 0;
    border: 1px solid var(--hg-color-border-default);
    border-radius: var(--hg-radius-lg);
    background: var(--hg-color-bg-surface);
    color: var(--hg-color-text-primary);
    box-shadow: 0 16px 48px rgb(11 20 38 / 0.18);
    overflow: auto;
  }
  dialog.wide {
    width: min(820px, calc(100vw - 32px));
  }
  dialog::backdrop {
    background: rgb(11 20 38 / 0.4);
  }
  .head {
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-xs);
    padding: var(--hg-spacing-xl) var(--hg-spacing-xl) 0;
  }
  .body {
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-lg);
    padding: var(--hg-spacing-xl);
  }
  .footer {
    display: flex;
    gap: var(--hg-spacing-sm);
    justify-content: flex-end;
    padding: var(--hg-spacing-lg) var(--hg-spacing-xl);
    border-top: 1px solid var(--hg-color-border-default);
    position: sticky;
    bottom: 0;
    background: var(--hg-color-bg-surface);
  }
</style>
