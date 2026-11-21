<script lang="ts">
  // A unified diff with added and removed lines marked for screen readers
  // as well as by color.
  let { diff }: { diff: string } = $props();

  const lines = $derived(
    diff
      .split('\n')
      .filter((l, i, all) => l !== '' || i < all.length - 1)
      .map((text) => ({
        text,
        kind: text.startsWith('@@')
          ? 'hunk'
          : text.startsWith('+')
            ? 'add'
            : text.startsWith('-')
              ? 'del'
              : 'same',
      })),
  );
</script>

{#if diff}
  <!-- Scrollable, so keyboard users must be able to focus it (WCAG 2.1.1). -->
  <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
  <pre class="diff" aria-label="Changes" tabindex="0"><code
      >{#each lines as line, i (i)}<span class={line.kind}
          >{#if line.kind === 'add'}<span class="visually-hidden">added: </span>{:else if line.kind === 'del'}<span
              class="visually-hidden">removed: </span
            >{/if}{line.text}
</span>{/each}</code
    ></pre>
{:else}
  <p class="secondary">No changes.</p>
{/if}

<style>
  .diff {
    margin: 0;
    max-height: 420px;
    overflow: auto;
    padding: var(--hg-spacing-md) 0;
    border: 1px solid var(--hg-color-border-default);
    border-radius: var(--hg-radius-md);
    background: var(--hg-color-bg-subtle);
    font: 400 12px/18px var(--hg-font-mono);
  }
  .diff span:not(.visually-hidden) {
    display: block;
    padding: 0 var(--hg-spacing-md);
    white-space: pre;
  }
  .add {
    background: var(--hg-color-status-healthy-bg);
    color: var(--hg-color-text-primary);
  }
  .del {
    background: var(--hg-color-status-critical-bg);
    color: var(--hg-color-text-primary);
  }
  .hunk {
    color: var(--hg-color-text-body);
  }
</style>
