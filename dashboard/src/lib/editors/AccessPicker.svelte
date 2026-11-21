<script lang="ts">
  // What a VPN device may reach: everything, or a few services.
  import type { Access } from '../api';

  let { value = $bindable('all'), services }: { value: Access; services: string[] } = $props();
  const uid = $props.id();

  const only = $derived(value !== 'all');
  const chosen = $derived(value === 'all' ? [] : value);

  function toggle(name: string, on: boolean) {
    const next = chosen.filter((n) => n !== name);
    if (on) next.push(name);
    value = next.sort();
  }
</script>

<fieldset>
  <legend class="strong">Can reach</legend>
  <label class="choice">
    <input type="radio" name="access-{uid}" checked={!only} onchange={() => (value = 'all')} />
    <span>
      <span>Everything</span>
      <span class="small secondary">This machine, the local network and every service.</span>
    </span>
  </label>
  <label class="choice">
    <input type="radio" name="access-{uid}" checked={only} onchange={() => (value = [...chosen])} />
    <span>
      <span>Only some services</span>
      <span class="small secondary">Their ports and nothing else.</span>
    </span>
  </label>
  {#if only}
    {#if services.length === 0}
      <p class="small secondary nested">Add a service first.</p>
    {:else}
      <div class="services nested">
        {#each services as name (name)}
          <label class="check">
            <input type="checkbox" checked={chosen.includes(name)} onchange={(e) => toggle(name, e.currentTarget.checked)} />
            <span class="mono-small">{name}</span>
          </label>
        {/each}
      </div>
      {#if chosen.length === 0}<p class="small error nested" role="alert">Choose at least one service.</p>{/if}
    {/if}
  {/if}
</fieldset>

<style>
  fieldset {
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-sm);
    margin: 0;
    padding: 0;
    border: none;
    min-width: 0;
  }
  legend {
    padding: 0;
    margin-bottom: var(--hg-spacing-xs);
  }
  .choice,
  .check {
    display: flex;
    gap: var(--hg-spacing-sm);
    align-items: flex-start;
    cursor: pointer;
  }
  .choice > span {
    display: flex;
    flex-direction: column;
  }
  input {
    margin: 3px 0 0;
    accent-color: var(--hg-color-accent-default);
  }
  .nested {
    margin-left: 22px;
  }
  .services {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(140px, 1fr));
    gap: var(--hg-spacing-xs) var(--hg-spacing-md);
  }
  .check {
    align-items: center;
  }
  .check input {
    margin: 0;
  }
  .error {
    color: var(--hg-color-status-critical-fg);
  }
</style>
