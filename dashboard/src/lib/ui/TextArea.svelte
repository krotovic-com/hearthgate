<script lang="ts">
  import type { HTMLTextareaAttributes } from 'svelte/elements';

  type Props = Omit<HTMLTextareaAttributes, 'value'> & {
    label: string;
    value?: string;
    hint?: string;
    error?: string | null;
    /** Code and technical values use the mono style. */
    mono?: boolean;
  };
  let { label, value = $bindable(''), hint, error = null, mono = false, id, rows = 4, ...rest }: Props = $props();
  const uid = $props.id();
  const areaId = $derived(id ?? `area-${uid}`);
  const hintId = $derived(`${areaId}-hint`);
</script>

<div class="field">
  <label for={areaId} class="strong">{label}</label>
  <textarea
    id={areaId}
    class:mono
    class:invalid={!!error}
    aria-invalid={error ? 'true' : undefined}
    aria-describedby={error || hint ? hintId : undefined}
    spellcheck={mono ? 'false' : undefined}
    {rows}
    bind:value
    {...rest}
  ></textarea>
  {#if error}
    <p id={hintId} class="small error" role="alert">{error}</p>
  {:else if hint}
    <p id={hintId} class="small secondary">{hint}</p>
  {/if}
</div>

<style>
  .field {
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-xs);
    width: 100%;
  }
  textarea {
    width: 100%;
    resize: vertical;
    padding: calc(var(--hg-spacing-sm) - 1px) calc(var(--hg-spacing-md) - 1px);
    border: 1px solid var(--hg-color-border-strong);
    border-radius: var(--hg-radius-md);
    background: var(--hg-color-bg-surface);
    color: var(--hg-color-text-primary);
    font: 400 14px/20px var(--hg-font-body);
  }
  textarea.mono {
    font: 400 13px/20px var(--hg-font-mono);
    tab-size: 2;
  }
  textarea:focus {
    outline: none;
    border: 2px solid var(--hg-color-border-focus);
    padding: calc(var(--hg-spacing-sm) - 2px) calc(var(--hg-spacing-md) - 2px);
  }
  textarea.invalid {
    border-color: var(--hg-color-status-critical-fg);
  }
  .error {
    color: var(--hg-color-status-critical-fg);
  }
</style>
