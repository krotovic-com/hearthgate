<script lang="ts">
  import PageHeader from '../lib/layout/PageHeader.svelte';
  import Card from '../lib/ui/Card.svelte';
  import StatCard from '../lib/ui/StatCard.svelte';
  import Badge from '../lib/ui/Badge.svelte';
  import Button from '../lib/ui/Button.svelte';
  import KeyValue from '../lib/ui/KeyValue.svelte';
  import ConfirmDialog from '../lib/ui/ConfirmDialog.svelte';
  import ServiceEditor from '../lib/editors/ServiceEditor.svelte';
  import type { Service } from '../lib/api';
  import { live } from '../lib/live.svelte';
  import { gw, draft, edit, routesUsingService } from '../lib/gateway.svelte';
  import { serviceHealth, serviceUpstreams, upstreamDown } from '../lib/health';
  import { clock, plural, rate } from '../lib/format';
  import { navigate, param, route } from '../lib/router.svelte';

  type Filter = 'all' | 'docker' | 'external' | 'private';
  let filter = $state<Filter>('all');
  let editorOpen = $state(false);
  let editing = $state<Service | null>(null);
  let newKind = $state<'external' | 'docker'>('external');
  let confirmOpen = $state(false);

  $effect(() => {
    const kind = param('new');
    if ((kind === 'external' || kind === 'docker') && gw.loaded) {
      editing = null;
      newKind = kind;
      editorOpen = true;
      navigate('/services', true);
    }
  });

  const services = $derived(draft.services);
  const isPublic = (s: Service) => routesUsingService(s.name).length > 0;
  const shown = $derived(
    services.filter((s) =>
      filter === 'all' ? true : filter === 'private' ? !isPublic(s) : s.kind === filter,
    ),
  );
  const selectedName = $derived(decodeURIComponent(route.path.split('/')[2] ?? ''));
  const selected = $derived(services.find((s) => s.name === selectedName) ?? null);
  const health = $derived(services.map((s) => serviceHealth(live.status, s)));
  const down = $derived(services.filter((_, i) => health[i]!.status === 'critical'));
  const publicCount = $derived(services.filter(isPublic).length);
  const isNew = (name: string) => !(gw.current.services ?? []).some((s) => s.name === name);

  function address(s: Service): string {
    const where = s.kind === 'docker' ? (s.container ?? s.name) : (s.host ?? '');
    return `${where}:${Object.values(s.ports).join(',')}`;
  }

  const counts = $derived({
    all: services.length,
    docker: services.filter((s) => s.kind === 'docker').length,
    external: services.filter((s) => s.kind === 'external').length,
    private: services.filter((s) => !isPublic(s)).length,
  });
  const filters: { key: Filter; label: string }[] = [
    { key: 'all', label: 'All' },
    { key: 'docker', label: 'Containers' },
    { key: 'external', label: 'Other services' },
    { key: 'private', label: 'Not public' },
  ];

  const upstreams = $derived(selected ? serviceUpstreams(live.status, selected.name) : []);
  const logLines = $derived(
    selected ? live.logs.filter((l) => l.message.includes(selected.name)).slice(-6) : [],
  );
  const details = $derived(
    selected
      ? [
          {
            label: selected.kind === 'docker' ? 'Container' : 'Host',
            value: (selected.kind === 'docker' ? selected.container : selected.host) ?? '',
          },
          ...(selected.network ? [{ label: 'Network', value: selected.network }] : []),
          { label: 'Ports', value: Object.entries(selected.ports).map(([n, p]) => `${n} ${p}`).join(', ') },
          {
            label: 'Health check',
            value: selected.health
              ? `${selected.health.type.toUpperCase()}${selected.health.path ? ` ${selected.health.path}` : ''} every ${selected.health.interval ?? '10s'}`
              : 'None',
          },
          {
            label: 'Public via',
            value: routesUsingService(selected.name).map((r) => `${r.interface}:${r.listener}`).join(', ') || 'Not public',
          },
        ]
      : [],
  );

  function remove() {
    const target = selected;
    if (!target) return;
    edit((s) => {
      s.services = s.services.filter((x) => x.name !== target.name);
      for (const r of s.routes) r.targets = r.targets.filter((t) => t.service !== target.name);
      s.routes = s.routes.filter((r) => r.targets.length > 0);
    });
    navigate('/services');
  }
</script>

<PageHeader
  title="Services"
  subtitle="Everything Hearthgate can forward traffic to. A service is reachable from the internet only through a route."
>
  {#snippet actions()}
    <Button
      variant="secondary"
      onclick={() => {
        editing = null;
        newKind = 'external';
        editorOpen = true;
      }}>Add service</Button
    >
    <Button
      onclick={() => {
        editing = null;
        newKind = 'docker';
        editorOpen = true;
      }}>Add container</Button
    >
  {/snippet}
</PageHeader>

<div class="stats">
  <StatCard
    label="Services"
    value={services.length}
    detail="{plural(counts.docker, 'container')} · {plural(counts.external, 'other service')}"
  />
  <StatCard label="Public" value={publicCount} detail="Reachable through a route" />
  <StatCard label="Not public" value={services.length - publicCount} detail="Not reachable from the internet" />
  <StatCard
    label="Health"
    value={down.length ? `${down.length} down` : services.length ? 'All up' : '–'}
    detail={down.length ? down.map((s) => s.name).join(', ') : 'Every routed service answers'}
  />
</div>

<div class="filters" role="group" aria-label="Show">
  {#each filters as f (f.key)}
    <button type="button" class="filter" aria-pressed={filter === f.key} onclick={() => (filter = f.key)}>
      {f.label} <span class="count">{counts[f.key]}</span>
    </button>
  {/each}
</div>

<div class="layout" class:with-panel={!!selected}>
  {#if services.length === 0}
    <Card><p class="secondary">No services yet. Add a container or a program on your network.</p></Card>
  {:else}
    <div class="table-wrap">
      <table>
        <thead>
          <tr>
            <th scope="col">Service</th>
            <th scope="col">Address</th>
            <th scope="col" class="right">Exposure · health</th>
          </tr>
        </thead>
        <tbody>
          {#each shown as s (s.name)}
            {@const h = serviceHealth(live.status, s)}
            <tr class:selected={s.name === selectedName}>
              <td>
                <a href="/services/{encodeURIComponent(s.name)}" class="name" aria-current={s.name === selectedName ? 'true' : undefined}>{s.name}</a>
                <p class="small secondary">{s.kind === 'docker' ? 'Container' : 'Service on the network'}</p>
              </td>
              <td class="mono-small">{address(s)}</td>
              <td class="right badges">
                <Badge status={isPublic(s) ? 'public' : 'neutral'} dot>{isPublic(s) ? 'Public' : 'Not public'}</Badge>
                {#if isNew(s.name)}
                  <Badge status="neutral">Not applied</Badge>
                {:else}
                  <Badge status={h.status} dot>{h.label}</Badge>
                {/if}
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}

  {#if selected}
    {@const h = serviceHealth(live.status, selected)}
    <aside class="panel" aria-labelledby="panel-title">
      <div class="panel-head">
        <h2 id="panel-title">{selected.name}</h2>
        <Badge status={h.status} dot>{h.label}</Badge>
      </div>
      <KeyValue rows={details} />
      {#if upstreams.length}
        <section>
          <h3 class="label secondary">Targets</h3>
          <ul class="targets">
            {#each upstreams as u (u.port)}
              <li>
                <span class="dot" class:down={upstreamDown(u)} aria-hidden="true"></span>
                <span class="mono-small">{u.endpoint}</span>
                <span class="small secondary">
                  {upstreamDown(u) ? (u.ejected ? 'taken out after failures' : 'failing') : 'up'} · {u.active} open · {rate(u.rate.requests)}
                </span>
              </li>
            {/each}
          </ul>
        </section>
      {/if}
      <section>
        <h3 class="label secondary">Recent log lines</h3>
        {#if logLines.length}
          <!-- Scrollable, so keyboard users must be able to focus it (WCAG 2.1.1). -->
          <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
          <pre class="log" tabindex="0" aria-label="Recent log lines">{#each logLines as l (l.id)}[{clock(new Date(l.time))}] {l.message}
{/each}</pre>
        {:else}
          <p class="small secondary">Nothing logged about {selected.name} recently.</p>
        {/if}
      </section>
      <div class="panel-actions">
        <Button
          variant="secondary"
          size="small"
          onclick={() => {
            editing = selected;
            editorOpen = true;
          }}>Edit</Button
        >
        <Button variant="secondary" size="small" onclick={() => navigate(`/routes?service=${selected.name}`)}>
          {isPublic(selected) ? 'View routes' : 'Publish'}
        </Button>
        <span class="spacer"></span>
        <Button variant="ghost" size="small" onclick={() => (confirmOpen = true)}>Remove</Button>
      </div>
    </aside>
  {/if}
</div>

<ServiceEditor bind:open={editorOpen} original={editing} kind={newKind} />
<ConfirmDialog
  bind:open={confirmOpen}
  title="Remove {selected?.name}?"
  message={selected && isPublic(selected)
    ? `It is removed from ${plural(routesUsingService(selected.name).length, 'route')}; routes left without targets are removed too.`
    : 'It stops being a target for Hearthgate. The service itself keeps running.'}
  onconfirm={remove}
/>

<style>
  .stats {
    display: flex;
    gap: var(--hg-spacing-lg);
  }
  .filters {
    display: inline-flex;
    width: fit-content;
    padding: 2px;
    gap: 2px;
    background: var(--hg-color-bg-subtle);
    border-radius: var(--hg-radius-md);
  }
  .filter {
    padding: var(--hg-spacing-xs) var(--hg-spacing-md);
    border: none;
    border-radius: 6px;
    background: transparent;
    color: var(--hg-color-text-body);
    font: 500 14px/20px var(--hg-font-body);
    cursor: pointer;
  }
  .filter[aria-pressed='true'] {
    background: var(--hg-color-bg-surface);
    color: var(--hg-color-text-primary);
    box-shadow: 0 1px 2px rgb(11 20 38 / 0.08);
  }
  .layout {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: var(--hg-spacing-xl);
    align-items: start;
  }
  .layout.with-panel {
    grid-template-columns: minmax(0, 1fr) 368px;
  }
  .table-wrap {
    background: var(--hg-color-bg-surface);
    border: 1px solid var(--hg-color-border-default);
    border-radius: var(--hg-radius-lg);
    overflow: hidden;
  }
  table {
    width: 100%;
    border-collapse: collapse;
  }
  th {
    padding: var(--hg-spacing-md) var(--hg-spacing-lg);
    text-align: left;
    font: 500 11px/16px var(--hg-font-body);
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--hg-color-text-secondary);
    border-bottom: 1px solid var(--hg-color-border-default);
  }
  td {
    padding: var(--hg-spacing-md) var(--hg-spacing-lg);
    border-bottom: 1px solid var(--hg-color-border-default);
    vertical-align: middle;
  }
  tr:last-child td {
    border-bottom: none;
  }
  tr.selected td {
    background: var(--hg-color-accent-subtle);
  }
  tr.selected .secondary {
    color: var(--hg-color-text-body);
  }
  .name {
    color: var(--hg-color-text-primary);
    text-decoration: none;
    font-weight: 500;
  }
  .name:hover {
    text-decoration: underline;
  }
  .right {
    text-align: right;
  }
  .badges {
    white-space: nowrap;
  }
  .badges :global(.badge + .badge) {
    margin-left: var(--hg-spacing-sm);
  }
  .panel {
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-lg);
    padding: var(--hg-spacing-xl);
    background: var(--hg-color-bg-surface);
    border: 1px solid var(--hg-color-border-default);
    border-radius: var(--hg-radius-lg);
    position: sticky;
    top: var(--hg-spacing-xl);
  }
  .panel-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--hg-spacing-sm);
  }
  section {
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-sm);
  }
  .targets {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-xs);
  }
  .targets li {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--hg-spacing-sm);
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: var(--hg-radius-full);
    background: var(--hg-color-status-healthy-fg);
  }
  .dot.down {
    background: var(--hg-color-status-critical-fg);
  }
  .log {
    margin: 0;
    padding: var(--hg-spacing-md);
    border-radius: var(--hg-radius-md);
    background: var(--hg-ink-900);
    color: var(--hg-ink-50);
    font: 400 12px/18px var(--hg-font-mono);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    max-height: 220px;
    overflow: auto;
  }
  .panel-actions {
    display: flex;
    gap: var(--hg-spacing-sm);
  }
  .spacer {
    flex: 1;
  }
  @media (max-width: 1100px) {
    .layout.with-panel {
      grid-template-columns: minmax(0, 1fr);
    }
    .stats {
      flex-wrap: wrap;
    }
  }
</style>
