<script lang="ts">
  import Dialog from '../ui/Dialog.svelte';
  import Button from '../ui/Button.svelte';
  import TextField from '../ui/TextField.svelte';
  import Alert from '../ui/Alert.svelte';
  import { api, type ContainerLogLine, type ContainerStats } from '../api';
  import { bytes } from '../vpn';

  let { open = $bindable(false), name }: { open?: boolean; name: string } = $props();

  let logs = $state<ContainerLogLine[]>([]);
  let stats = $state<ContainerStats | null>(null);
  let search = $state('');
  let error = $state<string | null>(null);
  let busy = $state(false);
  let output: HTMLElement | undefined = $state();

  async function load() {
    busy = true;
    try {
      [logs, stats] = await Promise.all([api.containerLogs(name, 300, search.trim()), api.containerStats(name).catch(() => null)]);
      error = null;
      queueMicrotask(() => output?.scrollTo({ top: output.scrollHeight }));
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      busy = false;
    }
  }

  $effect(() => {
    if (!open || !name) return;
    search = '';
    load();
    const timer = setInterval(load, 5000);
    return () => clearInterval(timer);
  });

  const time = (t: string) => t.replace(/^\d{4}-\d\d-\d\dT/, '').replace(/\.\d+Z$/, '');
</script>

<Dialog bind:open wide title="Logs of {name}">
  {#if stats}
    <p class="small secondary">
      CPU {stats.cpu_percent.toFixed(1)}% · memory {bytes(stats.memory_bytes)}{stats.memory_limit ? ` of ${bytes(stats.memory_limit)}` : ''} ·
      {stats.restarts === 1 ? '1 restart' : `${stats.restarts} restarts`}
    </p>
  {/if}
  <form
    class="search"
    onsubmit={(e) => {
      e.preventDefault();
      load();
    }}
  >
    <TextField label="Search" hideLabel bind:value={search} placeholder="Search the last 300 lines" />
    <Button variant="secondary" type="submit" {busy}>Search</Button>
  </form>
  {#if error}<Alert>{error}</Alert>{/if}
  <!-- Focusable so the output can be scrolled with the keyboard. -->
  <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
  <div class="output" bind:this={output} tabindex="0" role="log" aria-label="Output of {name}">
    {#each logs as line, i (i)}
      <div class="line" class:stderr={line.stderr}><span class="time">{time(line.time)}</span> {line.text}</div>
    {:else}
      <p class="small secondary">{busy ? 'Loading…' : 'No output.'}</p>
    {/each}
  </div>
  {#snippet footer()}
    <Button variant="secondary" onclick={() => (open = false)}>Close</Button>
  {/snippet}
</Dialog>

<style>
  .search {
    display: flex;
    gap: var(--hg-spacing-sm);
    align-items: flex-start;
  }
  .output {
    height: 360px;
    overflow: auto;
    padding: var(--hg-spacing-md);
    border-radius: var(--hg-radius-md);
    background: var(--hg-color-bg-canvas);
    border: 1px solid var(--hg-color-border-default);
    font: 400 12px/18px var(--hg-font-mono);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .output:focus-visible {
    outline: 2px solid var(--hg-color-border-focus);
  }
  .time {
    color: var(--hg-color-text-secondary);
  }
  .stderr {
    color: var(--hg-color-status-critical-fg);
  }
</style>
