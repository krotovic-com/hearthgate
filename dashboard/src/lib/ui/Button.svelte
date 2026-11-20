<script lang="ts">
  import type { Snippet } from 'svelte';
  import type { HTMLButtonAttributes } from 'svelte/elements';

  type Props = HTMLButtonAttributes & {
    variant?: 'primary' | 'secondary' | 'ghost' | 'danger';
    size?: 'medium' | 'small';
    busy?: boolean;
    children: Snippet;
  };
  let {
    variant = 'primary',
    size = 'medium',
    busy = false,
    type = 'button',
    disabled,
    children,
    ...rest
  }: Props = $props();
</script>

<button
  {type}
  class="button {variant} {size}"
  disabled={disabled || busy}
  aria-busy={busy || undefined}
  {...rest}
>
  {@render children()}
</button>

<style>
  .button {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: var(--hg-spacing-sm);
    border: 1px solid transparent;
    border-radius: var(--hg-radius-md);
    font: 500 14px/20px var(--hg-font-body);
    cursor: pointer;
    white-space: nowrap;
  }
  .medium {
    padding: calc(var(--hg-spacing-sm) - 1px) calc(var(--hg-spacing-lg) - 1px);
  }
  .small {
    padding: calc(var(--hg-spacing-xs) - 1px) calc(var(--hg-spacing-md) - 1px);
    font-size: 12px;
    line-height: 16px;
  }
  /* route-600 rather than the brand blue keeps white text above 4.5:1. */
  .primary {
    background: var(--hg-color-accent-hover);
    color: var(--hg-color-text-on-accent);
  }
  .primary:hover:not(:disabled) {
    background-image: linear-gradient(rgb(0 0 0 / 0.15), rgb(0 0 0 / 0.15));
  }
  .secondary {
    background: var(--hg-color-bg-surface);
    border-color: var(--hg-color-border-strong);
    color: var(--hg-color-text-primary);
  }
  .secondary:hover:not(:disabled),
  .ghost:hover:not(:disabled) {
    background: var(--hg-color-bg-subtle);
  }
  .ghost {
    background: transparent;
    color: var(--hg-color-text-primary);
  }
  .danger {
    background: var(--hg-color-status-critical-fg);
    color: var(--hg-color-text-on-accent);
  }
  .danger:hover:not(:disabled) {
    background-image: linear-gradient(rgb(0 0 0 / 0.12), rgb(0 0 0 / 0.12));
  }
  .button:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }
  .button[aria-busy='true'] {
    cursor: progress;
  }
</style>
