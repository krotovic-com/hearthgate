<script lang="ts">
  // One step of an install or rollback (Figma: Updates / plan step).
  import Icon from '../ui/Icon.svelte';
  import type { JobStep } from '../api';

  let { step, index }: { step: JobStep; index: number } = $props();
  const label: Record<JobStep['state'], string> = {
    pending: 'Not started',
    running: 'In progress',
    done: 'Done',
    failed: 'Failed',
    skipped: 'Skipped',
  };
</script>

<li class="step {step.state}" aria-current={step.state === 'running' ? 'step' : undefined}>
  <span class="mark" aria-hidden="true">
    {#if step.state === 'done'}<Icon name="check" />{:else if step.state === 'failed'}!{:else}{index + 1}{/if}
  </span>
  <div class="text">
    <p>{step.title}<span class="visually-hidden">: {label[step.state]}</span></p>
    {#if step.detail}<p class="small secondary">{step.detail}</p>{/if}
  </div>
</li>

<style>
  .step {
    display: flex;
    gap: var(--hg-spacing-md);
    padding: var(--hg-spacing-sm) var(--hg-spacing-md);
    border-radius: var(--hg-radius-md);
  }
  .mark {
    flex: 0 0 20px;
    height: 20px;
    margin-top: 1px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border-radius: var(--hg-radius-full);
    border: 1px solid var(--hg-color-border-strong);
    font: 500 11px/1 var(--hg-font-body);
    color: var(--hg-color-text-secondary);
  }
  .mark :global(.icon) {
    width: 12px;
    height: 12px;
  }
  .text {
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-2xs);
    min-width: 0;
  }
  .done .mark {
    border-color: transparent;
    background: var(--hg-color-status-healthy-bg);
    color: var(--hg-color-status-healthy-text);
  }
  .running {
    background: var(--hg-color-accent-subtle);
  }
  .running .mark {
    border-color: transparent;
    background: var(--hg-color-accent-default);
    color: var(--hg-color-text-on-accent);
  }
  .failed .mark {
    border-color: transparent;
    background: var(--hg-color-status-critical-bg);
    color: var(--hg-color-status-critical-text);
  }
  .pending .text p:first-child,
  .skipped .text p:first-child {
    color: var(--hg-color-text-secondary);
  }
  .skipped {
    opacity: 0.7;
  }
</style>
