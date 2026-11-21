<script lang="ts">
  import { onMount } from 'svelte';
  import PageHeader from '../lib/layout/PageHeader.svelte';
  import StatCard from '../lib/ui/StatCard.svelte';
  import Button from '../lib/ui/Button.svelte';
  import Alert from '../lib/ui/Alert.svelte';
  import Card from '../lib/ui/Card.svelte';
  import FindingCard from '../lib/ai/FindingCard.svelte';
  import PayloadPreview from '../lib/ai/PayloadPreview.svelte';
  import type { AiSeverity } from '../lib/api';
  import { providerLabel, scopeLabel, severityCounts } from '../lib/ai';
  import { ai, isRunning, loadAi, runReview } from '../lib/ai.svelte';
  import { gw } from '../lib/gateway.svelte';
  import { navigate } from '../lib/router.svelte';

  onMount(() => {
    loadAi();
  });

  const all = { kind: 'all' } as const;
  const enabled = $derived(ai.info?.enabled ?? false);
  const running = $derived(isRunning(all));
  let selectedId = $state<string | null>(null);
  const review = $derived(
    ai.reviews.find((r) => r.id === selectedId) ?? ai.reviews.find((r) => r.scope.kind === 'all') ?? null,
  );
  let filter = $state<AiSeverity | 'all'>('all');
  const shown = $derived(
    review ? review.findings.map((f, i) => ({ f, i })).filter(({ f }) => filter === 'all' || f.severity === filter) : [],
  );
  const weekly = $derived(gw.current.ai?.weekly_review ?? false);
  let previewOpen = $state(false);

  async function run() {
    const r = await runReview(all);
    if (r) {
      selectedId = r.id;
      filter = 'all';
    }
  }

  const count = (s: AiSeverity) => review?.findings.filter((f) => f.severity === s).length ?? 0;
</script>

<PageHeader
  title="AI review"
  subtitle="An AI model checks the whole setup and suggests how to make it safer and more reliable. Nothing changes until you apply it."
>
  {#snippet actions()}
    <Button variant="secondary" onclick={() => navigate('/settings?section=ai')}>
      {weekly ? 'Weekly review on' : 'Settings'}
    </Button>
    <Button busy={running} disabled={!enabled || running} onclick={run}>Run review now</Button>
  {/snippet}
</PageHeader>

{#if ai.loaded && !enabled}
  <Card title="The AI assistant is off">
    <p class="small secondary">
      While it is off, Hearthgate never connects to an AI provider. Turn it on in Settings, choose Claude, OpenAI, Gemini
      or a local model such as Ollama, and see exactly what would be sent before anything leaves this machine.
      Addresses, domains and secrets are always replaced by placeholders.
    </p>
    {#snippet actions()}
      <Button onclick={() => navigate('/settings?section=ai')}>Set up the assistant</Button>
    {/snippet}
  </Card>
{/if}

{#if ai.error}<Alert tone="critical">{ai.error}</Alert>{/if}
{#if running}<p class="small secondary" role="status">Reviewing the setup. This can take a minute…</p>{/if}

{#if review}
  <div class="stats">
    <StatCard label="Findings" value={review.findings.length} detail={severityCounts(review.findings)} />
    <StatCard
      label="Last review"
      value={new Date(review.created_unix * 1000).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}
      detail={new Date(review.created_unix * 1000).toLocaleDateString()}
    />
    <StatCard label="Model" value={review.model} detail={providerLabel[review.provider]} />
    <StatCard
      label="Revision"
      value={review.revision ?? '–'}
      detail={review.scheduled ? 'Weekly review' : 'Started by hand'}
    />
  </div>

  <div class="layout">
    <div class="main">
      <div class="tabs" role="group" aria-label="Show findings">
        {#each [['all', `All ${review.findings.length}`], ['high', `High ${count('high')}`], ['medium', `Medium ${count('medium')}`], ['low', `Low ${count('low')}`]] as [value, label] (value)}
          <button
            type="button"
            class="tab small"
            aria-pressed={filter === value}
            onclick={() => (filter = value as AiSeverity | 'all')}>{label}</button
          >
        {/each}
      </div>
      {#if review.summary}<p class="summary">{review.summary}</p>{/if}
      {#if review.findings.length === 0}
        <div class="clear" role="status">
          <p class="strong">All clear</p>
          <p class="small">The model found nothing that needs your attention.</p>
        </div>
      {:else}
        {#each shown as { f, i } (`${review.id}-${i}`)}
          <FindingCard {review} finding={f} index={i} />
        {:else}
          <p class="small secondary">No findings at this level.</p>
        {/each}
      {/if}
    </div>
    <aside class="side">
      <Card title="Past reviews">
        <ul class="past">
          {#each ai.reviews.slice(0, 10) as r (r.id)}
            <li>
              <button type="button" class="link small" aria-current={r.id === review.id ? 'true' : undefined} onclick={() => (selectedId = r.id)}>
                {new Date(r.created_unix * 1000).toLocaleString()}
              </button>
              <span class="small secondary">{scopeLabel(r.scope)} · {severityCounts(r.findings)}</span>
            </li>
          {/each}
        </ul>
      </Card>
      <div class="note small">
        <p class="strong">{providerLabel[review.provider]}</p>
        <p>
          Data was redacted before sending: addresses, domains and secrets were replaced with placeholders, and put back
          only here.
        </p>
        <Button variant="ghost" size="small" onclick={() => (previewOpen = !previewOpen)}>
          {previewOpen ? 'Hide what is sent' : 'Show what is sent'}
        </Button>
      </div>
    </aside>
  </div>
{:else if ai.loaded && enabled && !running}
  <Card title="No review yet">
    <p class="small secondary">Run the first review to see findings ranked by severity, each with a fix you can review.</p>
    {#snippet actions()}
      <Button variant="ghost" onclick={() => (previewOpen = !previewOpen)}>Show what is sent</Button>
    {/snippet}
  </Card>
{/if}

{#if previewOpen}
  <Card title="What a review sends">
    <PayloadPreview scope={all} />
  </Card>
{/if}

<style>
  .stats {
    display: grid;
    grid-template-columns: repeat(4, minmax(0, 1fr));
    gap: var(--hg-spacing-lg);
    margin-bottom: var(--hg-spacing-xl);
  }
  .layout {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 320px;
    gap: var(--hg-spacing-xl);
    align-items: start;
  }
  .main,
  .side {
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-md);
  }
  .tabs {
    display: inline-flex;
    gap: var(--hg-spacing-2xs);
    padding: var(--hg-spacing-2xs);
    background: var(--hg-color-bg-subtle);
    border-radius: var(--hg-radius-md);
    align-self: flex-start;
  }
  .tab {
    border: 0;
    background: transparent;
    padding: var(--hg-spacing-2xs) var(--hg-spacing-md);
    border-radius: var(--hg-radius-sm);
    color: var(--hg-color-text-body);
    cursor: pointer;
  }
  .tab[aria-pressed='true'] {
    background: var(--hg-color-bg-surface);
    color: var(--hg-color-text-primary);
  }
  .summary {
    margin: 0;
  }
  .clear {
    padding: var(--hg-spacing-lg) var(--hg-spacing-xl);
    background: var(--hg-color-status-healthy-bg);
    color: var(--hg-color-status-healthy-text);
    border-radius: var(--hg-radius-lg);
  }
  .clear p,
  .note p {
    margin: 0;
  }
  .past {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-sm);
  }
  .past li {
    display: flex;
    flex-direction: column;
  }
  .link {
    border: 0;
    background: none;
    padding: 0;
    text-align: left;
    color: var(--hg-color-text-link);
    cursor: pointer;
  }
  .link[aria-current='true'] {
    font-weight: 600;
  }
  .note {
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-xs);
    padding: var(--hg-spacing-md) var(--hg-spacing-lg);
    background: var(--hg-color-accent-subtle);
    border-radius: var(--hg-radius-lg);
  }
  @media (max-width: 1100px) {
    .stats {
      grid-template-columns: repeat(2, minmax(0, 1fr));
    }
    .layout {
      grid-template-columns: minmax(0, 1fr);
    }
  }
</style>
