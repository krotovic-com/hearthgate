<script lang="ts">
  import PageHeader from '../lib/layout/PageHeader.svelte';
  import Card from '../lib/ui/Card.svelte';
  import StatCard from '../lib/ui/StatCard.svelte';
  import Badge from '../lib/ui/Badge.svelte';
  import Button from '../lib/ui/Button.svelte';
  import Icon from '../lib/ui/Icon.svelte';
  import KeyValue from '../lib/ui/KeyValue.svelte';
  import ConfirmDialog from '../lib/ui/ConfirmDialog.svelte';
  import InterfaceEditor from '../lib/editors/InterfaceEditor.svelte';
  import type { Interface } from '../lib/api';
  import { live } from '../lib/live.svelte';
  import { gw, draft, edit, routesUsingInterface } from '../lib/gateway.svelte';
  import { certificateFor, interfaceHealth } from '../lib/health';
  import { modeLabels, plural, portLabel, strategyLabels } from '../lib/format';
  import { navigate, param } from '../lib/router.svelte';

  let editorOpen = $state(false);
  let editing = $state<Interface | null>(null);
  let removing = $state<Interface | null>(null);
  let confirmOpen = $state(false);

  // /interfaces?new=1 opens the editor (from the Overview's first steps).
  $effect(() => {
    if (param('new') && gw.loaded) {
      editing = null;
      editorOpen = true;
      navigate('/interfaces', true);
    }
  });

  // /interfaces?edit=<name> opens that interface (from Ask AI's Open in form).
  $effect(() => {
    const name = param('edit');
    if (name && gw.loaded) {
      editing = draft.interfaces.find((i) => i.name === name) ?? null;
      editorOpen = editing !== null;
      navigate('/interfaces', true);
    }
  });

  const interfaces = $derived(draft.interfaces);
  const ports = $derived(interfaces.flatMap((i) => i.listeners));
  const unhealthy = $derived(
    interfaces.filter((i) => !isNew(i.name) && interfaceHealth(live.status, i, gw.certificates).status !== 'healthy').length,
  );
  const valid = $derived(gw.certificates.filter((c) => c.days_left > 0));
  const soonest = $derived(valid.length ? Math.min(...valid.map((c) => c.days_left)) : null);
  const isNew = (name: string) => !(gw.current.interfaces ?? []).some((i) => i.name === name);

  function routesTo(iface: Interface): string {
    const routes = routesUsingInterface(iface.name);
    if (routes.length === 0) return 'No routes yet';
    return routes
      .map((r) => {
        const targets = r.targets.map((t) => `${t.service}:${t.port}`).join(', ');
        const lb = r.targets.length > 1 ? ` · ${strategyLabels[r.strategy ?? 'round-robin']?.toLowerCase()}` : '';
        const where = r.path_prefix && r.path_prefix !== '/' ? `${r.path_prefix} → ` : '';
        return `${where}${targets}${lb}`;
      })
      .join('; ');
  }

  function certificateText(iface: Interface): string {
    if (!iface.listeners.some((l) => l.mode === 'https')) {
      return iface.listeners.some((l) => l.mode === 'tls-passthrough')
        ? 'Handled by the service (passthrough)'
        : 'Not used (no HTTPS)';
    }
    const cert = certificateFor(iface, gw.certificates);
    if (!cert) return iface.certificate ? `Uploaded ${iface.certificate} is missing` : 'Requesting from Let’s Encrypt';
    const kind = cert.source === 'automatic' ? `renews in ${Math.max(0, cert.days_left - 30)} days` : 'uploaded';
    return `Valid · ${kind} · ${cert.issuer}`;
  }

  function rows(iface: Interface) {
    return [
      { label: 'Routes to', value: routesTo(iface) },
      { label: 'Certificate', value: certificateText(iface) },
      { label: 'Ports', value: iface.listeners.map((l) => `${portLabel(l.port, l.protocol)} ${modeLabels[l.mode]}`).join(', ') },
      ...(iface.bind?.length ? [{ label: 'Bound to', value: iface.bind.join(', ') }] : []),
      ...(iface.rate_limit ? [{ label: 'Rate limit', value: `${iface.rate_limit} new connections a second per client` }] : []),
    ];
  }

  function remove() {
    const target = removing;
    if (!target) return;
    edit((s) => {
      s.interfaces = s.interfaces.filter((i) => i.name !== target.name);
      s.routes = s.routes.filter((r) => r.interface !== target.name);
    });
  }
</script>

<PageHeader
  title="Public interfaces"
  subtitle="Domains and ports reachable from the internet. Every other port is closed."
>
  {#snippet actions()}
    <Button
      onclick={() => {
        editing = null;
        editorOpen = true;
      }}>Add public interface</Button
    >
  {/snippet}
</PageHeader>

<div class="stats">
  <StatCard
    label="Public interfaces"
    value={interfaces.length}
    detail={unhealthy ? `${unhealthy} need${unhealthy === 1 ? 's' : ''} attention` : 'All listening'}
  />
  <StatCard
    label="Open ports"
    value={ports.length}
    detail={ports.length ? ports.map((p) => p.port).join(' · ') : 'None'}
  />
  <StatCard label="Routes" value={draft.routes.length} detail="Connect ports to services" />
  <StatCard
    label="Certificates"
    value={`${valid.length} valid`}
    detail={soonest === null ? 'None yet' : `Next expiry in ${plural(soonest, 'day')}`}
  />
</div>

{#if interfaces.length === 0}
  <Card>
    <p class="secondary">
      Nothing is reachable from the internet yet. Add a public interface with your domain, then a route
      to a service.
    </p>
  </Card>
{:else}
  <div class="cards">
    {#each interfaces as iface (iface.name)}
      {@const health = interfaceHealth(live.status, iface, gw.certificates)}
      <article class="card" aria-labelledby="iface-{iface.name}">
        <div class="head">
          <Icon name="globe" />
          <h2 id="iface-{iface.name}" class="title">{iface.domains?.[0] ?? iface.name}</h2>
          {#if isNew(iface.name)}
            <Badge status="neutral">Not applied</Badge>
          {:else}
            <Badge status={health.status} dot>{health.label}</Badge>
          {/if}
        </div>
        {#if (iface.domains?.length ?? 0) > 1 || iface.domains?.[0] !== iface.name}
          <p class="mono-small secondary">
            {iface.name}{#if (iface.domains?.length ?? 0) > 1}&nbsp;· also {iface.domains?.slice(1).join(', ')}{/if}
          </p>
        {/if}
        <ul class="chips" aria-label="Ports">
          {#each iface.listeners as l (l.port)}
            <li class="chip mono-small">{portLabel(l.port, l.protocol)}{l.mode === 'http' && iface.listeners.some((x) => x.mode === 'https') ? ' → HTTPS' : ''}</li>
          {/each}
        </ul>
        <hr />
        <KeyValue rows={rows(iface)} labelWidth={96} mono={false} />
        <div class="actions">
          <Button
            variant="secondary"
            size="small"
            onclick={() => {
              editing = iface;
              editorOpen = true;
            }}>Edit<span class="visually-hidden"> {iface.name}</span></Button
          >
          <Button variant="ghost" size="small" onclick={() => navigate(`/routes?interface=${iface.name}`)}>
            Routes<span class="visually-hidden"> of {iface.name}</span>
          </Button>
          <span class="spacer"></span>
          <Button
            variant="ghost"
            size="small"
            onclick={() => {
              removing = iface;
              confirmOpen = true;
            }}>Remove<span class="visually-hidden"> {iface.name}</span></Button
          >
        </div>
      </article>
    {/each}
  </div>
{/if}

<InterfaceEditor bind:open={editorOpen} original={editing} />
<ConfirmDialog
  bind:open={confirmOpen}
  title="Remove {removing?.name}?"
  message={removing && routesUsingInterface(removing.name).length
    ? `Its ${plural(routesUsingInterface(removing.name).length, 'route')} will be removed too. The ports close when you apply the draft.`
    : 'The ports close when you apply the draft.'}
  onconfirm={remove}
/>

<style>
  .stats {
    display: flex;
    gap: var(--hg-spacing-lg);
  }
  .cards {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(420px, 1fr));
    gap: var(--hg-spacing-xl);
  }
  .card {
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-md);
    padding: var(--hg-spacing-xl);
    background: var(--hg-color-bg-surface);
    border: 1px solid var(--hg-color-border-default);
    border-radius: var(--hg-radius-lg);
    min-width: 0;
  }
  .head {
    display: flex;
    align-items: center;
    gap: var(--hg-spacing-sm);
  }
  .title {
    flex: 1;
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: var(--hg-spacing-sm);
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .chip {
    padding: 2px var(--hg-spacing-sm);
    background: var(--hg-color-bg-subtle);
    border-radius: var(--hg-radius-sm);
  }
  hr {
    width: 100%;
    border: none;
    border-top: 1px solid var(--hg-color-border-default);
    margin: var(--hg-spacing-xs) 0;
  }
  .actions {
    display: flex;
    gap: var(--hg-spacing-sm);
    margin-top: var(--hg-spacing-xs);
  }
  .spacer {
    flex: 1;
  }
  @media (max-width: 1100px) {
    .stats {
      flex-wrap: wrap;
    }
  }
</style>
