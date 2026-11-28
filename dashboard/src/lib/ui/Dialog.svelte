<script lang="ts" module>
  /** Moves a node to the end of <body>, outside the page that gets made inert. */
  function portal(node: HTMLElement) {
    document.body.appendChild(node);
    return {
      destroy() {
        node.remove();
      },
    };
  }
</script>

<script lang="ts">
  // A dialog on the native <dialog> element. It opens without the
  // browser's modal mode so the Ask AI panel stays usable beside it; the
  // page behind it is made inert instead (see overlay.svelte.ts), focus
  // moves inside, Escape closes it, and focus returns to where it was.
  import type { Snippet } from 'svelte';
  import { popOverlay, pushOverlay, overlays } from '../overlay.svelte';

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
  let overlay: number | null = null;
  let returnTo: HTMLElement | null = null;

  function shown() {
    if (!dialog || dialog.open) return;
    returnTo = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    overlay = pushOverlay(title, dialog);
    dialog.show();
    // Like a modal dialog: focus the first control inside.
    queueMicrotask(() => {
      if (!dialog || dialog.contains(document.activeElement)) return;
      const first = dialog.querySelector<HTMLElement>(
        '[autofocus], .body input:not([disabled]), .body select:not([disabled]), .body textarea:not([disabled]), button:not([disabled]), [href], [tabindex]:not([tabindex="-1"])',
      );
      (first ?? dialog).focus();
    });
  }

  function hidden() {
    if (overlay !== null) popOverlay(overlay);
    overlay = null;
    if (dialog?.open) dialog.close();
  }

  $effect(() => {
    if (!dialog) return;
    if (open) shown();
    else if (overlay !== null || dialog.open) {
      hidden();
      onclose?.();
      if (returnTo?.isConnected) returnTo.focus();
      returnTo = null;
    }
  });

  $effect(() => () => {
    if (overlay !== null) popOverlay(overlay);
  });

  const depth = $derived(overlays.stack.findIndex((o) => o.el === dialog));

  function keydown(event: KeyboardEvent) {
    if (event.key !== 'Escape' || event.defaultPrevented) return;
    if (overlays.top?.el !== dialog) return;
    event.preventDefault();
    event.stopPropagation();
    open = false;
  }
</script>

<div class="layer" use:portal style:--depth={Math.max(depth, 0)}>
  {#if open}<div class="backdrop" aria-hidden="true"></div>{/if}
  <dialog
    bind:this={dialog}
    class:wide
    aria-modal="true"
    aria-labelledby="{uid}-title"
    aria-describedby={description ? `${uid}-desc` : undefined}
    tabindex="-1"
    onkeydown={keydown}
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
</div>

<style>
  .layer {
    display: contents;
  }
  .backdrop {
    position: fixed;
    inset: 0;
    /* The Ask AI panel stays out from under the backdrop. */
    right: var(--hg-askai-width, 0px);
    z-index: calc(100 + var(--depth) * 2);
    background: rgb(11 20 38 / 0.4);
  }
  dialog {
    position: fixed;
    inset: 0;
    right: var(--hg-askai-width, 0px);
    z-index: calc(101 + var(--depth) * 2);
    margin: auto;
    width: min(560px, calc(100vw - var(--hg-askai-width, 0px) - 32px));
    height: fit-content;
    max-height: calc(100vh - 64px);
    padding: 0;
    border: 1px solid var(--hg-color-border-default);
    border-radius: var(--hg-radius-lg);
    background: var(--hg-color-bg-surface);
    color: var(--hg-color-text-primary);
    box-shadow: 0 16px 48px rgb(11 20 38 / 0.18);
    overflow: auto;
  }
  dialog:focus {
    outline: none;
  }
  dialog.wide {
    width: min(820px, calc(100vw - var(--hg-askai-width, 0px) - 32px));
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
