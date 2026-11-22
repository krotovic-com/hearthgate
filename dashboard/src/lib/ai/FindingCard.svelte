<script lang="ts">
  // One finding: severity, what is wrong, and a fix to load into the draft.
  import Badge from '../ui/Badge.svelte';
  import Button from '../ui/Button.svelte';
  import type { AiFinding, AiReview } from '../api';
  import { changeLines, severityBadge } from '../ai';
  import { useFix } from '../ai.svelte';
  import { gw, isDirty } from '../gateway.svelte';

  let { review, finding, index }: { review: AiReview; finding: AiFinding; index: number } = $props();

  let busy = $state(false);
  let result = $state<{ ok: boolean; text: string } | null>(null);
  let dismissed = $state(false);
  const dirty = $derived(gw.loaded && isDirty());
  const badge = $derived(severityBadge[finding.severity]);

  async function use() {
    busy = true;
    result = null;
    try {
      const summary = await useFix(review, index);
      result = {
        ok: true,
        text: `${summary || 'The fix'} is in your draft. Review the changes and apply them, or discard them.`,
      };
    } catch (e) {
      result = { ok: false, text: e instanceof Error ? e.message : String(e) };
    } finally {
      busy = false;
    }
  }
</script>

{#if !dismissed}
  <article class="finding {finding.severity}" aria-labelledby="f-{review.id}-{index}">
    <div class="meta">
      <Badge status={badge.status} dot>{badge.label}</Badge>
      {#if finding.area}<span class="label secondary area">{finding.area}</span>{/if}
    </div>
    <h3 id="f-{review.id}-{index}">{finding.title}</h3>
    {#if finding.detail}<p class="small secondary detail">{finding.detail}</p>{/if}
    {#if finding.fix}
      <div class="fix">
        <p class="small">{finding.fix.summary}</p>
        <pre class="mono-small changes">{#each changeLines(finding.fix.changes) as line, i (i)}<span class={line.sign === '-' ? 'del' : 'add'}>{line.sign} {line.text}</span>
{/each}</pre>
      </div>
    {/if}
    <div class="actions">
      {#if finding.fix}
        <Button size="small" {busy} disabled={dirty} onclick={use}>Put fix in draft</Button>
      {/if}
      <Button variant="ghost" size="small" onclick={() => (dismissed = true)}>Dismiss</Button>
      {#if finding.fix && dirty && !result}
        <span class="small secondary">Apply or discard your current draft first.</span>
      {/if}
    </div>
    {#if result}
      <p class="small" class:problem={!result.ok} role="status">{result.text}</p>
    {/if}
  </article>
{/if}

<style>
  .finding {
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-sm);
    padding: var(--hg-spacing-lg) var(--hg-spacing-xl);
    background: var(--hg-color-bg-surface);
    border: 1px solid var(--hg-color-border-default);
    border-left: 3px solid var(--hg-color-border-default);
    border-radius: var(--hg-radius-lg);
  }
  .finding.high {
    border-left-color: var(--hg-color-status-critical-fg);
  }
  .finding.medium {
    border-left-color: var(--hg-color-status-warning-fg);
  }
  .meta {
    display: flex;
    gap: var(--hg-spacing-sm);
    align-items: center;
  }
  .area {
    text-transform: uppercase;
  }
  h3 {
    margin: 0;
    font-size: 1rem;
  }
  .detail {
    white-space: pre-line;
    margin: 0;
  }
  .fix {
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-xs);
  }
  .fix p {
    margin: 0;
  }
  .changes {
    margin: 0;
    padding: var(--hg-spacing-sm) var(--hg-spacing-md);
    background: var(--hg-color-bg-subtle);
    border-radius: var(--hg-radius-md);
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
  .actions {
    display: flex;
    gap: var(--hg-spacing-sm);
    align-items: center;
    flex-wrap: wrap;
  }
  .problem {
    color: var(--hg-color-status-critical-text);
  }
</style>
