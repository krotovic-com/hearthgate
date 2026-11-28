<script lang="ts">
  // One line of release notes with its kind (Figma: Updates / Change line).
  import Badge from '../ui/Badge.svelte';
  import type { ReleaseNote } from '../api';
  import { noteBadge } from '../updates';

  let { note }: { note: ReleaseNote } = $props();
  const badge = $derived(noteBadge[note.kind]);
</script>

<li class="line">
  <span class="kind"><Badge status={badge.status}>{badge.label}</Badge></span>
  <span>{note.text}</span>
</li>

<style>
  .line {
    display: flex;
    align-items: baseline;
    gap: var(--hg-spacing-md);
  }
  .kind {
    flex: 0 0 84px;
    display: flex;
  }
  .kind :global(.badge) {
    text-transform: uppercase;
    letter-spacing: 0.04em;
    font-size: 11px;
  }
</style>
