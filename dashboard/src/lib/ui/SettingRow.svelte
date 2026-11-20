<script lang="ts">
  import type { Snippet } from 'svelte';

  let { label, description, children }: { label: string; description?: string; children?: Snippet } = $props();
</script>

<div class="row">
  <div class="text">
    <p>{label}</p>
    {#if description}<p class="small secondary">{description}</p>{/if}
  </div>
  {#if children}<div class="control">{@render children()}</div>{/if}
</div>

<style>
  .row {
    display: flex;
    align-items: center;
    gap: var(--hg-spacing-xl);
    padding: var(--hg-spacing-lg) var(--hg-spacing-xl);
    border-top: 1px solid var(--hg-color-border-default);
  }
  .text {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-2xs);
  }
  .control {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: var(--hg-spacing-sm);
    max-width: 50%;
    min-width: 0;
    flex-wrap: wrap;
  }
</style>
