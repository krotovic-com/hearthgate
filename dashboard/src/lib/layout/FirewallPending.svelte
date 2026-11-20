<script lang="ts">
  // A firewall change waits for proof that the dashboard can still reach
  // Hearthgate: a request over a connection opened after the change. This
  // sends that request as soon as the change shows up. If the new rules
  // cut this browser off, nothing arrives, and Hearthgate undoes the change
  // after 60 seconds by itself.
  import Button from '../ui/Button.svelte';
  import { api, ApiError } from '../api';
  import { live } from '../live.svelte';

  const pending = $derived(live.status?.firewall?.pending ?? null);

  let confirmed = $state<number | null>(null);
  let trying = $state<number | null>(null);
  let failed = $state<string | null>(null);
  let left = $state(0);

  async function confirm(revision: number) {
    if (trying === revision) return;
    trying = revision;
    failed = null;
    // The first try can reuse a connection from before the change, which
    // does not count. The server closes such connections, so the next try
    // opens a new one.
    for (let attempt = 0; attempt < 8; attempt++) {
      try {
        await api.confirmFirewall();
        confirmed = revision;
        setTimeout(() => {
          if (confirmed === revision) confirmed = null;
        }, 6000);
        break;
      } catch (e) {
        if (e instanceof ApiError && e.status === 404) break; // Already confirmed.
        failed = e instanceof Error ? e.message : String(e);
        await new Promise((r) => setTimeout(r, 750));
      }
    }
    trying = null;
  }

  $effect(() => {
    if (pending && confirmed !== pending.revision) void confirm(pending.revision);
  });

  // Count down between status updates.
  $effect(() => {
    if (!pending) return;
    left = pending.seconds_left;
    const timer = setInterval(() => (left = Math.max(0, left - 1)), 1000);
    return () => clearInterval(timer);
  });
</script>

{#if pending && confirmed !== pending.revision}
  <div class="banner" role="status">
    <div class="text">
      <p class="strong">Checking that the firewall change in revision {pending.revision} keeps the dashboard reachable</p>
      <p class="small secondary">
        Unless it's confirmed, Hearthgate undoes it in {left} {left === 1 ? 'second' : 'seconds'}.
        {#if failed}The last try failed: {failed}{/if}
      </p>
    </div>
    <Button size="small" disabled={trying !== null} onclick={() => confirm(pending.revision)}>Keep the change</Button>
  </div>
{:else if confirmed !== null}
  <div class="banner done" role="status">
    <p class="strong">Firewall change confirmed. The dashboard can still reach Hearthgate.</p>
  </div>
{/if}

<style>
  .banner {
    display: flex;
    align-items: center;
    gap: var(--hg-spacing-lg);
    padding: var(--hg-spacing-md) var(--hg-spacing-lg);
    border: 1px solid var(--hg-color-status-warning-fg);
    border-radius: var(--hg-radius-lg);
    background: var(--hg-color-status-warning-bg);
    color: var(--hg-color-status-warning-text);
  }
  .banner.done {
    border-color: var(--hg-color-status-healthy-fg);
    background: var(--hg-color-status-healthy-bg);
    color: var(--hg-color-status-healthy-text);
  }
  .text {
    flex: 1;
    min-width: 0;
  }
  .text .secondary {
    color: inherit;
  }
</style>
