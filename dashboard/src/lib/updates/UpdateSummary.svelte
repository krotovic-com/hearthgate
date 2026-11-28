<script lang="ts">
  // The head of an update card (Figma: Updates / Update summary):
  // an icon for the status, a title, a sentence, and actions.
  import type { Snippet } from 'svelte';
  import Icon, { type IconName } from '../ui/Icon.svelte';

  type Status = 'available' | 'up-to-date' | 'installing' | 'rolled-back';
  let {
    status,
    title,
    description,
    actions,
  }: { status: Status; title: string; description?: string; actions?: Snippet } = $props();
  const icon: Record<Status, IconName> = {
    available: 'arrow-up',
    'up-to-date': 'check',
    installing: 'refresh',
    'rolled-back': 'undo',
  };
</script>

<div class="summary">
  <span class="icon {status}" aria-hidden="true"><Icon name={icon[status]} /></span>
  <div class="text">
    <p class="title">{title}</p>
    {#if description}<p class="small secondary">{description}</p>{/if}
  </div>
  {#if actions}<div class="actions">{@render actions()}</div>{/if}
</div>

<style>
  .summary {
    display: flex;
    align-items: center;
    gap: var(--hg-spacing-lg);
  }
  .icon {
    flex: 0 0 40px;
    height: 40px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border-radius: var(--hg-radius-full);
    background: var(--hg-color-accent-subtle);
    color: var(--hg-color-text-link);
  }
  .icon.up-to-date {
    background: var(--hg-color-status-healthy-bg);
    color: var(--hg-color-status-healthy-text);
  }
  .icon.rolled-back {
    background: var(--hg-color-status-warning-bg);
    color: var(--hg-color-status-warning-text);
  }
  .icon.installing :global(.icon) {
    animation: spin 2s linear infinite;
  }
  @media (prefers-reduced-motion: reduce) {
    .icon.installing :global(.icon) {
      animation: none;
    }
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
  .text {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-2xs);
  }
  .title {
    font: 600 16px/24px var(--hg-font-display);
    color: var(--hg-color-text-primary);
  }
  .actions {
    display: flex;
    align-items: center;
    gap: var(--hg-spacing-sm);
    flex-wrap: wrap;
    justify-content: flex-end;
  }
</style>
