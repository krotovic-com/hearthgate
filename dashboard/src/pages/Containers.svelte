<script lang="ts">
  import { onMount } from 'svelte';
  import PageHeader from '../lib/layout/PageHeader.svelte';
  import StatCard from '../lib/ui/StatCard.svelte';
  import Badge, { type BadgeStatus } from '../lib/ui/Badge.svelte';
  import Button from '../lib/ui/Button.svelte';
  import Icon from '../lib/ui/Icon.svelte';
  import Alert from '../lib/ui/Alert.svelte';
  import ConfirmDialog from '../lib/ui/ConfirmDialog.svelte';
  import ContainerEditor from '../lib/editors/ContainerEditor.svelte';
  import StackEditor from '../lib/editors/StackEditor.svelte';
  import LogsDialog from '../lib/editors/LogsDialog.svelte';
  import { api, type DockerContainerInfo, type DockerInfo, type ImageUpdate, type ManagedContainer, type Stack } from '../lib/api';
  import { draft, edit, gw } from '../lib/gateway.svelte';
  import { shortImage } from '../lib/docker';
  import { plural } from '../lib/format';

  let info = $state<DockerInfo | null>(null);
  let loadError = $state<string | null>(null);
  let actionError = $state<string | null>(null);
  let busy = $state<string | null>(null);
  let updates = $state<ImageUpdate[] | null>(null);
  let checking = $state(false);

  async function load() {
    try {
      info = await api.docker();
      loadError = null;
    } catch (e) {
      loadError = e instanceof Error ? e.message : String(e);
    }
  }

  onMount(() => {
    load();
    const timer = setInterval(load, 5000);
    return () => clearInterval(timer);
  });

  const live = $derived(new Map((info?.containers ?? []).map((c) => [c.name, c])));
  const applied = $derived(new Set((gw.current.containers ?? []).map((c) => c.name)));
  const appliedStacks = $derived(new Set((gw.current.stacks ?? []).map((s) => s.name)));
  const stackContainers = (stack: string) =>
    (info?.containers ?? []).filter((c) => c.owner.kind === 'stack' && c.owner.stack === stack);
  const all = $derived(info?.containers ?? []);
  const running = $derived(all.filter((c) => c.state === 'running').length);
  const outdated = $derived(new Set((updates ?? []).filter((u) => u.update_available).map((u) => u.image)));

  function badge(c: DockerContainerInfo | undefined, pending: boolean): { status: BadgeStatus; label: string } {
    if (pending) return { status: 'neutral', label: 'Not applied' };
    if (!c) return { status: 'neutral', label: 'Unknown' };
    if (c.state === 'running') {
      if (c.health === 'unhealthy') return { status: 'critical', label: 'Unhealthy' };
      if (c.health === 'starting') return { status: 'warning', label: 'Starting' };
      return { status: 'healthy', label: 'Running' };
    }
    if (c.state === 'pulling') return { status: 'warning', label: 'Pulling image' };
    if (c.state === 'missing') return { status: 'warning', label: 'Not created yet' };
    if (c.state === 'restarting') return { status: 'warning', label: 'Restarting' };
    if (c.state === 'exited' || c.state === 'created') return { status: 'neutral', label: 'Stopped' };
    return { status: 'critical', label: c.state };
  }

  async function act(key: string, run: () => Promise<void>) {
    busy = key;
    actionError = null;
    try {
      await run();
      await load();
    } catch (e) {
      actionError = e instanceof Error ? e.message : String(e);
    } finally {
      busy = null;
    }
  }

  async function checkUpdates() {
    checking = true;
    actionError = null;
    try {
      updates = await api.imageUpdates();
    } catch (e) {
      actionError = e instanceof Error ? e.message : String(e);
    } finally {
      checking = false;
    }
  }

  // Dialogs.
  let containerOpen = $state(false);
  let editingContainer = $state<ManagedContainer | null>(null);
  let stackOpen = $state(false);
  let editingStack = $state<Stack | null>(null);
  let logsOpen = $state(false);
  let logsName = $state('');
  let removing = $state<{ kind: 'container' | 'stack'; name: string } | null>(null);
  let removeOpen = $state(false);

  function showLogs(name: string) {
    logsName = name;
    logsOpen = true;
  }
</script>

<PageHeader
  title="Containers"
  subtitle="Docker containers and Compose stacks that Hearthgate runs for you. They join Hearthgate's own network, so routes reach them without publishing ports."
>
  {#snippet actions()}
    <Button variant="secondary" busy={checking} disabled={!info?.available} onclick={checkUpdates}>Check for updates</Button>
    <Button
      variant="secondary"
      onclick={() => {
        editingStack = null;
        stackOpen = true;
      }}>Add stack</Button
    >
    <Button
      onclick={() => {
        editingContainer = null;
        containerOpen = true;
      }}>Add container</Button
    >
  {/snippet}
</PageHeader>

{#if info && !info.enabled}
  <Alert tone="neutral">
    Managed Docker is off. Hearthgate needs access to the Docker socket (add the hearthgate user to the docker group, or
    SupplementaryGroups=docker in its service), and docker.enabled and docker.manage on in the bootstrap file.
  </Alert>
{:else if info?.error}
  <Alert>Can't reach Docker: {info.error}</Alert>
{/if}
{#if loadError}<Alert>{loadError}</Alert>{/if}
{#if actionError}<Alert>{actionError}</Alert>{/if}
{#each info?.problems ?? [] as problem (problem)}
  <Alert tone="warning">{problem}</Alert>
{/each}

<div class="stats">
  <StatCard label="Running" value={running} detail={`of ${plural(all.length, 'container')}`} />
  <StatCard label="Stacks" value={draft.stacks.length} detail={plural(all.filter((c) => c.owner.kind === 'stack').length, 'container')} />
  <StatCard label="Network" value={info?.subnet ?? '–'} detail="hearthgate (Docker bridge)" />
  <StatCard
    label="Updates"
    value={updates ? outdated.size : '–'}
    detail={updates ? (outdated.size ? 'Newer images available' : 'Every image is current') : 'Not checked yet'}
  />
</div>

<section class="group" aria-labelledby="containers-title">
  <h2 id="containers-title" class="overline">Containers</h2>
  {#if draft.containers.length === 0}
    <div class="empty">
      <p class="strong">No containers yet</p>
      <p class="small secondary">Add one from an image, then point a route at it to publish it.</p>
    </div>
  {:else}
    <ul class="rows">
      {#each draft.containers as container (container.name)}
        {@const c = live.get(container.name)}
        {@const state = badge(c, !applied.has(container.name))}
        <li class="row">
          <Icon name="container" />
          <div class="name">
            <p class="strong">{container.name}</p>
            <p class="mono-small secondary">{shortImage(container.image)}</p>
          </div>
          <p class="mono-small address">{c?.address ?? ''}</p>
          <p class="small secondary detail">
            {#if outdated.has(container.image)}<span class="update">Update available · </span>{/if}{c?.status ?? ''}
          </p>
          <Badge status={state.status} dot={state.status === 'healthy'}>{state.label}</Badge>
          <div class="actions">
            <Button variant="ghost" size="small" disabled={!c || c.state === 'missing'} onclick={() => showLogs(container.name)}
              >Logs<span class="visually-hidden"> of {container.name}</span></Button
            >
            <Button
              variant="ghost"
              size="small"
              busy={busy === `restart:${container.name}`}
              disabled={c?.state !== 'running'}
              onclick={() => act(`restart:${container.name}`, () => api.restartContainer(container.name))}
              >Restart<span class="visually-hidden"> {container.name}</span></Button
            >
            <Button
              variant="ghost"
              size="small"
              busy={busy === `update:${container.name}`}
              disabled={!applied.has(container.name)}
              onclick={() => act(`update:${container.name}`, () => api.updateContainer(container.name))}
              >Update<span class="visually-hidden"> {container.name}</span></Button
            >
            <Button
              variant="secondary"
              size="small"
              onclick={() => {
                editingContainer = container;
                containerOpen = true;
              }}>Edit<span class="visually-hidden"> {container.name}</span></Button
            >
            <Button
              variant="ghost"
              size="small"
              onclick={() => {
                removing = { kind: 'container', name: container.name };
                removeOpen = true;
              }}>Remove<span class="visually-hidden"> {container.name}</span></Button
            >
          </div>
        </li>
      {/each}
    </ul>
  {/if}
</section>

<section class="group" aria-labelledby="stacks-title">
  <h2 id="stacks-title" class="overline">Stacks</h2>
  {#if draft.stacks.length === 0}
    <div class="empty">
      <p class="strong">No stacks yet</p>
      <p class="small secondary">Paste a Compose file to run several containers together.</p>
    </div>
  {/if}
  {#each draft.stacks as stack (stack.name)}
    {@const status = info?.stacks.find((s) => s.name === stack.name)}
    {@const members = stackContainers(stack.name)}
    {@const pending = !appliedStacks.has(stack.name)}
    <div class="stack">
      <div class="stack-head">
        <div class="name">
          <p class="strong">{stack.name}</p>
          <p class="small secondary">
            {#if pending}Not applied yet{:else if status}{status.running} of {plural(status.total, 'container')} running{:else}Waiting for Docker{/if}{stack.running === false ? ' · stopped' : ''}
          </p>
        </div>
        <div class="actions">
          <Button
            variant="ghost"
            size="small"
            busy={busy === `stack:${stack.name}`}
            disabled={pending || stack.running === false}
            onclick={() => act(`stack:${stack.name}`, () => api.updateStack(stack.name))}
            >Update<span class="visually-hidden"> stack {stack.name}</span></Button
          >
          <Button
            variant="secondary"
            size="small"
            onclick={() => {
              editingStack = stack;
              stackOpen = true;
            }}>Edit<span class="visually-hidden"> stack {stack.name}</span></Button
          >
          <Button
            variant="ghost"
            size="small"
            onclick={() => {
              removing = { kind: 'stack', name: stack.name };
              removeOpen = true;
            }}>Remove<span class="visually-hidden"> stack {stack.name}</span></Button
          >
        </div>
      </div>
      {#if status?.error}<Alert>{status.error}</Alert>{/if}
      {#if members.length}
        <ul class="rows inner">
          {#each members as c (c.name)}
            {@const state = badge(c, false)}
            <li class="row">
              <Icon name="container" />
              <div class="name">
                <p class="strong mono-name">{c.name}</p>
                <p class="mono-small secondary">{shortImage(c.image)}</p>
              </div>
              <p class="mono-small address">{c.address ?? ''}</p>
              <p class="small secondary detail">
                {#if outdated.has(c.image)}<span class="update">Update available · </span>{/if}{c.status}
              </p>
              <Badge status={state.status} dot={state.status === 'healthy'}>{state.label}</Badge>
              <div class="actions">
                <Button variant="ghost" size="small" onclick={() => showLogs(c.name)}
                  >Logs<span class="visually-hidden"> of {c.name}</span></Button
                >
                <Button
                  variant="ghost"
                  size="small"
                  busy={busy === `restart:${c.name}`}
                  disabled={c.state !== 'running'}
                  onclick={() => act(`restart:${c.name}`, () => api.restartContainer(c.name))}
                  >Restart<span class="visually-hidden"> {c.name}</span></Button
                >
              </div>
            </li>
          {/each}
        </ul>
      {/if}
    </div>
  {/each}
</section>

<ContainerEditor bind:open={containerOpen} original={editingContainer} />
<StackEditor bind:open={stackOpen} original={editingStack} />
<LogsDialog bind:open={logsOpen} name={logsName} />
<ConfirmDialog
  bind:open={removeOpen}
  title="Remove {removing?.kind === 'stack' ? 'stack ' : ''}{removing?.name}?"
  message={removing?.kind === 'stack'
    ? 'Its containers are removed when you apply the draft. Named volumes are kept.'
    : 'The container is removed when you apply the draft. Its volumes are kept. Services that point at it stop working.'}
  onconfirm={() => {
    const target = removing;
    if (!target) return;
    edit((s) => {
      if (target.kind === 'stack') s.stacks = (s.stacks ?? []).filter((x) => x.name !== target.name);
      else s.containers = (s.containers ?? []).filter((x) => x.name !== target.name);
    });
  }}
/>

<style>
  .stats {
    display: flex;
    gap: var(--hg-spacing-lg);
  }
  .group {
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
  .rows {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .rows,
  .empty,
  .stack {
    background: var(--hg-color-bg-surface);
    border: 1px solid var(--hg-color-border-default);
    border-radius: var(--hg-radius-lg);
  }
  .empty {
    padding: var(--hg-spacing-xl);
    text-align: center;
  }
  .row {
    display: flex;
    align-items: center;
    gap: var(--hg-spacing-md);
    padding: var(--hg-spacing-md) var(--hg-spacing-lg);
  }
  .row + .row {
    border-top: 1px solid var(--hg-color-border-default);
  }
  .name {
    width: 220px;
    flex-shrink: 0;
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .mono-name {
    font-family: var(--hg-font-mono);
    font-size: 13px;
  }
  .address {
    width: 100px;
    flex-shrink: 0;
  }
  .detail {
    flex: 1;
    min-width: 0;
  }
  .update {
    color: var(--hg-color-status-warning-fg);
    font-weight: 500;
  }
  .actions {
    display: flex;
    gap: var(--hg-spacing-2xs);
    flex-shrink: 0;
  }
  .stack {
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-md);
    padding: var(--hg-spacing-lg);
  }
  .stack-head {
    display: flex;
    align-items: center;
    gap: var(--hg-spacing-md);
  }
  .stack-head .name {
    flex: 1;
    width: auto;
  }
  .rows.inner {
    border-radius: var(--hg-radius-md);
  }
  @media (max-width: 1100px) {
    .stats {
      flex-wrap: wrap;
    }
    .row {
      flex-wrap: wrap;
    }
  }
</style>
