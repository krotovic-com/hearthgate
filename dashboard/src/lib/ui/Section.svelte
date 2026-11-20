<script lang="ts">
  import type { Snippet } from 'svelte';

  let { id, title, description, children }: { id: string; title: string; description?: string; children: Snippet } =
    $props();
</script>

<section {id} class="section" aria-labelledby="{id}-title" tabindex="-1">
  <div class="head">
    <h2 id="{id}-title">{title}</h2>
    {#if description}<p class="small secondary">{description}</p>{/if}
  </div>
  {@render children()}
</section>

<style>
  .section {
    background: var(--hg-color-bg-surface);
    border: 1px solid var(--hg-color-border-default);
    border-radius: var(--hg-radius-lg);
    scroll-margin-top: var(--hg-spacing-xl);
  }
  .section:focus {
    outline: none;
  }
  .head {
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-xs);
    padding: var(--hg-spacing-xl);
  }
</style>
