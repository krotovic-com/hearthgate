<script lang="ts">
  // A review's summary and findings, most severe first, or an all-clear.
  import FindingCard from './FindingCard.svelte';
  import type { AiReview } from '../api';
  import { providerLabel, scopeLabel, severityCounts } from '../ai';

  let { review }: { review: AiReview } = $props();
  const when = $derived(new Date(review.created_unix * 1000));
</script>

<div class="result">
  <p class="small secondary meta">
    {scopeLabel(review.scope)} · {when.toLocaleString()} · {providerLabel[review.provider]} ({review.model})
    {#if review.scheduled}· weekly review{/if}
    {#if review.revision}· revision {review.revision}{/if}
    · {severityCounts(review.findings)}
  </p>
  {#if review.summary}<p class="summary">{review.summary}</p>{/if}
  {#if review.findings.length === 0}
    <div class="clear" role="status">
      <p class="strong">All clear</p>
      <p class="small">Nothing needs your attention. No changes suggested.</p>
    </div>
  {:else}
    <div class="findings">
      {#each review.findings as finding, i (`${review.id}-${i}`)}
        <FindingCard {review} {finding} index={i} />
      {/each}
    </div>
  {/if}
</div>

<style>
  .result,
  .findings {
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-md);
  }
  .meta,
  .summary {
    margin: 0;
  }
  .clear {
    padding: var(--hg-spacing-lg) var(--hg-spacing-xl);
    background: var(--hg-color-status-healthy-bg);
    color: var(--hg-color-status-healthy-text);
    border-radius: var(--hg-radius-lg);
  }
  .clear p {
    margin: 0;
  }
</style>
