<script lang="ts">
  import { onMount } from 'svelte';
  import PageHeader from '../lib/layout/PageHeader.svelte';
  import Card from '../lib/ui/Card.svelte';
  import StatCard from '../lib/ui/StatCard.svelte';
  import Badge, { type BadgeStatus } from '../lib/ui/Badge.svelte';
  import Button from '../lib/ui/Button.svelte';
  import Toggle from '../lib/ui/Toggle.svelte';
  import TextField from '../lib/ui/TextField.svelte';
  import SettingRow from '../lib/ui/SettingRow.svelte';
  import Alert from '../lib/ui/Alert.svelte';
  import Dialog from '../lib/ui/Dialog.svelte';
  import ConfirmDialog from '../lib/ui/ConfirmDialog.svelte';
  import RuleEditor from '../lib/editors/RuleEditor.svelte';
  import AiPanel from '../lib/ai/AiPanel.svelte';
  import BlocklistEditor from '../lib/editors/BlocklistEditor.svelte';
  import { api, ApiError, type FirewallInfo, type FirewallRule, type RecentDrop } from '../lib/api';
  import { draft, editFirewall, gw } from '../lib/gateway.svelte';
  import { durationProblem, openPorts, parseNet, ruleRows, type RowKind } from '../lib/firewall';
  import { clock, duration, number, plural } from '../lib/format';

  let info = $state<FirewallInfo | null>(null);
  let recent = $state<RecentDrop[]>([]);
  let loadError = $state<string | null>(null);

  async function load() {
    try {
      [info, recent] = await Promise.all([api.firewall(), api.recentDrops()]);
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

  const fw = $derived(draft.firewall);
  const running = $derived(gw.current.firewall?.enabled ?? false);
  const interfaces = $derived(draft.interfaces);
  const rows = $derived(ruleRows(fw, interfaces));
  const open = $derived(openPorts(interfaces));

  const kinds: Record<RowKind, { label: string; status: BadgeStatus }> = {
    allow: { label: 'Allow', status: 'healthy' },
    block: { label: 'Block', status: 'critical' },
    limit: { label: 'Limit', status: 'warning' },
    open: { label: 'Open', status: 'public' },
  };

  // Dropped packets of the last hour, by port.
  const byPort = $derived.by(() => {
    const totals = new Map<string, number>();
    for (const d of recent) {
      const key = `${d.port}/${d.protocol}`;
      totals.set(key, (totals.get(key) ?? 0) + d.packets);
    }
    const sorted = [...totals.entries()].sort((a, b) => b[1] - a[1]).slice(0, 8);
    const max = sorted[0]?.[1] ?? 1;
    return sorted.map(([port, packets]) => ({ port, packets, share: Math.max(2, (packets / max) * 100), open: open.has(port) }));
  });

  const updated = $derived.by(() => {
    const times = (info?.feeds ?? []).map((f) => f.updated ?? 0).filter(Boolean);
    return times.length ? clock(new Date(Math.max(...times) * 1000)) : null;
  });
  const feedErrors = $derived((info?.feeds ?? []).filter((f) => f.error));

  // Rule editing.
  let editorOpen = $state(false);
  let editing = $state<FirewallRule | null>(null);
  let listsOpen = $state(false);
  let removing = $state<FirewallRule | null>(null);
  let confirmOpen = $state(false);

  function move(index: number, by: number) {
    editFirewall((f) => {
      const [rule] = f.rules.splice(index, 1);
      f.rules.splice(index + by, 0, rule!);
    });
  }

  // Automatic bans.
  let threshold = $state('');
  let banWindow = $state('');
  let banTime = $state('');
  $effect(() => {
    threshold = String(fw.auto_ban.threshold);
    banWindow = fw.auto_ban.window;
    banTime = fw.auto_ban.ban_time;
  });
  const thresholdError = $derived(/^[1-9]\d{0,5}$/.test(threshold) ? null : 'Use a whole number above 0.');
  const windowError = $derived(durationProblem(banWindow));
  const banTimeError = $derived(durationProblem(banTime));

  function saveAutoBan() {
    if (thresholdError || windowError || banTimeError) return;
    editFirewall((f) => {
      f.auto_ban = { ...f.auto_ban, threshold: Number(threshold), window: banWindow.trim(), ban_time: banTime.trim() };
    });
  }

  // Bans by hand.
  let banOpen = $state(false);
  let banAddress = $state('');
  let banDuration = $state('');
  let banError = $state<string | null>(null);
  let banBusy = $state(false);
  let unbanError = $state<string | null>(null);

  async function ban() {
    const net = parseNet(banAddress);
    if (!net || !/\/(32|128)$/.test(net)) {
      banError = 'Enter one address, like 203.0.113.7.';
      return;
    }
    if (banDuration.trim() && durationProblem(banDuration)) {
      banError = durationProblem(banDuration);
      return;
    }
    banBusy = true;
    banError = null;
    try {
      await api.ban(net.replace(/\/\d+$/, ''), banDuration.trim() || undefined);
      banOpen = false;
      await load();
    } catch (e) {
      banError = e instanceof ApiError || e instanceof Error ? e.message : String(e);
    } finally {
      banBusy = false;
    }
  }

  async function unban(address: string) {
    unbanError = null;
    try {
      await api.unban(address);
      await load();
    } catch (e) {
      unbanError = e instanceof Error ? e.message : String(e);
    }
  }

  const reasonLabels: Record<string, string> = {
    blocklist: 'blocklist',
    ban: 'banned',
    policy: 'not open',
    'rate-limit': 'rate limit',
  };
  let aiOpen = $state(false);
</script>

<PageHeader
  title="Firewall"
  subtitle="Every port is closed unless a public interface opens it. These rules filter the traffic that reaches an open port."
>
  {#snippet actions()}
    <Button variant="ghost" onclick={() => (aiOpen = true)}>Review rules with AI</Button>
    <Button variant="secondary" onclick={() => (listsOpen = true)}>Blocklists</Button>
    <Button
      onclick={() => {
        editing = null;
        editorOpen = true;
      }}>Add rule</Button
    >
  {/snippet}
</PageHeader>

{#if !fw.enabled}
  <Card title="The firewall is off">
    <p class="secondary">
      Hearthgate leaves this machine's packet filter alone. Turn it on to close every port that no public
      interface opens, and to use blocklists and automatic bans. After you apply the change, Hearthgate keeps
      it only if this dashboard can still reach it within 60 seconds.
    </p>
    <div>
      <Button onclick={() => editFirewall((f) => (f.enabled = true))}>Turn on the firewall</Button>
    </div>
  </Card>
{:else if !running}
  <Alert tone="neutral">The firewall turns on when you apply the draft.</Alert>
{/if}

{#if info?.error && running}
  <Alert>{info.error}</Alert>
{/if}
{#if loadError}
  <Alert>{loadError}</Alert>
{/if}
{#each feedErrors as f (f.name)}
  <Alert tone="warning">Can't download the blocklist {f.name}: {f.error}</Alert>
{/each}

<div class="stats">
  <StatCard
    label="Default policy"
    value={running ? 'Deny all' : 'Off'}
    detail={`${plural(open.size, 'port')} open on ${plural(interfaces.length, 'interface')}`}
  />
  <StatCard
    label="Blocked"
    value={info ? number(info.dropped.total) : '–'}
    detail={info?.dropped.total
      ? `${number(info.dropped.blocklist)} by lists · ${number(info.dropped.ban)} by bans`
      : 'Packets since the firewall started'}
  />
  <StatCard
    label="Blocklists"
    value={plural(fw.blocklists.filter((b) => b.enabled !== false).length + fw.countries.length, 'list')}
    detail={info?.feed_entries
      ? `${number(info.feed_entries)} addresses${updated ? ` · updated ${updated}` : ''}`
      : 'None loaded'}
  />
  <StatCard
    label="Banned now"
    value={info?.bans.length ?? 0}
    detail={fw.auto_ban.enabled ? `Auto-ban after ${fw.auto_ban.threshold} points` : 'Auto-ban off'}
  />
</div>

<div class="layout">
  <div class="column">
    <section class="rules" aria-labelledby="rules-title">
      <h2 id="rules-title" class="overline">Rules · checked top to bottom</h2>
      <ol>
        {#each rows as row (row.key)}
          <li class="rule">
            <span class="kind"><Badge status={kinds[row.kind].status}>{kinds[row.kind].label}</Badge></span>
            <div class="what">
              <p class="strong name" class:mono={row.index !== undefined}>{row.title}</p>
              <p class="small secondary">{row.detail}</p>
            </div>
            {#if row.index !== undefined}
              {@const rule = fw.rules[row.index]!}
              <div class="row-actions">
                <Button
                  variant="ghost"
                  size="small"
                  disabled={row.index === 0}
                  aria-label="Move {rule.name} up"
                  onclick={() => move(row.index!, -1)}>↑</Button
                >
                <Button
                  variant="ghost"
                  size="small"
                  disabled={row.index === fw.rules.length - 1}
                  aria-label="Move {rule.name} down"
                  onclick={() => move(row.index!, 1)}>↓</Button
                >
                <Button
                  variant="secondary"
                  size="small"
                  onclick={() => {
                    editing = rule;
                    editorOpen = true;
                  }}>Edit<span class="visually-hidden"> {rule.name}</span></Button
                >
                <Button
                  variant="ghost"
                  size="small"
                  onclick={() => {
                    removing = rule;
                    confirmOpen = true;
                  }}>Remove<span class="visually-hidden"> {rule.name}</span></Button
                >
              </div>
            {:else if row.key === 'lists'}
              <Button variant="ghost" size="small" onclick={() => (listsOpen = true)}>Edit<span class="visually-hidden"> blocklists</span></Button>
            {/if}
          </li>
        {/each}
        <li class="rule">
          <span class="kind"><Badge status="critical">Block</Badge></span>
          <div class="what">
            <p class="strong">Everything else</p>
            <p class="small secondary">Dropped without an answer</p>
          </div>
        </li>
      </ol>
      <p class="small secondary">Ports with no public interface never reach these rules. They are dropped first.</p>
    </section>

    <Card title="Settings">
      <div class="settings">
        <SettingRow label="Firewall" description="Close every port that no public interface opens.">
          <Toggle label="Firewall" checked={fw.enabled} onchange={(on) => editFirewall((f) => (f.enabled = on))} />
        </SettingRow>
        <SettingRow label="Allow the local network" description="Devices on the LAN reach every port, like SSH. Turn off to treat them like the internet.">
          <Toggle label="Allow the local network" checked={fw.allow_lan} onchange={(on) => editFirewall((f) => (f.allow_lan = on))} />
        </SettingRow>
        <SettingRow label="Answer ping" description="Reply to ICMP echo requests from anywhere.">
          <Toggle label="Answer ping" checked={fw.allow_ping} onchange={(on) => editFirewall((f) => (f.allow_ping = on))} />
        </SettingRow>
        <SettingRow
          label="Docker-published ports from the internet"
          description="Ports a container publishes with -p are reachable from the LAN only, unless this is on. Publish them through a public interface instead."
        >
          <Toggle
            label="Docker-published ports from the internet"
            checked={fw.allow_docker_ports}
            onchange={(on) => editFirewall((f) => (f.allow_docker_ports = on))}
          />
        </SettingRow>
        <SettingRow
          label="Automatic bans"
          description="Points for failed logins and TLS handshakes (1), unknown domains (2) and exploit probes (5). Never for the LAN or addresses that are never blocked."
        >
          <Toggle
            label="Automatic bans"
            checked={fw.auto_ban.enabled}
            onchange={(on) => editFirewall((f) => (f.auto_ban = { ...f.auto_ban, enabled: on }))}
          />
        </SettingRow>
        {#if fw.auto_ban.enabled}
          <div class="ban-fields">
            <TextField label="Points" bind:value={threshold} mono inputmode="numeric" error={thresholdError} onchange={saveAutoBan} />
            <TextField label="Within" bind:value={banWindow} mono error={windowError} onchange={saveAutoBan} />
            <TextField label="Ban for" bind:value={banTime} mono error={banTimeError} onchange={saveAutoBan} />
          </div>
        {/if}
      </div>
    </Card>
  </div>

  <div class="column side">
    <Card title="Blocked in the last hour, by port">
      {#if byPort.length === 0}
        <p class="small secondary">{running ? 'Nothing blocked in the last hour.' : 'Shown once the firewall is on.'}</p>
      {:else}
        <ul class="ports">
          {#each byPort as p (p.port)}
            <li>
              <div class="port-line">
                <span class="mono-small">{p.port}</span>
                <span class="small secondary">{p.open ? 'open' : 'not open'}</span>
                <span class="num small">{number(p.packets)}</span>
              </div>
              <svg class="bar" viewBox="0 0 100 4" preserveAspectRatio="none" aria-hidden="true">
                <rect class="track" width="100" height="4" rx="2" />
                <rect class="fill" class:open={p.open} width={p.share} height="4" rx="2" />
              </svg>
            </li>
          {/each}
        </ul>
        <ul class="sources" aria-label="Recent sources">
          {#each recent.slice(0, 5) as d (d.source + d.port + d.reason)}
            <li class="small"><span class="mono-small">{d.source}</span> <span class="secondary">→ {d.port}/{d.protocol} · {reasonLabels[d.reason] ?? d.reason}</span></li>
          {/each}
        </ul>
      {/if}
    </Card>

    <Card title="Banned now">
      {#snippet actions()}
        <Button
          variant="secondary"
          size="small"
          disabled={!running}
          onclick={() => {
            banAddress = '';
            banDuration = '';
            banError = null;
            banOpen = true;
          }}>Ban an address</Button
        >
      {/snippet}
      {#if unbanError}<Alert>{unbanError}</Alert>{/if}
      {#if !info || info.bans.length === 0}
        <p class="small secondary">No one is banned.</p>
      {:else}
        <ul class="bans">
          {#each info.bans as b (b.address)}
            <li>
              <div class="what">
                <p class="mono-small">{b.address}</p>
                <p class="small secondary">{duration(b.seconds_left)} left</p>
              </div>
              <Button variant="ghost" size="small" onclick={() => unban(b.address)}>
                Unban<span class="visually-hidden"> {b.address}</span>
              </Button>
            </li>
          {/each}
        </ul>
      {/if}
    </Card>
  </div>
</div>

<RuleEditor bind:open={editorOpen} original={editing} />
<BlocklistEditor bind:open={listsOpen} />
<AiPanel
  bind:open={aiOpen}
  scope={{ kind: 'firewall' }}
  title="AI rule review"
  description="The model checks your rules against the open ports, recent drops and bans. Nothing changes until you apply a fix from your draft."
/>

<ConfirmDialog
  bind:open={confirmOpen}
  title="Remove {removing?.name}?"
  message="The rule stops applying when you apply the draft."
  onconfirm={() => {
    const name = removing?.name;
    editFirewall((f) => (f.rules = f.rules.filter((r) => r.name !== name)));
  }}
/>
<Dialog bind:open={banOpen} title="Ban an address" description="It can't reach any port until the ban ends. Bans take effect at once.">
  <TextField label="Address" bind:value={banAddress} mono placeholder="203.0.113.7" />
  <TextField
    label="For"
    bind:value={banDuration}
    mono
    placeholder={fw.auto_ban.ban_time}
    hint="Like 1h or 7d. Leave empty for the automatic ban time."
  />
  {#if banError}<Alert>{banError}</Alert>{/if}
  {#snippet footer()}
    <Button variant="secondary" onclick={() => (banOpen = false)}>Cancel</Button>
    <Button busy={banBusy} onclick={ban}>Ban</Button>
  {/snippet}
</Dialog>

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
  .rules {
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-md);
  }
  .overline {
    font: 600 12px/16px var(--hg-font-body);
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--hg-color-text-secondary);
  }
  ol {
    list-style: none;
    margin: 0;
    padding: 0;
    background: var(--hg-color-bg-surface);
    border: 1px solid var(--hg-color-border-default);
    border-radius: var(--hg-radius-lg);
  }
  .rule {
    display: flex;
    align-items: center;
    gap: var(--hg-spacing-lg);
    padding: var(--hg-spacing-md) var(--hg-spacing-lg);
  }
  .rule + .rule {
    border-top: 1px solid var(--hg-color-border-default);
  }
  .kind {
    width: 64px;
    flex-shrink: 0;
  }
  .what {
    flex: 1;
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .name.mono {
    font-family: var(--hg-font-mono);
  }
  .row-actions {
    display: flex;
    gap: var(--hg-spacing-2xs);
  }
  .settings {
    margin: 0 calc(-1 * var(--hg-spacing-xl)) calc(-1 * var(--hg-spacing-xl));
  }
  .ban-fields {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: var(--hg-spacing-md);
    padding: 0 var(--hg-spacing-xl) var(--hg-spacing-lg);
  }
  .ports,
  .sources,
  .bans {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-md);
  }
  .sources {
    gap: var(--hg-spacing-xs);
    padding-top: var(--hg-spacing-md);
    border-top: 1px solid var(--hg-color-border-default);
  }
  .port-line {
    display: flex;
    gap: var(--hg-spacing-sm);
    align-items: baseline;
  }
  .port-line .num {
    margin-left: auto;
  }
  .bar {
    display: block;
    width: 100%;
    height: 4px;
    margin-top: var(--hg-spacing-xs);
  }
  .bar .track {
    fill: var(--hg-color-bg-subtle);
  }
  .bar .fill {
    fill: var(--hg-color-border-strong);
  }
  .bar .fill.open {
    fill: var(--hg-color-status-warning-fg);
  }
  .bans li {
    display: flex;
    align-items: center;
    gap: var(--hg-spacing-md);
  }
  @media (max-width: 1100px) {
    .stats {
      flex-wrap: wrap;
    }
    .layout {
      grid-template-columns: minmax(0, 1fr);
    }
  }
</style>
