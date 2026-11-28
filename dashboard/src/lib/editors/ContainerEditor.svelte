<script lang="ts">
  import Dialog from '../ui/Dialog.svelte';
  import Button from '../ui/Button.svelte';
  import TextField from '../ui/TextField.svelte';
  import TextArea from '../ui/TextArea.svelte';
  import Select from '../ui/Select.svelte';
  import Toggle from '../ui/Toggle.svelte';
  import type { ManagedContainer, RestartPolicy, Service } from '../api';
  import { draft, edit } from '../gateway.svelte';
  import { nameProblem, portProblem, slug } from './common';
  import { deviceProblem, groupProblem, lines, parseEnv, publishProblem, showCommand, showEnv, splitCommand, volumeProblem } from '../docker';

  let { open = $bindable(false), original = null }: { open?: boolean; original?: ManagedContainer | null } = $props();

  let name = $state('');
  let nameTouched = $state(false);
  let image = $state('');
  let command = $state('');
  let envText = $state('');
  let volumesText = $state('');
  let publishText = $state('');
  let devicesText = $state('');
  let groupsText = $state('');
  let restart = $state<RestartPolicy>('unless-stopped');
  let running = $state(true);
  let addService = $state(true);
  let servicePort = $state('80');
  let tried = $state(false);

  $effect(() => {
    if (!open) return;
    const c = original;
    name = c?.name ?? '';
    nameTouched = !!c;
    image = c?.image ?? '';
    command = showCommand(c?.command);
    envText = showEnv(c?.env);
    volumesText = (c?.volumes ?? []).join('\n');
    publishText = (c?.publish ?? []).join('\n');
    devicesText = (c?.devices ?? []).join('\n');
    groupsText = (c?.groups ?? []).join('\n');
    restart = c?.restart ?? 'unless-stopped';
    running = c?.running !== false;
    addService = !c;
    servicePort = '80';
    tried = false;
  });

  // "ghcr.io/org/app:1.2" → "app".
  $effect(() => {
    if (!nameTouched) name = slug(image.split('/').pop()?.split(/[:@]/)[0] ?? '');
  });

  const taken = $derived(draft.containers.map((c) => c.name).filter((n) => n !== original?.name));
  const nameError = $derived(nameProblem(name, taken, 'container'));
  const imageError = $derived(image.trim() && !/\s/.test(image.trim()) ? null : 'Enter an image, like nginx:1.27.');
  const env = $derived(parseEnv(envText));
  const volumeError = $derived(lines(volumesText).map(volumeProblem).find(Boolean) ?? null);
  const publishError = $derived(lines(publishText).map(publishProblem).find(Boolean) ?? null);
  const deviceError = $derived(lines(devicesText).map(deviceProblem).find(Boolean) ?? null);
  const groupError = $derived(lines(groupsText).map(groupProblem).find(Boolean) ?? null);
  const serviceTaken = $derived(draft.services.some((s) => s.name === name));
  const servicePortError = $derived(addService ? portProblem(servicePort) : null);
  const valid = $derived(!nameError && !imageError && !env.problem && !volumeError && !publishError && !deviceError && !groupError && !servicePortError);

  function save() {
    tried = true;
    if (!valid) return;
    const container: ManagedContainer = {
      name,
      image: image.trim(),
      running,
      command: splitCommand(command),
      env: env.env,
      volumes: lines(volumesText),
      publish: lines(publishText),
      devices: lines(devicesText),
      groups: lines(groupsText),
      restart,
    };
    edit((s) => {
      s.containers ??= [];
      const at = s.containers.findIndex((c) => c.name === original?.name);
      if (at >= 0) s.containers[at] = container;
      else s.containers.push(container);
      if (original && original.name !== name) {
        for (const svc of s.services) if (svc.kind === 'docker' && svc.container === original.name) svc.container = name;
      }
      if (addService && !serviceTaken) {
        const service: Service = {
          name,
          kind: 'docker',
          host: null,
          container: name,
          network: 'hearthgate',
          ports: { http: Number(servicePort) },
          health: null,
        };
        s.services.push(service);
      }
    });
    open = false;
  }
</script>

<Dialog
  bind:open
  wide
  title={original ? `Edit ${original.name}` : 'Add a container'}
  description="Hearthgate pulls the image, runs the container on its own Docker network and recreates it when you change it. Routes reach it without publishing ports."
>
  <div class="grid">
    <TextField label="Image" bind:value={image} mono placeholder="traefik/whoami:v1.10" error={tried ? imageError : null} />
    <TextField
      label="Name"
      bind:value={name}
      mono
      oninput={() => (nameTouched = true)}
      hint="Its name in Docker and in the internal DNS."
      error={tried ? nameError : null}
    />
  </div>
  <TextField label="Command (optional)" bind:value={command} mono placeholder="The image's own command" />
  <TextArea
    label="Environment variables"
    bind:value={envText}
    mono
    rows={3}
    placeholder="TZ=Europe/Prague"
    hint="One KEY=value per line."
    error={tried ? env.problem : null}
  />
  <div class="grid">
    <TextArea
      label="Volumes"
      bind:value={volumesText}
      mono
      rows={3}
      placeholder="/srv/app:/data"
      hint="One per line: /host/path:/path or name:/path, :ro for read-only."
      error={tried ? volumeError : null}
    />
    <TextArea
      label="Published ports (rarely needed)"
      bind:value={publishText}
      mono
      rows={3}
      placeholder="127.0.0.1:8080:80"
      hint="One per line. Use a route instead to make it public."
      error={tried ? publishError : null}
    />
  </div>
  <div class="grid">
    <TextArea
      label="Devices"
      bind:value={devicesText}
      mono
      rows={2}
      placeholder="/dev/dri"
      hint="One per line: /dev/host[:/dev/container[:rwm]]. /dev/dri passes an Intel or AMD GPU."
      error={tried ? deviceError : null}
    />
    <TextArea
      label="Extra groups"
      bind:value={groupsText}
      mono
      rows={2}
      placeholder="993"
      hint="One per line, so the container can open devices that group owns. Use the number from getent group render on the server, since the image may not know the name."
      error={tried ? groupError : null}
    />
  </div>
  <div class="grid">
    <Select
      label="Restart"
      bind:value={restart}
      options={[
        { value: 'unless-stopped', label: 'Unless stopped' },
        { value: 'always', label: 'Always' },
        { value: 'on-failure', label: 'On failure' },
        { value: 'no', label: 'Never' },
      ]}
    />
    <div class="toggle-row">
      <Toggle label="Running" bind:checked={running} />
      <span>Running</span>
    </div>
  </div>
  {#if !original}
    <div class="toggle-row">
      <Toggle label="Also add a service for it" bind:checked={addService} />
      <span>Also add a service for it, so a route can point at it</span>
    </div>
    {#if addService}
      {#if serviceTaken}
        <p class="small secondary">A service called {name} already exists, so none is added.</p>
      {:else}
        <TextField label="Port inside the container" bind:value={servicePort} mono inputmode="numeric" error={tried ? servicePortError : null} />
      {/if}
    {/if}
  {/if}
  {#snippet footer()}
    <Button variant="secondary" onclick={() => (open = false)}>Cancel</Button>
    <Button onclick={save}>{original ? 'Save to draft' : 'Add to draft'}</Button>
  {/snippet}
</Dialog>

<style>
  .grid {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: var(--hg-spacing-md);
    align-items: start;
  }
  .toggle-row {
    display: flex;
    align-items: center;
    gap: var(--hg-spacing-md);
    min-height: 40px;
    align-self: end;
  }
</style>
