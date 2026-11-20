<script lang="ts">
  import type { Snippet } from 'svelte';

  let { tone = 'critical', children }: { tone?: 'critical' | 'warning' | 'neutral'; children: Snippet } =
    $props();
</script>

<div class="alert {tone}" role={tone === 'critical' ? 'alert' : 'status'}>
  {@render children()}
</div>

<style>
  .alert {
    padding: var(--hg-spacing-md) var(--hg-spacing-lg);
    border-radius: var(--hg-radius-md);
  }
  .critical {
    background: var(--hg-color-status-critical-bg);
    color: var(--hg-color-status-critical-text);
  }
  .warning {
    background: var(--hg-color-status-warning-bg);
    color: var(--hg-color-status-warning-text);
  }
  .neutral {
    background: var(--hg-color-status-neutral-bg);
    color: var(--hg-color-text-primary);
  }
</style>
