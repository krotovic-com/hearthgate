<script lang="ts">
  import { onMount } from 'svelte';
  import Section from '../../lib/ui/Section.svelte';
  import SettingRow from '../../lib/ui/SettingRow.svelte';
  import Toggle from '../../lib/ui/Toggle.svelte';
  import Badge from '../../lib/ui/Badge.svelte';
  import Alert from '../../lib/ui/Alert.svelte';
  import { api, type RouterInfo } from '../../lib/api';
  import { draft, edit, gw } from '../../lib/gateway.svelte';
  import { portLabel } from '../../lib/format';

  let info = $state<RouterInfo | null>(null);

  onMount(() => {
    const load = () =>
      api
        .router()
        .then((r) => (info = r))
        .catch(() => {});
    load();
    const timer = setInterval(load, 10_000);
    return () => clearInterval(timer);
  });

  const running = $derived(gw.current.router?.upnp ?? false);
  const methods: Record<string, string> = { upnp: 'UPnP', 'nat-pmp': 'NAT-PMP' };
</script>

<Section
  id="router"
  title="Router"
  description="Port forwarding on your home router, so public interfaces are reachable from the internet."
>
  <SettingRow
    label="Forward ports automatically"
    description="Ask the router (UPnP or NAT-PMP) to forward each public port to this machine, and remove the forwards when ports close or Hearthgate stops. Any program on your network can use UPnP, so it's off unless you turn it on."
  >
    <Toggle
      label="Forward ports automatically"
      checked={draft.upnp}
      onchange={(on) => edit((s) => (s.router = { upnp: on }))}
    />
  </SettingRow>
  {#if running && info}
    <SettingRow label="Router" description={info.method ? `Found with ${methods[info.method] ?? info.method}` : undefined}>
      <span class="mono-small">{info.router ?? 'Looking for the router…'}</span>
    </SettingRow>
    {#if info.public_ip}
      <SettingRow label="Public address" description={info.local_ip ? `Forwarded to ${info.local_ip}` : undefined}>
        <span class="mono-small">{info.public_ip}</span>
      </SettingRow>
    {/if}
    <div class="notes">
      {#if info.error}<Alert tone="warning">{info.error}</Alert>{/if}
      {#if info.double_nat}
        <Alert tone="warning">
          The router's public address {info.public_ip} is itself private. Another router or your provider's
          network sits in front of it, so forwarded ports may stay unreachable from the internet. Forward the
          ports on that router too, or ask your provider for a public address.
        </Alert>
      {/if}
      {#if info.mappings.length}
        <ul class="mappings" aria-label="Forwarded ports">
          {#each info.mappings as m (`${m.port}/${m.protocol}`)}
            <li>
              <span class="mono-small">{portLabel(m.port, m.protocol)}</span>
              {#if m.active}
                <Badge status="healthy" dot>Forwarded</Badge>
              {:else}
                <Badge status="critical" dot>Refused</Badge>
                {#if m.error}<span class="small secondary">{m.error}</span>{/if}
              {/if}
            </li>
          {/each}
        </ul>
      {/if}
    </div>
  {/if}
</Section>

<style>
  .notes {
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-md);
    padding: 0 var(--hg-spacing-xl) var(--hg-spacing-lg);
  }
  .notes:empty {
    display: none;
  }
  .mappings {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-sm);
  }
  .mappings li {
    display: flex;
    align-items: center;
    gap: var(--hg-spacing-md);
  }
</style>
