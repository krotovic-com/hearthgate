<script lang="ts">
  import type { Snippet } from 'svelte';

  let { title, children, actions }: { title?: string; children: Snippet; actions?: Snippet } = $props();
</script>

<section class="card">
  {#if title || actions}
    <div class="head">
      {#if title}<h2>{title}</h2>{/if}
      {#if actions}<div class="actions">{@render actions()}</div>{/if}
    </div>
  {/if}
  {@render children()}
</section>

<style>
  .card {
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-lg);
    padding: var(--hg-spacing-xl);
    background: var(--hg-color-bg-surface);
    border: 1px solid var(--hg-color-border-default);
    border-radius: var(--hg-radius-lg);
    min-width: 0;
  }
  .head {
    display: flex;
    align-items: center;
    gap: var(--hg-spacing-md);
  }
  .head h2 {
    flex: 1;
  }
  .actions {
    display: flex;
    gap: var(--hg-spacing-sm);
  }
</style>
