<script lang="ts">
  // The route form in the Routes page's side panel.
  import { untrack } from 'svelte';
  import Button from '../ui/Button.svelte';
  import TextField from '../ui/TextField.svelte';
  import Select from '../ui/Select.svelte';
  import Toggle from '../ui/Toggle.svelte';
  import Icon from '../ui/Icon.svelte';
  import type { Route, Strategy } from '../api';
  import { draft, edit } from '../gateway.svelte';
  import { list, nameProblem, slug } from './common';
  import { modeLabels, portLabel, strategyLabels } from '../format';

  let {
    original = null,
    preset = {},
    onclose,
    onsaved,
  }: {
    original?: Route | null;
    preset?: { interface?: string; service?: string };
    onclose: () => void;
    onsaved: (name: string) => void;
  } = $props();

  let name = $state('');
  let nameTouched = $state(false);
  let iface = $state('');
  let listener = $state(0);
  let hosts = $state('');
  let pathPrefix = $state('/');
  let strip = $state(false);
  let strategy = $state<Strategy>('round-robin');
  let targets = $state<{ service: string; port: string; weight: string }[]>([]);
  let tried = $state(false);

  // The page remounts the editor for each route, so this runs once.
  untrack(() => {
    const r = original;
    const firstIface = draft.interfaces.find((i) => i.name === preset.interface) ?? draft.interfaces[0];
    const firstService = draft.services.find((s) => s.name === preset.service) ?? draft.services[0];
    name = r?.name ?? '';
    nameTouched = !!r;
    iface = r?.interface ?? firstIface?.name ?? '';
    const ifaceObj = draft.interfaces.find((i) => i.name === iface);
    listener =
      r?.listener ?? ifaceObj?.listeners.find((l) => l.mode === 'https')?.port ?? ifaceObj?.listeners[0]?.port ?? 0;
    hosts = (r?.hosts ?? []).join(', ');
    pathPrefix = r?.path_prefix ?? '/';
    strip = r?.strip_prefix ?? false;
    strategy = r?.strategy ?? 'round-robin';
    targets = r
      ? r.targets.map((t) => ({ service: t.service, port: String(t.port), weight: String(t.weight ?? 1) }))
      : firstService
        ? [{ service: firstService.name, port: Object.keys(firstService.ports)[0] ?? '', weight: '1' }]
        : [];
    tried = false;
  });

  const ifaceObj = $derived(draft.interfaces.find((i) => i.name === iface));
  const listenerObj = $derived(ifaceObj?.listeners.find((l) => l.port === listener));
  const mode = $derived(listenerObj?.mode ?? 'https');
  const http = $derived(mode === 'https' || mode === 'http');

  // Keep the port valid when the interface changes.
  $effect(() => {
    if (ifaceObj && !ifaceObj.listeners.some((l) => l.port === listener)) {
      listener = ifaceObj.listeners[0]?.port ?? 0;
    }
  });

  $effect(() => {
    if (!nameTouched) name = slug(`${iface}${listener && listener !== 443 && listener !== 80 ? `-${listener}` : ''}-${targets[0]?.service ?? ''}`);
  });

  const taken = $derived(draft.routes.map((r) => r.name).filter((n) => n !== original?.name));
  const nameError = $derived(nameProblem(name, taken, 'route'));
  const portsOf = (service: string) => Object.keys(draft.services.find((s) => s.name === service)?.ports ?? {});
  const targetErrors = $derived(
    targets.map((t) => (!t.service ? 'Choose a service.' : !portsOf(t.service).includes(t.port) ? 'Choose a port.' : null)),
  );
  const valid = $derived(!nameError && !!ifaceObj && !!listenerObj && targets.length > 0 && targetErrors.every((e) => !e));

  const ifaceOptions = $derived(draft.interfaces.map((i) => ({ value: i.name, label: i.domains?.[0] ? `${i.domains[0]} (${i.name})` : i.name })));
  const listenerOptions = $derived(
    (ifaceObj?.listeners ?? []).map((l) => ({ value: l.port, label: `${portLabel(l.port, l.protocol)} · ${modeLabels[l.mode]}` })),
  );
  const serviceOptions = $derived(draft.services.map((s) => ({ value: s.name, label: s.name })));
  const strategyOptions = (Object.keys(strategyLabels) as Strategy[]).map((s) => ({ value: s, label: strategyLabels[s]! }));

  function save() {
    tried = true;
    if (!valid) return;
    const route: Route = {
      name,
      interface: iface,
      listener,
      hosts: mode === 'raw' ? [] : list(hosts).map((h) => h.toLowerCase()),
      path_prefix: http ? pathPrefix || '/' : '/',
      strip_prefix: http ? strip : false,
      strategy,
      targets: targets.map((t) => ({
        service: t.service,
        port: /^\d+$/.test(t.port) && !portsOf(t.service).includes(t.port) ? Number(t.port) : t.port,
        weight: Math.max(1, Number(t.weight) || 1),
      })),
    };
    edit((s) => {
      const at = s.routes.findIndex((r) => r.name === original?.name);
      if (at >= 0) s.routes[at] = route;
      else s.routes.push(route);
    });
    onsaved(name);
  }

  function remove() {
    const target = original;
    if (!target) return;
    edit((s) => {
      s.routes = s.routes.filter((r) => r.name !== target.name);
    });
    onclose();
  }
</script>

<form
  class="editor"
  aria-labelledby="route-editor-title"
  onsubmit={(e) => {
    e.preventDefault();
    save();
  }}
>
  <div>
    <h2 id="route-editor-title">{original ? 'Edit route' : 'New route'}</h2>
    {#if original}
      <p class="mono-small secondary">
        {ifaceObj?.domains?.[0] ?? iface}:{portLabel(listener, listenerObj?.protocol)} → {targets.map((t) => t.service).join(', ')}
      </p>
    {/if}
  </div>
  {#if draft.interfaces.length === 0 || draft.services.length === 0}
    <p class="secondary">
      A route needs a public interface and a service. Add {draft.interfaces.length === 0 ? 'a public interface' : 'a service'} first.
    </p>
  {:else}
    <Select label="Public interface" bind:value={iface} options={ifaceOptions} />
    <Select label="Public port" bind:value={listener} options={listenerOptions} />
    {#if mode !== 'raw'}
      <TextField
        label="Only these domains (optional)"
        bind:value={hosts}
        mono
        placeholder="Every domain of the interface"
      />
    {/if}
    {#if http}
      <div class="two">
        <TextField label="Path" bind:value={pathPrefix} mono hint="Requests under this path." />
        <div class="toggle-row">
          <Toggle label="Remove the path before forwarding" bind:checked={strip} />
          <span class="small">Remove path before forwarding</span>
        </div>
      </div>
    {/if}
    <fieldset>
      <legend class="strong">Targets</legend>
      {#each targets as t, i (i)}
        <div class="target">
          <Select label="Service" bind:value={t.service} options={serviceOptions} onchange={() => (t.port = portsOf(t.service)[0] ?? '')} />
          <Select label="Port" bind:value={t.port} options={portsOf(t.service).map((p) => ({ value: p, label: `${p} (${draft.services.find((s) => s.name === t.service)?.ports[p]})` }))} />
          {#if targets.length > 1 && strategy === 'round-robin'}
            <TextField label="Weight" bind:value={t.weight} mono inputmode="numeric" />
          {/if}
          <Button variant="ghost" size="small" onclick={() => targets.splice(i, 1)} disabled={targets.length === 1}
            >Remove<span class="visually-hidden"> target {t.service}</span></Button
          >
        </div>
        {#if tried && targetErrors[i]}<p class="small error" role="alert">{targetErrors[i]}</p>{/if}
      {/each}
      <div>
        <Button
          variant="ghost"
          size="small"
          onclick={() => {
            const s = draft.services[0];
            if (s) targets.push({ service: s.name, port: Object.keys(s.ports)[0] ?? '', weight: '1' });
          }}><Icon name="plus" /> Add target</Button
        >
      </div>
    </fieldset>
    {#if targets.length > 1}
      <Select label="Load balancing" bind:value={strategy} options={strategyOptions} />
    {/if}
    <TextField label="Route name" bind:value={name} mono oninput={() => (nameTouched = true)} error={tried ? nameError : null} />
  {/if}
  <div class="actions">
    <Button type="submit" disabled={draft.interfaces.length === 0 || draft.services.length === 0}>Save to draft</Button>
    <Button variant="ghost" onclick={onclose}>Cancel</Button>
    {#if original}
      <span class="spacer"></span>
      <Button variant="danger" onclick={remove}>Delete</Button>
    {/if}
  </div>
</form>

<style>
  .editor {
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
  fieldset {
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-sm);
    margin: 0;
    padding: 0;
    border: none;
  }
  legend {
    padding: 0;
    margin-bottom: var(--hg-spacing-sm);
  }
  .target {
    display: grid;
    grid-template-columns: 1fr 1fr auto auto;
    gap: var(--hg-spacing-sm);
    align-items: end;
  }
  .two {
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-md);
  }
  .toggle-row {
    display: flex;
    align-items: center;
    gap: var(--hg-spacing-md);
  }
  .actions {
    display: flex;
    gap: var(--hg-spacing-sm);
  }
  .spacer {
    flex: 1;
  }
  .error {
    color: var(--hg-color-status-critical-fg);
  }
</style>
