<script lang="ts">
  let { label, value, detail }: { label: string; value: string | number; detail?: string } = $props();
</script>

<div class="stat">
  <p class="label">{label}</p>
  <p class="value num">{value}</p>
  {#if detail}<p class="small secondary">{detail}</p>{/if}
</div>

<style>
  .stat {
    flex: 1 0 0;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-xs);
    padding: var(--hg-spacing-lg);
    background: var(--hg-color-bg-surface);
    border: 1px solid var(--hg-color-border-default);
    border-radius: var(--hg-radius-lg);
    white-space: nowrap;
  }
  .value {
    font: 600 28px/36px var(--hg-font-display);
    letter-spacing: -0.02em;
  }
</style>
