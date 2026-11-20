<script lang="ts">
  // Shown while the draft differs from what is running: review the diff,
  // then apply it as a new revision, or throw it away.
  import Button from '../ui/Button.svelte';
  import Dialog from '../ui/Dialog.svelte';
  import DiffView from '../ui/DiffView.svelte';
  import TextField from '../ui/TextField.svelte';
  import Alert from '../ui/Alert.svelte';
  import { api, ApiError, type PreviewResponse } from '../api';
  import { gw, isDirty, draftSummary, applyDraft, discard } from '../gateway.svelte';
  import { plural } from '../format';

  let open = $state(false);
  let preview = $state<PreviewResponse | null>(null);
  let message = $state('');
  let error = $state<string | null>(null);
  let problems = $state<string[]>([]);
  let busy = $state(false);
  let done = $state<string | null>(null);

  const dirty = $derived(gw.loaded && isDirty());
  const summary = $derived(dirty ? draftSummary() : []);

  async function review() {
    error = null;
    problems = [];
    preview = null;
    message = summary.length === 1 ? summary[0]! : summary.join(', ');
    open = true;
    try {
      preview = await api.preview($state.snapshot(gw.draft));
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }

  async function apply() {
    busy = true;
    error = null;
    try {
      const revision = await applyDraft(message.trim() || 'Change from the dashboard');
      open = false;
      done = `Applied as revision ${revision.id}.`;
      setTimeout(() => (done = null), 6000);
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
      problems = e instanceof ApiError ? (e.problem.problems ?? []) : [];
    } finally {
      busy = false;
    }
  }
</script>

{#if dirty}
  <div class="bar" role="region" aria-label="Unapplied changes">
    <div class="text">
      <p class="strong">{plural(summary.length || 1, 'change')} not applied yet</p>
      <p class="small secondary">{summary.slice(0, 3).join(' · ')}{summary.length > 3 ? ' · …' : ''}</p>
    </div>
    <Button variant="ghost" onclick={discard}>Discard</Button>
    <Button onclick={review}>Review and apply</Button>
  </div>
{:else if done}
  <div class="bar done" role="status">
    <p class="strong">{done}</p>
    <a href="/history">See history</a>
  </div>
{/if}

<Dialog bind:open title="Review changes" description="This is what changes in the configuration. Nothing is applied until you confirm." wide>
  {#if preview}
    {#if !preview.valid}
      <Alert>
        <p class="strong">Fix these before applying:</p>
        <ul>
          {#each preview.problems as p (p)}<li>{p}</li>{/each}
        </ul>
      </Alert>
    {/if}
    <DiffView diff={preview.diff} />
  {:else if !error}
    <p class="secondary">Comparing with the running configuration…</p>
  {/if}
  {#if error}
    <Alert>
      <p>{error}</p>
      {#if problems.length}<ul>{#each problems as p (p)}<li>{p}</li>{/each}</ul>{/if}
    </Alert>
  {/if}
  <TextField label="Note for the history" bind:value={message} maxlength={200} />
  {#snippet footer()}
    <Button variant="secondary" onclick={() => (open = false)}>Keep editing</Button>
    <Button onclick={apply} {busy} disabled={!preview?.valid || !preview?.changed}>Apply changes</Button>
  {/snippet}
</Dialog>

<style>
  .bar {
    position: sticky;
    bottom: var(--hg-spacing-lg);
    z-index: 5;
    display: flex;
    align-items: center;
    gap: var(--hg-spacing-md);
    padding: var(--hg-spacing-md) var(--hg-spacing-lg);
    margin-top: auto;
    background: var(--hg-color-bg-surface);
    border: 1px solid var(--hg-color-accent-default);
    border-radius: var(--hg-radius-lg);
    box-shadow: 0 8px 24px rgb(11 20 38 / 0.12);
  }
  .bar.done {
    border-color: var(--hg-color-status-healthy-fg);
  }
  .text {
    flex: 1;
    min-width: 0;
  }
  ul {
    margin: var(--hg-spacing-xs) 0 0;
    padding-left: var(--hg-spacing-xl);
  }
</style>
