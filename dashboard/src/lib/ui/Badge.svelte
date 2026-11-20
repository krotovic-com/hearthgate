<script lang="ts" module>
  export type BadgeStatus = 'healthy' | 'warning' | 'critical' | 'neutral' | 'public' | 'vpn';
</script>

<script lang="ts">
  import type { Snippet } from 'svelte';

  let { status, dot = false, children }: { status: BadgeStatus; dot?: boolean; children: Snippet } = $props();
</script>

<span class="badge {status}">{#if dot}<span class="dot"></span>{/if}{@render children()}</span>

<style>
  .badge {
    display: inline-flex;
    align-items: center;
    gap: var(--hg-spacing-xs);
    padding: var(--hg-spacing-2xs) var(--hg-spacing-sm);
    border-radius: var(--hg-radius-full);
    font: 500 12px/16px var(--hg-font-body);
    white-space: nowrap;
  }
  .dot {
    width: 6px;
    height: 6px;
    border-radius: var(--hg-radius-full);
    background: currentColor;
  }
  .healthy,
  .vpn {
    background: var(--hg-color-status-healthy-bg);
    color: var(--hg-color-status-healthy-text);
  }
  .warning {
    background: var(--hg-color-status-warning-bg);
    color: var(--hg-color-status-warning-text);
  }
  .critical {
    background: var(--hg-color-status-critical-bg);
    color: var(--hg-color-status-critical-text);
  }
  .neutral {
    background: var(--hg-color-status-neutral-bg);
    color: var(--hg-color-status-neutral-text);
  }
  .public {
    background: var(--hg-color-accent-subtle);
    color: var(--hg-color-text-link);
  }
</style>
