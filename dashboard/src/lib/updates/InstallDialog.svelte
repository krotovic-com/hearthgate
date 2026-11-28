<script lang="ts">
  // "Install Hearthgate X": what's new, what happens, and when.
  import Dialog from '../ui/Dialog.svelte';
  import Button from '../ui/Button.svelte';
  import ChangeLine from './ChangeLine.svelte';
  import PlanStep from './PlanStep.svelte';
  import type { AvailableUpdate, JobStep, UpdateInfo } from '../api';
  import { bumpSentence, windowStart } from '../updates';
  import { plural } from '../format';

  let {
    open = $bindable(false),
    info,
    available,
    busy = false,
    oninstall,
  }: {
    open?: boolean;
    info: UpdateInfo;
    available: AvailableUpdate;
    busy?: boolean;
    oninstall: (when: 'now' | 'window') => void;
  } = $props();

  let showAll = $state(false);
  const notes = $derived(showAll ? available.notes : available.notes.slice(0, 5));
  const v = $derived(available.version);
  const cur = $derived(info.current);
  const how = $derived<Record<string, string>>({
    deb: `apt installs ${v} so the package manager stays in sync`,
    rpm: `The package manager installs ${v}`,
    tarball: 'The binary and its service file are replaced',
  });
  const steps = $derived<JobStep[]>([
    { title: 'Download and verify', detail: `Signature and SHA-256 checksum of ${available.asset ?? 'the release'}`, state: 'pending' },
    { title: `Back up your settings and ${cur}`, detail: 'The settings and the current binary are kept for rollback', state: 'pending' },
    { title: info.method === 'tarball' ? 'Install the new binary' : 'Install the package', detail: how[info.method] ?? '', state: 'pending' },
    { title: 'Restart Hearthgate', detail: 'Public traffic pauses for a few seconds', state: 'pending' },
    { title: 'Health check', detail: `Proxy, firewall, VPN and dashboard must come up within 2 minutes, or ${cur} comes back`, state: 'pending' },
  ]);
  const breaking = $derived(available.notes.some((n) => n.kind === 'breaking'));
</script>

<Dialog bind:open title="Install Hearthgate {v}" wide>
  <p class="secondary">{bumpSentence(cur, available)}</p>

  <h3 class="eyebrow">What’s new · {plural(available.notes.length, 'change')}</h3>
  {#if available.notes.length}
    <ul class="notes">
      {#each notes as note, i (i)}
        <ChangeLine {note} />
      {/each}
    </ul>
  {/if}
  <p class="small">
    {#if available.notes.length > 5 && !showAll}
      <button type="button" class="link" onclick={() => (showAll = true)}>And {available.notes.length - 5} more</button> ·
    {/if}
    <a href={available.page} target="_blank" rel="noreferrer">full release notes on GitHub</a>
  </p>

  <div class="verify small">
    Before anything changes, the download is checked against the Hearthgate release key built into this version, then
    checked again by the upgrade helper. A file that doesn’t match is never installed.
  </div>

  <h3 class="eyebrow">What happens</h3>
  <ol class="plan">
    {#each steps as step, i (i)}
      <PlanStep {step} index={i} />
    {/each}
  </ol>

  <div class="note small">
    The dashboard reconnects by itself after the restart. You can leave this page; the install keeps going on the
    server.
  </div>

  {#snippet footer()}
    <Button busy={busy} onclick={() => oninstall('now')}>{breaking ? 'I read it, install now' : 'Install now'}</Button>
    <Button variant="secondary" disabled={busy} onclick={() => oninstall('window')}>
      Install tonight at {windowStart(info.settings.window)}
    </Button>
    <Button variant="ghost" disabled={busy} onclick={() => (open = false)}>Cancel</Button>
  {/snippet}
</Dialog>

<style>
  .eyebrow {
    margin-top: var(--hg-spacing-lg);
    font: 500 12px/16px var(--hg-font-body);
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--hg-color-text-secondary);
  }
  .notes,
  .plan {
    list-style: none;
    margin: var(--hg-spacing-sm) 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-sm);
  }
  .plan {
    gap: 0;
  }
  .link {
    background: none;
    border: 0;
    padding: 0;
    font: inherit;
    color: var(--hg-color-text-link);
    cursor: pointer;
  }
  .verify,
  .note {
    margin-top: var(--hg-spacing-lg);
    padding: var(--hg-spacing-md) var(--hg-spacing-lg);
    border-radius: var(--hg-radius-md);
  }
  .verify {
    background: var(--hg-color-status-healthy-bg);
    color: var(--hg-color-status-healthy-text);
  }
  .note {
    background: var(--hg-color-accent-subtle);
    color: var(--hg-color-text-primary);
  }
</style>
