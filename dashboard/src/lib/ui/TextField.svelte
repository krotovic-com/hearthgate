<script lang="ts">
  import type { HTMLInputAttributes } from 'svelte/elements';

  type Props = Omit<HTMLInputAttributes, 'value'> & {
    label: string;
    value?: string;
    hint?: string;
    /** Keep the label for screen readers only. */
    hideLabel?: boolean;
    error?: string | null;
    /** Technical values (domains, ports, IPs, codes) use the mono style. */
    mono?: boolean;
  };
  let { label, hideLabel = false, value = $bindable(''), hint, error = null, mono = false, id, ...rest }: Props = $props();
  const uid = $props.id();
  const inputId = $derived(id ?? `field-${uid}`);
  const hintId = $derived(`${inputId}-hint`);
</script>

<div class="field" class:disabled={rest.disabled}>
  <label for={inputId} class="strong" class:visually-hidden={hideLabel}>{label}</label>
  <input
    id={inputId}
    class:mono
    class:invalid={!!error}
    aria-invalid={error ? 'true' : undefined}
    aria-describedby={error || hint ? hintId : undefined}
    bind:value
    {...rest}
  />
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
  .disabled {
    opacity: 0.6;
  }
  input {
    width: 100%;
    padding: calc(var(--hg-spacing-sm) - 1px) calc(var(--hg-spacing-md) - 1px);
    border: 1px solid var(--hg-color-border-strong);
    border-radius: var(--hg-radius-md);
    background: var(--hg-color-bg-surface);
    color: var(--hg-color-text-primary);
    font: 400 14px/20px var(--hg-font-body);
  }
  input.mono {
    font: 400 13px/20px var(--hg-font-mono);
  }
  input:focus {
    outline: none;
    border: 2px solid var(--hg-color-border-focus);
    padding: calc(var(--hg-spacing-sm) - 2px) calc(var(--hg-spacing-md) - 2px);
  }
  input.invalid {
    border-color: var(--hg-color-status-critical-fg);
  }
  input:disabled {
    background: var(--hg-color-bg-subtle);
    color: var(--hg-color-text-secondary);
  }
  .error {
    color: var(--hg-color-status-critical-fg);
  }
</style>
