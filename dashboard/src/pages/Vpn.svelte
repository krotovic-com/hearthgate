<script lang="ts">
  import { onMount } from 'svelte';
  import PageHeader from '../lib/layout/PageHeader.svelte';
  import Card from '../lib/ui/Card.svelte';
  import StatCard from '../lib/ui/StatCard.svelte';
  import Badge from '../lib/ui/Badge.svelte';
  import Button from '../lib/ui/Button.svelte';
  import Icon from '../lib/ui/Icon.svelte';
  import Toggle from '../lib/ui/Toggle.svelte';
  import TextField from '../lib/ui/TextField.svelte';
  import SettingRow from '../lib/ui/SettingRow.svelte';
  import Alert from '../lib/ui/Alert.svelte';
  import Dialog from '../lib/ui/Dialog.svelte';
  import ConfirmDialog from '../lib/ui/ConfirmDialog.svelte';
  import AccessPicker from '../lib/editors/AccessPicker.svelte';
  import { api, ApiError, type Access, type Peer, type PeerConfigResponse, type VpnInfo } from '../lib/api';
  import { draft, editVpn, gw } from '../lib/gateway.svelte';
  import { accessLabel, bytes, freeAddress, peerName, serverAddress, subnetProblem, vpnWithDefaults } from '../lib/vpn';
  import { ago, plural } from '../lib/format';

  let info = $state<VpnInfo | null>(null);
  let loadError = $state<string | null>(null);

  async function load() {
    try {
      info = await api.vpn();
      loadError = null;
    } catch (e) {
      loadError = e instanceof Error ? e.message : String(e);
    }
  }

  onMount(() => {
    load();
    const timer = setInterval(load, 10_000);
    return () => clearInterval(timer);
  });

  const vpn = $derived(draft.vpn);
  const dns = $derived(draft.dns);
  const applied = $derived(vpnWithDefaults(gw.current.vpn));
  const services = $derived(draft.services.map((s) => s.name));
  const live = $derived(new Map((info?.peers ?? []).map((p) => [p.public_key, p])));
  const online = $derived(vpn.peers.filter((p) => live.get(p.public_key)?.online).length);
  const transfer = $derived((info?.peers ?? []).reduce((sum, p) => sum + p.rx_bytes + p.tx_bytes, 0));
  const endpoint = $derived(info?.endpoint ?? null);

  function handshake(at: number | null | undefined): string {
    return at ? ago(new Date(at * 1000)) : 'Never';
  }

  // Adding a device.
  let label = $state('');
  let access = $state<Access>('all');
  let tried = $state(false);
  let busy = $state(false);
  let addError = $state<string | null>(null);
  let created = $state<(PeerConfigResponse & { name: string }) | null>(null);

  const name = $derived(peerName(label));
  const nameError = $derived(
    !name
      ? 'Give the device a name.'
      : vpn.peers.some((p) => p.name === name)
        ? `A device is already called ${name}.`
        : null,
  );
  const accessError = $derived(access !== 'all' && access.length === 0);
  const nextAddress = $derived(freeAddress(vpn));

  async function create() {
    tried = true;
    if (nameError || accessError) return;
    busy = true;
    addError = null;
    try {
      const state = $state.snapshot(gw.draft);
      state.vpn = { ...vpnWithDefaults(state.vpn), enabled: true };
      const result = await api.peerConfig({ name, access: $state.snapshot(access), state });
      editVpn((v) => {
        v.enabled = true;
        v.peers.push({ name, public_key: result.public_key, address: result.address, access: $state.snapshot(access) });
      });
      created = { ...result, name };
      label = '';
      access = 'all';
      tried = false;
    } catch (e) {
      addError = e instanceof ApiError || e instanceof Error ? e.message : String(e);
    } finally {
      busy = false;
    }
  }

  const qrSrc = $derived(created ? `data:image/svg+xml;base64,${btoa(created.qr_svg)}` : '');
  const confHref = $derived(created ? URL.createObjectURL(new Blob([created.config], { type: 'text/plain' })) : '');
  $effect(() => {
    const href = confHref;
    return () => {
      if (href) URL.revokeObjectURL(href);
    };
  });

  // Changing or removing a device.
  let editing = $state<Peer | null>(null);
  let editAccess = $state<Access>('all');
  let editOpen = $state(false);
  let removing = $state<Peer | null>(null);
  let removeOpen = $state(false);

  function saveAccess() {
    if (!editing || (editAccess !== 'all' && editAccess.length === 0)) return;
    const target = editing.name;
    const next = $state.snapshot(editAccess);
    editVpn((v) => {
      const peer = v.peers.find((p) => p.name === target);
      if (peer) peer.access = next;
    });
    editOpen = false;
  }

  // Settings.
  let endpointText = $state('');
  let portText = $state('');
  let subnetText = $state('');
  let zoneText = $state('');
  let upstreamText = $state('');
  $effect(() => {
    endpointText = vpn.endpoint ?? '';
    portText = String(vpn.port);
    subnetText = vpn.subnet;
    zoneText = dns.zone;
    upstreamText = dns.upstream.join(', ');
  });
  const portError = $derived(/^\d{1,5}$/.test(portText) && Number(portText) >= 1 && Number(portText) <= 65535 ? null : 'Use a port from 1 to 65535.');
  const subnetError = $derived(subnetProblem(subnetText));
  const zoneError = $derived(/^([a-z0-9]([a-z0-9-]*[a-z0-9])?\.)*[a-z0-9]([a-z0-9-]*[a-z0-9])?$/i.test(zoneText.trim()) ? null : 'Use a domain such as home.internal.');

  function saveSettings() {
    if (portError || subnetError || zoneError) return;
    editVpn((v, d) => {
      v.endpoint = endpointText.trim() || undefined;
      v.port = Number(portText);
      v.subnet = subnetText.trim();
      d.zone = zoneText.trim();
      d.upstream = upstreamText
        .split(/[\s,]+/)
        .map((s) => s.trim())
        .filter(Boolean);
    });
  }
</script>

<PageHeader
  title="VPN peers"
  subtitle="Devices that can reach your internal services over WireGuard, including ones that are not public. Names resolve through the internal DNS."
>
  {#snippet actions()}
    <a class="link-button" href="#dns">DNS names</a>
    <a class="link-button primary" href="#add-device">Add device</a>
  {/snippet}
</PageHeader>

{#if !vpn.enabled}
  <Card title="The VPN is off">
    <p class="secondary">
      Turn it on to reach this machine, your local network and every service from your phone or laptop, wherever
      you are. Hearthgate opens UDP port {vpn.port} for it.
    </p>
    <div>
      <Button onclick={() => editVpn((v) => (v.enabled = true))}>Turn on the VPN</Button>
    </div>
  </Card>
{:else if !applied.enabled}
  <Alert tone="neutral">The VPN turns on when you apply the draft.</Alert>
{/if}
{#if loadError}<Alert>{loadError}</Alert>{/if}
{#each info?.problems ?? [] as problem (problem)}
  <Alert tone="warning">{problem}</Alert>
{/each}

<div class="stats">
  <StatCard label="Online" value={online} detail={`of ${plural(vpn.peers.length, 'device')}`} />
  <StatCard
    label="Network"
    value={vpn.subnet}
    detail={`${vpn.port}/udp${endpoint ? ` via ${endpoint.replace(/:\d+$/, '')}` : ''}`}
  />
  <StatCard
    label="Transfer"
    value={bytes(transfer)}
    detail={info?.running ? `Since the VPN started${info.backend === 'userspace' ? ' · userspace' : ''}` : 'The VPN is not running'}
  />
  <StatCard label="DNS names" value={info?.names.length ?? 0} detail={dns.enabled ? `*.${dns.zone}` : 'Internal DNS is off'} />
</div>

<div class="layout">
  <div class="column">
    <section aria-labelledby="peers-title">
      <h2 id="peers-title" class="visually-hidden">Devices</h2>
      {#if vpn.peers.length === 0}
        <div class="empty">
          <p class="strong">No devices yet</p>
          <p class="small secondary">Add your phone or laptop to reach your services from anywhere.</p>
        </div>
      {:else}
        <ul class="peers">
          {#each vpn.peers as peer (peer.name)}
            {@const seen = live.get(peer.public_key)}
            {@const pending = !applied.peers.some((p) => p.public_key === peer.public_key)}
            <li class="peer">
              <Icon name="peers" />
              <div class="name">
                <p class="strong">{peer.name}</p>
                <p class="small secondary">{peer.name}.{dns.zone}</p>
              </div>
              <p class="mono-small address">{peer.address}</p>
              <div class="access">
                <p class="small">{accessLabel(peer.access)}</p>
                <p class="mono-small secondary">
                  {#if seen && (seen.rx_bytes || seen.tx_bytes)}↓ {bytes(seen.tx_bytes)}&nbsp;&nbsp;↑ {bytes(seen.rx_bytes)}{:else}No traffic yet{/if}
                </p>
              </div>
              <p class="small secondary handshake">{handshake(seen?.latest_handshake)}</p>
              <span class="status">
                {#if pending}
                  <Badge status="neutral">Not applied</Badge>
                {:else if seen?.online}
                  <Badge status="healthy" dot>Online</Badge>
                {:else}
                  <Badge status="neutral" dot>Offline</Badge>
                {/if}
              </span>
              <Button
                variant="secondary"
                size="small"
                onclick={() => {
                  editing = peer;
                  editAccess = peer.access ?? 'all';
                  editOpen = true;
                }}>Edit<span class="visually-hidden"> {peer.name}</span></Button
              >
            </li>
          {/each}
        </ul>
      {/if}
    </section>

    <section id="dns" class="card-like" aria-labelledby="dns-title">
      <h2 id="dns-title">Internal DNS</h2>
      <p class="small secondary">
        {#if dns.enabled}
          Answers on {serverAddress(vpn.subnet)}{dns.lan ? ' and on the local network' : ' over the VPN only'}. Every
          service and device gets a name; other names go to {dns.upstream.length ? dns.upstream.join(', ') : "this machine's resolvers"}.
        {:else}
          Off. Devices use their own DNS server.
        {/if}
      </p>
      {#if info && info.names.length}
        <ul class="records">
          {#each info.names as record (record.name)}
            <li>
              <span class="mono-small record-name">{record.name}</span>
              <span class="mono-small secondary">{record.addresses.join(', ')}</span>
            </li>
          {/each}
        </ul>
      {:else if dns.enabled}
        <p class="small secondary">Names appear once the VPN runs.</p>
      {/if}
    </section>

    <Card title="Settings">
      <div class="settings">
        <SettingRow label="VPN" description="Let your devices connect over WireGuard.">
          <Toggle label="VPN" checked={vpn.enabled} onchange={(on) => editVpn((v) => (v.enabled = on))} />
        </SettingRow>
        <SettingRow label="Internal DNS" description="Answer names like nas.{dns.zone} for VPN devices.">
          <Toggle label="Internal DNS" checked={dns.enabled} onchange={(on) => editVpn((_, d) => (d.enabled = on))} />
        </SettingRow>
        <SettingRow
          label="DNS on the local network"
          description="Also answer on this machine's LAN addresses, for devices at home that use it as their DNS server."
        >
          <Toggle label="DNS on the local network" checked={dns.lan} onchange={(on) => editVpn((_, d) => (d.lan = on))} />
        </SettingRow>
        <div class="fields">
          <TextField
            label="Devices connect to"
            bind:value={endpointText}
            mono
            placeholder={endpoint?.replace(/:\d+$/, '') ?? 'vpn.example.com'}
            hint={endpoint ? `Now ${endpoint}.` : 'A host name or address. Without one, new devices can\'t be added.'}
            onchange={saveSettings}
          />
          <TextField label="UDP port" bind:value={portText} mono inputmode="numeric" error={portError} onchange={saveSettings} />
          <TextField label="VPN network" bind:value={subnetText} mono error={subnetError} onchange={saveSettings} />
          <TextField label="DNS zone" bind:value={zoneText} mono error={zoneError} onchange={saveSettings} />
          <TextField
            label="Upstream resolvers"
            bind:value={upstreamText}
            mono
            placeholder="from /etc/resolv.conf"
            hint="For every other name, like 1.1.1.1 or 192.168.1.1."
            onchange={saveSettings}
          />
        </div>
      </div>
    </Card>
  </div>

  <div class="column side">
    <section id="add-device" class="card-like" aria-labelledby="add-title">
      <h2 id="add-title">Add a device</h2>
      {#if created}
        <p class="small secondary">Scan the code with the WireGuard app, or download the config file.</p>
        <div class="qr">
          <img src={qrSrc} alt="WireGuard configuration for {created.name}" width="176" height="176" />
          <p class="small secondary">
            The code contains the device's private key. It is shown only now: Hearthgate does not keep it.
          </p>
        </div>
        <Alert tone="neutral">{created.name} gets {created.address}. Apply the draft so it can connect.</Alert>
        <div class="buttons">
          <a class="link-button" href={confHref} download="{created.name}.conf">Download .conf</a>
          <Button variant="ghost" onclick={() => (created = null)}>Done</Button>
        </div>
      {:else}
        <p class="small secondary">Hearthgate creates the device's keys and shows its configuration once.</p>
        <TextField
          label="Device name"
          bind:value={label}
          mono
          placeholder="Kitchen tablet"
          error={tried ? nameError : null}
          hint={name && nextAddress ? `Gets address ${nextAddress} and the name ${name}.${dns.zone}.` : undefined}
        />
        <AccessPicker bind:value={access} {services} />
        {#if addError}<Alert>{addError}</Alert>{/if}
        <div class="buttons">
          <Button {busy} onclick={create}>Create device</Button>
        </div>
      {/if}
    </section>
  </div>
</div>

<Dialog bind:open={editOpen} title="Edit {editing?.name}" description="Its key and address stay the same.">
  <AccessPicker bind:value={editAccess} {services} />
  {#snippet footer()}
    <Button
      variant="danger"
      onclick={() => {
        removing = editing;
        editOpen = false;
        removeOpen = true;
      }}>Remove device</Button
    >
    <span class="spacer"></span>
    <Button variant="secondary" onclick={() => (editOpen = false)}>Cancel</Button>
    <Button disabled={editAccess !== 'all' && editAccess.length === 0} onclick={saveAccess}>Save</Button>
  {/snippet}
</Dialog>
<ConfirmDialog
  bind:open={removeOpen}
  title="Remove {removing?.name}?"
  message="It can't connect any more once you apply the draft. To add it again, create a new configuration."
  onconfirm={() => {
    const target = removing?.name;
    editVpn((v) => (v.peers = v.peers.filter((p) => p.name !== target)));
  }}
/>

<style>
  .stats {
    display: flex;
    gap: var(--hg-spacing-lg);
  }
  .layout {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 380px;
    gap: var(--hg-spacing-xl);
    align-items: start;
  }
  .column {
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-xl);
    min-width: 0;
  }
  .link-button {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    padding: var(--hg-spacing-sm) var(--hg-spacing-lg);
    border-radius: var(--hg-radius-md);
    border: 1px solid var(--hg-color-border-strong);
    background: var(--hg-color-bg-surface);
    color: var(--hg-color-text-primary);
    font: 500 14px/20px var(--hg-font-body);
    text-decoration: none;
    white-space: nowrap;
  }
  .link-button.primary {
    background: var(--hg-color-accent-hover);
    border-color: var(--hg-color-accent-hover);
    color: var(--hg-color-text-on-accent);
  }
  .link-button:focus-visible {
    outline: 2px solid var(--hg-color-border-focus);
    outline-offset: 2px;
  }
  .peers,
  .records {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .peers,
  .empty,
  .card-like {
    background: var(--hg-color-bg-surface);
    border: 1px solid var(--hg-color-border-default);
    border-radius: var(--hg-radius-lg);
  }
  .empty {
    padding: var(--hg-spacing-xl);
    text-align: center;
  }
  .peer {
    display: flex;
    align-items: center;
    gap: var(--hg-spacing-md);
    padding: var(--hg-spacing-md) var(--hg-spacing-lg);
  }
  .peer + .peer {
    border-top: 1px solid var(--hg-color-border-default);
  }
  .peer .name {
    width: 150px;
    flex-shrink: 0;
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .address {
    width: 88px;
    flex-shrink: 0;
  }
  .access {
    flex: 1;
    min-width: 0;
  }
  .handshake {
    width: 84px;
    flex-shrink: 0;
  }
  .status {
    flex-shrink: 0;
  }
  .spacer {
    flex: 1;
  }
  .card-like {
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-lg);
    padding: var(--hg-spacing-xl);
    scroll-margin-top: var(--hg-spacing-xl);
  }
  .card-like h2 {
    font: 600 20px/28px var(--hg-font-display);
    letter-spacing: -0.02em;
    margin: 0;
  }
  .records li {
    display: flex;
    gap: var(--hg-spacing-lg);
    padding: var(--hg-spacing-sm) var(--hg-spacing-2xs);
    border-bottom: 1px solid var(--hg-color-border-default);
    overflow-wrap: anywhere;
  }
  .record-name {
    width: 260px;
    flex-shrink: 0;
  }
  .settings {
    margin: 0 calc(-1 * var(--hg-spacing-xl)) calc(-1 * var(--hg-spacing-xl));
  }
  .fields {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: var(--hg-spacing-md);
    padding: var(--hg-spacing-lg) var(--hg-spacing-xl);
  }
  .qr {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--hg-spacing-sm);
    padding: var(--hg-spacing-lg);
    border-radius: var(--hg-radius-md);
    background: var(--hg-color-bg-canvas);
    text-align: center;
  }
  .qr img {
    background: #fff;
    padding: var(--hg-spacing-sm);
    border-radius: var(--hg-radius-sm);
  }
  .buttons {
    display: flex;
    gap: var(--hg-spacing-sm);
  }
  @media (max-width: 1100px) {
    .stats {
      flex-wrap: wrap;
    }
    .layout {
      grid-template-columns: minmax(0, 1fr);
    }
    .peer {
      flex-wrap: wrap;
    }
  }
</style>
