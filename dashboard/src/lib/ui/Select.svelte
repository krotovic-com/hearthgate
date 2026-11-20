<script lang="ts" generics="T extends string | number">
  import type { HTMLSelectAttributes } from 'svelte/elements';
  import Icon from './Icon.svelte';

  type Props = Omit<HTMLSelectAttributes, 'value'> & {
    label: string;
    value: T;
    options: { value: T; label: string }[];
    hint?: string;
    /** Keep the label for screen readers only. */
    hideLabel?: boolean;
  };
  let { label, hideLabel = false, value = $bindable(), options, hint, id, ...rest }: Props = $props();
  const uid = $props.id();
  const selectId = $derived(id ?? `select-${uid}`);
</script>

<div class="field">
  <label for={selectId} class="strong" class:visually-hidden={hideLabel}>{label}</label>
  <div class="wrap">
    <select id={selectId} bind:value aria-describedby={hint ? `${selectId}-hint` : undefined} {...rest}>
      {#each options as option (option.value)}
        <option value={option.value}>{option.label}</option>
      {/each}
    </select>
    <span class="chevron" aria-hidden="true"><Icon name="chevron" /></span>
  </div>
  {#if hint}<p id="{selectId}-hint" class="small secondary">{hint}</p>{/if}
</div>

<style>
  .field {
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-xs);
    width: 100%;
  }
  .wrap {
    position: relative;
  }
  select {
    appearance: none;
    width: 100%;
    padding: calc(var(--hg-spacing-sm) - 1px) 36px calc(var(--hg-spacing-sm) - 1px) calc(var(--hg-spacing-md) - 1px);
    border: 1px solid var(--hg-color-border-strong);
    border-radius: var(--hg-radius-md);
    background: var(--hg-color-bg-surface);
    color: var(--hg-color-text-primary);
    font: 400 14px/20px var(--hg-font-body);
    cursor: pointer;
  }
  select:focus {
    outline: none;
    border-color: var(--hg-color-border-focus);
    box-shadow: 0 0 0 1px var(--hg-color-border-focus);
  }
  .chevron {
    position: absolute;
    right: 12px;
    top: 50%;
    transform: translateY(-50%);
    display: flex;
    pointer-events: none;
    color: var(--hg-color-text-secondary);
  }
</style>
