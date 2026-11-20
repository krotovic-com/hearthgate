<script lang="ts">
  import Dialog from '../ui/Dialog.svelte';
  import Button from '../ui/Button.svelte';
  import TextField from '../ui/TextField.svelte';
  import Select from '../ui/Select.svelte';
  import Icon from '../ui/Icon.svelte';
  import Alert from '../ui/Alert.svelte';
  import type { Service } from '../api';
  import { draft, edit, routesUsingService } from '../gateway.svelte';
  import { nameProblem, portProblem, slug } from './common';

  let {
    open = $bindable(false),
    original = null,
    kind: initialKind = 'external',
  }: { open?: boolean; original?: Service | null; kind?: 'external' | 'docker' } = $props();

  let kind = $state<'external' | 'docker'>('external');
  let name = $state('');
  let nameTouched = $state(false);
  let host = $state('');
  let container = $state('');
  let network = $state('');
  let ports = $state<{ name: string; port: string }[]>([]);
  let check = $state<'none' | 'tcp' | 'http'>('none');
  let path = $state('/');
  let interval = $state('10s');
  let tried = $state(false);

  $effect(() => {
    if (!open) return;
    const s = original;
    kind = s?.kind ?? initialKind;
    name = s?.name ?? '';
    nameTouched = !!s;
    host = s?.host ?? '';
    container = s?.container ?? '';
    network = s?.network ?? '';
    ports = s ? Object.entries(s.ports).map(([name, port]) => ({ name, port: String(port) })) : [{ name: 'http', port: '' }];
    check = s?.health?.type ?? 'none';
    path = s?.health?.path ?? '/';
    interval = s?.health?.interval ?? '10s';
    tried = false;
  });

  $effect(() => {
    if (!nameTouched) name = slug(kind === 'docker' ? container : host.split('.')[0] ?? '');
  });

  const taken = $derived(draft.services.map((s) => s.name).filter((n) => n !== original?.name));
  const nameError = $derived(nameProblem(name, taken, 'service'));
  const whereError = $derived(
    kind === 'docker'
      ? container.trim()
        ? null
        : 'Enter the container name.'
      : host.trim()
        ? null
        : 'Enter a host name or IP address.',
  );
  const portNameErrors = $derived(
    ports.map((p) => (/^[a-z0-9-]+$/.test(p.name) ? null : 'Use lowercase letters, digits and dashes.')),
  );
  const portErrors = $derived(ports.map((p) => portProblem(p.port)));
  const duplicate = $derived(ports.some((p, i) => ports.findIndex((o) => o.name === p.name) !== i));
  const valid = $derived(!nameError && !whereError && portErrors.every((e) => !e) && portNameErrors.every((e) => !e) && !duplicate && ports.length > 0);
  const usedBy = $derived(original ? routesUsingService(original.name).length : 0);

  function save() {
    tried = true;
    if (!valid) return;
    const service: Service = {
      name,
      kind,
      host: kind === 'external' ? host.trim() : null,
      container: kind === 'docker' ? container.trim() : null,
      network: kind === 'docker' && network.trim() ? network.trim() : null,
      ports: Object.fromEntries(ports.map((p) => [p.name, Number(p.port)])),
      health:
        check === 'none'
          ? null
          : { ...(original?.health ?? {}), type: check, path: check === 'http' ? path || '/' : undefined, interval },
    };
    if (service.health && check !== 'http') delete service.health.path;
    edit((s) => {
      const at = s.services.findIndex((x) => x.name === original?.name);
      if (at >= 0) s.services[at] = service;
      else s.services.push(service);
      if (original && original.name !== name) {
        for (const r of s.routes) for (const t of r.targets) if (t.service === original.name) t.service = name;
      }
    });
    open = false;
  }
</script>

<Dialog
  bind:open
  title={original ? `Edit ${original.name}` : kind === 'docker' ? 'Add a container' : 'Add a service'}
  description="Something Hearthgate can forward traffic to. It is not reachable from the internet until a route points at it."
>
  <Select
    label="Where it runs"
    bind:value={kind}
    options={[
      { value: 'external', label: 'A program on this machine or network' },
      { value: 'docker', label: 'A Docker container on this machine' },
    ]}
  />
  {#if kind === 'docker'}
    <TextField
      label="Container name"
      bind:value={container}
      mono
      placeholder="nextcloud"
      hint="Hearthgate finds its address and follows restarts."
      error={tried ? whereError : null}
    />
    <TextField
      label="Docker network (optional)"
      bind:value={network}
      mono
      placeholder="First network of the container"
    />
  {:else}
    <TextField
      label="Host"
      bind:value={host}
      mono
      placeholder="127.0.0.1 or nas.lan"
      error={tried ? whereError : null}
    />
  {/if}
  <TextField
    label="Name"
    bind:value={name}
    mono
    oninput={() => (nameTouched = true)}
    error={tried ? nameError : null}
  />
  <fieldset>
    <legend class="strong">Ports</legend>
    {#each ports as p, i (i)}
      <div class="port-row">
        <TextField label="Port name" bind:value={p.name} mono error={tried ? portNameErrors[i] : null} />
        <TextField label="Port" bind:value={p.port} mono inputmode="numeric" error={tried ? portErrors[i] : null} />
        <Button variant="ghost" size="small" onclick={() => ports.splice(i, 1)} disabled={ports.length === 1}
          >Remove<span class="visually-hidden"> port {p.name}</span></Button
        >
      </div>
    {/each}
    {#if tried && duplicate}<p class="small error" role="alert">Each port name can be used once.</p>{/if}
    <div>
      <Button variant="ghost" size="small" onclick={() => ports.push({ name: `port${ports.length + 1}`, port: '' })}>
        <Icon name="plus" /> Add port
      </Button>
    </div>
  </fieldset>
  <fieldset>
    <legend class="strong">Health check</legend>
    <Select
      label="Check"
      bind:value={check}
      options={[
        { value: 'none', label: 'None (only notice failed connections)' },
        { value: 'tcp', label: 'TCP: the port accepts connections' },
        { value: 'http', label: 'HTTP: a page answers with 2xx or 3xx' },
      ]}
    />
    {#if check !== 'none'}
      <div class="two">
        {#if check === 'http'}<TextField label="Path" bind:value={path} mono />{/if}
        <TextField label="Every" bind:value={interval} mono hint="For example 10s or 1m." />
      </div>
    {/if}
  </fieldset>
  {#if usedBy && original && original.name !== name}
    <Alert tone="neutral">{usedBy} route{usedBy === 1 ? '' : 's'} will follow the new name.</Alert>
  {/if}
  {#snippet footer()}
    <Button variant="secondary" onclick={() => (open = false)}>Cancel</Button>
    <Button onclick={save}>{original ? 'Save to draft' : 'Add to draft'}</Button>
  {/snippet}
</Dialog>

<style>
  fieldset {
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-md);
    margin: 0;
    padding: 0;
    border: none;
  }
  legend {
    padding: 0;
    margin-bottom: var(--hg-spacing-sm);
  }
  .port-row {
    display: grid;
    grid-template-columns: 1fr 120px auto;
    gap: var(--hg-spacing-sm);
    align-items: end;
  }
  .two {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: var(--hg-spacing-sm);
  }
  .error {
    color: var(--hg-color-status-critical-fg);
  }
</style>
