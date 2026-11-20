<script lang="ts">
  import { onMount } from 'svelte';
  import Section from '../../lib/ui/Section.svelte';
  import SettingRow from '../../lib/ui/SettingRow.svelte';
  import Toggle from '../../lib/ui/Toggle.svelte';
  import Button from '../../lib/ui/Button.svelte';
  import TextField from '../../lib/ui/TextField.svelte';
  import Select from '../../lib/ui/Select.svelte';
  import Alert from '../../lib/ui/Alert.svelte';
  import { api, ApiError, type AccessInfo } from '../../lib/api';
  import { list, portProblem } from '../../lib/editors/common';

  let info = $state<AccessInfo | null>(null);
  let error = $state<string | null>(null);
  let problems = $state<string[]>([]);
  let saved = $state<string | null>(null);
  let busy = $state(false);

  let mode = $state<'lan' | 'custom'>('lan');
  let port = $state('7380');
  let listen = $state('');
  let allow = $state('');

  function fill(i: AccessInfo) {
    info = i;
    mode = i.settings.mode;
    port = String(i.settings.port);
    // Switching to custom starts from what runs now.
    const ips = i.effective.listen.map((a) => a.replace(/:\d+$/, '').replace(/^\[|\]$/g, ''));
    listen = (i.settings.mode === 'custom' ? i.settings.listen : ips.filter((ip) => ip !== '127.0.0.1')).join(', ');
    allow = (
      i.settings.mode === 'custom'
        ? i.settings.allow
        : i.effective.allow.filter((n) => n !== '127.0.0.0/8' && n !== '::1/128')
    ).join(', ');
  }

  onMount(() => {
    api
      .access()
      .then(fill)
      .catch((e) => (error = e instanceof Error ? e.message : String(e)));
  });

  async function save() {
    error = null;
    problems = [];
    saved = null;
    const portError = portProblem(port);
    if (portError) {
      error = portError;
      return;
    }
    busy = true;
    try {
      const before = location.port;
      const next = await api.setAccess({ mode, port: Number(port), listen: list(listen), allow: list(allow) });
      fill(next);
      saved =
        String(next.settings.port) !== (before || '443')
          ? `Saved. The dashboard now answers on port ${next.settings.port}; open it at https://${location.hostname}:${next.settings.port}/.`
          : 'Saved and applied.';
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
      problems = e instanceof ApiError ? (e.problem.problems ?? []) : [];
    } finally {
      busy = false;
    }
  }
</script>

<Section
  id="access"
  title="Dashboard access"
  description="The dashboard is never reachable from the internet. Connections from anywhere else are closed before anything is read."
>
  <SettingRow label="Only from your LAN" description="Built in and cannot be turned off.">
    <Toggle label="Only from your LAN" checked disabled />
  </SettingRow>
  {#if info}
    <SettingRow label="Your address" description="As Hearthgate sees this browser.">
      <span class="mono-small">{info.client}</span>
    </SettingRow>
    <SettingRow label="Listening on" description="Addresses the dashboard answers on now.">
      <span class="mono-small">{info.effective.listen.join('  ')}</span>
    </SettingRow>
    <SettingRow label="Allowed networks" description="Loopback is always allowed.">
      <span class="mono-small">{info.effective.allow.join('  ')}</span>
    </SettingRow>
    {#if info.fingerprint}
      <SettingRow
        label="Certificate fingerprint"
        description="Compare this with what your browser shows before trusting the dashboard's certificate."
      >
        <span class="mono-small fingerprint">{info.fingerprint}</span>
      </SettingRow>
    {/if}
    {#if info.locked}
      <div class="body">
        <Alert tone="neutral">
          Set by <span class="mono-small">admin.listen</span> or <span class="mono-small">admin.allow</span> in
          /etc/hearthgate/hearthgate.toml. Change it there and restart Hearthgate.
        </Alert>
      </div>
    {:else}
      <form
        class="body"
        onsubmit={(e) => {
          e.preventDefault();
          save();
        }}
      >
        <Select
          label="Which addresses"
          bind:value={mode}
          options={[
            { value: 'lan', label: "This machine's LAN addresses, found at every start" },
            { value: 'custom', label: 'Addresses and networks I choose' },
          ]}
        />
        <TextField label="Port" bind:value={port} mono inputmode="numeric" />
        {#if mode === 'custom'}
          <TextField
            label="Listen on"
            bind:value={listen}
            mono
            placeholder="192.168.1.10"
            hint="This machine's addresses, separated by commas. Loopback is always included."
          />
          <TextField
            label="Allowed networks"
            bind:value={allow}
            mono
            placeholder="192.168.1.0/24"
            hint="Only private ranges: 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16, 100.64.0.0/10, fc00::/7."
          />
        {:else}
          <p class="small secondary">
            Found now: {info.detected.listen.join(', ')} for {info.detected.allow.join(', ')}.
          </p>
        {/if}
        {#if error}
          <Alert>
            <p>{error}</p>
            {#if problems.length}<ul>{#each problems as p (p)}<li>{p}</li>{/each}</ul>{/if}
          </Alert>
        {/if}
        {#if saved}<Alert tone="neutral">{saved}</Alert>{/if}
        <div><Button type="submit" {busy}>Save and apply</Button></div>
        <p class="small secondary">
          Locked out? On the server, run <span class="mono-small">hearthgate admin-access --reset</span> and restart
          Hearthgate.
        </p>
      </form>
    {/if}
  {:else if error}
    <div class="body"><Alert>{error}</Alert></div>
  {/if}
</Section>

<style>
  .body {
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-lg);
    padding: var(--hg-spacing-lg) var(--hg-spacing-xl) var(--hg-spacing-xl);
    border-top: 1px solid var(--hg-color-border-default);
  }
  .fingerprint {
    overflow-wrap: anywhere;
    text-align: right;
  }
  ul {
    margin: var(--hg-spacing-xs) 0 0;
    padding-left: var(--hg-spacing-xl);
  }
</style>
