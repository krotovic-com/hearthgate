<script lang="ts">
  import PageHeader from '../lib/layout/PageHeader.svelte';
  import Card from '../lib/ui/Card.svelte';
  import Button from '../lib/ui/Button.svelte';
  import Badge from '../lib/ui/Badge.svelte';
  import Icon from '../lib/ui/Icon.svelte';
  import RouteEditor from '../lib/editors/RouteEditor.svelte';
  import type { Route } from '../lib/api';
  import { live } from '../lib/live.svelte';
  import { draft } from '../lib/gateway.svelte';
  import { serviceHealth } from '../lib/health';
  import { portLabel, strategyLabels } from '../lib/format';
  import { navigate, param, route as current } from '../lib/router.svelte';

  // Map geometry: fixed node height so the lines can be drawn without
  // measuring the DOM.
  const NODE = 62;
  const GAP = 16;
  const PITCH = NODE + GAP;
  const LINK_WIDTH = 200;

  const selectedName = $derived(decodeURIComponent(current.path.split('/')[2] ?? ''));
  const creating = $derived(selectedName === 'new');
  const selected = $derived(draft.routes.find((r) => r.name === selectedName) ?? null);
  const preset = $derived({ interface: param('interface') ?? undefined, service: param('service') ?? undefined });

  // Opening /routes?service=x with no route for it starts a new one.
  $effect(() => {
    const service = param('service');
    if (service && !selectedName) {
      const existing = draft.routes.find((r) => r.targets.some((t) => t.service === service));
      navigate(existing ? `/routes/${existing.name}` : `/routes/new?service=${service}`, true);
    }
  });

  const left = $derived(
    draft.interfaces.flatMap((i) =>
      i.listeners
        .filter((l) => l.mode !== 'http' || draft.routes.some((r) => r.interface === i.name && r.listener === l.port))
        .map((l) => ({ key: `${i.name}:${l.port}`, iface: i, listener: l })),
    ),
  );
  const routedServices = $derived(
    draft.services.filter((s) => draft.routes.some((r) => r.targets.some((t) => t.service === s.name))),
  );
  const unrouted = $derived(draft.services.filter((s) => !routedServices.includes(s)));
  const rows = $derived(Math.max(left.length, routedServices.length, 1));
  const height = $derived(rows * PITCH - GAP);

  const y = (i: number) => i * PITCH + NODE / 2;
  const edges = $derived(
    draft.routes.flatMap((r) =>
      r.targets.map((t, ti) => {
        const from = left.findIndex((n) => n.iface.name === r.interface && n.listener.port === r.listener);
        const to = routedServices.findIndex((s) => s.name === t.service);
        const svc = draft.services.find((s) => s.name === t.service);
        const down = svc ? serviceHealth(live.status, svc).status === 'critical' : false;
        return {
          key: `${r.name}-${ti}`,
          route: r.name,
          from,
          to,
          down,
          d: `M 0 ${y(from)} C ${LINK_WIDTH / 2} ${y(from)}, ${LINK_WIDTH / 2} ${y(to)}, ${LINK_WIDTH} ${y(to)}`,
        };
      }),
    ).filter((e) => e.from >= 0 && e.to >= 0),
  );

  function routesFrom(key: string): Route[] {
    return draft.routes.filter((r) => `${r.interface}:${r.listener}` === key);
  }

  function describe(r: Route): string {
    const where = [r.hosts?.length ? r.hosts.join(', ') : null, r.path_prefix && r.path_prefix !== '/' ? r.path_prefix : null]
      .filter(Boolean)
      .join(' ');
    const targets = r.targets.map((t) => `${t.service}:${t.port}`).join(', ');
    const lb = r.targets.length > 1 ? ` · ${strategyLabels[r.strategy ?? 'round-robin']}` : '';
    return `${where ? `${where} → ` : ''}${targets}${lb}`;
  }
</script>

<PageHeader
  title="Routes"
  subtitle="Connect public interfaces to services. A route with two or more targets is load-balanced."
>
  {#snippet actions()}
    <Button onclick={() => navigate('/routes/new')}>New route</Button>
  {/snippet}
</PageHeader>

<div class="layout" class:with-panel={creating || !!selected}>
  <div class="main">
    <Card>
      {#if draft.routes.length === 0 && left.length === 0}
        <p class="secondary">No public interfaces yet. Add one, then connect it to a service here.</p>
      {:else}
        <div class="map" aria-hidden="true">
          <p class="label secondary">Public interfaces</p>
          <span></span>
          <p class="label secondary right">Services</p>
          <div class="column">
            {#each left as n (n.key)}
              {@const rs = routesFrom(n.key)}
              <button
                type="button"
                tabindex="-1"
                class="node"
                class:active={rs.some((r) => r.name === selectedName)}
                onclick={() => navigate(rs[0] ? `/routes/${rs[0].name}` : `/routes/new?interface=${n.iface.name}`)}
              >
                <Icon name="globe" />
                <span class="node-text">
                  <span class="strong">{n.iface.domains?.[0] ?? n.iface.name}</span>
                  <span class="mono-small secondary">{portLabel(n.listener.port, n.listener.protocol)}{rs.length > 1 ? ` · ${rs.length} routes` : ''}</span>
                </span>
              </button>
            {/each}
          </div>
          <svg width={LINK_WIDTH} {height} viewBox="0 0 {LINK_WIDTH} {height}" class="links">
            {#each edges as e (e.key)}
              <path
                d={e.d}
                class="edge"
                class:selected={e.route === selectedName}
                class:down={e.down}
              />
            {/each}
          </svg>
          <div class="column">
            {#each routedServices as s (s.name)}
              {@const h = serviceHealth(live.status, s)}
              <div class="node" class:down={h.status === 'critical'}>
                <Icon name={s.kind === 'docker' ? 'container' : 'server'} />
                <span class="node-text">
                  <span class="strong">{s.name}</span>
                  <span class="mono-small secondary">{s.kind === 'docker' ? s.container : s.host}:{Object.values(s.ports).join(',')}{h.status === 'critical' ? ' · down' : ''}</span>
                </span>
              </div>
            {/each}
            {#if unrouted.length}
              <p class="label secondary unrouted-label">Not public</p>
              {#each unrouted as s (s.name)}
                <button type="button" tabindex="-1" class="node ghost" onclick={() => navigate(`/routes/new?service=${s.name}`)}>
                  <Icon name={s.kind === 'docker' ? 'container' : 'server'} />
                  <span class="node-text">
                    <span>{s.name}</span>
                    <span class="mono-small secondary">{s.kind === 'docker' ? s.container : s.host}</span>
                  </span>
                </button>
              {/each}
            {/if}
          </div>
        </div>
        <ul class="legend small secondary" aria-hidden="true">
          <li><span class="line"></span>Route</li>
          <li><span class="line selected"></span>Selected route</li>
          <li><span class="line down"></span>Target down</li>
        </ul>
      {/if}
    </Card>

    <Card title="All routes">
      {#if draft.routes.length === 0}
        <p class="secondary">No routes yet.</p>
      {:else}
        <ul class="route-list">
          {#each draft.routes as r (r.name)}
            {@const i = draft.interfaces.find((x) => x.name === r.interface)}
            <li>
              <a href="/routes/{encodeURIComponent(r.name)}" aria-current={r.name === selectedName ? 'true' : undefined}>
                <span class="strong">{r.name}</span>
                <span class="mono-small secondary">{i?.domains?.[0] ?? r.interface}:{r.listener} → {describe(r)}</span>
              </a>
              {#if r.targets.length > 1}<Badge status="public">{r.targets.length} targets</Badge>{/if}
            </li>
          {/each}
        </ul>
      {/if}
    </Card>
  </div>

  {#if creating || selected}
    {#key selectedName + current.search}
      <RouteEditor
        original={selected}
        {preset}
        onclose={() => navigate('/routes')}
        onsaved={(name) => navigate(`/routes/${name}`)}
      />
    {/key}
  {/if}
</div>

<style>
  .layout {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: var(--hg-spacing-xl);
    align-items: start;
  }
  .layout.with-panel {
    grid-template-columns: minmax(0, 1fr) 400px;
  }
  .main {
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-xl);
    min-width: 0;
  }
  .map {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 200px minmax(0, 1fr);
    row-gap: var(--hg-spacing-md);
  }
  .right {
    text-align: right;
  }
  .column {
    display: flex;
    flex-direction: column;
    gap: 16px;
    min-width: 0;
  }
  .node {
    display: flex;
    align-items: center;
    gap: var(--hg-spacing-md);
    height: 62px;
    padding: 0 var(--hg-spacing-lg);
    border: 1px solid var(--hg-color-border-strong);
    border-radius: var(--hg-radius-md);
    background: var(--hg-color-bg-surface);
    color: var(--hg-color-text-primary);
    font: inherit;
    text-align: left;
    min-width: 0;
  }
  button.node {
    cursor: pointer;
  }
  .node.active {
    border: 2px solid var(--hg-color-accent-default);
    padding: 0 calc(var(--hg-spacing-lg) - 1px);
  }
  .node.down {
    border-color: var(--hg-color-status-critical-fg);
  }
  .node.ghost {
    border-style: dashed;
    border-color: var(--hg-color-border-strong);
    color: var(--hg-color-text-secondary);
  }
  .node-text {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .node-text > span {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .unrouted-label {
    margin-top: var(--hg-spacing-lg);
  }
  .links {
    display: block;
    overflow: visible;
  }
  .edge {
    fill: none;
    stroke: var(--hg-color-border-strong);
    stroke-width: 1.5;
  }
  .edge.selected {
    stroke: var(--hg-color-accent-default);
    stroke-width: 2;
  }
  .edge.down {
    stroke: var(--hg-color-status-critical-fg);
    stroke-dasharray: 4 3;
  }
  .legend {
    display: flex;
    gap: var(--hg-spacing-xl);
    list-style: none;
    margin: var(--hg-spacing-md) 0 0;
    padding: 0;
  }
  .legend li {
    display: flex;
    align-items: center;
    gap: var(--hg-spacing-sm);
  }
  .line {
    width: 20px;
    border-top: 2px solid var(--hg-color-border-strong);
  }
  .line.selected {
    border-color: var(--hg-color-accent-default);
  }
  .line.down {
    border-top-style: dashed;
    border-color: var(--hg-color-status-critical-fg);
  }
  .route-list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
  }
  .route-list li {
    display: flex;
    align-items: center;
    gap: var(--hg-spacing-md);
    border-top: 1px solid var(--hg-color-border-default);
  }
  .route-list li:first-child {
    border-top: none;
  }
  .route-list a {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    padding: var(--hg-spacing-md) 0;
    color: var(--hg-color-text-primary);
    text-decoration: none;
  }
  .route-list a[aria-current='true'] .strong {
    color: var(--hg-color-text-link);
  }
  .route-list a:hover .strong {
    text-decoration: underline;
  }
  @media (max-width: 1100px) {
    .layout.with-panel {
      grid-template-columns: minmax(0, 1fr);
    }
  }
</style>
