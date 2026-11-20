<script lang="ts">
  let {
    checked = $bindable(false),
    label,
    disabled = false,
    onchange,
  }: { checked?: boolean; label: string; disabled?: boolean; onchange?: (on: boolean) => void } = $props();
</script>

<button
  type="button"
  role="switch"
  class="toggle"
  aria-checked={checked}
  aria-label={label}
  {disabled}
  onclick={() => {
    checked = !checked;
    onchange?.(checked);
  }}
>
  <span class="knob"></span>
</button>

<style>
  .toggle {
    position: relative;
    width: 36px;
    height: 20px;
    padding: 0;
    border: none;
    border-radius: var(--hg-radius-full);
    background: var(--hg-color-border-strong);
    cursor: pointer;
    flex-shrink: 0;
  }
  .toggle[aria-checked='true'] {
    background: var(--hg-color-accent-default);
  }
  .toggle:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }
  .knob {
    position: absolute;
    top: 2px;
    left: 2px;
    width: 16px;
    height: 16px;
    border-radius: var(--hg-radius-full);
    background: var(--hg-white);
    transition: transform 120ms ease;
  }
  .toggle[aria-checked='true'] .knob {
    transform: translateX(16px);
  }
</style>
