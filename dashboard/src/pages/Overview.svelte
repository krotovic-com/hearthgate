<script lang="ts">
  import { onMount } from 'svelte';
  import PageHeader from '../lib/layout/PageHeader.svelte';
  import Card from '../lib/ui/Card.svelte';
  import StatCard from '../lib/ui/StatCard.svelte';
  import Badge from '../lib/ui/Badge.svelte';
  import Button from '../lib/ui/Button.svelte';
  import KeyValue from '../lib/ui/KeyValue.svelte';
  import TrafficChart from '../lib/ui/TrafficChart.svelte';
  import { api, type TrafficBucket } from '../lib/api';
  import { live } from '../lib/live.svelte';
  import { gw } from '../lib/gateway.svelte';
  import { attentionItems, serviceHealth } from '../lib/health';
  import { ago, clock, duration, plural, portLabel } from '../lib/format';
  import { navigate } from '../lib/router.svelte';
  import UpdateSummary from '../lib/updates/UpdateSummary.svelte';
  import { loadUpdates, updates } from '../lib/updates.svelte';
  import { waitsReason } from '../lib/updates';

  let traffic = $state<TrafficBucket[]>([]);

  // "Later" hides a release until the next visit.
  let later = $state<string | null>(null);
  try {
    later = sessionStorage.getItem('hearthgate-update-later');
  } catch {
    /* storage may be blocked */
  }
  const update = $derived.by(() => {
    const info = updates.info;
    const a = info?.available;
    if (!info || !a || info.skipped.includes(a.version) || later === a.version) return null;
    return { info, a };
  });
  function hideUpdate(version: string) {
    later = version;
    try {
      sessionStorage.setItem('hearthgate-update-later', version);
    } catch {
      /* fine: hidden for this page only */
    }
  }

  onMount(() => {
    void loadUpdates();
    const load = () =>
      api
        .traffic()
        .then((t) => (traffic = t))
        .catch(() => {});
    load();
    const timer = setInterval(load, 60_000);
    return () => clearInterval(timer);
  });

  const status = $derived(live.status);
  const desired = $derived(gw.current);
  const interfaces = $derived(desired.interfaces ?? []);
  const services = $derived(desired.services ?? []);
  const routes = $derived(desired.routes ?? []);

  const ports = $derived(interfaces.flatMap((i) => i.listeners.map((l) => portLabel(l.port, l.protocol))));
  const health = $derived(services.map((s) => serviceHealth(status, s)));
  const down = $derived(health.filter((h) => h.status === 'critical').length);
  const running = $derived(health.filter((h) => h.status === 'healthy').length);
  const balanced = $derived(routes.filter((r) => r.targets.length > 1).length);
  const soonest = $derived(
    gw.certificates.length ? Math.min(...gw.certificates.map((c) => c.days_left)) : null,
  );
  const attention = $derived(attentionItems(status, desired, gw.certificates));

  // Changes from the history, and warnings and errors from the log.
  const activity = $derived(
    [
      ...gw.revisions.slice(0, 8).map((r) => ({
        key: `r${r.id}`,
        time: new Date(r.created_at),
        tone: 'neutral' as const,
        text: `${r.message} (revision ${r.id}, ${r.author.replace(/^dashboard:/, '')})`,
      })),
      ...live.logs
        .filter((l) => l.level === 'warn' || l.level === 'error')
        .slice(-8)
        .map((l) => ({
          key: `l${l.id}`,
          time: new Date(l.time),
          tone: l.level === 'error' ? ('critical' as const) : ('warning' as const),
          text: l.message,
        })),
    ]
      .sort((a, b) => b.time.getTime() - a.time.getTime())
      .slice(0, 7),
  );

  const subtitle = $derived(
    status
      ? [status.host.hostname, status.host.os, `Hearthgate ${status.version}`, `up ${duration(status.host.uptime)}`]
          .filter(Boolean)
          .join(' · ')
      : 'Connecting to the gateway…',
  );

  const gateway = $derived(
    status
      ? [
          { label: 'Host', value: status.host.hostname },
          { label: 'System', value: status.host.os ?? 'Unknown' },
          { label: 'Version', value: `${status.version} · up ${duration(status.host.uptime)}` },
          {
            label: 'Configuration',
            value: gw.revision
              ? `revision ${gw.revision.id} · ${ago(new Date(gw.revision.created_at))}`
              : 'none yet',
          },
          {
            label: 'Listening',
            value: status.listeners.length ? status.listeners.map((l) => l.socket).join(', ') : 'nothing',
          },
          {
            label: 'Upstreams',
            value: `${status.upstreams.length} · ${status.upstreams.reduce((s, u) => s + u.active, 0)} open connections`,
          },
          { label: 'Dashboard', value: location.host },
        ]
      : [],
  );
</script>

<PageHeader title="Overview" {subtitle}>
  {#snippet actions()}
    <Button variant="secondary" onclick={() => navigate('/history')}>View history</Button>
  {/snippet}
</PageHeader>

{#if update}
  <div class="update">
    <UpdateSummary
      status="available"
      title="Hearthgate {update.a.version} is available"
      description={waitsReason(update.info, update.a)}
    >
      {#snippet actions()}
        <Button variant="ghost" onclick={() => hideUpdate(update.a.version)}>Later</Button>
        <Button onclick={() => navigate(`/settings?section=updates${update.info.can_install ? '&install=1' : ''}`)}
          >Review update</Button
        >
      {/snippet}
    </UpdateSummary>
  </div>
{/if}

<div class="stats">
  <StatCard
    label="Public interfaces"
    value={interfaces.length}
    detail={ports.length ? `${plural(ports.length, 'port')} open` : 'Nothing exposed'}
  />
  <StatCard
    label="Services"
    value={services.length}
    detail={services.length ? `${down} down · ${running} running` : 'None yet'}
  />
  <StatCard
    label="Routes"
    value={routes.length}
    detail={balanced ? `${balanced} load-balanced` : 'One target each'}
  />
  <StatCard
    label="Certificates"
    value={gw.certificates.length ? `${gw.certificates.filter((c) => c.days_left > 0).length} valid` : '0'}
    detail={soonest === null ? 'None issued yet' : `Next expiry in ${plural(soonest, 'day')}`}
  />
</div>

{#if gw.loaded && interfaces.length === 0 && services.length === 0}
  <Card title="Publish your first service">
    <p class="secondary">
      Add the service you want to reach (a container or a program on this network), then a public
      interface with its domain, then a route between them. Nothing is reachable from the internet
      until you do.
    </p>
    <div class="row">
      <Button onclick={() => navigate('/services?new=external')}>Add a service</Button>
      <Button variant="secondary" onclick={() => navigate('/interfaces?new=1')}>Add a public interface</Button>
    </div>
  </Card>
{/if}

<div class="grid">
  <Card title="Traffic, last 24 hours">
    <TrafficChart buckets={traffic} />
  </Card>
  <Card title="Needs attention">
    {#if attention.length === 0}
      <p class="secondary">{status ? 'Nothing right now. Every service answers and every certificate is fresh.' : 'Checking…'}</p>
    {:else}
      <ul class="attention">
        {#each attention.slice(0, 5) as item (item.key)}
          <li>
            <div class="item-head">
              <p class="strong">{item.title}</p>
              <Badge status={item.status} dot>{item.badge}</Badge>
            </div>
            <p class="small secondary">{item.detail}</p>
            {#if item.href}
              <div><Button size="small" variant="secondary" onclick={() => navigate(item.href!)}>{item.action}</Button></div>
            {/if}
          </li>
        {/each}
      </ul>
    {/if}
  </Card>
  <Card title="Recent activity">
    {#if activity.length === 0}
      <p class="secondary">No changes or warnings yet.</p>
    {:else}
      <ul class="activity">
        {#each activity as a (a.key)}
          <li>
            <time class="mono-small secondary" datetime={a.time.toISOString()}>{clock(a.time)}</time>
            <span class="dot {a.tone}" aria-hidden="true"></span>
            <span>{a.text}</span>
          </li>
        {/each}
      </ul>
    {/if}
  </Card>
  <Card title="Gateway">
    <KeyValue rows={gateway} />
  </Card>
</div>

<style>
  .update {
    padding: var(--hg-spacing-lg) var(--hg-spacing-xl);
    background: var(--hg-color-bg-surface);
    border: 1px solid var(--hg-color-border-default);
    border-radius: var(--hg-radius-lg);
  }
  .stats {
    display: flex;
    gap: var(--hg-spacing-lg);
  }
  .grid {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 400px;
    gap: var(--hg-spacing-xl);
  }
  .row {
    display: flex;
    gap: var(--hg-spacing-sm);
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .attention {
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-md);
  }
  .attention li {
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-sm);
    padding: var(--hg-spacing-md) var(--hg-spacing-lg);
    border: 1px solid var(--hg-color-border-default);
    border-radius: var(--hg-radius-md);
  }
  .item-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--hg-spacing-sm);
  }
  .activity {
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-md);
  }
  .activity li {
    display: grid;
    grid-template-columns: 48px 8px 1fr;
    gap: var(--hg-spacing-md);
    align-items: baseline;
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: var(--hg-radius-full);
    align-self: center;
  }
  .dot.neutral {
    background: var(--hg-color-status-neutral-fg);
  }
  .dot.warning {
    background: var(--hg-color-status-warning-fg);
  }
  .dot.critical {
    background: var(--hg-color-status-critical-fg);
  }
  @media (max-width: 1100px) {
    .grid {
      grid-template-columns: minmax(0, 1fr);
    }
    .stats {
      flex-wrap: wrap;
    }
  }
</style>
