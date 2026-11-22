<script lang="ts">
  // "Review with AI" for one scope, in a dialog: the firewall's rules or
  // why a service is unhealthy. Shows the last result and runs a new one.
  import Dialog from '../ui/Dialog.svelte';
  import Button from '../ui/Button.svelte';
  import Alert from '../ui/Alert.svelte';
  import ReviewResult from './ReviewResult.svelte';
  import PayloadPreview from './PayloadPreview.svelte';
  import type { AiScope } from '../api';
  import { latestFor } from '../ai';
  import { ai, isRunning, loadAi, runReview } from '../ai.svelte';

  let {
    open = $bindable(false),
    scope,
    title,
    description,
  }: { open?: boolean; scope: AiScope; title: string; description: string } = $props();

  $effect(() => {
    if (open && !ai.loaded) loadAi();
  });

  const review = $derived(latestFor(ai.reviews, scope));
  const enabled = $derived(ai.info?.enabled ?? false);
  const running = $derived(isRunning(scope));
  let previewOpen = $state(false);
</script>

<Dialog bind:open {title} {description} wide>
  <div class="body">
    {#if ai.loaded && !enabled}
      <Alert tone="neutral">
        The AI assistant is off, so nothing is sent anywhere. Turn it on in <a href="/settings?section=ai">Settings, AI assistant</a>.
      </Alert>
    {/if}
    {#if ai.error}<Alert tone="critical">{ai.error}</Alert>{/if}
    {#if running}
      <p class="small secondary" role="status">Asking the model. This can take a minute…</p>
    {:else if review}
      <ReviewResult {review} />
    {:else if enabled}
      <p class="small secondary">No review yet. Data is redacted before it is sent.</p>
    {/if}
    {#if previewOpen}<PayloadPreview {scope} />{/if}
  </div>
  {#snippet footer()}
    <Button variant="ghost" onclick={() => (previewOpen = !previewOpen)}>
      {previewOpen ? 'Hide what is sent' : 'Show what is sent'}
    </Button>
    <Button variant="secondary" onclick={() => (open = false)}>Close</Button>
    <Button busy={running} disabled={!enabled || running} onclick={() => runReview(scope)}>
      {review ? 'Review again' : 'Review now'}
    </Button>
  {/snippet}
</Dialog>

<style>
  .body {
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-md);
  }
</style>
