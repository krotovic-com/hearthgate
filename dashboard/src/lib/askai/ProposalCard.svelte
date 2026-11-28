<script lang="ts">
  // A change the assistant proposes. Nothing happens until the user picks
  // Apply (a new revision, which can be undone), Open in form (into the
  // draft, to check or edit it in the normal form first) or Skip.
  import Badge from '../ui/Badge.svelte';
  import Button from '../ui/Button.svelte';
  import Icon from '../ui/Icon.svelte';
  import { changeLines } from '../ai';
  import { kindLabel } from '../askai';
  import { gw, isDirty } from '../gateway.svelte';
  import {
    applyProposal,
    openProposal,
    restoreProposal,
    skipProposal,
    undoProposal,
    type UiProposal,
  } from '../askai.svelte';

  let { proposal, item, index }: { proposal: UiProposal; item: number; index: number } = $props();

  let busy = $state<'apply' | 'open' | 'undo' | null>(null);
  let problem = $state<string | null>(null);
  const dirty = $derived(gw.loaded && isDirty());
  const canUndo = $derived(proposal.state === 'applied' && gw.revision?.id === proposal.revision);
  const titleId = $derived(`proposal-${item}-${index}`);

  async function run(kind: 'apply' | 'open' | 'undo', action: () => Promise<void>) {
    busy = kind;
    problem = null;
    try {
      await action();
    } catch (e) {
      problem = e instanceof Error ? e.message : String(e);
    } finally {
      busy = null;
    }
  }
</script>

<article class="proposal {proposal.state}" aria-labelledby={titleId}>
  <div class="meta">
    <span class="label secondary">Step {proposal.step} · {kindLabel[proposal.kind]}</span>
    {#if proposal.state === 'applied'}
      <Badge status="healthy" dot>Applied{proposal.revision ? ` · revision ${proposal.revision}` : ''}</Badge>
    {:else if proposal.state === 'draft'}
      <Badge status="neutral" dot>In your draft</Badge>
    {:else if proposal.state === 'skipped'}
      <Badge status="neutral">Skipped</Badge>
    {:else if proposal.state === 'undone'}
      <Badge status="neutral">Undone</Badge>
    {:else}
      <Badge status="warning" dot>Needs your OK</Badge>
    {/if}
  </div>
  <h3 id={titleId} class="strong">{proposal.title}</h3>
  {#if proposal.summary}<p class="small secondary summary">{proposal.summary}</p>{/if}
  {#if proposal.state !== 'skipped'}
    <pre class="mono-small changes">{#each changeLines(proposal.changes) as line, i (i)}<span
          class={line.sign === '-' ? 'del' : line.sign === '+' ? 'add' : 'mod'}>{line.sign} {line.text}</span
        >
{/each}</pre>
  {/if}
  <div class="actions">
    {#if proposal.state === 'pending'}
      <Button
        size="small"
        busy={busy === 'apply'}
        disabled={dirty || busy !== null}
        onclick={() => run('apply', () => applyProposal(item, index))}><Icon name="check" />Apply</Button
      >
      <Button
        variant="secondary"
        size="small"
        busy={busy === 'open'}
        disabled={busy !== null}
        onclick={() => run('open', () => openProposal(item, index))}>Open in form</Button
      >
      <Button variant="ghost" size="small" disabled={busy !== null} onclick={() => skipProposal(item, index)}
        >Skip</Button
      >
    {:else if canUndo}
      <Button
        variant="secondary"
        size="small"
        busy={busy === 'undo'}
        onclick={() => run('undo', () => undoProposal(item, index))}><Icon name="undo" />Undo</Button
      >
    {:else if proposal.state === 'skipped' || proposal.state === 'undone'}
      <Button variant="ghost" size="small" onclick={() => restoreProposal(item, index)}>Show again</Button>
    {/if}
  </div>
  {#if proposal.state === 'pending' && dirty && !problem}
    <p class="small secondary">Your draft has other changes. Apply or discard them first, or use Open in form.</p>
  {/if}
  {#if problem}<p class="small problem" role="alert">{problem}</p>{/if}
</article>

<style>
  .proposal {
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-xs);
    padding: var(--hg-spacing-md);
    background: var(--hg-color-bg-surface);
    border: 1px solid var(--hg-color-border-default);
    border-left: 3px solid var(--hg-color-accent-default);
    border-radius: var(--hg-radius-md);
  }
  .proposal.applied {
    border-left-color: var(--hg-color-status-healthy-fg);
  }
  .proposal.skipped,
  .proposal.undone,
  .proposal.draft {
    border-left-color: var(--hg-color-border-strong);
  }
  .meta {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: var(--hg-spacing-sm);
  }
  .meta .label {
    text-transform: uppercase;
  }
  h3 {
    margin: 0;
    font-size: 0.9375rem;
  }
  .summary {
    margin: 0;
    white-space: pre-line;
  }
  .changes {
    margin: 0;
    padding: var(--hg-spacing-sm) var(--hg-spacing-md);
    background: var(--hg-color-bg-subtle);
    border-radius: var(--hg-radius-sm, 4px);
    overflow-x: auto;
    white-space: pre-wrap;
    word-break: break-word;
  }
  .add {
    color: var(--hg-color-status-healthy-text);
  }
  .del {
    color: var(--hg-color-status-critical-text);
  }
  .mod {
    color: var(--hg-color-status-warning-text);
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: var(--hg-spacing-xs);
    margin-top: var(--hg-spacing-2xs);
  }
  .actions:empty {
    display: none;
  }
  p {
    margin: 0;
  }
  .problem {
    color: var(--hg-color-status-critical-text);
  }
</style>
