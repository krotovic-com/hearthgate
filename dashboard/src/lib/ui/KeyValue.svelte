<script lang="ts">
  // A definition list of label and value rows, as in the Figma panels.
  let {
    rows,
    labelWidth = 110,
    mono = true,
  }: { rows: { label: string; value: string }[]; labelWidth?: 96 | 110 | 120; mono?: boolean } = $props();
</script>

<dl class="kv w{labelWidth}">
  {#each rows as row (row.label)}
    <dt class="small secondary">{row.label}</dt>
    <dd class={mono ? 'mono-small' : 'small'}>{row.value}</dd>
  {/each}
</dl>

<style>
  .kv {
    display: grid;
    gap: var(--hg-spacing-sm) var(--hg-spacing-lg);
    margin: 0;
    align-items: baseline;
  }
  .w96 {
    grid-template-columns: 96px 1fr;
  }
  .w110 {
    grid-template-columns: 110px 1fr;
  }
  .w120 {
    grid-template-columns: 120px 1fr;
  }
  dd {
    margin: 0;
    min-width: 0;
    overflow-wrap: anywhere;
    color: var(--hg-color-text-primary);
  }
</style>
