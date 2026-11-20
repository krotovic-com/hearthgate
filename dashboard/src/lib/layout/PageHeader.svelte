<script lang="ts">
  import type { Snippet } from 'svelte';

  let { title, subtitle, actions }: { title: string; subtitle?: string; actions?: Snippet } = $props();
</script>

<header class="header">
  <div class="title">
    <h1 tabindex="-1">{title}</h1>
    {#if subtitle}<p class="secondary">{subtitle}</p>{/if}
  </div>
  {#if actions}<div class="actions">{@render actions()}</div>{/if}
</header>

<style>
  .header {
    display: flex;
    align-items: center;
    gap: var(--hg-spacing-md);
  }
  .title {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-xs);
  }
  .actions {
    display: flex;
    gap: var(--hg-spacing-sm);
  }
</style>
