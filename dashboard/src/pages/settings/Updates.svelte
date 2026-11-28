<script lang="ts">
  import { onMount } from 'svelte';
  import Section from '../../lib/ui/Section.svelte';
  import SettingRow from '../../lib/ui/SettingRow.svelte';
  import Toggle from '../../lib/ui/Toggle.svelte';
  import Select from '../../lib/ui/Select.svelte';
  import Button from '../../lib/ui/Button.svelte';
  import Badge from '../../lib/ui/Badge.svelte';
  import Alert from '../../lib/ui/Alert.svelte';
  import Dialog from '../../lib/ui/Dialog.svelte';
  import ConfirmDialog from '../../lib/ui/ConfirmDialog.svelte';
  import UpdateSummary from '../../lib/updates/UpdateSummary.svelte';
  import PlanStep from '../../lib/updates/PlanStep.svelte';
  import InstallDialog from '../../lib/updates/InstallDialog.svelte';
  import { api, type AutoInstall, type UpdateJob } from '../../lib/api';
  import { editUpdates, gw, updatesWithDefaults } from '../../lib/gateway.svelte';
  import {
    autoInstallLabel,
    jobRunning,
    noteCounts,
    shortDate,
    stepPosition,
    when,
    windowLabel,
    windowOptions,
    windowStart,
  } from '../../lib/updates';
  import {
    checkNow,
    installInWindow,
    installNow,
    loadUpdates,
    rollBack,
    unschedule,
    unskip,
    updates,
  } from '../../lib/updates.svelte';
  import { param } from '../../lib/router.svelte';

  let installOpen = $state(false);
  let rollbackOpen = $state(false);
  let historyOpen = $state(false);
  let logOpen = $state(false);
  let history = $state<UpdateJob[]>([]);

  onMount(() => {
    void loadUpdates().then(() => {
      if (param('install') && updates.info?.available && updates.info.can_install) installOpen = true;
    });
  });

  const info = $derived(updates.info);
  const s = $derived(updatesWithDefaults(gw.draft.updates));
  const available = $derived(info?.available && !info.skipped.includes(info.available.version) ? info.available : null);
  const job = $derived(info?.job ?? null);
  const running = $derived(jobRunning(job) || updates.reconnecting);
  // A rollback stays on top for a week, or until something newer happens.
  const rolledBack = $derived(
    job && !running && (job.outcome === 'rolled-back' || job.outcome === 'failed') &&
      Date.now() / 1000 - (job.finished_unix ?? job.started_unix) < 7 * 86_400
      ? job
      : null,
  );
  const lastChecked = $derived(
    info?.checked_unix ? `${when(info.checked_unix)} · from github.com/krotovic-com/hearthgate/releases` : 'Not yet',
  );
  const methodHint: Record<string, string> = {
    deb: 'Upgrades install the signed .deb from the release, so apt stays in sync.',
    rpm: 'Upgrades install the signed .rpm from the release, so the package manager stays in sync.',
    tarball: 'Upgrades replace the binary from the signed release archive, as install.sh does.',
    manual: 'Updates are shown here, but you install them the way you installed Hearthgate.',
  };

  async function install(whenTo: 'now' | 'window') {
    if (!available) return;
    const ok = whenTo === 'now' ? await installNow(available.version) : await installInWindow(available.version);
    if (ok) installOpen = false;
  }

  async function openHistory() {
    historyOpen = true;
    history = await api.updateHistory().catch(() => []);
  }

  const autoOptions = (['off', 'patch', 'minor'] as AutoInstall[]).map((v) => ({ value: v, label: autoInstallLabel[v] }));
  const outcomeText: Record<UpdateJob['outcome'], string> = {
    running: 'Running',
    installed: 'Installed',
    'rolled-back': 'Rolled back',
    restored: 'Rolled back by you',
    refused: 'Not started',
    failed: 'Failed',
  };
</script>

<section id="updates" class="stack" aria-label="Updates" tabindex="-1">
  <div class="card" aria-live="polite">
    {#if running && job}
      <div class="pad">
        <UpdateSummary
          status="installing"
          title={updates.reconnecting
            ? 'Hearthgate is restarting…'
            : job.action === 'rollback'
              ? `Rolling back to ${job.to ?? 'the previous version'}`
              : `Installing Hearthgate ${job.to}`}
          description="{stepPosition(job)} · started {when(job.started_unix).replace(/^Today at /, '')} · public traffic pauses for a few seconds while Hearthgate restarts."
        >
          {#snippet actions()}
            <Button variant="ghost" onclick={() => (logOpen = true)}>View log</Button>
          {/snippet}
        </UpdateSummary>
        <ol class="plan">
          {#each job.steps as step, i (i)}
            <PlanStep {step} index={i} />
          {/each}
        </ol>
      </div>
    {:else if updates.reconnecting}
      <div class="pad">
        <UpdateSummary status="installing" title="Hearthgate is restarting…" description="The dashboard reconnects by itself." />
      </div>
    {:else if rolledBack}
      <div class="pad">
        <UpdateSummary
          status="rolled-back"
          title={rolledBack.outcome === 'failed'
            ? `Update to ${rolledBack.to} failed`
            : `Update to ${rolledBack.to} was rolled back`}
          description={rolledBack.outcome === 'failed'
            ? 'Hearthgate may not have come back on its own. See the log, or run hearthgate update status on the server.'
            : `${when(rolledBack.finished_unix ?? rolledBack.started_unix)} the new version failed its health check, so ${rolledBack.from} was restored. Your settings did not change.`}
        >
          {#snippet actions()}
            <Button variant="secondary" onclick={() => (logOpen = true)}>View log</Button>
          {/snippet}
        </UpdateSummary>
      </div>
      <SettingRow label="What failed" description={rolledBack.detail ?? undefined} />
      <SettingRow label="Running now" description="Restored from the backup taken before the update.">
        <Badge status="healthy" dot>{info?.current}</Badge>
      </SettingRow>
      {#if rolledBack.to && info?.skipped.includes(rolledBack.to)}
        <SettingRow
          label="{rolledBack.to} is skipped"
          description="Automatic updates will not try {rolledBack.to} again. The next release will be offered as usual."
        >
          <Button size="small" variant="secondary" disabled={updates.busy} onclick={() => unskip(rolledBack.to!)}
            >Try {rolledBack.to} again</Button
          >
        </SettingRow>
      {/if}
    {:else if available && info}
      <div class="pad">
        <UpdateSummary
          status="available"
          title="Hearthgate {available.version} is available"
          description="You have {info.current}. Released {shortDate(available.published_at)} · {noteCounts(available.notes)}."
        >
          {#snippet actions()}
            <a class="button-link" href={available.page} target="_blank" rel="noreferrer">Release notes</a>
            {#if info.can_install}
              <Button onclick={() => (installOpen = true)}>Review and install</Button>
            {/if}
          {/snippet}
        </UpdateSummary>
      </div>
      {#if info.scheduled === available.version}
        <SettingRow label="Installs tonight" description="Starts at {windowStart(info.settings.window)}, at the beginning of the install window.">
          <Button size="small" variant="secondary" disabled={updates.busy} onclick={unschedule}>Cancel</Button>
        </SettingRow>
      {/if}
      <SettingRow
        label="Release signature"
        description="Checked against the Hearthgate release key built into this version, and again by the upgrade helper right before installing."
      >
        <Badge status="healthy" dot>Required</Badge>
      </SettingRow>
      <SettingRow label="Last checked" description={lastChecked}>
        <Button size="small" variant="secondary" busy={updates.checking} onclick={checkNow}>Check now</Button>
      </SettingRow>
    {:else}
      <div class="pad">
        <UpdateSummary
          status="up-to-date"
          title={info ? `Hearthgate ${info.current} is up to date` : 'Updates'}
          description={info?.error ? `The last check failed: ${info.error}` : info?.checked_unix ? 'This is the newest release.' : 'Not checked yet.'}
        />
      </div>
      <SettingRow label="Last checked" description={lastChecked}>
        <Button size="small" variant="secondary" busy={updates.checking} onclick={checkNow}>Check now</Button>
      </SettingRow>
    {/if}
    {#if info?.cannot_install && !running}
      <div class="notes"><Alert tone="neutral">{info.cannot_install}</Alert></div>
    {/if}
    {#if updates.error}
      <div class="notes"><Alert>{updates.error}</Alert></div>
    {/if}
  </div>

  <Section
    id="updates-automatic"
    title="Automatic updates"
    description="Hearthgate only asks GitHub for the release list. Nothing about this machine is sent. Changes go into your draft."
  >
    <SettingRow label="Check for updates" description="Once a day. You get a notice here and on the Overview when a new version is out.">
      <Toggle label="Check for updates" checked={s.check} onchange={(on) => editUpdates((u) => (u.check = on))} />
    </SettingRow>
    <SettingRow
      label="Install automatically"
      description="Major releases always wait for you, because they can change your configuration."
    >
      <Select
        label="Install automatically"
        hideLabel
        value={s.auto_install}
        options={autoOptions}
        onchange={(e) => editUpdates((u) => (u.auto_install = (e.currentTarget as HTMLSelectElement).value as AutoInstall))}
      />
    </SettingRow>
    <SettingRow
      label="Install window"
      description="Automatic and scheduled installs only start in this window, in this server’s time zone. Public traffic pauses for a few seconds."
    >
      <Select
        label="Install window"
        hideLabel
        value={s.window}
        options={windowOptions(s.window)}
        onchange={(e) => editUpdates((u) => (u.window = (e.currentTarget as HTMLSelectElement).value))}
      />
    </SettingRow>
    <SettingRow
      label="Roll back if the new version is unhealthy"
      description="Built in. If the new version fails its health check within 2 minutes, the previous one comes back."
    >
      <Toggle label="Roll back if the new version is unhealthy" checked disabled />
    </SettingRow>
  </Section>

  <Section id="updates-machine" title="This machine" description="How Hearthgate was installed decides how it is upgraded.">
    <SettingRow label="Installed with" description={info ? methodHint[info.method] : undefined}>
      <span class="mono-small">{info?.method_text ?? '…'}</span>
    </SettingRow>
    <SettingRow label="Running version">
      <span class="mono-small">{info?.current ?? '…'}</span>
    </SettingRow>
    {#if info?.rollback_to}
      <SettingRow label="Previous version" description="Kept for one-click rollback. Your current settings stay unless that version can’t read them.">
        <Button size="small" variant="secondary" disabled={running || updates.busy} onclick={() => (rollbackOpen = true)}
          >Roll back to {info.rollback_to}</Button
        >
      </SettingRow>
    {/if}
    <SettingRow label="Update history" description="Every install and rollback, with its log.">
      <Button size="small" variant="ghost" onclick={openHistory}>View history</Button>
    </SettingRow>
  </Section>
</section>

{#if info && available}
  <InstallDialog bind:open={installOpen} {info} {available} busy={updates.busy} oninstall={install} />
{/if}

<ConfirmDialog
  bind:open={rollbackOpen}
  title="Roll back to {info?.rollback_to}?"
  message="Hearthgate restarts on {info?.rollback_to}, so public traffic pauses for a few seconds. Your current settings are kept unless {info?.rollback_to} can’t read them; then the settings from before the update come back."
  confirm="Roll back"
  onconfirm={async () => {
    await rollBack();
  }}
/>

<Dialog bind:open={logOpen} title="Update log" wide>
  {#if job?.log.length}
    <pre class="log">{job.log.join('\n')}</pre>
  {:else}
    <p class="secondary">Nothing logged yet.</p>
  {/if}
</Dialog>

<Dialog bind:open={historyOpen} title="Update history" wide>
  {#if history.length === 0}
    <p class="secondary">No installs or rollbacks yet.</p>
  {:else}
    <ul class="history">
      {#each history as h (h.started_unix)}
        <li>
          <details>
            <summary>
              <span>{when(h.started_unix)} · {h.action === 'install' ? `${h.from} → ${h.to}` : `back to ${h.to}`}</span>
              <Badge status={h.outcome === 'installed' || h.outcome === 'restored' ? 'healthy' : h.outcome === 'refused' ? 'neutral' : 'warning'}
                >{outcomeText[h.outcome]}</Badge
              >
            </summary>
            {#if h.detail}<p class="small secondary">{h.detail}</p>{/if}
            <pre class="log">{h.log.join('\n')}</pre>
          </details>
        </li>
      {/each}
    </ul>
  {/if}
</Dialog>

<style>
  .stack {
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-xl);
    scroll-margin-top: var(--hg-spacing-xl);
  }
  .stack:focus {
    outline: none;
  }
  .card {
    background: var(--hg-color-bg-surface);
    border: 1px solid var(--hg-color-border-default);
    border-radius: var(--hg-radius-lg);
  }
  .pad {
    padding: var(--hg-spacing-xl);
  }
  .plan {
    list-style: none;
    margin: var(--hg-spacing-lg) 0 0;
    padding: 0;
    display: flex;
    flex-direction: column;
  }
  .notes {
    padding: 0 var(--hg-spacing-xl) var(--hg-spacing-xl);
  }
  .button-link {
    padding: var(--hg-spacing-sm) var(--hg-spacing-md);
    color: var(--hg-color-text-primary);
    font-weight: 500;
    text-decoration: none;
  }
  .button-link:hover {
    text-decoration: underline;
  }
  .log {
    max-height: 360px;
    overflow: auto;
    padding: var(--hg-spacing-md);
    background: var(--hg-color-bg-subtle);
    border-radius: var(--hg-radius-md);
    font: 12px/18px var(--hg-font-mono, monospace);
    white-space: pre-wrap;
  }
  .history {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-sm);
  }
  summary {
    display: flex;
    justify-content: space-between;
    gap: var(--hg-spacing-md);
    cursor: pointer;
  }
</style>
